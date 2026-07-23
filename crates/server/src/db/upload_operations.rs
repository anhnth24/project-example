//! Durable upload idempotency + reconciliation intents (P1B upload saga).

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOperationState {
    Started,
    Reserved,
    Putting,
    ObjectStored,
    Reconciling,
    CleanupPending,
    Completed,
    Refunded,
    Failed,
}

impl UploadOperationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Reserved => "reserved",
            Self::Putting => "putting",
            Self::ObjectStored => "object_stored",
            Self::Reconciling => "reconciling",
            Self::CleanupPending => "cleanup_pending",
            Self::Completed => "completed",
            Self::Refunded => "refunded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "started" => Ok(Self::Started),
            "reserved" => Ok(Self::Reserved),
            "putting" => Ok(Self::Putting),
            "object_stored" => Ok(Self::ObjectStored),
            "reconciling" => Ok(Self::Reconciling),
            "cleanup_pending" => Ok(Self::CleanupPending),
            "completed" => Ok(Self::Completed),
            "refunded" => Ok(Self::Refunded),
            "failed" => Ok(Self::Failed),
            other => Err(DbError::Config(format!(
                "unknown upload_operations.state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadOperation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub idempotency_key: String,
    pub envelope_sha256: String,
    pub content_sha256: String,
    pub state: UploadOperationState,
    pub attempt: i32,
    pub reservation_key: String,
    pub expected_object_key: Option<String>,
    pub object_key: Option<String>,
    pub object_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub job_id: Option<Uuid>,
    pub disposition: Option<String>,
    pub size_bytes: Option<i64>,
    pub canonical_format: Option<String>,
    pub original_filename: Option<String>,
    pub threat_class: Option<String>,
    pub reason_code: Option<String>,
    pub reviewed_by_user_id: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_reason: Option<String>,
    pub error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewUploadOperation<'a> {
    pub id: Uuid,
    pub idempotency_key: &'a str,
    pub envelope_sha256: &'a str,
    pub content_sha256: &'a str,
    pub reservation_key: &'a str,
    pub expected_object_key: &'a str,
    pub object_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub size_bytes: i64,
    pub original_filename: Option<&'a str>,
}

const SELECT_COLS: &str = "id, org_id, user_id, idempotency_key, envelope_sha256, content_sha256,
        state, attempt, reservation_key, expected_object_key, object_key, object_id,
        collection_id, document_id, version_id, job_id, disposition, size_bytes,
        canonical_format, original_filename, threat_class, reason_code,
        reviewed_by_user_id, reviewed_at, review_reason, error_code, created_at, updated_at";

pub async fn insert_started(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewUploadOperation<'_>,
) -> Result<Option<UploadOperation>, DbError> {
    let state = UploadOperationState::Started.as_str();
    let row = txn
        .query_opt(
            &format!(
                "INSERT INTO upload_operations (
                    id, org_id, user_id, idempotency_key, envelope_sha256, content_sha256,
                    state, attempt, reservation_key, expected_object_key, object_id,
                    collection_id, document_id, version_id, size_bytes, original_filename
                 ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, 1, $8, $9, $10, $11, $12, $13, $14, $15
                 )
                 ON CONFLICT (org_id, user_id, idempotency_key) DO NOTHING
                 RETURNING {SELECT_COLS}"
            ),
            &[
                &input.id,
                &ctx.org_id(),
                &ctx.user_id(),
                &input.idempotency_key,
                &input.envelope_sha256,
                &input.content_sha256,
                &state,
                &input.reservation_key,
                &input.expected_object_key,
                &input.object_id,
                &input.collection_id,
                &input.document_id,
                &input.version_id,
                &input.size_bytes,
                &input.original_filename,
            ],
        )
        .await?;
    row.map(|row| map_row(&row)).transpose()
}

pub async fn get_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    idempotency_key: &str,
) -> Result<UploadOperation, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {SELECT_COLS}
                 FROM upload_operations
                 WHERE org_id = $1 AND user_id = $2 AND idempotency_key = $3
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &ctx.user_id(), &idempotency_key],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_row(&row)
}

pub async fn get_by_id_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
) -> Result<UploadOperation, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {SELECT_COLS}
                 FROM upload_operations
                 WHERE org_id = $1 AND id = $2
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &operation_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_row(&row)
}

/// Collection-scoped lookup for quarantine approval (IDOR → NotFound).
pub async fn get_by_collection_document_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
    document_id: Uuid,
) -> Result<UploadOperation, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {SELECT_COLS}
                 FROM upload_operations
                 WHERE org_id = $1 AND collection_id = $2 AND document_id = $3
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &collection_id, &document_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_row(&row)
}

pub async fn cas_state(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    from: UploadOperationState,
    to: UploadOperationState,
) -> Result<UploadOperation, DbError> {
    let from_s = from.as_str();
    let to_s = to.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $4, updated_at = now()
                 WHERE org_id = $1 AND id = $2 AND state = $3
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &from_s, &to_s],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: from.as_str().into(),
            observed: "other".into(),
        })?;
    map_row(&row)
}

pub async fn mark_reserved(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    reservation_key: &str,
    attempt: i32,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::Reserved.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3,
                     reservation_key = $4,
                     attempt = $5,
                     error_code = NULL,
                     updated_at = now()
                 WHERE org_id = $1 AND id = $2
                   AND state IN ('started', 'failed', 'refunded')
                 RETURNING {SELECT_COLS}"
            ),
            &[
                &ctx.org_id(),
                &operation_id,
                &state,
                &reservation_key,
                &attempt,
            ],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "started|failed|refunded".into(),
            observed: "other".into(),
        })?;
    map_row(&row)
}

