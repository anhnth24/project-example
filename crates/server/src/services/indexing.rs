//! Index job bridge and indexing orchestration.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, RUNTIME_LOCAL_HASH};
use fileconv_knowledge::identity::BODY_TEXT_VERSION;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{
    DocumentState, EmbeddingRuntimePath, EventLogEntry, Job, JobStatus, JobType, OutboxEvent,
};
use crate::db::pool::with_org_txn_typed;
use crate::db::{
    chunks, claims as claim_repo, document_versions, documents, embedding_batches, index_metadata,
    jobs as repo,
};
use crate::jobs::{
    self, CheckpointPayload, EnqueueJob, EventPayload, JobError, JobPayload,
    CURRENT_EVENT_PAYLOAD_VERSION,
};
use crate::services::chunking::{prepare_chunks, PreparedChunk};
use crate::services::claims::{extract_typed_claims, ClaimValue};
use crate::services::document_state;
use crate::services::embedding::{self, EmbeddingError};
use crate::storage::keys::{authorize_key_for_version, parse_key_for_org};
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;
use crate::storage::{ObjectNamespace, StorageError};

#[derive(Debug)]
pub struct IndexingOutboxSink {
    generation: EnsureGenerationOwned,
}

impl IndexingOutboxSink {
    pub fn new(embedding_plan: &EmbeddingPlan) -> Result<Self, IndexingError> {
        Ok(Self {
            generation: EnsureGenerationOwned::from_embedding_plan(embedding_plan)?,
        })
    }
}

impl jobs::OutboxSink for IndexingOutboxSink {
    fn publish<'a>(
        &'a self,
        txn: &'a tokio_postgres::Transaction<'_>,
        ctx: &'a OrgContext,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<EventLogEntry, JobError>> + Send + 'a>> {
        Box::pin(async move {
            if event.event_type == "document.index_requested" {
                let payload = serde_json::from_value::<EventPayload>(event.payload.clone())
                    .map_err(|error| {
                        JobError::InvalidPayload(format!("event decode failed: {error}"))
                    })?;
                let document_id = payload.document_id.ok_or_else(|| {
                    JobError::InvalidPayload("index event missing document_id".into())
                })?;
                let version_id = payload.version_id.ok_or_else(|| {
                    JobError::InvalidPayload("index event missing version_id".into())
                })?;
                let document = documents::get_by_id(txn, ctx, document_id).await?;
                let metadata = index_metadata::ensure_active_generation(
                    txn,
                    ctx,
                    self.generation
                        .as_input_for_collection(Some(document.collection_id)),
                )
                .await?;
                let job_payload = JobPayload {
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    index_metadata_id: Some(metadata.id),
                    ..JobPayload::default()
                };
                jobs::enqueue_within_txn(
                    txn,
                    ctx,
                    EnqueueJob::new(
                        JobType::Index,
                        job_payload,
                        index_job_idempotency_key(metadata.id, version_id),
                    ),
                )
                .await?;
            }

            append_outbox_published(txn, ctx, event).await
        })
    }
}

/// One version can have one index job per immutable embedding generation.
///
/// The relay and staged-backfill paths must derive this key identically:
/// otherwise a normal index request and a staged request for the same target
/// generation can run concurrently and each create a different parent job.
fn index_job_idempotency_key(index_metadata_id: Uuid, version_id: Uuid) -> String {
    format!("index:{index_metadata_id}:{version_id}")
}

