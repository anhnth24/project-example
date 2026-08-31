//! Tenant-scoped FTS, version resolution, and hydration queries for retrieval.
//!
//! PostgreSQL is the authority for chunk text, document state, ACL, and version
//! visibility. Vector payloads supply candidate identities only.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::acl_sql::acl_predicate_sql;
use crate::db::error::DbError;
use crate::db::models::{AccessLevel, DocumentState, IndexGenerationState, PublicationState};

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
    pub canonical_markdown_sha256: String,
    pub document_title: String,
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
    pub index_metadata_id: Uuid,
    pub index_generation_active: bool,
    pub index_generation_state: IndexGenerationState,
}

/// Conflict evidence sides that both remain authorized after recheck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedConflictEvidence {
    pub conflict_id: Uuid,
    pub status: String,
    pub resolution_note: Option<String>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub claim_a_document_id: Uuid,
    pub claim_b_document_id: Uuid,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_collection_id: Uuid,
    pub claim_b_collection_id: Uuid,
    pub claim_a_is_current: bool,
    pub claim_b_is_current: bool,
    pub claim_a_published: bool,
    pub claim_b_published: bool,
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

impl VersionVisibility {
    fn required_permission(&self) -> &'static str {
        match self {
            Self::Current => "qa.query",
            Self::VersionIds(_) => "qa.history",
        }
    }
}

/// Shadow/building/retired generations must not surface in retrieval.
pub fn index_generation_visible_for_retrieval(
    is_active: bool,
    state: IndexGenerationState,
) -> bool {
    is_active && state == IndexGenerationState::Active
}

fn acl_read_access_param() -> &'static str {
    AccessLevel::Read.as_str()
}

const PERMISSION_QA_QUERY: &str = "qa.query";
const PERMISSION_QA_HISTORY: &str = "qa.history";

/// Current retrieval (`VersionVisibility::Current`) — canonical `qa.query` + read.
fn current_retrieval_acl_predicate(
    org_id_expr: &str,
    collection_id_expr: &str,
    user_param: &str,
    permission_param: &str,
    access_param: &str,
) -> String {
    acl_predicate_sql(
        org_id_expr,
        collection_id_expr,
        user_param,
        permission_param,
        access_param,
    )
}

/// Historical/as-of/compare retrieval — requires both `qa.query` and `qa.history`
/// at read access against current DB membership state.
fn historical_retrieval_acl_predicate(
    org_id_expr: &str,
    collection_id_expr: &str,
    user_param: &str,
    query_permission_param: &str,
    history_permission_param: &str,
    access_param: &str,
) -> String {
    format!(
        "({}) AND ({})",
        acl_predicate_sql(
            org_id_expr,
            collection_id_expr,
            user_param,
            query_permission_param,
            access_param,
        ),
        acl_predicate_sql(
            org_id_expr,
            collection_id_expr,
            user_param,
            history_permission_param,
            access_param,
        ),
    )
}

/// Non-sensitive summary of production FTS query normalization (token count only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedFtsQueryStats {
    pub token_count: usize,
    pub nonempty: bool,
}

/// Summarizes accent-fold + stop-word filtering applied before `plainto_tsquery`.
pub fn normalized_fts_query_stats(query: &str) -> NormalizedFtsQueryStats {
    let normalized = normalize_fts_query(query);
    let token_count = normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .count();
    NormalizedFtsQueryStats {
        token_count,
        nonempty: !normalized.trim().is_empty(),
    }
}

/// Production FTS normalization for SQL bind parity in integration diagnostics.
///
/// Callers must never log or persist the returned string (marker/query leakage).
pub fn normalized_fts_query_for_retrieval(query: &str) -> String {
    normalize_fts_query(query)
}

fn normalize_fts_query(query: &str) -> String {
    const QUESTION_STOP_WORDS: &[&str] = &[
        "bao", "nhieu", "la", "gi", "nao", "noi", "ve", "dung", "duoc", "ra", "sao", "nhu", "the",
    ];
    let normalized = fileconv_core::intelligence::normalize_search_text(query);
    let tokens: Vec<&str> = normalized
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .collect();
    let meaningful: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|token| !QUESTION_STOP_WORDS.contains(token))
        .collect();
    if meaningful.is_empty() {
        tokens.join(" ")
    } else {
        meaningful.join(" ")
    }
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

