//! Idempotent conversion promotion saga.

use deadpool_postgres::Pool;
use serde_json::Value as JsonValue;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::document_versions::{
    self, ConversionSourceVersion, NewDerivedArtifact, NewPublishedVersion,
};
use crate::db::error::DbError;
use crate::db::jobs as jobs_repo;
use crate::db::models::{ArtifactKind, DocumentVersion, Job, JobStatus, ResourceKind};
use crate::db::{documents, pool, quota as quota_repo};
use crate::jobs::{self, CheckpointPayload, EventPayload, JobError};
use crate::services::conversion::{checkpoint_with_step, ConversionIdentity, ConversionStep};
use crate::services::quota::QuotaError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionFault {
    AfterStagingPut,
    AfterVersionInsert,
    AfterPointerSwap,
    AfterOutboxInsert,
    /// Simulates a lost response after the transaction has committed.
    AfterCommit,
}

#[derive(Debug, Clone)]
pub struct PromoteConversionInput {
    pub job_id: Uuid,
    pub lease_token: String,
    pub claimed_attempts: i32,
    pub identity: ConversionIdentity,
    pub source: ConversionSourceVersion,
    pub artifact_id: Uuid,
    pub staged_object_key: String,
    pub markdown_sha256: String,
    pub markdown_byte_size: i64,
    pub quota_reservation_key: String,
    pub fault: Option<PromotionFault>,
}

#[derive(Debug, Clone)]
pub struct PromoteConversionOutcome {
    pub job: Job,
    pub version: DocumentVersion,
    pub artifact_created: bool,
    pub committed_object_key: String,
}

#[derive(Debug, Error)]
pub enum PromotionError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("quota error")]
    Quota(#[from] QuotaError),
    #[error("job lease was lost")]
    LeaseLost,
    #[error("promotion idempotency conflict")]
    IdempotencyConflict,
    #[error("promotion may have committed but its result was not acknowledged")]
    CommittedOutcomeUnknown,
    #[error("injected promotion fault at {0:?}")]
    Injected(PromotionFault),
}

impl From<PromotionError> for JobError {
    fn from(error: PromotionError) -> Self {
        match error {
            PromotionError::Db(error) => JobError::Database(error),
            PromotionError::Job(error) => error,
            PromotionError::Quota(error) => JobError::Database(DbError::Config(error.to_string())),
            PromotionError::LeaseLost => JobError::LeaseLost,
            PromotionError::IdempotencyConflict => {
                JobError::Database(DbError::Config("promotion idempotency conflict".into()))
            }
            PromotionError::CommittedOutcomeUnknown => JobError::Database(DbError::Config(
                "promotion result was not acknowledged after commit".into(),
            )),
            PromotionError::Injected(fault) => {
                JobError::Database(DbError::Config(format!("promotion fault: {fault:?}")))
            }
        }
    }
}

