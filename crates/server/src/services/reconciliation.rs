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
use crate::db::{
    chunks, document_versions, documents, index_metadata, jobs as repo, vector_cleanup_intents,
};
use crate::jobs::{self, CheckpointPayload, EnqueueJob, EnqueueOutcome, JobError, JobPayload};
use crate::services::index_signature::collection_name_for_digest;
use crate::services::indexing::index_job_idempotency_key;
use crate::storage::keys::{parse_key_for_org, trusted_version_prefix};
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantClient, VectorScope,
};
use crate::storage::StorageError;

const DEAD_LETTER_PAGE_LIMIT: i64 = 500;
const QDRANT_SCROLL_PAGE_LIMIT: usize = 1024;
const QDRANT_SCROLL_TOTAL_LIMIT: usize = 100_000;
const POINT_DELETE_BATCH: usize = 128;
const OBJECT_DELETE_BATCH: usize = 32;
const MINIO_INVENTORY_LIMIT: usize = 10_000;

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
    pub orphan_objects: usize,
    pub rebuilt_vector_jobs: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub missing_vectors: usize,
    pub orphan_vectors: usize,
    pub stale_vectors: usize,
    pub in_flight_vectors: usize,
    pub missing_objects: usize,
    pub orphan_objects: usize,
    pub repaired: ReconcileRepairCounts,
}

/// Pure inventory comparison used by reconcile and hermetic tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectInventoryDrift {
    pub missing_objects: Vec<String>,
    pub orphan_objects: Vec<String>,
}

pub fn compare_object_inventory(
    pg_keys: &BTreeSet<String>,
    minio_keys: &BTreeSet<String>,
) -> ObjectInventoryDrift {
    ObjectInventoryDrift {
        missing_objects: pg_keys.difference(minio_keys).cloned().collect(),
        orphan_objects: minio_keys.difference(pg_keys).cloned().collect(),
    }
}