async fn append_outbox_published(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    event: &OutboxEvent,
) -> Result<EventLogEntry, JobError> {
    if let Some(existing) = repo::find_outbox_published_event(txn, ctx, event.id).await? {
        return Ok(existing);
    }
    let payload = EventPayload {
        job_id: event.job_id,
        document_id: None,
        version_id: None,
        outbox_event_id: Some(event.id),
    }
    .to_json()?;
    let payload = repo::ValidatedEventPayload::new(payload)
        .map_err(|error| JobError::InvalidPayload(error.to_string()))?;
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexVersionOutcome {
    Finalized { job_id: Uuid, chunks: usize },
    CompleteOnly { chunks: usize },
    AlreadyIndexed,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexVersionInput<'a> {
    pub job: &'a Job,
    pub lease_token: &'a str,
    pub attempts: i32,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub embedding_batch_size: usize,
    pub approved_signature: Option<&'a str>,
    pub embedding_plan: &'a EmbeddingPlan,
    pub deadline: TokioInstant,
}

pub async fn index_version(
    db_pool: &Pool,
    storage: &MinioClient,
    _qdrant: &QdrantClient,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
) -> Result<IndexVersionOutcome, IndexingError> {
    if input.embedding_plan.runtime_path() == RUNTIME_LOCAL_HASH {
        return Err(IndexingError::UnapprovedEmbeddingRuntime);
    }
    let payload = jobs::decode_job_payload(input.job.payload_version, input.job.payload.clone())?;
    let document_id = payload.document_id.ok_or(IndexingError::InvalidPayload)?;
    let version_id = payload.version_id.ok_or(IndexingError::InvalidPayload)?;
    let (document, version, artifact) =
        load_index_source(db_pool, ctx, document_id, version_id).await?;
    let is_current = document.current_version_id == Some(version_id);

    let trusted_key = parse_key_for_org(&artifact.object_key, ctx.org_id())?;
    if trusted_key.namespace() != ObjectNamespace::Trusted {
        return Err(IndexingError::MarkdownNotTrusted);
    }
    authorize_key_for_version(&trusted_key, version_id)
        .map_err(|_| IndexingError::MarkdownKeyVersionMismatch)?;
    let bytes = storage.get_object(ctx.org_id(), &trusted_key).await?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != artifact.content_sha256 {
        return Err(IndexingError::MarkdownIntegrity);
    }
    let markdown = String::from_utf8(bytes.to_vec()).map_err(|_| IndexingError::MarkdownUtf8)?;
    let prepared_chunks = prepare_chunks(document_id, version_id, &markdown);

    let dimensions = input
        .embedding_plan
        .expected_dimensions()
        .ok_or(IndexingError::EmbeddingDimensionsUnknown)?;
    let signature = input.embedding_plan.index_signature(dimensions)?;
    let signature_digest = signature.digest();
    if let Some(approved) = input.approved_signature {
        if approved != signature_digest {
            return Err(IndexingError::SignatureMismatch);
        }
    }
    let runtime_path =
        EmbeddingRuntimePath::parse(signature.runtime_path).map_err(DbError::Config)?;
    let dimensions = i32::try_from(signature.dimensions).map_err(|_| {
        DbError::Config("embedding dimensions are out of range for database".into())
    })?;
    let ensured_metadata = ensure_generation(
        db_pool,
        ctx,
        index_metadata::EnsureGeneration {
            collection_id: Some(document.collection_id),
            signature_sha256: &signature_digest,
            chunking_version: signature.chunking_version,
            body_text_version: signature.body_text_version,
            query_normalization_version: signature.query_normalization_version,
            embedding_family: signature.embedding_family,
            embedding_revision: signature.embedding_revision,
            dimensions,
            normalized: signature.normalized,
            runtime_path,
        },
    )
    .await?;
    let metadata = if let Some(target_metadata_id) = payload.index_metadata_id {
        let target = load_generation(db_pool, ctx, target_metadata_id).await?;
        if target.index_signature_sha256 != signature_digest {
            return Err(IndexingError::SignatureMismatch);
        }
        target
    } else {
        ensured_metadata
    };
    if metadata.state == crate::db::models::IndexGenerationState::Building {
        enqueue_staged_backfill(db_pool, ctx, input, &metadata, version_id).await?;
    }
    heartbeat_once(db_pool, ctx, input).await?;

    if is_current {
        match document.state {
            DocumentState::Converted => {
                transition_current_to_indexing(db_pool, ctx, input, document_id, version_id)
                    .await?;
            }
            DocumentState::Indexing | DocumentState::Indexed => {}
            other => return Err(IndexingError::UnexpectedDocumentState(other)),
        }
    }

    let mut offset = checkpoint_offset(input.job)?;
    if offset > prepared_chunks.len() {
        return Err(IndexingError::InvalidCheckpoint);
    }
    while offset < prepared_chunks.len() {
        check_deadline(input.deadline)?;
        let batch_end = offset
            .saturating_add(input.embedding_batch_size)
            .min(prepared_chunks.len());
        let batch = &prepared_chunks[offset..batch_end];
        heartbeat_while(db_pool, ctx, input, async {
            persist_chunk_batch(
                db_pool,
                ctx,
                PersistBatchInput {
                    claim: input,
                    metadata_id: metadata.id,
                    signature_digest: &signature_digest,
                    document_id,
                    version_id,
                    batch,
                    batch_start: offset,
                    batch_end,
                    effective_from: version.effective_from,
                    effective_to: version.effective_to,
                },
            )
            .await?;
            Ok::<(), IndexingError>(())
        })
        .await?;
        offset = batch_end;
    }

    if prepared_chunks.is_empty() {
        mark_empty_backfill_complete(db_pool, ctx, input, &metadata, document_id, version_id)
            .await?;
    }
    let job = finalize_indexed(db_pool, ctx, input, metadata.id, document_id, version_id).await?;
    Ok(IndexVersionOutcome::Finalized {
        job_id: job.id,
        chunks: prepared_chunks.len(),
    })
}

async fn load_index_source(
    db_pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<
    (
        crate::db::models::Document,
        crate::db::models::DocumentVersion,
        document_versions::ArtifactInsertOutcome,
    ),
    IndexingError,
> {
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let version = document_versions::find_by_id(txn, &ctx, document_id, version_id)
                    .await?
                    .ok_or(DbError::NotFound)?;
                let artifact = document_versions::find_markdown_artifact(txn, &ctx, version_id)
                    .await?
                    .ok_or(DbError::NotFound)?;
                Ok::<_, IndexingError>((document, version, artifact))
            })
        }
    })
    .await
}

