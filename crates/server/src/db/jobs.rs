//! Tenant-scoped durable job, outbox, and event-log repositories.
//!
//! Callers should run these functions inside [`crate::db::pool::with_org_txn`]
//! so RLS `app.org_id` is set before any row is visible or mutable.

use std::time::Duration;

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
pub(crate) struct ValidatedJobPayload(JsonValue);

#[derive(Debug, Clone)]
pub(crate) struct ValidatedEventPayload(JsonValue);

#[derive(Debug, Clone)]
pub(crate) struct ValidatedCheckpointPayload(JsonValue);

impl ValidatedJobPayload {
    pub(crate) fn new(value: JsonValue) -> Result<Self, DbError> {
        validate_id_only_payload(&value)?;
        Ok(Self(value))
    }

    fn as_json(&self) -> &JsonValue {
        &self.0
    }
}

impl ValidatedEventPayload {
    pub(crate) fn new(value: JsonValue) -> Result<Self, DbError> {
        validate_id_only_payload(&value)?;
        Ok(Self(value))
    }

    fn as_json(&self) -> &JsonValue {
        &self.0
    }
}

impl ValidatedCheckpointPayload {
    pub(crate) fn new(value: JsonValue) -> Result<Self, DbError> {
        validate_checkpoint_payload(&value)?;
        Ok(Self(value))
    }