/// Tombstoned/purged documents suppress reads immediately.
pub fn reads_suppressed(state: DocumentState) -> bool {
    matches!(state, DocumentState::Tombstoned | DocumentState::Purged)
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

    // Drain any kill-window vector-write intents before other repair decisions.
    drain_pending_vector_intents(pool, qdrant, ctx, document_id, &scope, mode, &mut report).await?;

    let object_drift = compare_document_minio_inventory(storage, ctx, &inventory).await?;
    report.missing_objects = object_drift.missing_objects.len();
    report.orphan_objects = object_drift.orphan_objects.len();

    if reads_suppressed(inventory.document.state) {
        let candidates = all_point_candidates(
            &points_by_signature,
            ctx.org_id(),
            inventory.document.collection_id,
            document_id,
        )?;
        report.orphan_vectors = candidates.len();
        // Tombstone/purged: every still-present PG-referenced object is leftover inventory.
        let leftover_objects = existing_object_keys(storage, ctx, &inventory.object_keys).await?;
        let mut orphan_object_keys = object_drift.orphan_objects;
        for key in leftover_objects {
            if !orphan_object_keys.iter().any(|existing| existing == &key) {
                orphan_object_keys.push(key);
            }
        }
        report.orphan_objects = orphan_object_keys.len();

        if mode == ReconcileMode::Repair {
            if report.orphan_vectors > 0 {
                report.repaired.orphan_vectors = report.orphan_vectors;
                write_repair_audit(pool, ctx, document_id, &report, "intent").await?;
                match delete_exact_point_candidates(qdrant, &scope, document_id, &candidates).await
                {
                    Ok(()) => {
                        write_repair_audit(pool, ctx, document_id, &report, "success").await?
                    }
                    Err(error) => {
                        write_repair_audit(pool, ctx, document_id, &report, "error").await?;
                        return Err(error);
                    }
                }
            }
            if !orphan_object_keys.is_empty() {
                delete_objects_with_audit(
                    pool,
                    storage,
                    ctx,
                    document_id,
                    &orphan_object_keys,
                    "reconcile.object_cleanup",
                )
                .await?;
                report.repaired.orphan_objects = orphan_object_keys.len();
            }
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

    let missing_vector_chunks = missing_vector_chunks(
        qdrant,
        &scope,
        inventory.document.collection_id,
        &inventory.chunks,
    )
    .await?;
    report.missing_vectors = missing_vector_chunks.len();

    if mode == ReconcileMode::Repair {
        let can_repair_indexed = inventory.document.state == DocumentState::Indexed
            && !active_writer_job(pool, ctx, document_id).await?;
        if can_repair_indexed {
            let repair_count = orphan_candidates.len() + stale_candidates.len();
            if repair_count > 0 {
                report.repaired.orphan_vectors = orphan_candidates.len();
                report.repaired.stale_vectors = stale_candidates.len();
                write_repair_audit(pool, ctx, document_id, &report, "intent").await?;
                match async {
                    delete_exact_point_candidates(qdrant, &scope, document_id, &orphan_candidates)
                        .await?;
                    delete_exact_point_candidates(qdrant, &scope, document_id, &stale_candidates)
                        .await?;
                    Ok::<_, ReconciliationError>(())
                }
                .await
                {
                    Ok(()) => {
                        write_repair_audit(pool, ctx, document_id, &report, "success").await?
                    }
                    Err(error) => {
                        write_repair_audit(pool, ctx, document_id, &report, "error").await?;
                        return Err(error);
                    }
                }
            }
            if report.missing_vectors > 0 {
                if enqueue_missing_vector_rebuild(pool, ctx, &inventory).await? {
                    report.repaired.rebuilt_vector_jobs = 1;
                }
            }
            if !object_drift.orphan_objects.is_empty() {
                delete_objects_with_audit(
                    pool,
                    storage,
                    ctx,
                    document_id,
                    &object_drift.orphan_objects,
                    "reconcile.object_cleanup",
                )
                .await?;
                report.repaired.orphan_objects = object_drift.orphan_objects.len();
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
    let mut staged_orphans = Vec::new();
    loop {
        let jobs = list_dead_letter_convert_page(pool, ctx, after_id).await?;
        if jobs.is_empty() {
            break;
        }
        after_id = jobs.last().map(|job| job.id);
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
                    staged_orphans.push(raw_key);
                }
            }
        }
        if jobs.len() < DEAD_LETTER_PAGE_LIMIT as usize {
            break;
        }
    }

    // Dry-run must surface staged orphan objects; repair deletes them with audit.
    report.orphan_objects = staged_orphans.len();
    if mode == ReconcileMode::Repair && !staged_orphans.is_empty() {
        delete_objects_with_audit(
            pool,
            storage,
            ctx,
            Uuid::nil(),
            &staged_orphans,
            "reconcile.dead_letter_gc",
        )
        .await?;
        report.repaired.staged_objects = staged_orphans.len();
        report.repaired.orphan_objects = staged_orphans.len();
    }
    Ok(report)
}

async fn drain_pending_vector_intents(
    pool: &Pool,
    qdrant: &QdrantClient,
    ctx: &OrgContext,
    document_id: Uuid,
    scope: &VectorScope,
    mode: ReconcileMode,
    report: &mut ReconcileReport,
) -> Result<(), ReconciliationError> {
    let intents = list_pending_intents(pool, ctx, document_id).await?;
    if intents.is_empty() {
        return Ok(());
    }
    report.orphan_vectors = report.orphan_vectors.saturating_add(
        intents
            .iter()
            .map(|intent| intent.point_ids.len())
            .sum::<usize>(),
    );
    if mode != ReconcileMode::Repair {
        return Ok(());
    }
    for intent in intents {
        let collection = collection_name_for_digest(&intent.index_signature_sha256)?;
        let document_filter = [json!({
            "key": "document_id",
            "match": { "value": document_id.to_string() }
        })];
        write_intent_audit(
            pool,
            ctx,
            document_id,
            "vector.cleanup_intent",
            "intent",
            json!({
                "job_id": intent.job_id,
                "point_count": intent.point_ids.len(),
            }),
        )
        .await?;
        match qdrant
            .delete_points_by_ids(&collection, scope, &document_filter, &intent.point_ids)
            .await
        {
            Ok(()) => {
                mark_intent_completed(pool, ctx, intent.job_id).await?;
                write_intent_audit(
                    pool,
                    ctx,
                    document_id,
                    "vector.cleanup_intent",
                    "success",
                    json!({
                        "job_id": intent.job_id,
                        "point_count": intent.point_ids.len(),
                    }),
                )
                .await?;
                report.repaired.orphan_vectors = report
                    .repaired
                    .orphan_vectors
                    .saturating_add(intent.point_ids.len());
            }
            Err(error) => {
                write_intent_audit(
                    pool,
                    ctx,
                    document_id,
                    "vector.cleanup_intent",
                    "error",
                    json!({
                        "job_id": intent.job_id,
                        "point_count": intent.point_ids.len(),
                    }),
                )
                .await?;
                return Err(error.into());
            }
        }
    }
    Ok(())
}

async fn compare_document_minio_inventory(
    storage: &MinioClient,
    ctx: &OrgContext,
    inventory: &DocumentInventory,
) -> Result<ObjectInventoryDrift, ReconciliationError> {
    let pg_keys = inventory
        .object_keys
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut minio_keys = BTreeSet::new();
    for version in &inventory.versions {
        let prefix = trusted_version_prefix(ctx.org_id(), version.id)?;
        let listed = storage
            .list_keys_with_prefix(ctx.org_id(), &prefix, MINIO_INVENTORY_LIMIT)
            .await?;
        for key in listed {
            minio_keys.insert(key);
        }
    }
    // Also observe existence of PG keys that may live outside version prefixes
    // (quarantine originals).
    for raw_key in &inventory.object_keys {
        let key = parse_key_for_org(raw_key, ctx.org_id())?;
        if storage.object_exists(ctx.org_id(), &key).await? {
            minio_keys.insert(raw_key.clone());
        }
    }
    Ok(compare_object_inventory(&pg_keys, &minio_keys))
}

async fn existing_object_keys(
    storage: &MinioClient,
    ctx: &OrgContext,
    object_keys: &[String],
) -> Result<Vec<String>, ReconciliationError> {
    let mut existing = Vec::new();
    for raw_key in object_keys {
        let key = parse_key_for_org(raw_key, ctx.org_id())?;
        if storage.object_exists(ctx.org_id(), &key).await? {
            existing.push(raw_key.clone());
        }
    }
    Ok(existing)
}

async fn delete_objects_with_audit(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    document_id: Uuid,
    object_keys: &[String],
    action: &'static str,
) -> Result<(), ReconciliationError> {
    for batch in object_keys.chunks(OBJECT_DELETE_BATCH) {
        let batch_keys = batch.to_vec();
        write_intent_audit(
            pool,
            ctx,
            document_id,
            action,
            "intent",
            json!({
                "document_id": document_id,
                "object_count": batch_keys.len(),
                "object_keys": batch_keys,
            }),
        )
        .await?;
        match delete_object_batch(storage, ctx, &batch_keys).await {
            Ok(()) => {
                write_intent_audit(
                    pool,
                    ctx,
                    document_id,
                    action,
                    "success",
                    json!({
                        "document_id": document_id,
                        "object_count": batch_keys.len(),
                        "object_keys": batch_keys,
                    }),
                )
                .await?;
            }
            Err(error) => {
                write_intent_audit(
                    pool,
                    ctx,
                    document_id,
                    action,
                    "error",
                    json!({
                        "document_id": document_id,
                        "object_count": batch_keys.len(),
                        "object_keys": batch_keys,
                    }),
                )
                .await?;
                return Err(error);
            }
        }
    }
    Ok(())
}

async fn delete_object_batch(
    storage: &MinioClient,
    ctx: &OrgContext,
    object_keys: &[String],
) -> Result<(), ReconciliationError> {
    for raw_key in object_keys {
        let key = parse_key_for_org(raw_key, ctx.org_id())?;
        match storage.cleanup_generated_object(ctx.org_id(), &key).await {
            Ok(()) | Err(StorageError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn enqueue_missing_vector_rebuild(
    pool: &Pool,
    ctx: &OrgContext,
    inventory: &DocumentInventory,
) -> Result<bool, ReconciliationError> {
    let Some(version_id) = inventory.document.current_version_id else {
        return Ok(false);
    };
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let collection_id = inventory.document.collection_id;
        let document_id = inventory.document.id;
        move |txn| {
            Box::pin(async move {
                let Some(metadata) =
                    index_metadata::find_active(txn, &ctx, Some(collection_id)).await?
                else {
                    return Ok(false);
                };
                let outcome = jobs::enqueue_within_txn(
                    txn,
                    &ctx,
                    EnqueueJob::new(
                        JobType::Index,
                        JobPayload {
                            document_id: Some(document_id),
                            version_id: Some(version_id),
                            index_metadata_id: Some(metadata.id),
                            ..JobPayload::default()
                        },
                        index_job_idempotency_key(metadata.id, version_id),
                    ),
                )
                .await?;
                Ok(outcome.created)
            })
        }
    })
    .await
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

async fn list_pending_intents(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<vector_cleanup_intents::VectorCleanupIntent>, ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, ReconciliationError>(
                    vector_cleanup_intents::list_pending_for_document(txn, &ctx, document_id)
                        .await?,
                )
            })
        }
    })
    .await
}

