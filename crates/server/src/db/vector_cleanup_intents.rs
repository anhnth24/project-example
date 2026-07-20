//! Durable vector-write cleanup intents (P1B-I07 kill/race safety).

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorCleanupIntentStatus {
    Pending,
    Completed,
}

impl VectorCleanupIntentStatus {
    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            other => Err(DbError::Config(format!(
                "unknown vector cleanup intent status: {other}"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
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

/// Upserts a pending cleanup intent for the embedding job's vector write.
pub async fn upsert_pending(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewVectorCleanupIntent<'_>,
) -> Result<VectorCleanupIntent, DbError> {
    if input.point_ids.is_empty() {
        return Err(DbError::Config(
            "vector cleanup intent requires at least one point id".into(),
        ));
    }
    let row = txn
        .query_one(
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
        .await?;
    map_intent(&row)
}

pub async fn mark_completed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<(), DbError> {
    txn.execute(
        "UPDATE vector_cleanup_intents
         SET status = 'completed', updated_at = clock_timestamp()
         WHERE org_id = $1 AND job_id = $2 AND status = 'pending'",
        &[&ctx.org_id(), &job_id],
    )
    .await?;
    Ok(())
}

pub async fn has_pending_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT EXISTS (
                SELECT 1
                FROM vector_cleanup_intents
                WHERE org_id = $1 AND document_id = $2 AND status = 'pending'
             )",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn list_pending_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<VectorCleanupIntent>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {COLUMNS}
                 FROM vector_cleanup_intents
                 WHERE org_id = $1 AND document_id = $2 AND status = 'pending'
                 ORDER BY created_at, id"
            ),
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_intent).collect()
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
