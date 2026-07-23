//! Durable job service, payload contract, and outbox relay.
//!
//! Job, outbox, and event payloads are intentionally ID-only. Human content,
//! filenames, object keys, raw errors, and secrets remain outside payload JSON.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::documents::{self, MarkdownArtifactRecord, NewMarkdownArtifact};
use crate::db::error::DbError;
use crate::db::models::{EventLogEntry, Job, JobStatus, JobType, OutboxEvent};
use crate::db::{jobs as repo, pool};

pub const CURRENT_JOB_PAYLOAD_VERSION: i32 = 3;
pub const CURRENT_EVENT_PAYLOAD_VERSION: i32 = 2;

const MAX_IDEMPOTENCY_KEY_LEN: usize = 160;
pub const MAX_WORKER_ID_LEN: usize = 128;
const LEASE_TOKEN_SEPARATOR_LEN: usize = 1;
const LEASE_TOKEN_UUID_LEN: usize = 36;
pub const MAX_LEASE_TOKEN_LEN: usize =
    MAX_WORKER_ID_LEN + LEASE_TOKEN_SEPARATOR_LEN + LEASE_TOKEN_UUID_LEN;
const MAX_LAST_ERROR_LEN: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JobPayload {
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub collection_id: Option<Uuid>,
    pub upload_id: Option<Uuid>,
    pub batch_id: Option<Uuid>,
    /// Target immutable index generation for a staged backfill.
    pub index_metadata_id: Option<Uuid>,
    /// Parent conversion job for an independent reconciliation job.
    pub cleanup_target_job_id: Option<Uuid>,
    /// Newer version that superseded `version_id` (lifecycle refresh).
    pub related_version_id: Option<Uuid>,
}

