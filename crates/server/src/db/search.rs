//! Tenant-scoped retrieval search queries.

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchVersionMode {
    Current,
    AsOf(DateTime<Utc>),
    History {
        document_id: Uuid,
    },
    Compare {
        document_id: Uuid,
        version_ids: Vec<Uuid>,
    },
}

impl SearchVersionMode {
    pub fn sql_discriminator(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::AsOf(_) => "as_of",
            Self::History { .. } => "history",
            Self::Compare { .. } => "compare",
        }
    }

    fn as_of_or_now(&self) -> DateTime<Utc> {
        match self {
            Self::AsOf(timestamp) => *timestamp,
            _ => Utc::now(),
        }
    }

    fn document_id_or_nil(&self) -> Uuid {
        match self {
            Self::History { document_id } | Self::Compare { document_id, .. } => *document_id,
            _ => Uuid::nil(),
        }
    }

    fn version_ids_or_empty(&self) -> Vec<Uuid> {
        match self {
            Self::Compare { version_ids, .. } => version_ids.clone(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexicalCandidate {
    pub chunk_id: Uuid,
    pub chunk_identity: String,
    pub lexical_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HydratedChunk {
    pub chunk_id: Uuid,
    pub chunk_identity: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading_path: Vec<String>,
    pub body: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
}

pub async fn lexical_candidates(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    authorized_collection_ids: &[Uuid],
    query: &str,
    mode: &SearchVersionMode,
    limit: i64,
    index_signature: &str,
) -> Result<Vec<LexicalCandidate>, DbError> {
    let mode_name = mode.sql_discriminator();
    let as_of = mode.as_of_or_now();
    let document_id = mode.document_id_or_nil();
    let version_ids = mode.version_ids_or_empty();
    let rows = txn
        .query(
            "WITH query AS (
                SELECT websearch_to_tsquery('simple', $3) AS tsq
             )
             SELECT c.id AS chunk_id,
                    c.chunk_identity_sha256 AS chunk_identity,
                    ts_rank(c.tsv, query.tsq)::real AS lexical_score
             FROM chunks c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions v
               ON v.org_id = c.org_id
              AND v.document_id = c.document_id
              AND v.id = c.version_id
             CROSS JOIN query
             WHERE c.org_id = $1
               AND d.collection_id = ANY($2::uuid[])
               AND d.state = 'indexed'
               AND d.deleted_at IS NULL
               AND c.index_signature = $9
               AND c.tsv @@ query.tsq
               AND (
                    ($5 = 'current' AND v.is_current)
                 OR ($5 = 'as_of'
                     AND v.effective_from <= $6
                     AND (v.effective_to IS NULL OR v.effective_to > $6))
                 OR ($5 = 'history' AND d.id = $7)
                 OR ($5 = 'compare' AND d.id = $7 AND v.id = ANY($8::uuid[]))
               )
             ORDER BY lexical_score DESC, c.id ASC
             LIMIT $4",
            &[
                &ctx.org_id(),
                &authorized_collection_ids,
                &query,
                &limit,
                &mode_name,
                &as_of,
                &document_id,
                &version_ids,
                &index_signature,
            ],
        )
        .await?;
    rows.iter().map(map_lexical_candidate).collect()
}

pub async fn hydrate_chunks(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    authorized_collection_ids: &[Uuid],
    chunk_identities: &[String],
    mode: &SearchVersionMode,
    index_signature: &str,
) -> Result<Vec<HydratedChunk>, DbError> {
    if chunk_identities.is_empty() {
        return Ok(Vec::new());
    }
    let mode_name = mode.sql_discriminator();
    let as_of = mode.as_of_or_now();
    let document_id = mode.document_id_or_nil();
    let version_ids = mode.version_ids_or_empty();
    let rows = txn
        .query(
            "SELECT c.id AS chunk_id,
                    c.chunk_identity_sha256 AS chunk_identity,
                    c.document_id,
                    c.version_id,
                    d.collection_id,
                    v.version_number,
                    v.content_sha256,
                    c.heading_path,
                    c.body,
                    c.page,
                    c.slide,
                    c.sheet,
                    c.span_start,
                    c.span_end,
                    v.is_current,
                    v.effective_from,
                    v.effective_to
             FROM chunks c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions v
               ON v.org_id = c.org_id
              AND v.document_id = c.document_id
              AND v.id = c.version_id
             WHERE c.org_id = $1
               AND d.collection_id = ANY($2::uuid[])
               AND c.chunk_identity_sha256 = ANY($3::text[])
               AND d.state = 'indexed'
               AND d.deleted_at IS NULL
               AND c.index_signature = $4
               AND (
                    ($5 = 'current' AND v.is_current)
                 OR ($5 = 'as_of'
                     AND v.effective_from <= $6
                     AND (v.effective_to IS NULL OR v.effective_to > $6))
                 OR ($5 = 'history' AND d.id = $7)
                 OR ($5 = 'compare' AND d.id = $7 AND v.id = ANY($8::uuid[]))
               )
             ORDER BY c.chunk_identity_sha256 ASC",
            &[
                &ctx.org_id(),
                &authorized_collection_ids,
                &chunk_identities,
                &index_signature,
                &mode_name,
                &as_of,
                &document_id,
                &version_ids,
            ],
        )
        .await?;
    rows.iter().map(map_hydrated_chunk).collect()
}

fn map_lexical_candidate(row: &Row) -> Result<LexicalCandidate, DbError> {
    Ok(LexicalCandidate {
        chunk_id: row.get("chunk_id"),
        chunk_identity: row.get("chunk_identity"),
        lexical_score: row.get("lexical_score"),
    })
}

fn map_hydrated_chunk(row: &Row) -> Result<HydratedChunk, DbError> {
    Ok(HydratedChunk {
        chunk_id: row.get("chunk_id"),
        chunk_identity: row.get("chunk_identity"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        collection_id: row.get("collection_id"),
        version_number: row.get("version_number"),
        content_sha256: row.get("content_sha256"),
        heading_path: row.get("heading_path"),
        body: row.get("body"),
        page: row.get("page"),
        slide: row.get("slide"),
        sheet: row.get("sheet"),
        span_start: row.get("span_start"),
        span_end: row.get("span_end"),
        is_current: row.get("is_current"),
        effective_from: row.get("effective_from"),
        effective_to: row.get("effective_to"),
    })
}

#[cfg(test)]
mod tests {
    use super::SearchVersionMode;
    use uuid::Uuid;

    #[test]
    fn version_mode_maps_to_sql_discriminator() {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        assert_eq!(SearchVersionMode::Current.sql_discriminator(), "current");
        assert_eq!(
            SearchVersionMode::AsOf(chrono::Utc::now()).sql_discriminator(),
            "as_of"
        );
        assert_eq!(
            SearchVersionMode::History { document_id }.sql_discriminator(),
            "history"
        );
        assert_eq!(
            SearchVersionMode::Compare {
                document_id,
                version_ids: vec![version_id],
            }
            .sql_discriminator(),
            "compare"
        );
    }
}
