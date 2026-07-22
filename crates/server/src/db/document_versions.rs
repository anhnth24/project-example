//! Tenant-scoped immutable document-version promotion repository.

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{
    ArtifactKind, DerivedArtifact, Document, DocumentState, DocumentVersion, PublicationState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionSourceVersion {
    pub document_id: Uuid,
    pub source_version_id: Uuid,
    pub original_object_key: String,
    pub content_sha256: String,
    pub source_filename: Option<String>,
    pub source_content_type: Option<String>,
    pub byte_size: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewPublishedVersion<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub parent_version_id: Uuid,
    pub content_sha256: &'a str,
    pub original_object_key: &'a str,
    pub markdown_object_key: &'a str,
    pub source_filename: Option<&'a str>,
    pub source_content_type: Option<&'a str>,
    pub byte_size: i64,
    pub change_summary: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewDerivedArtifact<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub kind: ArtifactKind,
    pub object_key: &'a str,
    pub content_sha256: &'a str,
    pub content_type: &'a str,
    pub byte_size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInsertOutcome {
    pub id: Uuid,
    pub created: bool,
    pub object_key: String,
    pub content_sha256: String,
    pub byte_size: Option<i64>,
}

pub async fn source_for_conversion(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<ConversionSourceVersion, DbError> {
    let row = txn
        .query_opt(
            "SELECT document_id, id, original_object_key, content_sha256,
                    source_filename, source_content_type, byte_size
             FROM document_versions
             WHERE org_id = $1 AND document_id = $2 AND id = $3",
            &[&ctx.org_id(), &document_id, &version_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(ConversionSourceVersion {
        document_id: row.get("document_id"),
        source_version_id: row.get("id"),
        original_object_key: row.get("original_object_key"),
        content_sha256: row.get("content_sha256"),
        source_filename: row.get("source_filename"),
        source_content_type: row.get("source_content_type"),
        byte_size: row.get("byte_size"),
    })
}

pub async fn find_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<Option<DocumentVersion>, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    effective_from, effective_to, change_summary, created_by_user_id, created_at
             FROM document_versions
             WHERE org_id = $1 AND document_id = $2 AND id = $3",
            &[&ctx.org_id(), &document_id, &version_id],
        )
        .await?;
    row.map(|row| map_version(&row)).transpose()
}

pub async fn insert_published_version_if_absent(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewPublishedVersion<'_>,
) -> Result<(DocumentVersion, bool), DbError> {
    let publication_state = "published";
    let row = txn
        .query_opt(
            "WITH next_number AS (
                SELECT COALESCE(MAX(version_number), 0)::integer + 1 AS version_number
                FROM document_versions
                WHERE org_id = $2 AND document_id = $3
             ),
             inserted AS (
                INSERT INTO document_versions (
                    id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    change_summary, created_by_user_id
                )
                SELECT $1, $2, $3, next_number.version_number, $4, $5, false,
                       $6, $7, $8, $9, $10, $11, $12, $13
                FROM next_number
                ON CONFLICT (id) DO NOTHING
                RETURNING id, org_id, document_id, version_number, parent_version_id,
                          publication_state, is_current, content_sha256, original_object_key,
                          markdown_object_key, source_filename, source_content_type, byte_size,
                          effective_from, effective_to, change_summary, created_by_user_id,
                          created_at, true AS created
             )
             SELECT id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    effective_from, effective_to, change_summary, created_by_user_id,
                    created_at, created
             FROM inserted
             UNION ALL
             SELECT id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    effective_from, effective_to, change_summary, created_by_user_id,
                    created_at, false AS created
             FROM document_versions
             WHERE org_id = $2 AND document_id = $3 AND id = $1
             LIMIT 1",
            &[
                &input.id,
                &ctx.org_id(),
                &input.document_id,
                &input.parent_version_id,
                &publication_state,
                &input.content_sha256,
                &input.original_object_key,
                &input.markdown_object_key,
                &input.source_filename,
                &input.source_content_type,
                &Some(input.byte_size),
                &input.change_summary,
                &ctx.user_id(),
            ],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    let version = map_version(&row)?;
    Ok((version, row.get("created")))
}

pub async fn insert_artifact_if_absent(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewDerivedArtifact<'_>,
) -> Result<ArtifactInsertOutcome, DbError> {
    let kind = input.kind.as_str();
    let row = txn
        .query_one(
            "WITH inserted AS (
                INSERT INTO derived_artifacts (
                    id, org_id, document_id, version_id, artifact_kind, object_key,
                    content_sha256, content_type, byte_size
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (version_id, artifact_kind) DO NOTHING
                RETURNING id, object_key, content_sha256, byte_size, true AS created
             )
             SELECT id, object_key, content_sha256, byte_size, created FROM inserted
             UNION ALL
             SELECT id, object_key, content_sha256, byte_size, false AS created
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
                &input.content_type,
                &Some(input.byte_size),
            ],
        )
        .await?;
    Ok(ArtifactInsertOutcome {
        id: row.get("id"),
        object_key: row.get("object_key"),
        content_sha256: row.get("content_sha256"),
        byte_size: row.get("byte_size"),
        created: row.get("created"),
    })
}

pub async fn find_markdown_artifact(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    version_id: Uuid,
) -> Result<Option<ArtifactInsertOutcome>, DbError> {
    let kind = ArtifactKind::Markdown.as_str();
    let row = txn
        .query_opt(
            "SELECT id, object_key, content_sha256, byte_size, false AS created
             FROM derived_artifacts
             WHERE org_id = $1 AND version_id = $2 AND artifact_kind = $3",
            &[&ctx.org_id(), &version_id, &kind],
        )
        .await?;
    row.map(|row| {
        Ok(ArtifactInsertOutcome {
            id: row.get("id"),
            object_key: row.get("object_key"),
            content_sha256: row.get("content_sha256"),
            byte_size: row.get("byte_size"),
            created: row.get("created"),
        })
    })
    .transpose()
}

/// Lists distinct object keys recorded for every version/artifact of a document.
///
/// These rows are immutable inventory; purge deletes only the objects they name.
pub async fn list_object_keys_by_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<String>, DbError> {
    let rows = txn
        .query(
            "SELECT key
             FROM (
                SELECT original_object_key AS key
                FROM document_versions
                WHERE org_id = $1 AND document_id = $2
                UNION
                SELECT markdown_object_key AS key
                FROM document_versions
                WHERE org_id = $1 AND document_id = $2 AND markdown_object_key IS NOT NULL
                UNION
                SELECT object_key AS key
                FROM derived_artifacts
                WHERE org_id = $1 AND document_id = $2
             ) keys
             WHERE key IS NOT NULL
             ORDER BY key",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get("key")).collect())
}

/// Lists derived artifact inventory rows for reconcile identity validation.
pub async fn list_artifacts_by_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<DerivedArtifact>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, document_id, version_id, artifact_kind, object_key,
                    content_sha256, content_type, byte_size, created_at
             FROM derived_artifacts
             WHERE org_id = $1 AND document_id = $2
             ORDER BY version_id, artifact_kind, id",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_artifact).collect()
}

fn map_artifact(row: &Row) -> Result<DerivedArtifact, DbError> {
    let kind = ArtifactKind::parse(row.get("artifact_kind")).map_err(DbError::Config)?;
    Ok(DerivedArtifact {
        id: row.get("id"),
        org_id: row.get("org_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        artifact_kind: kind,
        object_key: row.get("object_key"),
        content_sha256: row.get("content_sha256"),
        content_type: row.get("content_type"),
        byte_size: row.get("byte_size"),
        created_at: row.get("created_at"),
    })
}

pub async fn object_key_is_referenced(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    object_key: &str,
) -> Result<bool, DbError> {
    let row = txn
        .query_one(
            "SELECT EXISTS (
                SELECT 1
                FROM document_versions
                WHERE org_id = $1
                  AND (
                    original_object_key = $2
                    OR markdown_object_key = $2
                  )
                UNION ALL
                SELECT 1
                FROM derived_artifacts
                WHERE org_id = $1 AND object_key = $2
             )",
            &[&ctx.org_id(), &object_key],
        )
        .await?;
    Ok(row.get(0))
}

pub async fn list_by_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Vec<DocumentVersion>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    effective_from, effective_to, change_summary, created_by_user_id, created_at
             FROM document_versions
             WHERE org_id = $1 AND document_id = $2
             ORDER BY version_number, id",
            &[&ctx.org_id(), &document_id],
        )
        .await?;
    rows.iter().map(map_version).collect()
}

