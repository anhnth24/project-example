//! Tombstone request and purge orchestration.

use std::collections::BTreeSet;
use std::time::Duration;

use deadpool_postgres::Pool;
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use tokio::time::Instant as TokioInstant;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::models::{Document, DocumentState, Job, JobStatus};
use crate::db::pool::with_org_txn_typed;
use crate::db::{
    chunks, document_versions, documents, index_metadata, jobs as repo, vector_cleanup_intents,
};
use crate::jobs::{
    self, CheckpointPayload, EventPayload, HeartbeatClaim, HeartbeatError, JobError,
};
use crate::services::document_state;
use crate::services::index_signature::collection_name_for_digest;
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::{QdrantClient, VectorScope};
use crate::storage::StorageError;

const PURGE_STEP_QDRANT: u64 = 1;
const PURGE_STEP_MINIO: u64 = 2;
const PURGE_STEP_CHUNKS: u64 = 3;
const OBJECT_DELETE_BATCH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteRequestOutcome {
    Requested(Document),
    AlreadyRequested(Document),
}

#[derive(Debug, Clone, Copy)]
pub struct PurgeDocumentInput<'a> {
    pub job: &'a Job,
    pub lease_token: &'a str,
    pub attempts: i32,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub deadline: TokioInstant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeDocumentOutcome {
    Purged { job_id: Uuid, deleted_chunks: u64 },
    AlreadyPurged { job_id: Uuid },
}

struct PurgePlan {
    document: Document,
    signatures: Vec<String>,
    object_keys: Vec<String>,
}

/// Fence condition before marking Purged (hermetic / unit-tested).
pub fn writers_are_quiesced(active_writers: bool, pending_cleanup_intents: bool) -> bool {
    !active_writers && !pending_cleanup_intents
}

/// Immediate read suppression after tombstone (hermetic / unit-tested).
pub fn document_reads_suppressed(state: DocumentState, deleted_at_set: bool) -> bool {
    matches!(state, DocumentState::Tombstoned | DocumentState::Purged) || deleted_at_set
}

pub async fn request_delete(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<DeleteRequestOutcome, DeletionError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                match document.state {
                    DocumentState::Tombstoned | DocumentState::Purged => {
                        Ok(DeleteRequestOutcome::AlreadyRequested(document))
                    }
                    DocumentState::Indexed => {
                        // POC limitation: deletion is currently defined after successful indexing.
                        // Tombstone holds the document row lock, then cancels any pending/leased
                        // writer jobs with I06 terminal compensation. Persist/finalize paths also
                        // lock this row, so a racing writer serializes behind the tombstone and
                        // aborts without resurrecting chunks or vectors.
                        let cancelled_writers =
                            repo::cancel_active_writer_jobs(txn, &ctx, document_id).await?;
                        let cancelled_count = cancelled_writers.len();
                        for cancelled in &cancelled_writers {
                            crate::services::indexing::handle_terminal_index_job(
                                txn, &ctx, cancelled,
                            )
                            .await
                            .map_err(DeletionError::Job)?;
                        }
                        document_state::apply_transition(
                            txn,
                            &ctx,
                            document_id,
                            DocumentState::Indexed,
                            DocumentState::Tombstoned,
                        )
                        .await?;
                        let tombstoned = documents::mark_deleted_at(txn, &ctx, document_id).await?;
                        let payload = validated_event_payload(EventPayload {
                            job_id: None,
                            document_id: Some(document_id),
                            version_id: tombstoned.current_version_id,
                            outbox_event_id: None,
                        })?;
                        repo::insert_outbox_event(
                            txn,
                            &ctx,
                            repo::NewOutboxEvent {
                                event_type: "document.delete_requested",
                                payload_version: jobs::CURRENT_EVENT_PAYLOAD_VERSION,
                                payload: &payload,
                                idempotency_key: &format!(
                                    "document.delete_requested.{document_id}"
                                ),
                                job_id: None,
                            },
                        )
                        .await?;
                        let resource_id = document_id.to_string();
                        let request_id = crate::services::audit::request_id_from_correlation();
                        write_audit(
                            txn,
                            AuditEvent {
                                org_id: ctx.org_id(),
                                actor_user_id: Some(ctx.user_id()),
                                action: "document.tombstone",
                                resource_type: "document",
                                resource_id: Some(&resource_id),
                                outcome: "success",
                                request_id: &request_id,
                                metadata: json!({
                                    "document_id": document_id,
                                    "version_id": tombstoned.current_version_id,
                                    "cancelled_writer_jobs": cancelled_count,
                                }),
                            },
                        )
                        .await?;
                        Ok(DeleteRequestOutcome::Requested(tombstoned))
                    }
                    other => Err(DeletionError::UnexpectedState(other)),
                }
            })
        }
    })
    .await
}

