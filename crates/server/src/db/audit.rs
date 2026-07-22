//! Append-only audit_log repository (tenant-scoped).

use serde_json::Value as JsonValue;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{AuditLogEntry, AuditOutcome};

const ALLOWED_ACTIONS: &[&str] = &[
    "auth.login",
    "auth.deny",
    "auth.logout",
    "auth.refresh",
    "auth.refresh.reuse",
    "auth.revoke_all",
    "document.upload",
    "document.delete",
    "document.tombstone",
    "document.publish",
    "document.reindex",
    "document.purge",
    "document.purge_objects",
    "job.enqueue",
    "quota.deny",
    "reconcile.repair",
    "vector.cleanup_intent",
    "object.cleanup",
];

const ALLOWED_RESOURCES: &[&str] = &["session", "document", "job", "quota", "object"];

/// Insert fields for one append-only audit row.
pub struct NewAuditEvent<'a> {
    pub actor_user_id: Option<Uuid>,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: Option<&'a str>,
    pub outcome: &'a str,
    pub request_id: &'a str,
    pub metadata: JsonValue,
}

pub async fn append(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    event: NewAuditEvent<'_>,
) -> Result<Uuid, DbError> {
    validate_event(&event)?;
    let row = txn
        .query_one(
            "INSERT INTO audit_log (
                org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
            &[
                &ctx.org_id(),
                &event.actor_user_id,
                &event.action,
                &event.resource_type,
                &event.resource_id,
                &event.outcome,
                &event.metadata,
                &event.request_id,
            ],
        )
        .await?;
    Ok(row.get(0))
}

/// Compatibility insert used by session code that already holds org_id on the event.
pub async fn append_raw(
    txn: &Transaction<'_>,
    org_id: Uuid,
    event: NewAuditEvent<'_>,
) -> Result<Uuid, DbError> {
    validate_event(&event)?;
    let row = txn
        .query_one(
            "INSERT INTO audit_log (
                org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
            &[
                &org_id,
                &event.actor_user_id,
                &event.action,
                &event.resource_type,
                &event.resource_id,
                &event.outcome,
                &event.metadata,
                &event.request_id,
            ],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn list_for_org(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
) -> Result<Vec<AuditLogEntry>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, seq, actor_user_id, action, resource_type, resource_id,
                    outcome, metadata, request_id, created_at
             FROM audit_log
             WHERE org_id = $1
             ORDER BY seq DESC
             LIMIT $2",
            &[&ctx.org_id(), &limit],
        )
        .await?;
    rows.iter().map(map_row).collect()
}

fn map_row(row: &tokio_postgres::Row) -> Result<AuditLogEntry, DbError> {
    let outcome_raw: String = row.get("outcome");
    let outcome = match outcome_raw.as_str() {
        "success" => AuditOutcome::Success,
        "deny" => AuditOutcome::Deny,
        "error" => AuditOutcome::Error,
        other => return Err(DbError::Config(format!("unknown audit outcome: {other}"))),
    };
    Ok(AuditLogEntry {
        id: row.get("id"),
        org_id: row.get("org_id"),
        seq: row.get("seq"),
        actor_user_id: row.get("actor_user_id"),
        action: row.get("action"),
        resource_type: row.get("resource_type"),
        resource_id: row.get("resource_id"),
        outcome,
        metadata: row.get("metadata"),
        request_id: row.get("request_id"),
        created_at: row.get("created_at"),
    })
}

fn validate_event(event: &NewAuditEvent<'_>) -> Result<(), DbError> {
    if !ALLOWED_ACTIONS.contains(&event.action) {
        return Err(DbError::Config("audit_action_invalid".into()));
    }
    if !ALLOWED_RESOURCES.contains(&event.resource_type) {
        return Err(DbError::Config("audit_resource_type_invalid".into()));
    }
    if !matches!(event.outcome, "success" | "deny" | "error") {
        return Err(DbError::Config("audit_outcome_invalid".into()));
    }
    if Uuid::parse_str(event.request_id).is_err() {
        return Err(DbError::Config("audit_request_id_must_be_uuid".into()));
    }
    if let Some(resource_id) = event.resource_id {
        if Uuid::parse_str(resource_id).is_err()
            || resource_id.contains("mh1.")
            || resource_id.contains("Bearer ")
            || resource_id.starts_with("eyJ")
        {
            return Err(DbError::Config("audit_resource_id_invalid".into()));
        }
    }
    if matches!(event.metadata, JsonValue::Object(_)) {
        Ok(())
    } else {
        Err(DbError::Config("audit_metadata_must_be_object".into()))
    }
}
