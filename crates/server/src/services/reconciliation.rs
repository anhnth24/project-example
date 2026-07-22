//! PostgreSQL-authoritative drift detection and safe destructive repair.

use std::collections::{BTreeMap, BTreeSet};

use deadpool_postgres::Pool;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::models::{
    Chunk, DerivedArtifact, Document, DocumentState, DocumentVersion, JobType,
};
use crate::db::pool::with_org_txn_typed;
use crate::db::{
    chunks, document_versions, documents, embedding_batches, index_metadata, jobs as repo,
    vector_cleanup_intents,
};
use crate::jobs::{self, CheckpointPayload, EnqueueJob, EnqueueOutcome, JobError, JobPayload};
use crate::services::index_signature::collection_name_for_digest;
use crate::services::indexing::{
    batch_covers_missing_ordinals, missing_chunks_fingerprint, repair_embedding_job_idempotency_key,
};
use crate::storage::keys::{parse_key_for_org, trusted_version_prefix, ObjectNamespace};
use crate::storage::minio::{MinioClient, ObservedObjectIdentity};
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

/// Object-type-specific identity expectations for MinIO HEAD validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedObjectKind {
    /// Quarantine originals bound for conversion (I06): org + source document/version + hash/size.
    QuarantineOriginal,
    /// Trusted artifacts after promotion: org + document/version + hash/size.
    TrustedArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedObjectIdentity {
    pub key: String,
    pub kind: ExpectedObjectKind,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub content_sha256: Option<String>,
    pub byte_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectObservation {
    Missing,
    Present { identity_ok: bool },
}

/// Validate observed MinIO identity against PG expectations.
pub fn object_identity_matches(
    expected: &ExpectedObjectIdentity,
    observed: &ObservedObjectIdentity,
    org_id: Uuid,
) -> bool {
    if observed.org_id != Some(org_id) {
        return false;
    }
    match expected.kind {
        ExpectedObjectKind::QuarantineOriginal | ExpectedObjectKind::TrustedArtifact => {
            // Quarantine originals must carry source document/version metadata
            // (same invariants as convert::verify_quarantine_metadata).
            if observed.document_id != expected.document_id {
                return false;
            }
            if observed.version_id != expected.version_id {
                return false;
            }
        }
    }
    if let Some(sha) = expected.content_sha256.as_deref() {
        if observed.content_sha256.as_deref() != Some(sha) {
            return false;
        }
    }
    if let Some(len) = expected.byte_size {
        if observed.content_length != Some(len) {
            return false;
        }
    }
    true
}

/// Build object-type-specific HEAD expectations from authoritative PG metadata.
pub fn expected_object_identity(
    key: String,
    org_id: Uuid,
    document_id: Uuid,
    version_id: Option<Uuid>,
    content_sha256: Option<String>,
    byte_size: Option<u64>,
) -> Result<ExpectedObjectIdentity, ReconciliationError> {
    let parsed = parse_key_for_org(&key, org_id)?;
    Ok(match parsed.namespace() {
        ObjectNamespace::Quarantine => ExpectedObjectIdentity {
            key,
            kind: ExpectedObjectKind::QuarantineOriginal,
            document_id: Some(document_id),
            version_id,
            content_sha256,
            byte_size,
        },
        ObjectNamespace::Trusted => ExpectedObjectIdentity {
            key,
            kind: ExpectedObjectKind::TrustedArtifact,
            document_id: Some(document_id),
            version_id,
            content_sha256,
            byte_size,
        },
    })
}

/// Prefer the upload/source version for a shared `original_object_key`.
///
/// Promoted Markdown versions reuse the quarantine original key but store the
/// Markdown hash/size in `content_sha256`/`byte_size` — those must not be used
/// as the original object identity.
pub fn authoritative_original_source<'a>(
    versions: &'a [DocumentVersion],
    original_key: &str,
) -> Option<&'a DocumentVersion> {
    let related = versions
        .iter()
        .filter(|version| version.original_object_key == original_key)
        .collect::<Vec<_>>();
    if related.is_empty() {
        return None;
    }
    for candidate in &related {
        if related
            .iter()
            .any(|child| child.parent_version_id == Some(candidate.id))
        {
            return Some(candidate);
        }
    }
    if let Some(source) = related
        .iter()
        .find(|version| version.markdown_object_key.is_none())
    {
        return Some(source);
    }
    related
        .into_iter()
        .min_by_key(|version| (version.version_number, version.id))
}