impl JobPayload {
    pub fn to_json(&self) -> Result<JsonValue, JobError> {
        let value = serde_json::to_value(self).map_err(|error| {
            JobError::InvalidPayload(format!("job payload serialization failed: {error}"))
        })?;
        validate_job_payload_json(&value)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
struct JobPayloadV1 {
    document_id: Option<Uuid>,
    version_id: Option<Uuid>,
}

impl From<JobPayloadV1> for JobPayload {
    fn from(value: JobPayloadV1) -> Self {
        Self {
            document_id: value.document_id,
            version_id: value.version_id,
            collection_id: None,
            upload_id: None,
            batch_id: None,
            index_metadata_id: None,
            cleanup_target_job_id: None,
            related_version_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EventPayload {
    pub job_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub outbox_event_id: Option<Uuid>,
}

impl EventPayload {
    fn for_job(job: &Job) -> Self {
        Self {
            job_id: Some(job.id),
            document_id: job.document_id,
            version_id: job.version_id,
            outbox_event_id: None,
        }
    }

    fn for_outbox(outbox: &OutboxEvent) -> Self {
        Self {
            job_id: outbox.job_id,
            document_id: None,
            version_id: None,
            outbox_event_id: Some(outbox.id),
        }
    }

    pub fn to_json(&self) -> Result<JsonValue, JobError> {
        let value = serde_json::to_value(self).map_err(|error| {
            JobError::InvalidPayload(format!("event payload serialization failed: {error}"))
        })?;
        validate_event_payload_json(&value)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CheckpointPayload {
    pub cursor_id: Option<Uuid>,
    pub completed_ids: Vec<Uuid>,
    pub staged_object_keys: Vec<String>,
    pub offset: Option<u64>,
}

impl CheckpointPayload {
    pub fn to_json(&self) -> Result<JsonValue, JobError> {
        let value = serde_json::to_value(self).map_err(|error| {
            JobError::InvalidPayload(format!("checkpoint serialization failed: {error}"))
        })?;
        validate_checkpoint_payload_json(&value)?;
        Ok(value)
    }
}

#[derive(Debug, Clone)]
pub struct EnqueueJob {
    pub id: Uuid,
    pub job_type: JobType,
    pub payload: JobPayload,
    pub idempotency_key: String,
    pub max_attempts: u32,
    pub available_after: Duration,
}

impl EnqueueJob {
    pub fn new(job_type: JobType, payload: JobPayload, idempotency_key: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_type,
            payload,
            idempotency_key: idempotency_key.into(),
            max_attempts: 5,
            available_after: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnqueueOutcome {
    pub job: Job,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CancelOutcome {
    Cancelled(Job),
    AlreadyCancelled(Job),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxPublication {
    pub outbox: OutboxEvent,
    pub event: EventLogEntry,
}

#[derive(Debug, Clone)]
pub struct CompleteConvertArtifact<'a> {
    pub artifact_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub object_key: &'a str,
    pub content_sha256: &'a str,
    pub byte_size: i64,
}

#[derive(Debug, Clone)]
pub struct CompleteConvertOutcome {
    pub job: Job,
    pub artifact: MarkdownArtifactRecord,
}

pub trait OutboxSink: Send + Sync {
    fn publish<'a>(
        &'a self,
        txn: &'a tokio_postgres::Transaction<'_>,
        ctx: &'a OrgContext,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<EventLogEntry, JobError>> + Send + 'a>>;
}

#[derive(Clone, Copy)]
pub struct HeartbeatClaim<'a> {
    pub db_pool: &'a Pool,
    pub ctx: &'a OrgContext,
    pub job_id: Uuid,
    pub lease_token: &'a str,
    pub attempts: i32,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub deadline: TokioInstant,
}

#[derive(Debug, Error)]
pub enum HeartbeatError {
    #[error("job error")]
    Job(#[from] JobError),
    #[error("job exceeded configured maximum duration")]
    TimedOut,
}

pub async fn heartbeat_once_claimed(claim: HeartbeatClaim<'_>) -> Result<(), HeartbeatError> {
    match timeout(
        heartbeat_call_timeout(claim.lease_ttl, claim.deadline),
        heartbeat(
            claim.db_pool,
            claim.ctx,
            claim.job_id,
            claim.lease_token,
            claim.attempts,
            claim.lease_ttl,
        ),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(HeartbeatError::Job(error)),
        Err(_) => Err(HeartbeatError::TimedOut),
    }
}

pub async fn heartbeat_while_claimed<T, E, Fut>(
    claim: HeartbeatClaim<'_>,
    future: Fut,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
    E: From<HeartbeatError>,
{
    heartbeat_once_claimed(claim).await.map_err(E::from)?;
    tokio::pin!(future);
    let mut heartbeat = interval_at(
        TokioInstant::now() + claim.heartbeat_interval,
        claim.heartbeat_interval,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            result = &mut future => return result,
            _ = sleep_until(claim.deadline) => return Err(E::from(HeartbeatError::TimedOut)),
            _ = heartbeat.tick() => heartbeat_once_claimed(claim).await.map_err(E::from)?,
        }
    }
}

#[derive(Debug, Default)]
pub struct EventLogOutboxSink;

impl OutboxSink for EventLogOutboxSink {
    fn publish<'a>(
        &'a self,
        txn: &'a tokio_postgres::Transaction<'_>,
        ctx: &'a OrgContext,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<EventLogEntry, JobError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(existing) = repo::find_outbox_published_event(txn, ctx, event.id).await? {
                return Ok(existing);
            }
            let payload = validated_event_payload(EventPayload::for_outbox(event))?;
            repo::append_event_log(
                txn,
                ctx,
                repo::NewEventLogEntry {
                    event_type: "outbox.published",
                    payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
                    payload: &payload,
                    job_id: event.job_id,
                    document_id: None,
                    version_id: None,
                },
            )
            .await
            .map_err(Into::into)
        })
    }
}

#[derive(Debug, Error)]
pub enum JobError {
    #[error("job payload is invalid: {0}")]
    InvalidPayload(String),
    #[error("idempotency key is invalid")]
    InvalidIdempotencyKey,
    #[error("lease owner is invalid")]
    InvalidLeaseOwner,
    #[error("duration must be positive and fit in PostgreSQL seconds")]
    InvalidDuration,
    #[error("limit must be positive and fit in i64")]
    InvalidLimit,
    #[error("max attempts must be at least one and fit in i32")]
    InvalidMaxAttempts,
    #[error("job lease is not owned by this worker")]
    LeaseLost,
    #[error("job was not found")]
    NotFound,
    #[error("job in status {0:?} cannot be cancelled")]
    CannotCancelTerminal(JobStatus),
    #[error("database error")]
    Database(#[from] DbError),
}

impl JobError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload(_) => "job_invalid_payload",
            Self::InvalidIdempotencyKey => "job_invalid_idempotency_key",
            Self::InvalidLeaseOwner => "job_invalid_lease_owner",
            Self::InvalidDuration => "job_invalid_duration",
            Self::InvalidLimit => "job_invalid_limit",
            Self::InvalidMaxAttempts => "job_invalid_max_attempts",
            Self::LeaseLost => "job_lease_lost",
            Self::NotFound => "job_not_found",
            Self::CannotCancelTerminal(_) => "job_cannot_cancel_terminal",
            Self::Database(_) => "job_database_error",
        }
    }
}

pub async fn enqueue(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: EnqueueJob,
) -> Result<EnqueueOutcome, JobError> {
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| Box::pin(async move { enqueue_within_txn(txn, &ctx, input).await })
    })
    .await
}

pub(crate) async fn enqueue_within_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    input: EnqueueJob,
) -> Result<EnqueueOutcome, JobError> {
    validate_idempotency_key(&input.idempotency_key)?;
    let max_attempts = checked_max_attempts(input.max_attempts)?;
    let available_after = checked_duration(input.available_after)?;
    let payload = validated_job_payload(&input.payload)?;
    validate_job_payload_lineage(&input.payload)?;

    let observed_at = repo::fresh_clock_timestamp(txn).await?;
    let available_at = observed_at
        .checked_add_signed(available_after)
        .ok_or(JobError::InvalidDuration)?;
    let event_payload = EventPayload {
        job_id: Some(input.id),
        document_id: input.payload.document_id,
        version_id: input.payload.version_id,
        outbox_event_id: None,
    }
    .to_validated()?;
    let outbox_idempotency_key = format!("job.enqueued:{}", input.id);
    let (job, created) = repo::insert_job_with_outbox(
        txn,
        ctx,
        repo::NewJob {
            id: input.id,
            job_type: input.job_type,
            payload_version: CURRENT_JOB_PAYLOAD_VERSION,
            payload: &payload,
            max_attempts,
            idempotency_key: &input.idempotency_key,
            document_id: input.payload.document_id,
            version_id: input.payload.version_id,
            available_at,
            outbox_event_type: "job.enqueued",
            outbox_payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
            outbox_payload: &event_payload,
            outbox_idempotency_key: &outbox_idempotency_key,
        },
    )
    .await?;
    Ok(EnqueueOutcome { job, created })
}

pub async fn claim(
    db_pool: &Pool,
    ctx: &OrgContext,
    worker_id: &str,
    limit: u32,
    lease_ttl: Duration,
) -> Result<Vec<Job>, JobError> {
    validate_worker_id(worker_id)?;
    let limit = checked_limit(limit)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let worker_id = worker_id.to_string();
        move |txn| {
            Box::pin(async move {
                repo::claim_pending(txn, &ctx, &worker_id, limit, lease_ttl_secs)
                    .await
                    .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn claim_type(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_type: JobType,
    worker_id: &str,
    limit: u32,
    lease_ttl: Duration,
) -> Result<Vec<Job>, JobError> {
    validate_worker_id(worker_id)?;
    let limit = checked_limit(limit)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let worker_id = worker_id.to_string();
        move |txn| {
            Box::pin(async move {
                repo::claim_pending_of_type(txn, &ctx, job_type, &worker_id, limit, lease_ttl_secs)
                    .await
                    .map_err(Into::into)
            })
        }
    })
    .await
}

/// Claims reconcile jobs for either conversion cleanup or document-drift workers.
pub async fn claim_reconcile(
    db_pool: &Pool,
    ctx: &OrgContext,
    worker_id: &str,
    limit: u32,
    lease_ttl: Duration,
    require_cleanup_target: bool,
) -> Result<Vec<Job>, JobError> {
    validate_worker_id(worker_id)?;
    let limit = checked_limit(limit)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let worker_id = worker_id.to_string();
        move |txn| {
            Box::pin(async move {
                repo::claim_pending_reconcile(
                    txn,
                    &ctx,
                    &worker_id,
                    limit,
                    lease_ttl_secs,
                    require_cleanup_target,
                )
                .await
                .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn heartbeat(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    lease_ttl: Duration,
) -> Result<(), JobError> {
    validate_lease_identifier(lease_token)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.to_string();
        move |txn| {
            Box::pin(async move {
                if repo::heartbeat(
                    txn,
                    &ctx,
                    job_id,
                    &lease_token,
                    claimed_attempts,
                    lease_ttl_secs,
                )
                .await?
                {
                    Ok(())
                } else {
                    Err(JobError::LeaseLost)
                }
            })
        }
    })
    .await
}

pub async fn checkpoint(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    checkpoint: CheckpointPayload,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    let checkpoint = validated_checkpoint_payload(checkpoint)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.to_string();
        move |txn| {
            Box::pin(async move {
                repo::save_checkpoint(
                    txn,
                    &ctx,
                    job_id,
                    &lease_token,
                    claimed_attempts,
                    &checkpoint,
                )
                .await?
                .ok_or(JobError::LeaseLost)
            })
        }
    })
    .await
}

pub(crate) async fn checkpoint_within_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    checkpoint: CheckpointPayload,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    let checkpoint = validated_checkpoint_payload(checkpoint)?;
    repo::save_checkpoint(txn, ctx, job_id, lease_token, claimed_attempts, &checkpoint)
        .await?
        .ok_or(JobError::LeaseLost)
}

pub async fn reclaim_expired(
    db_pool: &Pool,
    ctx: &OrgContext,
    limit: u32,
    backoff: Duration,
) -> Result<Vec<Job>, JobError> {
    let limit = checked_limit(limit)?;
    let backoff_secs = checked_duration_secs(backoff)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let jobs = repo::reclaim_expired(txn, &ctx, limit, backoff_secs).await?;
                for job in &jobs {
                    let event_type = match job.status {
                        JobStatus::Pending => "job.reclaimed",
                        JobStatus::DeadLetter => "job.dead_lettered",
                        _ => {
                            return Err(JobError::Database(DbError::Config(format!(
                                "unexpected reclaim status: {:?}",
                                job.status
                            ))));
                        }
                    };
                    let outbox_key = transition_key(job, event_type);
                    write_job_event(txn, &ctx, job, event_type, &outbox_key).await?;
                    if job.status == JobStatus::DeadLetter {
                        crate::services::indexing::handle_terminal_index_job(txn, &ctx, job)
                            .await?;
                    }
                }
                Ok(jobs)
            })
        }
    })
    .await
}

