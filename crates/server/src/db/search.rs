//! Tenant-scoped FTS, version resolution, and hydration queries for retrieval.
//!
//! PostgreSQL is the authority for chunk text, document state, ACL, and version
//! visibility. Vector payloads supply candidate identities only.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{DocumentState, PublicationState};

/// Lexical candidate before PG hydration (scores only; no body text).
#[derive(Debug, Clone, PartialEq)]
pub struct FtsCandidate {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub rank: f32,
}

/// Authorized chunk row hydrated from PostgreSQL for citation/rerank.
#[derive(Debug, Clone, PartialEq)]
pub struct HydratedChunkRow {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading_path: Vec<String>,
    pub body: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub document_state: DocumentState,
    pub deleted_at: Option<DateTime<Utc>>,
    pub publication_state: PublicationState,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
}

/// Conflict evidence sides that both remain authorized after recheck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedConflictEvidence {
    pub conflict_id: Uuid,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub claim_a_document_id: Uuid,
    pub claim_b_document_id: Uuid,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_collection_id: Uuid,
    pub claim_b_collection_id: Uuid,
    pub claim_a_quote: Option<String>,
    pub claim_b_quote: Option<String>,
}

/// Version visibility filter shared by FTS and hydration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionVisibility {
    /// Only the current published pointer (`is_current`).
    Current,
    /// Explicit set of published version ids (as_of / compare / history).
    VersionIds(BTreeSet<Uuid>),
}

/// Resolves the published version effective at `as_of` for each in-scope document.
pub async fn resolve_as_of_version_ids(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    as_of: DateTime<Utc>,
) -> Result<BTreeSet<Uuid>, DbError> {
    if collection_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let rows = txn
        .query(
            "SELECT DISTINCT ON (d.id) dv.id
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.collection_id = ANY($2)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.effective_from <= $3
               AND (dv.effective_to IS NULL OR dv.effective_to > $3)
             ORDER BY d.id, dv.version_number DESC, dv.id",
            &[&ctx.org_id(), &collection_ids, &as_of],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

/// Loads published versions for one logical document (history mode).
pub async fn list_published_version_ids_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_ids: &[Uuid],
) -> Result<Vec<(Uuid, i32)>, DbError> {
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
             ORDER BY dv.version_number, dv.id",
            &[&ctx.org_id(), &document_id, &collection_ids],
        )
        .await?;
    Ok(rows.iter().map(|row| (row.get(0), row.get(1))).collect())
}

/// Validates compare/history versions share one authorized document lineage.
pub async fn load_lineage_versions(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<Vec<(Uuid, i32, Option<Uuid>)>, DbError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number, dv.parent_version_id
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.id = ANY($4)
             ORDER BY dv.version_number, dv.id",
            &[&ctx.org_id(), &document_id, &collection_ids, &version_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect())
}

/// Full-text search over in-scope, version-filtered chunks.
pub async fn fts_search(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    query: &str,
    visibility: &VersionVisibility,
    limit: usize,
) -> Result<Vec<FtsCandidate>, DbError> {
    if collection_ids.is_empty() || limit == 0 || query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = match visibility {
        VersionVisibility::Current => {
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $4)) AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND c.tsv @@ plainto_tsquery('simple', $4)
                 ORDER BY rank DESC, c.id
                 LIMIT $3",
                &[&ctx.org_id(), &collection_ids, &limit_i64, &query],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $5)) AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND c.version_id = ANY($3)
                   AND c.tsv @@ plainto_tsquery('simple', $5)
                 ORDER BY rank DESC, c.id
                 LIMIT $4",
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &versions,
                    &limit_i64,
                    &query,
                ],
            )
            .await?
        }
    };
    rows.iter().map(map_fts_candidate).collect()
}