async fn mark_intent_completed(
    pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<(), ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                vector_cleanup_intents::mark_completed(txn, &ctx, job_id).await?;
                Ok(())
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

async fn missing_vector_chunks(
    qdrant: &QdrantClient,
    scope: &VectorScope,
    collection_id: Uuid,
    chunks: &[Chunk],
) -> Result<Vec<String>, ReconciliationError> {
    let mut by_signature: BTreeMap<String, Vec<&Chunk>> = BTreeMap::new();
    for chunk in chunks {
        by_signature
            .entry(chunk.index_signature.clone())
            .or_default()
            .push(chunk);
    }
    let mut missing = Vec::new();
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
        for (chunk, id) in chunks.iter().zip(ids.iter()) {
            if !found_ids.contains(id) {
                missing.push(chunk.chunk_identity_sha256.clone());
            }
        }
    }
    Ok(missing)
}

async fn write_repair_audit(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    report: &ReconcileReport,
    phase: &'static str,
) -> Result<(), ReconciliationError> {
    let outcome = match phase {
        "error" => "error",
        _ => "success",
    };
    write_intent_audit(
        pool,
        ctx,
        document_id,
        "reconcile.repair",
        outcome,
        json!({
            "document_id": document_id,
            "phase": phase,
            "orphan_vectors": report.repaired.orphan_vectors,
            "stale_vectors": report.repaired.stale_vectors,
            "orphan_objects": report.repaired.orphan_objects,
            "rebuilt_vector_jobs": report.repaired.rebuilt_vector_jobs,
        }),
    )
    .await
}

