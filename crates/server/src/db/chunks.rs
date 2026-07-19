//! Tenant-scoped chunk repository (ADR 0007).

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::Chunk;

/// Input for inserting a retrieval chunk under the tenant.
#[derive(Debug, Clone)]
pub struct NewChunk<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub ordinal: i32,
    pub heading_path: &'a [String],
    pub body: &'a str,
    pub body_text_version: &'a str,
    pub chunk_identity_sha256: &'a str,
    pub index_metadata_id: Uuid,
    pub index_signature: &'a str,
}

/// Inserts a chunk row; `org_id` always comes from `ctx`.
pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewChunk<'_>,
) -> Result<Chunk, DbError> {
    let heading_path: Vec<&str> = input.heading_path.iter().map(String::as_str).collect();
    let row = txn
        .query_one(
            "INSERT INTO chunks (
                id, org_id, document_id, version_id, ordinal, heading_path, body,
                body_text_version, chunk_identity_sha256, index_metadata_id,
                index_signature, tsv
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                to_tsvector('simple', $7)
             )
             RETURNING id, org_id, document_id, version_id, ordinal, heading_path, body,
                       body_text_version, chunk_identity_sha256, index_metadata_id,
                       index_signature, page, slide, sheet, span_start, span_end,
                       tsv::text AS tsv, created_at",
            &[
                &input.id,
                &ctx.org_id(),
                &input.document_id,
                &input.version_id,
                &input.ordinal,
                &heading_path,
                &input.body,
                &input.body_text_version,
                &input.chunk_identity_sha256,
                &input.index_metadata_id,
                &input.index_signature,
            ],
        )
        .await?;
    map_chunk(&row)
}

/// Idempotently inserts a chunk row by canonical chunk identity.
pub async fn insert_if_absent(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewChunk<'_>,
) -> Result<Chunk, DbError> {
    let heading_path: Vec<&str> = input.heading_path.iter().map(String::as_str).collect();
    let row = txn
        .query_one(
            "WITH inserted AS (
                INSERT INTO chunks (
                    id, org_id, document_id, version_id, ordinal, heading_path, body,
                    body_text_version, chunk_identity_sha256, index_metadata_id,
                    index_signature
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11
                )
                ON CONFLICT (chunk_identity_sha256) DO NOTHING
                RETURNING id, org_id, document_id, version_id, ordinal, heading_path, body,
                          body_text_version, chunk_identity_sha256, index_metadata_id,
                          index_signature, page, slide, sheet, span_start, span_end,
                          tsv::text AS tsv, created_at
             )
             SELECT id, org_id, document_id, version_id, ordinal, heading_path, body,
                    body_text_version, chunk_identity_sha256, index_metadata_id,
                    index_signature, page, slide, sheet, span_start, span_end,
                    tsv, created_at
             FROM inserted
             UNION ALL
             SELECT id, org_id, document_id, version_id, ordinal, heading_path, body,
                    body_text_version, chunk_identity_sha256, index_metadata_id,
                    index_signature, page, slide, sheet, span_start, span_end,
                    tsv::text AS tsv, created_at
             FROM chunks
             WHERE org_id = $2 AND chunk_identity_sha256 = $9
             LIMIT 1",
            &[
                &input.id,
                &ctx.org_id(),
                &input.document_id,
                &input.version_id,
                &input.ordinal,
                &heading_path,
                &input.body,
                &input.body_text_version,
                &input.chunk_identity_sha256,
                &input.index_metadata_id,
                &input.index_signature,
            ],
        )
        .await?;
    map_chunk(&row)
}

/// Lists chunks for a document within the tenant.
pub async fn list_by_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<Chunk>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, document_id, version_id, ordinal, heading_path, body,
                    body_text_version, chunk_identity_sha256, index_metadata_id,
                    index_signature, page, slide, sheet, span_start, span_end,
                    tsv::text AS tsv, created_at
             FROM chunks
             WHERE org_id = $1 AND document_id = $2
             ORDER BY ordinal",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_chunk).collect()
}

/// Counts chunks visible under the tenant (cross-org denial evidence).
pub async fn count(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT count(*)::bigint FROM chunks WHERE org_id = $1",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn count_by_version(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    version_id: Uuid,
) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT count(*)::bigint FROM chunks WHERE org_id = $1 AND version_id = $2",
            &[&ctx.org_id(), &version_id],
        )
        .await?;
    Ok(row.get(0))
}

fn map_chunk(row: &Row) -> Result<Chunk, DbError> {
    Ok(Chunk {
        id: row.get("id"),
        org_id: row.get("org_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        ordinal: row.get("ordinal"),
        heading_path: row.get("heading_path"),
        body: row.get("body"),
        body_text_version: row.get("body_text_version"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        index_metadata_id: row.get("index_metadata_id"),
        index_signature: row.get("index_signature"),
        page: row.get("page"),
        slide: row.get("slide"),
        sheet: row.get("sheet"),
        span_start: row.get("span_start"),
        span_end: row.get("span_end"),
        tsv: row.get("tsv"),
        created_at: row.get("created_at"),
    })
}
