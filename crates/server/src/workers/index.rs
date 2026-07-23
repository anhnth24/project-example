//! Durable index job worker.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::{EmbeddingPlan, RUNTIME_LOCAL_HASH};
use thiserror::Error;
use tokio::time::{timeout_at, Instant as TokioInstant};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::config::Profile;
use crate::db::models::{Job, JobStatus, JobType};
use crate::jobs::{self, JobError};
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::indexing::{self, IndexVersionInput, IndexVersionOutcome, IndexingError};
use crate::services::lifecycle::{self, LifecycleError};
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const DEFAULT_MAX_JOB_DURATION_SECS: u64 = 300;
const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 64;
const MAX_EMBEDDING_BATCH_SIZE: usize = 4096;

#[derive(Debug, Clone)]
pub struct IndexWorkerConfig {
    pub worker_id: String,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub max_job_duration: Duration,
    pub embedding_batch_size: usize,
}

impl IndexWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            max_job_duration: Duration::from_secs(DEFAULT_MAX_JOB_DURATION_SECS),
            embedding_batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
        }
    }

    pub fn validate(&self) -> Result<(), IndexWorkerError> {
        if self.heartbeat_interval.is_zero()
            || self.lease_ttl.is_zero()
            || self.heartbeat_interval > self.lease_ttl / 3
        {
            return Err(IndexWorkerError::InvalidHeartbeatConfig);
        }
        if self.max_job_duration.is_zero() {
            return Err(IndexWorkerError::InvalidMaxJobDuration);
        }
        if self.embedding_batch_size == 0 || self.embedding_batch_size > MAX_EMBEDDING_BATCH_SIZE {
            return Err(IndexWorkerError::InvalidBatchSize);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct IndexWorker {
    db_pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    config: IndexWorkerConfig,
    approved_signature: Option<String>,
    embedding_plan: EmbeddingPlan,
    /// Alternates Index ↔ LifecycleRefresh claim preference (ConvertWorker pattern).
    next_claim_prefers_lifecycle: Arc<AtomicBool>,
}

impl IndexWorker {
    pub fn new(
        db_pool: Pool,
        storage: MinioClient,
        qdrant: QdrantClient,
        config: IndexWorkerConfig,
        profile: Profile,
        approved_signature: Option<String>,
    ) -> Result<Self, IndexWorkerError> {
        let runtime = ApprovedEmbeddingRuntime::from_env(approved_signature.as_deref(), profile)
            .map_err(IndexWorkerError::Embedding)?;
        Self::new_with_plan(
            db_pool,
            storage,
            qdrant,
            config,
            approved_signature,
            runtime.plan().clone(),
        )
    }

    pub fn new_with_plan(
        db_pool: Pool,
        storage: MinioClient,
        qdrant: QdrantClient,
        config: IndexWorkerConfig,
        approved_signature: Option<String>,
        embedding_plan: EmbeddingPlan,
    ) -> Result<Self, IndexWorkerError> {
        config.validate()?;
        if embedding_plan.runtime_path() == RUNTIME_LOCAL_HASH {
            return Err(IndexWorkerError::UnapprovedEmbeddingRuntime);
        }
        let dimensions = embedding_plan
            .expected_dimensions()
            .ok_or(IndexWorkerError::EmbeddingDimensionsUnknown)?;
        if let Some(signature) = approved_signature.as_deref() {
            let approved = embedding_plan
                .index_signature(dimensions)
                .map_err(IndexWorkerError::Knowledge)?
                .digest();
            if signature != approved {
                return Err(IndexWorkerError::SignatureMismatch);
            }
        }
        Ok(Self {
            db_pool,
            storage,
            qdrant,
            config,
            approved_signature,
            embedding_plan,
            next_claim_prefers_lifecycle: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn run_once(&self, ctx: &OrgContext) -> Result<IndexWorkerRun, IndexWorkerError> {
        // Fairness: alternate Index ↔ LifecycleRefresh so a continuous index
        // backlog cannot starve demoted-version lifecycle refresh.
        let claim_order = if self
            .next_claim_prefers_lifecycle
            .fetch_xor(true, Ordering::Relaxed)
        {
            [JobType::LifecycleRefresh, JobType::Index]
        } else {
            [JobType::Index, JobType::LifecycleRefresh]
        };
        for job_type in claim_order {
            let jobs = jobs::claim_type(
                &self.db_pool,
                ctx,
                job_type,
                &self.config.worker_id,
                DEFAULT_CLAIM_LIMIT,
                self.config.lease_ttl,
            )
            .await?;
            if let Some(job) = jobs.into_iter().next() {
                let started = std::time::Instant::now();
                let payload =
                    jobs::decode_job_payload(job.payload_version, job.payload.clone()).ok();
                let corr = payload
                    .as_ref()
                    .map(|payload| {
                        crate::telemetry::from_job_payload(
                            job.id,
                            payload,
                            crate::telemetry::WorkerIds {
                                org_id: Some(ctx.org_id()),
                                actor_id: Some(ctx.user_id()),
                                index_signature: self.approved_signature.clone(),
                            },
                        )
                    })
                    .unwrap_or_else(|| {
                        crate::telemetry::CorrelationContext::new(uuid::Uuid::new_v4().to_string())
                    });
                let is_lifecycle = job.job_type == JobType::LifecycleRefresh;
                let result = crate::telemetry::scope(corr.clone(), async {
                    let result = if is_lifecycle {
                        self.process_lifecycle_job(ctx, job).await
                    } else {
                        self.process_claimed_job(ctx, job).await
                    };
                    let outcome = match &result {
                        Ok(IndexWorkerRun::Completed { .. }) => "success",
                        Ok(IndexWorkerRun::Failed { .. }) => "failed",
                        Ok(IndexWorkerRun::LeaseLost { .. }) => "retry",
                        Ok(IndexWorkerRun::NoJob) => "idle",
                        Err(_) => "error",
                    };
                    let elapsed = started.elapsed();
                    let span_name = if is_lifecycle {
                        "worker.lifecycle"
                    } else {
                        "worker.index"
                    };
                    crate::telemetry::complete_current_span(
                        span_name, "CONSUMER", outcome, elapsed,
                    );
                    result
                })
                .await;
                return result;
            }
        }
        Ok(IndexWorkerRun::NoJob)
    }

    /// Returns the immutable embedding plan used to resolve target generations
    /// for index requests before workers claim them.
    pub fn embedding_plan(&self) -> &EmbeddingPlan {
        &self.embedding_plan
    }

    pub async fn process_claimed_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<IndexWorkerRun, IndexWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(IndexWorkerError::MissingLease)?
            .to_string();
        let attempts = job.attempts;
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        let input = IndexVersionInput {
            job: &job,
            lease_token: &lease_token,
            attempts,
            lease_ttl: self.config.lease_ttl,
            heartbeat_interval: self.config.heartbeat_interval,
            embedding_batch_size: self.config.embedding_batch_size,
            approved_signature: self.approved_signature.as_deref(),
            embedding_plan: &self.embedding_plan,
            deadline,
        };
        let result = timeout_at(
            deadline,
            indexing::index_version(&self.db_pool, &self.storage, &self.qdrant, ctx, input),
        )
        .await;
        match result {
            Ok(Ok(IndexVersionOutcome::Finalized { job_id, chunks })) => {
                Ok(IndexWorkerRun::Completed { job_id, chunks })
            }
            Ok(Ok(IndexVersionOutcome::CompleteOnly { chunks })) => {
                match jobs::complete(&self.db_pool, ctx, job.id, &lease_token, attempts).await {
                    Ok(completed) => Ok(IndexWorkerRun::Completed {
                        job_id: completed.id,
                        chunks,
                    }),
                    Err(JobError::LeaseLost) => Ok(IndexWorkerRun::LeaseLost { job_id: job.id }),
                    Err(error) => Err(IndexWorkerError::Job(error)),
                }
            }
            Ok(Ok(IndexVersionOutcome::AlreadyIndexed)) => {
                match jobs::complete(&self.db_pool, ctx, job.id, &lease_token, attempts).await {
                    Ok(completed) => Ok(IndexWorkerRun::Completed {
                        job_id: completed.id,
                        chunks: 0,
                    }),
                    Err(JobError::LeaseLost) => Ok(IndexWorkerRun::LeaseLost { job_id: job.id }),
                    Err(error) => Err(IndexWorkerError::Job(error)),
                }
            }
            Ok(Ok(IndexVersionOutcome::Aborted)) | Ok(Err(IndexingError::DocumentDeleted)) => {
                self.fail_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    IndexingError::DocumentDeleted.safe_job_error(),
                )
                .await
            }
            Ok(Err(IndexingError::Job(JobError::LeaseLost))) => {
                Ok(IndexWorkerRun::LeaseLost { job_id: job.id })
            }
            Ok(Err(error)) if error.is_retryable_job_failure() => {
                self.fail_claimed(ctx, &job, &lease_token, attempts, error.safe_job_error())
                    .await
            }
            Ok(Err(error)) => Err(IndexWorkerError::Indexing(error)),
            Err(_) => {
                self.fail_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    IndexWorkerError::JobTimedOut.safe_job_error(),
                )
                .await
            }
        }
    }

    async fn process_lifecycle_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<IndexWorkerRun, IndexWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(IndexWorkerError::MissingLease)?
            .to_string();
        let attempts = job.attempts;
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        let result = timeout_at(
            deadline,
            lifecycle::refresh_version_lifecycle(
                &self.db_pool,
                &self.qdrant,
                ctx,
                &job,
                &lease_token,
                attempts,
            ),
        )
        .await;
        match result {
            Ok(Ok(())) => {
                match jobs::complete(&self.db_pool, ctx, job.id, &lease_token, attempts).await {
                    Ok(completed) => Ok(IndexWorkerRun::Completed {
                        job_id: completed.id,
                        chunks: 0,
                    }),
                    Err(JobError::LeaseLost) => Ok(IndexWorkerRun::LeaseLost { job_id: job.id }),
                    Err(error) => Err(IndexWorkerError::Job(error)),
                }
            }
            Ok(Err(LifecycleError::Job(JobError::LeaseLost))) => {
                Ok(IndexWorkerRun::LeaseLost { job_id: job.id })
            }
            Ok(Err(error)) if error.is_retryable_job_failure() => {
                self.fail_lifecycle_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    error.safe_job_error(),
                )
                .await
            }
            Ok(Err(error)) => Err(IndexWorkerError::Lifecycle(error)),
            Err(_) => {
                self.fail_lifecycle_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    IndexWorkerError::JobTimedOut.safe_job_error(),
                )
                .await
            }
        }
    }

    async fn fail_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        last_error: &str,
    ) -> Result<IndexWorkerRun, IndexWorkerError> {
        match indexing::fail_index_job(&self.db_pool, ctx, job, lease_token, attempts, last_error)
            .await
        {
            Ok(failed) => Ok(IndexWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            }),
            Err(IndexingError::Job(JobError::LeaseLost)) => {
                Ok(IndexWorkerRun::LeaseLost { job_id: job.id })
            }
            Err(error) => Err(IndexWorkerError::Indexing(error)),
        }
    }

    async fn fail_lifecycle_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        last_error: &str,
    ) -> Result<IndexWorkerRun, IndexWorkerError> {
        match jobs::fail(
            &self.db_pool,
            ctx,
            job.id,
            lease_token,
            attempts,
            last_error,
        )
        .await
        {
            Ok(failed) => Ok(IndexWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            }),
            Err(JobError::LeaseLost) => Ok(IndexWorkerRun::LeaseLost { job_id: job.id }),
            Err(error) => Err(IndexWorkerError::Job(error)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexWorkerRun {
    NoJob,
    Completed { job_id: Uuid, chunks: usize },
    Failed { job_id: Uuid, terminal: bool },
    LeaseLost { job_id: Uuid },
}

#[derive(Debug, Error)]
pub enum IndexWorkerError {
    #[error("job error")]
    Job(#[from] JobError),
    #[error("indexing error")]
    Indexing(#[from] IndexingError),
    #[error("lifecycle error")]
    Lifecycle(#[from] LifecycleError),
    #[error("claimed job is missing a lease token")]
    MissingLease,
    #[error("worker heartbeat interval must be <= one third of lease ttl")]
    InvalidHeartbeatConfig,
    #[error("worker max job duration must be positive")]
    InvalidMaxJobDuration,
    #[error("embedding batch size must be between 1 and 4096")]
    InvalidBatchSize,
    #[error("configured index signature does not match approved local signature")]
    SignatureMismatch,
    #[error("knowledge error")]
    Knowledge(fileconv_knowledge::KnowledgeError),
    #[error("embedding runtime error")]
    Embedding(#[from] crate::services::embedding::EmbeddingError),
    #[error("approved embedding runtime did not declare vector dimensions")]
    EmbeddingDimensionsUnknown,
    #[error("the local hash runtime is not approved for server indexing")]
    UnapprovedEmbeddingRuntime,
    #[error("index job exceeded configured maximum duration")]
    JobTimedOut,
}

impl IndexWorkerError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Job(_) => "index job error",
            Self::Indexing(error) => error.safe_job_error(),
            Self::Lifecycle(error) => error.safe_job_error(),
            Self::MissingLease => "index missing lease",
            Self::InvalidHeartbeatConfig => "index heartbeat config invalid",
            Self::InvalidMaxJobDuration => "index max job duration invalid",
            Self::InvalidBatchSize => "index batch size invalid",
            Self::SignatureMismatch => "index signature mismatch",
            Self::Knowledge(_) => "index knowledge error",
            Self::Embedding(_) => "index embedding runtime error",
            Self::EmbeddingDimensionsUnknown => "index embedding dimensions missing",
            Self::UnapprovedEmbeddingRuntime => "index embedding runtime not approved",
            Self::JobTimedOut => "index job timed out",
        }
    }
}
