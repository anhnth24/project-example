//! Durable reconciliation job worker.

use std::time::Duration;

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::time::Instant as TokioInstant;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::models::{Job, JobStatus};
use crate::jobs::{self, HeartbeatClaim, HeartbeatError, JobError};
use crate::services::reconciliation::{self, ReconcileMode, ReconcileReport, ReconciliationError};
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const DEFAULT_MAX_JOB_DURATION_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct ReconcileWorkerConfig {
    pub worker_id: String,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub max_job_duration: Duration,
    pub mode: ReconcileMode,
}

impl ReconcileWorkerConfig {
    pub fn new(worker_id: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            max_job_duration: Duration::from_secs(DEFAULT_MAX_JOB_DURATION_SECS),
            mode: ReconcileMode::DryRun,
        }
    }

    pub fn validate(&self) -> Result<(), ReconcileWorkerError> {
        if self.heartbeat_interval.is_zero()
            || self.lease_ttl.is_zero()
            || self.heartbeat_interval > self.lease_ttl / 3
        {
            return Err(ReconcileWorkerError::InvalidHeartbeatConfig);
        }
        if self.max_job_duration.is_zero() {
            return Err(ReconcileWorkerError::InvalidMaxJobDuration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ReconcileWorker {
    db_pool: Pool,
    storage: MinioClient,
    qdrant: QdrantClient,
    config: ReconcileWorkerConfig,
}

impl ReconcileWorker {
    pub fn new(
        db_pool: Pool,
        storage: MinioClient,
        qdrant: QdrantClient,
        config: ReconcileWorkerConfig,
    ) -> Result<Self, ReconcileWorkerError> {
        config.validate()?;
        Ok(Self {
            db_pool,
            storage,
            qdrant,
            config,
        })
    }

    pub async fn run_once(
        &self,
        ctx: &OrgContext,
    ) -> Result<ReconcileWorkerRun, ReconcileWorkerError> {
        // Document-drift reconcile only — conversion cleanup shares JobType::Reconcile
        // but carries cleanup_target_job_id and is claimed by ConvertWorker.
        let jobs = jobs::claim_reconcile(
            &self.db_pool,
            ctx,
            &self.config.worker_id,
            DEFAULT_CLAIM_LIMIT,
            self.config.lease_ttl,
            false,
        )
        .await?;
        let Some(job) = jobs.into_iter().next() else {
            return Ok(ReconcileWorkerRun::NoJob);
        };
        self.process_claimed_job(ctx, job).await
    }

    pub async fn process_claimed_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<ReconcileWorkerRun, ReconcileWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(ReconcileWorkerError::MissingLease)?
            .to_string();
        let attempts = job.attempts;
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        let result = self
            .heartbeat_while(
                ctx,
                &job,
                &lease_token,
                attempts,
                deadline,
                self.reconcile_claimed(ctx, &job),
            )
            .await;
        match result {
            Ok(report) => {
                if let Err(ReconcileWorkerError::Job(JobError::LeaseLost)) = self
                    .heartbeat_once(ctx, &job, &lease_token, attempts, deadline)
                    .await
                {
                    return Ok(ReconcileWorkerRun::LeaseLost { job_id: job.id });
                }
                match jobs::complete(&self.db_pool, ctx, job.id, &lease_token, attempts).await {
                    Ok(completed) => Ok(ReconcileWorkerRun::Completed {
                        job_id: completed.id,
                        report,
                    }),
                    Err(JobError::LeaseLost) => {
                        Ok(ReconcileWorkerRun::LeaseLost { job_id: job.id })
                    }
                    Err(error) => Err(ReconcileWorkerError::Job(error)),
                }
            }
            Err(ReconcileWorkerError::Job(JobError::LeaseLost))
            | Err(ReconcileWorkerError::Reconciliation(ReconciliationError::Job(
                JobError::LeaseLost,
            ))) => Ok(ReconcileWorkerRun::LeaseLost { job_id: job.id }),
            Err(ReconcileWorkerError::Reconciliation(error))
                if error.is_retryable_job_failure() =>
            {
                self.fail_claimed(ctx, &job, &lease_token, attempts, error.safe_job_error())
                    .await
            }
            Err(ReconcileWorkerError::JobTimedOut) => {
                self.fail_claimed(
                    ctx,
                    &job,
                    &lease_token,
                    attempts,
                    ReconcileWorkerError::JobTimedOut.safe_job_error(),
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn reconcile_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
    ) -> Result<ReconcileReport, ReconciliationError> {
        let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
        let mut report = if let Some(document_id) = payload.document_id {
            reconciliation::reconcile_document(
                &self.db_pool,
                &self.storage,
                &self.qdrant,
                ctx,
                document_id,
                self.config.mode,
            )
            .await?
        } else {
            ReconcileReport::default()
        };
        let gc_report = reconciliation::reconcile_dead_letter_jobs(
            &self.db_pool,
            &self.storage,
            ctx,
            self.config.mode,
        )
        .await?;
        report.repaired.staged_objects = gc_report.repaired.staged_objects;
        Ok(report)
    }

    async fn heartbeat_once(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
    ) -> Result<(), ReconcileWorkerError> {
        jobs::heartbeat_once_claimed(self.heartbeat_claim(
            ctx,
            job,
            lease_token,
            attempts,
            deadline,
        ))
        .await
        .map_err(Into::into)
    }

    async fn heartbeat_while<T, Fut>(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
        future: Fut,
    ) -> Result<T, ReconcileWorkerError>
    where
        Fut: std::future::Future<Output = Result<T, ReconciliationError>>,
    {
        jobs::heartbeat_while_claimed(
            self.heartbeat_claim(ctx, job, lease_token, attempts, deadline),
            async move { future.await.map_err(ReconcileWorkerError::Reconciliation) },
        )
        .await
    }

    fn heartbeat_claim<'a>(
        &'a self,
        ctx: &'a OrgContext,
        job: &'a Job,
        lease_token: &'a str,
        attempts: i32,
        deadline: TokioInstant,
    ) -> HeartbeatClaim<'a> {
        HeartbeatClaim {
            db_pool: &self.db_pool,
            ctx,
            job_id: job.id,
            lease_token,
            attempts,
            lease_ttl: self.config.lease_ttl,
            heartbeat_interval: self.config.heartbeat_interval,
            deadline,
        }
    }

    async fn fail_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        last_error: &str,
    ) -> Result<ReconcileWorkerRun, ReconcileWorkerError> {
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
            Ok(failed) => Ok(ReconcileWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            }),
            Err(JobError::LeaseLost) => Ok(ReconcileWorkerRun::LeaseLost { job_id: job.id }),
            Err(error) => Err(ReconcileWorkerError::Job(error)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileWorkerRun {
    NoJob,
    Completed {
        job_id: Uuid,
        report: ReconcileReport,
    },
    Failed {
        job_id: Uuid,
        terminal: bool,
    },
    LeaseLost {
        job_id: Uuid,
    },
}

#[derive(Debug, Error)]
pub enum ReconcileWorkerError {
    #[error("job error")]
    Job(#[from] JobError),
    #[error("reconcile error")]
    Reconciliation(#[from] ReconciliationError),
    #[error("claimed job is missing a lease token")]
    MissingLease,
    #[error("worker heartbeat interval must be <= one third of lease ttl")]
    InvalidHeartbeatConfig,
    #[error("worker max job duration must be positive")]
    InvalidMaxJobDuration,
    #[error("reconcile job exceeded configured maximum duration")]
    JobTimedOut,
}

impl From<HeartbeatError> for ReconcileWorkerError {
    fn from(value: HeartbeatError) -> Self {
        match value {
            HeartbeatError::Job(error) => Self::Job(error),
            HeartbeatError::TimedOut => Self::JobTimedOut,
        }
    }
}

impl ReconcileWorkerError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Job(_) => "reconcile job error",
            Self::Reconciliation(error) => error.safe_job_error(),
            Self::MissingLease => "reconcile missing lease",
            Self::InvalidHeartbeatConfig => "reconcile heartbeat config invalid",
            Self::InvalidMaxJobDuration => "reconcile max job duration invalid",
            Self::JobTimedOut => "reconcile job timed out",
        }
    }
}
