//! Index job bridge and indexing orchestration.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::LOCAL_VECTOR_DIMENSIONS;
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
use crate::db::{chunks, claims as claim_repo, documents, index_metadata, jobs as repo};
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
use crate::storage::qdrant::{ChunkPointPayload, QdrantClient, UpsertPoint, VectorScope};
use crate::storage::{ObjectNamespace, StorageError};
use crate::workers::embedding::{EmbeddingWorker, EmbeddingWorkerError};

#[derive(Debug, Default)]
pub struct IndexingOutboxSink;

impl IndexingOutboxSink {
    pub fn new() -> Self {
        Self
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
                let job_payload = JobPayload {
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    ..JobPayload::default()
                };
                jobs::enqueue_within_txn(
                    txn,
                    ctx,
                    EnqueueJob::new(JobType::Index, job_payload, format!("index:{version_id}")),
                )
                .await?;
            }

            append_outbox_published(txn, ctx, event).await
        })
    }
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
    pub deadline: TokioInstant,
    pub embedding_worker: &'a EmbeddingWorker,
}

pub async fn index_version(
    db_pool: &Pool,
    storage: &MinioClient,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    input: IndexVersionInput<'_>,
) -> Result<IndexVersionOutcome, IndexingError> {
    let payload = jobs::decode_job_payload(input.job.payload_version, input.job.payload.clone())?;
    let document_id = payload.document_id.ok_or(IndexingError::InvalidPayload)?;
    let version_id = payload.version_id.ok_or(IndexingError::InvalidPayload)?;
    let (document, version, artifact) =
        load_index_source(db_pool, ctx, document_id, version_id).await?;
    let is_current = document.current_version_id == Some(version_id);
    let is_effective = version.effective_to.is_none();

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

    let plan = embedding::approved_plan();
    let signature = plan.index_signature(LOCAL_VECTOR_DIMENSIONS)?;
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
    let metadata = ensure_generation(
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
    heartbeat_once(db_pool, ctx, input).await?;

    if is_current {
        match document.state {
            DocumentState::Indexed => return Ok(IndexVersionOutcome::AlreadyIndexed),
            DocumentState::Converted => {
                transition_current_to_indexing(db_pool, ctx, input, document_id, version_id)
                    .await?;
            }
            DocumentState::Indexing => {}
            other => return Err(IndexingError::UnexpectedDocumentState(other)),
        }
    }

    let scope = VectorScope::new(ctx.org_id(), [document.collection_id]);
    let collection_name = if prepared_chunks.is_empty() {
        None
    } else {
        Some(qdrant.ensure_collection_for_signature(&signature).await?)
    };
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
        let collection_name = collection_name
            .as_ref()
            .ok_or(IndexingError::MissingQdrantCollection)?;
        heartbeat_while(db_pool, ctx, input, async {
            let vectors = embed_batch(batch, input.embedding_worker).await?;
            let points = batch
                .iter()
                .zip(vectors)
                .map(|(chunk, vector)| {
                    Ok(UpsertPoint {
                        chunk_identity: chunk.chunk_identity.clone(),
                        vector,
                        payload: ChunkPointPayload {
                            org_id: ctx.org_id(),
                            collection_id: document.collection_id,
                            document_id,
                            version_id,
                            chunk_id: chunk.chunk_identity.clone(),
                            ordinal: u64::try_from(chunk.ordinal)
                                .map_err(|_| IndexingError::ChunkOrdinal)?,
                            is_current,
                            is_effective,
                            index_generation: u32::try_from(metadata.generation)
                                .map_err(|_| IndexingError::IndexGeneration)?,
                        },
                    })
                })
                .collect::<Result<Vec<_>, IndexingError>>()?;
            // TODO(I07): superseded-version point flags need reconciliation when a
            // newer version changes which version is current/effective.
            // TODO(I07): retrieval must filter on committed PG version state and
            // document indexed state, not solely on these Qdrant payload flags.
            qdrant
                .upsert_points(collection_name, &scope, &points)
                .await?;
            // TODO(I07): Qdrant is durable before the PG chunk/checkpoint commit;
            // dead-lettered attempts may leave orphan vectors for reconcile/GC.
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

    if is_current {
        let job = finalize_indexed(db_pool, ctx, input, document_id, version_id).await?;
        Ok(IndexVersionOutcome::Finalized {
            job_id: job.id,
            chunks: prepared_chunks.len(),
        })
    } else {
        Ok(IndexVersionOutcome::CompleteOnly {
            chunks: prepared_chunks.len(),
        })
    }
}

async fn load_index_source(
    db_pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<
    (
        crate::db::models::Document,
        documents::IndexVersionRecord,
        documents::IndexMarkdownArtifact,
    ),
    IndexingError,
> {
    with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let version = documents::find_index_version(txn, &ctx, document_id, version_id)
                    .await?
                    .ok_or(DbError::NotFound)?;
                let artifact = documents::find_markdown_artifact_for_index(txn, &ctx, version_id)
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
                let document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                if document.current_version_id != Some(version_id) {
                    return Err(IndexingError::CurrentVersionChanged);
                }
                document_state::apply_transition(
                    txn,
                    &ctx,
                    document_id,
                    DocumentState::Indexing,
                    DocumentState::Indexed,
                )
                .await?;
                let completed = repo::complete_owned(txn, &ctx, job_id, &lease_token, attempts)
                    .await?
                    .ok_or(IndexingError::Job(JobError::LeaseLost))?;
                write_job_succeeded_event(txn, &ctx, &completed).await?;
                Ok::<_, IndexingError>(completed)
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
    fn as_input(&self) -> index_metadata::EnsureGeneration<'_> {
        index_metadata::EnsureGeneration {
            collection_id: self.collection_id,
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

async fn embed_batch(
    batch: &[PreparedChunk],
    embedding_worker: &EmbeddingWorker,
) -> Result<Vec<Vec<f32>>, IndexingError> {
    let bodies = batch
        .iter()
        .map(|chunk| chunk.body.clone())
        .collect::<Vec<_>>();
    embedding_worker
        .embed_bodies(bodies)
        .await
        .map_err(Into::into)
}

struct PersistBatchInput<'a> {
    claim: IndexVersionInput<'a>,
    metadata_id: Uuid,
    signature_digest: &'a str,
    document_id: Uuid,
    version_id: Uuid,
    batch: &'a [PreparedChunk],
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
    let batch_end = u64::try_from(input.batch_end).map_err(|_| IndexingError::CheckpointOffset)?;
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
    #[error("embedding worker error")]
    EmbeddingWorker(#[from] EmbeddingWorkerError),
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
            Self::EmbeddingWorker(_) => "index embedding worker error",
            Self::InvalidPayload => "index payload invalid",
            Self::UnexpectedDocumentState(_) => "index document state invalid",
            Self::MarkdownNotTrusted => "index markdown key invalid",
            Self::MarkdownKeyVersionMismatch => "index markdown key version invalid",
            Self::MarkdownIntegrity => "index markdown integrity failed",
            Self::MarkdownUtf8 => "index markdown utf8 invalid",
            Self::SignatureMismatch => "index signature mismatch",
            Self::InvalidCheckpoint => "index checkpoint invalid",
            Self::CheckpointOffset => "index checkpoint offset invalid",
            Self::ChunkOrdinal => "index chunk ordinal invalid",
            Self::IndexGeneration => "index generation invalid",
            Self::MissingQdrantCollection => "index qdrant collection missing",
            Self::CurrentVersionChanged => "index current version changed",
            Self::JobTimedOut => "index job timed out",
        }
    }

    pub fn is_retryable_job_failure(&self) -> bool {
        !matches!(self, Self::Job(JobError::LeaseLost))
    }
}