/// Deduplicate originals and attach per-version Markdown/artifact expectations.
pub fn build_expected_objects_for_document(
    org_id: Uuid,
    document_id: Uuid,
    versions: &[DocumentVersion],
    artifacts: &[DerivedArtifact],
) -> Result<Vec<ExpectedObjectIdentity>, ReconciliationError> {
    let mut expected_objects = Vec::new();
    let mut seen_originals = BTreeSet::new();
    for version in versions {
        if seen_originals.insert(version.original_object_key.clone()) {
            let source = authoritative_original_source(versions, &version.original_object_key)
                .unwrap_or(version);
            expected_objects.push(expected_object_identity(
                source.original_object_key.clone(),
                org_id,
                document_id,
                Some(source.id),
                Some(source.content_sha256.clone()),
                source.byte_size.and_then(|value| u64::try_from(value).ok()),
            )?);
        }
        if let Some(markdown_key) = version.markdown_object_key.clone() {
            let artifact = artifacts
                .iter()
                .find(|item| item.object_key == markdown_key);
            expected_objects.push(expected_object_identity(
                markdown_key,
                org_id,
                document_id,
                Some(version.id),
                artifact.map(|item| item.content_sha256.clone()),
                artifact
                    .and_then(|item| item.byte_size)
                    .and_then(|value| u64::try_from(value).ok()),
            )?);
        }
    }
    for artifact in artifacts {
        if expected_objects
            .iter()
            .any(|item| item.key == artifact.object_key)
        {
            continue;
        }
        expected_objects.push(expected_object_identity(
            artifact.object_key.clone(),
            org_id,
            document_id,
            Some(artifact.version_id),
            Some(artifact.content_sha256.clone()),
            artifact
                .byte_size
                .and_then(|value| u64::try_from(value).ok()),
        )?);
    }
    Ok(expected_objects)
}

/// After stale/orphan point deletes, stale chunk identities become missing and
/// must be included in repair requeue (pre-delete missing set is insufficient).
pub fn chunk_ids_needing_vector_repair(
    missing_chunk_ids: &[String],
    stale_chunk_ids: &[String],
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for chunk_id in missing_chunk_ids {
        ids.insert(chunk_id.clone());
    }
    for chunk_id in stale_chunk_ids {
        ids.insert(chunk_id.clone());
    }
    ids.into_iter().collect()
}

