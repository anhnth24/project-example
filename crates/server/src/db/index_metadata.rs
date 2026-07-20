//! Tenant-scoped index metadata generation repository.

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{EmbeddingRuntimePath, IndexGenerationState, IndexMetadata};

const INDEX_METADATA_COLUMNS: &str = "id, org_id, collection_id, index_signature_sha256, \
    identity_version, chunking_version, body_text_version, query_normalization_version, \
    embedding_family, embedding_revision, dimensions, normalized, runtime_path, generation, \
    is_active, state, created_at";

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
        if let Some(staged) = find_generation_by_signature_for_update(
            txn,
            ctx,
            input.collection_id,
            input.signature_sha256,
        )
        .await?
        {
            return Ok(staged);
        }
        return insert_generation(txn, ctx, input, IndexGenerationState::Building).await;
    }

    if let Some(staged) = find_generation_by_signature_for_update(
        txn,
        ctx,
        input.collection_id,
        input.signature_sha256,
    )
    .await?
    {
        return Ok(staged);
    }

    insert_generation(txn, ctx, input, IndexGenerationState::Active).await
}

async fn insert_generation(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: EnsureGeneration<'_>,
    state: IndexGenerationState,
) -> Result<IndexMetadata, DbError> {
    let runtime_path = input.runtime_path.as_str();
    let is_active = state == IndexGenerationState::Active;
    let row = txn
        .query_opt(
            &format!(
                "WITH next_generation AS (
                    SELECT COALESCE(MAX(generation), 0)::integer + 1 AS generation
                    FROM index_metadata
                    WHERE org_id = $1
                      AND collection_id IS NOT DISTINCT FROM $2
                 )
                 INSERT INTO index_metadata (
                    org_id, collection_id, index_signature_sha256, chunking_version,
                    body_text_version, query_normalization_version, embedding_family,
                    embedding_revision, dimensions, normalized, runtime_path, generation,
                    is_active, state
                 )
                 SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                        next_generation.generation, $12, $13
                 FROM next_generation
                 ON CONFLICT DO NOTHING
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
                &is_active,
                &state.as_str(),
            ],
        )
        .await?;
    if let Some(row) = row {
        return map_index_metadata(&row);
    }

    find_generation_by_signature_for_update(txn, ctx, input.collection_id, input.signature_sha256)
        .await?
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

pub async fn find_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<Option<IndexMetadata>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {INDEX_METADATA_COLUMNS}
                 FROM index_metadata
                 WHERE org_id = $1 AND id = $2"
            ),
            &[&ctx.org_id(), &metadata_id],
        )
        .await?;
    row.map(|row| map_index_metadata(&row)).transpose()
}

async fn find_generation_by_signature_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Option<Uuid>,
    signature_sha256: &str,
) -> Result<Option<IndexMetadata>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {INDEX_METADATA_COLUMNS}
                 FROM index_metadata
                 WHERE org_id = $1
                   AND collection_id IS NOT DISTINCT FROM $2
                   AND index_signature_sha256 = $3
                   AND state IN ('building', 'shadow', 'active')
                 ORDER BY generation DESC
                 LIMIT 1
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &collection_id, &signature_sha256],
        )
        .await?;
    row.map(|row| map_index_metadata(&row)).transpose()
}

/// Marks a fully rebuilt generation as shadow. It stays invisible to readers
/// until [`cut_over_shadow_generation`] is called after verification.
pub async fn mark_shadow(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<IndexMetadata, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE index_metadata
                 SET state = 'shadow'
                 WHERE org_id = $1 AND id = $2 AND state = 'building'
                 RETURNING {INDEX_METADATA_COLUMNS}"
            ),
            &[&ctx.org_id(), &metadata_id],
        )
        .await?;
    if let Some(row) = row {
        return map_index_metadata(&row);
    }
    find_by_id(txn, ctx, metadata_id)
        .await?
        .filter(|metadata| metadata.state == IndexGenerationState::Shadow)
        .ok_or(DbError::NotFound)
}

/// Atomically flips the collection's active pointer from its current generation
/// to a shadow generation. Callers must perform shadow verification first.
pub async fn cut_over_shadow_generation(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<IndexMetadata, DbError> {
    let initial = find_by_id(txn, ctx, metadata_id)
        .await?
        .ok_or(DbError::NotFound)?;
    lock_generation_scope(txn, ctx, initial.collection_id).await?;
    let candidate = find_by_id_for_update(txn, ctx, metadata_id)
        .await?
        .ok_or(DbError::NotFound)?;
    if candidate.state != IndexGenerationState::Shadow {
        return Err(DbError::Config(
            "only a shadow generation can be cut over".into(),
        ));
    }
    txn.execute(
        "UPDATE index_metadata
         SET is_active = false, state = 'draining'
         WHERE org_id = $1
           AND collection_id IS NOT DISTINCT FROM $2
           AND is_active",
        &[&ctx.org_id(), &candidate.collection_id],
    )
    .await?;
    let row = txn
        .query_opt(
            &format!(
                "UPDATE index_metadata
                 SET is_active = true, state = 'active'
                 WHERE org_id = $1 AND id = $2 AND state = 'shadow'
                 RETURNING {INDEX_METADATA_COLUMNS}"
            ),
            &[&ctx.org_id(), &metadata_id],
        )
        .await?;
    row.map(|row| map_index_metadata(&row))
        .transpose()?
        .ok_or(DbError::NotFound)
}

async fn find_by_id_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    metadata_id: Uuid,
) -> Result<Option<IndexMetadata>, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {INDEX_METADATA_COLUMNS}
                 FROM index_metadata
                 WHERE org_id = $1 AND id = $2
                 FOR UPDATE"
            ),
            &[&ctx.org_id(), &metadata_id],
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
        state: IndexGenerationState::parse(&row.get::<_, String>("state"))
            .map_err(DbError::Config)?,
        created_at: row.get("created_at"),
    })
}