pub async fn complete(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.to_string();
        move |txn| {
            Box::pin(async move {
                complete_within_txn(txn, &ctx, job_id, &lease_token, claimed_attempts).await
            })
        }
    })
    .await
}

/// Fenced completion plus its event/outbox record, for workflows that must
/// atomically finalize their own durable state alongside a job.
pub(crate) async fn complete_within_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    let job = repo::complete_owned(txn, ctx, job_id, lease_token, claimed_attempts)
        .await?
        .ok_or(JobError::LeaseLost)?;
    let outbox_key = transition_key(&job, "job.succeeded");
    write_job_event(txn, ctx, &job, "job.succeeded", &outbox_key).await?;
    Ok(job)
}

pub async fn complete_with_markdown_artifact(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    artifact: CompleteConvertArtifact<'_>,
) -> Result<CompleteConvertOutcome, JobError> {
    validate_lease_identifier(lease_token)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.to_string();
        let object_key = artifact.object_key.to_string();
        let content_sha256 = artifact.content_sha256.to_string();
        move |txn| {
            Box::pin(async move {
                let job = repo::complete_owned(txn, &ctx, job_id, &lease_token, claimed_attempts)
                    .await?
                    .ok_or(JobError::LeaseLost)?;
                let artifact = documents::insert_markdown_artifact(
                    txn,
                    &ctx,
                    NewMarkdownArtifact {
                        id: artifact.artifact_id,
                        document_id: artifact.document_id,
                        version_id: artifact.version_id,
                        object_key: &object_key,
                        content_sha256: &content_sha256,
                        byte_size: artifact.byte_size,
                    },
                )
                .await?;
                let outbox_key = transition_key(&job, "job.succeeded");
                write_job_event(txn, &ctx, &job, "job.succeeded", &outbox_key).await?;
                Ok(CompleteConvertOutcome { job, artifact })
            })
        }
    })
    .await
}