/// Keyset page of immutable versions for one document.
pub async fn list_page_by_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    limit: i64,
    after_version_number: Option<i32>,
    after_id: Option<Uuid>,
) -> Result<Vec<DocumentVersion>, DbError> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT id, org_id, document_id, version_number, parent_version_id,
                    publication_state, is_current, content_sha256, original_object_key,
                    markdown_object_key, source_filename, source_content_type, byte_size,
                    effective_from, effective_to, change_summary, created_by_user_id, created_at
             FROM document_versions
             WHERE org_id = $1
               AND document_id = $2
               AND (
                 $3::integer IS NULL
                 OR (version_number, id) > ($3::integer, $4::uuid)
               )
             ORDER BY version_number, id
             LIMIT $5",
            &[
                &ctx.org_id(),
                &document_id,
                &after_version_number,
                &after_id,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_version).collect()
}

/// Atomically publishes `version_id` as the document's current pointer.
///
/// Requires a published version, non-deleted document in a publishable state
/// (`converted` or `indexed`). Idempotent when the version is already current.
/// Callers that need indexing coordination must enqueue in the same transaction.
/// Publish a draft (or reaffirm current) via authoritative
/// `markhand_publish_document_version`, then return the updated document row.
///
/// Semantics match the SQL helper: draft→published+current; already-current is
/// idempotent; published-but-not-current (superseded) is rejected.
pub async fn publish_current_version(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<Document, DbError> {
    let document = crate::db::documents::get_by_id_for_update(txn, ctx, document_id).await?;
    if document.deleted_at.is_some()
        || matches!(
            document.state,
            DocumentState::Tombstoned | DocumentState::Purged
        )
    {
        return Err(DbError::NotFound);
    }
    if !matches!(
        document.state,
        DocumentState::Converted
            | DocumentState::Indexed
            | DocumentState::Uploaded
            | DocumentState::Converting
    ) {
        return Err(DbError::StaleState {
            expected: "converted, indexed, or conversion-eligible".into(),
            observed: document.state.to_string(),
        });
    }

    if let Err(error) = txn
        .query_one(
            "SELECT markhand_publish_document_version($1, $2, $3)",
            &[&ctx.org_id(), &document_id, &version_id],
        )
        .await
    {
        return Err(map_publish_sql_error(error));
    }

    crate::db::documents::get_by_id(txn, ctx, document_id).await
}

fn map_publish_sql_error(error: tokio_postgres::Error) -> DbError {
    let db = error.as_db_error();
    let code = db.map(|err| err.code().code());
    let message = db.map(|err| err.message()).unwrap_or("");
    if code == Some("P0002") || message.contains("not found for document") {
        return DbError::NotFound;
    }
    if message.contains("already published and not current") {
        return DbError::Config("version_superseded".into());
    }
    if message.contains("future-dated")
        || message.contains("effective_from")
        || code == Some("23000")
        || code == Some("23514")
    {
        return DbError::Config("invalid_publish".into());
    }
    DbError::Query(error)
}

pub async fn promote_current_if_needed(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document: &Document,
    version_id: Uuid,
) -> Result<(), DbError> {
    let row = txn
        .query_one(
            "SELECT is_current
             FROM document_versions
             WHERE org_id = $1 AND document_id = $2 AND id = $3
             FOR UPDATE",
            &[&ctx.org_id(), &document.id, &version_id],
        )
        .await?;
    let already_current: bool = row.get("is_current");
    if already_current {
        return if document.current_version_id == Some(version_id) {
            Ok(())
        } else {
            Err(DbError::StaleState {
                expected: version_id.to_string(),
                observed: document
                    .current_version_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "none".into()),
            })
        };
    }
    if !is_eligible_for_conversion_promotion(document) {
        return Err(DbError::StaleState {
            expected: "uploaded, converting, or converted and not deleted".into(),
            observed: document.state.to_string(),
        });
    }

    let effective_to = txn
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .get::<_, chrono::DateTime<chrono::Utc>>(0);
    txn.execute(
        "UPDATE document_versions
         SET is_current = false, effective_to = $3
         WHERE org_id = $1
           AND document_id = $2
           AND is_current
           AND effective_to IS NULL",
        &[&ctx.org_id(), &document.id, &effective_to],
    )
    .await?;
    txn.execute(
        "UPDATE document_versions
         SET is_current = true
         WHERE org_id = $1 AND document_id = $2 AND id = $3",
        &[&ctx.org_id(), &document.id, &version_id],
    )
    .await?;
    let expected_state = document.state.as_str();
    let updated = txn
        .execute(
            "UPDATE documents
         SET current_version_id = $3, state = 'converted', updated_at = clock_timestamp()
         WHERE org_id = $1
           AND id = $2
           AND state = $4
           AND deleted_at IS NULL",
            &[&ctx.org_id(), &document.id, &version_id, &expected_state],
        )
        .await?;
    if updated != 1 {
        return Err(DbError::StaleState {
            expected: expected_state.into(),
            observed: "missing_or_changed".into(),
        });
    }
    Ok(())
}

