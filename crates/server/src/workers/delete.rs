//! Durable delete job worker.

use std::time::Duration;

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::time::{timeout_at, Instant as TokioInstant};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::models::{Job, JobStatus, JobType};
use crate::jobs::{self, JobError};
use crate::services::deletion::{self, DeletionError, PurgeDocumentInput, PurgeDocumentOutcome};
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const DEFAULT_MAX_JOB_DURATION_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct DeleteWorkerConfig {
    pub worker_id: String,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub max_job_duration: Duration,
}

impl DeleteWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            max_job_duration: Duration::from_secs(DEFAULT_MAX_JOB_DURATION_SECS),
        }
    }

    pub fn validate(&self) -> Result<(), DeleteWorkerError> {
        if self.heartbeat_interval.is_zero()
            || self.lease_ttl.is_zero()
            || self.heartbeat_interval > self.lease_ttl / 3
        {
            return Err(DeleteWorkerError::InvalidHeartbeatConfig);
        }
        if self.max_job_duration.is_zero() {
            return Err(DeleteWorkerError::InvalidMaxJobDuration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct DeleteWorker {
    db_pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    config: DeleteWorkerConfig,
}

impl DeleteWorker {
    pub fn new(
        db_pool: Pool,
        storage: MinioClient,
        qdrant: QdrantClient,
        config: DeleteWorkerConfig,
    ) -> Result<Self, DeleteWorkerError> {
        config.validate()?;
        Ok(Self {
            db_pool,
            storage,
            qdrant,
            config,
        })
    }

    pub async fn run_once(&self, ctx: &OrgContext) -> Result<DeleteWorkerRun, DeleteWorkerError> {
        let jobs = jobs::claim_type(
            &self.db_pool,
            ctx,
            JobType::Delete,
            &self.config.worker_id,
            DEFAULT_CLAIM_LIMIT,
            self.config.lease_ttl,
        )
        .await?;
        let Some(job) = jobs.into_iter().next() else {
            return Ok(DeleteWorkerRun::NoJob);
        };
        self.process_claimed_job(ctx, job).await
    }

    pub async fn process_claimed_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<DeleteWorkerRun, DeleteWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(DeleteWorkerError::MissingLease)?
            .to_string();
        let attempts = job.attempts;
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        let input = PurgeDocumentInput {
            job: &job,
            lease_token: &lease_token,
            attempts,
            lease_ttl: self.config.lease_ttl,
            heartbeat_interval: self.config.heartbeat_interval,
            deadline,
        };
        let result = timeout_at(
            deadline,
            deletion::purge_document(&self.db_pool, &self.storage, &self.qdrant, ctx, input),
        )
        .await;
        match result {
            Ok(Ok(PurgeDocumentOutcome::Purged {
                job_id,
                deleted_chunks,
            })) => Ok(DeleteWorkerRun::Completed {
                job_id,
                deleted_chunks,
            }),
            Ok(Ok(PurgeDocumentOutcome::AlreadyPurged { job_id })) => {
                Ok(DeleteWorkerRun::Completed {
                    job_id,
                    deleted_chunks: 0,
                })
            }
            Ok(Err(DeletionError::Job(JobError::LeaseLost))) => {
                Ok(DeleteWorkerRun::LeaseLost { job_id: job.id })
            }
            Ok(Err(error)) if error.is_retryable_job_failure() => {
                self.fail_claimed(ctx, &job, &lease_token, attempts, error.safe_job_error())
                    .await
            }
            Ok(Err(error)) => Err(DeleteWorkerError::Deletion(error)),
            Err(_) => {
                self.fail_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    DeleteWorkerError::JobTimedOut.safe_job_error(),
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
    ) -> Result<DeleteWorkerRun, DeleteWorkerError> {
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
            Ok(failed) => Ok(DeleteWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            }),
            Err(JobError::LeaseLost) => Ok(DeleteWorkerRun::LeaseLost { job_id: job.id }),
            Err(error) => Err(DeleteWorkerError::Job(error)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteWorkerRun {
    NoJob,
    Completed { job_id: Uuid, deleted_chunks: u64 },
    Failed { job_id: Uuid, terminal: bool },
    LeaseLost { job_id: Uuid },
}

#[derive(Debug, Error)]
pub enum DeleteWorkerError {
    #[error("job error")]
    Job(#[from] JobError),
    #[error("delete error")]
    Deletion(#[from] DeletionError),
    #[error("claimed job is missing a lease token")]
    MissingLease,
    #[error("worker heartbeat interval must be <= one third of lease ttl")]
    InvalidHeartbeatConfig,
    #[error("worker max job duration must be positive")]
    InvalidMaxJobDuration,
    #[error("delete job exceeded configured maximum duration")]
    JobTimedOut,
}

impl DeleteWorkerError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Job(_) => "delete job error",
            Self::Deletion(error) => error.safe_job_error(),
            Self::MissingLease => "delete missing lease",
            Self::InvalidHeartbeatConfig => "delete heartbeat config invalid",
            Self::InvalidMaxJobDuration => "delete max job duration invalid",
            Self::JobTimedOut => "delete job timed out",
        }
    }
}