pub async fn fail(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    last_error: &str,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    let last_error = sanitize_last_error(last_error);
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.to_string();
        move |txn| {
            Box::pin(async move {
                fail_within_txn(
                    txn,
                    &ctx,
                    job_id,
                    &lease_token,
                    claimed_attempts,
                    &last_error,
                )
                .await
            })
        }
    })
    .await
}

/// Fenced job failure plus its event/outbox record, for a caller that needs to
/// update another system-of-record row in the same transaction.
pub(crate) async fn fail_within_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    last_error: &str,
) -> Result<Job, JobError> {
    validate_lease_identifier(lease_token)?;
    let last_error = sanitize_last_error(last_error);
    let current = repo::get_by_id_for_update(txn, ctx, job_id)
        .await?
        .filter(|job| {
            job.status == JobStatus::Leased
                && job.lease_owner.as_deref() == Some(lease_token)
                && job.attempts == claimed_attempts
        })
        .ok_or(JobError::LeaseLost)?;
    let backoff_secs = backoff_for_attempt(current.attempts)?;
    let job = repo::fail_owned(
        txn,
        ctx,
        job_id,
        lease_token,
        claimed_attempts,
        &last_error,
        backoff_secs,
    )
    .await?
    .ok_or(JobError::LeaseLost)?;
    let event_type = match job.status {
        JobStatus::Pending => "job.retry_scheduled",
        JobStatus::DeadLetter => "job.dead_lettered",
        _ => {
            return Err(JobError::Database(DbError::Config(format!(
                "unexpected failure status: {:?}",
                job.status
            ))));
        }
    };
    let outbox_key = transition_key(&job, event_type);
    write_job_event(txn, ctx, &job, event_type, &outbox_key).await?;
    Ok(job)
}