pub async fn purge_document(
    pool: &Pool,
    storage: &MinioClient,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    input: PurgeDocumentInput<'_>,
) -> Result<PurgeDocumentOutcome, DeletionError> {
    let payload = jobs::decode_job_payload(input.job.payload_version, input.job.payload.clone())?;
    let document_id = payload.document_id.ok_or(DeletionError::InvalidPayload)?;
    let plan = load_purge_plan(pool, ctx, document_id).await?;
    if plan.document.state == DocumentState::Purged {
        let completed =
            jobs::complete(pool, ctx, input.job.id, input.lease_token, input.attempts).await?;
        return Ok(PurgeDocumentOutcome::AlreadyPurged {
            job_id: completed.id,
        });
    }
    if plan.document.state != DocumentState::Tombstoned {
        return Err(DeletionError::UnexpectedState(plan.document.state));
    }

    let mut offset = checkpoint_offset(input.job)?;
    if offset > PURGE_STEP_CHUNKS {
        return Err(DeletionError::InvalidCheckpoint);
    }
    if offset < PURGE_STEP_QDRANT {
        check_deadline(input.deadline)?;
        heartbeat_while(
            pool,
            ctx,
            input,
            delete_qdrant_points(qdrant, ctx, &plan, document_id),
        )
        .await?;
        save_checkpoint(pool, ctx, input, PURGE_STEP_QDRANT).await?;
        offset = PURGE_STEP_QDRANT;
    }
    if offset < PURGE_STEP_MINIO {
        check_deadline(input.deadline)?;
        heartbeat_while(
            pool,
            ctx,
            input,
            delete_minio_objects_audited(pool, storage, ctx, document_id, &plan.object_keys),
        )
        .await?;
        save_checkpoint(pool, ctx, input, PURGE_STEP_MINIO).await?;
        offset = PURGE_STEP_MINIO;
    }
    let deleted_chunks = if offset < PURGE_STEP_CHUNKS {
        check_deadline(input.deadline)?;
        let deleted =
            heartbeat_while(pool, ctx, input, delete_chunks(pool, ctx, document_id)).await?;
        save_checkpoint(pool, ctx, input, PURGE_STEP_CHUNKS).await?;
        deleted
    } else {
        0
    };

    check_deadline(input.deadline)?;
    // Drain any vector-write intents left by a killed embedding worker, then
    // re-sweep Qdrant before the quiesce fence.
    heartbeat_while(
        pool,
        ctx,
        input,
        drain_vector_cleanup_intents(pool, qdrant, ctx, &plan, document_id),
    )
    .await?;
    heartbeat_while(
        pool,
        ctx,
        input,
        delete_qdrant_points(qdrant, ctx, &plan, document_id),
    )
    .await?;
    let completed = finalize_purged(pool, ctx, input, document_id, deleted_chunks).await?;
    Ok(PurgeDocumentOutcome::Purged {
        job_id: completed.id,
        deleted_chunks,
    })
}

async fn load_purge_plan(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<PurgePlan, DeletionError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let mut signatures = BTreeSet::new();
                for signature in
                    chunks::distinct_index_signatures_by_document(txn, &ctx, document_id).await?
                {
                    signatures.insert(signature);
                }
                for signature in
                    index_metadata::list_signatures_by_collection(txn, &ctx, document.collection_id)
                        .await?
                {
                    signatures.insert(signature);
                }
                let object_keys =
                    document_versions::list_object_keys_by_document(txn, &ctx, document_id).await?;
                Ok(PurgePlan {
                    document,
                    signatures: signatures.into_iter().collect(),
                    object_keys,
                })
            })
        }
    })
    .await
}

async fn delete_qdrant_points(
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    plan: &PurgePlan,
    document_id: Uuid,
) -> Result<(), DeletionError> {
    let scope = VectorScope::new(ctx.org_id(), [plan.document.collection_id]);
    let filter = [json!({
        "key": "document_id",
        "match": { "value": document_id.to_string() }
    })];
    for digest in &plan.signatures {
        let collection = collection_name_for_digest(digest)?;
        qdrant.delete_by_scope(&collection, &scope, &filter).await?;
    }
    Ok(())
}