    fn as_json(&self) -> &JsonValue {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct NewJob<'a> {
    pub id: Uuid,
    pub job_type: JobType,
    pub payload_version: i32,
    pub payload: &'a ValidatedJobPayload,
    pub max_attempts: i32,
    pub idempotency_key: &'a str,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub available_at: DateTime<Utc>,
    pub outbox_event_type: &'a str,
    pub outbox_payload_version: i32,
    pub outbox_payload: &'a ValidatedEventPayload,
    pub outbox_idempotency_key: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewOutboxEvent<'a> {
    pub event_type: &'a str,
    pub payload_version: i32,
    pub payload: &'a ValidatedEventPayload,
    pub idempotency_key: &'a str,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewEventLogEntry<'a> {
    pub event_type: &'a str,
    pub payload_version: i32,
    pub payload: &'a ValidatedEventPayload,
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
    let payload = input.payload.as_json();
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
                &payload,
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

/// Read-only job lookup for API status routes (no row lock).
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

/// Keyset page of jobs for API list routes.
///
/// Jobs with a `document_id` are visible only when that document's collection is
/// in `allowed_collection_ids`. Jobs without a document remain visible to the org.
pub async fn list_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    document_id: Option<Uuid>,
    limit: i64,
    after_created_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<Job>, DbError> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            &format!(
                "SELECT {JOB_COLUMNS_J}
                 FROM jobs j
                 LEFT JOIN documents d
                   ON d.org_id = j.org_id AND d.id = j.document_id
                 WHERE j.org_id = $1
                   AND ($2::uuid IS NULL OR j.document_id = $2)
                   AND (
                     j.document_id IS NULL
                     OR d.collection_id = ANY($3)
                   )
                   AND (
                     $4::timestamptz IS NULL
                     OR (j.created_at, j.id) > ($4::timestamptz, $5::uuid)
                   )
                 ORDER BY j.created_at, j.id
                 LIMIT $6"
            ),
            &[
                &ctx.org_id(),
                &document_id,
                &allowed_collection_ids,
                &after_created_at,
                &after_id,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

/// Pending queue depth and age of the oldest available pending job for a type.
pub async fn pending_queue_stats(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_type: JobType,
) -> Result<(u64, Duration), DbError> {
    let job_type = job_type.as_str();
    let row = txn
        .query_one(
            "SELECT
                count(*)::bigint AS depth,
                COALESCE(
                    EXTRACT(EPOCH FROM (clock_timestamp() - min(available_at))),
                    0
                )::bigint AS oldest_age_secs
             FROM jobs
             WHERE org_id = $1
               AND job_type = $2
               AND status = 'pending'
               AND available_at <= clock_timestamp()",
            &[&ctx.org_id(), &job_type],
        )
        .await?;
    let depth = u64::try_from(row.get::<_, i64>("depth")).unwrap_or(0);
    let oldest_age_secs = u64::try_from(row.get::<_, i64>("oldest_age_secs")).unwrap_or(0);
    Ok((depth, Duration::from_secs(oldest_age_secs)))
}

pub async fn claim_pending(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    worker_id: &str,
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
                     lease_owner = $3 || ':' || gen_random_uuid()::text,
                     lease_expires_at = clock_timestamp() + ($4::bigint * interval '1 second'),
                     heartbeat_at = clock_timestamp(),
                     started_at = COALESCE(started_at, clock_timestamp()),
                     attempts = attempts + 1,
                     updated_at = clock_timestamp()
                 FROM candidates
                 WHERE j.org_id = $1 AND j.id = candidates.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[&ctx.org_id(), &limit, &worker_id, &lease_ttl_secs],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn claim_pending_of_type(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_type: JobType,
    worker_id: &str,
    limit: i64,
    lease_ttl_secs: i64,
) -> Result<Vec<Job>, DbError> {
    let job_type = job_type.as_str();
    let rows = txn
        .query(
            &format!(
                "WITH candidates AS (
                    SELECT id
                    FROM jobs
                    WHERE org_id = $1
                      AND job_type = $2
                      AND status = 'pending'
                      AND available_at <= clock_timestamp()
                    ORDER BY available_at, created_at, id
                    FOR UPDATE SKIP LOCKED
                    LIMIT $3
                 )
                 UPDATE jobs j
                 SET status = 'leased',
                     lease_owner = $4 || ':' || gen_random_uuid()::text,
                     lease_expires_at = clock_timestamp() + ($5::bigint * interval '1 second'),
                     heartbeat_at = clock_timestamp(),
                     started_at = COALESCE(started_at, clock_timestamp()),
                     attempts = attempts + 1,
                     updated_at = clock_timestamp()
                 FROM candidates
                 WHERE j.org_id = $1 AND j.id = candidates.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[
                &ctx.org_id(),
                &job_type,
                &limit,
                &worker_id,
                &lease_ttl_secs,
            ],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn heartbeat(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    lease_ttl_secs: i64,
) -> Result<bool, DbError> {
    let updated = txn
        .execute(
            "UPDATE jobs
             SET heartbeat_at = clock_timestamp(),
                 lease_expires_at = clock_timestamp() + ($5::bigint * interval '1 second'),
                 updated_at = clock_timestamp()
             WHERE org_id = $1
               AND id = $2
               AND lease_owner = $3
               AND attempts = $4
               AND status = 'leased'",
            &[
                &ctx.org_id(),
                &job_id,
                &lease_token,
                &claimed_attempts,
                &lease_ttl_secs,
            ],
        )
        .await?;
    Ok(updated == 1)
}

pub async fn save_checkpoint(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    checkpoint: &ValidatedCheckpointPayload,
) -> Result<Option<Job>, DbError> {
    let checkpoint = checkpoint.as_json();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE jobs
                 SET checkpoint = $5, updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND lease_owner = $3
                   AND attempts = $4
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &job_id,
                &lease_token,
                &claimed_attempts,
                &checkpoint,
            ],
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
                "WITH locked AS (
                    SELECT id
                    FROM jobs
                    WHERE org_id = $1
                      AND status = 'leased'
                    ORDER BY lease_expires_at, id
                    FOR UPDATE SKIP LOCKED
                    LIMIT $2
                 ),
                 observed AS (
                    SELECT clock_timestamp() AS now
                 ),
                 expired AS (
                    SELECT j.id, observed.now
                    FROM jobs j
                    JOIN locked ON locked.id = j.id
                    CROSS JOIN observed
                    WHERE j.org_id = $1
                      AND j.status = 'leased'
                      AND j.lease_expires_at < observed.now
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
                             THEN expired.now + ($3::bigint * interval '1 second')
                         ELSE j.available_at
                     END,
                     finished_at = CASE
                         WHEN j.attempts < j.max_attempts THEN NULL
                         ELSE expired.now
                     END,
                     last_error = CASE
                         WHEN j.attempts < j.max_attempts THEN j.last_error
                         ELSE COALESCE(j.last_error, 'lease expired after max attempts')
                     END,
                     updated_at = expired.now
                 FROM expired
                 WHERE j.org_id = $1 AND j.id = expired.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[&ctx.org_id(), &limit, &backoff_secs],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn list_dead_letter_of_type(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_type: JobType,
    after_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Job>, DbError> {
    let job_type = job_type.as_str();
    let rows = txn
        .query(
            &format!(
                "SELECT {JOB_COLUMNS}
                 FROM jobs
                 WHERE org_id = $1 AND job_type = $2 AND status = 'dead_letter'
                   AND ($3::uuid IS NULL OR id > $3)
                 ORDER BY id
                 LIMIT $4"
            ),
            &[&ctx.org_id(), &job_type, &after_id, &limit],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn has_active_writer_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT EXISTS (
                SELECT 1
                FROM jobs
                WHERE org_id = $1
                  AND document_id = $2
                  AND job_type IN ('index', 'embedding_batch')
                  AND status IN ('pending', 'leased', 'running')
             )",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    Ok(row.get(0))
}

/// Cancels in-flight index/embedding writers for a document and returns them.
pub async fn cancel_active_writer_jobs(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<Job>, DbError> {
    let rows = txn
        .query(
            &format!(
                "UPDATE jobs
                 SET status = 'cancelled',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     finished_at = clock_timestamp(),
                     updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND document_id = $2
                   AND job_type IN ('index', 'embedding_batch')
                   AND status IN ('pending', 'leased', 'running')
                 RETURNING {JOB_COLUMNS}"
            ),
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

/// Claims reconcile jobs that either require or exclude conversion cleanup targets.
///
/// Conversion cleanup (I05) and document-drift reconcile (I07) share `job_type =
/// reconcile` but are consumed by different workers. The payload key
/// `cleanup_target_job_id` is the durable discriminator.
pub async fn claim_pending_reconcile(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    worker_id: &str,
    limit: i64,
    lease_ttl_secs: i64,
    require_cleanup_target: bool,
) -> Result<Vec<Job>, DbError> {
    let rows = txn
        .query(
            &format!(
                "WITH candidates AS (
                    SELECT id
                    FROM jobs
                    WHERE org_id = $1
                      AND job_type = 'reconcile'
                      AND status = 'pending'
                      AND available_at <= clock_timestamp()
                      AND (
                        CASE
                          WHEN $5 THEN payload->>'cleanup_target_job_id' IS NOT NULL
                          ELSE payload->>'cleanup_target_job_id' IS NULL
                        END
                      )
                    ORDER BY available_at, created_at, id
                    FOR UPDATE SKIP LOCKED
                    LIMIT $2
                 )
                 UPDATE jobs j
                 SET status = 'leased',
                     lease_owner = $3 || ':' || gen_random_uuid()::text,
                     lease_expires_at = clock_timestamp() + ($4::bigint * interval '1 second'),
                     heartbeat_at = clock_timestamp(),
                     started_at = COALESCE(started_at, clock_timestamp()),
                     attempts = attempts + 1,
                     updated_at = clock_timestamp()
                 FROM candidates
                 WHERE j.org_id = $1 AND j.id = candidates.id
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[
                &ctx.org_id(),
                &limit,
                &worker_id,
                &lease_ttl_secs,
                &require_cleanup_target,
            ],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn complete_owned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
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
                   AND attempts = $4
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id, &lease_token, &claimed_attempts],
        )
        .await?;
    row.map(|row| map_job(&row)).transpose()
}

pub async fn fail_owned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
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
                             THEN clock_timestamp() + ($6::bigint * interval '1 second')
                         ELSE available_at
                     END,
                     finished_at = CASE
                         WHEN attempts < max_attempts THEN NULL
                         ELSE clock_timestamp()
                     END,
                     last_error = $5,
                     updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND id = $2
                   AND lease_owner = $3
                   AND attempts = $4
                   AND status = 'leased'
                 RETURNING {JOB_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &job_id,
                &lease_token,
                &claimed_attempts,
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

/// Cancels pending or leased embedding jobs owned by an index parent. The
/// caller writes the corresponding events and compensates the durable batches
/// in the same transaction.
pub async fn cancel_embedding_children(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_job_id: Uuid,
) -> Result<Vec<Job>, DbError> {
    let rows = txn
        .query(
            &format!(
                "WITH children AS (
                    SELECT batch.job_id
                    FROM embedding_batches AS batch
                    WHERE batch.org_id = $1
                      AND batch.index_job_id = $2
                 )
                 UPDATE jobs AS job
                 SET status = 'cancelled',
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     heartbeat_at = NULL,
                     finished_at = clock_timestamp(),
                     updated_at = clock_timestamp()
                 FROM children
                 WHERE job.org_id = $1
                   AND job.id = children.job_id
                   AND job.status IN ('pending', 'leased')
                 RETURNING {JOB_COLUMNS_J}"
            ),
            &[&ctx.org_id(), &index_job_id],
        )
        .await?;
    rows.iter().map(map_job).collect()
}

pub async fn insert_outbox_event(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewOutboxEvent<'_>,
) -> Result<OutboxEvent, DbError> {
    let payload = input.payload.as_json();
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
                &payload,
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
    let payload = input.payload.as_json();
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
                &payload,
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

pub async fn find_outbox_published_event(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    outbox_id: Uuid,
) -> Result<Option<EventLogEntry>, DbError> {
    let outbox_id = outbox_id.to_string();
    let row = txn
        .query_opt(
            &format!(
                "SELECT {EVENT_COLUMNS}
                 FROM event_log
                 WHERE org_id = $1
                   AND event_type = 'outbox.published'
                   AND payload->>'outbox_event_id' = $2
                 ORDER BY sequence_no
                 LIMIT 1"
            ),
            &[&ctx.org_id(), &outbox_id],
        )
        .await?;
    row.map(|row| map_event_log_entry(&row)).transpose()
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

fn validate_id_only_payload(value: &JsonValue) -> Result<(), DbError> {
    reject_forbidden_keys(value)?;
    validate_id_only_value(value)
}

fn reject_forbidden_keys(value: &JsonValue) -> Result<(), DbError> {
    match value {
        JsonValue::Object(map) => {
            for (key, nested) in map {
                let normalized = key.to_ascii_lowercase();
                if [
                    "content", "secret", "token", "password", "markdown", "body", "text",
                ]
                .iter()
                .any(|forbidden| normalized.contains(forbidden))
                {
                    return Err(DbError::Config(format!("forbidden payload field: {key}")));
                }
                reject_forbidden_keys(nested)?;
            }
        }
        JsonValue::Array(values) => {
            for nested in values {
                reject_forbidden_keys(nested)?;
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
    Ok(())
}

fn validate_id_only_value(value: &JsonValue) -> Result<(), DbError> {
    validate_id_only_value_at(value, None)
}

fn validate_id_only_value_at(value: &JsonValue, key: Option<&str>) -> Result<(), DbError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(value) => {
            if key == Some("traceparent") {
                return validate_traceparent_payload(value);
            }
            Uuid::parse_str(value)
                .map(|_| ())
                .map_err(|_| DbError::Config("payload strings must be UUIDs".into()))
        }
        JsonValue::Array(values) => {
            for nested in values {
                validate_id_only_value_at(nested, None)?;
            }
            Ok(())
        }
        JsonValue::Object(map) => {
            for (nested_key, nested) in map {
                validate_id_only_value_at(nested, Some(nested_key.as_str()))?;
            }
            Ok(())
        }
        JsonValue::Bool(_) | JsonValue::Number(_) => Err(DbError::Config(
            "job/event payloads may contain only UUID strings, W3C traceparent, arrays, objects, or null"
                .into(),
        )),
    }
}

fn validate_traceparent_payload(value: &str) -> Result<(), DbError> {
    crate::telemetry::validate_traceparent(value).map_err(DbError::Config)
}

fn validate_checkpoint_payload(value: &JsonValue) -> Result<(), DbError> {
    reject_forbidden_keys(value)?;
    match value {
        JsonValue::Object(map) => {
            for (key, nested) in map {
                match key.as_str() {
                    "offset" => {
                        if !(nested.is_null() || nested.as_u64().is_some()) {
                            return Err(DbError::Config(
                                "checkpoint offset must be an unsigned integer".into(),
                            ));
                        }
                    }
                    "staged_object_keys" => validate_checkpoint_object_keys(nested)?,
                    _ => validate_id_only_value(nested)?,
                }
            }
            Ok(())
        }
        _ => Err(DbError::Config(
            "checkpoint payload must be an object".into(),
        )),
    }
}

fn validate_checkpoint_object_keys(value: &JsonValue) -> Result<(), DbError> {
    let JsonValue::Array(values) = value else {
        return Err(DbError::Config(
            "checkpoint staged_object_keys must be an array".into(),
        ));
    };
    for value in values {
        let JsonValue::String(key) = value else {
            return Err(DbError::Config(
                "checkpoint staged_object_keys entries must be strings".into(),
            ));
        };
        if key.is_empty()
            || key.len() > 256
            || !key.starts_with("trusted/")
            || key.starts_with('/')
            || key.contains('\\')
            || key.contains('\0')
            || key.chars().any(char::is_control)
            || key
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(".."))
        {
            return Err(DbError::Config(
                "checkpoint staged_object_keys entry is invalid".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validated_payloads_reject_nested_forbidden_fields() {
        assert!(ValidatedJobPayload::new(json!({
            "document_id": Uuid::new_v4(),
            "nested": [{ "secret_token": Uuid::new_v4() }]
        }))
        .is_err());
        assert!(ValidatedEventPayload::new(json!({
            "outbox_event_id": Uuid::new_v4(),
            "items": [{ "body": Uuid::new_v4() }]
        }))
        .is_err());
    }

    #[test]
    fn repo_write_types_are_validated_newtypes_not_raw_json_values() {
        let payload = ValidatedJobPayload::new(json!({ "document_id": Uuid::new_v4() }))
            .expect("validated job payload");
        let event = ValidatedEventPayload::new(json!({ "job_id": Uuid::new_v4() }))
            .expect("validated event payload");
        let _new_job = NewJob {
            id: Uuid::new_v4(),
            job_type: JobType::Convert,
            payload_version: 1,
            payload: &payload,
            max_attempts: 1,
            idempotency_key: "compile-time-typed",
            document_id: None,
            version_id: None,
            available_at: Utc::now(),
            outbox_event_type: "job.enqueued",
            outbox_payload_version: 1,
            outbox_payload: &event,
            outbox_idempotency_key: "outbox-typed",
        };
    }
}
