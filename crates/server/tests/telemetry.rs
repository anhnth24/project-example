//! Live telemetry/audit tests. They self-skip unless `MARKHAND_TEST_DATABASE_URL` is set.

use axum::body::Body;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::{RuntimeEndpoints, SecretString, ServerConfig};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::services::audit::{record_audit_event, SafeAuditEvent};
use fileconv_server::state::RuntimeState;
use http_body_util::BodyExt;
use serde_json::json;
use tokio_postgres::NoTls;
use tower::ServiceExt;
use uuid::Uuid;

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
            None
        }
    }
}

fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
    let (without_query, query) = match base_url.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (base_url, None),
    };
    let prefix = without_query
        .rsplit_once('/')
        .map(|(head, _)| head)
        .expect("database URL must include a path");
    match query {
        Some(tail) => format!("{prefix}/{database_name}?{tail}"),
        None => format!("{prefix}/{database_name}"),
    }
}

async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("database connection failed: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_telemetry_{}", Uuid::new_v4().simple());
        let admin_url = rewrite_database_url(base_url, "postgres");
        let admin = connect_raw(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        Self {
            admin_url,
            db_name: db_name.clone(),
            url: rewrite_database_url(base_url, &db_name),
        }
    }

    async fn drop(self) {
        let admin = connect_raw(&self.admin_url).await;
        let _ = admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = '{}' AND pid <> pg_backend_pid(); \
                 DROP DATABASE IF EXISTS \"{}\"",
                self.db_name, self.db_name
            ))
            .await;
    }
}

struct LiveEnv {
    db: EphemeralDb,
    pool: Pool,
    app: axum::Router,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new(&db.url),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .expect("runtime");
        let app = router(AppState::from_parts(runtime, pool.clone(), None).expect("app state"));
        Some(Self { db, pool, app })
    }

    async fn shutdown(self) {
        self.db.drop().await;
    }
}

async fn seed_context(pool: &Pool, permission: bool) -> OrgContext {
    let org_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let permissions = if permission {
        vec!["qa.query"]
    } else {
        Vec::new()
    };
    let ctx = OrgContext::try_new(org_id, user_id, permissions, [collection_id]).unwrap();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(
                    txn,
                    &ctx,
                    &format!("telemetry-{}", org_id.simple()),
                    "Telemetry",
                )
                .await?;
                orgs::ensure_user(
                    txn,
                    &ctx,
                    user_id,
                    &format!("telemetry-{}@example.test", user_id.simple()),
                    "Telemetry",
                )
                .await?;
                orgs::ensure_membership(txn, &ctx).await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed context");
    ctx
}

async fn audit_rows(pool: &Pool, ctx: &OrgContext) -> Vec<(String, String, serde_json::Value)> {
    let org_id = ctx.org_id();
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT action, outcome, metadata FROM audit_log WHERE org_id = $1 ORDER BY seq",
                    &[&org_id],
                )
                .await?;
            Ok(rows
                .into_iter()
                .map(|row| (row.get(0), row.get(1), row.get(2)))
                .collect())
        })
    })
    .await
    .expect("audit rows")
}

#[tokio::test]
async fn live_audit_redacts_metadata_and_metrics_reflect_requests() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };

    let allowed = seed_context(&env.pool, true).await;
    record_audit_event(
        &env.pool,
        &allowed,
        SafeAuditEvent {
            action: "qa.query",
            resource_type: "qa",
            resource_id: None,
            outcome: "success",
            request_id: "req-live-telemetry-1".into(),
            metadata: json!({
                "endpoint": "search",
                "documentContent": "CANARY_DOCUMENT_CONTENT",
                "token": "CANARY_TOKEN"
            }),
        },
    )
    .await
    .expect("allowed audit");

    let denied = seed_context(&env.pool, false).await;
    record_audit_event(
        &env.pool,
        &denied,
        SafeAuditEvent {
            action: "qa.query",
            resource_type: "qa",
            resource_id: None,
            outcome: "deny",
            request_id: "req-live-telemetry-2".into(),
            metadata: json!({ "reason": "permission_denied" }),
        },
    )
    .await
    .expect("deny audit");

    let response = env
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let metrics = env
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let metrics = String::from_utf8(metrics.to_vec()).unwrap();
    assert!(metrics.contains("markhand_http_requests_total"));
    assert!(metrics.contains(r#"route="/api/v1/health/live""#));
    assert!(!metrics.contains(allowed.org_id().to_string().as_str()));
    assert!(!metrics.contains("CANARY_TOKEN"));

    let rows = audit_rows(&env.pool, &allowed).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "qa.query");
    assert_eq!(rows[0].1, "success");
    let rendered = rows[0].2.to_string();
    assert!(!rendered.contains("CANARY_DOCUMENT_CONTENT"));
    assert!(!rendered.contains("CANARY_TOKEN"));
    assert!(rendered.contains("[REDACTED]"));

    let rows = audit_rows(&env.pool, &denied).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "deny");
    assert_eq!(rows[0].2["reason"], "permission_denied");

    env.shutdown().await;
}
