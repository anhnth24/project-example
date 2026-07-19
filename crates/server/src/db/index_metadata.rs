//! Tenant-scoped index metadata generation repository.

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{EmbeddingRuntimePath, IndexMetadata};

const INDEX_METADATA_COLUMNS: &str = "id, org_id, collection_id, index_signature_sha256, \
    identity_version, chunking_version, body_text_version, query_normalization_version, \
    embedding_family, embedding_revision, dimensions, normalized, runtime_path, generation, \
    is_active, created_at";

#[derive(Debug, Clone)]
pub struct EnsureGeneration<'a> {
    pub collection_id: Option<Uuid>,
    pub signature_sha256: &'a str,
    pub chunking_version: &'a str,
    pub body_text_version: &'a str,
    pub query_normalization_version: &'a str,
    pub embedding_family: &'a str,
    pub embedding_revision: &'a str,
    pub dimensions: i32,
    pub normalized: bool,
    pub runtime_path: EmbeddingRuntimePath,
}

pub async fn find_active(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Option<Uuid>,
) -> Result<Option<IndexMetadata>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {INDEX_METADATA_COLUMNS}
                 FROM index_metadata
                 WHERE org_id = $1
                   AND collection_id IS NOT DISTINCT FROM $2
                   AND is_active"
            ),
            &[&ctx.org_id(), &collection_id],
        )
        .await?;
    row.map(|row| map_index_metadata(&row)).transpose()
}

pub async fn ensure_active_generation(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: EnsureGeneration<'_>,
) -> Result<IndexMetadata, DbError> {
    lock_generation_scope(txn, ctx, input.collection_id).await?;
    if let Some(active) = find_active_for_update(txn, ctx, input.collection_id).await? {
        if active.index_signature_sha256 == input.signature_sha256 {
            return Ok(active);
        }
        return Err(DbError::Config(
            "index signature change requires an explicit reindex".into(),
        ));
    }

    let runtime_path = input.runtime_path.as_str();
    let row = txn
        .query_opt(
            &format!(
                "WITH next_generation AS (
                    SELECT COALESCE(MAX(generation), 0)::integer + 1 AS generation
                    FROM index_metadata
                    WHERE org_id = $1
                      AND index_signature_sha256 = $3
                 )
                 INSERT INTO index_metadata (
                    org_id, collection_id, index_signature_sha256, chunking_version,
                    body_text_version, query_normalization_version, embedding_family,
                    embedding_revision, dimensions, normalized, runtime_path, generation,
                    is_active
                 )
                 SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                        next_generation.generation, true
                 FROM next_generation
                 ON CONFLICT (org_id, index_signature_sha256, generation) DO NOTHING
                 RETURNING {INDEX_METADATA_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &input.collection_id,
                &input.signature_sha256,
                &input.chunking_version,
                &input.body_text_version,
                &input.query_normalization_version,
                &input.embedding_family,
                &input.embedding_revision,
                &input.dimensions,
                &input.normalized,
                &runtime_path,
            ],
        )
        .await?;
    if let Some(row) = row {
        return map_index_metadata(&row);
    }
    find_active(txn, ctx, input.collection_id)
        .await?
        .filter(|metadata| metadata.index_signature_sha256 == input.signature_sha256)
        .ok_or(DbError::NotFound)
}

async fn find_active_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Option<Uuid>,
) -> Result<Option<IndexMetadata>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {INDEX_METADATA_COLUMNS}
                 FROM index_metadata
                 WHERE org_id = $1
                   AND collection_id IS NOT DISTINCT FROM $2
                   AND is_active
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &collection_id],
        )
        .await?;
    row.map(|row| map_index_metadata(&row)).transpose()
}

async fn lock_generation_scope(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Option<Uuid>,
) -> Result<(), DbError> {
    let collection = collection_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string());
    let key = format!("index_metadata:{}:{collection}", ctx.org_id());
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&key])
        .await?;
    Ok(())
}

fn map_index_metadata(row: &Row) -> Result<IndexMetadata, DbError> {
    let runtime_path: String = row.get("runtime_path");
    Ok(IndexMetadata {
        id: row.get("id"),
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        index_signature_sha256: row.get("index_signature_sha256"),
        identity_version: row.get("identity_version"),
        chunking_version: row.get("chunking_version"),
        body_text_version: row.get("body_text_version"),
        query_normalization_version: row.get("query_normalization_version"),
        embedding_family: row.get("embedding_family"),
        embedding_revision: row.get("embedding_revision"),
        dimensions: row.get("dimensions"),
        normalized: row.get("normalized"),
        runtime_path: EmbeddingRuntimePath::parse(&runtime_path).map_err(DbError::Config)?,
        generation: row.get("generation"),
        is_active: row.get("is_active"),
        created_at: row.get("created_at"),
    })
}