/// Classify MinIO drift. Purged/tombstoned docs do not treat intentional
/// absences as missing — only leftovers count as orphans.
pub fn classify_minio_drift(
    state: DocumentState,
    expected: &[ExpectedObjectIdentity],
    observations: &BTreeMap<String, ObjectObservation>,
    listed_keys: &BTreeSet<String>,
) -> ObjectInventoryDrift {
    let expected_keys = expected
        .iter()
        .map(|item| item.key.clone())
        .collect::<BTreeSet<_>>();
    let mut missing = Vec::new();
    let mut orphan = Vec::new();
    let suppressed = reads_suppressed(state);

    for item in expected {
        match observations.get(&item.key) {
            Some(ObjectObservation::Present { identity_ok: true }) if !suppressed => {}
            Some(ObjectObservation::Present { identity_ok: false }) if !suppressed => {
                missing.push(item.key.clone());
            }
            Some(ObjectObservation::Present { .. }) if suppressed => {
                // Intentionally deleted inventory that still exists is orphan leftover.
                orphan.push(item.key.clone());
            }
            Some(ObjectObservation::Missing) | None if suppressed => {
                // Absence after purge/tombstone cleanup is expected — not missing.
            }
            Some(ObjectObservation::Missing) | None => missing.push(item.key.clone()),
            _ => {}
        }
    }
    for key in listed_keys {
        if !expected_keys.contains(key) {
            orphan.push(key.clone());
        }
    }
    orphan.sort();
    orphan.dedup();
    missing.sort();
    ObjectInventoryDrift {
        missing_objects: missing,
        orphan_objects: orphan,
    }
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
    expected_objects: Vec<ExpectedObjectIdentity>,
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
    // Close/advance the durable readiness generation before durable enqueue so
    // readiness cannot race ahead of newly queued reconcile work.
    open_startup_reconciliation_generation(pool, &format!("reconcile enqueued:{reason}"))
        .await
        .map_err(|error| match error {
            ReconciliationError::Db(db) => JobError::Database(db),
            other => JobError::Database(DbError::Config(other.to_string())),
        })?;
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
        // Purged/tombstoned: absence is expected; only leftovers are actionable.
        report.missing_objects = 0;
        let orphan_object_keys = object_drift.orphan_objects.clone();
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
            // Stale deletes leave those chunk identities without vectors; union
            // them into the repair set (pre-delete missing set alone is wrong).
            let stale_chunk_ids = stale_candidates
                .iter()
                .map(|candidate| candidate.payload.chunk_id.clone())
                .collect::<Vec<_>>();
            let repair_chunk_ids =
                chunk_ids_needing_vector_repair(&missing_vector_chunks, &stale_chunk_ids);
            report.missing_vectors = repair_chunk_ids.len();
            if !repair_chunk_ids.is_empty() {
                let rebuilt =
                    requeue_missing_vector_batches(pool, ctx, &inventory, &repair_chunk_ids)
                        .await?;
                report.repaired.rebuilt_vector_jobs = rebuilt;
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
    let intents = list_open_intents(pool, ctx, document_id).await?;
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
    let scope = scope.clone();
    let qdrant = qdrant.clone();
    let drained = vector_cleanup_intents::with_vector_mutation_lock(pool, ctx, document_id, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let intents =
                    vector_cleanup_intents::list_open_for_document(txn, &ctx, document_id).await?;
                let mut cancelled_writers = false;
                let mut repaired = 0usize;
                for intent in intents {
                    let Some(plan) = vector_cleanup_intents::plan_intent_cleanup(intent.status)
                    else {
                        continue;
                    };
                    let collection = collection_name_for_digest(&intent.index_signature_sha256)?;
                    let document_filter = [json!({
                        "key": "document_id",
                        "match": { "value": document_id.to_string() }
                    })];
                    match plan {
                        vector_cleanup_intents::IntentCleanupPlan::CleanThenDelete => {
                            vector_cleanup_intents::cas_mark_cleaned_from(
                                txn,
                                &ctx,
                                intent.job_id,
                                vector_cleanup_intents::VectorCleanupIntentStatus::Pending,
                            )
                            .await
                            .map_err(map_reconcile_intent_error)?;
                            qdrant
                                .delete_points_by_ids(
                                    &collection,
                                    &scope,
                                    &document_filter,
                                    &intent.point_ids,
                                )
                                .await?;
                        }
                        vector_cleanup_intents::IntentCleanupPlan::CancelDeleteThenClean => {
                            if !cancelled_writers {
                                repo::cancel_active_writer_jobs(txn, &ctx, document_id).await?;
                                cancelled_writers = true;
                            }
                            qdrant
                                .delete_points_by_ids(
                                    &collection,
                                    &scope,
                                    &document_filter,
                                    &intent.point_ids,
                                )
                                .await?;
                            vector_cleanup_intents::cas_mark_cleaned_from(
                                txn,
                                &ctx,
                                intent.job_id,
                                vector_cleanup_intents::VectorCleanupIntentStatus::Writing,
                            )
                            .await
                            .map_err(map_reconcile_intent_error)?;
                        }
                    }
                    repaired = repaired.saturating_add(intent.point_ids.len());
                }
                Ok(repaired)
            })
        }
    })
    .await;
    match drained {
        Ok(repaired) => {
            report.repaired.orphan_vectors =
                report.repaired.orphan_vectors.saturating_add(repaired);
            if repaired > 0 {
                write_intent_audit(
                    pool,
                    ctx,
                    document_id,
                    "vector.cleanup_intent",
                    "success",
                    json!({
                        "document_id": document_id,
                        "point_count": repaired,
                    }),
                )
                .await?;
            }
            Ok(())
        }
        Err(error) => {
            write_intent_audit(
                pool,
                ctx,
                document_id,
                "vector.cleanup_intent",
                "error",
                json!({
                    "document_id": document_id,
                }),
            )
            .await?;
            Err(error)
        }
    }
}

