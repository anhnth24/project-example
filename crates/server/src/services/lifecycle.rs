//! Durable Qdrant lifecycle-refresh jobs after version promotion.

use deadpool_postgres::Pool;
use serde_json::json;
use thiserror::Error;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{Job, JobStatus, JobType};
use crate::db::pool::with_org_txn_typed;
use crate::db::{chunks, documents, index_metadata, jobs as repo};
use crate::jobs::{self, EnqueueJob, EnqueueOutcome, JobError, JobPayload};
use crate::services::index_signature::collection_name_for_digest;
use crate::storage::error::StorageError;
use crate::storage::qdrant::{point_id_from_org_collection_and_chunk, QdrantClient, VectorScope};

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("lifecycle refresh payload is invalid")]
    InvalidPayload,
    #[error("claimed lifecycle job is missing a lease token")]
    MissingLease,
}

impl LifecycleError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "lifecycle database error",
            Self::Job(_) => "lifecycle job error",
            Self::Storage(_) => "lifecycle storage error",
            Self::InvalidPayload => "lifecycle payload invalid",
            Self::MissingLease => "lifecycle missing lease",
        }
    }

    pub fn is_retryable_job_failure(&self) -> bool {
        matches!(self, Self::Db(_) | Self::Storage(_) | Self::Job(_))
    }
}

pub fn lifecycle_refresh_idempotency_key(
    generation_id: Uuid,
    previous_version_id: Uuid,
    new_version_id: Uuid,
) -> String {
    format!("lifecycle_refresh:{generation_id}:{previous_version_id}:{new_version_id}")
}