pub async fn cancel(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<CancelOutcome, JobError> {
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let current = repo::get_by_id_for_update(txn, &ctx, job_id)
                    .await?
                    .ok_or(JobError::NotFound)?;
                match current.status {
                    JobStatus::Pending | JobStatus::Leased => {
                        // External/admin cancel is intentionally token-free: a user can
                        // cancel pending/running work without knowing a worker lease.
                        // Worker-side completion/failure/checkpoint remains fenced by
                        // exact lease token, claimed attempts, and status='leased'.
                        let job = repo::cancel_job(txn, &ctx, job_id)
                            .await?
                            .ok_or(JobError::NotFound)?;
                        let children = repo::cancel_embedding_children(txn, &ctx, job.id).await?;
                        let outbox_key = transition_key(&job, "job.cancelled");
                        write_job_event(txn, &ctx, &job, "job.cancelled", &outbox_key).await?;
                        crate::services::indexing::handle_terminal_index_job(txn, &ctx, &job)
                            .await?;
                        for child in &children {
                            let outbox_key = transition_key(child, "job.cancelled");
                            write_job_event(txn, &ctx, child, "job.cancelled", &outbox_key).await?;
                            crate::services::indexing::handle_terminal_index_job(txn, &ctx, child)
                                .await?;
                        }
                        Ok(CancelOutcome::Cancelled(job))
                    }
                    JobStatus::Cancelled => Ok(CancelOutcome::AlreadyCancelled(current)),
                    JobStatus::Succeeded
                    | JobStatus::Failed
                    | JobStatus::DeadLetter
                    | JobStatus::Running => Err(JobError::CannotCancelTerminal(current.status)),
                }
            })
        }
    })
    .await
}

