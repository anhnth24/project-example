//! Upload saga: pre-persist envelope → reserve → put → fresh-auth register+finalize.
//!
//! Lock order: principal authz advisory → operation row → quota locks.
//! Reconcile CAS: `object_stored` → `reconciling` before external refund/delete;
//! commit CAS only `object_stored` → `completed`. Delete failure → `cleanup_pending`.

use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::require_permission;
use crate::db::documents::{self, NewDocument};
use crate::db::error::DbError;
use crate::db::models::{AuditOutcome, JobType};
use crate::db::pool::with_org_txn_typed;
use crate::db::upload_operations::{
    self, NewUploadOperation, UploadOperation, UploadOperationState,
};
use crate::jobs::{self, EnqueueJob, JobError, JobPayload};
use crate::services::audit::{self, AuditRecord};
use crate::services::authz_lock;
use crate::services::quota::{self, QuotaError, QuotaSnapshot};
use crate::storage::keys::quarantine_key;
use crate::storage::minio::MinioClient;
use crate::storage::{parse_key_for_org, ObjectKey};

use super::{Disposition, QuarantineIdentity, UploadError, UploadOutcome};

pub const PERMISSION_QUARANTINE_REVIEW: &str = "doc.quarantine.review";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredUpload {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub job_id: Option<Uuid>,
    pub collection_id: Uuid,
    pub created_job: bool,
    pub disposition: Disposition,
}

/// Stable upload response body fields (requestId is volatile and omitted).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StableUploadResponse {
    pub disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threat_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    pub object_key: String,
    pub object_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub canonical_format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
    pub collection_id: String,
    pub document_id: String,
    pub version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SagaSuccess {
    pub outcome: UploadOutcome,
    pub registered: RegisteredUpload,
    pub quota_snapshot: QuotaSnapshot,
    pub replayed: bool,
    pub stable: StableUploadResponse,
}

#[derive(Debug, thiserror::Error)]
pub enum SagaError {
    #[error("upload error")]
    Upload(#[from] UploadError),
    #[error("quota error")]
    Quota(#[from] QuotaError),
    #[error("permission denied")]
    PermissionDenied,
    #[error("not found")]
    NotFound,
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("idempotency in progress")]
    IdempotencyInProgress,
    #[error("database error")]
    Database(#[from] DbError),
    #[error("job error")]
    Job(#[from] JobError),
    #[error("internal error")]
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SagaFaultBarrier {
    AfterReserve,
    AfterObjectPut,
    BeforeCommit,
    RegistrationFail,
    AfterClaimReconcile,
}

#[cfg(any(test, feature = "test-hooks"))]
static ARMED_FAULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(any(test, feature = "test-hooks"))]
static PAUSE_BEFORE_COMMIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(any(test, feature = "test-hooks"))]
static PAUSE_AFTER_RECONCILE_CLAIM: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(any(test, feature = "test-hooks"))]
static PAUSE_BEFORE_APPROVE_COMMIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Process-wide guard: fault/pause atomics are shared; live hook tests must serialize.
#[cfg(any(test, feature = "test-hooks"))]
static HOOK_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(any(test, feature = "test-hooks"))]
pub struct HookTestGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

