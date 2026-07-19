//! PostgreSQL-authoritative drift detection and safe destructive repair.

use std::collections::{BTreeMap, BTreeSet};

use deadpool_postgres::Pool;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::models::{Chunk, Document, DocumentState, DocumentVersion, JobType};
use crate::db::pool::with_org_txn_typed;
use crate::db::{chunks, document_versions, documents, index_metadata, jobs as repo};
use crate::jobs::{self, CheckpointPayload, EnqueueJob, EnqueueOutcome, JobError, JobPayload};
use crate::services::index_signature::collection_name_for_digest;
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantClient, VectorScope,
};
use crate::storage::StorageError;

const DEAD_LETTER_PAGE_LIMIT: i64 = 500;
const QDRANT_SCROLL_PAGE_LIMIT: usize = 1024;
const QDRANT_SCROLL_TOTAL_LIMIT: usize = 100_000;
const POINT_DELETE_BATCH: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileMode {
    DryRun,
    Repair,
}

impl ReconcileMode {
    pub fn parse(value: &str) -> Result<Self, ReconciliationError> {
        match value {
            "dry-run" | "dry_run" | "dryrun" => Ok(Self::DryRun),
            "repair" => Ok(Self::Repair),
            _ => Err(ReconciliationError::InvalidMode),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileRepairCounts {
    pub orphan_vectors: usize,
    pub stale_vectors: usize,
    pub staged_objects: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub missing_vectors: usize,
    pub orphan_vectors: usize,
    pub stale_vectors: usize,
    pub in_flight_vectors: usize,
    pub missing_objects: usize,
    pub repaired: ReconcileRepairCounts,
}

struct DocumentInventory {
    document: Document,
    versions: Vec<DocumentVersion>,
    chunks: Vec<Chunk>,
    object_keys: Vec<String>,
    signatures: Vec<String>,
    active_writer_job: bool,
}

#[derive(Debug, Clone)]
struct VectorCandidate {
    signature: String,
    point_id: Uuid,
    payload: ChunkPointPayload,
}

pub async fn enqueue_reconcile(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    reason: &str,
) -> Result<EnqueueOutcome, JobError> {
    jobs::enqueue(
        pool,
        ctx,
        EnqueueJob::new(
            JobType::Reconcile,
            JobPayload {
                document_id: Some(document_id),
                ..JobPayload::default()
            },
            format!("reconcile:{document_id}:{reason}"),
        ),
    )
    .await
}

pub async fn reconcile_document(
    pool: &Pool,
    storage: &MinioClient,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    document_id: Uuid,
    mode: ReconcileMode,
) -> Result<ReconcileReport, ReconciliationError> {
    let inventory = load_inventory(pool, ctx, document_id).await?;
    let scope = VectorScope::new(ctx.org_id(), [inventory.document.collection_id]);
    let points_by_signature =
        scroll_document_points(qdrant, &scope, &inventory.signatures, document_id).await?;
    let mut report = ReconcileReport::default();

    if matches!(
        inventory.document.state,
        DocumentState::Tombstoned | DocumentState::Purged
    ) {
        let candidates = all_point_candidates(
            &points_by_signature,
            ctx.org_id(),
            inventory.document.collection_id,
            document_id,
        )?;
        report.orphan_vectors = candidates.len();
        if mode == ReconcileMode::Repair && report.orphan_vectors > 0 {
            report.repaired.orphan_vectors = report.orphan_vectors;
            // Audit the planned destructive repair BEFORE deleting so a crash
            // between the vector delete and the audit can never leave an
            // unaudited deletion (audit_log.outcome is limited to success/deny/error).
            write_repair_audit(pool, ctx, document_id, &report, "success").await?;
            delete_exact_point_candidates(qdrant, &scope, document_id, &candidates).await?;
        }
        return Ok(report);
    }

    let chunk_ids = inventory
        .chunks
        .iter()
        .map(|chunk| chunk.chunk_identity_sha256.clone())
        .collect::<BTreeSet<_>>();
    let versions = inventory
        .versions
        .iter()
        .map(|version| (version.id, version))
        .collect::<BTreeMap<_, _>>();
    let mut orphan_candidates = Vec::new();
    let mut stale_candidates = Vec::new();

    for (signature, points) in &points_by_signature {
        for (point_id, payload) in points {
            if !point_candidate_matches(
                payload,
                ctx.org_id(),
                inventory.document.collection_id,
                document_id,
            ) {
                continue;
            }
            if !chunk_ids.contains(&payload.chunk_id) {
                if inventory.document.state == DocumentState::Indexed
                    && !inventory.active_writer_job
                {
                    report.orphan_vectors += 1;
                    orphan_candidates.push(VectorCandidate {
                        signature: signature.clone(),
                        point_id: *point_id,
                        payload: payload.clone(),
                    });
                } else {
                    report.in_flight_vectors += 1;
                }
                continue;
            }
            let stale_current = payload.is_current
                && Some(payload.version_id) != inventory.document.current_version_id;
            let stale_effective = payload.is_effective
                && versions
                    .get(&payload.version_id)
                    .is_some_and(|version| version.effective_to.is_some());
            if stale_current || stale_effective {
                report.stale_vectors += 1;
                stale_candidates.push(VectorCandidate {
                    signature: signature.clone(),
                    point_id: *point_id,
                    payload: payload.clone(),
                });
            }
        }
    }

    report.missing_vectors = count_missing_vectors(
        qdrant,
        &scope,
        inventory.document.collection_id,
        &inventory.chunks,
    )
    .await?;
    report.missing_objects = count_missing_objects(storage, ctx, &inventory.object_keys).await?;

    if mode == ReconcileMode::Repair {
        let can_repair_indexed = inventory.document.state == DocumentState::Indexed
            && !active_writer_job(pool, ctx, document_id).await?;
        if can_repair_indexed {
            let repair_count = orphan_candidates.len() + stale_candidates.len();
            if repair_count > 0 {
                report.repaired.orphan_vectors = orphan_candidates.len();
                report.repaired.stale_vectors = stale_candidates.len();
                // Durable audit before the destructive delete (see note above).
                write_repair_audit(pool, ctx, document_id, &report, "success").await?;
                delete_exact_point_candidates(qdrant, &scope, document_id, &orphan_candidates)
                    .await?;
                delete_exact_point_candidates(qdrant, &scope, document_id, &stale_candidates)
                    .await?;
            }
        } else {
            report.in_flight_vectors += orphan_candidates.len();
        }
    }

    Ok(report)
}

pub async fn reconcile_dead_letter_jobs(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    mode: ReconcileMode,
) -> Result<ReconcileReport, ReconciliationError> {
    let mut report = ReconcileReport::default();
    let mut after_id = None;
    loop {
        let jobs = list_dead_letter_convert_page(pool, ctx, after_id).await?;
        if jobs.is_empty() {
            break;
        }
        after_id = jobs.last().map(|job| job.id);
        if mode == ReconcileMode::Repair {
            for job in &jobs {
                let Some(checkpoint) = job.checkpoint.clone() else {
                    continue;
                };
                let checkpoint = serde_json::from_value::<CheckpointPayload>(checkpoint)
                    .map_err(|_| ReconciliationError::InvalidCheckpoint)?;
                for raw_key in checkpoint.staged_object_keys {
                    if object_key_is_referenced(pool, ctx, &raw_key).await? {
                        continue;
                    }
                    let key = parse_key_for_org(&raw_key, ctx.org_id())?;
                    if storage.object_exists(ctx.org_id(), &key).await? {
                        storage.cleanup_generated_object(ctx.org_id(), &key).await?;
                        report.repaired.staged_objects += 1;
                    }
                }
            }
        }
        if jobs.len() < DEAD_LETTER_PAGE_LIMIT as usize {
            break;
        }
    }
    if mode == ReconcileMode::Repair && report.repaired.staged_objects > 0 {
        write_dead_letter_audit(pool, ctx, report.repaired.staged_objects).await?;
    }
    Ok(report)
}

async fn list_dead_letter_convert_page(
    pool: &Pool,
    ctx: &OrgContext,
    after_id: Option<Uuid>,
) -> Result<Vec<crate::db::models::Job>, ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, ReconciliationError>(
                    repo::list_dead_letter_of_type(
                        txn,
                        &ctx,
                        JobType::Convert,
                        after_id,
                        DEAD_LETTER_PAGE_LIMIT,
                    )
                    .await?,
                )
            })
        }
    })
    .await
}