async fn ensure_generation(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: index_metadata::EnsureGeneration<'_>,
) -> Result<crate::db::models::IndexMetadata, IndexingError> {
    let owned = EnsureGenerationOwned::from(input);
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, IndexingError>(
                    index_metadata::ensure_active_generation(txn, &ctx, owned.as_input()).await?,
                )
            })
        }
    })
    .await
}

async fn load_generation(
    db_pool: &Pool,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<crate::db::models::IndexMetadata, IndexingError> {
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                index_metadata::find_by_id(txn, &ctx, metadata_id)
                    .await?
                    .ok_or(DbError::NotFound)
                    .map_err(IndexingError::from)
            })
        }
    })
    .await
}

/// Expands a signature change into durable index jobs for every current version
/// in the collection. The active generation stays unchanged; this only fills
/// the immutable staging generation.
async fn enqueue_staged_backfill(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
    metadata: &crate::db::models::IndexMetadata,
    current_version_id: Uuid,
) -> Result<(), IndexingError> {
    let metadata_id = metadata.id;
    let collection_id = metadata
        .collection_id
        .ok_or(IndexingError::MissingCollection)?;
    let job_id = input.job.id;
    let lease_token = input.lease_token.to_string();
    let attempts = input.attempts;
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                let targets = embedding_batches::seed_generation_backfills(
                    txn,
                    &ctx,
                    metadata_id,
                    collection_id,
                )
                .await?;
                for (document_id, version_id) in targets {
                    if version_id == current_version_id {
                        continue;
                    }
                    jobs::enqueue_within_txn(
                        txn,
                        &ctx,
                        EnqueueJob::new(
                            JobType::Index,
                            JobPayload {
                                document_id: Some(document_id),
                                version_id: Some(version_id),
                                index_metadata_id: Some(metadata_id),
                                ..JobPayload::default()
                            },
                            index_job_idempotency_key(metadata_id, version_id),
                        ),
                    )
                    .await?;
                }
                Ok::<_, IndexingError>(())
            })
        }
    })
    .await
}

async fn mark_empty_backfill_complete(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
    metadata: &crate::db::models::IndexMetadata,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), IndexingError> {
    let metadata_id = metadata.id;
    let lease_token = input.lease_token.to_string();
    let job_id = input.job.id;
    let attempts = input.attempts;
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                embedding_batches::mark_generation_backfilled(
                    txn,
                    &ctx,
                    metadata_id,
                    document_id,
                    version_id,
                )
                .await?;
                mark_generation_shadow_if_complete(txn, &ctx, metadata_id).await?;
                let document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                if document.current_version_id == Some(version_id)
                    && document.state == DocumentState::Indexing
                {
                    document_state::apply_transition(
                        txn,
                        &ctx,
                        document_id,
                        DocumentState::Indexing,
                        DocumentState::Indexed,
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
}

pub(crate) async fn mark_generation_shadow_if_complete(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<(), IndexingError> {
    lock_generation_completion(txn, ctx, metadata_id).await?;
    if embedding_batches::generation_backfill_complete(txn, ctx, metadata_id).await? {
        let _ = index_metadata::mark_shadow(txn, ctx, metadata_id).await?;
    }
    Ok(())
}

/// Serializes the final "all backfills complete" observation across documents.
/// Each worker may already hold a distinct document row lock; this generation
/// lock prevents two finalizers from both missing the other's uncommitted
/// `backfilled` transition.
async fn lock_generation_completion(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<(), DbError> {
    let key = format!("index_generation_completion:{}:{metadata_id}", ctx.org_id());
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&key])
        .await?;
    Ok(())
}

/// Completes one document's generation backfill only after the parent index job
/// has published its full batch set and every batch has succeeded.
pub(crate) async fn complete_document_backfill_if_ready(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<bool, IndexingError> {
    // The document row is the authoritative completion gate. An index job and
    // multiple embedding jobs can all make the last-successful transition.
    // Locking before observing batch/index-job state serializes those checks,
    // so a concurrent finalizer cannot each observe another uncommitted batch
    // and leave the generation permanently in `indexing`.
    let document = documents::get_by_id_for_update(txn, ctx, document_id).await?;
    if !embedding_batches::document_batches_complete(txn, ctx, metadata_id, document_id, version_id)
        .await?
    {
        return Ok(false);
    }

    embedding_batches::mark_generation_backfilled(txn, ctx, metadata_id, document_id, version_id)
        .await?;
    mark_generation_shadow_if_complete(txn, ctx, metadata_id).await?;

    if document.current_version_id == Some(version_id) && document.state == DocumentState::Indexing
    {
        document_state::apply_transition(
            txn,
            ctx,
            document_id,
            DocumentState::Indexing,
            DocumentState::Indexed,
        )
        .await?;
    }
    Ok(true)
}

/// Operator/verification-gated cutover. Callers must validate shadow retrieval
/// and citation evidence before making the staged generation visible.
pub async fn cut_over_shadow_generation(
    db_pool: &Pool,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<crate::db::models::IndexMetadata, IndexingError> {
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                index_metadata::cut_over_shadow_generation(txn, &ctx, metadata_id)
                    .await
                    .map_err(IndexingError::from)
            })
        }
    })
    .await
}