#[cfg(any(test, feature = "test-hooks"))]
pub fn acquire_hook_test_guard() -> HookTestGuard {
    let guard = HOOK_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ARMED_FAULT.store(0, std::sync::atomic::Ordering::SeqCst);
    PAUSE_BEFORE_COMMIT.store(false, std::sync::atomic::Ordering::SeqCst);
    PAUSE_AFTER_RECONCILE_CLAIM.store(false, std::sync::atomic::Ordering::SeqCst);
    PAUSE_BEFORE_APPROVE_COMMIT.store(false, std::sync::atomic::Ordering::SeqCst);
    HookTestGuard(guard)
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_saga_fault(barrier: SagaFaultBarrier) {
    let code = match barrier {
        SagaFaultBarrier::AfterReserve => 1,
        SagaFaultBarrier::AfterObjectPut => 2,
        SagaFaultBarrier::BeforeCommit => 3,
        SagaFaultBarrier::RegistrationFail => 4,
        SagaFaultBarrier::AfterClaimReconcile => 5,
    };
    ARMED_FAULT.store(code, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_pause_before_commit() {
    PAUSE_BEFORE_COMMIT.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn resume_before_commit() {
    PAUSE_BEFORE_COMMIT.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_pause_after_reconcile_claim() {
    PAUSE_AFTER_RECONCILE_CLAIM.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn resume_after_reconcile_claim() {
    PAUSE_AFTER_RECONCILE_CLAIM.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_pause_before_approve_commit() {
    PAUSE_BEFORE_APPROVE_COMMIT.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn resume_before_approve_commit() {
    PAUSE_BEFORE_APPROVE_COMMIT.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-hooks"))]
async fn wait_if_paused_before_commit() {
    while PAUSE_BEFORE_COMMIT.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
async fn wait_if_paused_before_commit() {}

#[cfg(any(test, feature = "test-hooks"))]
async fn wait_if_paused_after_reconcile_claim() {
    while PAUSE_AFTER_RECONCILE_CLAIM.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
async fn wait_if_paused_after_reconcile_claim() {}

#[cfg(any(test, feature = "test-hooks"))]
async fn wait_if_paused_before_approve_commit() {
    while PAUSE_BEFORE_APPROVE_COMMIT.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
async fn wait_if_paused_before_approve_commit() {}

#[cfg(any(test, feature = "test-hooks"))]
fn trip_fault(barrier: SagaFaultBarrier) -> bool {
    let expected = match barrier {
        SagaFaultBarrier::AfterReserve => 1,
        SagaFaultBarrier::AfterObjectPut => 2,
        SagaFaultBarrier::BeforeCommit => 3,
        SagaFaultBarrier::RegistrationFail => 4,
        SagaFaultBarrier::AfterClaimReconcile => 5,
    };
    ARMED_FAULT
        .compare_exchange(
            expected,
            0,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn trip_fault(_barrier: SagaFaultBarrier) -> bool {
    false
}

#[derive(Debug)]
pub struct SagaInput {
    pub collection_id: Uuid,
    pub idempotency_key: String,
    pub reservation_key: String,
    pub streamed: super::StreamedUpload,
    pub declared_filename: Option<String>,
    pub declared_content_type: Option<String>,
    pub identity: QuarantineIdentity,
}

/// Canonical envelope digest: content SHA + collection + normalized metadata.
pub fn envelope_sha256(
    content_sha256: &str,
    collection_id: Uuid,
    filename: Option<&str>,
    content_type: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content_sha256.as_bytes());
    hasher.update(b"|");
    hasher.update(collection_id.as_bytes());
    hasher.update(b"|");
    let name = filename.map(normalize_filename).unwrap_or_default();
    hasher.update(name.as_bytes());
    hasher.update(b"|");
    let ctype = content_type
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    hasher.update(ctype.as_bytes());
    hex::encode(hasher.finalize())
}

fn normalize_filename(name: &str) -> String {
    name.chars()
        .filter(|ch| !ch.is_control())
        .take(255)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Full upload saga after multipart stream.
pub async fn run_upload_saga(
    pool: &Pool,
    storage: &MinioClient,
    provisional_ctx: &OrgContext,
    limits: &super::LimitsConfig,
    input: SagaInput,
) -> Result<SagaSuccess, SagaError> {
    let content_sha = input.streamed.sha256_hex.clone();
    let size_bytes = input.streamed.size_bytes;
    let envelope = envelope_sha256(
        &content_sha,
        input.collection_id,
        input.declared_filename.as_deref(),
        input.declared_content_type.as_deref(),
    );
    let expected_key = quarantine_key(
        provisional_ctx.org_id(),
        input.identity.object_id,
        input.declared_filename.as_deref(),
    )
    .map_err(|_| SagaError::Internal)?;
    let expected_key_str = expected_key.as_str();

    let begin = begin_or_replay(
        pool,
        provisional_ctx,
        &input,
        &content_sha,
        &envelope,
        &expected_key_str,
        size_bytes,
    )
    .await?;
    if let Some(replay) = begin {
        return Ok(replay);
    }

    // Bind pre-persisted IDs/keys (retries must not remint object/document IDs).
    let binding = load_operation_binding(pool, provisional_ctx, &input.idempotency_key).await?;
    let op_id = binding.op_id;
    let collection_id = binding.collection_id;
    let identity = QuarantineIdentity {
        object_id: binding.object_id,
        collection_id: binding.collection_id,
        document_id: binding.document_id,
        version_id: binding.version_id,
    };

    // Persist reserved attempt (new reservation key per attempt) before put.
    let reservation_key =
        transition_reserved(pool, provisional_ctx, op_id, &input.reservation_key).await?;

    let reservation =
        super::quota_reserve_hook(pool, provisional_ctx, &reservation_key, size_bytes).await;
    if let Err(error) = reservation {
        let _ = mark_op_failed(pool, provisional_ctx, op_id, "quota_reserve_failed").await;
        return Err(SagaError::Quota(error));
    }
    let reservation = reservation.expect("checked");
    if !reservation.storage.created || !reservation.document.created {
        let _ = compensate_op(
            pool,
            storage,
            provisional_ctx,
            op_id,
            &reservation_key,
            None,
            "reservation_conflict",
        )
        .await;
        return Err(SagaError::Quota(QuotaError::ReservationConflict));
    }
    if trip_fault(SagaFaultBarrier::AfterReserve) {
        compensate_op(
            pool,
            storage,
            provisional_ctx,
            op_id,
            &reservation_key,
            None,
            "fault_after_reserve",
        )
        .await?;
        return Err(SagaError::Internal);
    }

    transition_putting(pool, provisional_ctx, op_id).await?;

    let SagaInput {
        collection_id: _,
        idempotency_key: _,
        reservation_key: _,
        streamed,
        declared_filename,
        declared_content_type,
        identity: _,
    } = input;

    let outcome = match super::validate_and_quarantine_with_identity(
        provisional_ctx,
        storage,
        limits,
        streamed,
        declared_filename.as_deref(),
        declared_content_type.as_deref(),
        Some(identity),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let _ = compensate_op(
                pool,
                storage,
                provisional_ctx,
                op_id,
                &reservation_key,
                None,
                "upload_validation_failed",
            )
            .await;
            return Err(SagaError::Upload(error));
        }
    };

    mark_object_stored_intent(pool, provisional_ctx, op_id, &outcome).await?;

    if trip_fault(SagaFaultBarrier::AfterObjectPut) {
        compensate_op(
            pool,
            storage,
            provisional_ctx,
            op_id,
            &reservation_key,
            Some(&outcome.object_key),
            "fault_after_object_put",
        )
        .await?;
        return Err(SagaError::Internal);
    }

    if trip_fault(SagaFaultBarrier::BeforeCommit) {
        compensate_op(
            pool,
            storage,
            provisional_ctx,
            op_id,
            &reservation_key,
            Some(&outcome.object_key),
            "fault_before_commit",
        )
        .await?;
        return Err(SagaError::Internal);
    }

    wait_if_paused_before_commit().await;
    let _ = (declared_filename, declared_content_type);

    match commit_registration_and_finalize(
        pool,
        provisional_ctx,
        collection_id,
        &reservation_key,
        op_id,
        identity,
        &outcome,
    )
    .await
    {
        Ok(success) => Ok(success),
        Err(error) => {
            let _ = compensate_op(
                pool,
                storage,
                provisional_ctx,
                op_id,
                &reservation_key,
                Some(&outcome.object_key),
                error_code_for(&error),
            )
            .await;
            Err(error)
        }
    }
}

struct OperationBinding {
    op_id: Uuid,
    object_id: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
}

async fn load_operation_binding(
    pool: &Pool,
    ctx: &OrgContext,
    idempotency_key: &str,
) -> Result<OperationBinding, SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let idempotency_key = idempotency_key.to_string();
        move |txn| {
            Box::pin(async move {
                let op = upload_operations::get_for_update(txn, &ctx, &idempotency_key).await?;
                Ok(OperationBinding {
                    op_id: op.id,
                    object_id: op.object_id,
                    collection_id: op.collection_id,
                    document_id: op.document_id,
                    version_id: op.version_id,
                })
            })
        }
    })
    .await
}

async fn transition_reserved(
    pool: &Pool,
    ctx: &OrgContext,
    op_id: Uuid,
    base_reservation_key: &str,
) -> Result<String, SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let base_reservation_key = base_reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                let op = upload_operations::get_by_id_for_update(txn, &ctx, op_id).await?;
                let attempt = match op.state {
                    UploadOperationState::Started => 1,
                    UploadOperationState::Failed | UploadOperationState::Refunded => {
                        op.attempt.saturating_add(1)
                    }
                    UploadOperationState::Reserved => return Ok(op.reservation_key),
                    _ => {
                        return Err(SagaError::IdempotencyInProgress);
                    }
                };
                let reservation_key = if attempt <= 1 {
                    base_reservation_key
                } else {
                    format!("{base_reservation_key}:a{attempt}")
                };
                upload_operations::mark_reserved(txn, &ctx, op_id, &reservation_key, attempt)
                    .await?;
                Ok(reservation_key)
            })
        }
    })
    .await
}

async fn transition_putting(pool: &Pool, ctx: &OrgContext, op_id: Uuid) -> Result<(), SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                upload_operations::mark_putting(txn, &ctx, op_id).await?;
                Ok(())
            })
        }
    })
    .await
}

