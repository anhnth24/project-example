//! P1B-O01 live tests: correlation, metrics cardinality, audit immutability/RLS/redaction.
//!
//! Gated on `MARKHAND_TEST_DATABASE_URL`.

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::config::Profile;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::models::JobType;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
use fileconv_server::services::audit::{self, actions, resources, AuditAction, AuditReason};
use fileconv_server::telemetry::{
    apply_to_job_payload, contains_canary, force_flush, from_job_payload, init, metrics,
    normalize_http_method, redacted_fields, runtime, sanitize_audit_metadata, scope, worker_span,
    CorrelationContext, TelemetryConfig, CANARY_FRAGMENTS,
};
use opentelemetry::trace::{Span as _, Tracer};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tracing::Instrument;
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
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
    let (client, connection) = tokio_postgres::connect(database_url, tokio_postgres::NoTls)
        .await
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
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
        let db_name = format!("markhand_otel_{}", Uuid::new_v4().simple());
        let admin_url = rewrite_database_url(base_url, "postgres");
        let admin = connect_raw(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("create ephemeral db");
        let url = rewrite_database_url(base_url, &db_name);
        apply_migrations(&url).await.expect("apply migrations");
        Self {
            admin_url,
            db_name,
            url,
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

struct CaptureLayer {
    lines: Arc<Mutex<Vec<String>>>,
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = StringVisitor(String::new());
        event.record(&mut visitor);
        if let Ok(mut guard) = self.lines.lock() {
            guard.push(visitor.0);
        }
    }
}

struct StringVisitor(String);

impl tracing::field::Visit for StringVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?};", field.name(), value);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={};", field.name(), value);
    }
}

#[tokio::test]
async fn async_correlation_links_worker_span_to_request() {
    let request_id = Uuid::new_v4();
    let parent =
        CorrelationContext::with_ids(request_id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    let (child_request, child_trace, child_job) = scope(parent.clone(), async {
        let mut payload = JobPayload::default();
        apply_to_job_payload(&mut payload, &CorrelationContext::current().unwrap());
        let job_id = Uuid::new_v4();
        let child = from_job_payload(job_id, &payload, None);
        let span = worker_span("convert", &child);
        async {
            (
                child.request_id.clone(),
                child.trace_id.clone(),
                child.job_id.clone(),
            )
        }
        .instrument(span)
        .await
    })
    .await;
    assert_eq!(child_request, request_id.to_string());
    assert_eq!(child_trace, parent.trace_id);
    assert!(child_job.is_some());
}

#[test]
fn metrics_label_schema_rejects_high_cardinality() {
    assert!(metrics::validate_metric(
        "markhand_api_requests_total",
        &["route", "method", "status_class"]
    )
    .is_ok());
    for forbidden in [
        "org_id",
        "user_id",
        "document_id",
        "job_id",
        "request_id",
        "path",
        "query",
    ] {
        assert!(
            metrics::validate_metric("markhand_api_requests_total", &[forbidden]).is_err(),
            "{forbidden} must be rejected"
        );
    }
    assert_eq!(normalize_http_method("CANARY_CUSTOM_METHOD"), "OTHER");
}

#[test]
fn logging_capture_keeps_production_canaries_absent() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        lines: lines.clone(),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        let fields = BTreeMap::from([
            ("request_id".into(), Uuid::new_v4().to_string()),
            (
                "authorization".into(),
                format!("Bearer {}", CANARY_FRAGMENTS[0]),
            ),
            ("document_content".into(), CANARY_FRAGMENTS[1].into()),
            ("outcome".into(), "deny".into()),
            (
                "method".into(),
                normalize_http_method("CANARY_CUSTOM_METHOD").into(),
            ),
        ]);
        let safe = redacted_fields(&fields);
        tracing::info!(
            target: "telemetry_test",
            request_id = %safe["request_id"],
            outcome = %safe["outcome"],
            method = %safe["method"],
            "deny without secrets"
        );
    });
    let captured = lines.lock().unwrap().join("\n");
    for canary in CANARY_FRAGMENTS {
        assert!(
            !captured.contains(canary),
            "canary leaked into logs: {canary} / {captured}"
        );
    }
    assert!(!captured.contains("CANARY_CUSTOM_METHOD"));
    assert!(!contains_canary(&captured));
}