pub async fn promote_conversion(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: PromoteConversionInput,
) -> Result<PromoteConversionOutcome, PromotionError> {
    let fault = input.fault;
    let outcome = pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let job = jobs_repo::get_by_id_for_update(txn, &ctx, input.job_id)
                    .await?
                    .filter(|job| {
                        job.status == JobStatus::Leased
                            && job.lease_owner.as_deref() == Some(input.lease_token.as_str())
                            && job.attempts == input.claimed_attempts
                    })
                    .ok_or(PromotionError::LeaseLost)?;

                let document =
                    documents::get_by_id_for_update(txn, &ctx, input.source.document_id).await?;
                let promoted_version_id = input.identity.promoted_version_id();
                let existing = document_versions::find_by_id(
                    txn,
                    &ctx,
                    input.source.document_id,
                    promoted_version_id,
                )
                .await?;

                let (version, created) = match existing {
                    Some(version) => {
                        ensure_existing_version_matches(&version, &input)?;
                        (version, false)
                    }
                    None => {
                        let inserted = document_versions::insert_published_version_if_absent(
                            txn,
                            &ctx,
                            NewPublishedVersion {
                                id: promoted_version_id,
                                document_id: input.source.document_id,
                                parent_version_id: input.source.source_version_id,
                                content_sha256: &input.markdown_sha256,
                                original_object_key: &input.source.original_object_key,
                                markdown_object_key: &input.staged_object_key,
                                source_filename: input.source.source_filename.as_deref(),
                                source_content_type: Some("text/markdown; charset=utf-8"),
                                byte_size: input.markdown_byte_size,
                                change_summary: "Converted upload to Markdown",
                            },
                        )
                        .await?;
                        maybe_fault(input.fault, PromotionFault::AfterVersionInsert)?;
                        inserted
                    }
                };

                let artifact = document_versions::insert_artifact_if_absent(
                    txn,
                    &ctx,
                    NewDerivedArtifact {
                        id: input.artifact_id,
                        document_id: input.source.document_id,
                        version_id: promoted_version_id,
                        kind: ArtifactKind::Markdown,
                        object_key: &input.staged_object_key,
                        content_sha256: &input.markdown_sha256,
                        content_type: "text/markdown; charset=utf-8",
                        byte_size: input.markdown_byte_size,
                    },
                )
                .await?;
                ensure_artifact_matches(&artifact, &input)?;
                if version.markdown_object_key.as_deref() != Some(artifact.object_key.as_str()) {
                    return Err(PromotionError::IdempotencyConflict);
                }

                if created || version.is_current {
                    document_versions::promote_current_if_needed(
                        txn,
                        &ctx,
                        input.source.document_id,
                        promoted_version_id,
                    )
                    .await?;
                }
                maybe_fault(input.fault, PromotionFault::AfterPointerSwap)?;

                let index_payload = validated_event_payload(EventPayload {
                    job_id: Some(job.id),
                    document_id: Some(input.source.document_id),
                    version_id: Some(promoted_version_id),
                    outbox_event_id: None,
                })?;
                let index_outbox_key = input.identity.index_outbox_key();
                jobs_repo::insert_outbox_event(
                    txn,
                    &ctx,
                    jobs_repo::NewOutboxEvent {
                        event_type: "document.index_requested",
                        payload_version: jobs::CURRENT_EVENT_PAYLOAD_VERSION,
                        payload: &index_payload,
                        idempotency_key: &index_outbox_key,
                        job_id: Some(job.id),
                    },
                )
                .await?;
                maybe_fault(input.fault, PromotionFault::AfterOutboxInsert)?;

                finalize_storage_quota_locked(
                    txn,
                    &ctx,
                    &input.quota_reservation_key,
                    input.markdown_byte_size,
                    input.job_id,
                )
                .await?;

                let checkpoint = checkpoint_with_step(
                    job.checkpoint.as_ref(),
                    &input.identity,
                    ConversionStep::Promoted,
                );
                save_checkpoint_locked(
                    txn,
                    &ctx,
                    input.job_id,
                    &input.lease_token,
                    input.claimed_attempts,
                    checkpoint,
                )
                .await?;

                let completed = jobs_repo::complete_owned(
                    txn,
                    &ctx,
                    input.job_id,
                    &input.lease_token,
                    input.claimed_attempts,
                )
                .await?
                .ok_or(PromotionError::LeaseLost)?;
                write_job_succeeded_event(txn, &ctx, &completed).await?;

                // The version/document relation is the ACL inheritance boundary: new
                // versions stay on the same document and collection.
                if document.id != version.document_id || document.org_id != version.org_id {
                    return Err(PromotionError::IdempotencyConflict);
                }

                Ok(PromoteConversionOutcome {
                    job: completed,
                    version,
                    artifact_created: artifact.created,
                    committed_object_key: artifact.object_key,
                })
            })
        }
    })
    .await?;
    if fault == Some(PromotionFault::AfterCommit) {
        return Err(PromotionError::CommittedOutcomeUnknown);
    }
    Ok(outcome)
}