async fn mark_op_failed(
    pool: &Pool,
    ctx: &OrgContext,
    op_id: Uuid,
    error_code: &str,
) -> Result<(), SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let error_code = error_code.to_string();
        move |txn| {
            Box::pin(async move {
                let _ = upload_operations::mark_failed(txn, &ctx, op_id, &error_code).await;
                Ok(())
            })
        }
    })
    .await
}

async fn begin_or_replay(
    pool: &Pool,
    ctx: &OrgContext,
    input: &SagaInput,
    content_sha: &str,
    envelope: &str,
    expected_object_key: &str,
    size_bytes: u64,
) -> Result<Option<SagaSuccess>, SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let idempotency_key = input.idempotency_key.clone();
        let reservation_key = input.reservation_key.clone();
        let collection_id = input.collection_id;
        let content_sha = content_sha.to_string();
        let envelope = envelope.to_string();
        let expected_object_key = expected_object_key.to_string();
        let identity = input.identity;
        let filename = input.declared_filename.clone();
        move |txn| {
            Box::pin(async move {
                authz_lock::lock_principal_authz(txn, ctx.org_id(), ctx.user_id()).await?;

                let inserted = upload_operations::insert_started(
                    txn,
                    &ctx,
                    NewUploadOperation {
                        id: Uuid::new_v4(),
                        idempotency_key: &idempotency_key,
                        envelope_sha256: &envelope,
                        content_sha256: &content_sha,
                        reservation_key: &reservation_key,
                        expected_object_key: &expected_object_key,
                        object_id: identity.object_id,
                        collection_id,
                        document_id: identity.document_id,
                        version_id: identity.version_id,
                        size_bytes: size_bytes as i64,
                        original_filename: filename.as_deref(),
                    },
                )
                .await?;
                let we_own = inserted.is_some();
                let op = match inserted {
                    Some(op) => op,
                    None => upload_operations::get_for_update(txn, &ctx, &idempotency_key).await?,
                };

                // Envelope + collection must match for any reuse.
                if op.envelope_sha256 != envelope || op.collection_id != collection_id {
                    return Err(SagaError::IdempotencyConflict);
                }
                if op.content_sha256 != content_sha {
                    return Err(SagaError::IdempotencyConflict);
                }

                match op.state {
                    UploadOperationState::Completed => {
                        // Fresh-auth original operation collection before returning.
                        let fresh = reload_principal_locked(txn, &ctx).await?;
                        require_permission(&fresh, "doc.upload")
                            .map_err(|_| SagaError::PermissionDenied)?;
                        ensure_collection_writable(txn, &fresh, op.collection_id).await?;
                        Ok(Some(replay_from_operation(&op)?))
                    }
                    UploadOperationState::Started
                    | UploadOperationState::Reserved
                    | UploadOperationState::Putting
                    | UploadOperationState::ObjectStored
                    | UploadOperationState::Reconciling
                    | UploadOperationState::CleanupPending => {
                        if !we_own {
                            return Err(SagaError::IdempotencyInProgress);
                        }
                        Ok(None)
                    }
                    UploadOperationState::Failed | UploadOperationState::Refunded => {
                        // Retry with new reservation attempt (caller supplies new key).
                        Ok(None)
                    }
                }
            })
        }
    })
    .await
}

