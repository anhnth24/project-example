//! Durable embedding-batch and generation-backfill repositories.
//!
//! A batch stores only IDs, ordinal range, and a canonical-input checksum. The
//! actual chunk text remains in PostgreSQL and is loaded by the embedding worker
//! after it has acquired the durable job lease.

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingBatchStatus {
    Pending,
    Succeeded,
    Failed,
}

impl EmbeddingBatchStatus {
    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "pending" => Ok(Self::Pending),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            other => Err(DbError::Config(format!(
                "unknown embedding batch status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingBatch {
    pub id: Uuid,
    pub org_id: Uuid,
    pub index_job_id: Uuid,
    pub job_id: Uuid,
    pub index_metadata_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub start_ordinal: i32,
    pub end_ordinal: i32,
    pub input_sha256: String,
    pub status: EmbeddingBatchStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewEmbeddingBatch<'a> {
    pub id: Uuid,
    pub index_job_id: Uuid,
    pub job_id: Uuid,
    pub index_metadata_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub start_ordinal: i32,
    pub end_ordinal: i32,
    pub input_sha256: &'a str,
}

const COLUMNS: &str = "id, org_id, index_job_id, job_id, index_metadata_id, document_id, \
    version_id, start_ordinal, end_ordinal, input_sha256, status, created_at, completed_at";

pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewEmbeddingBatch<'_>,
) -> Result<EmbeddingBatch, DbError> {
    let row = txn
        .query_one(
            &format!(
                "INSERT INTO embedding_batches (
                    id, org_id, index_job_id, job_id, index_metadata_id, document_id,
                    version_id, start_ordinal, end_ordinal, input_sha256
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                 RETURNING {COLUMNS}"
            ),
            &[
                &input.id,
                &ctx.org_id(),
                &input.index_job_id,
                &input.job_id,
                &input.index_metadata_id,
                &input.document_id,
                &input.version_id,
                &input.start_ordinal,
                &input.end_ordinal,
                &input.input_sha256,
            ],
        )
        .await?;
    map_batch(&row)
}

pub async fn find_by_id_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    batch_id: Uuid,
) -> Result<Option<EmbeddingBatch>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {COLUMNS}
                 FROM embedding_batches
                 WHERE org_id = $1 AND id = $2
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &batch_id],
        )
        .await?;
    row.map(|row| map_batch(&row)).transpose()
}

pub async fn find_by_job_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<Option<EmbeddingBatch>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {COLUMNS}
                 FROM embedding_batches
                 WHERE org_id = $1 AND job_id = $2"
            ),
            &[&ctx.org_id(), &job_id],
        )
        .await?;
    row.map(|row| map_batch(&row)).transpose()
}

pub async fn mark_succeeded(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    batch_id: Uuid,
) -> Result<EmbeddingBatch, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE embedding_batches
                 SET status = 'succeeded', completed_at = COALESCE(completed_at, clock_timestamp())
                 WHERE org_id = $1 AND id = $2 AND status IN ('pending', 'succeeded')
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &batch_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_batch(&row)
}

/// Resets a succeeded/failed batch so a repair embedding job can replay it.
pub async fn requeue_for_repair(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    batch_id: Uuid,
    new_job_id: Uuid,
) -> Result<EmbeddingBatch, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE embedding_batches
                 SET status = 'pending',
                     job_id = $3,
                     completed_at = NULL
                 WHERE org_id = $1 AND id = $2
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &batch_id, &new_job_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_batch(&row)
}

pub async fn list_by_document_version(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<Vec<EmbeddingBatch>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {COLUMNS}
                 FROM embedding_batches
                 WHERE org_id = $1
                   AND index_metadata_id = $2
                   AND document_id = $3
                   AND version_id = $4
                 ORDER BY start_ordinal, id
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &index_metadata_id, &document_id, &version_id],
        )
        .await?;
    rows.iter().map(map_batch).collect()
}

pub async fn mark_failed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    batch_id: Uuid,
) -> Result<EmbeddingBatch, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE embedding_batches
                 SET status = 'failed', completed_at = clock_timestamp()
                 WHERE org_id = $1 AND id = $2 AND status <> 'succeeded'
                 RETURNING {COLUMNS}"
            ),
            &[&ctx.org_id(), &batch_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_batch(&row)
}

