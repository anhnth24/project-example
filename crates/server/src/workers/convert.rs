//! Converter job worker.

use std::time::Duration;

use bytes::Bytes;
use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::time::interval;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::documents::{self, NewMarkdownArtifact};
use crate::db::error::DbError;
use crate::db::models::{Job, JobType};
use crate::db::pool::with_org_txn;
use crate::jobs::{self, JobError, JobPayload};
use crate::storage::keys::{parse_key_for_org, trusted_key};
use crate::storage::minio::{MinioClient, ObjectIdentityMeta};
use crate::storage::StorageError;

use super::limits::ResourceLimits;
use super::sandbox::{
    self, SandboxCancel, SandboxConfig, SandboxError, SandboxExit, SandboxInput, SandboxOutput,
};

const DEFAULT_CLAIM_LIMIT: u32 = 1;
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;

#[derive(Debug, Clone)]
pub struct ConvertWorkerConfig {
    pub worker_id: String,
    pub claim_limit: u32,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub sandbox: SandboxConfig,
}

impl ConvertWorkerConfig {
    pub fn new(worker_id: impl Into<String>, sandbox: SandboxConfig) -> Self {
        Self {
            worker_id: worker_id.into(),
            claim_limit: DEFAULT_CLAIM_LIMIT,
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            sandbox,
        }
    }

    pub fn default_for_fileconv(worker_id: impl Into<String>, lease_ttl: Duration) -> Self {
        Self {
            worker_id: worker_id.into(),
            claim_limit: DEFAULT_CLAIM_LIMIT,
            lease_ttl,
            heartbeat_interval: Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            sandbox: SandboxConfig {
                argv_template: vec!["fileconv".into(), "one".into(), "{input}".into()],
                limits: ResourceLimits::default(),
            },
        }
    }
}

#[derive(Clone)]
pub struct ConvertWorker {
    db_pool: Pool,
    storage: MinioClient,
    config: ConvertWorkerConfig,
}

impl ConvertWorker {
    pub fn new(
        db_pool: Pool,
        storage: MinioClient,
        config: ConvertWorkerConfig,
    ) -> Result<Self, ConvertWorkerError> {
        config
            .sandbox
            .validate()
            .map_err(ConvertWorkerError::Sandbox)?;
        sandbox::preflight().map_err(ConvertWorkerError::Sandbox)?;
        Ok(Self {
            db_pool,
            storage,
            config,
        })
    }