fn replay_from_operation(op: &UploadOperation) -> Result<SagaSuccess, SagaError> {
    let disposition = match op.disposition.as_deref() {
        Some("accepted") => Disposition::Accepted,
        Some("quarantined") => Disposition::Quarantined,
        _ => return Err(SagaError::Internal),
    };
    let object_key_raw = op
        .object_key
        .clone()
        .or_else(|| op.expected_object_key.clone())
        .ok_or(SagaError::Internal)?;
    let key = parse_key_for_org(&object_key_raw, op.org_id).map_err(|_| SagaError::Internal)?;
    let format = op
        .canonical_format
        .as_deref()
        .and_then(|s| super::CanonicalFormat::parse(s).ok())
        .unwrap_or(super::CanonicalFormat::PlainText);
    let size_bytes = op.size_bytes.unwrap_or(0).max(0) as u64;
    let threat_class = op.threat_class.as_deref().and_then(parse_threat_class);
    let reason_code = op.reason_code.as_deref().and_then(parse_reason_code);
    let outcome = UploadOutcome {
        disposition,
        threat_class,
        reason_code,
        object_key: key,
        object_id: op.object_id,
        sha256_hex: op.content_sha256.clone(),
        size_bytes,
        canonical_format: format,
        original_filename: op.original_filename.clone(),
    };
    let registered = RegisteredUpload {
        document_id: op.document_id,
        version_id: op.version_id,
        job_id: op.job_id,
        collection_id: op.collection_id,
        created_job: false,
        disposition,
    };
    let stable = stable_from_parts(&outcome, &registered);
    Ok(SagaSuccess {
        outcome,
        registered,
        quota_snapshot: QuotaSnapshot {
            resource_kind: crate::db::models::ResourceKind::StorageBytes,
            limit: 0,
            committed: 0,
            active_reserved: 0,
            remaining: 0,
        },
        replayed: true,
        stable,
    })
}

pub fn stable_from_parts(
    outcome: &UploadOutcome,
    registered: &RegisteredUpload,
) -> StableUploadResponse {
    StableUploadResponse {
        disposition: outcome.disposition.as_str().to_string(),
        threat_class: outcome.threat_class.map(|t| t.as_str().to_string()),
        reason_code: outcome.reason_code.map(|r| r.as_str().to_string()),
        object_key: outcome.object_key.as_str(),
        object_id: outcome.object_id.to_string(),
        sha256: outcome.sha256_hex.clone(),
        size_bytes: outcome.size_bytes,
        canonical_format: outcome.canonical_format.as_str().to_string(),
        original_filename: outcome.original_filename.clone(),
        collection_id: registered.collection_id.to_string(),
        document_id: registered.document_id.to_string(),
        version_id: registered.version_id.to_string(),
        job_id: registered.job_id.map(|id| id.to_string()),
    }
}

