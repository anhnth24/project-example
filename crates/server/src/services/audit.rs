//! Safe, append-only business audit helpers (P1B-O01).
//!
//! Audit rows store action/deny metadata only — never document content, prompts,
//! tokens, passwords, or signed URLs. Mutation paths must write audit in the same
//! DB transaction as the business change (or fail the mutation).

use deadpool_postgres::Pool;
use serde_json::{json, Value as JsonValue};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::models::AuditOutcome;
use crate::db::pool::with_org_txn;
use crate::telemetry::{redacted_fields, AuditEvent as TelemetryAuditShape};

const FORBIDDEN_METADATA_KEYS: &[&str] = &[
    "password",
    "token",
    "refresh_token",
    "access_token",
    "authorization",
    "prompt",
    "document_content",
    "markdown",
    "signed_url",
    "object_key",
];

/// Bundled fields for a durable audit write.
#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    pub request_id: &'a str,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: Option<&'a str>,
    pub outcome: AuditOutcome,
    pub metadata: JsonValue,
}

impl AuditOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Deny => "deny",
            Self::Error => "error",
        }
    }
}

/// Sanitizes metadata: drops forbidden keys and redacts sensitive fragments.
pub fn sanitize_metadata(metadata: JsonValue) -> JsonValue {
    let JsonValue::Object(map) = metadata else {
        return json!({});
    };
    let mut safe = serde_json::Map::new();
    for (key, value) in map {
        let lowered = key.to_ascii_lowercase();
        if FORBIDDEN_METADATA_KEYS
            .iter()
            .any(|forbidden| lowered.contains(forbidden))
        {
            continue;
        }
        match value {
            JsonValue::String(text) => {
                let mut fields = std::collections::BTreeMap::new();
                fields.insert(key.clone(), text);
                let redacted = redacted_fields(&fields);
                safe.insert(
                    key,
                    JsonValue::String(redacted.values().next().cloned().unwrap_or_default()),
                );
            }
            JsonValue::Number(_) | JsonValue::Bool(_) | JsonValue::Null => {
                safe.insert(key, value);
            }
            other => {
                let _ = other;
            }
        }
    }
    JsonValue::Object(safe)
}

#[cfg(any(test, feature = "test-hooks"))]
static INJECT_AUDIT_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Arms a one-shot failure for the next [`record_in_txn`] (test-hooks / unit tests only).
#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_injected_audit_failure() {
    INJECT_AUDIT_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Write an audit row inside an existing org transaction (mutation co-commit).
pub async fn record_in_txn(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    record: AuditRecord<'_>,
) -> Result<(), DbError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if INJECT_AUDIT_FAILURE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(DbError::Config("injected_audit_failure".into()));
    }
    let metadata = sanitize_metadata(record.metadata);
    write_audit(
        txn,
        AuditEvent {
            org_id: ctx.org_id(),
            actor_user_id: Some(ctx.user_id()),
            action: record.action,
            resource_type: record.resource_type,
            resource_id: record.resource_id,
            outcome: record.outcome.as_str(),
            metadata,
            request_id: record.request_id,
        },
    )
    .await
}

/// Durable deny audit for permission/authorization failures (must not be swallowed).
pub async fn record_deny(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    action: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    reason: &str,
) -> Result<(), DbError> {
    record(
        pool,
        ctx,
        AuditRecord {
            request_id,
            action,
            resource_type,
            resource_id,
            outcome: AuditOutcome::Deny,
            metadata: json!({ "reason": reason }),
        },
    )
    .await
}

pub async fn record(pool: &Pool, ctx: &OrgContext, record: AuditRecord<'_>) -> Result<(), DbError> {
    let metadata = sanitize_metadata(record.metadata);
    let action = record.action.to_string();
    let resource_type = record.resource_type.to_string();
    let resource_id = record.resource_id.map(str::to_string);
    let outcome = record.outcome.as_str().to_string();
    let request_id = record.request_id.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: &action,
                        resource_type: &resource_type,
                        resource_id: resource_id.as_deref(),
                        outcome: &outcome,
                        metadata,
                        request_id: &request_id,
                    },
                )
                .await
            })
        }
    })
    .await
}

#[derive(Debug, Clone)]
pub struct TelemetryAuditInput<'a> {
    pub org_id: Uuid,
    pub actor_id: Uuid,
    pub request_id: &'a str,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: &'a str,
    pub outcome: AuditOutcome,
    pub metadata: &'a [(String, String)],
}

/// Converts a durable audit row into the telemetry envelope (no secrets).
pub fn to_telemetry_envelope(input: TelemetryAuditInput<'_>) -> TelemetryAuditShape {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in input.metadata {
        map.insert(key.clone(), value.clone());
    }
    TelemetryAuditShape {
        version: 1,
        occurred_at: chrono::Utc::now().to_rfc3339(),
        request_id: input.request_id.into(),
        org_id: input.org_id.to_string(),
        actor_id: input.actor_id.to_string(),
        action: input.action.into(),
        target_type: input.resource_type.into(),
        target_id: input.resource_id.into(),
        outcome: input.outcome.as_str().into(),
        metadata: redacted_fields(&map),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_secrets_and_content() {
        let cleaned = sanitize_metadata(json!({
            "reason": "ok",
            "password": "secret",
            "prompt": "leak",
            "object_key": "trusted/abc",
            "attempts": 2
        }));
        assert_eq!(cleaned["reason"], "ok");
        assert_eq!(cleaned["attempts"], 2);
        assert!(cleaned.get("password").is_none());
        assert!(cleaned.get("prompt").is_none());
        assert!(cleaned.get("object_key").is_none());
    }

    #[test]
    fn typed_outcomes_are_stable_wire_values() {
        assert_eq!(AuditOutcome::Success.as_str(), "success");
        assert_eq!(AuditOutcome::Deny.as_str(), "deny");
        assert_eq!(AuditOutcome::Error.as_str(), "error");
    }

    #[test]
    fn deny_and_success_records_preserve_request_id_for_correlation() {
        let request_id = "11111111-1111-1111-1111-111111111111";
        let deny = AuditRecord {
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: Some("doc-1"),
            outcome: AuditOutcome::Deny,
            metadata: json!({ "reason": "permission_denied" }),
        };
        let success = AuditRecord {
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: Some("doc-1"),
            outcome: AuditOutcome::Success,
            metadata: json!({}),
        };
        assert_eq!(deny.request_id, success.request_id);
        assert_eq!(deny.outcome, AuditOutcome::Deny);
        let envelope = to_telemetry_envelope(TelemetryAuditInput {
            org_id: Uuid::nil(),
            actor_id: Uuid::nil(),
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: "doc-1",
            outcome: AuditOutcome::Deny,
            metadata: &[],
        });
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.outcome, "deny");
    }
}