pub async fn append_event(
    db_pool: &Pool,
    ctx: &OrgContext,
    event_type: &str,
    payload: EventPayload,
) -> Result<EventLogEntry, JobError> {
    validate_event_type(event_type)?;
    let job_id = payload.job_id;
    let document_id = payload.document_id;
    let version_id = payload.version_id;
    let payload = payload.to_validated()?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let event_type = event_type.to_string();
        move |txn| {
            Box::pin(async move {
                repo::append_event_log(
                    txn,
                    &ctx,
                    repo::NewEventLogEntry {
                        event_type: &event_type,
                        payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
                        payload: &payload,
                        job_id,
                        document_id,
                        version_id,
                    },
                )
                .await
                .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn relay_outbox(
    db_pool: &Pool,
    ctx: &OrgContext,
    limit: u32,
) -> Result<Vec<OutboxPublication>, JobError> {
    let sink = Arc::new(EventLogOutboxSink);
    relay_outbox_with_sink(db_pool, ctx, limit, &sink).await
}

pub async fn relay_outbox_with_sink<S>(
    db_pool: &Pool,
    ctx: &OrgContext,
    limit: u32,
    sink: &Arc<S>,
) -> Result<Vec<OutboxPublication>, JobError>
where
    S: OutboxSink + ?Sized + 'static,
{
    let limit = checked_limit(limit)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let sink = Arc::clone(sink);
        move |txn| {
            Box::pin(async move {
                let outbox_events = repo::claim_unpublished_outbox(txn, &ctx, limit).await?;
                let mut publications = Vec::with_capacity(outbox_events.len());
                for (index, outbox) in outbox_events.into_iter().enumerate() {
                    // Isolate each event in its own savepoint so a poison event (one that
                    // always fails to publish) cannot abort the shared transaction and
                    // block every later event for this org (head-of-line blocking). A
                    // failed event is rolled back and left unpublished for the next relay
                    // pass; healthy events in the same batch still commit.
                    let savepoint = format!("outbox_relay_{index}");
                    txn.batch_execute(&format!("SAVEPOINT {savepoint}"))
                        .await
                        .map_err(DbError::from)?;
                    match publish_one(txn, &ctx, sink.as_ref(), &outbox).await {
                        Ok(publication) => {
                            txn.batch_execute(&format!("RELEASE SAVEPOINT {savepoint}"))
                                .await
                                .map_err(DbError::from)?;
                            publications.push(publication);
                        }
                        Err(error) => {
                            txn.batch_execute(&format!("ROLLBACK TO SAVEPOINT {savepoint}"))
                                .await
                                .map_err(DbError::from)?;
                            txn.batch_execute(&format!("RELEASE SAVEPOINT {savepoint}"))
                                .await
                                .map_err(DbError::from)?;
                            tracing::warn!(
                                target: "outbox",
                                outbox_id = %outbox.id,
                                event_type = %outbox.event_type,
                                error = %error,
                                "outbox event publish failed; skipped to unblock the batch"
                            );
                        }
                    }
                }
                Ok(publications)
            })
        }
    })
    .await
}

async fn publish_one<S>(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    sink: &S,
    outbox: &OutboxEvent,
) -> Result<OutboxPublication, JobError>
where
    S: OutboxSink + ?Sized,
{
    let event = sink.publish(txn, ctx, outbox).await?;
    let Some(published) = repo::mark_outbox_published(txn, ctx, outbox.id).await? else {
        return Err(JobError::Database(DbError::Config(
            "claimed outbox event was already published".into(),
        )));
    };
    Ok(OutboxPublication {
        outbox: published,
        event,
    })
}

pub fn decode_job_payload(version: i32, payload: JsonValue) -> Result<JobPayload, JobError> {
    validate_job_payload_json(&payload)?;
    match version {
        1 => serde_json::from_value::<JobPayloadV1>(payload)
            .map(Into::into)
            .map_err(|error| JobError::InvalidPayload(format!("v1 decode failed: {error}"))),
        2 | 3 => serde_json::from_value::<JobPayload>(payload)
            .map_err(|error| JobError::InvalidPayload(format!("v2 decode failed: {error}"))),
        other => Err(JobError::InvalidPayload(format!(
            "unsupported payload version: {other}"
        ))),
    }
}

async fn write_job_event(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job: &Job,
    event_type: &str,
    outbox_idempotency_key: &str,
) -> Result<(), JobError> {
    let payload = EventPayload::for_job(job).to_validated()?;
    repo::append_event_and_outbox(
        txn,
        ctx,
        repo::NewEventLogEntry {
            event_type,
            payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
            payload: &payload,
            job_id: Some(job.id),
            document_id: job.document_id,
            version_id: job.version_id,
        },
        outbox_idempotency_key,
    )
    .await?;
    Ok(())
}

impl EventPayload {
    fn to_validated(&self) -> Result<repo::ValidatedEventPayload, JobError> {
        let value = self.to_json()?;
        repo::ValidatedEventPayload::new(value).map_err(payload_validation_error)
    }
}

fn validated_job_payload(payload: &JobPayload) -> Result<repo::ValidatedJobPayload, JobError> {
    let value = payload.to_json()?;
    repo::ValidatedJobPayload::new(value).map_err(payload_validation_error)
}

fn validated_event_payload(payload: EventPayload) -> Result<repo::ValidatedEventPayload, JobError> {
    payload.to_validated()
}

fn validated_checkpoint_payload(
    checkpoint: CheckpointPayload,
) -> Result<repo::ValidatedCheckpointPayload, JobError> {
    let value = checkpoint.to_json()?;
    repo::ValidatedCheckpointPayload::new(value).map_err(payload_validation_error)
}

fn validate_job_payload_json(value: &JsonValue) -> Result<(), JobError> {
    repo::ValidatedJobPayload::new(value.clone())
        .map(|_| ())
        .map_err(payload_validation_error)
}

fn validate_event_payload_json(value: &JsonValue) -> Result<(), JobError> {
    repo::ValidatedEventPayload::new(value.clone())
        .map(|_| ())
        .map_err(payload_validation_error)
}

fn validate_checkpoint_payload_json(value: &JsonValue) -> Result<(), JobError> {
    repo::ValidatedCheckpointPayload::new(value.clone())
        .map(|_| ())
        .map_err(payload_validation_error)
}

fn payload_validation_error(error: DbError) -> JobError {
    JobError::InvalidPayload(error.to_string())
}

fn validate_job_payload_lineage(payload: &JobPayload) -> Result<(), JobError> {
    if payload.version_id.is_some() && payload.document_id.is_none() {
        return Err(JobError::InvalidPayload(
            "version_id requires document_id".into(),
        ));
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> Result<(), JobError> {
    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_LEN || key.chars().any(char::is_control) {
        return Err(JobError::InvalidIdempotencyKey);
    }
    Ok(())
}

fn validate_worker_id(value: &str) -> Result<(), JobError> {
    if value.is_empty() || value.len() > MAX_WORKER_ID_LEN || value.chars().any(char::is_control) {
        return Err(JobError::InvalidLeaseOwner);
    }
    Ok(())
}

fn validate_lease_identifier(value: &str) -> Result<(), JobError> {
    if value.is_empty() || value.len() > MAX_LEASE_TOKEN_LEN || value.chars().any(char::is_control)
    {
        return Err(JobError::InvalidLeaseOwner);
    }
    Ok(())
}

fn validate_event_type(event_type: &str) -> Result<(), JobError> {
    if event_type.trim().is_empty() || event_type.chars().any(char::is_control) {
        return Err(JobError::InvalidPayload("event_type is invalid".into()));
    }
    Ok(())
}

fn checked_max_attempts(max_attempts: u32) -> Result<i32, JobError> {
    if max_attempts == 0 {
        return Err(JobError::InvalidMaxAttempts);
    }
    i32::try_from(max_attempts).map_err(|_| JobError::InvalidMaxAttempts)
}

fn checked_limit(limit: u32) -> Result<i64, JobError> {
    if limit == 0 {
        return Err(JobError::InvalidLimit);
    }
    Ok(i64::from(limit))
}

fn checked_duration(duration: Duration) -> Result<TimeDelta, JobError> {
    TimeDelta::from_std(duration).map_err(|_| JobError::InvalidDuration)
}

fn checked_duration_secs(duration: Duration) -> Result<i64, JobError> {
    let secs = duration.as_secs();
    if secs == 0 {
        return Err(JobError::InvalidDuration);
    }
    i64::try_from(secs).map_err(|_| JobError::InvalidDuration)
}

fn heartbeat_call_timeout(lease_ttl: Duration, deadline: TokioInstant) -> Duration {
    let mut lease_bound = lease_ttl / 3;
    if lease_bound.is_zero() {
        lease_bound = Duration::from_millis(1);
    }
    let remaining = deadline.saturating_duration_since(TokioInstant::now());
    remaining.min(lease_bound)
}

fn backoff_for_attempt(attempts: i32) -> Result<i64, JobError> {
    let attempts = u32::try_from(attempts.max(1)).map_err(|_| JobError::InvalidMaxAttempts)?;
    let shift = attempts.saturating_sub(1).min(8);
    Ok(1_i64 << shift)
}

fn sanitize_last_error(error: &str) -> String {
    error
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_LAST_ERROR_LEN)
        .collect()
}

fn transition_key(job: &Job, event_type: &str) -> String {
    format!("{event_type}:{}:{}", job.id, job.attempts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn older_job_payload_version_is_backward_readable() {
        let document_id = Uuid::new_v4();
        let payload = json!({ "document_id": document_id });
        let decoded = decode_job_payload(1, payload).expect("decode v1 payload");
        assert_eq!(decoded.document_id, Some(document_id));
        assert_eq!(decoded.version_id, None);
        assert_eq!(decoded.collection_id, None);
    }

    #[test]
    fn embedding_batch_payload_keeps_only_durable_ids() {
        let batch_id = Uuid::new_v4();
        let metadata_id = Uuid::new_v4();
        let payload = JobPayload {
            document_id: Some(Uuid::new_v4()),
            version_id: Some(Uuid::new_v4()),
            batch_id: Some(batch_id),
            index_metadata_id: Some(metadata_id),
            ..JobPayload::default()
        };
        let decoded = decode_job_payload(CURRENT_JOB_PAYLOAD_VERSION, payload.to_json().unwrap())
            .expect("embedding payload should be valid");
        assert_eq!(decoded.batch_id, Some(batch_id));
        assert_eq!(decoded.index_metadata_id, Some(metadata_id));
    }

    #[test]
    fn payload_rejects_content_and_secret_fields() {
        let content = json!({ "document_id": Uuid::new_v4(), "content": "hello" });
        assert!(matches!(
            decode_job_payload(2, content),
            Err(JobError::InvalidPayload(_))
        ));
        let secret = json!({ "secret_token": Uuid::new_v4() });
        assert!(matches!(
            decode_job_payload(2, secret),
            Err(JobError::InvalidPayload(_))
        ));
    }

    #[test]
    fn checkpoint_allows_progress_but_not_text() {
        let checkpoint = CheckpointPayload {
            cursor_id: Some(Uuid::new_v4()),
            completed_ids: vec![Uuid::new_v4()],
            staged_object_keys: vec![],
            offset: Some(42),
        };
        assert!(checkpoint.to_json().is_ok());
        assert!(validate_checkpoint_payload_json(&json!({ "body": "not allowed" })).is_err());
    }

    #[test]
    fn backoff_is_bounded_power_of_two() {
        assert_eq!(backoff_for_attempt(1).unwrap(), 1);
        assert_eq!(backoff_for_attempt(3).unwrap(), 4);
        assert_eq!(backoff_for_attempt(99).unwrap(), 256);
    }
}