async fn mark_object_stored_intent(
    pool: &Pool,
    ctx: &OrgContext,
    op_id: Uuid,
    outcome: &UploadOutcome,
) -> Result<(), SagaError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let object_key = outcome.object_key.as_str().to_string();
        let disposition = outcome.disposition.as_str().to_string();
        let format = outcome.canonical_format.as_str().to_string();
        let threat = outcome.threat_class.map(|t| t.as_str().to_string());
        let reason = outcome.reason_code.map(|r| r.as_str().to_string());
        let filename = outcome.original_filename.clone();
        let size = outcome.size_bytes as i64;
        move |txn| {
            Box::pin(async move {
                // Operation lock before any further work.
                let _op = upload_operations::get_by_id_for_update(txn, &ctx, op_id).await?;
                upload_operations::mark_object_stored(
                    txn,
                    &ctx,
                    op_id,
                    &object_key,
                    &disposition,
                    &format,
                    threat.as_deref(),
                    reason.as_deref(),
                    filename.as_deref(),
                    size,
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
}

async fn commit_registration_and_finalize(
    pool: &Pool,
    provisional_ctx: &OrgContext,
    collection_id: Uuid,
    reservation_key: &str,
    op_id: Uuid,
    identity: QuarantineIdentity,
    outcome: &UploadOutcome,
) -> Result<SagaSuccess, SagaError> {
    with_org_txn_typed(pool, provisional_ctx, {
        let provisional_ctx = provisional_ctx.clone();
        let reservation_key = reservation_key.to_string();
        let document_id = identity.document_id;
        let version_id = identity.version_id;
        let outcome = outcome.clone();
        move |txn| {
            Box::pin(async move {
                if trip_fault(SagaFaultBarrier::RegistrationFail) {
                    return Err(SagaError::Internal);
                }

                // Lock order: principal authz → operation → quota.
                authz_lock::lock_principal_authz(
                    txn,
                    provisional_ctx.org_id(),
                    provisional_ctx.user_id(),
                )
                .await?;
                let op =
                    upload_operations::get_by_id_for_update(txn, &provisional_ctx, op_id).await?;
                if op.state == UploadOperationState::Completed {
                    let fresh = reload_principal_locked(txn, &provisional_ctx).await?;
                    ensure_collection_writable(txn, &fresh, op.collection_id).await?;
                    return replay_from_operation(&op);
                }
                if op.state == UploadOperationState::Reconciling
                    || op.state == UploadOperationState::CleanupPending
                    || op.state == UploadOperationState::Refunded
                {
                    return Err(SagaError::Internal);
                }
                if op.state != UploadOperationState::ObjectStored {
                    return Err(SagaError::Internal);
                }

                let fresh = reload_principal_locked(txn, &provisional_ctx).await?;
                require_permission(&fresh, "doc.upload")
                    .map_err(|_| SagaError::PermissionDenied)?;
                ensure_collection_writable(txn, &fresh, collection_id).await?;

                let registered = register_rows(
                    txn,
                    &fresh,
                    collection_id,
                    &outcome,
                    document_id,
                    version_id,
                )
                .await?;

                // Quota after operation lock (fixed order).
                let settlement =
                    quota::finalize_upload_in_txn(txn, &fresh, &reservation_key).await?;

                // CAS only object_stored → completed.
                upload_operations::mark_completed(txn, &fresh, op_id, registered.job_id).await?;

                let stable = stable_from_parts(&outcome, &registered);
                Ok(SagaSuccess {
                    outcome,
                    registered,
                    quota_snapshot: settlement.storage_quota,
                    replayed: false,
                    stable,
                })
            })
        }
    })
    .await
}

async fn reload_principal_locked(
    txn: &tokio_postgres::Transaction<'_>,
    provisional: &OrgContext,
) -> Result<OrgContext, SagaError> {
    let org_id = provisional.org_id();
    let user_id = provisional.user_id();

    // Membership/user rows are read under the principal advisory lock (not
    // FOR UPDATE) so suspend/disable writers are not blocked while a saga
    // pauses; re-check after pause observes the fresh disabled_at.
    let membership = txn
        .query_opt(
            "SELECT role FROM org_memberships
             WHERE org_id = $1 AND user_id = $2",
            &[&org_id, &user_id],
        )
        .await
        .map_err(DbError::from)?;
    if membership.is_none() {
        return Err(SagaError::PermissionDenied);
    }

    let user_row = txn
        .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
        .await
        .map_err(DbError::from)?
        .ok_or(SagaError::PermissionDenied)?;
    let disabled_at: Option<chrono::DateTime<chrono::Utc>> = user_row.get(0);
    if disabled_at.is_some() {
        return Err(SagaError::PermissionDenied);
    }

    let permission_rows = txn
        .query(
            "SELECT p.code
             FROM org_memberships m
             JOIN roles r
               ON r.org_id = m.org_id AND r.code = m.role
             JOIN role_permissions rp
               ON rp.org_id = r.org_id AND rp.role_id = r.id
             JOIN permissions p
               ON p.id = rp.permission_id
             WHERE m.org_id = $1 AND m.user_id = $2
             ORDER BY p.code",
            &[&org_id, &user_id],
        )
        .await
        .map_err(DbError::from)?;
    let permissions: Vec<String> = permission_rows.iter().map(|row| row.get(0)).collect();

    let collection_rows = txn
        .query(
            "SELECT c.id
             FROM collections c
             WHERE c.org_id = $1
               AND c.deleted_at IS NULL
               AND (
                 c.visibility = 'org'
                 OR c.owner_user_id = $2
                 OR EXISTS (
                   SELECT 1 FROM collection_user_access cua
                   WHERE cua.org_id = c.org_id
                     AND cua.collection_id = c.id
                     AND cua.user_id = $2
                 )
               )",
            &[&org_id, &user_id],
        )
        .await
        .map_err(DbError::from)?;
    let collections: Vec<Uuid> = collection_rows.iter().map(|row| row.get(0)).collect();

    OrgContext::try_new(org_id, user_id, permissions, collections).map_err(|_| SagaError::Internal)
}

async fn ensure_collection_writable(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
) -> Result<(), SagaError> {
    if !ctx.allows_collection(collection_id) {
        return Err(SagaError::PermissionDenied);
    }
    let row = txn
        .query_opt(
            "SELECT id FROM collections
             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL
             FOR UPDATE",
            &[&ctx.org_id(), &collection_id],
        )
        .await
        .map_err(DbError::from)?;
    if row.is_none() {
        return Err(SagaError::PermissionDenied);
    }
    Ok(())
}

async fn register_rows(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
    outcome: &UploadOutcome,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<RegisteredUpload, SagaError> {
    let title = outcome
        .original_filename
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Untitled")
        .chars()
        .take(200)
        .collect::<String>();
    let object_key = outcome.object_key.as_str().to_string();
    let sha = outcome.sha256_hex.clone();
    let size = outcome.size_bytes as i64;
    let content_type = mime_for(outcome.canonical_format).to_string();
    let filename = outcome.original_filename.clone();
    let upload_id = outcome.object_id;

    documents::insert(
        txn,
        ctx,
        NewDocument {
            id: document_id,
            collection_id,
            title: &title,
        },
    )
    .await?;
    txn.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, publication_state,
            is_current, content_sha256, original_object_key,
            source_filename, source_content_type, byte_size, created_by_user_id
         ) VALUES (
            $1, $2, $3, 1, 'draft', false, $4, $5, $6, $7, $8, $9
         )",
        &[
            &version_id,
            &ctx.org_id(),
            &document_id,
            &sha,
            &object_key,
            &filename,
            &content_type,
            &size,
            &ctx.user_id(),
        ],
    )
    .await
    .map_err(DbError::from)?;

    let (job_id, created_job) = if outcome.disposition == Disposition::Accepted {
        let enqueue = jobs::enqueue_within_txn(
            txn,
            ctx,
            EnqueueJob::new(
                JobType::Convert,
                JobPayload {
                    document_id: Some(document_id),
                    version_id: Some(version_id),
                    collection_id: Some(collection_id),
                    upload_id: Some(upload_id),
                    ..JobPayload::default()
                },
                format!("convert-{version_id}"),
            ),
        )
        .await?;
        (Some(enqueue.job.id), enqueue.created)
    } else {
        (None, false)
    };

    Ok(RegisteredUpload {
        document_id,
        version_id,
        job_id,
        collection_id,
        created_job,
        disposition: outcome.disposition,
    })
}

#[derive(Debug, Clone)]
pub struct ApproveIntakeRequest<'a> {
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub reason: Option<&'a str>,
    pub request_id: &'a str,
}

/// Explicit idempotent approval: requires `doc.quarantine.review`, collection-scoped.
pub async fn approve_quarantined_upload(
    pool: &Pool,
    ctx: &OrgContext,
    req: ApproveIntakeRequest<'_>,
) -> Result<RegisteredUpload, SagaError> {
    require_permission(ctx, PERMISSION_QUARANTINE_REVIEW)
        .map_err(|_| SagaError::PermissionDenied)?;
    let reason = req.reason.map(str::to_string);
    let request_id = req.request_id.to_string();
    let collection_id = req.collection_id;
    let document_id = req.document_id;
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                authz_lock::lock_principal_authz(txn, ctx.org_id(), ctx.user_id()).await?;
                let fresh = reload_principal_locked(txn, &ctx).await?;
                require_permission(&fresh, PERMISSION_QUARANTINE_REVIEW)
                    .map_err(|_| SagaError::PermissionDenied)?;
                ensure_collection_writable(txn, &fresh, collection_id).await?;

                let op = upload_operations::get_by_collection_document_for_update(
                    txn,
                    &fresh,
                    collection_id,
                    document_id,
                )
                .await
                .map_err(|error| match error {
                    DbError::NotFound => SagaError::NotFound,
                    other => SagaError::Database(other),
                })?;
                if op.state != UploadOperationState::Completed {
                    return Err(SagaError::NotFound);
                }
                if op.disposition.as_deref() != Some("quarantined") {
                    return Err(SagaError::NotFound);
                }
                let version_id = op.version_id;

                if let Some(job_id) = op.job_id {
                    return Ok(RegisteredUpload {
                        document_id,
                        version_id,
                        job_id: Some(job_id),
                        collection_id,
                        created_job: false,
                        disposition: Disposition::Quarantined,
                    });
                }

                wait_if_paused_before_approve_commit().await;

                // Re-check after pause (suspend/revoke may have landed).
                let fresh = reload_principal_locked(txn, &ctx).await?;
                require_permission(&fresh, PERMISSION_QUARANTINE_REVIEW)
                    .map_err(|_| SagaError::PermissionDenied)?;

                let enqueue = jobs::enqueue_within_txn(
                    txn,
                    &fresh,
                    EnqueueJob::new(
                        JobType::Convert,
                        JobPayload {
                            document_id: Some(document_id),
                            version_id: Some(version_id),
                            collection_id: Some(collection_id),
                            upload_id: Some(op.object_id),
                            ..JobPayload::default()
                        },
                        format!("convert-{version_id}"),
                    ),
                )
                .await?;
                upload_operations::set_job_id_and_review(
                    txn,
                    &fresh,
                    op.id,
                    enqueue.job.id,
                    fresh.user_id(),
                    reason.as_deref(),
                )
                .await?;
                let resource_id = document_id.to_string();
                audit::record_in_txn(
                    txn,
                    &fresh,
                    AuditRecord {
                        request_id: &request_id,
                        action: "upload.approve_intake",
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "collectionId": collection_id,
                            "jobId": enqueue.job.id,
                            "created": enqueue.created,
                            "reason": reason,
                        }),
                    },
                )
                .await?;
                Ok(RegisteredUpload {
                    document_id,
                    version_id,
                    job_id: Some(enqueue.job.id),
                    collection_id,
                    created_job: enqueue.created,
                    disposition: Disposition::Quarantined,
                })
            })
        }
    })
    .await
}