async fn drain_vector_cleanup_intents(
    pool: &Pool,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    plan: &PurgePlan,
    document_id: Uuid,
) -> Result<(), DeletionError> {
    // Shared document lock spans PG finalize/cancel and Qdrant deletes so a
    // writer cannot authorize+upsert between cleanup steps.
    // Orchestration (mirrored by hermetic IntentDrainBackend tests):
    // - pending: finalize cleaned first (fence begin_write), then delete
    // - writing: cancel writers, delete possible write, then finalize cleaned
    let collection_id = plan.document.collection_id;
    let qdrant = qdrant.clone();
    vector_cleanup_intents::with_vector_mutation_lock(pool, ctx, document_id, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let intents =
                    vector_cleanup_intents::list_open_for_document(txn, &ctx, document_id).await?;
                let scope = VectorScope::new(ctx.org_id(), [collection_id]);
                let mut cancelled_writers = false;
                for intent in intents {
                    let Some(cleanup_plan) =
                        vector_cleanup_intents::plan_intent_cleanup(intent.status)
                    else {
                        continue;
                    };
                    let collection = collection_name_for_digest(&intent.index_signature_sha256)?;
                    let document_filter = [json!({
                        "key": "document_id",
                        "match": { "value": document_id.to_string() }
                    })];
                    match cleanup_plan {
                        vector_cleanup_intents::IntentCleanupPlan::CleanThenDelete => {
                            vector_cleanup_intents::cas_mark_cleaned_from(
                                txn,
                                &ctx,
                                intent.job_id,
                                vector_cleanup_intents::VectorCleanupIntentStatus::Pending,
                            )
                            .await
                            .map_err(map_intent_drain_error)?;
                            qdrant
                                .delete_points_by_ids(
                                    &collection,
                                    &scope,
                                    &document_filter,
                                    &intent.point_ids,
                                )
                                .await?;
                        }
                        vector_cleanup_intents::IntentCleanupPlan::CancelDeleteThenClean => {
                            if !cancelled_writers {
                                repo::cancel_active_writer_jobs(txn, &ctx, document_id).await?;
                                cancelled_writers = true;
                            }
                            qdrant
                                .delete_points_by_ids(
                                    &collection,
                                    &scope,
                                    &document_filter,
                                    &intent.point_ids,
                                )
                                .await?;
                            vector_cleanup_intents::cas_mark_cleaned_from(
                                txn,
                                &ctx,
                                intent.job_id,
                                vector_cleanup_intents::VectorCleanupIntentStatus::Writing,
                            )
                            .await
                            .map_err(map_intent_drain_error)?;
                        }
                    }
                }
                Ok(())
            })
        }
    })
    .await
}

fn map_intent_drain_error(
    error: vector_cleanup_intents::VectorCleanupIntentError,
) -> DeletionError {
    match error {
        vector_cleanup_intents::VectorCleanupIntentError::Db(db) => DeletionError::Db(db),
        _ => DeletionError::WritersNotQuiesced,
    }
}

async fn delete_minio_objects_audited(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    document_id: Uuid,
    object_keys: &[String],
) -> Result<(), DeletionError> {
    for batch in object_keys.chunks(OBJECT_DELETE_BATCH) {
        let batch_keys = batch.to_vec();
        write_object_cleanup_audit(pool, ctx, document_id, "intent", &batch_keys).await?;
        match delete_minio_object_batch(storage, ctx, &batch_keys).await {
            Ok(()) => {
                write_object_cleanup_audit(pool, ctx, document_id, "success", &batch_keys).await?;
            }
            Err(error) => {
                write_object_cleanup_audit(pool, ctx, document_id, "error", &batch_keys).await?;
                return Err(error);
            }
        }
    }
    Ok(())
}