/// Records every current version that must be rebuilt in a staging generation.
/// Inserts are idempotent, so an interrupted expand can safely resume.
pub async fn seed_generation_backfills(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    collection_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, DbError> {
    let rows = txn
        .query(
            "WITH source AS (
                SELECT d.id AS document_id, d.current_version_id AS version_id
                FROM documents d
                WHERE d.org_id = $1
                  AND d.collection_id = $2
                  AND d.deleted_at IS NULL
                  AND d.current_version_id IS NOT NULL
             ),
             inserted AS (
                INSERT INTO index_generation_backfills (
                    org_id, index_metadata_id, document_id, version_id
                )
                SELECT $1, $3, document_id, version_id
                FROM source
                ON CONFLICT (org_id, index_metadata_id, document_id, version_id) DO NOTHING
             )
             SELECT document_id, version_id FROM source
             ORDER BY document_id",
            &[&ctx.org_id(), &collection_id, &index_metadata_id],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get("document_id"), row.get("version_id")))
        .collect())
}

pub async fn mark_generation_indexing(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), DbError> {
    txn.execute(
        "UPDATE index_generation_backfills
         SET status = 'indexing'
         WHERE org_id = $1
           AND index_metadata_id = $2
           AND document_id = $3
           AND version_id = $4
           AND status = 'pending'",
        &[&ctx.org_id(), &index_metadata_id, &document_id, &version_id],
    )
    .await?;
    Ok(())
}

pub async fn mark_generation_backfilled(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), DbError> {
    txn.execute(
        "UPDATE index_generation_backfills
         SET status = 'backfilled', completed_at = COALESCE(completed_at, clock_timestamp())
         WHERE org_id = $1
           AND index_metadata_id = $2
           AND document_id = $3
           AND version_id = $4
           AND status IN ('pending', 'indexing', 'backfilled')",
        &[&ctx.org_id(), &index_metadata_id, &document_id, &version_id],
    )
    .await?;
    Ok(())
}

pub async fn mark_generation_failed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), DbError> {
    txn.execute(
        "UPDATE index_generation_backfills
         SET status = 'failed', completed_at = clock_timestamp()
         WHERE org_id = $1
           AND index_metadata_id = $2
           AND document_id = $3
           AND version_id = $4
           AND status <> 'backfilled'",
        &[&ctx.org_id(), &index_metadata_id, &document_id, &version_id],
    )
    .await?;
    Ok(())
}

pub async fn document_batches_complete(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT count(*)::bigint > 0 AS has_batches,
                    COALESCE(bool_and(batch.status = 'succeeded'), false) AS all_batches_succeeded,
                    COALESCE(bool_and(index_job.status = 'succeeded'), false)
                        AS all_index_jobs_succeeded
             FROM embedding_batches AS batch
             INNER JOIN jobs AS index_job
                ON index_job.org_id = batch.org_id
               AND index_job.id = batch.index_job_id
             WHERE batch.org_id = $1
               AND batch.index_metadata_id = $2
               AND batch.document_id = $3
               AND batch.version_id = $4",
            &[&ctx.org_id(), &index_metadata_id, &document_id, &version_id],
        )
        .await?;
    Ok(all_document_batches_complete(
        row.get("has_batches"),
        row.get("all_batches_succeeded"),
        row.get("all_index_jobs_succeeded"),
    ))
}

pub async fn generation_backfill_complete(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    index_metadata_id: Uuid,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT count(*)::bigint > 0
                    AND bool_and(status = 'backfilled')
             FROM index_generation_backfills
             WHERE org_id = $1 AND index_metadata_id = $2",
            &[&ctx.org_id(), &index_metadata_id],
        )
        .await?;
    Ok(row.get(0))
}

fn map_batch(row: &Row) -> Result<EmbeddingBatch, DbError> {
    Ok(EmbeddingBatch {
        id: row.get("id"),
        org_id: row.get("org_id"),
        index_job_id: row.get("index_job_id"),
        job_id: row.get("job_id"),
        index_metadata_id: row.get("index_metadata_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        start_ordinal: row.get("start_ordinal"),
        end_ordinal: row.get("end_ordinal"),
        input_sha256: row.get("input_sha256"),
        status: EmbeddingBatchStatus::parse(&row.get::<_, String>("status"))?,
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}

fn all_document_batches_complete(
    has_batches: bool,
    all_batches_succeeded: bool,
    all_index_jobs_succeeded: bool,
) -> bool {
    has_batches && all_batches_succeeded && all_index_jobs_succeeded
}

#[cfg(test)]
mod tests {
    use super::all_document_batches_complete;

    #[test]
    fn document_finalization_waits_for_parent_index_job_and_all_batches() {
        assert!(!all_document_batches_complete(false, true, true));
        assert!(!all_document_batches_complete(true, false, true));
        assert!(!all_document_batches_complete(true, true, false));
        assert!(all_document_batches_complete(true, true, true));
    }
}