/// Reconcile stale ops: CAS to reconciling, then refund/delete externally.
pub async fn reconcile_stale_uploads(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    older_than: chrono::DateTime<chrono::Utc>,
    limit: i64,
) -> Result<u64, SagaError> {
    let claimed: Vec<UploadOperation> = with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                // Operation locks first (SKIP LOCKED), then no quota here yet.
                upload_operations::claim_stale_for_reconcile(txn, &ctx, older_than, limit)
                    .await
                    .map_err(SagaError::from)
            })
        }
    })
    .await?;

    let mut cleaned = 0_u64;
    for op in claimed {
        if trip_fault(SagaFaultBarrier::AfterClaimReconcile) {
            // Leave in reconciling for the barrier interleaving test.
            continue;
        }
        wait_if_paused_after_reconcile_claim().await;
        let key = op
            .object_key
            .as_deref()
            .or(op.expected_object_key.as_deref())
            .and_then(|raw| parse_key_for_org(raw, op.org_id).ok());
        compensate_op(
            pool,
            storage,
            ctx,
            op.id,
            &op.reservation_key,
            key.as_ref(),
            "reconcile_stale",
        )
        .await?;
        cleaned = cleaned.saturating_add(1);
    }
    Ok(cleaned)
}

/// Backward-compatible name.
pub async fn reconcile_stale_object_stored(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    older_than: chrono::DateTime<chrono::Utc>,
    limit: i64,
) -> Result<u64, SagaError> {
    reconcile_stale_uploads(pool, storage, ctx, older_than, limit).await
}