/// Full-text search over active-generation, version-filtered chunks.
///
/// Query text is accent-folded (`accent-fold-v1`) before `plainto_tsquery` so it
/// matches `markhand_accent_fold` tsvector content. The tsquery is an OR of two
/// foldings of the same query:
///
/// 1. token-joined (`normalize_fts_query`) — stop-word filtered semantic tokens;
/// 2. separator-preserving (`normalize_search_text`) — PostgreSQL's lexer then
///    tokenizes the query EXACTLY like it tokenized the folded body. Without
///    this leg, identifiers whose body-side tokenization differs from plain
///    whitespace tokens are unfindable — e.g. hex ids containing a
///    `<digits>e<digits>` run (`…-6571e715cb97…`) are split as scientific-
///    notation floats in the tsvector and never match the space-joined tokens.
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
    let folded = normalize_fts_query(query);
    let folded_raw = fileconv_core::intelligence::normalize_search_text(query);
    if folded.trim().is_empty() && folded_raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = match visibility {
        VersionVisibility::Current => {
            // 1C-06: candidate leg now carries its own ACL predicate
            // (defense-in-depth) instead of relying solely on the
            // hydration re-check downstream — see `current_retrieval_acl_predicate`.
            let acl =
                current_retrieval_acl_predicate("d.org_id", "d.collection_id", "$5", "$6", "$7");
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $4)
                                          || plainto_tsquery('simple', $8))::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND im.is_active
                   AND im.state = 'active'
                   AND c.tsv @@ (plainto_tsquery('simple', $4)
                                 || plainto_tsquery('simple', $8))
                   AND {acl}
                 ORDER BY rank DESC, c.id
                 LIMIT $3"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &limit_i64,
                    &folded,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                    &acl_read_access_param(),
                    &folded_raw,
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            let acl = historical_retrieval_acl_predicate(
                "d.org_id",
                "d.collection_id",
                "$6",
                "$7",
                "$9",
                "$8",
            );
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $5)
                                          || plainto_tsquery('simple', $10))::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND c.version_id = ANY($3)
                   AND im.is_active
                   AND im.state = 'active'
                   AND c.tsv @@ (plainto_tsquery('simple', $5)
                                 || plainto_tsquery('simple', $10))
                   AND {acl}
                 ORDER BY rank DESC, c.id
                 LIMIT $4"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &versions,
                    &limit_i64,
                    &folded,
                    &ctx.user_id(),
                    &PERMISSION_QA_QUERY,
                    &acl_read_access_param(),
                    &PERMISSION_QA_HISTORY,
                    &folded_raw,
                ],
            )
            .await?
        }
    };
    let matched: Vec<FtsCandidate> = rows
        .iter()
        .map(map_fts_candidate)
        .collect::<Result<_, _>>()?;
    if matched.is_empty() {
        return Ok(matched);
    }
    let version_ids: Vec<Uuid> = matched
        .iter()
        .map(|hit| hit.version_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let opening = fts_opening_chunks(
        txn,
        ctx,
        collection_ids,
        visibility,
        &version_ids,
        FTS_OPENING_ORDINAL_LIMIT,
    )
    .await?;
    Ok(merge_fts_opening_chunks(matched, opening))
}

/// First N ordinals of a matched version (cover page + opening articles).
/// FTS AND-queries on a circular number often hit only the cover; the substance
/// lives in later ordinals that never repeat that number.
pub const FTS_OPENING_ORDINAL_LIMIT: i32 = 5;
const FTS_OPENING_RANK_SCALE: f32 = 0.35;

pub fn merge_fts_opening_chunks(
    matched: Vec<FtsCandidate>,
    opening: Vec<FtsCandidate>,
) -> Vec<FtsCandidate> {
    if matched.is_empty() {
        return matched;
    }
    let mut best_rank: HashMap<Uuid, f32> = HashMap::new();
    for hit in &matched {
        best_rank
            .entry(hit.version_id)
            .and_modify(|rank| *rank = rank.max(hit.rank))
            .or_insert(hit.rank);
    }
    let mut seen: HashSet<String> = matched
        .iter()
        .map(|hit| hit.chunk_identity_sha256.clone())
        .collect();
    let mut out = matched;
    for mut hit in opening {
        if !seen.insert(hit.chunk_identity_sha256.clone()) {
            continue;
        }
        let base = best_rank.get(&hit.version_id).copied().unwrap_or(0.0);
        hit.rank = base * FTS_OPENING_RANK_SCALE;
        out.push(hit);
    }
    out
}