fn is_eligible_for_conversion_promotion(document: &Document) -> bool {
    document.deleted_at.is_none()
        && matches!(
            document.state,
            DocumentState::Uploaded | DocumentState::Converting | DocumentState::Converted
        )
}

fn map_version(row: &Row) -> Result<DocumentVersion, DbError> {
    let publication_state: String = row.get("publication_state");
    let publication_state = match publication_state.as_str() {
        "draft" => PublicationState::Draft,
        "published" => PublicationState::Published,
        other => {
            return Err(DbError::Config(format!(
                "unknown publication state: {other}"
            )));
        }
    };
    Ok(DocumentVersion {
        id: row.get("id"),
        org_id: row.get("org_id"),
        document_id: row.get("document_id"),
        version_number: row.get("version_number"),
        parent_version_id: row.get("parent_version_id"),
        publication_state,
        is_current: row.get("is_current"),
        content_sha256: row.get("content_sha256"),
        original_object_key: row.get("original_object_key"),
        markdown_object_key: row.get("markdown_object_key"),
        source_filename: row.get("source_filename"),
        source_content_type: row.get("source_content_type"),
        byte_size: row.get("byte_size"),
        effective_from: row.get("effective_from"),
        effective_to: row.get("effective_to"),
        change_summary: row.get("change_summary"),
        created_by_user_id: row.get("created_by_user_id"),
        created_at: row.get("created_at"),
    })
}