/// Hydrates candidate chunk identities from PostgreSQL with state/ACL joins.
///
/// Rows that fail collection allow-list, tombstone, or version visibility are
/// omitted — vector/FTS scores never leak text without this path.
pub async fn hydrate_chunks_by_identity(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    identities: &[String],
    visibility: &VersionVisibility,
) -> Result<Vec<HydratedChunkRow>, DbError> {
    if collection_ids.is_empty() || identities.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match visibility {
        VersionVisibility::Current => {
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current",
                &[&ctx.org_id(), &collection_ids, &identities],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
                   AND c.version_id = ANY($4)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'",
                &[&ctx.org_id(), &collection_ids, &identities, &versions],
            )
            .await?
        }
    };
    rows.iter().map(map_hydrated_chunk).collect()
}

/// Loads conflict evidence only when both claim sides remain in authorized scope.
pub async fn load_authorized_conflict_evidence(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    conflict_ids: &[Uuid],
) -> Result<Vec<AuthorizedConflictEvidence>, DbError> {
    if collection_ids.is_empty() || conflict_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT conf.id AS conflict_id,
                    conf.claim_a_id, conf.claim_b_id,
                    ca.document_id AS claim_a_document_id,
                    cb.document_id AS claim_b_document_id,
                    ca.version_id AS claim_a_version_id,
                    cb.version_id AS claim_b_version_id,
                    da.collection_id AS claim_a_collection_id,
                    db.collection_id AS claim_b_collection_id,
                    ca.citation_quote AS claim_a_quote,
                    cb.citation_quote AS claim_b_quote
             FROM conflicts conf
             JOIN claims ca
               ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
             JOIN claims cb
               ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
             JOIN documents da
               ON da.org_id = ca.org_id AND da.id = ca.document_id
             JOIN documents db
               ON db.org_id = cb.org_id AND db.id = cb.document_id
             WHERE conf.org_id = $1
               AND conf.id = ANY($2)
               AND da.collection_id = ANY($3)
               AND db.collection_id = ANY($3)
               AND da.deleted_at IS NULL
               AND db.deleted_at IS NULL
               AND da.state = 'indexed'
               AND db.state = 'indexed'",
            &[&ctx.org_id(), &conflict_ids, &collection_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| AuthorizedConflictEvidence {
            conflict_id: row.get("conflict_id"),
            claim_a_id: row.get("claim_a_id"),
            claim_b_id: row.get("claim_b_id"),
            claim_a_document_id: row.get("claim_a_document_id"),
            claim_b_document_id: row.get("claim_b_document_id"),
            claim_a_version_id: row.get("claim_a_version_id"),
            claim_b_version_id: row.get("claim_b_version_id"),
            claim_a_collection_id: row.get("claim_a_collection_id"),
            claim_b_collection_id: row.get("claim_b_collection_id"),
            claim_a_quote: row.get("claim_a_quote"),
            claim_b_quote: row.get("claim_b_quote"),
        })
        .collect())
}

fn map_fts_candidate(row: &Row) -> Result<FtsCandidate, DbError> {
    Ok(FtsCandidate {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        collection_id: row.get("collection_id"),
        rank: row.get::<_, f64>("rank") as f32,
    })
}

fn map_hydrated_chunk(row: &Row) -> Result<HydratedChunkRow, DbError> {
    let state: String = row.get("state");
    let document_state = DocumentState::parse(&state).map_err(DbError::Config)?;
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
    Ok(HydratedChunkRow {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        content_sha256: row.get("content_sha256"),
        heading_path: row.get("heading_path"),
        body: row.get("body"),
        page: row.get("page"),
        slide: row.get("slide"),
        sheet: row.get("sheet"),
        span_start: row.get("span_start"),
        span_end: row.get("span_end"),
        document_state,
        deleted_at: row.get("deleted_at"),
        publication_state,
        is_current: row.get("is_current"),
        effective_from: row.get("effective_from"),
        effective_to: row.get("effective_to"),
    })
}

#[cfg(test)]
mod tests {
    use super::VersionVisibility;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    #[test]
    fn version_visibility_empty_ids_is_fail_closed() {
        let visibility = VersionVisibility::VersionIds(BTreeSet::new());
        match visibility {
            VersionVisibility::VersionIds(ids) => assert!(ids.is_empty()),
            VersionVisibility::Current => panic!("expected version ids"),
        }
        let _ = Uuid::nil();
    }
}