/// Enqueues one idempotent filter-only lifecycle refresh per materialized
/// generation of the demoted version (same promotion transaction).
///
/// When the previous version has no durable chunks yet, returns an empty list
/// — never invents work from the active generation.
pub async fn enqueue_refresh_within_txn(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_id: Uuid,
    previous_version_id: Uuid,
    new_version_id: Uuid,
) -> Result<Vec<EnqueueOutcome>, JobError> {
    let generation_ids =
        chunks::list_generations_for_version(txn, ctx, previous_version_id).await?;
    let mut outcomes = Vec::with_capacity(generation_ids.len());
    for generation_id in generation_ids {
        let metadata = index_metadata::find_by_id(txn, ctx, generation_id)
            .await?
            .ok_or_else(|| {
                JobError::Database(DbError::Config(format!(
                    "lifecycle generation missing: {generation_id}"
                )))
            })?;
        let job_collection_id = metadata.collection_id.unwrap_or(collection_id);
        let payload = JobPayload {
            document_id: Some(document_id),
            version_id: Some(previous_version_id),
            related_version_id: Some(new_version_id),
            collection_id: Some(job_collection_id),
            index_metadata_id: Some(generation_id),
            ..JobPayload::default()
        };
        let outcome = jobs::enqueue_within_txn(
            txn,
            ctx,
            EnqueueJob::new(
                JobType::LifecycleRefresh,
                payload,
                lifecycle_refresh_idempotency_key(
                    generation_id,
                    previous_version_id,
                    new_version_id,
                ),
            ),
        )
        .await?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

#[derive(Debug, Clone, Copy)]
struct LifecycleSnapshot {
    org_id: Uuid,
    collection_id: Uuid,
    version_id: Uuid,
    is_current: bool,
    is_effective: bool,
}

/// Re-reads version state under the document lock, then applies a filter-only
/// Qdrant payload update. Retries converge: a later claim re-reads PG and
/// rewrites markers without depending on superseded index-job replay.
pub async fn refresh_version_lifecycle(
    db_pool: &Pool,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    job: &Job,
    lease_token: &str,
    attempts: i32,
) -> Result<(), LifecycleError> {
    if job.lease_owner.as_deref() != Some(lease_token) {
        return Err(LifecycleError::MissingLease);
    }
    let payload = jobs::decode_job_payload(job.payload_version, job.payload.clone())?;
    let document_id = payload.document_id.ok_or(LifecycleError::InvalidPayload)?;
    let version_id = payload.version_id.ok_or(LifecycleError::InvalidPayload)?;
    let index_metadata_id = payload
        .index_metadata_id
        .ok_or(LifecycleError::InvalidPayload)?;
    let collection_id = payload
        .collection_id
        .ok_or(LifecycleError::InvalidPayload)?;
    let job_id = job.id;
    let lease_token = lease_token.to_string();

    let (snapshot, identities, signature) = with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                let _document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let version =
                    crate::db::document_versions::find_by_id(txn, &ctx, document_id, version_id)
                        .await?
                        .ok_or(LifecycleError::InvalidPayload)?;
                let metadata = index_metadata::find_by_id(txn, &ctx, index_metadata_id)
                    .await?
                    .ok_or(LifecycleError::InvalidPayload)?;
                let identities = chunks::list_identities_by_version_and_generation(
                    txn,
                    &ctx,
                    version_id,
                    index_metadata_id,
                )
                .await?;
                let generation_collection =
                    metadata.collection_id.unwrap_or(document.collection_id);
                let snapshot = LifecycleSnapshot {
                    org_id: ctx.org_id(),
                    collection_id: generation_collection,
                    version_id,
                    is_current: document.current_version_id == Some(version_id),
                    is_effective: version.effective_to.is_none(),
                };
                if snapshot.collection_id != collection_id {
                    return Err(LifecycleError::InvalidPayload);
                }
                Ok((snapshot, identities, metadata.index_signature_sha256))
            })
        }
    })
    .await?;

    if !identities.is_empty() {
        let collection_name = collection_name_for_digest(&signature)?;
        let scope = VectorScope::new(snapshot.org_id, [snapshot.collection_id]);
        let point_ids = identities
            .iter()
            .map(|identity| {
                point_id_from_org_collection_and_chunk(
                    snapshot.org_id,
                    snapshot.collection_id,
                    identity,
                )
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let version_filter = [json!({
            "key": "version_id",
            "match": { "value": snapshot.version_id.to_string() }
        })];
        qdrant
            .set_payload_fields(
                &collection_name,
                &scope,
                &point_ids,
                &json!({
                    "is_current": snapshot.is_current,
                    "is_effective": snapshot.is_effective,
                }),
                &version_filter,
            )
            .await?;
    }

    // Re-read under the document lock; if promotion raced, fail retryably so
    // the durable job converges on the latest PG state without losing work.
    let still_matches = with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let lease_token = lease_token.clone();
        move |txn| {
            Box::pin(async move {
                verify_claimed_job(txn, &ctx, job_id, &lease_token, attempts).await?;
                let _document = documents::get_by_id_for_update(txn, &ctx, document_id).await?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let version =
                    crate::db::document_versions::find_by_id(txn, &ctx, document_id, version_id)
                        .await?
                        .ok_or(LifecycleError::InvalidPayload)?;
                let is_current = document.current_version_id == Some(version_id);
                let is_effective = version.effective_to.is_none();
                Ok::<bool, LifecycleError>(
                    is_current == snapshot.is_current && is_effective == snapshot.is_effective,
                )
            })
        }
    })
    .await?;
    if !still_matches {
        return Err(LifecycleError::Db(DbError::StaleState {
            expected: format!(
                "current={} effective={}",
                snapshot.is_current, snapshot.is_effective
            ),
            observed: "changed_under_refresh".into(),
        }));
    }

    Ok(())
}

async fn verify_claimed_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    lease_token: &str,
    attempts: i32,
) -> Result<Job, LifecycleError> {
    repo::get_by_id_for_update(txn, ctx, job_id)
        .await?
        .filter(|job| {
            job.status == JobStatus::Leased
                && job.lease_owner.as_deref() == Some(lease_token)
                && job.attempts == attempts
        })
        .ok_or(LifecycleError::Job(JobError::LeaseLost))
}