async fn load_inventory(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<DocumentInventory, ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                let versions = document_versions::list_by_document(txn, &ctx, document_id).await?;
                let chunks = chunks::list_by_document(txn, &ctx, document_id).await?;
                let object_keys =
                    document_versions::list_object_keys_by_document(txn, &ctx, document_id).await?;
                let active_writer_job = repo::has_active_writer_job(txn, &ctx, document_id).await?;
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
                Ok(DocumentInventory {
                    document,
                    versions,
                    chunks,
                    object_keys,
                    signatures: signatures.into_iter().collect(),
                    active_writer_job,
                })
            })
        }
    })
    .await
}

async fn scroll_document_points(
    qdrant: &QdrantClient,
    scope: &VectorScope,
    signatures: &[String],
    document_id: Uuid,
) -> Result<BTreeMap<String, Vec<(Uuid, ChunkPointPayload)>>, ReconciliationError> {
    let filter = [json!({
        "key": "document_id",
        "match": { "value": document_id.to_string() }
    })];
    let mut out = BTreeMap::new();
    let mut total_points = 0usize;
    for digest in signatures {
        let collection = collection_name_for_digest(digest)?;
        let mut points = Vec::new();
        let mut offset = None;
        loop {
            let page = qdrant
                .scroll_points_page(
                    &collection,
                    scope,
                    &filter,
                    QDRANT_SCROLL_PAGE_LIMIT,
                    offset,
                )
                .await?;
            total_points = total_points.saturating_add(page.points.len());
            points.extend(page.points);
            if total_points > QDRANT_SCROLL_TOTAL_LIMIT {
                return Err(ReconciliationError::ScrollLimitExceeded);
            }
            let Some(next) = page.next_page_offset else {
                break;
            };
            offset = Some(next);
        }
        out.insert(digest.clone(), points);
    }
    Ok(out)
}

