//! Converter job worker.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{
    interval_at, sleep_until, timeout, timeout_at, Instant as TokioInstant, MissedTickBehavior,
};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::document_versions::ConversionSourceVersion;
use crate::db::error::DbError;
use crate::db::jobs as jobs_repo;
use crate::db::models::{Job, JobStatus, JobType, ResourceKind};
use crate::db::pool::with_org_txn;
use crate::jobs::{self, EnqueueJob, JobError, JobPayload};
use crate::services::artifacts::{self, MarkdownStageInput, StagedMarkdown};
use crate::services::conversion::{
    checkpoint_with_staged_key, checkpoint_with_step, staged_keys_from_checkpoint,
    ConversionIdentity, ConversionStep,
};
use crate::services::promotion::{self, PromoteConversionInput, PromotionError, PromotionFault};
use crate::services::quota::{self, QuotaError, DEFAULT_RESERVATION_TTL};
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::{ObjectNamespace, StorageError};

use super::limits::ResourceLimits;
use super::sandbox::{
    self, SandboxCancel, SandboxConfig, SandboxError, SandboxExit, SandboxInput, SandboxOutput,
};

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
const DEFAULT_JOB_GRACE_SECS: u64 = 30;
const SANDBOX_CANCEL_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const RECONCILIATION_MAX_ATTEMPTS: u32 = 32;

#[derive(Debug, Clone)]
pub struct ConvertWorkerConfig {
    pub worker_id: String,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub sandbox: SandboxConfig,
    pub post_upload_settlement_delay: Duration,
    pub max_job_duration: Duration,
    pub promotion_fault: Option<PromotionFault>,
    pub quota_reservation_ttl: Duration,
    pub fail_cleanup_delete: bool,
    pub fail_quota_refund: bool,
    pub lose_staged_handle_after_put: bool,
    pub pause_after_staging: Option<ConvertWorkerPause>,
}

