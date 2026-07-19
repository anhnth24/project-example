//! Durable job service, payload contract, and outbox relay.
//!
//! Job, outbox, and event payloads are intentionally ID-only. Human content,
//! filenames, object keys, raw errors, and secrets remain outside payload JSON.

use std::time::Duration;

use chrono::TimeDelta;
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{EventLogEntry, Job, JobStatus, JobType, OutboxEvent};
use crate::db::{jobs as repo, pool};

pub const CURRENT_JOB_PAYLOAD_VERSION: i32 = 2;
pub const CURRENT_EVENT_PAYLOAD_VERSION: i32 = 2;

const MAX_IDEMPOTENCY_KEY_LEN: usize = 160;
const MAX_LEASE_OWNER_LEN: usize = 128;
const MAX_LAST_ERROR_LEN: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JobPayload {
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub collection_id: Option<Uuid>,
    pub upload_id: Option<Uuid>,
    pub batch_id: Option<Uuid>,
}

impl JobPayload {
    pub fn to_json(&self) -> Result<JsonValue, JobError> {
        let value = serde_json::to_value(self).map_err(|error| {
            JobError::InvalidPayload(format!("job payload serialization failed: {error}"))
        })?;
        assert_id_only_payload(&value)?;
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        assert_id_only_payload(&value)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CheckpointPayload {
    pub cursor_id: Option<Uuid>,
    pub completed_ids: Vec<Uuid>,
    pub offset: Option<u64>,
}

impl CheckpointPayload {
    pub fn to_json(&self) -> Result<JsonValue, JobError> {
        let value = serde_json::to_value(self).map_err(|error| {
            JobError::InvalidPayload(format!("checkpoint serialization failed: {error}"))
        })?;
        assert_checkpoint_payload(&value)?;
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
    validate_idempotency_key(&input.idempotency_key)?;
    let max_attempts = checked_max_attempts(input.max_attempts)?;
    let available_after = checked_duration(input.available_after)?;
    let payload_json = input.payload.to_json()?;
    validate_job_payload_lineage(&input.payload)?;

    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
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
                .to_json()?;
                let outbox_idempotency_key = format!("job.enqueued:{}", input.id);
                let (job, created) = repo::insert_job_with_outbox(
                    txn,
                    &ctx,
                    repo::NewJob {
                        id: input.id,
                        job_type: input.job_type,
                        payload_version: CURRENT_JOB_PAYLOAD_VERSION,
                        payload: &payload_json,
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
            })
        }
    })
    .await
}

pub async fn claim(
    db_pool: &Pool,
    ctx: &OrgContext,
    lease_owner: &str,
    limit: u32,
    lease_ttl: Duration,
) -> Result<Vec<Job>, JobError> {
    validate_lease_owner(lease_owner)?;
    let limit = checked_limit(limit)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_owner = lease_owner.to_string();
        move |txn| {
            Box::pin(async move {
                repo::claim_pending(txn, &ctx, &lease_owner, limit, lease_ttl_secs)
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
    lease_owner: &str,
    lease_ttl: Duration,
) -> Result<bool, JobError> {
    validate_lease_owner(lease_owner)?;
    let lease_ttl_secs = checked_duration_secs(lease_ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_owner = lease_owner.to_string();
        move |txn| {
            Box::pin(async move {
                repo::heartbeat(txn, &ctx, job_id, &lease_owner, lease_ttl_secs)
                    .await
                    .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn checkpoint(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
    checkpoint: CheckpointPayload,
) -> Result<Job, JobError> {
    validate_lease_owner(lease_owner)?;
    let checkpoint = checkpoint.to_json()?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_owner = lease_owner.to_string();
        move |txn| {
            Box::pin(async move {
                repo::save_checkpoint(txn, &ctx, job_id, &lease_owner, &checkpoint)
                    .await?
                    .ok_or(JobError::LeaseLost)
            })
        }
    })
    .await
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
    lease_owner: &str,
) -> Result<Job, JobError> {
    validate_lease_owner(lease_owner)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_owner = lease_owner.to_string();
        move |txn| {
            Box::pin(async move {
                let job = repo::complete_owned(txn, &ctx, job_id, &lease_owner)
                    .await?
                    .ok_or(JobError::LeaseLost)?;
                let outbox_key = transition_key(&job, "job.succeeded");
                write_job_event(txn, &ctx, &job, "job.succeeded", &outbox_key).await?;
                Ok(job)
            })
        }
    })
    .await
}

pub async fn fail(
    db_pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_owner: &str,
    last_error: &str,
) -> Result<Job, JobError> {
    validate_lease_owner(lease_owner)?;
    let last_error = sanitize_last_error(last_error);
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_owner = lease_owner.to_string();
        move |txn| {
            Box::pin(async move {
                let current = repo::get_by_id_for_update(txn, &ctx, job_id)
                    .await?
                    .filter(|job| {
                        job.status == JobStatus::Leased
                            && job.lease_owner.as_deref() == Some(lease_owner.as_str())
                    })
                    .ok_or(JobError::LeaseLost)?;
                let backoff_secs = backoff_for_attempt(current.attempts)?;
                let job =
                    repo::fail_owned(txn, &ctx, job_id, &lease_owner, &last_error, backoff_secs)
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
                write_job_event(txn, &ctx, &job, event_type, &outbox_key).await?;
                Ok(job)
            })
        }
    })
    .await
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
                        let job = repo::cancel_job(txn, &ctx, job_id)
                            .await?
                            .ok_or(JobError::NotFound)?;
                        let outbox_key = transition_key(&job, "job.cancelled");
                        write_job_event(txn, &ctx, &job, "job.cancelled", &outbox_key).await?;
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
    let payload = payload.to_json()?;
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
                        job_id: None,
                        document_id: None,
                        version_id: None,
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
    let limit = checked_limit(limit)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let outbox_events = repo::claim_unpublished_outbox(txn, &ctx, limit).await?;
                let mut publications = Vec::with_capacity(outbox_events.len());
                for outbox in outbox_events {
                    let payload = EventPayload::for_outbox(&outbox).to_json()?;
                    let event = repo::append_event_log(
                        txn,
                        &ctx,
                        repo::NewEventLogEntry {
                            event_type: "outbox.published",
                            payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
                            payload: &payload,
                            job_id: outbox.job_id,
                            document_id: None,
                            version_id: None,
                        },
                    )
                    .await?;
                    let Some(outbox) = repo::mark_outbox_published(txn, &ctx, outbox.id).await?
                    else {
                        return Err(JobError::Database(DbError::Config(
                            "claimed outbox event was already published".into(),
                        )));
                    };
                    publications.push(OutboxPublication { outbox, event });
                }
                Ok(publications)
            })
        }
    })
    .await
}