async fn delete_minio_object_batch(
    storage: &MinioClient,
    ctx: &OrgContext,
    object_keys: &[String],
) -> Result<(), DeletionError> {
    for raw_key in object_keys {
        let key = parse_key_for_org(raw_key, ctx.org_id())?;
        match storage.delete_object(ctx.org_id(), &key).await {
            Ok(()) | Err(StorageError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn write_object_cleanup_audit(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    phase: &'static str,
    object_keys: &[String],
) -> Result<(), DeletionError> {
    let outcome = match phase {
        "error" => "error",
        _ => "success",
    };
    let object_keys = object_keys.to_vec();
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let resource_id = document_id.to_string();
                let request_id = crate::services::audit::request_id_from_correlation();
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: "document.purge_objects",
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome,
                        request_id: &request_id,
                        metadata: json!({
                            "document_id": document_id,
                            "phase": phase,
                            "object_count": object_keys.len(),
                        }),
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
}

async fn delete_chunks(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<u64, DeletionError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move { Ok(chunks::delete_by_document(txn, &ctx, document_id).await?) })
        }
    })
    .await
}

async fn finalize_purged(
    pool: &Pool,
    ctx: &OrgContext,
    input: PurgeDocumentInput<'_>,
    document_id: Uuid,
    deleted_chunks: u64,
) -> Result<Job, DeletionError> {
    let lease_token = input.lease_token.to_string();
    let job_id = input.job.id;
    let attempts = input.attempts;
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                // Quiesce fence: cancel any resurrected writers and refuse Purged
                // while cleanup intents or active writers remain.
                let cancelled = repo::cancel_active_writer_jobs(txn, &ctx, document_id).await?;
                for cancelled in &cancelled {
                    crate::services::indexing::handle_terminal_index_job(txn, &ctx, cancelled)
                        .await
                        .map_err(DeletionError::Job)?;
                }
                let active_writers = repo::has_active_writer_job(txn, &ctx, document_id).await?;
                let open_intents =
                    vector_cleanup_intents::has_open_for_document(txn, &ctx, document_id).await?;
                if !writers_are_quiesced(active_writers, open_intents) {
                    return Err(DeletionError::WritersNotQuiesced);
                }
                document_state::apply_transition(
                    txn,
                    &ctx,
                    document_id,
                    DocumentState::Tombstoned,
                    DocumentState::Purged,
                )
                .await?;
                let completed = repo::complete_owned(txn, &ctx, job_id, &lease_token, attempts)
                    .await?
                    .ok_or(DeletionError::Job(JobError::LeaseLost))?;
                write_job_succeeded_event(txn, &ctx, &completed).await?;
                let resource_id = document_id.to_string();
                let request_id = crate::services::audit::request_id_from_correlation();
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: "document.purge",
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome: "success",
                        request_id: &request_id,
                        metadata: json!({
                            "document_id": document_id,
                            "job_id": job_id,
                            "deleted_chunks": deleted_chunks,
                        }),
                    },
                )
                .await?;
                Ok(completed)
            })
        }
    })
    .await
}

async fn verify_claimed_job(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    attempts: i32,
) -> Result<Job, DeletionError> {
    repo::get_by_id_for_update(txn, ctx, job_id)
        .await?
        .filter(|job| {
            job.status == JobStatus::Leased
                && job.lease_owner.as_deref() == Some(lease_token)
                && job.attempts == attempts
        })
        .ok_or(DeletionError::Job(JobError::LeaseLost))
}