impl ConvertWorkerConfig {
    pub fn new(worker_id: impl Into<String>, sandbox: SandboxConfig) -> Self {
        let max_job_duration =
            sandbox.limits.wall_timeout + Duration::from_secs(DEFAULT_JOB_GRACE_SECS);
        Self {
            worker_id: worker_id.into(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            sandbox,
            post_upload_settlement_delay: Duration::ZERO,
            max_job_duration,
            promotion_fault: None,
            quota_reservation_ttl: DEFAULT_RESERVATION_TTL,
            fail_cleanup_delete: false,
            fail_quota_refund: false,
            lose_staged_handle_after_put: false,
            pause_after_staging: None,
        }
    }

    pub fn default_for_fileconv(worker_id: impl Into<String>, lease_ttl: Duration) -> Self {
        Self {
            worker_id: worker_id.into(),
            lease_ttl,
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            sandbox: SandboxConfig {
                argv_template: vec![
                    "/usr/local/bin/fileconv".into(),
                    "one".into(),
                    "{input}".into(),
                ],
                limits: ResourceLimits::default(),
            },
            post_upload_settlement_delay: Duration::ZERO,
            max_job_duration: ResourceLimits::default().wall_timeout
                + Duration::from_secs(DEFAULT_JOB_GRACE_SECS),
            promotion_fault: None,
            quota_reservation_ttl: DEFAULT_RESERVATION_TTL,
            fail_cleanup_delete: false,
            fail_quota_refund: false,
            lose_staged_handle_after_put: false,
            pause_after_staging: None,
        }
    }

    pub fn validate(&self) -> Result<(), ConvertWorkerError> {
        self.sandbox
            .validate()
            .map_err(ConvertWorkerError::Sandbox)?;
        if self.heartbeat_interval.is_zero()
            || self.lease_ttl.is_zero()
            || self.heartbeat_interval > self.lease_ttl / 3
        {
            return Err(ConvertWorkerError::InvalidHeartbeatConfig);
        }
        if self.max_job_duration < self.sandbox.limits.wall_timeout {
            return Err(ConvertWorkerError::InvalidMaxJobDuration);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ConvertWorkerPause {
    pub staged: Arc<Notify>,
    pub release: Arc<Notify>,
}

impl std::fmt::Debug for ConvertWorkerPause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConvertWorkerPause")
    }
}

struct ClaimedJobScope<'a> {
    ctx: &'a OrgContext,
    job: &'a Job,
    lease_token: &'a str,
    attempts: i32,
    deadline: TokioInstant,
}

enum PromotionWait {
    Finished(Result<promotion::PromoteConversionOutcome, PromotionError>),
    ReconciliationNeeded,
}

#[derive(Clone)]
pub struct ConvertWorker {
    db_pool: Pool,
    storage: MinioClient,
    config: ConvertWorkerConfig,
    next_claim_prefers_reconciliation: Arc<AtomicBool>,
}

impl ConvertWorker {
    pub fn new(
        db_pool: Pool,
        storage: MinioClient,
        config: ConvertWorkerConfig,
    ) -> Result<Self, ConvertWorkerError> {
        config.validate()?;
        sandbox::preflight().map_err(ConvertWorkerError::Sandbox)?;
        Ok(Self {
            db_pool,
            storage,
            config,
            next_claim_prefers_reconciliation: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn run_once(&self, ctx: &OrgContext) -> Result<ConvertWorkerRun, ConvertWorkerError> {
        let claim_order = if self
            .next_claim_prefers_reconciliation
            .fetch_xor(true, Ordering::Relaxed)
        {
            [JobType::Reconcile, JobType::Convert]
        } else {
            [JobType::Convert, JobType::Reconcile]
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
                return if job_type == JobType::Convert {
                    self.process_claimed_job(ctx, job).await
                } else {
                    self.process_reconciliation_job(ctx, job).await
                };
            }
        }
        Ok(ConvertWorkerRun::NoJob)
    }

    pub async fn process_claimed_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<ConvertWorkerRun, ConvertWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(ConvertWorkerError::MissingLease)?
            .to_string();
        let attempts = job.attempts;
        let deadline = TokioInstant::now() + self.config.max_job_duration;
        match self
            .convert_claimed(ctx, &job, &lease_token, attempts, deadline)
            .await
        {
            Ok(ConversionOutput {
                markdown,
                markdown_sha256,
                markdown_len,
                source,
                identity,
                checkpoint,
            }) => {
                let byte_size = match i64::try_from(markdown_len) {
                    Ok(byte_size) => byte_size,
                    Err(_) => return Err(ConvertWorkerError::InvalidMarkdownLength),
                };
                if let Err(error) = self
                    .cleanup_checkpointed_staging(ctx, &identity, checkpoint.as_ref(), None)
                    .await
                {
                    self.enqueue_conversion_reconciliation(ctx, &job).await?;
                    let failed = jobs::fail(
                        &self.db_pool,
                        ctx,
                        job.id,
                        &lease_token,
                        attempts,
                        error.safe_job_error(),
                    )
                    .await?;
                    return Ok(ConvertWorkerRun::Failed {
                        job_id: failed.id,
                        terminal: failed.status == crate::db::models::JobStatus::DeadLetter,
                    });
                }
                let quota_reservation_key =
                    quota_reservation_key_for_attempt(&identity, job.id, attempts);
                if let Err(error) = self
                    .heartbeat_while(
                        ctx,
                        &job,
                        &lease_token,
                        attempts,
                        deadline,
                        quota::reserve(
                            &self.db_pool,
                            ctx,
                            &quota_reservation_key,
                            ResourceKind::StorageBytes,
                            markdown_len,
                            self.config.quota_reservation_ttl,
                            Some(job.id),
                        ),
                    )
                    .await
                {
                    return self
                        .fail_retryable_or_return(ctx, &job, &lease_token, attempts, error)
                        .await;
                }

                let mut staged: Option<StagedMarkdown> = None;
                let mut checkpoint_for_compensation = checkpoint.clone();
                let current_staging_key = artifacts::markdown_key(
                    &identity,
                    identity.promoted_version_id(),
                    job.id,
                    attempts,
                    &lease_token,
                )?;
                let current_staging_key_string = current_staging_key.as_str();
                let claimed = ClaimedJobScope {
                    ctx,
                    job: &job,
                    lease_token: &lease_token,
                    attempts,
                    deadline,
                };
                let promotion_result = async {
                    let saved = self
                        .save_checkpoint_payload(
                            &claimed,
                            checkpoint_with_staged_key(
                                checkpoint.as_ref(),
                                &identity,
                                &current_staging_key_string,
                            ),
                        )
                        .await?;
                    checkpoint_for_compensation = saved.checkpoint;
                    let staged_markdown = self
                        .heartbeat_while(
                            ctx,
                            &job,
                            &lease_token,
                            attempts,
                            deadline,
                            artifacts::stage_markdown(
                                &self.storage,
                                ctx,
                                MarkdownStageInput {
                                    collection_id: None,
                                    document_id: source.document_id,
                                    promoted_version_id: identity.promoted_version_id(),
                                    staging_key: current_staging_key.clone(),
                                    markdown,
                                    markdown_sha256: markdown_sha256.clone(),
                                    markdown_len,
                                },
                            ),
                        )
                        .await?;
                    if self.config.lose_staged_handle_after_put {
                        return Err(ConvertWorkerError::Promotion(PromotionError::Injected(
                            PromotionFault::AfterStagingPut,
                        )));
                    }
                    staged = Some(staged_markdown.clone());
                    self.save_checkpoint_step(
                        &claimed,
                        &identity,
                        checkpoint_for_compensation.as_ref(),
                        ConversionStep::Staged,
                    )
                    .await?;
                    if let Some(pause) = &self.config.pause_after_staging {
                        pause.staged.notify_waiters();
                        pause.release.notified().await;
                    }
                    if self.config.promotion_fault == Some(PromotionFault::AfterStagingPut) {
                        return Err(ConvertWorkerError::Promotion(PromotionError::Injected(
                            PromotionFault::AfterStagingPut,
                        )));
                    }
                    if !self.config.post_upload_settlement_delay.is_zero() {
                        let delay = self.config.post_upload_settlement_delay;
                        self.heartbeat_while(ctx, &job, &lease_token, attempts, deadline, async {
                            tokio::time::sleep(delay).await;
                            Ok::<(), JobError>(())
                        })
                        .await?;
                    }
                    Ok::<PromotionWait, ConvertWorkerError>(
                        self.await_promotion_or_reconciliation(
                            ctx,
                            &job,
                            &lease_token,
                            attempts,
                            deadline,
                            PromoteConversionInput {
                                job_id: job.id,
                                lease_token: lease_token.to_string(),
                                claimed_attempts: attempts,
                                identity: identity.clone(),
                                source: source.clone(),
                                artifact_id: identity.markdown_artifact_id(),
                                staged_object_key: staged_markdown.object_key.clone(),
                                markdown_sha256: markdown_sha256.clone(),
                                markdown_byte_size: byte_size,
                                quota_reservation_key: quota_reservation_key.clone(),
                                fault: self.config.promotion_fault,
                            },
                        )
                        .await,
                    )
                }
                .await;

                let completed = match promotion_result {
                    Ok(PromotionWait::Finished(Ok(outcome))) => outcome,
                    Ok(PromotionWait::ReconciliationNeeded) => {
                        self.enqueue_conversion_reconciliation(ctx, &job).await?;
                        return Ok(ConvertWorkerRun::ReconciliationNeeded { job_id: job.id });
                    }
                    Ok(PromotionWait::Finished(Err(PromotionError::LeaseLost)))
                    | Err(ConvertWorkerError::LeaseLost)
                    | Err(ConvertWorkerError::Job(JobError::LeaseLost)) => {
                        self.enqueue_conversion_reconciliation(ctx, &job).await?;
                        return Ok(ConvertWorkerRun::LeaseLost { job_id: job.id });
                    }
                    Ok(PromotionWait::Finished(Err(error))) => {
                        let compensation = self
                            .compensate_after_promotion_failure(
                                ctx,
                                &identity,
                                checkpoint_for_compensation.as_ref(),
                                staged.as_ref().map(|staged| staged.object_key.as_str()),
                                &quota_reservation_key,
                            )
                            .await;
                        if compensation.is_err() {
                            self.enqueue_conversion_reconciliation(ctx, &job).await?;
                        }
                        let job_error = compensation
                            .err()
                            .unwrap_or_else(|| ConvertWorkerError::Promotion(error));
                        if !job_error.is_retryable_job_failure() {
                            return Err(job_error);
                        }
                        let failed = jobs::fail(
                            &self.db_pool,
                            ctx,
                            job.id,
                            &lease_token,
                            attempts,
                            job_error.safe_job_error(),
                        )
                        .await?;
                        return Ok(ConvertWorkerRun::Failed {
                            job_id: failed.id,
                            terminal: failed.status == crate::db::models::JobStatus::DeadLetter,
                        });
                    }
                    Err(error) => {
                        let compensation = self
                            .compensate_after_promotion_failure(
                                ctx,
                                &identity,
                                checkpoint_for_compensation.as_ref(),
                                staged.as_ref().map(|staged| staged.object_key.as_str()),
                                &quota_reservation_key,
                            )
                            .await;
                        if compensation.is_err() {
                            self.enqueue_conversion_reconciliation(ctx, &job).await?;
                        }
                        let job_error = compensation.err().unwrap_or(error);
                        if !job_error.is_retryable_job_failure() {
                            return Err(job_error);
                        }
                        let failed = jobs::fail(
                            &self.db_pool,
                            ctx,
                            job.id,
                            &lease_token,
                            attempts,
                            job_error.safe_job_error(),
                        )
                        .await?;
                        return Ok(ConvertWorkerRun::Failed {
                            job_id: failed.id,
                            terminal: failed.status == crate::db::models::JobStatus::DeadLetter,
                        });
                    }
                };
                Ok(ConvertWorkerRun::Completed {
                    job_id: completed.job.id,
                    markdown_bytes: markdown_len as usize,
                })
            }
            Err(ConvertWorkerError::LeaseLost) => {
                Ok(ConvertWorkerRun::LeaseLost { job_id: job.id })
            }
            Err(ConvertWorkerError::SandboxCancelled) => {
                Ok(ConvertWorkerRun::LeaseLost { job_id: job.id })
            }
            Err(error) if error.is_retryable_job_failure() => {
                let failed = jobs::fail(
                    &self.db_pool,
                    ctx,
                    job.id,
                    &lease_token,
                    attempts,
                    error.safe_job_error(),
                )
                .await?;
                Ok(ConvertWorkerRun::Failed {
                    job_id: failed.id,
                    terminal: failed.status == crate::db::models::JobStatus::DeadLetter,
                })
            }
            Err(error) => Err(error),
        }
    }

    async fn process_reconciliation_job(
        &self,
        ctx: &OrgContext,
        job: Job,
    ) -> Result<ConvertWorkerRun, ConvertWorkerError> {
        let lease_token = job
            .lease_owner
            .as_deref()
            .ok_or(ConvertWorkerError::MissingLease)?;
        let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
        let parent_job_id = payload
            .cleanup_target_job_id
            .ok_or(ConvertWorkerError::InvalidReconciliationPayload)?;
        let parent = self.reconciliation_parent(ctx, parent_job_id).await?;
        if parent.job_type != JobType::Convert {
            return Err(ConvertWorkerError::InvalidReconciliationPayload);
        }
        if matches!(
            parent.status,
            JobStatus::Pending | JobStatus::Leased | JobStatus::Running
        ) {
            let failed = jobs::fail(
                &self.db_pool,
                ctx,
                job.id,
                lease_token,
                job.attempts,
                "conversion cleanup waiting for parent terminal state",
            )
            .await?;
            return Ok(ConvertWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            });
        }

        let parent_payload =
            jobs::decode_job_payload(parent.payload_version, parent.payload.clone())?;
        let document_id = parent_payload
            .document_id
            .ok_or(ConvertWorkerError::InvalidReconciliationPayload)?;
        let source_version_id = parent_payload
            .version_id
            .ok_or(ConvertWorkerError::InvalidReconciliationPayload)?;
        let identity = ConversionIdentity::new(
            ctx.org_id(),
            document_id,
            source_version_id,
            parent.idempotency_key.clone(),
        );
        let cleanup_result = async {
            self.cleanup_checkpointed_staging(ctx, &identity, parent.checkpoint.as_ref(), None)
                .await?;
            self.refund_attempt_reservations(ctx, &identity, &parent)
                .await
        }
        .await;
        if let Err(error) = cleanup_result {
            let failed = jobs::fail(
                &self.db_pool,
                ctx,
                job.id,
                lease_token,
                job.attempts,
                error.safe_job_error(),
            )
            .await?;
            return Ok(ConvertWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == JobStatus::DeadLetter,
            });
        }
        let completed =
            jobs::complete(&self.db_pool, ctx, job.id, lease_token, job.attempts).await?;
        Ok(ConvertWorkerRun::Reconciled {
            job_id: completed.id,
        })
    }

