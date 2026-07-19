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
) -> Result<CounterPeriod, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_one(
            "SELECT
                CASE WHEN $1 = 'tokens'
                    THEN date_trunc('month', now())
                    ELSE timestamptz '1970-01-01 00:00:00+00'
                END AS period_start,
                CASE WHEN $1 = 'tokens'
                    THEN date_trunc('month', now()) + interval '1 month'
                    ELSE timestamptz '9999-12-31 00:00:00+00'
                END AS period_end",
            &[&kind],
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
) -> Result<i64, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_one(
            "SELECT COALESCE(SUM(amount), 0)::bigint
             FROM quota_reservations
             WHERE org_id = $1
               AND resource_kind = $2
               AND status = 'reserved'
               AND expires_at > now()",
            &[&ctx.org_id(), &kind],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn usage(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    kind: ResourceKind,
) -> Result<QuotaUsage, DbError> {
    let period = current_period(txn, kind).await?;
    Ok(QuotaUsage {
        limit: quota_limit(txn, ctx, kind).await?,
        committed: committed_usage(txn, ctx, kind, period).await?,
        active_reserved: active_reserved(txn, ctx, kind).await?,
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

pub async fn insert_reserved(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    kind: ResourceKind,
    amount: i64,
    ttl_secs: i64,
    job_id: Option<Uuid>,
) -> Result<Option<QuotaReservation>, DbError> {
    let kind = kind.as_str();
    let row = txn
        .query_opt(
            "INSERT INTO quota_reservations (
                org_id, reservation_key, resource_kind, amount, expires_at, job_id
             )
             VALUES ($1, $2, $3, $4, now() + ($5::bigint * interval '1 second'), $6)
             ON CONFLICT (org_id, reservation_key) DO NOTHING
             RETURNING id, org_id, reservation_key, resource_kind, amount, status,
                       expires_at, job_id, created_at, settled_at",
            &[
                &ctx.org_id(),
                &reservation_key,
                &kind,
                &amount,
                &ttl_secs,
                &job_id,
            ],
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
             SET status = $3, settled_at = now()
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
                       updated_at = now()",
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
) -> Result<u64, DbError> {
    let updated = txn
        .execute(
            "WITH expired AS (
                SELECT id
                FROM quota_reservations
                WHERE org_id = $1 AND status = 'reserved' AND expires_at <= now()
                ORDER BY expires_at, id
                LIMIT $2
                FOR UPDATE SKIP LOCKED
             )
             UPDATE quota_reservations qr
             SET status = 'expired', settled_at = now()
             FROM expired
             WHERE qr.org_id = $1 AND qr.id = expired.id",
            &[&ctx.org_id(), &batch_size],
        )
        .await?;
    Ok(updated)
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