async fn transition_current_to_indexing(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), IndexingError> {
    let lease_token = input.lease_token.to_string();
    let job_id = input.job.id;
    let attempts = input.attempts;
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                let document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                if document.current_version_id != Some(version_id) {
                    return Err(IndexingError::CurrentVersionChanged);
                }
                document_state::apply_transition(
                    txn,
                    &ctx,
                    document_id,
                    DocumentState::Converted,
                    DocumentState::Indexing,
                )
                .await?;
                Ok::<(), IndexingError>(())
            })
        }
    })
    .await
}

pub async fn finalize_indexed(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
    metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<Job, IndexingError> {
    let lease_token = input.lease_token.to_string();
    let job_id = input.job.id;
    let attempts = input.attempts;
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                let completed = repo::complete_owned(txn, &ctx, job_id, &lease_token, attempts)
                    .await?
                    .ok_or(IndexingError::Job(JobError::LeaseLost))?;
                write_job_succeeded_event(txn, &ctx, &completed).await?;
                complete_document_backfill_if_ready(
                    txn,
                    &ctx,
                    metadata_id,
                    document_id,
                    version_id,
                )
                .await?;
                Ok::<_, IndexingError>(completed)
            })
        }
    })
    .await
}

/// Fails an index-related job and, only when it has exhausted its retry budget,
/// moves its still-current document to `failed` in the *same* transaction.
/// This prevents a terminal job/document split-brain state.
pub async fn fail_index_job(
    db_pool: &Pool,
    ctx: &OrgContext,
    job: &Job,
    lease_token: &str,
    attempts: i32,
    last_error: &str,
) -> Result<Job, IndexingError> {
    let job_id = job.id;
    let lease_token = lease_token.to_string();
    let last_error = last_error.to_string();
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let failed =
                    jobs::fail_within_txn(txn, &ctx, job_id, &lease_token, attempts, &last_error)
                        .await?;
                if failed.status != JobStatus::DeadLetter {
                    return Ok::<_, IndexingError>(failed);
                }
                handle_dead_letter_index_job(txn, &ctx, &failed).await?;
                Ok(failed)
            })
        }
    })
    .await
}

/// Applies the durable failure side effects for a terminal index or embedding
/// job. This is shared by owned-worker failures and lease reclamation so they
/// cannot leave a dead-letter job with a still-indexing document or backfill.
pub(crate) async fn handle_dead_letter_index_job(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job: &Job,
) -> Result<(), JobError> {
    if !matches!(job.job_type, JobType::Index | JobType::EmbeddingBatch) {
        return Ok(());
    }

    let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
    if let Some(batch_id) = payload.batch_id {
        let batch = embedding_batches::mark_failed(txn, ctx, batch_id).await?;
        embedding_batches::mark_generation_failed(
            txn,
            ctx,
            batch.index_metadata_id,
            batch.document_id,
            batch.version_id,
        )
        .await?;
    } else if let (Some(metadata_id), Some(document_id), Some(version_id)) = (
        payload.index_metadata_id,
        payload.document_id,
        payload.version_id,
    ) {
        embedding_batches::mark_generation_failed(txn, ctx, metadata_id, document_id, version_id)
            .await?;
    }

    if let (Some(document_id), Some(version_id)) = (job.document_id, job.version_id) {
        let document = documents::get_by_id_for_update(txn, ctx, document_id).await?;
        if document.current_version_id == Some(version_id)
            && should_fail_current_document(document.state)
        {
            document_state::apply_transition(
                txn,
                ctx,
                document_id,
                document.state,
                DocumentState::Failed,
            )
            .await?;
        }
    }
    Ok(())
}