    async fn await_promotion_or_reconciliation(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
        input: PromoteConversionInput,
    ) -> PromotionWait {
        // Once the promote transaction starts, dropping its future leaves commit
        // status unknowable. Every cancellation/transport path therefore becomes
        // reconciliation work rather than an immediate object deletion.
        if self
            .heartbeat_once(ctx, job, lease_token, attempts, deadline)
            .await
            .is_err()
        {
            return PromotionWait::ReconciliationNeeded;
        }
        let promotion = promotion::promote_conversion(&self.db_pool, ctx, input);
        tokio::pin!(promotion);
        let mut heartbeat = heartbeat_interval(self.config.heartbeat_interval);
        loop {
            tokio::select! {
                biased;
                result = &mut promotion => {
                    return match result {
                        Err(PromotionError::Db(DbError::Pool(_) | DbError::Query(_)))
                        | Err(PromotionError::CommittedOutcomeUnknown) => {
                            PromotionWait::ReconciliationNeeded
                        }
                        result => PromotionWait::Finished(result),
                    };
                }
                _ = sleep_until(deadline) => return PromotionWait::ReconciliationNeeded,
                _ = heartbeat.tick() => {
                    let heartbeat = timeout(
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
                    .await;
                    if !matches!(heartbeat, Ok(Ok(()))) {
                        return PromotionWait::ReconciliationNeeded;
                    }
                }
            }
        }
    }

    async fn enqueue_conversion_reconciliation(
        &self,
        ctx: &OrgContext,
        parent_job: &Job,
    ) -> Result<(), ConvertWorkerError> {
        let document_id = parent_job
            .document_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        let version_id = parent_job
            .version_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        let mut input = EnqueueJob::new(
            JobType::Reconcile,
            JobPayload {
                document_id: Some(document_id),
                version_id: Some(version_id),
                collection_id: None,
                upload_id: None,
                batch_id: None,
                cleanup_target_job_id: Some(parent_job.id),
            },
            format!("convert.cleanup:{}", parent_job.id),
        );
        input.max_attempts = RECONCILIATION_MAX_ATTEMPTS;
        jobs::enqueue(&self.db_pool, ctx, input).await?;
        Ok(())
    }

    async fn reconciliation_parent(
        &self,
        ctx: &OrgContext,
        parent_job_id: Uuid,
    ) -> Result<Job, ConvertWorkerError> {
        with_org_txn(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    jobs_repo::get_by_id_for_update(txn, &ctx, parent_job_id)
                        .await?
                        .ok_or(DbError::NotFound)
                })
            }
        })
        .await
        .map_err(ConvertWorkerError::Db)
    }

    async fn refund_attempt_reservations(
        &self,
        ctx: &OrgContext,
        identity: &ConversionIdentity,
        parent: &Job,
    ) -> Result<(), ConvertWorkerError> {
        for attempts in 1..=parent.attempts.max(0) {
            let reservation_key = quota_reservation_key_for_attempt(identity, parent.id, attempts);
            match quota::refund(&self.db_pool, ctx, &reservation_key).await {
                Ok(_) | Err(QuotaError::ReservationNotFound) => {}
                Err(error) => return Err(ConvertWorkerError::Quota(error)),
            }
        }
        Ok(())
    }

    async fn convert_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
    ) -> Result<ConversionOutput, ConvertWorkerError> {
        let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
        let source = with_deadline(deadline, self.load_source(ctx, payload)).await?;
        let identity = ConversionIdentity::new(
            ctx.org_id(),
            source.document_id,
            source.source_version_id,
            job.idempotency_key.clone(),
        );
        let claimed = ClaimedJobScope {
            ctx,
            job,
            lease_token,
            attempts,
            deadline,
        };
        let mut checkpoint = job.checkpoint.clone();
        let quarantine_key = parse_key_for_org(&source.original_object_key, ctx.org_id())?;
        if quarantine_key.namespace() != ObjectNamespace::Quarantine {
            return Err(ConvertWorkerError::SourceNotQuarantine);
        }
        let metadata = self
            .heartbeat_while(
                ctx,
                job,
                lease_token,
                attempts,
                deadline,
                self.storage.head_metadata(ctx.org_id(), &quarantine_key),
            )
            .await?;
        self.verify_quarantine_metadata(&source, &metadata)?;
        let format = metadata
            .get("canonical-format")
            .or_else(|| metadata.get("x-amz-meta-canonical-format"))
            .ok_or(ConvertWorkerError::MissingCanonicalFormat)?;
        if is_audio_format(format) {
            return Err(ConvertWorkerError::AudioConversionDisabled);
        }
        let canonical_extension =
            canonical_extension(format).ok_or(ConvertWorkerError::UnsupportedCanonicalFormat)?;
        let input = self
            .heartbeat_while(
                ctx,
                job,
                lease_token,
                attempts,
                deadline,
                self.storage.get_object(ctx.org_id(), &quarantine_key),
            )
            .await?;
        let source_for_prepare = source.clone();
        let metadata_for_prepare = metadata.clone();
        let canonical_extension = canonical_extension.to_string();
        let sandbox_input = self
            .heartbeat_while(ctx, job, lease_token, attempts, deadline, async move {
                tokio::task::spawn_blocking(move || {
                    verify_downloaded_integrity(
                        &source_for_prepare,
                        input.as_ref(),
                        &metadata_for_prepare,
                    )?;
                    Ok::<_, ConvertWorkerError>(SandboxInput {
                        bytes: input.to_vec(),
                        canonical_extension,
                    })
                })
                .await
                .map_err(|_| ConvertWorkerError::SandboxJoin)?
            })
            .await?;
        let saved = self
            .save_checkpoint_step(
                &claimed,
                &identity,
                checkpoint.as_ref(),
                ConversionStep::Downloaded,
            )
            .await?;
        checkpoint = saved.checkpoint;
        let sandbox_output = self
            .run_sandbox_with_heartbeat(ctx, job, lease_token, attempts, deadline, sandbox_input)
            .await?;
        if sandbox_output.stdout_truncated || sandbox_output.stderr_truncated {
            return Err(ConvertWorkerError::SandboxOutputTruncated);
        }
        match sandbox_output.exit {
            SandboxExit::Success => {}
            SandboxExit::TimedOut => return Err(ConvertWorkerError::SandboxTimedOut),
            SandboxExit::Cancelled => return Err(ConvertWorkerError::SandboxCancelled),
            SandboxExit::Exit(_) | SandboxExit::Signaled(_) => {
                return Err(ConvertWorkerError::ConverterFailed);
            }
        }
        let markdown = sandbox_output.stdout;
        let markdown_sha256 = hex::encode(Sha256::digest(&markdown));
        let markdown_len =
            u64::try_from(markdown.len()).map_err(|_| ConvertWorkerError::InvalidMarkdownLength)?;
        let saved = self
            .save_checkpoint_step(
                &claimed,
                &identity,
                checkpoint.as_ref(),
                ConversionStep::Converted,
            )
            .await?;
        Ok(ConversionOutput {
            markdown,
            markdown_sha256,
            markdown_len,
            source,
            identity,
            checkpoint: saved.checkpoint,
        })
    }

    fn verify_quarantine_metadata(
        &self,
        source: &ConversionSourceVersion,
        metadata: &HashMap<String, String>,
    ) -> Result<(), ConvertWorkerError> {
        match metadata.get("disposition").map(String::as_str) {
            Some("accepted" | "quarantined") => {}
            _ => return Err(ConvertWorkerError::InvalidQuarantineMetadata),
        }
        if metadata.get("content-sha256").map(String::as_str)
            != Some(source.content_sha256.as_str())
        {
            return Err(ConvertWorkerError::InvalidQuarantineMetadata);
        }
        let version_id = source.source_version_id.to_string();
        if metadata.get("version-id").map(String::as_str) != Some(version_id.as_str()) {
            return Err(ConvertWorkerError::InvalidQuarantineMetadata);
        }
        let document_id = source.document_id.to_string();
        if metadata.get("document-id").map(String::as_str) != Some(document_id.as_str()) {
            return Err(ConvertWorkerError::InvalidQuarantineMetadata);
        }
        Ok(())
    }

    async fn run_sandbox_with_heartbeat(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        deadline: TokioInstant,
        input: SandboxInput,
    ) -> Result<SandboxOutput, ConvertWorkerError> {
        self.heartbeat_once(ctx, job, lease_token, attempts, deadline)
            .await?;
        let cancel = SandboxCancel::default();
        let sandbox_config = self.config.sandbox.clone();
        let sandbox_cancel = cancel.clone();
        let mut handle = tokio::task::spawn_blocking(move || {
            sandbox::run(&sandbox_config, input, &sandbox_cancel)
        });
        let mut heartbeat = heartbeat_interval(self.config.heartbeat_interval);
        loop {
            tokio::select! {
                biased;
                result = &mut handle => {
                    return result
                        .map_err(|_| ConvertWorkerError::SandboxJoin)?
                        .map_err(ConvertWorkerError::Sandbox);
                }
                _ = sleep_until(deadline) => {
                    cancel.cancel();
                    let _ = await_cancelled_sandbox(&mut handle).await;
                    return Err(ConvertWorkerError::JobTimedOut);
                }
                _ = heartbeat.tick() => {
                    match timeout(heartbeat_call_timeout(self.config.lease_ttl, deadline), jobs::heartbeat(
                        &self.db_pool,
                        ctx,
                        job.id,
                        lease_token,
                        attempts,
                        self.config.lease_ttl,
                    ))
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(JobError::LeaseLost)) => {
                            cancel.cancel();
                            let _ = await_cancelled_sandbox(&mut handle).await;
                            return Err(ConvertWorkerError::LeaseLost);
                        }
                        Ok(Err(error)) => {
                            cancel.cancel();
                            let _ = await_cancelled_sandbox(&mut handle).await;
                            return Err(ConvertWorkerError::Job(error));
                        }
                        Err(_) => {
                            cancel.cancel();
                            let _ = await_cancelled_sandbox(&mut handle).await;
                            return Err(ConvertWorkerError::JobTimedOut);
                        }
                    }
                }
            }
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
    ) -> Result<T, ConvertWorkerError>
    where
        Fut: Future<Output = Result<T, E>>,
        E: Into<ConvertWorkerError>,
    {
        self.heartbeat_once(ctx, job, lease_token, attempts, deadline)
            .await?;
        tokio::pin!(future);
        let mut heartbeat = heartbeat_interval(self.config.heartbeat_interval);
        loop {
            tokio::select! {
                biased;
                result = &mut future => {
                    return result.map_err(Into::into);
                }
                _ = sleep_until(deadline) => {
                    return Err(ConvertWorkerError::JobTimedOut);
                }
                _ = heartbeat.tick() => {
                    match timeout(heartbeat_call_timeout(self.config.lease_ttl, deadline), jobs::heartbeat(
                        &self.db_pool,
                        ctx,
                        job.id,
                        lease_token,
                        attempts,
                        self.config.lease_ttl,
                    ))
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(JobError::LeaseLost)) => return Err(ConvertWorkerError::LeaseLost),
                        Ok(Err(error)) => return Err(ConvertWorkerError::Job(error)),
                        Err(_) => return Err(ConvertWorkerError::JobTimedOut),
                    }
                }
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
    ) -> Result<(), ConvertWorkerError> {
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
            Ok(Err(JobError::LeaseLost)) => Err(ConvertWorkerError::LeaseLost),
            Ok(Err(error)) => Err(ConvertWorkerError::Job(error)),
            Err(_) => Err(ConvertWorkerError::JobTimedOut),
        }
    }

    async fn load_source(
        &self,
        ctx: &OrgContext,
        payload: JobPayload,
    ) -> Result<ConversionSourceVersion, ConvertWorkerError> {
        let document_id = payload
            .document_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        let version_id = payload
            .version_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        with_org_txn(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    crate::db::document_versions::source_for_conversion(
                        txn,
                        &ctx,
                        document_id,
                        version_id,
                    )
                    .await
                })
            }
        })
        .await
        .map_err(ConvertWorkerError::Db)
    }

    async fn save_checkpoint_step(
        &self,
        claimed: &ClaimedJobScope<'_>,
        identity: &ConversionIdentity,
        existing: Option<&serde_json::Value>,
        step: ConversionStep,
    ) -> Result<Job, ConvertWorkerError> {
        let checkpoint = checkpoint_with_step(existing, identity, step);
        self.heartbeat_while(
            claimed.ctx,
            claimed.job,
            claimed.lease_token,
            claimed.attempts,
            claimed.deadline,
            jobs::checkpoint(
                &self.db_pool,
                claimed.ctx,
                claimed.job.id,
                claimed.lease_token,
                claimed.attempts,
                checkpoint,
            ),
        )
        .await
    }

    async fn save_checkpoint_payload(
        &self,
        claimed: &ClaimedJobScope<'_>,
        checkpoint: jobs::CheckpointPayload,
    ) -> Result<Job, ConvertWorkerError> {
        self.heartbeat_while(
            claimed.ctx,
            claimed.job,
            claimed.lease_token,
            claimed.attempts,
            claimed.deadline,
            jobs::checkpoint(
                &self.db_pool,
                claimed.ctx,
                claimed.job.id,
                claimed.lease_token,
                claimed.attempts,
                checkpoint,
            ),
        )
        .await
    }

    async fn cleanup_checkpointed_staging(
        &self,
        ctx: &OrgContext,
        identity: &ConversionIdentity,
        checkpoint: Option<&serde_json::Value>,
        extra_key: Option<&str>,
    ) -> Result<(), ConvertWorkerError> {
        let mut keys = staged_keys_from_checkpoint(checkpoint);
        if let Some(extra_key) = extra_key {
            if !keys.iter().any(|key| key == extra_key) {
                keys.push(extra_key.to_string());
            }
        }
        for key in keys {
            self.cleanup_staging_key_if_uncommitted(ctx, identity, &key)
                .await?;
        }
        Ok(())
    }

    async fn cleanup_staging_key_if_uncommitted(
        &self,
        ctx: &OrgContext,
        identity: &ConversionIdentity,
        object_key: &str,
    ) -> Result<(), ConvertWorkerError> {
        if let Some(committed) =
            promotion::committed_markdown_object_key(&self.db_pool, ctx, identity).await?
        {
            if committed == object_key {
                return Ok(());
            }
        }
        if self.config.fail_cleanup_delete {
            eprintln!(
                "fileconv-server: injected staged conversion artifact cleanup failure; key={object_key}"
            );
            return Err(ConvertWorkerError::CompensationDeferred);
        }
        let key = parse_key_for_org(object_key, ctx.org_id())?;
        if !key.belongs_to_version(identity.promoted_version_id()) {
            return Err(ConvertWorkerError::CompensationDeferred);
        }
        if let Err(error) = self
            .storage
            .cleanup_generated_object(ctx.org_id(), &key)
            .await
        {
            eprintln!(
                "fileconv-server: staged conversion artifact cleanup failed; key={} code={}",
                object_key,
                error.code()
            );
            return Err(ConvertWorkerError::CompensationDeferred);
        }
        Ok(())
    }

    async fn compensate_after_promotion_failure(
        &self,
        ctx: &OrgContext,
        identity: &ConversionIdentity,
        checkpoint: Option<&serde_json::Value>,
        staged_key: Option<&str>,
        quota_reservation_key: &str,
    ) -> Result<(), ConvertWorkerError> {
        let mut deferred = false;
        if let Err(error) = self
            .cleanup_checkpointed_staging(ctx, identity, checkpoint, staged_key)
            .await
        {
            eprintln!(
                "fileconv-server: conversion staging cleanup deferred: {}",
                error.safe_job_error()
            );
            deferred = true;
        }
        if self.config.fail_quota_refund {
            eprintln!(
                "fileconv-server: injected conversion quota refund failure; reservation_key={quota_reservation_key}"
            );
            // TODO(I07): durable dead-letter GC/reconciliation should consume the
            // checkpointed staging keys and quota marker if retries are exhausted.
            deferred = true;
        } else if let Err(error) = quota::refund(&self.db_pool, ctx, quota_reservation_key).await {
            eprintln!(
                "fileconv-server: conversion quota refund failed; reservation_key={} code={}",
                quota_reservation_key,
                error.code()
            );
            // I02 quota reservations are bounded by expires_at; the sweep path is
            // the durable backstop if inline refund cannot settle this attempt.
            // TODO(I07): emit/consume a durable conversion-cleanup marker for
            // permanently dead-lettered conversion attempts.
            deferred = true;
        }
        if deferred {
            Err(ConvertWorkerError::CompensationDeferred)
        } else {
            Ok(())
        }
    }

    async fn fail_retryable_or_return(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        error: ConvertWorkerError,
    ) -> Result<ConvertWorkerRun, ConvertWorkerError> {
        if error.is_retryable_job_failure() {
            let failed = jobs::fail(
                &self.db_pool,
                ctx,
                job.id,
                lease_token,
                attempts,
                error.safe_job_error(),
            )
            .await?;
            Ok(ConvertWorkerRun::Failed {
                job_id: failed.id,
                terminal: failed.status == crate::db::models::JobStatus::DeadLetter,
            })
        } else {
            Err(error)
        }
    }
}