fn map_reconcile_intent_error(
    error: vector_cleanup_intents::VectorCleanupIntentError,
) -> ReconciliationError {
    match error {
        vector_cleanup_intents::VectorCleanupIntentError::Db(db) => ReconciliationError::Db(db),
        _ => ReconciliationError::InvalidCheckpoint,
    }
}

async fn compare_document_minio_inventory(
    storage: &MinioClient,
    ctx: &OrgContext,
    inventory: &DocumentInventory,
) -> Result<ObjectInventoryDrift, ReconciliationError> {
    let mut listed_keys = BTreeSet::new();
    for version in &inventory.versions {
        let prefix = trusted_version_prefix(ctx.org_id(), version.id)?;
        let listed = storage
            .list_keys_with_prefix(ctx.org_id(), &prefix, MINIO_INVENTORY_LIMIT)
            .await?;
        for key in listed {
            listed_keys.insert(key);
        }
    }
    let mut observations = BTreeMap::new();
    for expected in &inventory.expected_objects {
        let key = parse_key_for_org(&expected.key, ctx.org_id())?;
        let observation = match storage.observe_object_identity(ctx.org_id(), &key).await? {
            None => ObjectObservation::Missing,
            Some(observed) => {
                listed_keys.insert(expected.key.clone());
                ObjectObservation::Present {
                    identity_ok: object_identity_matches(expected, &observed, ctx.org_id()),
                }
            }
        };
        observations.insert(expected.key.clone(), observation);
    }
    Ok(classify_minio_drift(
        inventory.document.state,
        &inventory.expected_objects,
        &observations,
        &listed_keys,
    ))
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

async fn requeue_missing_vector_batches(
    pool: &Pool,
    ctx: &OrgContext,
    inventory: &DocumentInventory,
    missing_chunk_ids: &[String],
) -> Result<usize, ReconciliationError> {
    let Some(version_id) = inventory.document.current_version_id else {
        return Ok(0);
    };
    let missing_set = missing_chunk_ids.iter().cloned().collect::<BTreeSet<_>>();
    let missing_ordinals = inventory
        .chunks
        .iter()
        .filter(|chunk| missing_set.contains(&chunk.chunk_identity_sha256))
        .map(|chunk| chunk.ordinal)
        .collect::<Vec<_>>();
    if missing_ordinals.is_empty() {
        return Ok(0);
    }
    let fingerprint = missing_chunks_fingerprint(missing_chunk_ids);
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let collection_id = inventory.document.collection_id;
        let document_id = inventory.document.id;
        move |txn| {
            Box::pin(async move {
                let Some(metadata) =
                    index_metadata::find_active(txn, &ctx, Some(collection_id)).await?
                else {
                    return Ok(0usize);
                };
                let batches = embedding_batches::list_by_document_version(
                    txn,
                    &ctx,
                    metadata.id,
                    document_id,
                    version_id,
                )
                .await?;
                let mut rebuilt = 0usize;
                for batch in batches {
                    if !batch_covers_missing_ordinals(
                        batch.start_ordinal,
                        batch.end_ordinal,
                        &missing_ordinals,
                    ) {
                        continue;
                    }
                    let repair_key = repair_embedding_job_idempotency_key(batch.id, &fingerprint);
                    let outcome = jobs::enqueue_within_txn(
                        txn,
                        &ctx,
                        EnqueueJob::new(
                            JobType::EmbeddingBatch,
                            JobPayload {
                                document_id: Some(document_id),
                                version_id: Some(version_id),
                                batch_id: Some(batch.id),
                                index_metadata_id: Some(metadata.id),
                                ..JobPayload::default()
                            },
                            repair_key,
                        ),
                    )
                    .await?;
                    if outcome.created {
                        embedding_batches::requeue_for_repair(txn, &ctx, batch.id, outcome.job.id)
                            .await?;
                        rebuilt += 1;
                    } else if outcome.job.status == crate::db::models::JobStatus::Pending
                        || outcome.job.status == crate::db::models::JobStatus::Leased
                    {
                        // Idempotent repair job already open; ensure batch points at it.
                        embedding_batches::requeue_for_repair(txn, &ctx, batch.id, outcome.job.id)
                            .await?;
                    }
                }
                Ok(rebuilt)
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
                let artifacts =
                    document_versions::list_artifacts_by_document(txn, &ctx, document_id).await?;
                let active_writer_job = repo::has_active_writer_job(txn, &ctx, document_id).await?;
                let expected_objects = build_expected_objects_for_document(
                    ctx.org_id(),
                    document_id,
                    &versions,
                    &artifacts,
                )?;
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
                    expected_objects,
                    signatures: signatures.into_iter().collect(),
                    active_writer_job,
                })
            })
        }
    })
    .await
}