fn should_fail_current_document(state: DocumentState) -> bool {
    matches!(state, DocumentState::Converted | DocumentState::Indexing)
}

async fn verify_claimed_job(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    attempts: i32,
) -> Result<Job, IndexingError> {
    repo::get_by_id_for_update(txn, ctx, job_id)
        .await?
        .filter(|job| {
            job.status == JobStatus::Leased
                && job.lease_owner.as_deref() == Some(lease_token)
                && job.attempts == attempts
        })
        .ok_or(IndexingError::Job(JobError::LeaseLost))
}

async fn write_job_succeeded_event(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job: &Job,
) -> Result<(), IndexingError> {
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
) -> Result<repo::ValidatedEventPayload, IndexingError> {
    let value: JsonValue = payload.to_json().map_err(IndexingError::Job)?;
    repo::ValidatedEventPayload::new(value)
        .map_err(|error| IndexingError::Job(JobError::InvalidPayload(error.to_string())))
}

#[derive(Debug)]
struct EnsureGenerationOwned {
    collection_id: Option<Uuid>,
    signature_sha256: String,
    chunking_version: String,
    body_text_version: String,
    query_normalization_version: String,
    embedding_family: String,
    embedding_revision: String,
    dimensions: i32,
    normalized: bool,
    runtime_path: EmbeddingRuntimePath,
}

impl From<index_metadata::EnsureGeneration<'_>> for EnsureGenerationOwned {
    fn from(input: index_metadata::EnsureGeneration<'_>) -> Self {
        Self {
            collection_id: input.collection_id,
            signature_sha256: input.signature_sha256.to_string(),
            chunking_version: input.chunking_version.to_string(),
            body_text_version: input.body_text_version.to_string(),
            query_normalization_version: input.query_normalization_version.to_string(),
            embedding_family: input.embedding_family.to_string(),
            embedding_revision: input.embedding_revision.to_string(),
            dimensions: input.dimensions,
            normalized: input.normalized,
            runtime_path: input.runtime_path,
        }
    }
}

impl EnsureGenerationOwned {
    fn from_embedding_plan(embedding_plan: &EmbeddingPlan) -> Result<Self, IndexingError> {
        let dimensions = embedding_plan
            .expected_dimensions()
            .ok_or(IndexingError::EmbeddingDimensionsUnknown)?;
        let signature = embedding_plan.index_signature(dimensions)?;
        let dimensions = i32::try_from(signature.dimensions).map_err(|_| {
            DbError::Config("embedding dimensions are out of range for database".into())
        })?;
        let runtime_path =
            EmbeddingRuntimePath::parse(signature.runtime_path).map_err(DbError::Config)?;
        Ok(Self {
            collection_id: None,
            signature_sha256: signature.digest(),
            chunking_version: signature.chunking_version.to_string(),
            body_text_version: signature.body_text_version.to_string(),
            query_normalization_version: signature.query_normalization_version.to_string(),
            embedding_family: signature.embedding_family.to_string(),
            embedding_revision: signature.embedding_revision.to_string(),
            dimensions,
            normalized: signature.normalized,
            runtime_path,
        })
    }

    fn as_input(&self) -> index_metadata::EnsureGeneration<'_> {
        self.as_input_for_collection(self.collection_id)
    }

    fn as_input_for_collection(
        &self,
        collection_id: Option<Uuid>,
    ) -> index_metadata::EnsureGeneration<'_> {
        index_metadata::EnsureGeneration {
            collection_id,
            signature_sha256: &self.signature_sha256,
            chunking_version: &self.chunking_version,
            body_text_version: &self.body_text_version,
            query_normalization_version: &self.query_normalization_version,
            embedding_family: &self.embedding_family,
            embedding_revision: &self.embedding_revision,
            dimensions: self.dimensions,
            normalized: self.normalized,
            runtime_path: self.runtime_path,
        }
    }
}