async fn with_deadline<T, E, Fut>(
    deadline: TokioInstant,
    future: Fut,
) -> Result<T, ConvertWorkerError>
where
    Fut: Future<Output = Result<T, E>>,
    E: Into<ConvertWorkerError>,
{
    match timeout_at(deadline, future).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(ConvertWorkerError::JobTimedOut),
    }
}

fn verify_downloaded_integrity(
    source: &ConversionSourceVersion,
    bytes: &[u8],
    metadata: &HashMap<String, String>,
) -> Result<(), ConvertWorkerError> {
    let expected_size = source
        .byte_size
        .ok_or(ConvertWorkerError::InvalidQuarantineMetadata)?;
    if expected_size < 0 || bytes.len() as i64 != expected_size {
        return Err(ConvertWorkerError::InvalidQuarantineMetadata);
    }
    let meta_len = metadata
        .get("content-length-bytes")
        .ok_or(ConvertWorkerError::InvalidQuarantineMetadata)?
        .parse::<usize>()
        .map_err(|_| ConvertWorkerError::InvalidQuarantineMetadata)?;
    if meta_len != bytes.len() {
        return Err(ConvertWorkerError::InvalidQuarantineMetadata);
    }
    let actual_sha256 = hex::encode(Sha256::digest(bytes));
    if actual_sha256 != source.content_sha256.as_str() {
        return Err(ConvertWorkerError::InvalidQuarantineMetadata);
    }
    Ok(())
}

