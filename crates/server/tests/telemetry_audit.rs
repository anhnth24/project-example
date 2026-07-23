//! P1B-O01 live evidence: correlation, append-only audit, canary redaction, app-role.
//!
//! Gated on `MARKHAND_TEST_DATABASE_URL` + `MARKHAND_TEST_APP_DATABASE_URL`.

mod common;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool,
    seed_user_with_permissions,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::models::AuditOutcome;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self, JobPayload};
use fileconv_server::services::audit::{self, AuditRecord};
use fileconv_server::telemetry::{
    apply_to_job_payload, contains_canary, from_job_payload, CorrelationContext, MetricsRegistry,
    WorkerIds, CANARY_FRAGMENTS,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn live_o01_audit_append_only_correlation_and_canary() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let email = format!("o01-{}@example.test", user.simple());
    seed_user_with_permissions(
        &pool,
        org,
        user,
        &email,
        "markhand-dev-password",
        &["doc.upload", "doc.delete", "qa.query"],
    )
    .await;
    let ctx = OrgContext::try_new(org, user, ["doc.upload", "doc.delete", "qa.query"], []).unwrap();
    let request_id = Uuid::new_v4().to_string();
    let resource_id = Uuid::nil().to_string();

    for (outcome, action) in [
        (AuditOutcome::Success, "document.upload"),
        (AuditOutcome::Deny, "document.delete"),
        (AuditOutcome::Error, "collection.update"),
        (AuditOutcome::Success, "search.query"),
    ] {
        audit::record(
            &pool,
            &ctx,
            AuditRecord {
                request_id: &request_id,
                action,
                resource_type: "document",
                resource_id: Some(&resource_id),
                outcome,
                metadata: json!({ "reason": "system" }),
            },
        )
        .await
        .expect("audit write");
    }

    // Intent outcome exact roundtrip via typed helper + durable row.
    audit::record(
        &pool,
        &ctx,
        AuditRecord {
            request_id: &request_id,
            action: "vector.cleanup_intent",
            resource_type: "document",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Intent,
            metadata: json!({ "document_id": resource_id, "phase": "intent" }),
        },
    )
    .await
    .expect("intent audit write");
    // Also keep raw SQL intent for document.purge_objects compatibility.
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        let resource_id = resource_id.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO audit_log (
                        org_id, actor_user_id, action, resource_type, resource_id,
                        outcome, metadata, request_id
                     ) VALUES ($1, $2, 'document.purge_objects', 'object', $3, 'intent', '{}'::jsonb, $4)",
                    &[&ctx.org_id(), &ctx.user_id(), &resource_id, &request_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("intent audit row");

    let count: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM audit_log
                         WHERE org_id = $1 AND request_id = $2",
                        &[&ctx.org_id(), &request_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("count audits");
    assert!(count >= 5, "expected correlated audits, got {count}");

    // Append-only: UPDATE/DELETE must fail under app role.
    let update_err = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE audit_log SET outcome = 'deny'
                     WHERE org_id = $1 AND request_id = $2",
                    &[&ctx.org_id(), &request_id],
                )
                .await
                .map_err(|error| fileconv_server::db::error::DbError::Config(error.to_string()))
            })
        }
    })
    .await;
    assert!(
        update_err.is_err(),
        "audit_log UPDATE must be rejected (append-only)"
    );
    let delete_err = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM audit_log WHERE org_id = $1 AND request_id = $2",
                    &[&ctx.org_id(), &request_id],
                )
                .await
                .map_err(|error| fileconv_server::db::error::DbError::Config(error.to_string()))
            })
        }
    })
    .await;
    assert!(
        delete_err.is_err(),
        "audit_log DELETE must be rejected (append-only)"
    );

    // Negative: free-text reason / nested / forbidden keys must fail closed.
    let denied = audit::record(
        &pool,
        &ctx,
        AuditRecord {
            request_id: &request_id,
            action: "document.upload",
            resource_type: "document",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: json!({
                "password": "CANARY_SECRET_TOKEN",
                "prompt": "CANARY_PROMPT_TEXT",
                "reason": "safe"
            }),
        },
    )
    .await;
    assert!(denied.is_err(), "non-allowlisted/canary metadata must fail");

    audit::record(
        &pool,
        &ctx,
        AuditRecord {
            request_id: &request_id,
            action: "document.upload",
            resource_type: "document",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: json!({ "reason": "upload_accepted" }),
        },
    )
    .await
    .expect("allowlisted metadata must write");
    let meta: String = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT metadata::text FROM audit_log
                         WHERE org_id = $1 AND request_id = $2 AND metadata ? 'reason'
                         ORDER BY created_at DESC LIMIT 1",
                        &[&ctx.org_id(), &request_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("read sanitized metadata");
    assert!(meta.contains("upload_accepted"));
    assert!(!contains_canary(&meta));
    assert!(!meta.contains("password"));
    assert!(!meta.contains("prompt"));

    // Bootstrap grants: app cannot UPDATE/DELETE/TRUNCATE audit or disable trigger.
    let truncate_err = with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.batch_execute("TRUNCATE audit_log")
                    .await
                    .map_err(|error| fileconv_server::db::error::DbError::Config(error.to_string()))
            })
        }
    })
    .await;
    assert!(
        truncate_err.is_err(),
        "TRUNCATE audit_log must fail for app role"
    );
    let disable_err = with_org_txn(&pool, &ctx, {
        move |txn| {
            Box::pin(async move {
                txn.batch_execute(
                    "ALTER TABLE audit_log DISABLE TRIGGER trg_audit_log__immutability",
                )
                .await
                .map_err(|error| fileconv_server::db::error::DbError::Config(error.to_string()))
            })
        }
    })
    .await;
    assert!(
        disable_err.is_err(),
        "app role must not be able to disable audit immutability trigger"
    );

    // Async job payload carries request_id + traceparent (round-trip encode/decode).
    let corr = CorrelationContext::new(request_id.clone());
    let mut payload = JobPayload {
        document_id: Some(Uuid::new_v4()),
        version_id: Some(Uuid::new_v4()),
        ..JobPayload::default()
    };
    apply_to_job_payload(&mut payload, &corr);
    let encoded = payload.to_json().expect("payload json");
    assert!(encoded.get("request_id").is_some() || encoded.get("requestId").is_some());
    assert!(encoded.get("traceparent").is_some());
    let decoded = jobs::decode_job_payload(jobs::CURRENT_JOB_PAYLOAD_VERSION, encoded)
        .expect("decode correlated payload");
    let restored = from_job_payload(
        Uuid::new_v4(),
        &decoded,
        WorkerIds {
            org_id: Some(org),
            actor_id: Some(user),
            index_signature: None,
        },
    );
    assert_eq!(restored.request_id, request_id);
    assert_eq!(restored.trace_id, corr.trace_id);
    assert!(restored.traceparent.is_some());

    // Metrics / export records must not contain canaries or tenant labels.
    MetricsRegistry::reset_for_tests();
    fileconv_server::telemetry::record_http_request(
        "api.v1.health.live",
        "2xx",
        std::time::Duration::from_millis(5),
    );
    fileconv_server::telemetry::emit_span(
        "http.request",
        &request_id,
        &corr.trace_id,
        "server",
        "2xx",
        std::time::Duration::from_millis(5),
    );
    let body = MetricsRegistry::render_prometheus();
    for canary in CANARY_FRAGMENTS {
        assert!(
            !body.contains(canary),
            "metrics leaked canary {canary}: {body}"
        );
    }
    assert!(!body.contains("org_id="));
    assert!(!body.contains("document_id="));
    assert!(!contains_canary(&body));
    let drained = MetricsRegistry::drain_export_for_tests(8);
    let export_json = serde_json::to_string(
        &drained
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "requestId": r.request_id,
                    "traceId": r.trace_id,
                    "outcome": r.outcome,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!contains_canary(&export_json));
    assert!(export_json.contains(&request_id));

    // Free strings (name/slug/filename/content_type) must never persist.
    let long_name = "N".repeat(200);
    let rejected_name = audit::record(
        &pool,
        &ctx,
        AuditRecord {
            request_id: &request_id,
            action: "collection.create",
            resource_type: "collection",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: json!({ "name": long_name, "slug": "canary-slug" }),
        },
    )
    .await;
    assert!(
        rejected_name.is_err(),
        "name/slug free strings must fail audit"
    );
    audit::record(
        &pool,
        &ctx,
        AuditRecord {
            request_id: &request_id,
            action: "collection.create",
            resource_type: "collection",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: json!({
                "collection_id": resource_id,
                "name_chars": 200,
                "slug_chars": 12
            }),
        },
    )
    .await
    .expect("200-char collection name must not fail audit when only counts are stored");

    // Authenticated deny/error route matrix keyed by internal request id.
    let matrix_rid = Uuid::new_v4().to_string();
    for (action, resource) in [
        ("ask.query", "ask"),
        ("search.query", "search"),
        ("document.upload", "document"),
        ("document.reindex", "document"),
    ] {
        audit::record_deny(
            &pool,
            &ctx,
            &matrix_rid,
            action,
            resource,
            None,
            "permission_denied",
        )
        .await
        .unwrap_or_else(|error| panic!("deny audit {action}: {error}"));
        audit::record(
            &pool,
            &ctx,
            AuditRecord {
                request_id: &matrix_rid,
                action,
                resource_type: resource,
                resource_id: None,
                outcome: AuditOutcome::Error,
                metadata: json!({ "reason": "validation_failed" }),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("error audit {action}: {error}"));
    }
    let matrix_count: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let matrix_rid = matrix_rid.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM audit_log
                         WHERE org_id = $1 AND request_id = $2
                           AND action = ANY($3)
                           AND outcome IN ('deny', 'error')",
                        &[
                            &ctx.org_id(),
                            &matrix_rid,
                            &vec![
                                "ask.query".to_string(),
                                "search.query".to_string(),
                                "document.upload".to_string(),
                                "document.reindex".to_string(),
                            ],
                        ],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("matrix count");
    assert_eq!(
        matrix_count, 8,
        "expected 4 deny + 4 error rows by internal request id, got {matrix_count}"
    );

    ephemeral.drop().await;
}
