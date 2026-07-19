//! Tenant-scoped document repository (ADR 0007).

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{ArtifactKind, Document, DocumentState};

/// Input for creating a document in `uploaded` state.
#[derive(Debug, Clone)]
pub struct NewDocument<'a> {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub title: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSource {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub original_object_key: String,
    pub content_sha256: String,
    pub byte_size: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewMarkdownArtifact<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub object_key: &'a str,
    pub content_sha256: &'a str,
    pub byte_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownArtifactRecord {
    pub object_key: String,
    pub created: bool,
}

/// Inserts a document owned by the acting user under `ctx.org_id`.
pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewDocument<'_>,
) -> Result<Document, DbError> {
    let state = DocumentState::Uploaded.as_str();
    let row = txn
        .query_one(
            "INSERT INTO documents (
                id, org_id, collection_id, title, state, created_by_user_id
             ) VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, org_id, collection_id, title, state, current_version_id,
                       created_by_user_id, created_at, updated_at, deleted_at",
            &[
                &input.id,
                &ctx.org_id(),
                &input.collection_id,
                &input.title,
                &state,
                &ctx.user_id(),
            ],
        )
        .await?;
    map_document(&row)
}

/// Fetches a document by id within the tenant.
pub async fn get_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Document, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, collection_id, title, state, current_version_id,
                    created_by_user_id, created_at, updated_at, deleted_at
             FROM documents
             WHERE org_id = $1 AND id = $2",
            &[&ctx.org_id(), &document_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_document(&row)
}

/// Lists visible documents in a collection, ordered by stable creation key.
pub async fn list_by_collection(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
    after: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<Document>, DbError> {
    let rows = match after {
        Some((after_created_at, after_id)) => {
            txn.query(
                "SELECT id, org_id, collection_id, title, state, current_version_id,
                        created_by_user_id, created_at, updated_at, deleted_at
                 FROM documents
                 WHERE org_id = $1
                   AND collection_id = $2
                   AND deleted_at IS NULL
                   AND state <> 'purged'
                   AND (created_at, id) > ($3, $4)
                 ORDER BY created_at, id
                 LIMIT $5",
                &[
                    &ctx.org_id(),
                    &collection_id,
                    &after_created_at,
                    &after_id,
                    &limit,
                ],
            )
            .await?
        }
        None => {
            txn.query(
                "SELECT id, org_id, collection_id, title, state, current_version_id,
                        created_by_user_id, created_at, updated_at, deleted_at
                 FROM documents
                 WHERE org_id = $1
                   AND collection_id = $2
                   AND deleted_at IS NULL
                   AND state <> 'purged'
                 ORDER BY created_at, id
                 LIMIT $3",
                &[&ctx.org_id(), &collection_id, &limit],
            )
            .await?
        }
    };
    rows.iter().map(map_document).collect()
}

/// Locks the document row for an atomic state transition (`SELECT … FOR UPDATE`).
///
/// Intended for the document state machine (and tests that exercise lock contention).
/// There is no public API to write an arbitrary state — use
/// [`crate::services::document_state`].
pub async fn get_by_id_for_update(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Document, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, collection_id, title, state, current_version_id,
                    created_by_user_id, created_at, updated_at, deleted_at
             FROM documents
             WHERE org_id = $1 AND id = $2
             FOR UPDATE",
            &[&ctx.org_id(), &document_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_document(&row)
}

/// Compare-and-set state write used only by the checked state machine.
///
/// Updates only when `org_id`, `id`, and `expected_state` all match (CAS). Not
/// `pub` — callers must go through [`crate::services::document_state`].
async fn update_state_cas(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    expected_state: DocumentState,
    new_state: DocumentState,
) -> Result<Document, DbError> {
    let expected = expected_state.as_str();
    let state = new_state.as_str();
    let row = txn
        .query_opt(
            "UPDATE documents
             SET state = $4, updated_at = now()
             WHERE org_id = $1 AND id = $2 AND state = $3
             RETURNING id, org_id, collection_id, title, state, current_version_id,
                       created_by_user_id, created_at, updated_at, deleted_at",
            &[&ctx.org_id(), &document_id, &expected, &state],
        )
        .await?
        .ok_or_else(|| DbError::StaleState {
            expected: expected_state.to_string(),
            observed: "missing_or_changed".to_string(),
        })?;
    map_document(&row)
}

/// Module-private entry used by the state machine (same crate).
pub(crate) async fn cas_state(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    expected_state: DocumentState,
    new_state: DocumentState,
) -> Result<Document, DbError> {
    update_state_cas(txn, ctx, document_id, expected_state, new_state).await
}

/// Sets `deleted_at` exactly once for tombstoned documents.
pub async fn mark_deleted_at(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Document, DbError> {
    let row = txn
        .query_opt(
            "UPDATE documents
             SET deleted_at = COALESCE(deleted_at, clock_timestamp()),
                 updated_at = clock_timestamp()
             WHERE org_id = $1 AND id = $2
             RETURNING id, org_id, collection_id, title, state, current_version_id,
                       created_by_user_id, created_at, updated_at, deleted_at",
            &[&ctx.org_id(), &document_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_document(&row)
}

/// Counts documents for the tenant (used by cross-org denial tests).
pub async fn count(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "SELECT count(*)::bigint FROM documents WHERE org_id = $1",
            &[&ctx.org_id()],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn get_version_source_for_convert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<VersionSource, DbError> {
    let row = txn
        .query_opt(
            "SELECT document_id, id, original_object_key, content_sha256, byte_size
             FROM document_versions
             WHERE org_id = $1 AND document_id = $2 AND id = $3",
            &[&ctx.org_id(), &document_id, &version_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(VersionSource {
        document_id: row.get("document_id"),
        version_id: row.get("id"),
        original_object_key: row.get("original_object_key"),
        content_sha256: row.get("content_sha256"),
        byte_size: row.get("byte_size"),
    })
}

pub async fn insert_markdown_artifact(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewMarkdownArtifact<'_>,
) -> Result<MarkdownArtifactRecord, DbError> {
    let kind = ArtifactKind::Markdown.as_str();
    let content_type = "text/markdown; charset=utf-8";
    let row = txn
        .query_one(
            "WITH inserted AS (
                INSERT INTO derived_artifacts (
                    id, org_id, document_id, version_id, artifact_kind, object_key,
                    content_sha256, content_type, byte_size
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (version_id, artifact_kind) DO NOTHING
                RETURNING object_key, true AS created
             )
             SELECT object_key, created FROM inserted
             UNION ALL
             SELECT object_key, false AS created
             FROM derived_artifacts
             WHERE org_id = $2 AND version_id = $4 AND artifact_kind = $5
             LIMIT 1",
            &[
                &input.id,
                &ctx.org_id(),
                &input.document_id,
                &input.version_id,
                &kind,
                &input.object_key,
                &input.content_sha256,
                &content_type,
                &input.byte_size,
            ],
        )
        .await?;
    Ok(MarkdownArtifactRecord {
        object_key: row.get("object_key"),
        created: row.get("created"),
    })
}

pub(crate) fn map_document(row: &Row) -> Result<Document, DbError> {
    let state: String = row.get("state");
    Ok(Document {
        id: row.get("id"),
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        title: row.get("title"),
        state: DocumentState::parse(&state).map_err(DbError::Config)?,
        current_version_id: row.get("current_version_id"),
        created_by_user_id: row.get("created_by_user_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        deleted_at: row.get("deleted_at"),
    })
}