async fn compensate_op(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    op_id: Uuid,
    reservation_key: &str,
    object_key: Option<&ObjectKey>,
    error_code: &str,
) -> Result<(), SagaError> {
    // Claim reconciling if still object_stored (commit race loses here safely).
    let claimed: bool = with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let op = upload_operations::get_by_id_for_update(txn, &ctx, op_id).await?;
                let claimed = match op.state {
                    UploadOperationState::Completed | UploadOperationState::Refunded => false,
                    UploadOperationState::ObjectStored => {
                        upload_operations::cas_state(
                            txn,
                            &ctx,
                            op_id,
                            UploadOperationState::ObjectStored,
                            UploadOperationState::Reconciling,
                        )
                        .await?;
                        true
                    }
                    UploadOperationState::Reconciling
                    | UploadOperationState::CleanupPending
                    | UploadOperationState::Reserved
                    | UploadOperationState::Putting
                    | UploadOperationState::Started
                    | UploadOperationState::Failed => true,
                };
                Ok::<bool, SagaError>(claimed)
            })
        }
    })
    .await?;
    if !claimed {
        return Ok(());
    }

    // Quota after operation claim (lock order respected across txns via CAS).
    if let Err(error) = quota::refund_upload(pool, ctx, reservation_key).await {
        eprintln!(
            "fileconv-server: upload saga refund failed; reservation_key={} code={}",
            reservation_key,
            error.code()
        );
    }

    if let Some(key) = object_key {
        if let Err(cleanup_error) = storage.cleanup_generated_object(ctx.org_id(), key).await {
            eprintln!(
                "fileconv-server: upload saga object cleanup failed; code={}",
                cleanup_error.code()
            );
            let _ = with_org_txn_typed(pool, ctx, {
                let ctx = ctx.clone();
                let error_code = error_code.to_string();
                move |txn| {
                    Box::pin(async move {
                        let op = upload_operations::get_by_id_for_update(txn, &ctx, op_id).await?;
                        if op.state != UploadOperationState::Completed {
                            upload_operations::mark_cleanup_pending(txn, &ctx, op_id, &error_code)
                                .await?;
                        }
                        Ok::<(), SagaError>(())
                    })
                }
            })
            .await;
            return Ok(());
        }
    }

    let _ = with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let error_code = error_code.to_string();
        move |txn| {
            Box::pin(async move {
                let op = upload_operations::get_by_id_for_update(txn, &ctx, op_id).await?;
                if op.state != UploadOperationState::Completed {
                    upload_operations::mark_refunded(txn, &ctx, op_id, &error_code).await?;
                }
                Ok::<(), SagaError>(())
            })
        }
    })
    .await;
    Ok(())
}