async fn write_job_succeeded_event(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job: &Job,
) -> Result<(), DeletionError> {
    let payload = validated_event_payload(EventPayload {
        job_id: Some(job.id),
        document_id: job.document_id,
        version_id: job.version_id,
        outbox_event_id: None,
    })?;
    repo::append_event_and_outbox(
        txn,
        ctx,
        repo::NewEventLogEntry {
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
) -> Result<repo::ValidatedEventPayload, DeletionError> {
    let value: JsonValue = payload.to_json().map_err(DeletionError::Job)?;
    repo::ValidatedEventPayload::new(value)
        .map_err(|error| DeletionError::Job(JobError::InvalidPayload(error.to_string())))
}

fn checkpoint_offset(job: &Job) -> Result<u64, DeletionError> {
    let Some(value) = job.checkpoint.clone() else {
        return Ok(0);
    };
    let checkpoint = serde_json::from_value::<CheckpointPayload>(value)
        .map_err(|_| DeletionError::InvalidCheckpoint)?;
    Ok(checkpoint.offset.unwrap_or(0))
}

async fn save_checkpoint(
    pool: &Pool,
    ctx: &OrgContext,
    input: PurgeDocumentInput<'_>,
    offset: u64,
) -> Result<(), DeletionError> {
    jobs::checkpoint(
        pool,
        ctx,
        input.job.id,
        input.lease_token,
        input.attempts,
        CheckpointPayload {
            offset: Some(offset),
            ..CheckpointPayload::default()
        },
    )
    .await?;
    Ok(())
}

async fn heartbeat_while<T, Fut>(
    pool: &Pool,
    ctx: &OrgContext,
    input: PurgeDocumentInput<'_>,
    future: Fut,
) -> Result<T, DeletionError>
where
    Fut: std::future::Future<Output = Result<T, DeletionError>>,
{
    jobs::heartbeat_while_claimed(heartbeat_claim(pool, ctx, input), future).await
}

fn check_deadline(deadline: TokioInstant) -> Result<(), DeletionError> {
    if TokioInstant::now() >= deadline {
        Err(DeletionError::JobTimedOut)
    } else {
        Ok(())
    }
}

fn heartbeat_claim<'a>(
    pool: &'a Pool,
    ctx: &'a OrgContext,
    input: PurgeDocumentInput<'a>,
) -> HeartbeatClaim<'a> {
    HeartbeatClaim {
        db_pool: pool,
        ctx,
        job_id: input.job.id,
        lease_token: input.lease_token,
        attempts: input.attempts,
        lease_ttl: input.lease_ttl,
        heartbeat_interval: input.heartbeat_interval,
        deadline: input.deadline,
    }
}

#[derive(Debug, Error)]
pub enum DeletionError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("delete payload is missing document_id")]
    InvalidPayload,
    #[error("delete checkpoint is invalid")]
    InvalidCheckpoint,
    #[error("document is in unexpected state {0:?}")]
    UnexpectedState(DocumentState),
    #[error("delete job exceeded configured maximum duration")]
    JobTimedOut,
    #[error("writers or cleanup intents are not quiesced for purge")]
    WritersNotQuiesced,
}

impl From<HeartbeatError> for DeletionError {
    fn from(value: HeartbeatError) -> Self {
        match value {
            HeartbeatError::Job(error) => Self::Job(error),
            HeartbeatError::TimedOut => Self::JobTimedOut,
        }
    }
}

impl DeletionError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "delete database error",
            Self::Job(_) => "delete job error",
            Self::Storage(_) => "delete storage error",
            Self::InvalidPayload => "delete payload invalid",
            Self::InvalidCheckpoint => "delete checkpoint invalid",
            Self::UnexpectedState(_) => "delete document state invalid",
            Self::JobTimedOut => "delete job timed out",
            Self::WritersNotQuiesced => "delete writers not quiesced",
        }
    }

    pub fn is_retryable_job_failure(&self) -> bool {
        !matches!(self, Self::Job(JobError::LeaseLost))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::vector_cleanup_intents::{
        apply_intent_event, IntentEvent, IntentTransitionError, VectorCleanupIntentStatus,
    };

    #[test]
    fn quiesce_fence_blocks_purge_while_writers_or_intents_remain() {
        assert!(writers_are_quiesced(false, false));
        assert!(!writers_are_quiesced(true, false));
        assert!(!writers_are_quiesced(false, true));
        assert!(!writers_are_quiesced(true, true));
    }

    #[test]
    fn tombstone_suppresses_reads_immediately() {
        assert!(document_reads_suppressed(DocumentState::Tombstoned, true));
        assert!(document_reads_suppressed(DocumentState::Purged, true));
        assert!(document_reads_suppressed(DocumentState::Indexed, true));
        assert!(!document_reads_suppressed(DocumentState::Indexed, false));
    }

    #[test]
    fn purge_finalization_waits_for_open_intent_then_allows_cleaned() {
        let writing =
            apply_intent_event(VectorCleanupIntentStatus::Pending, IntentEvent::BeginWrite)
                .unwrap();
        assert!(writing.blocks_purge());
        assert!(!writers_are_quiesced(false, writing.blocks_purge()));
        let cleaned = apply_intent_event(writing, IntentEvent::MarkCleaned).unwrap();
        assert!(!cleaned.blocks_purge());
        assert!(writers_are_quiesced(false, cleaned.blocks_purge()));
        assert_eq!(
            apply_intent_event(cleaned, IntentEvent::MarkCommitted),
            Err(IntentTransitionError::AlreadyCleaned)
        );
    }
}
