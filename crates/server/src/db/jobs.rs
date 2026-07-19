//! Tenant-scoped durable job, outbox, and event-log repositories.
//!
//! Callers should run these functions inside [`crate::db::pool::with_org_txn`]
//! so RLS `app.org_id` is set before any row is visible or mutable.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{EventLogEntry, Job, JobStatus, JobType, OutboxEvent};

const JOB_COLUMNS: &str = "id, org_id, job_type, status, payload_version, payload, attempts, \
    max_attempts, lease_owner, lease_expires_at, heartbeat_at, checkpoint, idempotency_key, \
    document_id, version_id, available_at, started_at, finished_at, last_error, created_at, \
    updated_at";

const JOB_COLUMNS_J: &str = "j.id, j.org_id, j.job_type, j.status, j.payload_version, j.payload, \
    j.attempts, j.max_attempts, j.lease_owner, j.lease_expires_at, j.heartbeat_at, j.checkpoint, \
    j.idempotency_key, j.document_id, j.version_id, j.available_at, j.started_at, j.finished_at, \
    j.last_error, j.created_at, j.updated_at";

const OUTBOX_COLUMNS: &str = "id, org_id, event_type, payload_version, payload, idempotency_key, \
    job_id, published_at, created_at";

const EVENT_COLUMNS: &str = "id, org_id, sequence_no, event_type, payload_version, payload, \
    job_id, document_id, version_id, created_at";

#[derive(Debug, Clone)]
pub struct NewJob<'a> {
    pub id: Uuid,
    pub job_type: JobType,
    pub payload_version: i32,
    pub payload: &'a JsonValue,
    pub max_attempts: i32,
    pub idempotency_key: &'a str,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub available_at: DateTime<Utc>,
    pub outbox_event_type: &'a str,
    pub outbox_payload_version: i32,
    pub outbox_payload: &'a JsonValue,
    pub outbox_idempotency_key: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewOutboxEvent<'a> {
    pub event_type: &'a str,
    pub payload_version: i32,
    pub payload: &'a JsonValue,
    pub idempotency_key: &'a str,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewEventLogEntry<'a> {
    pub event_type: &'a str,
    pub payload_version: i32,
    pub payload: &'a JsonValue,
    pub job_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
}

pub async fn fresh_clock_timestamp(txn: &Transaction<'_>) -> Result<DateTime<Utc>, DbError> {
    let row = txn.query_one("SELECT clock_timestamp()", &[]).await?;
    Ok(row.get(0))
}

pub async fn insert_job_with_outbox(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewJob<'_>,
) -> Result<(Job, bool), DbError> {
    let job_type = input.job_type.as_str();
    let row = txn
        .query_opt(
            &format!(
                "INSERT INTO jobs (
                    id, org_id, job_type, payload_version, payload, max_attempts,
                    idempotency_key, document_id, version_id, available_at
                 )
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (org_id, job_type, idempotency_key) DO NOTHING
                 RETURNING {JOB_COLUMNS}"
            ),
            &[
                &input.id,
                &ctx.org_id(),
                &job_type,
                &input.payload_version,
                &input.payload,
                &input.max_attempts,
                &input.idempotency_key,
                &input.document_id,
                &input.version_id,
                &input.available_at,
            ],
        )
        .await?;

    if let Some(row) = row {
        let job = map_job(&row)?;
        let outbox = insert_outbox_event(
            txn,
            ctx,
            NewOutboxEvent {
                event_type: input.outbox_event_type,
                payload_version: input.outbox_payload_version,
                payload: input.outbox_payload,
                idempotency_key: input.outbox_idempotency_key,
                job_id: Some(job.id),
            },
        )
        .await?;
        if outbox.job_id != Some(job.id) {
            return Err(DbError::Config(
                "outbox idempotency key conflicts with another job".into(),
            ));
        }
        return Ok((job, true));
    }

    let existing = find_by_idempotency_key(txn, ctx, input.job_type, input.idempotency_key)
        .await?
        .ok_or(DbError::NotFound)?;
    Ok((existing, false))
}