fn error_code_for(error: &SagaError) -> &'static str {
    match error {
        SagaError::PermissionDenied => "permission_denied",
        SagaError::NotFound => "not_found",
        SagaError::Quota(_) => "quota_finalize_failed",
        SagaError::Database(_) => "database_error",
        SagaError::Job(_) => "job_enqueue_failed",
        SagaError::IdempotencyConflict => "idempotency_conflict",
        SagaError::IdempotencyInProgress => "idempotency_in_progress",
        SagaError::Upload(_) => "upload_error",
        SagaError::Internal => "internal",
    }
}

fn parse_threat_class(value: &str) -> Option<super::ThreatClass> {
    use super::ThreatClass::*;
    Some(match value {
        "extension_spoof" => ExtensionSpoof,
        "mime_mismatch" => MimeMismatch,
        "unsupported_format" => UnsupportedFormat,
        "archive_bomb" => ArchiveBomb,
        "archive_path_traversal" => ArchiveTraversal,
        "nested_archive" => NestedArchive,
        "malformed_ooxml" => MalformedOoxml,
        "parser_corruption" => ParserCorruption,
        "oversize" => Oversize,
        "truncated_upload" => TruncatedUpload,
        "pdf_page_bomb" => PdfPageBomb,
        "image_pixel_bomb" => ImagePixelBomb,
        "audio_duration_limit" => AudioDurationLimit,
        "csv_formula" => CsvFormula,
        "prompt_injection" => PromptInjection,
        "active_content" => ActiveContent,
        "permission_denied" => PermissionDenied,
        "storage_failure" => StorageFailure,
        "multipart_invalid" => MultipartInvalid,
        "internal" => Internal,
        _ => return None,
    })
}

fn parse_reason_code(value: &str) -> Option<super::ReasonCode> {
    use super::ReasonCode::*;
    Some(match value {
        "extension_magic_mismatch" => ExtensionMagicMismatch,
        "magic_unrecognized" => MagicUnrecognized,
        "upload_too_large" => UploadTooLarge,
        "stream_interrupted" => StreamInterrupted,
        "archive_entry_limit" => ArchiveEntryLimit,
        "archive_uncompressed_limit" => ArchiveUncompressedLimit,
        "archive_compression_ratio" => ArchiveCompressionRatio,
        "archive_path_traversal" => ArchivePathTraversal,
        "nested_archive_entry" => NestedArchiveEntry,
        "missing_content_types" => MissingContentTypes,
        "missing_format_paths" => MissingFormatPaths,
        "malformed_archive" => MalformedArchive,
        "malformed_xml" => MalformedXml,
        "pdf_missing_eof" => PdfMissingEof,
        "pdf_page_limit" => PdfPageLimit,
        "image_pixel_limit" => ImagePixelLimit,
        "audio_duration_review" => AudioDurationReview,
        "csv_formula_review" => CsvFormulaReview,
        "prompt_injection_review" => PromptInjectionReview,
        "html_active_content" => HtmlActiveContent,
        "permission_denied" => PermissionDenied,
        "storage_unavailable" => StorageUnavailable,
        "multipart_missing_file" => MultipartMissingFile,
        "multipart_too_many_files" => MultipartTooManyFiles,
        "multipart_too_many_parts" => MultipartTooManyParts,
        "multipart_header_too_large" => MultipartHeaderTooLarge,
        "multipart_timeout" => MultipartTimeout,
        "fail_closed" => FailClosed,
        _ => return None,
    })
}

fn mime_for(format: super::CanonicalFormat) -> &'static str {
    match format {
        super::CanonicalFormat::Pdf => "application/pdf",
        super::CanonicalFormat::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        super::CanonicalFormat::Pptx => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        super::CanonicalFormat::Xlsx => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        super::CanonicalFormat::Ods => "application/vnd.oasis.opendocument.spreadsheet",
        super::CanonicalFormat::Csv => "text/csv",
        super::CanonicalFormat::Html => "text/html",
        super::CanonicalFormat::PlainText => "text/plain",
        super::CanonicalFormat::Png => "image/png",
        super::CanonicalFormat::Jpeg => "image/jpeg",
        super::CanonicalFormat::Webp => "image/webp",
        super::CanonicalFormat::Tiff => "image/tiff",
        super::CanonicalFormat::Bmp => "image/bmp",
        super::CanonicalFormat::Wav => "audio/wav",
        super::CanonicalFormat::Mp3 => "audio/mpeg",
        super::CanonicalFormat::Ogg => "audio/ogg",
        super::CanonicalFormat::Flac => "audio/flac",
        super::CanonicalFormat::M4a => "audio/mp4",
        super::CanonicalFormat::Xls => "application/vnd.ms-excel",
        super::CanonicalFormat::Xlsb => "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        super::CanonicalFormat::ZipContainer => "application/zip",
    }
}
