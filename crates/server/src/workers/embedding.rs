//! Durable worker for bounded, provider-backed embedding batches.
//!
//! Index jobs only prepare immutable chunks and enqueue one `EmbeddingBatch`
//! job per bounded range. This worker is the sole component that sends text to
//! the approved embedding runtime, so durable job leasing supplies backpressure
//! across processes and restarts.

use std::time::Duration;

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::embedding_batches::{self, EmbeddingBatch};
use crate::db::error::DbError;
use crate::db::models::{Job, JobStatus, JobType};
use crate::db::pool::with_org_txn_typed;
use crate::db::{chunks, documents, index_metadata, jobs as job_repo};
use crate::jobs::{self, JobError};
use crate::services::embedding::{
    canonical_inputs_sha256, ApprovedEmbeddingRuntime, EmbeddingError,
};
use crate::services::indexing::{self, IndexingError};
use crate::storage::qdrant::{ChunkPointPayload, QdrantClient, UpsertPoint, VectorScope};
use crate::storage::StorageError;

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const DEFAULT_MAX_JOB_DURATION_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct EmbeddingWorkerConfig {
    pub worker_id: String,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub max_job_duration: Duration,
}

impl EmbeddingWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            max_job_duration: Duration::from_secs(DEFAULT_MAX_JOB_DURATION_SECS),
        }
    }

    fn validate(&self) -> Result<(), EmbeddingWorkerError> {
        if self.lease_ttl.is_zero()
            || self.heartbeat_interval.is_zero()
            || self.heartbeat_interval > self.lease_ttl / 3
        {
            return Err(EmbeddingWorkerError::InvalidHeartbeatConfig);
        }
        if self.max_job_duration.is_zero() {
            return Err(EmbeddingWorkerError::InvalidMaxJobDuration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EmbeddingWorker {
    db_pool: Pool,
    qdrant: QdrantClient,
    config: EmbeddingWorkerConfig,
    runtime: ApprovedEmbeddingRuntime,
}

impl EmbeddingWorker {
    pub fn new(
        db_pool: Pool,
        qdrant: QdrantClient,
        config: EmbeddingWorkerConfig,
        runtime: ApprovedEmbeddingRuntime,
    ) -> Result<Self, EmbeddingWorkerError> {
        config.validate()?;
        Ok(Self {
            db_pool,
            qdrant,
            config,
            runtime,
        })
    }

    pub async fn run_once(
        &self,
        ctx: &OrgContext,
    ) -> Result<EmbeddingWorkerRun, EmbeddingWorkerError> {
        let jobs = jobs::claim_type(
            &self.db_pool,
            ctx,
            JobType::EmbeddingBatch,
            &self.config.worker_id,
            DEFAULT_CLAIM_LIMIT,
            self.config.lease_ttl,
        )
        .await?;
        let Some(job) = jobs.into_iter().next() else {
            return Ok(EmbeddingWorkerRun::NoJob);
        };
        self.process_claimed_job(ctx, job).await
    }

    pub async fn process_claimed_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<EmbeddingWorkerRun, EmbeddingWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(EmbeddingWorkerError::MissingLease)?
            .to_string();
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        let result = self
            .process(ctx, &job, &lease_token, job.attempts, deadline)
            .await;
        match result {
            Ok(()) => Ok(EmbeddingWorkerRun::Completed { job_id: job.id }),
            Err(EmbeddingWorkerError::Job(JobError::LeaseLost))
            | Err(EmbeddingWorkerError::LeaseLost) => {
                Ok(EmbeddingWorkerRun::LeaseLost { job_id: job.id })
            }
            Err(error) if error.is_retryable() => {
                self.fail_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    job.attempts,
                    error.safe_job_error(),
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn process(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
    ) -> Result<(), EmbeddingWorkerError> {
        let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
        let batch_id = payload
            .batch_id
            .ok_or(EmbeddingWorkerError::InvalidPayload)?;
        let source = self.load_batch_source(ctx, job.id, batch_id).await?;
        let expected_dimensions = self
            .runtime
            .plan()
            .expected_dimensions()
            .ok_or(EmbeddingWorkerError::EmbeddingDimensionsUnknown)?;
        let signature = self.runtime.plan().index_signature(expected_dimensions)?;
        // Check every immutable target-generation property before sending chunk
        // text to a provider. A stale, cross-collection, or retired batch must
        // not create vectors or a phantom collection.
        indexing::validate_target_generation(
            &source.metadata,
            source.collection_id,
            &signature.digest(),
        )?;
        let inputs = source
            .chunks
            .iter()
            .map(|chunk| {
                ApprovedEmbeddingRuntime::canonical_input(
                    &chunk.heading_path.join(" > "),
                    &chunk.body,
                )
            })
            .collect::<Vec<_>>();
        if canonical_inputs_sha256(&inputs) != source.batch.input_sha256 {
            return Err(EmbeddingWorkerError::InputChecksumMismatch);
        }
        if inputs.len()
            != usize::try_from(source.batch.end_ordinal - source.batch.start_ordinal)
                .map_err(|_| EmbeddingWorkerError::InvalidPayload)?
        {
            return Err(EmbeddingWorkerError::ChunkRangeMismatch);
        }

        let vectors = self
            .heartbeat_while(
                ctx,
                job,
                lease_token,
                attempts,
                deadline,
                self.runtime.embed(&inputs),
            )
            .await?;
        let dimensions = vectors
            .first()
            .map(Vec::len)
            .ok_or(EmbeddingWorkerError::ChunkRangeMismatch)?;
        if dimensions != expected_dimensions
            || vectors
                .iter()
                .any(|vector| vector.len() != expected_dimensions)
        {
            return Err(EmbeddingWorkerError::SignatureMismatch);
        }
        let collection_name = self
            .heartbeat_while(
                ctx,
                job,
                lease_token,
                attempts,
                deadline,
                self.qdrant.ensure_collection_for_signature(&signature),
            )
            .await?;
        // The provider call can take long enough for a newer version to be
        // promoted. Re-read the lifecycle under the document row lock
        // immediately before the external write; never reuse the stale flags
        // captured while loading the batch source.
        let lifecycle = self
            .heartbeat_while(
                ctx,
                job,
                lease_token,
                attempts,
                deadline,
                self.load_lifecycle_fence(ctx, job, lease_token, attempts, batch_id),
            )
            .await?;
        let scope = VectorScope::new(ctx.org_id(), [source.collection_id]);
        let points = source
            .chunks
            .iter()
            .zip(vectors)
            .map(|(chunk, vector)| {
                Ok(UpsertPoint {
                    chunk_identity: chunk.chunk_identity_sha256.clone(),
                    vector,
                    payload: ChunkPointPayload {
                        org_id: ctx.org_id(),
                        collection_id: source.collection_id,
                        document_id: source.batch.document_id,
                        version_id: source.batch.version_id,
                        chunk_id: chunk.chunk_identity_sha256.clone(),
                        ordinal: u64::try_from(chunk.ordinal)
                            .map_err(|_| EmbeddingWorkerError::InvalidPayload)?,
                        is_current: lifecycle.is_current,
                        is_effective: lifecycle.is_effective,
                        index_generation: u32::try_from(source.metadata.generation)
                            .map_err(|_| EmbeddingWorkerError::InvalidPayload)?,
                    },
                })
            })
            .collect::<Result<Vec<_>, EmbeddingWorkerError>>()?;
        self.heartbeat_while(
            ctx,
            job,
            lease_token,
            attempts,
            deadline,
            self.qdrant.upsert_points(&collection_name, &scope, &points),
        )
        .await?;
        self.complete_batch(ctx, job, lease_token, attempts, batch_id)
            .await
    }

    async fn load_batch_source(
        &self,
        ctx: &OrgContext,
        job_id: Uuid,
        batch_id: Uuid,
    ) -> Result<EmbeddingBatchSource, EmbeddingWorkerError> {
        with_org_txn_typed(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    let batch = embedding_batches::find_by_id_for_update(txn, &ctx, batch_id)
                        .await?
                        .ok_or(crate::db::error::DbError::NotFound)?;
                    if batch.job_id != job_id
                        || batch.status != embedding_batches::EmbeddingBatchStatus::Pending
                    {
                        return Err(EmbeddingWorkerError::InvalidPayload);
                    }
                    let metadata = index_metadata::find_by_id(txn, &ctx, batch.index_metadata_id)
                        .await?
                        .ok_or(crate::db::error::DbError::NotFound)?;
                    let document = documents::get_by_id(txn, &ctx, batch.document_id).await?;
                    let chunks = chunks::list_generation_range(
                        txn,
                        &ctx,
                        batch.index_metadata_id,
                        batch.document_id,
                        batch.version_id,
                        batch.start_ordinal,
                        batch.end_ordinal,
                    )
                    .await?;
                    Ok(EmbeddingBatchSource {
                        collection_id: document.collection_id,
                        batch,
                        metadata,
                        chunks,
                    })
                })
            }
        })
        .await
    }

    /// Fences lifecycle markers immediately before the Qdrant upsert. The
    /// document lock serializes this observation with conversion promotion,
    /// while the leased-job and pending-batch checks prevent a cancelled or
    /// superseded worker from publishing stale lifecycle flags.
    async fn load_lifecycle_fence(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        batch_id: Uuid,
    ) -> Result<PointLifecycle, EmbeddingWorkerError> {
        let job_id = job.id;
        let lease_token = lease_token.to_string();
        with_org_txn_typed(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    let claimed = job_repo::get_by_id_for_update(txn, &ctx, job_id)
                        .await?
                        .filter(|claimed| {
                            claimed.status == JobStatus::Leased
                                && claimed.lease_owner.as_deref() == Some(lease_token.as_str())
                                && claimed.attempts == attempts
                        })
                        .ok_or(EmbeddingWorkerError::LeaseLost)?;
                    let batch = embedding_batches::find_by_id_for_update(txn, &ctx, batch_id)
                        .await?
                        .filter(|batch| {
                            batch.job_id == claimed.id
                                && batch.status == embedding_batches::EmbeddingBatchStatus::Pending
                        })
                        .ok_or(EmbeddingWorkerError::InvalidPayload)?;
                    let document =
                        documents::get_by_id_for_update(txn, &ctx, batch.document_id).await?;
                    let version = crate::db::document_versions::find_by_id(
                        txn,
                        &ctx,
                        batch.document_id,
                        batch.version_id,
                    )
                    .await?
                    .ok_or(crate::db::error::DbError::NotFound)?;
                    Ok(point_lifecycle(
                        document.current_version_id,
                        batch.version_id,
                        version.effective_to.is_none(),
                    ))
                })
            }
        })
        .await
    }

    async fn complete_batch(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        batch_id: Uuid,
    ) -> Result<(), EmbeddingWorkerError> {
        let job_id = job.id;
        let lease_token = lease_token.to_string();
        with_org_txn_typed(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    let batch = embedding_batches::find_by_id_for_update(txn, &ctx, batch_id)
                        .await?
                        .ok_or(crate::db::error::DbError::NotFound)?;
                    if batch.job_id != job_id {
                        return Err(EmbeddingWorkerError::InvalidPayload);
                    }
                    let completed =
                        jobs::complete_within_txn(txn, &ctx, job_id, &lease_token, attempts)
                            .await?;
                    embedding_batches::mark_succeeded(txn, &ctx, batch_id).await?;
                    indexing::complete_document_backfill_if_ready(
                        txn,
                        &ctx,
                        batch.index_metadata_id,
                        batch.document_id,
                        batch.version_id,
                    )
                    .await?;
                    Ok(completed)
                })
            }
        })
        .await?;
        Ok(())
    }

    async fn fail_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        last_error: &str,
    ) -> Result<EmbeddingWorkerRun, EmbeddingWorkerError> {
        match indexing::fail_index_job(&self.db_pool, ctx, job, lease_token, attempts, last_error)
            .await
        {
            Ok(failed) => Ok(EmbeddingWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            }),
            Err(IndexingError::Job(JobError::LeaseLost)) => {
                Ok(EmbeddingWorkerRun::LeaseLost { job_id: job.id })
            }
            Err(error) => Err(EmbeddingWorkerError::Indexing(error)),
        }
    }

    async fn heartbeat_while<T, E, Fut>(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
        future: Fut,
    ) -> Result<T, EmbeddingWorkerError>
    where
        Fut: std::future::Future<Output = Result<T, E>>,
        E: Into<EmbeddingWorkerError>,
    {
        self.heartbeat_once(ctx, job, lease_token, attempts, deadline)
            .await?;
        tokio::pin!(future);
        let mut heartbeat = heartbeat_interval(self.config.heartbeat_interval);
        loop {
            tokio::select! {
                biased;
                result = &mut future => return result.map_err(Into::into),
                _ = sleep_until(deadline) => return Err(EmbeddingWorkerError::JobTimedOut),
                _ = heartbeat.tick() => self.heartbeat_once(ctx, job, lease_token, attempts, deadline).await?,
            }
        }
    }

    async fn heartbeat_once(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
    ) -> Result<(), EmbeddingWorkerError> {
        match timeout(
            heartbeat_call_timeout(self.config.lease_ttl, deadline),
            jobs::heartbeat(
                &self.db_pool,
                ctx,
                job.id,
                lease_token,
                attempts,
                self.config.lease_ttl,
            ),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(JobError::LeaseLost)) => Err(EmbeddingWorkerError::LeaseLost),
            Ok(Err(error)) => Err(EmbeddingWorkerError::Job(error)),
            Err(_) => Err(EmbeddingWorkerError::JobTimedOut),
        }
    }
}