struct PersistBatchInput<'a> {
    claim: IndexVersionInput<'a>,
    metadata_id: Uuid,
    signature_digest: &'a str,
    document_id: Uuid,
    version_id: Uuid,
    batch: &'a [PreparedChunk],
    batch_start: usize,
    batch_end: usize,
    effective_from: chrono::DateTime<chrono::Utc>,
    effective_to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn persist_chunk_batch(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: PersistBatchInput<'_>,
) -> Result<(), IndexingError> {
    let batch = input.batch.to_vec();
    let signature_digest = input.signature_digest.to_string();
    let metadata_id = input.metadata_id;
    let batch_start =
        i32::try_from(input.batch_start).map_err(|_| IndexingError::CheckpointOffset)?;
    let batch_end = u64::try_from(input.batch_end).map_err(|_| IndexingError::CheckpointOffset)?;
    let batch_end_ordinal =
        i32::try_from(input.batch_end).map_err(|_| IndexingError::CheckpointOffset)?;
    let lease_token = input.claim.lease_token.to_string();
    let job_id = input.claim.job.id;
    let attempts = input.claim.attempts;
    let document_id = input.document_id;
    let version_id = input.version_id;
    let effective_from = input.effective_from;
    let effective_to = input.effective_to;
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                for chunk in &batch {
                    let persisted_chunk = chunks::insert_if_absent(
                        txn,
                        &ctx,
                        chunks::NewChunk {
                            id: Uuid::new_v4(),
                            document_id,
                            version_id,
                            ordinal: chunk.ordinal,
                            heading_path: &chunk.heading_path,
                            body: &chunk.body,
                            body_text_version: BODY_TEXT_VERSION,
                            chunk_identity_sha256: &chunk.chunk_identity,
                            index_metadata_id: metadata_id,
                            index_signature: &signature_digest,
                        },
                    )
                    .await?;
                    persist_claims_for_chunk(
                        txn,
                        &ctx,
                        job_id,
                        document_id,
                        version_id,
                        persisted_chunk.id,
                        effective_from,
                        effective_to,
                        chunk,
                    )
                    .await?;
                }
                let inputs = batch
                    .iter()
                    .map(|chunk| {
                        embedding::ApprovedEmbeddingRuntime::canonical_input(
                            &chunk.heading_joined,
                            &chunk.body,
                        )
                    })
                    .collect::<Vec<_>>();
                let input_sha256 = embedding::canonical_inputs_sha256(&inputs);
                let batch_id = Uuid::new_v4();
                let embedding_job = EnqueueJob::new(
                    JobType::EmbeddingBatch,
                    JobPayload {
                        document_id: Some(document_id),
                        version_id: Some(version_id),
                        batch_id: Some(batch_id),
                        index_metadata_id: Some(metadata_id),
                        ..JobPayload::default()
                    },
                    format!(
                        "embedding:{metadata_id}:{version_id}:{batch_start}:{batch_end_ordinal}"
                    ),
                );
                let outcome = jobs::enqueue_within_txn(txn, &ctx, embedding_job).await?;
                if outcome.created {
                    embedding_batches::insert(
                        txn,
                        &ctx,
                        embedding_batches::NewEmbeddingBatch {
                            id: batch_id,
                            index_job_id: job_id,
                            job_id: outcome.job.id,
                            index_metadata_id: metadata_id,
                            document_id,
                            version_id,
                            start_ordinal: batch_start,
                            end_ordinal: batch_end_ordinal,
                            input_sha256: &input_sha256,
                        },
                    )
                    .await?;
                } else {
                    let existing = embedding_batches::find_by_job_id(txn, &ctx, outcome.job.id)
                        .await?
                        .ok_or(IndexingError::EmbeddingBatchMissing)?;
                    if existing.index_metadata_id != metadata_id
                        || existing.document_id != document_id
                        || existing.version_id != version_id
                        || existing.start_ordinal != batch_start
                        || existing.end_ordinal != batch_end_ordinal
                        || existing.input_sha256 != input_sha256
                    {
                        return Err(IndexingError::EmbeddingBatchMismatch);
                    }
                }
                embedding_batches::mark_generation_indexing(
                    txn,
                    &ctx,
                    metadata_id,
                    document_id,
                    version_id,
                )
                .await?;
                jobs::checkpoint_within_txn(
                    txn,
                    &ctx,
                    job_id,
                    &lease_token,
                    attempts,
                    CheckpointPayload {
                        offset: Some(batch_end),
                        ..CheckpointPayload::default()
                    },
                )
                .await?;
                Ok::<(), IndexingError>(())
            })
        }
    })
    .await
}

async fn persist_claims_for_chunk(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    chunk_id: Uuid,
    effective_from: chrono::DateTime<chrono::Utc>,
    effective_to: Option<chrono::DateTime<chrono::Utc>>,
    chunk: &PreparedChunk,
) -> Result<(), IndexingError> {
    for claim in extract_typed_claims(&chunk.body, version_id, &chunk.chunk_identity) {
        let (value_number, value_text, value_boolean, value_date, value_money) =
            claim_value_fields(&claim.value);
        let input = claim_repo::NewClaim {
            id: claim.id,
            document_id,
            version_id,
            chunk_id,
            claim_key: &claim.claim_key,
            subject: &claim.subject,
            predicate: &claim.predicate,
            value_type: claim.value.value_type(),
            value_number,
            value_text,
            value_boolean,
            value_date,
            value_money,
            unit: claim.unit.as_deref(),
            scope: &claim.scope,
            effective_from,
            effective_to,
            citation_quote: &claim.citation_quote,
            citation_span_start: claim.citation_span_start,
            citation_span_end: claim.citation_span_end,
        };
        let claim_id = claim_repo::insert_if_absent(txn, ctx, &input).await?;
        for candidate_id in claim_repo::find_conflict_candidates(txn, ctx, &input).await? {
            enqueue_conflict_candidate(
                txn,
                ctx,
                job_id,
                document_id,
                version_id,
                claim_id,
                candidate_id,
            )
            .await?;
        }
    }
    Ok(())
}