pub fn decode_job_payload(version: i32, payload: JsonValue) -> Result<JobPayload, JobError> {
    assert_id_only_payload(&payload)?;
    match version {
        1 => serde_json::from_value::<JobPayloadV1>(payload)
            .map(Into::into)
            .map_err(|error| JobError::InvalidPayload(format!("v1 decode failed: {error}"))),
        2 => serde_json::from_value::<JobPayload>(payload)
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
    let payload = EventPayload::for_job(job).to_json()?;
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

fn validate_job_payload_lineage(payload: &JobPayload) -> Result<(), JobError> {
    if payload.version_id.is_some() && payload.document_id.is_none() {
        return Err(JobError::InvalidPayload(
            "version_id requires document_id".into(),
        ));
    }
    Ok(())
}

fn assert_id_only_payload(value: &JsonValue) -> Result<(), JobError> {
    assert_no_forbidden_keys(value)?;
    assert_id_only_value(value)
}

fn assert_no_forbidden_keys(value: &JsonValue) -> Result<(), JobError> {
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
                    return Err(JobError::InvalidPayload(format!(
                        "forbidden payload field: {key}"
                    )));
                }
                assert_no_forbidden_keys(nested)?;
            }
        }
        JsonValue::Array(values) => {
            for nested in values {
                assert_no_forbidden_keys(nested)?;
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {}
    }
    Ok(())
}

fn assert_id_only_value(value: &JsonValue) -> Result<(), JobError> {
    match value {
        JsonValue::Null => Ok(()),
        JsonValue::String(value) => Uuid::parse_str(value)
            .map(|_| ())
            .map_err(|_| JobError::InvalidPayload("payload strings must be UUIDs".into())),
        JsonValue::Array(values) => {
            for nested in values {
                assert_id_only_value(nested)?;
            }
            Ok(())
        }
        JsonValue::Object(map) => {
            for nested in map.values() {
                assert_id_only_value(nested)?;
            }
            Ok(())
        }
        JsonValue::Bool(_) | JsonValue::Number(_) => Err(JobError::InvalidPayload(
            "job/event payloads may contain only UUID strings, arrays, objects, or null".into(),
        )),
    }
}

fn assert_checkpoint_payload(value: &JsonValue) -> Result<(), JobError> {
    assert_no_forbidden_keys(value)?;
    match value {
        JsonValue::Object(map) => {
            for (key, nested) in map {
                match key.as_str() {
                    "offset" => {
                        if !(nested.is_null() || nested.as_u64().is_some()) {
                            return Err(JobError::InvalidPayload(
                                "checkpoint offset must be an unsigned integer".into(),
                            ));
                        }
                    }
                    _ => assert_id_only_value(nested)?,
                }
            }
            Ok(())
        }
        _ => Err(JobError::InvalidPayload(
            "checkpoint payload must be an object".into(),
        )),
    }
}

fn validate_idempotency_key(key: &str) -> Result<(), JobError> {
    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_LEN || key.chars().any(char::is_control) {
        return Err(JobError::InvalidIdempotencyKey);
    }
    Ok(())
}

fn validate_lease_owner(owner: &str) -> Result<(), JobError> {
    if owner.is_empty() || owner.len() > MAX_LEASE_OWNER_LEN || owner.chars().any(char::is_control)
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
            offset: Some(42),
        };
        assert!(checkpoint.to_json().is_ok());
        assert!(assert_checkpoint_payload(&json!({ "body": "not allowed" })).is_err());
    }

    #[test]
    fn backoff_is_bounded_power_of_two() {
        assert_eq!(backoff_for_attempt(1).unwrap(), 1);
        assert_eq!(backoff_for_attempt(3).unwrap(), 4);
        assert_eq!(backoff_for_attempt(99).unwrap(), 256);
    }
}