async fn list_open_intents(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<vector_cleanup_intents::VectorCleanupIntent>, ReconciliationError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                Ok::<_, ReconciliationError>(
                    vector_cleanup_intents::list_open_for_document(txn, &ctx, document_id).await?,
                )
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

/// Durable readiness key written by the reconciliation service (P1B-R06).
pub const STARTUP_RECONCILIATION_KEY: &str = "startup_reconciliation";

/// Load the durable startup-reconciliation marker (missing → not ready).
pub async fn is_startup_reconciliation_ready(pool: &Pool) -> Result<bool, ReconciliationError> {
    let client = pool.get().await.map_err(DbError::from)?;
    let row = client
        .query_opt(
            "SELECT ready FROM runtime_readiness WHERE key = $1",
            &[&STARTUP_RECONCILIATION_KEY],
        )
        .await
        .map_err(DbError::from)?;
    Ok(row.map(|row| row.get::<_, bool>(0)).unwrap_or(false))
}

/// Atomically close readiness and advance the generation (enqueue / bootstrap).
pub async fn open_startup_reconciliation_generation(
    pool: &Pool,
    detail: &str,
) -> Result<i64, ReconciliationError> {
    let client = pool.get().await.map_err(DbError::from)?;
    let generation: i64 = client
        .query_one(
            "SELECT markhand_runtime_readiness_open($1, $2)",
            &[&STARTUP_RECONCILIATION_KEY, &detail],
        )
        .await
        .map_err(DbError::from)?
        .get(0);
    Ok(generation)
}

/// Atomically certify ready for the current generation when pending/leased is empty.
///
/// Generation 0 (never bootstrapped) cannot become ready. Errors from the
/// SECURITY DEFINER helpers propagate to the caller.
pub async fn try_certify_startup_reconciliation(
    pool: &Pool,
    detail: &str,
) -> Result<bool, ReconciliationError> {
    let client = pool.get().await.map_err(DbError::from)?;
    let ready: bool = client
        .query_one(
            "SELECT markhand_runtime_readiness_try_ready($1, $2)",
            &[&STARTUP_RECONCILIATION_KEY, &detail],
        )
        .await
        .map_err(DbError::from)?
        .get(0);
    Ok(ready)
}

/// Global pending+leased reconcile count via SECURITY DEFINER (not RLS-hidden).
pub async fn pending_reconcile_jobs(pool: &Pool) -> Result<i64, ReconciliationError> {
    let client = pool.get().await.map_err(DbError::from)?;
    let pending: i64 = client
        .query_one("SELECT markhand_pending_reconcile_jobs()", &[])
        .await
        .map_err(DbError::from)?
        .get(0);
    Ok(pending)
}

/// Startup bootstrap: open a generation, then certify only if the queue is idle.
///
/// An empty queue alone cannot certify a never-ran startup — opening the
/// generation records that bootstrap explicitly ran.
pub async fn bootstrap_startup_reconciliation(pool: &Pool) -> Result<bool, ReconciliationError> {
    let _generation = open_startup_reconciliation_generation(pool, "startup bootstrap").await?;
    try_certify_startup_reconciliation(pool, "startup bootstrap certified").await
}

/// After a reconcile job reaches a terminal success, re-evaluate the durable marker.
pub async fn certify_after_reconcile_success(pool: &Pool) -> Result<bool, ReconciliationError> {
    try_certify_startup_reconciliation(pool, "reconcile generation certified").await
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
    fn purged_docs_do_not_report_intentional_absences_as_missing() {
        let doc_id = Uuid::new_v4();
        let expected = vec![ExpectedObjectIdentity {
            key: "trusted/a/v1/obj1".into(),
            kind: ExpectedObjectKind::TrustedArtifact,
            document_id: Some(doc_id),
            version_id: Some(Uuid::new_v4()),
            content_sha256: Some("abc".into()),
            byte_size: Some(3),
        }];
        let mut observations = BTreeMap::new();
        observations.insert("trusted/a/v1/obj1".into(), ObjectObservation::Missing);
        let listed = BTreeSet::from(["trusted/a/v1/extra".to_string()]);
        let drift = classify_minio_drift(DocumentState::Purged, &expected, &observations, &listed);
        assert!(drift.missing_objects.is_empty());
        assert_eq!(drift.orphan_objects, vec!["trusted/a/v1/extra".to_string()]);
    }

    #[test]
    fn identity_mismatch_counts_as_missing_for_indexed_docs() {
        let org = Uuid::new_v4();
        let doc = Uuid::new_v4();
        let version = Uuid::new_v4();
        let expected = ExpectedObjectIdentity {
            key: "k".into(),
            kind: ExpectedObjectKind::TrustedArtifact,
            document_id: Some(doc),
            version_id: Some(version),
            content_sha256: Some("deadbeef".into()),
            byte_size: Some(4),
        };
        let observed = ObservedObjectIdentity {
            org_id: Some(org),
            document_id: Some(doc),
            version_id: Some(version),
            content_sha256: Some("cafebabe".into()),
            content_length: Some(4),
        };
        assert!(!object_identity_matches(&expected, &observed, org));
    }

    #[test]
    fn quarantine_original_matches_i06_source_document_version_and_hash() {
        let org = Uuid::new_v4();
        let doc = Uuid::new_v4();
        let source_version = Uuid::new_v4();
        let expected = ExpectedObjectIdentity {
            key: "quarantine/x/y".into(),
            kind: ExpectedObjectKind::QuarantineOriginal,
            document_id: Some(doc),
            version_id: Some(source_version),
            content_sha256: Some("abc".into()),
            byte_size: Some(3),
        };
        let observed = ObservedObjectIdentity {
            org_id: Some(org),
            document_id: Some(doc),
            version_id: Some(source_version),
            content_sha256: Some("abc".into()),
            content_length: Some(3),
        };
        assert!(object_identity_matches(&expected, &observed, org));
        let missing_ids = ObservedObjectIdentity {
            org_id: Some(org),
            document_id: None,
            version_id: None,
            content_sha256: Some("abc".into()),
            content_length: Some(3),
        };
        assert!(!object_identity_matches(&expected, &missing_ids, org));
    }

    #[test]
    fn authoritative_original_ignores_promoted_markdown_hash() {
        let doc = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let promoted_id = Uuid::new_v4();
        let original_key = "quarantine/org/obj".to_string();
        let source = DocumentVersion {
            id: source_id,
            org_id: Uuid::new_v4(),
            document_id: doc,
            version_number: 1,
            parent_version_id: None,
            publication_state: crate::db::models::PublicationState::Draft,
            is_current: false,
            content_sha256: "original-hash".into(),
            original_object_key: original_key.clone(),
            markdown_object_key: None,
            source_filename: None,
            source_content_type: None,
            byte_size: Some(10),
            effective_from: chrono::Utc::now(),
            effective_to: None,
            change_summary: None,
            created_by_user_id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
        };
        let promoted = DocumentVersion {
            id: promoted_id,
            version_number: 2,
            parent_version_id: Some(source_id),
            publication_state: crate::db::models::PublicationState::Published,
            is_current: true,
            content_sha256: "markdown-hash".into(),
            original_object_key: original_key.clone(),
            markdown_object_key: Some("trusted/org/v/md".into()),
            byte_size: Some(99),
            ..source.clone()
        };
        let versions = vec![source, promoted];
        let chosen = authoritative_original_source(&versions, &original_key).unwrap();
        assert_eq!(chosen.id, source_id);
        assert_eq!(chosen.content_sha256, "original-hash");
        assert_eq!(chosen.byte_size, Some(10));
    }

    #[test]
    fn stale_chunk_ids_are_unioned_into_vector_repair_set() {
        let missing = vec!["aa".into(), "bb".into()];
        let stale = vec!["bb".into(), "cc".into()];
        assert_eq!(
            chunk_ids_needing_vector_repair(&missing, &stale),
            vec!["aa".to_string(), "bb".to_string(), "cc".to_string()]
        );
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
        let report = ReconcileReport {
            orphan_objects: 3,
            ..Default::default()
        };
        assert_eq!(report.repaired.staged_objects, 0);
        assert_eq!(report.orphan_objects, 3);
    }
}