async fn await_cancelled_sandbox(
    handle: &mut JoinHandle<Result<SandboxOutput, SandboxError>>,
) -> Result<(), ConvertWorkerError> {
    match timeout(SANDBOX_CANCEL_REAP_TIMEOUT, handle).await {
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(error))) => Err(ConvertWorkerError::Sandbox(error)),
        Ok(Err(_)) => Err(ConvertWorkerError::SandboxJoin),
        Err(_) => Err(ConvertWorkerError::JobTimedOut),
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

pub fn artifact_object_id_for_attempt(job_id: Uuid, attempts: i32) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"markhand-convert-artifact-attempt-v1");
    hasher.update(job_id.as_bytes());
    hasher.update(attempts.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn quota_reservation_key_for_attempt(
    identity: &ConversionIdentity,
    job_id: Uuid,
    attempts: i32,
) -> String {
    format!(
        "{}.{}.{}",
        identity.storage_quota_reservation_key(),
        job_id,
        attempts.max(0)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertWorkerRun {
    NoJob,
    Completed {
        job_id: Uuid,
        markdown_bytes: usize,
    },
    Failed {
        job_id: Uuid,
        terminal: bool,
    },
    LeaseLost {
        job_id: Uuid,
    },
    /// Promotion completion is uncertain; a durable reconciliation job was queued.
    ReconciliationNeeded {
        job_id: Uuid,
    },
    Reconciled {
        job_id: Uuid,
    },
}

#[derive(Debug)]
struct ConversionOutput {
    markdown: Vec<u8>,
    markdown_sha256: String,
    markdown_len: u64,
    source: ConversionSourceVersion,
    identity: ConversionIdentity,
    checkpoint: Option<serde_json::Value>,
}

#[derive(Debug, Error)]
pub enum ConvertWorkerError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("artifact staging error")]
    ArtifactStage(#[from] artifacts::ArtifactStageError),
    #[error("promotion error")]
    Promotion(#[from] PromotionError),
    #[error("quota error")]
    Quota(#[from] QuotaError),
    #[error("conversion compensation is deferred")]
    CompensationDeferred,
    #[error("sandbox error")]
    Sandbox(#[from] SandboxError),
    #[error("sandbox task failed")]
    SandboxJoin,
    #[error("sandbox timed out")]
    SandboxTimedOut,
    #[error("sandbox was cancelled")]
    SandboxCancelled,
    #[error("sandbox output exceeded configured cap")]
    SandboxOutputTruncated,
    #[error("converter exited unsuccessfully")]
    ConverterFailed,
    #[error("job lease was lost")]
    LeaseLost,
    #[error("claimed job is missing a lease token")]
    MissingLease,
    #[error("convert payload is missing document_id or version_id")]
    InvalidConvertPayload,
    #[error("reconciliation payload is invalid")]
    InvalidReconciliationPayload,
    #[error("quarantine object is missing canonical format metadata")]
    MissingCanonicalFormat,
    #[error("quarantine object has unsupported canonical format metadata")]
    UnsupportedCanonicalFormat,
    #[error("markdown output is too large")]
    InvalidMarkdownLength,
    #[error("worker heartbeat interval must be <= one third of lease ttl")]
    InvalidHeartbeatConfig,
    #[error("worker max job duration must be at least the sandbox wall timeout")]
    InvalidMaxJobDuration,
    #[error("convert job exceeded configured maximum duration")]
    JobTimedOut,
    #[error("source object is not in quarantine namespace")]
    SourceNotQuarantine,
    #[error("quarantine metadata failed identity or integrity validation")]
    InvalidQuarantineMetadata,
    #[error("audio conversion is disabled for the isolated worker")]
    AudioConversionDisabled,
}

impl ConvertWorkerError {
    fn is_retryable_job_failure(&self) -> bool {
        matches!(
            self,
            Self::Db(_)
                | Self::Storage(_)
                | Self::ArtifactStage(_)
                | Self::Promotion(_)
                | Self::Quota(_)
                | Self::CompensationDeferred
                | Self::Sandbox(_)
                | Self::SandboxJoin
                | Self::SandboxTimedOut
                | Self::SandboxOutputTruncated
                | Self::ConverterFailed
                | Self::InvalidConvertPayload
                | Self::InvalidReconciliationPayload
                | Self::MissingCanonicalFormat
                | Self::UnsupportedCanonicalFormat
                | Self::InvalidMarkdownLength
                | Self::JobTimedOut
                | Self::AudioConversionDisabled
                | Self::SourceNotQuarantine
                | Self::InvalidQuarantineMetadata
        )
    }

    fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "convert database error",
            Self::Storage(_) => "convert storage error",
            Self::ArtifactStage(_) => "convert artifact staging error",
            Self::Promotion(_) => "convert promotion error",
            Self::Quota(_) => "convert quota error",
            Self::CompensationDeferred => "convert compensation deferred",
            Self::Sandbox(_) => "convert sandbox error",
            Self::SandboxJoin => "convert sandbox join error",
            Self::SandboxTimedOut => "convert sandbox timeout",
            Self::SandboxCancelled => "convert sandbox cancelled",
            Self::SandboxOutputTruncated => "convert sandbox output truncated",
            Self::ConverterFailed => "converter exited unsuccessfully",
            Self::LeaseLost => "convert lease lost",
            Self::Job(_) => "convert job error",
            Self::MissingLease => "convert missing lease",
            Self::InvalidConvertPayload => "convert payload invalid",
            Self::InvalidReconciliationPayload => "convert reconciliation payload invalid",
            Self::MissingCanonicalFormat => "convert canonical format missing",
            Self::UnsupportedCanonicalFormat => "convert canonical format unsupported",
            Self::InvalidMarkdownLength => "convert markdown too large",
            Self::InvalidHeartbeatConfig => "convert heartbeat config invalid",
            Self::InvalidMaxJobDuration => "convert max job duration invalid",
            Self::JobTimedOut => "convert job timed out",
            Self::SourceNotQuarantine => "convert source not quarantine",
            Self::InvalidQuarantineMetadata => "convert quarantine metadata invalid",
            Self::AudioConversionDisabled => "convert audio disabled",
        }
    }
}

pub fn canonical_extension(format: &str) -> Option<&'static str> {
    match format {
        "pdf" => Some("pdf"),
        "docx" => Some("docx"),
        "pptx" => Some("pptx"),
        "xlsx" => Some("xlsx"),
        "ods" => Some("ods"),
        "xls" => Some("xls"),
        "xlsb" => Some("xlsb"),
        "csv" => Some("csv"),
        "html" => Some("html"),
        "txt" => Some("txt"),
        "png" => Some("png"),
        "jpeg" => Some("jpg"),
        "jpg" => Some("jpg"),
        "webp" => Some("webp"),
        "tiff" => Some("tiff"),
        "bmp" => Some("bmp"),
        "wav" | "mp3" | "ogg" | "flac" | "m4a" => None,
        "zip" => Some("zip"),
        _ => None,
    }
}

fn is_audio_format(format: &str) -> bool {
    matches!(format, "wav" | "mp3" | "ogg" | "flac" | "m4a")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_extensions_are_server_derived() {
        assert_eq!(canonical_extension("jpeg"), Some("jpg"));
        assert_eq!(canonical_extension("../../etc/passwd"), None);
        assert_eq!(canonical_extension("txt"), Some("txt"));
        assert_eq!(canonical_extension("mp3"), None);
    }
}