pub async fn find_by_idempotency_key(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_type: JobType,
    idempotency_key: &str,
) -> Result<Option<Job>, DbError> {
    let job_type = job_type.as_str();
    let row = txn
        .query_opt(
            &format!(
                "SELECT {JOB_COLUMNS}
                 FROM jobs
                 WHERE org_id = $1 AND job_type = $2 AND idempotency_key = $3"
            ),
            &[&ctx.org_id(), &job_type, &idempotency_key],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn get_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {JOB_COLUMNS}
                 FROM jobs
                 WHERE org_id = $1 AND id = $2"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn get_by_id_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {JOB_COLUMNS}
                 FROM jobs
                 WHERE org_id = $1 AND id = $2
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn claim_pending(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    lease_owner: &str,
    limit: i64,
    lease_ttl_secs: i64,
) -> Result<Vec<Job>, DbError> {
    let rows = txn
        .query(
            &format!(
                "WITH candidates AS (
                    SELECT id
                    FROM jobs
                    WHERE org_id = $1
                      AND status = 'pending'
                      AND available_at <= clock_timestamp()
                    ORDER BY available_at, created_at, id
                    FOR UPDATE SKIP LOCKED
                    LIMIT $2
                 )
                 UPDATE jobs j
                 SET status = 'leased',
                     lease_owner = $3,
                     lease_expires_at = clock_timestamp() + ($4::bigint * interval '1 second'),
                     heartbeat_at = clock_timestamp(),
                     started_at = COALESCE(started_at, clock_timestamp()),
                     attempts = attempts + 1,
                     updated_at = clock_timestamp()
                 FROM candidates
                 WHERE j.org_id = $1 AND j.id = candidates.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[&ctx.org_id(), &limit, &lease_owner, &lease_ttl_secs],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn heartbeat(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
    lease_ttl_secs: i64,
) -> Result<bool, DbError> {
    let updated = txn
        .execute(
            "UPDATE jobs
             SET heartbeat_at = clock_timestamp(),
                 lease_expires_at = clock_timestamp() + ($4::bigint * interval '1 second'),
                 updated_at = clock_timestamp()
             WHERE org_id = $1
               AND id = $2
               AND lease_owner = $3
               AND status = 'leased'",
            &[&ctx.org_id(), &job_id, &lease_owner, &lease_ttl_secs],
        )
        .await?;
    Ok(updated == 1)
}

pub async fn save_checkpoint(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
    checkpoint: &JsonValue,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE jobs
                 SET checkpoint = $4, updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND lease_owner = $3
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id, &lease_owner, &checkpoint],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn reclaim_expired(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
    backoff_secs: i64,
) -> Result<Vec<Job>, DbError> {
    let rows = txn
        .query(
            &format!(
                "WITH expired AS (
                    SELECT id
                    FROM jobs
                    WHERE org_id = $1
                      AND status = 'leased'
                      AND lease_expires_at < clock_timestamp()
                    ORDER BY lease_expires_at, id
                    FOR UPDATE SKIP LOCKED
                    LIMIT $2
                 )
                 UPDATE jobs j
                 SET status = CASE
                         WHEN j.attempts < j.max_attempts THEN 'pending'
                         ELSE 'dead_letter'
                     END,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     available_at = CASE
                         WHEN j.attempts < j.max_attempts
                             THEN clock_timestamp() + ($3::bigint * interval '1 second')
                         ELSE j.available_at
                     END,
                     finished_at = CASE
                         WHEN j.attempts < j.max_attempts THEN NULL
                         ELSE clock_timestamp()
                     END,
                     last_error = CASE
                         WHEN j.attempts < j.max_attempts THEN j.last_error
                         ELSE COALESCE(j.last_error, 'lease expired after max attempts')
                     END,
                     updated_at = clock_timestamp()
                 FROM expired
                 WHERE j.org_id = $1 AND j.id = expired.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[&ctx.org_id(), &limit, &backoff_secs],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn complete_owned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE jobs
                 SET status = 'succeeded',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     finished_at = clock_timestamp(),
                     updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND lease_owner = $3
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id, &lease_owner],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn fail_owned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
    last_error: &str,
    backoff_secs: i64,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE jobs
                 SET status = CASE
                         WHEN attempts < max_attempts THEN 'pending'
                         ELSE 'dead_letter'
                     END,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     available_at = CASE
                         WHEN attempts < max_attempts
                             THEN clock_timestamp() + ($5::bigint * interval '1 second')
                         ELSE available_at
                     END,
                     finished_at = CASE
                         WHEN attempts < max_attempts THEN NULL
                         ELSE clock_timestamp()
                     END,
                     last_error = $4,
                     updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND lease_owner = $3
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &job_id,
                &lease_owner,
                &last_error,
                &backoff_secs,
            ],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn cancel_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<Job>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE jobs
                 SET status = 'cancelled',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     finished_at = clock_timestamp(),
                     updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND status IN ('pending', 'leased')
                 RETURNING {JOB_COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn insert_outbox_event(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewOutboxEvent<'_>,
) -> Result<OutboxEvent, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "INSERT INTO outbox_events (
                    org_id, event_type, payload_version, payload, idempotency_key, job_id
                 )
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (org_id, event_type, idempotency_key) DO NOTHING
                 RETURNING {OUTBOX_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &input.event_type,
                &input.payload_version,
                &input.payload,
                &input.idempotency_key,
                &input.job_id,
            ],
        )
        .await?;
    if let Some(row) = row {
        return map_outbox_event(&row);
    }
    let row = txn
        .query_one(
            &format!(
                "SELECT {OUTBOX_COLUMNS}
                 FROM outbox_events
                 WHERE org_id = $1 AND event_type = $2 AND idempotency_key = $3"
            ),
            &[&ctx.org_id(), &input.event_type, &input.idempotency_key],
        )
        .await?;
    map_outbox_event(&row)
}

pub async fn append_event_log(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewEventLogEntry<'_>,
) -> Result<EventLogEntry, DbError> {
    lock_event_log_sequence(txn, ctx).await?;
    let sequence_no = next_event_sequence(txn, ctx).await?;
    let row = txn
        .query_one(
            &format!(
                "INSERT INTO event_log (
                    org_id, sequence_no, event_type, payload_version, payload,
                    job_id, document_id, version_id
                 )
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 RETURNING {EVENT_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &sequence_no,
                &input.event_type,
                &input.payload_version,
                &input.payload,
                &input.job_id,
                &input.document_id,
                &input.version_id,
            ],
        )
        .await?;
    map_event_log_entry(&row)
}

pub async fn append_event_and_outbox(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    event: NewEventLogEntry<'_>,
    outbox_idempotency_key: &str,
) -> Result<(EventLogEntry, OutboxEvent), DbError> {
    let entry = append_event_log(txn, ctx, event.clone()).await?;
    let outbox = insert_outbox_event(
        txn,
        ctx,
        NewOutboxEvent {
            event_type: event.event_type,
            payload_version: event.payload_version,
            payload: event.payload,
            idempotency_key: outbox_idempotency_key,
            job_id: event.job_id,
        },
    )
    .await?;
    Ok((entry, outbox))
}

pub async fn claim_unpublished_outbox(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
) -> Result<Vec<OutboxEvent>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {OUTBOX_COLUMNS}
                 FROM outbox_events
                 WHERE org_id = $1 AND published_at IS NULL
                 ORDER BY created_at, id
                 FOR UPDATE SKIP LOCKED
                 LIMIT $2"
            ),
            &[&ctx.org_id(), &limit],
        )
        .await?;
    rows.iter().map(map_outbox_event).collect()
}

