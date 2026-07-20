//! Live readiness integration tests.
//!
//! These tests self-skip unless PostgreSQL, Qdrant, and MinIO test endpoints are
//! provided, because `/health/ready` verifies all dependencies.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use deadpool_postgres::Pool;
use fileconv_server::api::ApiError;
use fileconv_server::config::{RuntimeEndpoints, SecretString, ServerConfig};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::create_pool;
use fileconv_server::db::readiness_fence;
use fileconv_server::http::{router, AppState};
use fileconv_server::state::RuntimeState;
use http_body_util::BodyExt;
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

fn test_qdrant_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_QDRANT_URL unset");
            None
        }
    }
}

fn test_minio_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ENDPOINT unset");
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
        let db_name = format!("markhand_ready_{}", Uuid::new_v4().simple());
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
    qdrant_url: String,
    minio_url: String,
}

impl LiveEnv {
    async fn boot() -> Option<Self> {
        let base_url = test_database_url()?;
        let qdrant_url = test_qdrant_url()?;
        let minio_url = test_minio_url()?;
        let db = EphemeralDb::create(&base_url).await;
        apply_migrations(&db.url).await.expect("apply migrations");
        let pool = create_pool(&db.url).expect("pool");
        Some(Self {
            db,
            pool,
            qdrant_url,
            minio_url,
        })
    }

    fn app(&self) -> axum::Router {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new(self.db.url.clone()),
                qdrant_url: self.qdrant_url.clone(),
                minio_url: self.minio_url.clone(),
            }))
            .expect("runtime");
        router(AppState::from_parts(runtime, self.pool.clone(), None).expect("app state"))
    }
}

async fn get_ready(app: axum::Router) -> (StatusCode, Vec<u8>) {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn readiness_fence_gates_ready_probe_live() {
    let Some(env) = LiveEnv::boot().await else {
        return;
    };

    readiness_fence::set_reconciling(&env.pool, Some("restore in progress"))
        .await
        .expect("set reconciling");
    let (status, body) = get_ready(env.app()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let error: ApiError = serde_json::from_slice(&body).unwrap();
    assert_eq!(error.code, "not_reconciled");

    readiness_fence::set_ready(&env.pool, None)
        .await
        .expect("set ready");
    let (status, _) = get_ready(env.app()).await;
    assert_eq!(status, StatusCode::OK);

    env.db.drop().await;
}
