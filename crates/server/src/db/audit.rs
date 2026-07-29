//! Append-oriented audit log reads (P1B-O01 / 1C-11). Writes go through
//! auth::session / services::audit — this module is read-only.

use chrono::{DateTime, Utc};
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

/// Optional exact-match / bound filters for [`list_page`]. `None` means
/// "no constraint on this field" — never a wildcard footgun since every
/// predicate below is parameterized (`$n IS NULL OR ...`).
#[derive(Debug, Clone, Default)]
pub struct AuditListFilter {
    /// Exact `action` string match (e.g. `"member.role_change"`).
    pub action: Option<String>,
    /// Exact `actor_user_id` match.
    pub actor_user_id: Option<Uuid>,
    /// Inclusive lower bound on `created_at`.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive upper bound on `created_at`.
    pub to: Option<DateTime<Utc>>,
}

/// Cursor-paginated, filtered audit rows for the tenant (1C-11 read endpoint).
///
/// Ordering is `(created_at DESC, id DESC)` — the same stable
/// newest-first tuple cursor convention as `db::documents::list_in_collection`.
/// `org_id = $1` plus RLS (`audit_log_org_isolation`, migration `0010`) both
/// enforce tenant isolation; this predicate is defense-in-depth, not the only
/// gate.
pub async fn list_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    filter: &AuditListFilter,
    limit: i64,
    after_created_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<AuditLogEntry>, DbError> {
    // Callers pass `Pagination::from_query(..).limit + 1` (max 100 + 1 = 101)
    // to detect `has_more` without an extra COUNT query — clamp to 101, not
    // 100, or that sentinel row would get silently clamped away right at the
    // page-size boundary and `has_more` would go stale exactly at limit=100.
    let limit = limit.clamp(1, 101);
    let rows = txn
        .query(
            "SELECT id, org_id, seq, actor_user_id, action, resource_type, resource_id,
                    outcome, metadata, request_id, created_at
             FROM audit_log
             WHERE org_id = $1
               AND ($2::text IS NULL OR action = $2)
               AND ($3::uuid IS NULL OR actor_user_id = $3)
               AND ($4::timestamptz IS NULL OR created_at >= $4)
               AND ($5::timestamptz IS NULL OR created_at <= $5)
               AND (
                    $6::timestamptz IS NULL
                    OR (created_at, id) < ($6::timestamptz, $7::uuid)
               )
             ORDER BY created_at DESC, id DESC
             LIMIT $8",
            &[
                &ctx.org_id(),
                &filter.action,
                &filter.actor_user_id,
                &filter.from,
                &filter.to,
                &after_created_at,
                &after_id,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_entry).collect()
}

fn map_entry(row: &Row) -> Result<AuditLogEntry, DbError> {
    let outcome = AuditOutcome::parse(row.get::<_, &str>("outcome")).map_err(DbError::Config)?;
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