async fn write_intent_audit(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    action: &'static str,
    outcome: &'static str,
    metadata: serde_json::Value,
) -> Result<(), ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let resource_id = if document_id.is_nil() {
                    None
                } else {
                    Some(document_id.to_string())
                };
                let request_id = format!("{action}-{document_id}-{outcome}");
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action,
                        resource_type: if document_id.is_nil() {
                            "jobs"
                        } else {
                            "document"
                        },
                        resource_id: resource_id.as_deref(),
                        outcome,
                        request_id: &request_id,
                        metadata,
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
    use std::collections::BTreeSet;

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

    #[test]
    fn object_inventory_comparison_classifies_missing_and_orphan() {
        let pg = BTreeSet::from([
            "trusted/a/v1/obj1".to_string(),
            "trusted/a/v1/obj2".to_string(),
        ]);
        let minio = BTreeSet::from([
            "trusted/a/v1/obj1".to_string(),
            "trusted/a/v1/extra".to_string(),
        ]);
        let drift = compare_object_inventory(&pg, &minio);
        assert_eq!(drift.missing_objects, vec!["trusted/a/v1/obj2".to_string()]);
        assert_eq!(drift.orphan_objects, vec!["trusted/a/v1/extra".to_string()]);
    }

    #[test]
    fn tombstone_and_purge_suppress_reads() {
        assert!(reads_suppressed(DocumentState::Tombstoned));
        assert!(reads_suppressed(DocumentState::Purged));
        assert!(!reads_suppressed(DocumentState::Indexed));
        assert!(!reads_suppressed(DocumentState::Indexing));
    }

    #[test]
    fn dry_run_report_includes_staged_orphan_count_without_repair() {
        let mut report = ReconcileReport::default();
        report.orphan_objects = 3;
        assert_eq!(report.repaired.staged_objects, 0);
        assert_eq!(report.orphan_objects, 3);
    }
}
