//! Durable vector-write cleanup intents (P1B-I07 kill/race safety).

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use thiserror::Error;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::pool::{apply_org_context, OrgTxnTypedFuture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorCleanupIntentStatus {
    Pending,
    Writing,
    Cleaned,
    Committed,
}

impl VectorCleanupIntentStatus {
    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "pending" => Ok(Self::Pending),
            "writing" => Ok(Self::Writing),
            "cleaned" => Ok(Self::Cleaned),
            "committed" => Ok(Self::Committed),
            // Legacy 0014 value — treat as committed for forward compatibility.
            "completed" => Ok(Self::Committed),
            other => Err(DbError::Config(format!(
                "unknown vector cleanup intent status: {other}"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Writing => "writing",
            Self::Cleaned => "cleaned",
            Self::Committed => "committed",
        }
    }

    pub const fn blocks_purge(self) -> bool {
        matches!(self, Self::Pending | Self::Writing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentEvent {
    BeginWrite,
    MarkCleaned,
    MarkCommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentTransitionError {
    AlreadyCleaned,
    InvalidState,
}

/// Pure CAS transition table used by repositories and hermetic tests.
pub fn apply_intent_event(
    status: VectorCleanupIntentStatus,
    event: IntentEvent,
) -> Result<VectorCleanupIntentStatus, IntentTransitionError> {
    match (status, event) {
        (VectorCleanupIntentStatus::Pending, IntentEvent::BeginWrite) => {
            Ok(VectorCleanupIntentStatus::Writing)
        }
        (
            VectorCleanupIntentStatus::Pending | VectorCleanupIntentStatus::Writing,
            IntentEvent::MarkCleaned,
        ) => Ok(VectorCleanupIntentStatus::Cleaned),
        (VectorCleanupIntentStatus::Writing, IntentEvent::MarkCommitted) => {
            Ok(VectorCleanupIntentStatus::Committed)
        }
        (VectorCleanupIntentStatus::Cleaned, IntentEvent::BeginWrite)
        | (VectorCleanupIntentStatus::Cleaned, IntentEvent::MarkCommitted) => {
            Err(IntentTransitionError::AlreadyCleaned)
        }
        (VectorCleanupIntentStatus::Committed, _) => Err(IntentTransitionError::InvalidState),
        (VectorCleanupIntentStatus::Cleaned, IntentEvent::MarkCleaned) => {
            Ok(VectorCleanupIntentStatus::Cleaned)
        }
        _ => Err(IntentTransitionError::InvalidState),
    }
}

/// Writer may call the external upsert only while the intent is still `writing`.
pub fn authorize_vector_upsert(
    status: VectorCleanupIntentStatus,
) -> Result<(), IntentTransitionError> {
    match status {
        VectorCleanupIntentStatus::Writing => Ok(()),
        VectorCleanupIntentStatus::Cleaned => Err(IntentTransitionError::AlreadyCleaned),
        _ => Err(IntentTransitionError::InvalidState),
    }
}

/// How cleanup must treat an open intent (production orchestration plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentCleanupPlan {
    /// Safe to finalize first: writer has not entered the upsert window.
    CleanThenDelete,
    /// Writer holds `writing`: fence/cancel, delete possible write, then finalize.
    CancelDeleteThenClean,
}

pub fn plan_intent_cleanup(status: VectorCleanupIntentStatus) -> Option<IntentCleanupPlan> {
    match status {
        VectorCleanupIntentStatus::Pending => Some(IntentCleanupPlan::CleanThenDelete),
        VectorCleanupIntentStatus::Writing => Some(IntentCleanupPlan::CancelDeleteThenClean),
        VectorCleanupIntentStatus::Cleaned | VectorCleanupIntentStatus::Committed => None,
    }
}

/// Open intent view for fake-backed drain orchestration tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCleanupIntent {
    pub job_id: Uuid,
    pub status: VectorCleanupIntentStatus,
    pub point_ids: Vec<Uuid>,
    pub index_signature_sha256: String,
}

/// Backend used by production drain orchestration (real PG/Qdrant or hermetic fake).
pub trait IntentDrainBackend {
    fn list_open(&mut self) -> Vec<OpenCleanupIntent>;
    fn cancel_writers(&mut self) -> Result<(), String>;
    fn delete_points(&mut self, intent: &OpenCleanupIntent) -> Result<(), String>;
    fn mark_cleaned(&mut self, job_id: Uuid) -> Result<(), IntentTransitionError>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntentDrainReport {
    pub cleaned: usize,
    pub points_deleted: usize,
    pub writers_cancelled: bool,
}

/// Production cleanup orchestration shared by the delete worker and hermetic tests.
///
/// Callers must hold [`with_vector_mutation_lock`] (or an equivalent shared lock)
/// across both this plan and the external Qdrant delete so a writer cannot upsert
/// between authorize and cleanup finalize.
pub fn drain_cleanup_intents_orchestrated<B: IntentDrainBackend>(
    backend: &mut B,
) -> Result<IntentDrainReport, String> {
    let open = backend.list_open();
    let mut report = IntentDrainReport::default();
    let mut cancelled = false;
    for intent in open {
        let Some(plan) = plan_intent_cleanup(intent.status) else {
            continue;
        };
        match plan {
            IntentCleanupPlan::CleanThenDelete => {
                backend
                    .mark_cleaned(intent.job_id)
                    .map_err(|error| format!("mark_cleaned: {error:?}"))?;
                let count = intent.point_ids.len();
                backend.delete_points(&intent)?;
                report.cleaned += 1;
                report.points_deleted += count;
            }
            IntentCleanupPlan::CancelDeleteThenClean => {
                if !cancelled {
                    backend.cancel_writers()?;
                    cancelled = true;
                    report.writers_cancelled = true;
                }
                let count = intent.point_ids.len();
                backend.delete_points(&intent)?;
                backend
                    .mark_cleaned(intent.job_id)
                    .map_err(|error| format!("mark_cleaned: {error:?}"))?;
                report.cleaned += 1;
                report.points_deleted += count;
            }
        }
    }
    Ok(report)
}

fn vector_mutation_lock_key(org_id: Uuid, document_id: Uuid) -> String {
    format!("vector-mutation:{org_id}:{document_id}")
}

/// Serialize vector upsert and cleanup for one document.
///
/// Holds a transaction-scoped advisory lock for the full closure, including any
/// external Qdrant I/O inside `f`. Writers should CAS `pending → writing` inside
/// this lock immediately before upsert and only commit after the upsert returns,
/// so a kill rolls back `writing` while the durable `pending` intent (recorded
/// earlier) still covers any orphaned points.
pub async fn with_vector_mutation_lock<T, F, E>(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> OrgTxnTypedFuture<'c, T, E>,
    E: From<DbError>,
{
    let mut client = pool.get().await.map_err(DbError::from).map_err(E::from)?;
    let txn = client
        .transaction()
        .await
        .map_err(DbError::from)
        .map_err(E::from)?;
    apply_org_context(&txn, ctx).await.map_err(E::from)?;
    let lock_key = vector_mutation_lock_key(ctx.org_id(), document_id);
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&lock_key])
        .await
        .map_err(DbError::from)
        .map_err(E::from)?;
    match f(&txn).await {
        Ok(value) => {
            txn.commit().await.map_err(DbError::from).map_err(E::from)?;
            Ok(value)
        }
        Err(error) => {
            let _ = txn.rollback().await;
            Err(error)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorCleanupIntent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub job_id: Uuid,
    pub index_signature_sha256: String,
    pub point_ids: Vec<Uuid>,
    pub status: VectorCleanupIntentStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewVectorCleanupIntent<'a> {
    pub document_id: Uuid,
    pub job_id: Uuid,
    pub index_signature_sha256: &'a str,
    pub point_ids: &'a [Uuid],
}

const COLUMNS: &str = "id, org_id, document_id, job_id, index_signature_sha256, point_ids, \
    status, created_at, updated_at";

#[derive(Debug, Error)]
pub enum VectorCleanupIntentError {
    #[error("database error")]
    Db(#[from] DbError),
    #[error("cleanup intent was already cleaned; upsert is fenced")]
    AlreadyCleaned,
    #[error("cleanup intent transition is invalid")]
    InvalidState,
}

impl From<IntentTransitionError> for VectorCleanupIntentError {
    fn from(value: IntentTransitionError) -> Self {
        match value {
            IntentTransitionError::AlreadyCleaned => Self::AlreadyCleaned,
            IntentTransitionError::InvalidState => Self::InvalidState,
        }
    }
}

/// Upserts a pending cleanup intent. Refuses to revive cleaned/committed intents.
pub async fn upsert_pending(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewVectorCleanupIntent<'_>,
) -> Result<VectorCleanupIntent, VectorCleanupIntentError> {
    if input.point_ids.is_empty() {
        return Err(
            DbError::Config("vector cleanup intent requires at least one point id".into()).into(),
        );
    }
    let row = txn
        .query_opt(
            &format!(
                "INSERT INTO vector_cleanup_intents (
                    org_id, document_id, job_id, index_signature_sha256, point_ids, status
                 ) VALUES ($1,$2,$3,$4,$5,'pending')
                 ON CONFLICT (org_id, job_id) DO UPDATE SET
                    document_id = EXCLUDED.document_id,
                    index_signature_sha256 = EXCLUDED.index_signature_sha256,
                    point_ids = EXCLUDED.point_ids,
                    status = 'pending',
                    updated_at = clock_timestamp()
                 WHERE vector_cleanup_intents.status = 'pending'
                 RETURNING {COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &input.document_id,
                &input.job_id,
                &input.index_signature_sha256,
                &input.point_ids,
            ],
        )
        .await
        .map_err(DbError::from)?;
    if let Some(row) = row {
        return Ok(map_intent(&row)?);
    }
    // Conflict row exists but was not pending — do not revive writing/cleaned.
    match find_by_job(txn, ctx, input.job_id).await? {
        Some(intent) if intent.status == VectorCleanupIntentStatus::Writing => Ok(intent),
        Some(intent) if intent.status == VectorCleanupIntentStatus::Cleaned => {
            Err(VectorCleanupIntentError::AlreadyCleaned)
        }
        _ => Err(VectorCleanupIntentError::InvalidState),
    }
}

/// Returns the intent only when it is still `writing` (pre-upsert authorization).
pub async fn require_writing_for_upsert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<VectorCleanupIntent, VectorCleanupIntentError> {
    let intent = find_by_job(txn, ctx, job_id)
        .await?
        .ok_or(VectorCleanupIntentError::InvalidState)?;
    authorize_vector_upsert(intent.status)?;
    Ok(intent)
}

/// CAS pending → writing immediately before the external upsert.
pub async fn cas_begin_write(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<VectorCleanupIntent, VectorCleanupIntentError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE vector_cleanup_intents
                 SET status = 'writing', updated_at = clock_timestamp()
                 WHERE org_id = $1 AND job_id = $2 AND status = 'pending'
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await
        .map_err(DbError::from)?;
    match row {
        Some(row) => Ok(map_intent(&row)?),
        None => {
            let current = find_by_job(txn, ctx, job_id).await?;
            match current.map(|intent| intent.status) {
                Some(VectorCleanupIntentStatus::Cleaned) => {
                    Err(VectorCleanupIntentError::AlreadyCleaned)
                }
                _ => Err(VectorCleanupIntentError::InvalidState),
            }
        }
    }
}

/// CAS a specific open status → cleaned (pending before delete, writing after delete).
pub async fn cas_mark_cleaned_from(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
    from: VectorCleanupIntentStatus,
) -> Result<Option<VectorCleanupIntent>, VectorCleanupIntentError> {
    let from_status = from.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE vector_cleanup_intents
                 SET status = 'cleaned', updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND job_id = $2
                   AND status = $3
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id, &from_status],
        )
        .await
        .map_err(DbError::from)?;
    row.map(|row| map_intent(&row))
        .transpose()
        .map_err(Into::into)
}

/// Convenience: mark cleaned from whichever open state is present.
pub async fn cas_mark_cleaned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<VectorCleanupIntent>, VectorCleanupIntentError> {
    if let Some(intent) =
        cas_mark_cleaned_from(txn, ctx, job_id, VectorCleanupIntentStatus::Pending).await?
    {
        return Ok(Some(intent));
    }
    cas_mark_cleaned_from(txn, ctx, job_id, VectorCleanupIntentStatus::Writing).await
}

/// CAS writing → committed after a successful batch complete.
pub async fn cas_mark_committed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<VectorCleanupIntent, VectorCleanupIntentError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE vector_cleanup_intents
                 SET status = 'committed', updated_at = clock_timestamp()
                 WHERE org_id = $1 AND job_id = $2 AND status = 'writing'
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await
        .map_err(DbError::from)?;
    match row {
        Some(row) => Ok(map_intent(&row)?),
        None => {
            let current = find_by_job(txn, ctx, job_id).await?;
            match current.map(|intent| intent.status) {
                Some(VectorCleanupIntentStatus::Cleaned) => {
                    Err(VectorCleanupIntentError::AlreadyCleaned)
                }
                _ => Err(VectorCleanupIntentError::InvalidState),
            }
        }
    }
}

pub async fn has_open_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT EXISTS (
                SELECT 1
                FROM vector_cleanup_intents
                WHERE org_id = $1
                  AND document_id = $2
                  AND status IN ('pending', 'writing')
             )",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn list_open_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<VectorCleanupIntent>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {COLUMNS}
                 FROM vector_cleanup_intents
                 WHERE org_id = $1
                   AND document_id = $2
                   AND status IN ('pending', 'writing')
                 ORDER BY created_at, id
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_intent).collect()
}

async fn find_by_job(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<VectorCleanupIntent>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {COLUMNS}
                 FROM vector_cleanup_intents
                 WHERE org_id = $1 AND job_id = $2"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await?;
    row.map(|row| map_intent(&row)).transpose()
}

fn map_intent(row: &Row) -> Result<VectorCleanupIntent, DbError> {
    Ok(VectorCleanupIntent {
        id: row.get("id"),
        org_id: row.get("org_id"),
        document_id: row.get("document_id"),
        job_id: row.get("job_id"),
        index_signature_sha256: row.get("index_signature_sha256"),
        point_ids: row.get("point_ids"),
        status: VectorCleanupIntentStatus::parse(row.get("status"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaned_intent_cannot_begin_write_or_commit() {
        assert_eq!(
            apply_intent_event(VectorCleanupIntentStatus::Cleaned, IntentEvent::BeginWrite),
            Err(IntentTransitionError::AlreadyCleaned)
        );
        assert_eq!(
            apply_intent_event(
                VectorCleanupIntentStatus::Cleaned,
                IntentEvent::MarkCommitted
            ),
            Err(IntentTransitionError::AlreadyCleaned)
        );
    }

    #[test]
    fn happy_path_pending_writing_committed() {
        let writing =
            apply_intent_event(VectorCleanupIntentStatus::Pending, IntentEvent::BeginWrite)
                .unwrap();
        assert_eq!(writing, VectorCleanupIntentStatus::Writing);
        assert_eq!(
            apply_intent_event(writing, IntentEvent::MarkCommitted).unwrap(),
            VectorCleanupIntentStatus::Committed
        );
    }

    #[test]
    fn drain_can_clean_pending_or_writing() {
        assert_eq!(
            apply_intent_event(VectorCleanupIntentStatus::Pending, IntentEvent::MarkCleaned)
                .unwrap(),
            VectorCleanupIntentStatus::Cleaned
        );
        assert_eq!(
            apply_intent_event(VectorCleanupIntentStatus::Writing, IntentEvent::MarkCleaned)
                .unwrap(),
            VectorCleanupIntentStatus::Cleaned
        );
    }

    #[test]
    fn open_states_block_purge() {
        assert!(VectorCleanupIntentStatus::Pending.blocks_purge());
        assert!(VectorCleanupIntentStatus::Writing.blocks_purge());
        assert!(!VectorCleanupIntentStatus::Cleaned.blocks_purge());
        assert!(!VectorCleanupIntentStatus::Committed.blocks_purge());
    }

    #[test]
    fn cleaned_writer_must_not_upsert() {
        assert!(authorize_vector_upsert(VectorCleanupIntentStatus::Writing).is_ok());
        assert_eq!(
            authorize_vector_upsert(VectorCleanupIntentStatus::Cleaned),
            Err(IntentTransitionError::AlreadyCleaned)
        );
    }

    #[test]
    fn writing_cleanup_plans_cancel_before_finalize() {
        assert_eq!(
            plan_intent_cleanup(VectorCleanupIntentStatus::Pending),
            Some(IntentCleanupPlan::CleanThenDelete)
        );
        assert_eq!(
            plan_intent_cleanup(VectorCleanupIntentStatus::Writing),
            Some(IntentCleanupPlan::CancelDeleteThenClean)
        );
    }
}