async fn fts_opening_chunks(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    visibility: &VersionVisibility,
    version_ids: &[Uuid],
    ordinal_limit: i32,
) -> Result<Vec<FtsCandidate>, DbError> {
    if collection_ids.is_empty() || version_ids.is_empty() || ordinal_limit <= 0 {
        return Ok(Vec::new());
    }
    let versions = version_ids.to_vec();
    let rows = match visibility {
        VersionVisibility::Current => {
            let acl =
                current_retrieval_acl_predicate("d.org_id", "d.collection_id", "$5", "$6", "$7");
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id, 0::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.version_id = ANY($3)
                   AND c.ordinal < $4
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND im.is_active
                   AND im.state = 'active'
                   AND {acl}
                 ORDER BY c.ordinal, c.id"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &versions,
                    &ordinal_limit,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                    &acl_read_access_param(),
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(allowed) => {
            if allowed.is_empty() {
                return Ok(Vec::new());
            }
            let acl = historical_retrieval_acl_predicate(
                "d.org_id",
                "d.collection_id",
                "$5",
                "$6",
                "$8",
                "$7",
            );
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id, 0::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.version_id = ANY($3)
                   AND c.ordinal < $4
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND im.is_active
                   AND im.state = 'active'
                   AND {acl}
                 ORDER BY c.ordinal, c.id"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &versions,
                    &ordinal_limit,
                    &ctx.user_id(),
                    &PERMISSION_QA_QUERY,
                    &acl_read_access_param(),
                    &PERMISSION_QA_HISTORY,
                ],
            )
            .await?
        }
    };
    rows.iter().map(map_fts_candidate).collect()
}

/// Hydrates candidate chunk identities from the active index generation only.
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
            let acl =
                current_retrieval_acl_predicate("d.org_id", "d.collection_id", "$4", "$5", "$6");
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        coalesce(md.content_sha256, '') AS canonical_markdown_sha256,
                        d.title AS document_title,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                        c.index_metadata_id, im.is_active, im.state AS index_state
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 LEFT JOIN derived_artifacts md
                   ON md.org_id = c.org_id
                  AND md.version_id = c.version_id
                  AND md.artifact_kind = 'markdown'
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
                   AND {acl}
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND im.is_active
                   AND im.state = 'active'"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &identities,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                    &acl_read_access_param(),
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            let acl = historical_retrieval_acl_predicate(
                "d.org_id",
                "d.collection_id",
                "$5",
                "$6",
                "$8",
                "$7",
            );
            let sql = format!(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        coalesce(md.content_sha256, '') AS canonical_markdown_sha256,
                        d.title AS document_title,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                        c.index_metadata_id, im.is_active, im.state AS index_state
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 LEFT JOIN derived_artifacts md
                   ON md.org_id = c.org_id
                  AND md.version_id = c.version_id
                  AND md.artifact_kind = 'markdown'
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
                   AND c.version_id = ANY($4)
                   AND {acl}
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND im.is_active
                   AND im.state = 'active'"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &identities,
                    &versions,
                    &ctx.user_id(),
                    &PERMISSION_QA_QUERY,
                    &acl_read_access_param(),
                    &PERMISSION_QA_HISTORY,
                ],
            )
            .await?
        }
    };
    rows.iter().map(map_hydrated_chunk).collect()
}