    pub async fn run_once(&self, ctx: &OrgContext) -> Result<ConvertWorkerRun, ConvertWorkerError> {
        let jobs = jobs::claim_type(
            &self.db_pool,
            ctx,
            JobType::Convert,
            &self.config.worker_id,
            self.config.claim_limit,
            self.config.lease_ttl,
        )
        .await?;
        let Some(job) = jobs.into_iter().next() else {
            return Ok(ConvertWorkerRun::NoJob);
        };
        self.process_claimed_job(ctx, job).await
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
        match self
            .convert_claimed(ctx, &job, &lease_token, attempts)
            .await
        {
            Ok(ConversionSuccess {
                markdown,
                artifact_key,
                markdown_sha256,
                markdown_len,
            }) => {
                let stored = match self
                    .record_markdown_artifact(
                        ctx,
                        &job,
                        &artifact_key,
                        &markdown_sha256,
                        markdown_len,
                    )
                    .await
                {
                    Ok(stored) => stored,
                    Err(error) if error.is_retryable_job_failure() => {
                        let _ = self
                            .storage
                            .cleanup_generated_object(ctx.org_id(), &artifact_key)
                            .await;
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
                    Err(error) => return Err(error),
                };
                if !stored.created && stored.object_key != artifact_key.as_str() {
                    let _ = self
                        .storage
                        .cleanup_generated_object(ctx.org_id(), &artifact_key)
                        .await;
                }
                let completed = match jobs::complete(
                    &self.db_pool,
                    ctx,
                    job.id,
                    &lease_token,
                    attempts,
                )
                .await
                {
                    Ok(job) => job,
                    Err(JobError::LeaseLost) => {
                        return Ok(ConvertWorkerRun::LeaseLost { job_id: job.id });
                    }
                    Err(error) => return Err(ConvertWorkerError::Job(error)),
                };
                Ok(ConvertWorkerRun::Completed {
                    job_id: completed.id,
                    markdown_bytes: markdown.len(),
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

    async fn convert_claimed(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
    ) -> Result<ConversionSuccess, ConvertWorkerError> {
        let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
        let source = self.load_source(ctx, payload).await?;
        let quarantine_key = parse_key_for_org(&source.original_object_key, ctx.org_id())?;
        let metadata = self
            .storage
            .head_metadata(ctx.org_id(), &quarantine_key)
            .await?;
        let format = metadata
            .get("canonical-format")
            .or_else(|| metadata.get("x-amz-meta-canonical-format"))
            .ok_or(ConvertWorkerError::MissingCanonicalFormat)?;
        let canonical_extension =
            canonical_extension(format).ok_or(ConvertWorkerError::UnsupportedCanonicalFormat)?;
        let input = self
            .storage
            .get_object(ctx.org_id(), &quarantine_key)
            .await?;
        let sandbox_output = self
            .run_sandbox_with_heartbeat(
                ctx,
                job,
                lease_token,
                attempts,
                SandboxInput {
                    bytes: input.to_vec(),
                    canonical_extension: canonical_extension.to_string(),
                },
            )
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
        let artifact_key = trusted_key(ctx.org_id(), source.version_id, Uuid::new_v4(), None)?;
        let meta = ObjectIdentityMeta {
            org_id: ctx.org_id(),
            collection_id: None,
            document_id: Some(source.document_id),
            version_id: Some(source.version_id),
            original_filename: None,
            canonical_format: Some("md".into()),
            content_sha256: Some(markdown_sha256.clone()),
            content_length: Some(markdown_len),
            disposition: Some("trusted".into()),
        };
        self.storage
            .put_object(
                ctx.org_id(),
                &artifact_key,
                Bytes::from(markdown.clone()),
                &meta,
                "text/markdown; charset=utf-8",
            )
            .await?;
        Ok(ConversionSuccess {
            markdown,
            artifact_key,
            markdown_sha256,
            markdown_len,
        })
    }

    async fn run_sandbox_with_heartbeat(
        &self,
        ctx: &OrgContext,
        job: &Job,
        lease_token: &str,
        attempts: i32,
        input: SandboxInput,
    ) -> Result<SandboxOutput, ConvertWorkerError> {
        let cancel = SandboxCancel::default();
        let sandbox_config = self.config.sandbox.clone();
        let sandbox_cancel = cancel.clone();
        let mut handle = tokio::task::spawn_blocking(move || {
            sandbox::run(&sandbox_config, input, &sandbox_cancel)
        });
        let mut heartbeat = interval(self.config.heartbeat_interval);
        loop {
            tokio::select! {
                result = &mut handle => {
                    return result
                        .map_err(|_| ConvertWorkerError::SandboxJoin)?
                        .map_err(ConvertWorkerError::Sandbox);
                }
                _ = heartbeat.tick() => {
                    match jobs::heartbeat(
                        &self.db_pool,
                        ctx,
                        job.id,
                        lease_token,
                        attempts,
                        self.config.lease_ttl,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(JobError::LeaseLost) => {
                            cancel.cancel();
                            let _ = handle.await;
                            return Err(ConvertWorkerError::LeaseLost);
                        }
                        Err(error) => {
                            cancel.cancel();
                            let _ = handle.await;
                            return Err(ConvertWorkerError::Job(error));
                        }
                    }
                }
            }
        }
    }

    async fn load_source(
        &self,
        ctx: &OrgContext,
        payload: JobPayload,
    ) -> Result<documents::VersionSource, ConvertWorkerError> {
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
                    documents::get_version_source_for_convert(txn, &ctx, document_id, version_id)
                        .await
                })
            }
        })
        .await
        .map_err(ConvertWorkerError::Db)
    }

    async fn record_markdown_artifact(
        &self,
        ctx: &OrgContext,
        job: &Job,
        object_key: &crate::storage::ObjectKey,
        content_sha256: &str,
        byte_size: u64,
    ) -> Result<documents::MarkdownArtifactRecord, ConvertWorkerError> {
        let document_id = job
            .document_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        let version_id = job
            .version_id
            .ok_or(ConvertWorkerError::InvalidConvertPayload)?;
        let byte_size =
            i64::try_from(byte_size).map_err(|_| ConvertWorkerError::InvalidMarkdownLength)?;
        with_org_txn(&self.db_pool, ctx, {
            let ctx = ctx.clone();
            let object_key = object_key.as_str();
            let content_sha256 = content_sha256.to_string();
            move |txn| {
                Box::pin(async move {
                    documents::insert_markdown_artifact(
                        txn,
                        &ctx,
                        NewMarkdownArtifact {
                            id: Uuid::new_v4(),
                            document_id,
                            version_id,
                            object_key: &object_key,
                            content_sha256: &content_sha256,
                            byte_size,
                        },
                    )
                    .await
                })
            }
        })
        .await
        .map_err(ConvertWorkerError::Db)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertWorkerRun {
    NoJob,
    Completed { job_id: Uuid, markdown_bytes: usize },
    Failed { job_id: Uuid, terminal: bool },
    LeaseLost { job_id: Uuid },
}

#[derive(Debug)]
struct ConversionSuccess {
    markdown: Vec<u8>,
    artifact_key: crate::storage::ObjectKey,
    markdown_sha256: String,
    markdown_len: u64,
}

#[derive(Debug, Error)]
pub enum ConvertWorkerError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
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
    #[error("quarantine object is missing canonical format metadata")]
    MissingCanonicalFormat,
    #[error("quarantine object has unsupported canonical format metadata")]
    UnsupportedCanonicalFormat,
    #[error("markdown output is too large")]
    InvalidMarkdownLength,
}

impl ConvertWorkerError {
    fn is_retryable_job_failure(&self) -> bool {
        matches!(
            self,
            Self::Db(_)
                | Self::Storage(_)
                | Self::Sandbox(_)
                | Self::SandboxJoin
                | Self::SandboxTimedOut
                | Self::SandboxOutputTruncated
                | Self::ConverterFailed
                | Self::InvalidConvertPayload
                | Self::MissingCanonicalFormat
                | Self::UnsupportedCanonicalFormat
                | Self::InvalidMarkdownLength
        )
    }

    fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "convert database error",
            Self::Storage(_) => "convert storage error",
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
            Self::MissingCanonicalFormat => "convert canonical format missing",
            Self::UnsupportedCanonicalFormat => "convert canonical format unsupported",
            Self::InvalidMarkdownLength => "convert markdown too large",
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
        "wav" => Some("wav"),
        "mp3" => Some("mp3"),
        "ogg" => Some("ogg"),
        "flac" => Some("flac"),
        "m4a" => Some("m4a"),
        "zip" => Some("zip"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_extensions_are_server_derived() {
        assert_eq!(canonical_extension("jpeg"), Some("jpg"));
        assert_eq!(canonical_extension("../../etc/passwd"), None);
        assert_eq!(canonical_extension("txt"), Some("txt"));
    }
}