pub async fn mark_outbox_published(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    outbox_id: Uuid,
) -> Result<Option<OutboxEvent>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE outbox_events
                 SET published_at = clock_timestamp()
                 WHERE org_id = $1 AND id = $2 AND published_at IS NULL
                 RETURNING {OUTBOX_COLUMNS}"
            ),
            &[&ctx.org_id(), &outbox_id],
        )
        .await?;
    row.map(|row| map_outbox_event(&row)).transpose()
}

pub async fn event_log_sequences(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
) -> Result<Vec<i64>, DbError> {
    let rows = txn
        .query(
            "SELECT sequence_no
             FROM event_log
             WHERE org_id = $1
             ORDER BY sequence_no",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

async fn lock_event_log_sequence(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<(), DbError> {
    let org = ctx.org_id().to_string();
    txn.execute(
        "SELECT pg_advisory_xact_lock(hashtext('eventlog:' || $1))",
        &[&org],
    )
    .await?;
    Ok(())
}

async fn next_event_sequence(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT COALESCE(MAX(sequence_no), 0)::bigint + 1
             FROM event_log
             WHERE org_id = $1",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(row.get(0))
}

pub(crate) fn map_job(row: &Row) -> Result<Job, DbError> {
    let job_type: String = row.get("job_type");
    let status: String = row.get("status");
    Ok(Job {
        id: row.get("id"),
        org_id: row.get("org_id"),
        job_type: JobType::parse(&job_type).map_err(DbError::Config)?,
        status: JobStatus::parse(&status).map_err(DbError::Config)?,
        payload_version: row.get("payload_version"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
        lease_owner: row.get("lease_owner"),
        lease_expires_at: row.get("lease_expires_at"),
        heartbeat_at: row.get("heartbeat_at"),
        checkpoint: row.get("checkpoint"),
        idempotency_key: row.get("idempotency_key"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        available_at: row.get("available_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(crate) fn map_outbox_event(row: &Row) -> Result<OutboxEvent, DbError> {
    Ok(OutboxEvent {
        id: row.get("id"),
        org_id: row.get("org_id"),
        event_type: row.get("event_type"),
        payload_version: row.get("payload_version"),
        payload: row.get("payload"),
        idempotency_key: row.get("idempotency_key"),
        job_id: row.get("job_id"),
        published_at: row.get("published_at"),
        created_at: row.get("created_at"),
    })
}

pub(crate) fn map_event_log_entry(row: &Row) -> Result<EventLogEntry, DbError> {
    Ok(EventLogEntry {
        id: row.get("id"),
        org_id: row.get("org_id"),
        sequence_no: row.get("sequence_no"),
        event_type: row.get("event_type"),
        payload_version: row.get("payload_version"),
        payload: row.get("payload"),
        job_id: row.get("job_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        created_at: row.get("created_at"),
    })
}