pub async fn mark_putting(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
) -> Result<UploadOperation, DbError> {
    cas_state(
        txn,
        ctx,
        operation_id,
        UploadOperationState::Reserved,
        UploadOperationState::Putting,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn mark_object_stored(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    object_key: &str,
    disposition: &str,
    canonical_format: &str,
    threat_class: Option<&str>,
    reason_code: Option<&str>,
    original_filename: Option<&str>,
    size_bytes: i64,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::ObjectStored.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3,
                     object_key = $4,
                     disposition = $5,
                     canonical_format = $6,
                     threat_class = $7,
                     reason_code = $8,
                     original_filename = COALESCE($9, original_filename),
                     size_bytes = $10,
                     updated_at = now()
                 WHERE org_id = $1 AND id = $2 AND state = 'putting'
                 RETURNING {SELECT_COLS}"
            ),
            &[
                &ctx.org_id(),
                &operation_id,
                &state,
                &object_key,
                &disposition,
                &canonical_format,
                &threat_class,
                &reason_code,
                &original_filename,
                &size_bytes,
            ],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "putting".into(),
            observed: "other".into(),
        })?;
    map_row(&row)
}

/// Commit registration: CAS only `object_stored` → `completed` (never from reconciling).
pub async fn mark_completed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::Completed.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3,
                     job_id = $4,
                     error_code = NULL,
                     updated_at = now()
                 WHERE org_id = $1 AND id = $2 AND state = 'object_stored'
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &state, &job_id],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "object_stored".into(),
            observed: "other".into(),
        })?;
    map_row(&row)
}

pub async fn mark_cleanup_pending(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    error_code: &str,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::CleanupPending.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3, error_code = $4, updated_at = now()
                 WHERE org_id = $1 AND id = $2
                   AND state IN ('reconciling', 'cleanup_pending', 'object_stored')
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &state, &error_code],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "reconciling|cleanup_pending|object_stored".into(),
            observed: "other".into(),
        })?;
    map_row(&row)
}

pub async fn mark_refunded(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    error_code: &str,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::Refunded.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3, error_code = $4, updated_at = now()
                 WHERE org_id = $1 AND id = $2
                   AND state NOT IN ('completed')
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &state, &error_code],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "non-completed".into(),
            observed: "completed".into(),
        })?;
    map_row(&row)
}

pub async fn mark_failed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    error_code: &str,
) -> Result<UploadOperation, DbError> {
    let state = UploadOperationState::Failed.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET state = $3, error_code = $4, updated_at = now()
                 WHERE org_id = $1 AND id = $2
                   AND state NOT IN ('completed')
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &state, &error_code],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "non-completed".into(),
            observed: "completed".into(),
        })?;
    map_row(&row)
}

pub async fn set_job_id_and_review(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    operation_id: Uuid,
    job_id: Uuid,
    reviewer_id: Uuid,
    reason: Option<&str>,
) -> Result<UploadOperation, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE upload_operations
                 SET job_id = $3,
                     reviewed_by_user_id = $4,
                     reviewed_at = now(),
                     review_reason = $5,
                     updated_at = now()
                 WHERE org_id = $1 AND id = $2 AND state = 'completed'
                 RETURNING {SELECT_COLS}"
            ),
            &[&ctx.org_id(), &operation_id, &job_id, &reviewer_id, &reason],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_row(&row)
}

/// Claim stale open ops for reconcile: CAS to reconciling where applicable.
pub async fn claim_stale_for_reconcile(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    older_than: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<UploadOperation>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {SELECT_COLS}
                 FROM upload_operations
                 WHERE org_id = $1
                   AND state IN (
                        'started', 'reserved', 'putting', 'object_stored',
                        'reconciling', 'cleanup_pending'
                   )
                   AND updated_at < $2
                 ORDER BY updated_at ASC
                 LIMIT $3
                 FOR UPDATE SKIP LOCKED"
            ),
            &[&ctx.org_id(), &older_than, &limit],
        )
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let op = map_row(&row)?;
        let next = match op.state {
            UploadOperationState::ObjectStored | UploadOperationState::Reconciling => {
                UploadOperationState::Reconciling
            }
            UploadOperationState::CleanupPending => UploadOperationState::CleanupPending,
            UploadOperationState::Started
            | UploadOperationState::Reserved
            | UploadOperationState::Putting => UploadOperationState::Reconciling,
            _ => continue,
        };
        if op.state != next {
            out.push(cas_state(txn, ctx, op.id, op.state, next).await?);
        } else {
            out.push(op);
        }
    }
    Ok(out)
}

fn map_row(row: &Row) -> Result<UploadOperation, DbError> {
    Ok(UploadOperation {
        id: row.get("id"),
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        idempotency_key: row.get("idempotency_key"),
        envelope_sha256: row.get("envelope_sha256"),
        content_sha256: row.get("content_sha256"),
        state: UploadOperationState::parse(row.get("state"))?,
        attempt: row.get("attempt"),
        reservation_key: row.get("reservation_key"),
        expected_object_key: row.get("expected_object_key"),
        object_key: row.get("object_key"),
        object_id: row.get("object_id"),
        collection_id: row.get("collection_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        job_id: row.get("job_id"),
        disposition: row.get("disposition"),
        size_bytes: row.get("size_bytes"),
        canonical_format: row.get("canonical_format"),
        original_filename: row.get("original_filename"),
        threat_class: row.get("threat_class"),
        reason_code: row.get("reason_code"),
        reviewed_by_user_id: row.get("reviewed_by_user_id"),
        reviewed_at: row.get("reviewed_at"),
        review_reason: row.get("review_reason"),
        error_code: row.get("error_code"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
