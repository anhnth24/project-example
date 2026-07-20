//! Durable vector-write cleanup intents (P1B-I07 kill/race safety).

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

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
                 WHERE vector_cleanup_intents.status IN ('pending', 'writing')
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
    let Some(row) = row else {
        // Conflict row exists but is cleaned/committed — fence the upsert.
        return Err(VectorCleanupIntentError::AlreadyCleaned);
    };
    Ok(map_intent(&row)?)
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

/// CAS pending|writing → cleaned after a successful scoped vector delete.
pub async fn cas_mark_cleaned(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<VectorCleanupIntent>, VectorCleanupIntentError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE vector_cleanup_intents
                 SET status = 'cleaned', updated_at = clock_timestamp()
                 WHERE org_id = $1
                   AND job_id = $2
                   AND status IN ('pending', 'writing')
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await
        .map_err(DbError::from)?;
    row.map(|row| map_intent(&row))
        .transpose()
        .map_err(Into::into)
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
}