async fn delete_exact_point_candidates(
    qdrant: &QdrantClient,
    scope: &VectorScope,
    document_id: Uuid,
    candidates: &[VectorCandidate],
) -> Result<(), ReconciliationError> {
    let collection_id = single_collection_id(scope)?;
    let mut by_signature: BTreeMap<String, Vec<&VectorCandidate>> = BTreeMap::new();
    for candidate in candidates {
        if !point_candidate_matches(&candidate.payload, scope.org_id, collection_id, document_id) {
            continue;
        }
        by_signature
            .entry(candidate.signature.clone())
            .or_default()
            .push(candidate);
    }
    for (digest, candidates) in by_signature {
        let collection = collection_name_for_digest(&digest)?;
        let document_filter = [json!({
            "key": "document_id",
            "match": { "value": document_id.to_string() }
        })];
        for batch in candidates.chunks(POINT_DELETE_BATCH) {
            let ids = batch
                .iter()
                .map(|candidate| candidate.point_id)
                .collect::<Vec<_>>();
            qdrant
                .delete_points_by_ids(&collection, scope, &document_filter, &ids)
                .await?;
        }
    }
    Ok(())
}

fn single_collection_id(scope: &VectorScope) -> Result<Uuid, ReconciliationError> {
    if scope.collection_ids.len() != 1 {
        return Err(ReconciliationError::Storage(StorageError::MissingScope));
    }
    scope
        .collection_ids
        .iter()
        .next()
        .copied()
        .ok_or(ReconciliationError::Storage(StorageError::MissingScope))
}

fn all_point_candidates(
    points_by_signature: &BTreeMap<String, Vec<(Uuid, ChunkPointPayload)>>,
    org_id: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
) -> Result<Vec<VectorCandidate>, ReconciliationError> {
    let mut candidates = Vec::new();
    for (signature, points) in points_by_signature {
        for (point_id, payload) in points {
            if point_candidate_matches(payload, org_id, collection_id, document_id) {
                candidates.push(VectorCandidate {
                    signature: signature.clone(),
                    point_id: *point_id,
                    payload: payload.clone(),
                });
            }
        }
    }
    Ok(candidates)
}

fn point_candidate_matches(
    payload: &ChunkPointPayload,
    org_id: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
) -> bool {
    payload.org_id == org_id
        && payload.collection_id == collection_id
        && payload.document_id == document_id
}

async fn active_writer_job(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<bool, ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, ReconciliationError>(
                    repo::has_active_writer_job(txn, &ctx, document_id).await?,
                )
            })
        }
    })
    .await
}