fn claim_value_fields(
    value: &ClaimValue,
) -> (
    Option<rust_decimal::Decimal>,
    Option<&str>,
    Option<bool>,
    Option<chrono::NaiveDate>,
    Option<rust_decimal::Decimal>,
) {
    match value {
        ClaimValue::Number(value) => (Some(*value), None, None, None, None),
        ClaimValue::Money(value) => (None, None, None, None, Some(*value)),
        ClaimValue::Boolean(value) => (None, None, Some(*value), None, None),
        ClaimValue::Date(value) => (None, None, None, Some(*value), None),
        ClaimValue::Enum(value) | ClaimValue::Text(value) => {
            (None, Some(value.as_str()), None, None, None)
        }
    }
}

async fn enqueue_conflict_candidate(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    first_claim_id: Uuid,
    second_claim_id: Uuid,
) -> Result<(), IndexingError> {
    let (claim_a_id, claim_b_id) = if first_claim_id < second_claim_id {
        (first_claim_id, second_claim_id)
    } else {
        (second_claim_id, first_claim_id)
    };
    let payload = repo::ValidatedEventPayload::new(serde_json::json!({
        "claim_a_id": claim_a_id,
        "claim_b_id": claim_b_id,
        "document_id": document_id,
        "version_id": version_id,
    }))?;
    repo::insert_outbox_event(
        txn,
        ctx,
        repo::NewOutboxEvent {
            event_type: "claim.conflict_candidate",
            payload_version: jobs::CURRENT_EVENT_PAYLOAD_VERSION,
            payload: &payload,
            idempotency_key: &format!("claim.conflict_candidate:{claim_a_id}:{claim_b_id}"),
            job_id: Some(job_id),
        },
    )
    .await?;
    Ok(())
}

fn checkpoint_offset(job: &Job) -> Result<usize, IndexingError> {
    let Some(value) = job.checkpoint.clone() else {
        return Ok(0);
    };
    let checkpoint = serde_json::from_value::<CheckpointPayload>(value)
        .map_err(|_| IndexingError::InvalidCheckpoint)?;
    let offset = checkpoint.offset.unwrap_or(0);
    usize::try_from(offset).map_err(|_| IndexingError::CheckpointOffset)
}

fn check_deadline(deadline: TokioInstant) -> Result<(), IndexingError> {
    if TokioInstant::now() >= deadline {
        Err(IndexingError::JobTimedOut)
    } else {
        Ok(())
    }
}

async fn heartbeat_while<T, Fut>(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
    future: Fut,
) -> Result<T, IndexingError>
where
    Fut: Future<Output = Result<T, IndexingError>>,
{
    heartbeat_once(db_pool, ctx, input).await?;
    tokio::pin!(future);
    let mut heartbeat = heartbeat_interval(input.heartbeat_interval);
    loop {
        tokio::select! {
            biased;
            result = &mut future => return result,
            _ = sleep_until(input.deadline) => return Err(IndexingError::JobTimedOut),
            _ = heartbeat.tick() => heartbeat_once(db_pool, ctx, input).await?,
        }
    }
}

async fn heartbeat_once(
    db_pool: &Pool,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
) -> Result<(), IndexingError> {
    match timeout(
        heartbeat_call_timeout(input.lease_ttl, input.deadline),
        jobs::heartbeat(
            db_pool,
            ctx,
            input.job.id,
            input.lease_token,
            input.attempts,
            input.lease_ttl,
        ),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(JobError::LeaseLost)) => Err(IndexingError::Job(JobError::LeaseLost)),
        Ok(Err(error)) => Err(IndexingError::Job(error)),
        Err(_) => Err(IndexingError::JobTimedOut),
    }
}