pub async fn promoted_version(
    db_pool: &Pool,
    ctx: &OrgContext,
    identity: &ConversionIdentity,
) -> Result<Option<DocumentVersion>, PromotionError> {
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let identity = identity.clone();
        move |txn| {
            Box::pin(async move {
                document_versions::find_by_id(
                    txn,
                    &ctx,
                    identity.document_id,
                    identity.promoted_version_id(),
                )
                .await
                .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn committed_markdown_object_key(
    db_pool: &Pool,
    ctx: &OrgContext,
    identity: &ConversionIdentity,
) -> Result<Option<String>, PromotionError> {
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let identity = identity.clone();
        move |txn| {
            Box::pin(async move {
                let version_id = identity.promoted_version_id();
                let Some(artifact) =
                    document_versions::find_markdown_artifact(txn, &ctx, version_id).await?
                else {
                    return Ok(None);
                };
                if artifact.id != identity.markdown_artifact_id() {
                    return Err(PromotionError::IdempotencyConflict);
                }
                Ok(Some(artifact.object_key))
            })
        }
    })
    .await
}

fn ensure_existing_version_matches(
    version: &DocumentVersion,
    input: &PromoteConversionInput,
) -> Result<(), PromotionError> {
    if version.document_id != input.source.document_id
        || version.parent_version_id != Some(input.source.source_version_id)
        || version.content_sha256 != input.markdown_sha256
        || version.markdown_object_key.is_none()
    {
        return Err(PromotionError::IdempotencyConflict);
    }
    Ok(())
}

fn ensure_artifact_matches(
    artifact: &document_versions::ArtifactInsertOutcome,
    input: &PromoteConversionInput,
) -> Result<(), PromotionError> {
    if artifact.id != input.artifact_id
        || artifact.content_sha256 != input.markdown_sha256
        || artifact.byte_size != Some(input.markdown_byte_size)
    {
        return Err(PromotionError::IdempotencyConflict);
    }
    Ok(())
}

async fn save_checkpoint_locked(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    claimed_attempts: i32,
    checkpoint: CheckpointPayload,
) -> Result<(), PromotionError> {
    let checkpoint = checkpoint
        .to_json()
        .map_err(PromotionError::Job)
        .and_then(|value| {
            jobs_repo::ValidatedCheckpointPayload::new(value)
                .map_err(|error| PromotionError::Job(JobError::InvalidPayload(error.to_string())))
        })?;
    jobs_repo::save_checkpoint(txn, ctx, job_id, lease_token, claimed_attempts, &checkpoint)
        .await?
        .ok_or(PromotionError::LeaseLost)?;
    Ok(())
}

async fn write_job_succeeded_event(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job: &Job,
) -> Result<(), PromotionError> {
    let payload = validated_event_payload(EventPayload {
        job_id: Some(job.id),
        document_id: job.document_id,
        version_id: job.version_id,
        outbox_event_id: None,
    })?;
    jobs_repo::append_event_and_outbox(
        txn,
        ctx,
        jobs_repo::NewEventLogEntry {
            event_type: "job.succeeded",
            payload_version: jobs::CURRENT_EVENT_PAYLOAD_VERSION,
            payload: &payload,
            job_id: Some(job.id),
            document_id: job.document_id,
            version_id: job.version_id,
        },
        &format!("job.succeeded:{}:{}", job.id, job.attempts),
    )
    .await?;
    Ok(())
}

fn validated_event_payload(
    payload: EventPayload,
) -> Result<jobs_repo::ValidatedEventPayload, PromotionError> {
    let value: JsonValue = payload.to_json().map_err(PromotionError::Job)?;
    jobs_repo::ValidatedEventPayload::new(value)
        .map_err(|error| PromotionError::Job(JobError::InvalidPayload(error.to_string())))
}

async fn finalize_storage_quota_locked(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    amount: i64,
    job_id: Uuid,
) -> Result<(), PromotionError> {
    quota_repo::lock_admission(txn, ctx, ResourceKind::StorageBytes).await?;
    let observed_at = quota_repo::fresh_clock_timestamp(txn).await?;
    let reservation = quota_repo::get_by_key_for_update(txn, ctx, reservation_key)
        .await
        .map_err(|error| match error {
            DbError::NotFound => PromotionError::Quota(QuotaError::ReservationNotFound),
            other => PromotionError::Db(other),
        })?;
    if reservation.resource_kind != ResourceKind::StorageBytes
        || reservation.amount != amount
        || reservation.job_id != Some(job_id)
    {
        return Err(PromotionError::Quota(QuotaError::ReservationConflict));
    }
    match reservation.status {
        crate::db::models::ReservationStatus::Finalized => return Ok(()),
        crate::db::models::ReservationStatus::Refunded => {
            return Err(PromotionError::Quota(QuotaError::RefundedCannotFinalize));
        }
        crate::db::models::ReservationStatus::Expired => {
            return Err(PromotionError::Quota(QuotaError::ExpiredCannotFinalize));
        }
        crate::db::models::ReservationStatus::Reserved => {}
    }
    let finalized =
        quota_repo::finalize_reserved_by_key(txn, ctx, reservation_key, observed_at).await?;
    let Some(finalized) = finalized else {
        return Err(PromotionError::Quota(QuotaError::ReservationConflict));
    };
    let period = quota_repo::current_period(txn, finalized.resource_kind, observed_at).await?;
    let current = quota_repo::lock_committed_counter(txn, ctx, finalized.resource_kind, period)
        .await?
        .unwrap_or(0);
    let value = current
        .checked_add(finalized.amount)
        .ok_or(QuotaError::ArithmeticOverflow)?;
    quota_repo::upsert_counter_value(txn, ctx, finalized.resource_kind, period, value).await?;
    Ok(())
}

fn maybe_fault(
    configured: Option<PromotionFault>,
    point: PromotionFault,
) -> Result<(), PromotionError> {
    if configured == Some(point) {
        Err(PromotionError::Injected(point))
    } else {
        Ok(())
    }
}