async fn object_key_is_referenced(
    pool: &Pool,
    ctx: &OrgContext,
    object_key: &str,
) -> Result<bool, ReconciliationError> {
    let object_key = object_key.to_string();
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, ReconciliationError>(
                    document_versions::object_key_is_referenced(txn, &ctx, &object_key).await?,
                )
            })
        }
    })
    .await
}

async fn count_missing_vectors(
    qdrant: &QdrantClient,
    scope: &VectorScope,
    collection_id: Uuid,
    chunks: &[Chunk],
) -> Result<usize, ReconciliationError> {
    let mut by_signature: BTreeMap<String, Vec<&Chunk>> = BTreeMap::new();
    for chunk in chunks {
        by_signature
            .entry(chunk.index_signature.clone())
            .or_default()
            .push(chunk);
    }
    let mut missing = 0;
    for (digest, chunks) in by_signature {
        let collection = collection_name_for_digest(&digest)?;
        let ids = chunks
            .iter()
            .map(|chunk| {
                point_id_from_org_collection_and_chunk(
                    scope.org_id,
                    collection_id,
                    &chunk.chunk_identity_sha256,
                )
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let found = qdrant.get_points(&collection, scope, &ids).await?;
        let found_ids = found.into_iter().map(|(id, _)| id).collect::<BTreeSet<_>>();
        missing += ids.iter().filter(|id| !found_ids.contains(id)).count();
    }
    Ok(missing)
}

async fn count_missing_objects(
    storage: &MinioClient,
    ctx: &OrgContext,
    object_keys: &[String],
) -> Result<usize, ReconciliationError> {
    let mut missing = 0;
    for raw_key in object_keys {
        let key = parse_key_for_org(raw_key, ctx.org_id())?;
        if !storage.object_exists(ctx.org_id(), &key).await? {
            missing += 1;
        }
    }
    Ok(missing)
}

async fn write_repair_audit(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    report: &ReconcileReport,
    outcome: &'static str,
) -> Result<(), ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let repaired = report.repaired.clone();
        move |txn| {
            Box::pin(async move {
                let resource_id = document_id.to_string();
                let request_id = format!("reconcile-{document_id}");
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: "reconcile.repair",
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome,
                        request_id: &request_id,
                        metadata: json!({
                            "document_id": document_id,
                            "orphan_vectors": repaired.orphan_vectors,
                            "stale_vectors": repaired.stale_vectors,
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

async fn write_dead_letter_audit(
    pool: &Pool,
    ctx: &OrgContext,
    staged_objects: usize,
) -> Result<(), ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: "reconcile.repair",
                        resource_type: "jobs",
                        resource_id: None,
                        outcome: "success",
                        request_id: "reconcile-gc",
                        metadata: json!({
                            "dead_letter_staged_objects": staged_objects,
                            "quota_marker_cleanup": "todo_i02_expiry_sweep",
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

#[derive(Debug, Error)]
pub enum ReconciliationError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("reconcile mode is invalid")]
    InvalidMode,
    #[error("reconcile checkpoint is invalid")]
    InvalidCheckpoint,
    #[error("reconcile qdrant scroll limit exceeded")]
    ScrollLimitExceeded,
}

impl ReconciliationError {
    pub fn safe_job_error(&self) -> &'static str {
        match self {
            Self::Db(_) => "reconcile database error",
            Self::Job(_) => "reconcile job error",
            Self::Storage(_) => "reconcile storage error",
            Self::InvalidMode => "reconcile mode invalid",
            Self::InvalidCheckpoint => "reconcile checkpoint invalid",
            Self::ScrollLimitExceeded => "reconcile scroll limit exceeded",
        }
    }

    pub fn is_retryable_job_failure(&self) -> bool {
        !matches!(self, Self::Job(JobError::LeaseLost))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reconcile_modes() {
        assert_eq!(
            ReconcileMode::parse("dry-run").unwrap(),
            ReconcileMode::DryRun
        );
        assert_eq!(
            ReconcileMode::parse("repair").unwrap(),
            ReconcileMode::Repair
        );
        assert!(matches!(
            ReconcileMode::parse("delete"),
            Err(ReconciliationError::InvalidMode)
        ));
    }
}
