//! Append-oriented audit log reads (P1B-O01). Writes go through auth::session.

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{AuditLogEntry, AuditOutcome};

/// Lists recent audit rows for the tenant (bounded).
pub async fn list_recent(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
) -> Result<Vec<AuditLogEntry>, DbError> {
    let limit = limit.clamp(1, 200);
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
    rows.iter().map(map_entry).collect()
}

fn map_entry(row: &Row) -> Result<AuditLogEntry, DbError> {
    let outcome = match row.get::<_, &str>("outcome") {
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

/// Inserts a download capability redemption marker (single-use JTI).
pub async fn insert_download_redemption(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    jti: Uuid,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    let row = txn
        .query_opt(
            "INSERT INTO download_capability_redemptions (org_id, jti, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (org_id, jti) DO NOTHING
             RETURNING jti",
            &[&ctx.org_id(), &jti, &expires_at],
        )
        .await?;
    Ok(row.is_some())
}