fn heartbeat_interval(interval: Duration) -> tokio::time::Interval {
    let mut heartbeat = interval_at(TokioInstant::now() + interval, interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat
}

fn heartbeat_call_timeout(lease_ttl: Duration, deadline: TokioInstant) -> Duration {
    let mut lease_bound = lease_ttl / 3;
    if lease_bound.is_zero() {
        lease_bound = Duration::from_millis(1);
    }
    let remaining = deadline.saturating_duration_since(TokioInstant::now());
    remaining.min(lease_bound)
}

#[derive(Debug, Error)]
pub enum IndexingError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("knowledge error")]
    Knowledge(#[from] fileconv_knowledge::KnowledgeError),
    #[error("embedding error")]
    Embedding(#[from] EmbeddingError),
    #[error("index payload is missing document_id or version_id")]
    InvalidPayload,
    #[error("document is in unexpected state {0:?}")]
    UnexpectedDocumentState(DocumentState),
    #[error("markdown artifact key is not trusted")]
    MarkdownNotTrusted,
    #[error("markdown artifact key is bound to another version")]
    MarkdownKeyVersionMismatch,
    #[error("markdown artifact integrity check failed")]
    MarkdownIntegrity,
    #[error("markdown artifact is not utf-8")]
    MarkdownUtf8,
    #[error("configured index signature does not match approved local signature")]
    SignatureMismatch,
    #[error("approved embedding runtime did not declare vector dimensions")]
    EmbeddingDimensionsUnknown,
    #[error("the local hash runtime is not approved for server indexing")]
    UnapprovedEmbeddingRuntime,
    #[error("index checkpoint is invalid")]
    InvalidCheckpoint,
    #[error("checkpoint offset is out of range")]
    CheckpointOffset,
    #[error("chunk ordinal is out of range")]
    ChunkOrdinal,
    #[error("index generation is out of range")]
    IndexGeneration,
    #[error("qdrant collection was not initialized")]
    MissingQdrantCollection,
    #[error("index generation is missing its collection scope")]
    MissingCollection,
    #[error("existing embedding job is missing its durable batch record")]
    EmbeddingBatchMissing,
    #[error("existing embedding batch does not match its immutable range or input")]
    EmbeddingBatchMismatch,
    #[error("document current version changed while indexing")]
    CurrentVersionChanged,
    #[error("index job exceeded configured maximum duration")]
    JobTimedOut,
}

impl IndexingError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "index database error",
            Self::Job(_) => "index job error",
            Self::Storage(_) => "index storage error",
            Self::Knowledge(_) => "index knowledge error",
            Self::Embedding(_) => "index embedding error",
            Self::InvalidPayload => "index payload invalid",
            Self::UnexpectedDocumentState(_) => "index document state invalid",
            Self::MarkdownNotTrusted => "index markdown key invalid",
            Self::MarkdownKeyVersionMismatch => "index markdown key version invalid",
            Self::MarkdownIntegrity => "index markdown integrity failed",
            Self::MarkdownUtf8 => "index markdown utf8 invalid",
            Self::SignatureMismatch => "index signature mismatch",
            Self::EmbeddingDimensionsUnknown => "index embedding dimensions missing",
            Self::UnapprovedEmbeddingRuntime => "index embedding runtime not approved",
            Self::InvalidCheckpoint => "index checkpoint invalid",
            Self::CheckpointOffset => "index checkpoint offset invalid",
            Self::ChunkOrdinal => "index chunk ordinal invalid",
            Self::IndexGeneration => "index generation invalid",
            Self::MissingQdrantCollection => "index qdrant collection missing",
            Self::MissingCollection => "index collection missing",
            Self::EmbeddingBatchMissing => "embedding batch missing",
            Self::EmbeddingBatchMismatch => "embedding batch mismatch",
            Self::CurrentVersionChanged => "index current version changed",
            Self::JobTimedOut => "index job timed out",
        }
    }

    pub fn is_retryable_job_failure(&self) -> bool {
        !matches!(self, Self::Job(JobError::LeaseLost))
    }
}

#[cfg(test)]
mod tests {
    use super::{index_job_idempotency_key, should_fail_current_document};
    use crate::db::models::DocumentState;
    use uuid::Uuid;

    #[test]
    fn staged_backfill_failure_does_not_fail_an_indexed_active_document() {
        assert!(should_fail_current_document(DocumentState::Converted));
        assert!(should_fail_current_document(DocumentState::Indexing));
        assert!(!should_fail_current_document(DocumentState::Indexed));
    }

    #[test]
    fn normal_and_staged_requests_share_a_generation_scoped_key() {
        let version_id = Uuid::new_v4();
        let generation_id = Uuid::new_v4();
        assert_eq!(
            index_job_idempotency_key(generation_id, version_id),
            index_job_idempotency_key(generation_id, version_id)
        );
        assert_ne!(
            index_job_idempotency_key(generation_id, version_id),
            index_job_idempotency_key(Uuid::new_v4(), version_id)
        );
    }
}