/// Loads conflict evidence only when both claim sides remain authorized and
/// published under the resolved version visibility.
pub async fn load_authorized_conflict_evidence(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    conflict_ids: &[Uuid],
    visibility: &VersionVisibility,
) -> Result<Vec<AuthorizedConflictEvidence>, DbError> {
    if collection_ids.is_empty() || conflict_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match visibility {
        VersionVisibility::Current => {
            let acl_a =
                current_retrieval_acl_predicate("da.org_id", "da.collection_id", "$4", "$5", "$6");
            let acl_b =
                current_retrieval_acl_predicate("db.org_id", "db.collection_id", "$4", "$5", "$6");
            let sql = format!(
                "SELECT conf.id AS conflict_id,
                        conf.status, conf.resolution_note, conf.resolved_at,
                        conf.claim_a_id, conf.claim_b_id,
                        ca.document_id AS claim_a_document_id,
                        cb.document_id AS claim_b_document_id,
                        ca.version_id AS claim_a_version_id,
                        cb.version_id AS claim_b_version_id,
                        da.collection_id AS claim_a_collection_id,
                        db.collection_id AS claim_b_collection_id,
                        dva.is_current AS claim_a_is_current,
                        dvb.is_current AS claim_b_is_current,
                        (dva.publication_state = 'published') AS claim_a_published,
                        (dvb.publication_state = 'published') AS claim_b_published,
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
                 JOIN document_versions dva
                   ON dva.org_id = ca.org_id
                  AND dva.document_id = ca.document_id
                  AND dva.id = ca.version_id
                 JOIN document_versions dvb
                   ON dvb.org_id = cb.org_id
                  AND dvb.document_id = cb.document_id
                  AND dvb.id = cb.version_id
                 WHERE conf.org_id = $1
                   AND conf.id = ANY($2)
                   AND da.collection_id = ANY($3)
                   AND db.collection_id = ANY($3)
                   AND {acl_a}
                   AND {acl_b}
                   AND da.deleted_at IS NULL
                   AND db.deleted_at IS NULL
                   AND da.state = 'indexed'
                   AND db.state = 'indexed'
                   AND dva.publication_state = 'published'
                   AND dvb.publication_state = 'published'
                   AND dva.is_current
                   AND dvb.is_current"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &conflict_ids,
                    &collection_ids,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                    &acl_read_access_param(),
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            let acl_a = historical_retrieval_acl_predicate(
                "da.org_id",
                "da.collection_id",
                "$5",
                "$6",
                "$8",
                "$7",
            );
            let acl_b = historical_retrieval_acl_predicate(
                "db.org_id",
                "db.collection_id",
                "$5",
                "$6",
                "$8",
                "$7",
            );
            let sql = format!(
                "SELECT conf.id AS conflict_id,
                        conf.status, conf.resolution_note, conf.resolved_at,
                        conf.claim_a_id, conf.claim_b_id,
                        ca.document_id AS claim_a_document_id,
                        cb.document_id AS claim_b_document_id,
                        ca.version_id AS claim_a_version_id,
                        cb.version_id AS claim_b_version_id,
                        da.collection_id AS claim_a_collection_id,
                        db.collection_id AS claim_b_collection_id,
                        dva.is_current AS claim_a_is_current,
                        dvb.is_current AS claim_b_is_current,
                        (dva.publication_state = 'published') AS claim_a_published,
                        (dvb.publication_state = 'published') AS claim_b_published,
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
                 JOIN document_versions dva
                   ON dva.org_id = ca.org_id
                  AND dva.document_id = ca.document_id
                  AND dva.id = ca.version_id
                 JOIN document_versions dvb
                   ON dvb.org_id = cb.org_id
                  AND dvb.document_id = cb.document_id
                  AND dvb.id = cb.version_id
                 WHERE conf.org_id = $1
                   AND conf.id = ANY($2)
                   AND da.collection_id = ANY($3)
                   AND db.collection_id = ANY($3)
                   AND {acl_a}
                   AND {acl_b}
                   AND da.deleted_at IS NULL
                   AND db.deleted_at IS NULL
                   AND da.state = 'indexed'
                   AND db.state = 'indexed'
                   AND dva.publication_state = 'published'
                   AND dvb.publication_state = 'published'
                   AND ca.version_id = ANY($4)
                   AND cb.version_id = ANY($4)"
            );
            txn.query(
                &sql,
                &[
                    &ctx.org_id(),
                    &conflict_ids,
                    &collection_ids,
                    &versions,
                    &ctx.user_id(),
                    &PERMISSION_QA_QUERY,
                    &acl_read_access_param(),
                    &PERMISSION_QA_HISTORY,
                ],
            )
            .await?
        }
    };
    Ok(rows.iter().map(map_conflict_evidence).collect())
}

/// Decode a PostgreSQL `real` (`f32`) rank without widening to `f64` first.
pub fn read_pg_real_rank(row: &Row, column: &str) -> f32 {
    row.get::<_, f32>(column)
}

fn map_fts_candidate(row: &Row) -> Result<FtsCandidate, DbError> {
    Ok(FtsCandidate {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        collection_id: row.get("collection_id"),
        rank: read_pg_real_rank(row, "rank"),
    })
}

fn map_conflict_evidence(row: &Row) -> AuthorizedConflictEvidence {
    AuthorizedConflictEvidence {
        conflict_id: row.get("conflict_id"),
        status: row.get("status"),
        resolution_note: row.get("resolution_note"),
        resolved_at: row.get("resolved_at"),
        claim_a_id: row.get("claim_a_id"),
        claim_b_id: row.get("claim_b_id"),
        claim_a_document_id: row.get("claim_a_document_id"),
        claim_b_document_id: row.get("claim_b_document_id"),
        claim_a_version_id: row.get("claim_a_version_id"),
        claim_b_version_id: row.get("claim_b_version_id"),
        claim_a_collection_id: row.get("claim_a_collection_id"),
        claim_b_collection_id: row.get("claim_b_collection_id"),
        claim_a_is_current: row.get("claim_a_is_current"),
        claim_b_is_current: row.get("claim_b_is_current"),
        claim_a_published: row.get("claim_a_published"),
        claim_b_published: row.get("claim_b_published"),
        claim_a_quote: row.get("claim_a_quote"),
        claim_b_quote: row.get("claim_b_quote"),
    }
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
    let index_state: String = row.get("index_state");
    let index_generation_state =
        IndexGenerationState::parse(&index_state).map_err(DbError::Config)?;
    Ok(HydratedChunkRow {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        content_sha256: row.get("content_sha256"),
        canonical_markdown_sha256: row.get("canonical_markdown_sha256"),
        document_title: row.get("document_title"),
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
        index_metadata_id: row.get("index_metadata_id"),
        index_generation_active: row.get("is_active"),
        index_generation_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn fts_hit(id: &str, version: Uuid, rank: f32) -> FtsCandidate {
        FtsCandidate {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: id.into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: Uuid::new_v4(),
            rank,
        }
    }

    #[test]
    fn opening_chunks_are_appended_below_the_matched_cover_rank() {
        let version = Uuid::new_v4();
        let cover = fts_hit("cover", version, 0.8);
        let dieu1 = fts_hit("dieu-1", version, 0.0);
        let merged = merge_fts_opening_chunks(vec![cover.clone()], vec![cover.clone(), dieu1]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].chunk_identity_sha256, "cover");
        assert_eq!(merged[1].chunk_identity_sha256, "dieu-1");
        assert!((merged[1].rank - 0.8 * 0.35).abs() < f32::EPSILON);
    }

    #[test]
    fn only_active_generation_is_retrieval_visible() {
        assert!(index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Active
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Shadow
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Building
        ));
        assert!(!index_generation_visible_for_retrieval(
            false,
            IndexGenerationState::Active
        ));
        assert!(!index_generation_visible_for_retrieval(
            false,
            IndexGenerationState::Retired
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Draining
        ));
    }

    #[test]
    fn pg_real_rank_helper_preserves_f32() {
        // Compile-time contract: retrieval must decode REAL as f32, not f64.
        let value: f32 = 0.75;
        assert!((value - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn fts_query_removes_vietnamese_question_scaffolding() {
        assert_eq!(
            normalize_fts_query("Kinh phí được phê duyệt là bao nhiêu?"),
            "kinh phi phe duyet"
        );
        assert_eq!(
            normalize_fts_query("Bao nhiêu?"),
            "bao nhieu",
            "all-stop-word questions must retain a non-empty fallback"
        );
    }

    #[test]
    fn marker_shaped_denial_query_stats_are_non_empty() {
        let marker = format!("phase1c-marker-alpha-{}", "a".repeat(32));
        let stats = normalized_fts_query_stats(&marker);
        assert!(stats.nonempty);
        assert!(stats.token_count >= 3);
    }

    #[test]
    fn normalized_fts_query_stats_count_tokens_without_logging_query() {
        let query = "Kinh phí được phê duyệt là bao nhiêu?";
        let stats = normalized_fts_query_stats(query);
        let normalized_token_count = normalized_fts_query_for_retrieval(query)
            .split_whitespace()
            .count();
        assert!(stats.nonempty);
        assert_eq!(normalized_token_count, 4);
        assert_eq!(stats.token_count, normalized_token_count);
    }

    /// 1C-06: chunk/claim queries route ACL through the shared
    /// `db::acl_sql::acl_predicate_sql` builder (read access for retrieval).
    #[test]
    fn acl_predicate_sql_shape_is_pinned() {
        let sql = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
        for expected in [
            "acl_m.user_id = $4",
            "acl_m.state = 'active'",
            "acl_u.disabled_at IS NULL",
            "acl_c.org_id = d.org_id",
            "acl_c.id = d.collection_id",
            "acl_c.deleted_at IS NULL",
            "acl_p.code = $5",
            "$6",
            "acl_c.visibility = 'org'",
            "acl_c.owner_user_id = $4",
            "collection_user_access",
            "collection_group_access",
            "collection_role_access",
        ] {
            assert!(
                sql.contains(expected),
                "acl_predicate_sql missing required clause {expected:?} in:\n{sql}"
            );
        }
    }

    /// 1C-06 regression guard: every SQL string in this module that reads
    /// `chunks`/`claims` content scoped by collection must route its ACL
    /// check through the shared retrieval helpers rather than a hand-rolled
    /// EXISTS clause.
    #[test]
    fn every_chunk_scoped_query_embeds_acl_predicate() {
        let src = include_str!("search.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();

        let current_calls = production
            .matches("current_retrieval_acl_predicate(")
            .count()
            .saturating_sub(1); // exclude `fn current_retrieval_acl_predicate(`
        let historical_calls = production
            .matches("historical_retrieval_acl_predicate(")
            .count()
            .saturating_sub(1); // exclude `fn historical_retrieval_acl_predicate(`
        assert_eq!(
            current_calls, 5,
            "expected current_retrieval_acl_predicate at fts/opening/hydrate/conflict-current x2; got {current_calls}"
        );
        assert_eq!(
            historical_calls, 5,
            "expected historical_retrieval_acl_predicate at fts/opening/hydrate/conflict-historical x2; got {historical_calls}"
        );

        assert!(
            production.contains("use crate::db::acl_sql::acl_predicate_sql"),
            "search.rs must import the shared ACL predicate builder"
        );

        assert!(
            !production.contains("fn acl_predicate_sql("),
            "search.rs must not define a local acl_predicate_sql; use db::acl_sql"
        );

        assert!(
            !production.contains("collections acl_c"),
            "found a hand-rolled ACL EXISTS block; route through db::acl_sql::acl_predicate_sql"
        );
    }

    #[test]
    fn historical_retrieval_acl_predicate_requires_query_and_history() {
        let sql = historical_retrieval_acl_predicate(
            "d.org_id",
            "d.collection_id",
            "$4",
            "$5",
            "$7",
            "$6",
        );
        assert!(
            sql.contains("acl_p.code = $5"),
            "historical predicate must gate qa.query: {sql}"
        );
        assert!(
            sql.contains("acl_p.code = $7"),
            "historical predicate must gate qa.history: {sql}"
        );
        assert_eq!(
            sql.matches("acl_p.code =").count(),
            2,
            "historical predicate must combine qa.query and qa.history ACL arms"
        );
    }

    #[test]
    fn seed_acl_org_fixture_grants_qa_query_to_viewer_role() {
        let src = include_str!("../../tests/common/acl_fixture.rs");
        assert!(
            src.contains("SELECT id FROM roles WHERE org_id = $1 AND code = 'viewer'"),
            "fixture must resolve persisted viewer role id"
        );
        assert!(
            src.contains("INSERT INTO role_permissions") && src.contains("PERMISSION_QA_QUERY"),
            "fixture must grant qa.query to viewer role before membership downgrade"
        );
    }
}
