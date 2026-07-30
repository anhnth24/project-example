//! Tenant-scoped quota repositories.
//!
//! All functions require an [`OrgContext`] and are intended to run inside
//! `pool::with_org_txn`, so RLS `app.org_id` is set before any quota row is
//! read or mutated. Storage byte and document counters use a single all-time
//! period (`1970-01-01` through `9999-12-31`) because their limits are
//! cumulative. Token counters use the current UTC calendar month. Concurrent
//! jobs are a live gauge derived only from active reservations and never write
//! `usage_counters`.

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{QuotaReservation, ReservationStatus, ResourceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterPeriod {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaUsage {
    pub limit: i64,
    pub committed: i64,
    pub active_reserved: i64,
}

pub struct ReservationInsert<'a> {
    pub reservation_key: &'a str,
    pub kind: ResourceKind,
    pub amount: i64,
    pub ttl_secs: i64,
    pub job_id: Option<Uuid>,
    pub observed_at: DateTime<Utc>,
}

pub async fn fresh_clock_timestamp(txn: &Transaction<'_>) -> Result<DateTime<Utc>, DbError> {
    let row = txn.query_one("SELECT clock_timestamp()", &[]).await?;
    Ok(row.get(0))
}

pub async fn lock_admission(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
) -> Result<(), DbError> {
    let kind = kind.as_str();
    let org = ctx.org_id().to_string();
    txn.execute(
        "SELECT pg_advisory_xact_lock(hashtext('quota:' || $1 || ':' || $2))",
        &[&org, &kind],
    )
    .await?;
    Ok(())
}

pub async fn quota_limit(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
) -> Result<i64, DbError> {
    let column = match kind {
        ResourceKind::StorageBytes => "max_storage_bytes",
        ResourceKind::Documents => "max_documents::bigint",
        ResourceKind::ConcurrentJobs => "max_concurrent_jobs::bigint",
        ResourceKind::Tokens => "max_monthly_tokens",
    };
    let sql = format!("SELECT {column} FROM org_quotas WHERE org_id = $1");
    let row = txn
        .query_opt(&sql, &[&ctx.org_id()])
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(row.get(0))
}

pub async fn current_period(
    txn: &Transaction<'_>,
    kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<CounterPeriod, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_one(
            "SELECT
                CASE WHEN $1 = 'tokens'
                    THEN date_trunc('month', $2::timestamptz)
                    ELSE timestamptz '1970-01-01 00:00:00+00'
                END AS period_start,
                CASE WHEN $1 = 'tokens'
                    THEN date_trunc('month', $2::timestamptz) + interval '1 month'
                    ELSE timestamptz '9999-12-31 00:00:00+00'
                END AS period_end",
            &[&kind, &observed_at],
        )
        .await?;
    Ok(CounterPeriod {
        start: row.get("period_start"),
        end: row.get("period_end"),
    })
}

pub async fn committed_usage(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
    period: CounterPeriod,
) -> Result<i64, DbError> {
    let Some(counter_key) = kind.counter_key() else {
        return Ok(0);
    };
    let row = txn
        .query_opt(
            "SELECT value
             FROM usage_counters
             WHERE org_id = $1 AND counter_key = $2 AND period_start = $3",
            &[&ctx.org_id(), &counter_key, &period.start],
        )
        .await?;
    Ok(row.map_or(0, |row| row.get(0)))
}

pub async fn lock_committed_counter(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
    period: CounterPeriod,
) -> Result<Option<i64>, DbError> {
    let Some(counter_key) = kind.counter_key() else {
        return Ok(None);
    };
    let row = txn
        .query_opt(
            "SELECT value
             FROM usage_counters
             WHERE org_id = $1 AND counter_key = $2 AND period_start = $3
             FOR UPDATE",
            &[&ctx.org_id(), &counter_key, &period.start],
        )
        .await?;
    Ok(row.map(|row| row.get(0)))
}

pub async fn active_reserved(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<i64, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_one(
            "SELECT COALESCE(SUM(amount), 0)::bigint
             FROM quota_reservations
             WHERE org_id = $1
               AND resource_kind = $2
               AND status = 'reserved'
               AND expires_at > $3",
            &[&ctx.org_id(), &kind, &observed_at],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn usage(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<QuotaUsage, DbError> {
    let period = current_period(txn, kind, observed_at).await?;
    Ok(QuotaUsage {
        limit: quota_limit(txn, ctx, kind).await?,
        committed: committed_usage(txn, ctx, kind, period).await?,
        active_reserved: active_reserved(txn, ctx, kind, observed_at).await?,
    })
}

pub async fn find_by_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<Option<QuotaReservation>, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, reservation_key, resource_kind, amount, status,
                    expires_at, job_id, created_at, settled_at
             FROM quota_reservations
             WHERE org_id = $1 AND reservation_key = $2",
            &[&ctx.org_id(), &reservation_key],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

pub async fn find_active_matching_by_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    kind: ResourceKind,
    amount: i64,
    job_id: Option<Uuid>,
    observed_at: DateTime<Utc>,
) -> Result<Option<QuotaReservation>, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_opt(
            "SELECT id, org_id, reservation_key, resource_kind, amount, status,
                    expires_at, job_id, created_at, settled_at
             FROM quota_reservations
             WHERE org_id = $1
               AND reservation_key = $2
               AND resource_kind = $3
               AND amount = $4
               AND job_id IS NOT DISTINCT FROM $5
               AND status = 'reserved'
               AND expires_at > $6",
            &[
                &ctx.org_id(),
                &reservation_key,
                &kind,
                &amount,
                &job_id,
                &observed_at,
            ],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

pub async fn kind_by_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<ResourceKind, DbError> {
    let row = txn
        .query_opt(
            "SELECT resource_kind
             FROM quota_reservations
             WHERE org_id = $1 AND reservation_key = $2",
            &[&ctx.org_id(), &reservation_key],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    let resource_kind: String = row.get(0);
    ResourceKind::parse(&resource_kind).map_err(DbError::Config)
}

pub async fn insert_reserved(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    spec: ReservationInsert<'_>,
) -> Result<Option<QuotaReservation>, DbError> {
    let kind = spec.kind.as_str();
    let row = txn
        .query_opt(
            "INSERT INTO quota_reservations (
                org_id, reservation_key, resource_kind, amount, expires_at, job_id
             )
             VALUES ($1, $2, $3, $4, $7::timestamptz + ($5::bigint * interval '1 second'), $6)
             ON CONFLICT (org_id, reservation_key) DO NOTHING
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[
                &ctx.org_id(),
                &spec.reservation_key,
                &kind,
                &spec.amount,
                &spec.ttl_secs,
                &spec.job_id,
                &spec.observed_at,
            ],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

pub async fn finalize_reserved_by_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<QuotaReservation>, DbError> {
    let row = txn
        .query_opt(
            "UPDATE quota_reservations
             SET status = 'finalized', settled_at = $3
             WHERE org_id = $1
               AND reservation_key = $2
               AND status = 'reserved'
               AND expires_at > $3
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[&ctx.org_id(), &reservation_key, &observed_at],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

/// Finalize a reservation with the *measured* amount (token settlement).
///
/// The reservation was admitted with an estimate; the committed counter must
/// record actual usage. The single UPDATE keeps the terminal-state idempotency
/// contract identical to [`finalize_reserved_by_key`] — a reservation that is
/// no longer `reserved` returns `None` and the caller inspects its status.
pub async fn finalize_reserved_by_key_with_amount(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    actual_amount: i64,
    observed_at: DateTime<Utc>,
) -> Result<Option<QuotaReservation>, DbError> {
    let row = txn
        .query_opt(
            "UPDATE quota_reservations
             SET status = 'finalized', amount = $4, settled_at = $3
             WHERE org_id = $1
               AND reservation_key = $2
               AND status = 'reserved'
               AND expires_at > $3
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[
                &ctx.org_id(),
                &reservation_key,
                &observed_at,
                &actual_amount,
            ],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

/// Refund every active reservation of `kind` attached to `job_id` (slot release).
///
/// Used by the job terminal transitions (complete/fail/cancel/reclaim) so a
/// concurrent-jobs slot frees exactly when the job stops running. Refund only
/// ever *increases* available capacity, so this intentionally does not take the
/// admission advisory lock (taking it after the job-row locks those callers
/// already hold could deadlock against claim, which locks admission first).
pub async fn refund_reserved_by_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<u64, DbError> {
    let kind = kind.as_str();
    let updated = txn
        .execute(
            "UPDATE quota_reservations
             SET status = 'refunded', settled_at = $4
             WHERE org_id = $1
               AND job_id = $2
               AND resource_kind = $3
               AND status = 'reserved'",
            &[&ctx.org_id(), &job_id, &kind, &observed_at],
        )
        .await?;
    Ok(updated)
}

/// Extend the expiry of active reservations tied to `job_id` (heartbeat).
///
/// SKIP LOCKED: a terminal transition (promotion/complete/fail) refunds this
/// row inside its own transaction and may hold the row lock until commit.
/// The heartbeat must never queue behind that — its call timeout is a
/// fraction of the lease, and a blocked heartbeat turns a successful commit
/// into a spurious reconciliation. A skipped row needs no extension anyway:
/// whoever holds it is settling the reservation.
pub async fn extend_reserved_by_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    kind: ResourceKind,
    ttl_secs: i64,
) -> Result<u64, DbError> {
    let kind = kind.as_str();
    let updated = txn
        .execute(
            "UPDATE quota_reservations
             SET expires_at = clock_timestamp() + ($4::bigint * interval '1 second')
             WHERE id IN (
                 SELECT id FROM quota_reservations
                 WHERE org_id = $1
                   AND job_id = $2
                   AND resource_kind = $3
                   AND status = 'reserved'
                 FOR UPDATE SKIP LOCKED
             )",
            &[&ctx.org_id(), &job_id, &kind, &ttl_secs],
        )
        .await?;
    Ok(updated)
}

/// Refund `concurrent_jobs` reservations whose job is gone or no longer leased.
///
/// Crash between a job's terminal transition and its slot refund leaves the
/// reservation `reserved` until TTL expiry; reconcile releases those slots
/// immediately instead of waiting out the TTL.
pub async fn refund_orphaned_job_slots(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    observed_at: DateTime<Utc>,
) -> Result<u64, DbError> {
    let updated = txn
        .execute(
            "UPDATE quota_reservations qr
             SET status = 'refunded', settled_at = $2
             WHERE qr.org_id = $1
               AND qr.resource_kind = 'concurrent_jobs'
               AND qr.status = 'reserved'
               AND (
                    qr.job_id IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM jobs j
                        WHERE j.org_id = qr.org_id
                          AND j.id = qr.job_id
                          AND j.status = 'leased'
                    )
               )",
            &[&ctx.org_id(), &observed_at],
        )
        .await?;
    Ok(updated)
}

/// Ground truth for the storage counter: bytes of live source versions plus
/// live derived artifacts (documents not soft-deleted/purged).
pub async fn actual_storage_bytes(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT
                COALESCE((
                    SELECT SUM(dv.byte_size)::bigint
                    FROM document_versions dv
                    JOIN documents d ON d.org_id = dv.org_id AND d.id = dv.document_id
                    WHERE dv.org_id = $1
                      AND d.deleted_at IS NULL
                      AND d.state <> 'purged'
                ), 0)
              + COALESCE((
                    SELECT SUM(da.byte_size)::bigint
                    FROM derived_artifacts da
                    JOIN documents d ON d.org_id = da.org_id AND d.id = da.document_id
                    WHERE da.org_id = $1
                      AND d.deleted_at IS NULL
                      AND d.state <> 'purged'
                ), 0)",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(row.get(0))
}

/// Ground truth for the documents counter: live (not soft-deleted/purged) documents.
pub async fn actual_document_count(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT COUNT(*)::bigint
             FROM documents
             WHERE org_id = $1
               AND deleted_at IS NULL
               AND state <> 'purged'",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn refund_reserved_by_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<QuotaReservation>, DbError> {
    let row = txn
        .query_opt(
            "UPDATE quota_reservations
             SET status = 'refunded', settled_at = $3
             WHERE org_id = $1
               AND reservation_key = $2
               AND status = 'reserved'
               AND expires_at > $3
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[&ctx.org_id(), &reservation_key, &observed_at],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

pub async fn expire_reserved_by_key_if_due(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<QuotaReservation>, DbError> {
    let row = txn
        .query_opt(
            "UPDATE quota_reservations
             SET status = 'expired', settled_at = $3
             WHERE org_id = $1
               AND reservation_key = $2
               AND status = 'reserved'
               AND expires_at <= $3
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[&ctx.org_id(), &reservation_key, &observed_at],
        )
        .await?;
    row.map(|row| map_reservation(&row)).transpose()
}

pub async fn get_by_key_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<QuotaReservation, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, reservation_key, resource_kind, amount, status,
                    expires_at, job_id, created_at, settled_at
             FROM quota_reservations
             WHERE org_id = $1 AND reservation_key = $2
             FOR UPDATE",
            &[&ctx.org_id(), &reservation_key],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_reservation(&row)
}

pub async fn set_status(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_id: Uuid,
    status: ReservationStatus,
) -> Result<QuotaReservation, DbError> {
    let status = status.as_str();
    let row = txn
        .query_one(
            "UPDATE quota_reservations
             SET status = $3, settled_at = clock_timestamp()
             WHERE org_id = $1 AND id = $2
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[&ctx.org_id(), &reservation_id, &status],
        )
        .await?;
    map_reservation(&row)
}

pub async fn upsert_counter_value(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
    period: CounterPeriod,
    value: i64,
) -> Result<(), DbError> {
    let Some(counter_key) = kind.counter_key() else {
        return Ok(());
    };
    txn.execute(
        "INSERT INTO usage_counters (org_id, counter_key, period_start, period_end, value)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (org_id, counter_key, period_start)
         DO UPDATE SET value = EXCLUDED.value,
                       period_end = EXCLUDED.period_end,
                       updated_at = clock_timestamp()",
        &[
            &ctx.org_id(),
            &counter_key,
            &period.start,
            &period.end,
            &value,
        ],
    )
    .await?;
    Ok(())
}

pub async fn expire_reserved_batch(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    batch_size: i64,
    observed_at: DateTime<Utc>,
) -> Result<u64, DbError> {
    let updated = txn
        .execute(
            "WITH expired AS (
                SELECT id
                FROM quota_reservations
                WHERE org_id = $1 AND status = 'reserved' AND expires_at <= $3
                ORDER BY expires_at, id
                LIMIT $2
                FOR UPDATE SKIP LOCKED
             )
             UPDATE quota_reservations qr
             SET status = 'expired', settled_at = $3
             FROM expired
             WHERE qr.org_id = $1 AND qr.id = expired.id",
            &[&ctx.org_id(), &batch_size, &observed_at],
        )
        .await?;
    Ok(updated)
}

pub async fn list_org_ids_for_sweep(pool: &deadpool_postgres::Pool) -> Result<Vec<Uuid>, DbError> {
    let client = pool.get().await?;
    let rows = client.query("SELECT id FROM orgs ORDER BY id", &[]).await?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

pub(crate) fn map_reservation(row: &Row) -> Result<QuotaReservation, DbError> {
    let resource_kind: String = row.get("resource_kind");
    let status: String = row.get("status");
    Ok(QuotaReservation {
        id: row.get("id"),
        org_id: row.get("org_id"),
        reservation_key: row.get("reservation_key"),
        resource_kind: ResourceKind::parse(&resource_kind).map_err(DbError::Config)?,
        amount: row.get("amount"),
        status: ReservationStatus::parse(&status).map_err(DbError::Config)?,
        expires_at: row.get("expires_at"),
        job_id: row.get("job_id"),
        created_at: row.get("created_at"),
        settled_at: row.get("settled_at"),
    })
}