struct EmbeddingBatchSource {
    collection_id: Uuid,
    batch: EmbeddingBatch,
    metadata: crate::db::models::IndexMetadata,
    chunks: Vec<crate::db::models::Chunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PointLifecycle {
    is_current: bool,
    is_effective: bool,
}

fn point_lifecycle(
    current_version_id: Option<Uuid>,
    version_id: Uuid,
    is_effective: bool,
) -> PointLifecycle {
    PointLifecycle {
        is_current: current_version_id == Some(version_id),
        is_effective,
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
    lease_bound.min(deadline.saturating_duration_since(TokioInstant::now()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingWorkerRun {
    NoJob,
    Completed { job_id: Uuid },
    Failed { job_id: Uuid, terminal: bool },
    LeaseLost { job_id: Uuid },
}

#[derive(Debug, Error)]
pub enum EmbeddingWorkerError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("indexing error")]
    Indexing(#[from] IndexingError),
    #[error("embedding runtime error")]
    Embedding(#[from] EmbeddingError),
    #[error("knowledge error")]
    Knowledge(#[from] fileconv_knowledge::KnowledgeError),
    #[error("vector storage error")]
    Storage(#[from] StorageError),
    #[error("claimed job is missing a lease token")]
    MissingLease,
    #[error("embedding job payload is invalid")]
    InvalidPayload,
    #[error("embedding batch canonical input checksum does not match")]
    InputChecksumMismatch,
    #[error("embedding batch does not match its stored chunk range")]
    ChunkRangeMismatch,
    #[error("approved embedding runtime did not declare vector dimensions")]
    EmbeddingDimensionsUnknown,
    #[error("embedding signature differs from target index generation")]
    SignatureMismatch,
    #[error("embedding worker heartbeat interval must be <= one third of lease ttl")]
    InvalidHeartbeatConfig,
    #[error("embedding worker maximum job duration must be positive")]
    InvalidMaxJobDuration,
    #[error("embedding job exceeded configured maximum duration")]
    JobTimedOut,
    #[error("job lease was lost")]
    LeaseLost,
}

impl EmbeddingWorkerError {
    fn is_retryable(&self) -> bool {
        !matches!(self, Self::LeaseLost | Self::Job(JobError::LeaseLost))
    }

    fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "embedding database error",
            Self::Job(_) => "embedding job error",
            Self::Indexing(error) => error.safe_job_error(),
            Self::Embedding(_) => "embedding runtime error",
            Self::Knowledge(_) => "embedding knowledge error",
            Self::Storage(_) => "embedding vector storage error",
            Self::MissingLease => "embedding missing lease",
            Self::InvalidPayload => "embedding payload invalid",
            Self::InputChecksumMismatch => "embedding input checksum mismatch",
            Self::ChunkRangeMismatch => "embedding chunk range invalid",
            Self::EmbeddingDimensionsUnknown => "embedding dimensions missing",
            Self::SignatureMismatch => "embedding signature mismatch",
            Self::InvalidHeartbeatConfig => "embedding heartbeat config invalid",
            Self::InvalidMaxJobDuration => "embedding max job duration invalid",
            Self::JobTimedOut => "embedding job timed out",
            Self::LeaseLost => "embedding lease lost",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::point_lifecycle;
    use uuid::Uuid;

    #[test]
    fn pre_upsert_lifecycle_fence_does_not_reuse_a_superseded_version_flag() {
        let current = Uuid::new_v4();
        let stale = Uuid::new_v4();
        let lifecycle = point_lifecycle(Some(current), stale, false);
        assert!(!lifecycle.is_current);
        assert!(!lifecycle.is_effective);

        let lifecycle = point_lifecycle(Some(current), current, true);
        assert!(lifecycle.is_current);
        assert!(lifecycle.is_effective);
    }
}