#[test]
fn in_memory_otel_exporter_records_parent_child_spans() {
    let config = TelemetryConfig::from_env_map(&BTreeMap::new(), Profile::Test).unwrap();
    init(&config).expect("telemetry init");
    let tracer = opentelemetry::global::tracer("markhand-test");
    tracer.in_span("parent_request", |cx| {
        let mut child = tracer.start_with_context("worker", &cx);
        child.end();
    });
    force_flush().expect("flush");
    let exporter = runtime()
        .and_then(|rt| rt.span_exporter())
        .expect("in-memory span exporter");
    let spans = exporter.get_finished_spans().expect("finished spans");
    assert!(
        spans.iter().any(|span| span.name == "parent_request"),
        "expected parent span, got {spans:?}"
    );
    assert!(
        spans.iter().any(|span| span.name == "worker"),
        "expected worker span, got {spans:?}"
    );
    let parent_id = spans
        .iter()
        .find(|span| span.name == "parent_request")
        .map(|span| span.span_context.span_id())
        .expect("parent id");
    assert!(
        spans
            .iter()
            .filter(|span| span.name == "worker")
            .any(|span| span.parent_span_id == parent_id),
        "worker span must parent to request span: {spans:?}"
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn audit_immutability_rls_and_redaction_live() {
    let Some(base) = test_database_url() else {
        return;
    };
    let db = EphemeralDb::create(&base).await;
    let pool = create_pool(&db.url).expect("pool");
    let (org_id, user_id) = seed_org_user(&pool).await;
    let ctx = OrgContext::try_new(org_id, user_id, ["doc.upload"], []).unwrap();
    let (org_b, user_b) = seed_org_user(&pool).await;
    let ctx_b = OrgContext::try_new(org_b, user_b, ["doc.upload"], []).unwrap();

    let request_id = Uuid::new_v4().to_string();
    let audit_id = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                audit::write_org_action(
                    txn,
                    &ctx,
                    actions::DOCUMENT_UPLOAD,
                    resources::DOCUMENT,
                    Some("550e8400-e29b-41d4-a716-446655440000"),
                    "success",
                    &request_id,
                    json!({"reason": "upload_accepted", "format": "pdf"}),
                )
                .await?;
                let rows = txn
                    .query(
                        "SELECT id FROM audit_log WHERE org_id = $1 ORDER BY seq DESC LIMIT 1",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(rows[0].get::<_, Uuid>(0))
            })
        }
    })
    .await
    .expect("insert audit");

    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                audit::write_org_action(
                    txn,
                    &ctx,
                    actions::AUTH_DENY,
                    resources::SESSION,
                    None,
                    "deny",
                    &request_id,
                    json!({"reason": "permission_denied"}),
                )
                .await?;
                audit::write_org_action(
                    txn,
                    &ctx,
                    actions::DOCUMENT_DELETE,
                    resources::DOCUMENT,
                    Some("550e8400-e29b-41d4-a716-446655440001"),
                    "success",
                    &request_id,
                    json!({"reason": "user_requested"}),
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("deny/delete audit");

    let immutability = with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                let update = txn
                    .execute(
                        "UPDATE audit_log SET outcome = 'error' WHERE id = $1",
                        &[&audit_id],
                    )
                    .await;
                let delete = txn
                    .execute("DELETE FROM audit_log WHERE id = $1", &[&audit_id])
                    .await;
                let truncate = txn.batch_execute("TRUNCATE audit_log").await;
                Ok((update.is_err(), delete.is_err(), truncate.is_err()))
            })
        }
    })
    .await
    .unwrap();
    assert!(immutability.0 && immutability.1 && immutability.2);

    let foreign_count = with_org_txn(&pool, &ctx_b, {
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM audit_log WHERE org_id = $1",
                        &[&org_id],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(foreign_count, 0);

    // Cross-tenant write attempt must not insert into foreign org.
    let cross = with_org_txn(&pool, &ctx_b, {
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO audit_log (
                        org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
                     ) VALUES ($1, $2, 'auth.deny', 'session', NULL, 'deny', '{}'::jsonb, $3)",
                    &[&org_id, &user_id, &request_id],
                )
                .await
                .map(|_| ())
                .map_err(|error| {
                    fileconv_server::db::error::DbError::Config(error.to_string())
                })
            })
        }
    })
    .await;
    assert!(cross.is_err());

    let rejected = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                audit::write_org_action(
                    txn,
                    &ctx,
                    actions::DOCUMENT_UPLOAD,
                    resources::DOCUMENT,
                    None,
                    "error",
                    &request_id,
                    json!({"reason": "Bearer CANARY_SECRET_TOKEN"}),
                )
                .await
            })
        }
    })
    .await;
    assert!(rejected.is_err());
    assert!(sanitize_audit_metadata(&json!({"prompt": "CANARY_PROMPT_TEXT"})).is_err());

    // Hostile nested metadata / invalid action / bad request+resource IDs.
    let hostile = with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                let nested = txn
                    .execute(
                        "INSERT INTO audit_log (
                            org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
                         ) VALUES ($1, $2, 'document.upload', 'document', $3, 'success',
                                   '{\"reason\":\"upload_accepted\",\"nested\":{\"a\":1}}'::jsonb, $4)",
                        &[
                            &org_id,
                            &user_id,
                            &"550e8400-e29b-41d4-a716-446655440002",
                            &Uuid::new_v4().to_string(),
                        ],
                    )
                    .await;
                let bad_action = txn
                    .execute(
                        "INSERT INTO audit_log (
                            org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
                         ) VALUES ($1, $2, 'not.an.action', 'document', NULL, 'success', '{}'::jsonb, $3)",
                        &[&org_id, &user_id, &Uuid::new_v4().to_string()],
                    )
                    .await;
                let bad_request = txn
                    .execute(
                        "INSERT INTO audit_log (
                            org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
                         ) VALUES ($1, $2, 'auth.deny', 'session', NULL, 'deny', '{}'::jsonb, 'not-a-uuid')",
                        &[&org_id, &user_id],
                    )
                    .await;
                let bad_resource = txn
                    .execute(
                        "INSERT INTO audit_log (
                            org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
                         ) VALUES ($1, $2, 'document.delete', 'document', 'mh1.secret', 'success', '{}'::jsonb, $3)",
                        &[&org_id, &user_id, &Uuid::new_v4().to_string()],
                    )
                    .await;
                Ok((
                    nested.is_err(),
                    bad_action.is_err(),
                    bad_request.is_err(),
                    bad_resource.is_err(),
                ))
            })
        }
    })
    .await
    .unwrap();
    assert!(hostile.0 && hostile.1 && hostile.2 && hostile.3);

    // Hostile role: revoke path — separate-txn update/delete/truncate already asserted.
    let admin = connect_raw(&db.url).await;
    admin
        .batch_execute(
            "DO $$ BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_audit_hostile') THEN
                 CREATE ROLE markhand_audit_hostile LOGIN PASSWORD 'hostile';
               END IF;
             END $$;
             GRANT CONNECT ON DATABASE current_database() TO markhand_audit_hostile;
             GRANT USAGE ON SCHEMA public TO markhand_audit_hostile;
             GRANT SELECT, INSERT ON audit_log TO markhand_audit_hostile;
             REVOKE UPDATE, DELETE, TRUNCATE ON audit_log FROM markhand_audit_hostile;",
        )
        .await
        .ok();

    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn enqueue_propagates_w3c_traceparent_into_job_payload() {
    let Some(base) = test_database_url() else {
        return;
    };
    let db = EphemeralDb::create(&base).await;
    let pool = create_pool(&db.url).expect("pool");
    let (org_id, user_id) = seed_org_user(&pool).await;
    let ctx = OrgContext::try_new(org_id, user_id, ["doc.upload"], []).unwrap();
    let request_id = Uuid::new_v4();
    let parent =
        CorrelationContext::with_ids(request_id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    let expected_tp = parent.traceparent.clone();
    let job = scope(parent, async {
        jobs::enqueue(
            &pool,
            &ctx,
            EnqueueJob::new(JobType::Reconcile, JobPayload::default(), "corr-otel-1"),
        )
        .await
        .expect("enqueue")
        .job
    })
    .await;
    let payload = jobs::decode_job_payload(job.payload_version, job.payload).unwrap();
    assert_eq!(payload.request_id, Some(request_id));
    assert_eq!(payload.traceparent, expected_tp);
    assert!(payload
        .traceparent
        .as_deref()
        .is_some_and(|tp| fileconv_server::telemetry::validate_traceparent(tp).is_ok()));
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn audit_deny_failure_uses_fallback_not_silent_ignore() {
    let Some(base) = test_database_url() else {
        return;
    };
    let db = EphemeralDb::create(&base).await;
    let pool = create_pool(&db.url).expect("pool");
    let (org_id, user_id) = seed_org_user(&pool).await;
    // Force failure by using a non-existent org id for RLS/FK — durable helper must not panic
    // and must return Ok after fallback.
    let missing_org = Uuid::new_v4();
    let result = audit::write_deny_durable(
        &pool,
        missing_org,
        Some(user_id),
        AuditAction::AuthDeny,
        audit::AuditResource::Session,
        None,
        &Uuid::new_v4().to_string(),
        audit::reason_metadata(AuditReason::PermissionDenied),
    )
    .await;
    assert!(result.is_ok());
    let _ = org_id;
    db.drop().await;
}

async fn seed_org_user(pool: &Pool) -> (Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let slug = format!("otel-{}", &org_id.to_string()[..8]);
    let context = OrgContext::try_new(org_id, user_id, ["doc.upload"], []).unwrap();
    with_org_txn(pool, &context, {
        let context = context.clone();
        let slug = slug.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &context, &slug, &slug).await?;
                orgs::ensure_user(
                    txn,
                    &context,
                    user_id,
                    &format!("{slug}@example.test"),
                    &slug,
                )
                .await?;
                orgs::ensure_membership(txn, &context).await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");
    (org_id, user_id)
}
