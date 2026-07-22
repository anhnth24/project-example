//! Tenant-scoped hybrid retrieval (P1B-R01).
//!
//! Pipeline: resolve scope + version mode → embed query → parallel Qdrant/FTS →
//! knowledge merge/rerank → PostgreSQL hydration/ACL/version recheck.
//!
//! Every public entry requires [`OrgContext`]. Chunk text and citations come only
//! from authorized PG hydration — never from Qdrant payload authority.

pub mod fts;
pub mod hydrate;
pub mod vector;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use fileconv_knowledge::embedding::EmbeddingPlan;
use fileconv_knowledge::query::PreparedQuery;
use fileconv_knowledge::rank::{hybrid_rerank_score, sort_hybrid_hits, VECTOR_WEIGHT};
use fileconv_knowledge::types::{HybridSearchHit, SourceAnchor};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, ResolveError};
use crate::db::error::DbError;
use crate::db::index_metadata;
use crate::db::models::IndexMetadata;
use crate::db::pool::with_org_txn;
use crate::db::search::{
    self, index_generation_visible_for_retrieval, AuthorizedConflictEvidence, VersionVisibility,
};
use crate::services::embedding::{ApprovedEmbeddingRuntime, EmbeddingError};
use crate::services::index_signature::collection_name_for_digest;
use crate::services::retrieval::fts::{self as fts_leg, LexicalCandidate};
use crate::services::retrieval::hydrate::{
    collect_candidate_identities, text_only_from_hydration, AuthorizedChunk,
};
use crate::services::retrieval::vector::{
    self as vector_leg, retain_version_ids, suppress_non_current_for_mode, VectorCandidate,
};
use crate::storage::error::StorageError;
use crate::storage::qdrant::{QdrantClient, VectorScope};

/// Permission required for retrieval (POC seed).
pub const PERMISSION_QA_QUERY: &str = "qa.query";
/// Additional permission required whenever retrieval can expose a superseded version.
pub const PERMISSION_QA_HISTORY: &str = "qa.history";

/// Candidate pull depth per leg before merge (desktop uses 250/500).
const LEG_CANDIDATE_LIMIT: usize = 250;
/// Bound embedding so a hung provider cannot stall retrieval forever.
pub const EMBED_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound each retrieval leg (FTS / Qdrant) independently.
pub const LEG_TIMEOUT: Duration = Duration::from_secs(5);

/// Version-aware retrieval mode (ADR 0002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionMode {
    Current,
    AsOf {
        at: DateTime<Utc>,
    },
    Compare {
        document_id: Uuid,
        version_a: Uuid,
        version_b: Uuid,
    },
    History {
        document_id: Uuid,
    },
}

/// Retrieval request. Empty collection filter uses the full OrgContext allow-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalRequest {
    pub query: String,
    pub collection_ids: Option<BTreeSet<Uuid>>,
    pub mode: VersionMode,
    pub limit: usize,
    /// Optional conflict ids to hydrate when both sides remain authorized.
    pub conflict_ids: Vec<Uuid>,
}

/// One authorized, reranked hit ready for citation / Q&A.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHit {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading: String,
    pub snippet: String,
    pub body: String,
    pub lexical_score: f32,
    pub vector_score: f32,
    pub rerank_score: f32,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub span_start: usize,
    pub span_end: usize,
}

/// Hybrid retrieval response (no public unauthenticated path).
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalResponse {
    pub hits: Vec<RetrievalHit>,
    pub warnings: Vec<String>,
    pub embedding_mode: String,
    pub conflict_evidence: Vec<AuthorizedConflictEvidence>,
    /// Frozen knowledge weight used for rerank (regression anchor).
    pub vector_weight: f32,
}

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("empty retrieval scope")]
    EmptyScope,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invalid retrieval request: {0}")]
    InvalidRequest(&'static str),
    #[error("compare/history versions are not in one lineage")]
    LineageMismatch,
    #[error("search dependency unavailable")]
    DependencyUnavailable,
    #[error("database error")]
    Database(#[from] DbError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("embedding error")]
    Embedding(#[from] EmbeddingError),
    #[error("both retrieval legs failed")]
    BothLegsFailed,
}

impl RetrievalError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyScope => "retrieval_empty_scope",
            Self::PermissionDenied => "retrieval_permission_denied",
            Self::InvalidRequest(_) => "retrieval_invalid_request",
            Self::LineageMismatch => "retrieval_lineage_mismatch",
            Self::DependencyUnavailable => "dependency_unavailable",
            Self::Database(_) => "retrieval_database",
            Self::Storage(_) => "retrieval_storage",
            Self::Embedding(_) => "retrieval_embedding",
            Self::BothLegsFailed => "retrieval_both_legs_failed",
        }
    }
}

impl From<ResolveError> for RetrievalError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::PermissionDenied | ResolveError::CollectionDenied => {
                Self::PermissionDenied
            }
            _ => Self::PermissionDenied,
        }
    }
}

/// Resolved tenant search scope (fail closed when empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScope {
    pub org_id: Uuid,
    pub collection_ids: BTreeSet<Uuid>,
}

/// Intersect requested collections with OrgContext allow-list; empty → deny.
pub fn resolve_scope(
    ctx: &OrgContext,
    requested: Option<&BTreeSet<Uuid>>,
) -> Result<ResolvedScope, RetrievalError> {
    let allowed = ctx.allowed_collection_ids();
    if allowed.is_empty() {
        return Err(RetrievalError::EmptyScope);
    }
    let collection_ids = match requested {
        None => allowed.clone(),
        Some(requested) => {
            if requested.is_empty() {
                return Err(RetrievalError::EmptyScope);
            }
            // Cross-scope: any requested id outside allow-list is a hard deny.
            if requested.iter().any(|id| !allowed.contains(id)) {
                return Err(RetrievalError::PermissionDenied);
            }
            let intersection: BTreeSet<Uuid> = requested
                .iter()
                .copied()
                .filter(|id| allowed.contains(id))
                .collect();
            if intersection.is_empty() {
                return Err(RetrievalError::EmptyScope);
            }
            intersection
        }
    };
    Ok(ResolvedScope {
        org_id: ctx.org_id(),
        collection_ids,
    })
}

/// Validates request shape before any store I/O.
pub fn validate_request(request: &RetrievalRequest) -> Result<(), RetrievalError> {
    if request.query.trim().is_empty() {
        return Err(RetrievalError::InvalidRequest("query is empty"));
    }
    if !(1..=100).contains(&request.limit) {
        return Err(RetrievalError::InvalidRequest("limit must be 1..=100"));
    }
    match &request.mode {
        VersionMode::Compare {
            version_a,
            version_b,
            ..
        } => {
            if version_a == version_b {
                return Err(RetrievalError::InvalidRequest(
                    "compare requires two distinct versions",
                ));
            }
        }
        VersionMode::Current | VersionMode::AsOf { .. } | VersionMode::History { .. } => {}
    }
    Ok(())
}

fn require_mode_permissions(ctx: &OrgContext, mode: &VersionMode) -> Result<(), RetrievalError> {
    require_permission(ctx, PERMISSION_QA_QUERY)?;
    if !matches!(mode, VersionMode::Current) {
        require_permission(ctx, PERMISSION_QA_HISTORY)?;
    }
    Ok(())
}

/// Wire pool + optional vector/embed backends, then run hybrid search.
///
/// Routes must call this (or [`hybrid_search`]) instead of touching Qdrant/storage
/// clients directly (ADR 0001).
pub async fn hybrid_search_with_backends(
    pool: &Pool,
    qdrant: Option<&QdrantClient>,
    embedder: Option<&ApprovedEmbeddingRuntime>,
    ctx: &OrgContext,
    request: RetrievalRequest,
) -> Result<RetrievalResponse, RetrievalError> {
    let qdrant = qdrant.ok_or(RetrievalError::DependencyUnavailable)?;
    hybrid_search(pool, qdrant, embedder, ctx, request).await
}

/// Tenant-scoped hybrid search. OrgContext is mandatory on every path.
pub async fn hybrid_search(
    pool: &Pool,
    qdrant: &QdrantClient,
    embedder: Option<&ApprovedEmbeddingRuntime>,
    ctx: &OrgContext,
    request: RetrievalRequest,
) -> Result<RetrievalResponse, RetrievalError> {
    require_mode_permissions(ctx, &request.mode)?;
    validate_request(&request)?;
    let scope = resolve_scope(ctx, request.collection_ids.as_ref())?;
    let collection_ids: Vec<Uuid> = scope.collection_ids.iter().copied().collect();

    let resolved = resolve_version_visibility(pool, ctx, &collection_ids, &request.mode).await?;
    let document_filter = match &request.mode {
        VersionMode::Compare { document_id, .. } | VersionMode::History { document_id } => {
            Some(*document_id)
        }
        VersionMode::Current | VersionMode::AsOf { .. } => None,
    };

    let prepared = PreparedQuery::new(&request.query);
    let mut warnings = Vec::new();
    let runtime_plan = embedder.map(ApprovedEmbeddingRuntime::plan);
    let query_vector = match embedder {
        Some(runtime) => {
            match with_timeout(EMBED_TIMEOUT, runtime.embed(&[request.query.clone()])).await {
                Ok(Ok(mut vectors)) => vectors.pop().filter(|vector| !vector.is_empty()),
                Ok(Err(_)) => {
                    warnings.push("Embedding provider error; using FTS-only retrieval.".into());
                    None
                }
                Err(_) => {
                    warnings.push("Embedding timed out; using FTS-only retrieval.".into());
                    None
                }
            }
        }
        None => {
            warnings.push("No embedding runtime configured; using FTS-only retrieval.".into());
            None
        }
    };
    let embedding_mode = embedder
        .map(|runtime| runtime.plan().runtime_path().to_string())
        .unwrap_or_else(|| "fts_only".into());

    let leg_limit = LEG_CANDIDATE_LIMIT.max(request.limit);

    let lexical_future = async {
        with_timeout(
            LEG_TIMEOUT,
            fts_leg::search_lexical(
                pool,
                ctx,
                &collection_ids,
                &request.query,
                &resolved.visibility,
                leg_limit,
            ),
        )
        .await
    };

    let vector_future = async {
        match query_vector.as_deref() {
            Some(vector) => {
                with_timeout(
                    LEG_TIMEOUT,
                    search_all_vector_legs(VectorLegSearch {
                        pool,
                        qdrant,
                        ctx,
                        collection_ids: &collection_ids,
                        query_vector: vector,
                        visibility: &resolved.visibility,
                        document_id: document_filter,
                        limit: leg_limit,
                        runtime_plan,
                        warnings: &mut warnings,
                    }),
                )
                .await
            }
            None => Ok(Ok(Vec::new())),
        }
    };

    let (lexical_result, vector_result) = tokio::join!(lexical_future, vector_future);

    let mut lexical_failed = false;
    let mut vector_failed = false;
    let mut lexical = match lexical_result {
        Ok(Ok(rows)) => fts_leg::filter_lexical_in_scope(&collection_ids, rows),
        Ok(Err(_)) | Err(_) => {
            lexical_failed = true;
            warnings.push("FTS leg unavailable; continuing with vector-only retrieval.".into());
            Vec::new()
        }
    };
    let mut vector_candidates = match vector_result {
        Ok(Ok(rows)) => rows,
        Ok(Err(_)) | Err(_) => {
            vector_failed = true;
            warnings.push("Vector leg unavailable; continuing with FTS-only retrieval.".into());
            Vec::new()
        }
    };

    // One-leg outage / timeout is graceful; only fail when every attempted leg errored.
    let vector_attempted = query_vector.is_some();
    if lexical_failed && (vector_failed || !vector_attempted) {
        return Err(RetrievalError::BothLegsFailed);
    }

    vector_candidates = suppress_non_current_for_mode(&resolved.visibility, vector_candidates);
    if let VersionVisibility::VersionIds(ref allowed) = resolved.visibility {
        vector_candidates = retain_version_ids(allowed, vector_candidates);
        lexical.retain(|candidate| allowed.contains(&candidate.version_id));
    }
    if let Some(document_id) = document_filter {
        lexical.retain(|candidate| candidate.document_id == document_id);
        vector_candidates.retain(|candidate| candidate.document_id == document_id);
    }

    let identities: Vec<String> = collect_candidate_identities(
        lexical
            .iter()
            .map(|candidate| candidate.chunk_identity_sha256.clone()),
        vector_candidates
            .iter()
            .map(|candidate| candidate.chunk_identity_sha256.clone()),
    )
    .into_iter()
    .collect();

    let hydrated = hydrate::hydrate_authorized_chunks(
        pool,
        ctx,
        &collection_ids,
        &identities,
        &resolved.visibility,
    )
    .await?;

    let hits = merge_rerank_hydrated(
        &lexical,
        &vector_candidates,
        &hydrated,
        &prepared.tokens,
        request.limit,
    );

    let conflict_evidence = if request.conflict_ids.is_empty() {
        Vec::new()
    } else {
        hydrate::hydrate_authorized_conflict_evidence(
            pool,
            ctx,
            &collection_ids,
            &request.conflict_ids,
            &resolved.visibility,
        )
        .await?
    };

    // Freeze VECTOR_WEIGHT so accidental reranker drift fails compile/tests.
    let _anchor: f32 = VECTOR_WEIGHT;
    debug_assert!((_anchor - 0.55).abs() < f32::EPSILON);

    Ok(RetrievalResponse {
        hits,
        warnings,
        embedding_mode,
        conflict_evidence,
        vector_weight: VECTOR_WEIGHT,
    })
}

struct ResolvedVisibility {
    visibility: VersionVisibility,
}

async fn resolve_version_visibility(
    pool: &Pool,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    mode: &VersionMode,
) -> Result<ResolvedVisibility, RetrievalError> {
    match mode {
        VersionMode::Current => Ok(ResolvedVisibility {
            visibility: VersionVisibility::Current,
        }),
        VersionMode::AsOf { at } => {
            let at = *at;
            let collection_ids = collection_ids.to_vec();
            let ids = with_org_txn(pool, ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        search::resolve_as_of_version_ids(txn, &ctx, &collection_ids, at).await
                    })
                }
            })
            .await?;
            Ok(ResolvedVisibility {
                visibility: VersionVisibility::VersionIds(ids),
            })
        }
        VersionMode::Compare {
            document_id,
            version_a,
            version_b,
        } => {
            let document_id = *document_id;
            let wanted = [*version_a, *version_b];
            let collection_ids = collection_ids.to_vec();
            let loaded = with_org_txn(pool, ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        search::load_lineage_versions(
                            txn,
                            &ctx,
                            document_id,
                            &wanted,
                            &collection_ids,
                        )
                        .await
                    })
                }
            })
            .await?;
            if !same_lineage_pair(&loaded, *version_a, *version_b) {
                return Err(RetrievalError::LineageMismatch);
            }
            Ok(ResolvedVisibility {
                visibility: VersionVisibility::VersionIds(BTreeSet::from([*version_a, *version_b])),
            })
        }
        VersionMode::History { document_id } => {
            let document_id = *document_id;
            let collection_ids = collection_ids.to_vec();
            let versions = with_org_txn(pool, ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        search::list_published_version_ids_for_document(
                            txn,
                            &ctx,
                            document_id,
                            &collection_ids,
                        )
                        .await
                    })
                }
            })
            .await?;
            if versions.is_empty() {
                return Err(RetrievalError::LineageMismatch);
            }
            let ids = versions.into_iter().map(|(id, _)| id).collect();
            Ok(ResolvedVisibility {
                visibility: VersionVisibility::VersionIds(ids),
            })
        }
    }
}

/// Compare/history: both versions must exist under the same document lineage.
pub fn same_lineage_pair(
    loaded: &[(Uuid, i32, Option<Uuid>)],
    version_a: Uuid,
    version_b: Uuid,
) -> bool {
    if version_a == version_b {
        return false;
    }
    let ids: BTreeSet<Uuid> = loaded.iter().map(|(id, _, _)| *id).collect();
    ids.contains(&version_a) && ids.contains(&version_b) && loaded.len() == 2
}

struct VectorLegSearch<'a> {
    pool: &'a Pool,
    qdrant: &'a QdrantClient,
    ctx: &'a OrgContext,
    collection_ids: &'a [Uuid],
    query_vector: &'a [f32],
    visibility: &'a VersionVisibility,
    document_id: Option<Uuid>,
    limit: usize,
    runtime_plan: Option<&'a EmbeddingPlan>,
    warnings: &'a mut Vec<String>,
}

/// Bound a retrieval dependency so hung legs degrade instead of stalling.
pub async fn with_timeout<T, E>(
    timeout: Duration,
    future: impl Future<Output = Result<T, E>>,
) -> Result<Result<T, E>, ()> {
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => Ok(result),
        Err(_) => Err(()),
    }
}

/// Active generation whose index signature matches the approved embedding runtime
/// **and** the actual query vector dimensionality (never search with a mismatched vector).
pub fn generation_compatible_with_runtime(
    meta: &IndexMetadata,
    plan: &EmbeddingPlan,
    query_dimensions: usize,
) -> bool {
    if query_dimensions == 0 {
        return false;
    }
    if !index_generation_visible_for_retrieval(meta.is_active, meta.state) {
        return false;
    }
    let Ok(dimensions) = usize::try_from(meta.dimensions) else {
        return false;
    };
    if dimensions != query_dimensions {
        return false;
    }
    if let Some(expected) = plan.expected_dimensions() {
        if expected != query_dimensions {
            return false;
        }
    }
    match plan.index_signature(query_dimensions) {
        Ok(signature) => signature.digest() == meta.index_signature_sha256,
        Err(_) => false,
    }
}

async fn search_all_vector_legs(
    input: VectorLegSearch<'_>,
) -> Result<Vec<VectorCandidate>, RetrievalError> {
    let VectorLegSearch {
        pool,
        qdrant,
        ctx,
        collection_ids,
        query_vector,
        visibility,
        document_id,
        limit,
        runtime_plan,
        warnings,
    } = input;
    let collection_ids_owned = collection_ids.to_vec();
    let active = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                index_metadata::list_active_for_collections(txn, &ctx, &collection_ids_owned).await
            })
        }
    })
    .await?;

    // Only search generations whose signature matches this runtime + query dims.
    // Never send one query vector into an incompatible Qdrant collection.
    let query_dimensions = query_vector.len();
    let mut by_signature: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
    let mut skipped_incompatible = 0usize;
    let Some(plan) = runtime_plan else {
        warnings.push(
            "Embedding plan missing; refusing vector search without signature compatibility."
                .into(),
        );
        return Ok(Vec::new());
    };
    for meta in active {
        if !generation_compatible_with_runtime(&meta, plan, query_dimensions) {
            skipped_incompatible += 1;
            continue;
        }
        let Some(collection_id) = meta.collection_id else {
            continue;
        };
        by_signature
            .entry(meta.index_signature_sha256)
            .or_default()
            .insert(collection_id);
    }
    if skipped_incompatible > 0 {
        warnings.push(format!(
            "Skipped {skipped_incompatible} active generation(s) with incompatible index signature."
        ));
    }

    let mut all = Vec::new();
    for (digest, collections) in by_signature {
        // Each digest group shares one signature; query_vector dims already matched.
        let name = collection_name_for_digest(&digest)?;
        let scope = VectorScope::new(ctx.org_id(), collections);
        let hits = vector_leg::search_vectors(
            qdrant,
            &name,
            &scope,
            query_vector,
            visibility,
            document_id,
            limit,
        )
        .await?;
        let hits = vector_leg::filter_candidates_in_scope(&scope, hits)?;
        all.extend(hits);
    }
    all.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.chunk_identity_sha256.cmp(&right.chunk_identity_sha256))
    });
    all.truncate(limit);
    Ok(all)
}

/// Merge lexical/vector ranks with frozen knowledge rerank, using hydrated text only.
pub fn merge_rerank_hydrated(
    lexical: &[LexicalCandidate],
    vector: &[VectorCandidate],
    hydrated: &HashMap<String, AuthorizedChunk>,
    query_tokens: &[String],
    limit: usize,
) -> Vec<RetrievalHit> {
    let lexical_rank: HashMap<&str, (usize, f32)> = lexical
        .iter()
        .enumerate()
        .map(|(rank, candidate)| {
            (
                candidate.chunk_identity_sha256.as_str(),
                (rank, candidate.score),
            )
        })
        .collect();
    let vector_rank: HashMap<&str, (usize, f32)> = vector
        .iter()
        .enumerate()
        .map(|(rank, candidate)| {
            (
                candidate.chunk_identity_sha256.as_str(),
                (rank, candidate.score),
            )
        })
        .collect();

    let mut hybrid_hits = Vec::new();
    let mut meta: HashMap<String, &AuthorizedChunk> = HashMap::new();

    // Unique identity union — dual-leg candidates must not produce duplicate hits.
    let mut identities: HashSet<&str> = HashSet::new();
    identities.extend(lexical_rank.keys().copied());
    identities.extend(vector_rank.keys().copied());

    for identity in identities {
        let Some(chunk) = text_only_from_hydration(identity, hydrated) else {
            // Stale vector / unauthorized: never emit text.
            continue;
        };
        let (lexical_position, lexical_score) = lexical_rank
            .get(identity)
            .copied()
            .unwrap_or((usize::MAX, 0.0));
        let (vector_position, vector_score) = vector_rank
            .get(identity)
            .copied()
            .unwrap_or((usize::MAX, 0.0));
        let snippet = extract_snippet(&chunk.body, query_tokens);
        hybrid_hits.push(HybridSearchHit {
            chunk_id: chunk.chunk_identity_sha256.clone(),
            source_rel: chunk.document_id.to_string(),
            md_rel: chunk.version_id.to_string(),
            heading: chunk.heading.clone(),
            snippet: snippet.clone(),
            lexical_score,
            vector_score,
            rerank_score: hybrid_rerank_score(
                (lexical_position != usize::MAX).then_some(lexical_position),
                (vector_position != usize::MAX).then_some(vector_position),
                vector_score,
                query_tokens,
                &chunk.heading,
                &chunk.body,
            ),
            anchor: SourceAnchor {
                page: chunk.page,
                slide: chunk.slide,
                sheet: chunk.sheet.clone(),
                start: chunk.span_start,
                end: chunk.span_end,
            },
        });
        meta.insert(chunk.chunk_identity_sha256.clone(), chunk);
    }

    sort_hybrid_hits(&mut hybrid_hits);
    hybrid_hits.truncate(limit.clamp(1, 100));

    hybrid_hits
        .into_iter()
        .filter_map(|hit| {
            let chunk = meta.get(&hit.chunk_id)?;
            Some(RetrievalHit {
                chunk_id: chunk.chunk_id,
                chunk_identity_sha256: chunk.chunk_identity_sha256.clone(),
                collection_id: chunk.collection_id,
                document_id: chunk.document_id,
                version_id: chunk.version_id,
                version_number: chunk.version_number,
                content_sha256: chunk.content_sha256.clone(),
                heading: chunk.heading.clone(),
                snippet: hit.snippet,
                body: chunk.body.clone(),
                lexical_score: hit.lexical_score,
                vector_score: hit.vector_score,
                rerank_score: hit.rerank_score,
                is_current: chunk.is_current,
                effective_from: chunk.effective_from,
                effective_to: chunk.effective_to,
                page: chunk.page,
                slide: chunk.slide,
                sheet: chunk.sheet.clone(),
                span_start: chunk.span_start,
                span_end: chunk.span_end,
            })
        })
        .collect()
}

fn extract_snippet(body: &str, query_tokens: &[String]) -> String {
    const MAX: usize = 240;
    if body.chars().count() <= MAX {
        return body.to_string();
    }
    let lowered = fileconv_core::intelligence::normalize_search_text(body);
    let mut start = 0usize;
    for token in query_tokens {
        if let Some(index) = lowered.find(token.as_str()) {
            start = index.saturating_sub(40);
            break;
        }
    }
    body.chars().skip(start).take(MAX).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::IndexGenerationState;
    use crate::services::retrieval::hydrate::authorize_hydrated_row;
    use chrono::TimeZone;
    use fileconv_knowledge::rank::VECTOR_WEIGHT;

    fn ctx_with(collections: impl IntoIterator<Item = Uuid>) -> OrgContext {
        OrgContext::try_new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            [PERMISSION_QA_QUERY],
            collections,
        )
        .unwrap()
    }

    fn authorized(
        identity: &str,
        collection_id: Uuid,
        version_id: Uuid,
        is_current: bool,
        body: &str,
    ) -> AuthorizedChunk {
        AuthorizedChunk {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: identity.into(),
            collection_id,
            document_id: Uuid::new_v4(),
            version_id,
            version_number: 1,
            content_sha256: "b".repeat(64),
            heading: "Đối soát".into(),
            body: body.into(),
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: body.len(),
            is_current,
            effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
        }
    }

    #[test]
    fn empty_scope_denied_fail_closed() {
        let ctx = ctx_with([]);
        assert!(matches!(
            resolve_scope(&ctx, None),
            Err(RetrievalError::EmptyScope)
        ));
        let allowed = Uuid::new_v4();
        let ctx = ctx_with([allowed]);
        assert!(matches!(
            resolve_scope(&ctx, Some(&BTreeSet::new())),
            Err(RetrievalError::EmptyScope)
        ));
        let foreign = Uuid::new_v4();
        assert!(matches!(
            resolve_scope(&ctx, Some(&BTreeSet::from([foreign]))),
            Err(RetrievalError::PermissionDenied)
        ));
    }

    #[test]
    fn cross_scope_request_denied() {
        let allowed = Uuid::new_v4();
        let foreign = Uuid::new_v4();
        let ctx = ctx_with([allowed]);
        let err = resolve_scope(&ctx, Some(&BTreeSet::from([allowed, foreign]))).unwrap_err();
        assert!(matches!(err, RetrievalError::PermissionDenied));
    }

    #[test]
    fn historical_modes_require_explicit_history_permission() {
        let collection = Uuid::new_v4();
        let query_only = ctx_with([collection]);
        let modes = [
            VersionMode::AsOf { at: Utc::now() },
            VersionMode::Compare {
                document_id: Uuid::new_v4(),
                version_a: Uuid::new_v4(),
                version_b: Uuid::new_v4(),
            },
            VersionMode::History {
                document_id: Uuid::new_v4(),
            },
        ];
        for mode in &modes {
            assert!(matches!(
                require_mode_permissions(&query_only, mode),
                Err(RetrievalError::PermissionDenied)
            ));
        }

        let allowed = OrgContext::try_new(
            query_only.org_id(),
            query_only.user_id(),
            [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
            [collection],
        )
        .unwrap();
        assert!(require_mode_permissions(&allowed, &VersionMode::Current).is_ok());
        for mode in &modes {
            assert!(require_mode_permissions(&allowed, mode).is_ok());
        }
    }

    #[test]
    fn stale_vector_candidate_never_returns_text_without_hydration() {
        let collection = Uuid::new_v4();
        let version = Uuid::new_v4();
        let lexical = Vec::new();
        let vector = vec![VectorCandidate {
            chunk_identity_sha256: "stale".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 0.99,
            payload_is_current: true,
        }];
        let hydrated = HashMap::new();
        let hits = merge_rerank_hydrated(&lexical, &vector, &hydrated, &["doi".into()], 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn current_mode_merge_skips_superseded_hydrated_rows() {
        // Hydration layer already filters; merge must not invent text either.
        let collection = Uuid::new_v4();
        let current_version = Uuid::new_v4();
        let superseded = Uuid::new_v4();
        let chunk = authorized(
            "cur",
            collection,
            current_version,
            true,
            "Đối soát giao theo ngày",
        );
        let mut hydrated = HashMap::new();
        hydrated.insert("cur".into(), chunk);
        let vector = vec![
            VectorCandidate {
                chunk_identity_sha256: "cur".into(),
                document_id: Uuid::new_v4(),
                version_id: current_version,
                collection_id: collection,
                score: 0.8,
                payload_is_current: true,
            },
            VectorCandidate {
                chunk_identity_sha256: "old".into(),
                document_id: Uuid::new_v4(),
                version_id: superseded,
                collection_id: collection,
                score: 0.95,
                payload_is_current: false,
            },
        ];
        let hits = merge_rerank_hydrated(&[], &vector, &hydrated, &["doi".into()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_identity_sha256, "cur");
        assert!(hits[0].is_current);
    }

    #[test]
    fn as_of_visibility_uses_resolved_version_set() {
        let v1 = Uuid::new_v4();
        let v2 = Uuid::new_v4();
        let visibility = VersionVisibility::VersionIds(BTreeSet::from([v1]));
        let collection = Uuid::new_v4();
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, [PERMISSION_QA_QUERY], [collection]).unwrap();
        let mut row = crate::db::search::HydratedChunkRow {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "id".into(),
            org_id: org,
            collection_id: collection,
            document_id: Uuid::new_v4(),
            version_id: v2,
            version_number: 2,
            content_sha256: "c".repeat(64),
            heading_path: vec!["H".into()],
            body: "body".into(),
            page: None,
            slide: None,
            sheet: None,
            span_start: Some(0),
            span_end: Some(4),
            document_state: crate::db::models::DocumentState::Indexed,
            deleted_at: None,
            publication_state: crate::db::models::PublicationState::Published,
            is_current: true,
            effective_from: Utc::now(),
            effective_to: None,
            index_metadata_id: Uuid::new_v4(),
            index_generation_active: true,
            index_generation_state: IndexGenerationState::Active,
        };
        assert!(authorize_hydrated_row(&ctx, &row, &visibility).is_none());
        row.version_id = v1;
        assert!(authorize_hydrated_row(&ctx, &row, &visibility).is_some());
    }

    #[test]
    fn compare_history_require_same_lineage() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let loaded = vec![(a, 1, None), (b, 2, Some(a))];
        assert!(same_lineage_pair(&loaded, a, b));
        assert!(!same_lineage_pair(&loaded, a, Uuid::new_v4()));
        assert!(!same_lineage_pair(&[(a, 1, None)], a, b));
    }

    #[test]
    fn one_leg_outage_still_hydrates_safely() {
        let collection = Uuid::new_v4();
        let version = Uuid::new_v4();
        let chunk = authorized(
            "only-fts",
            collection,
            version,
            true,
            "Đối soát giao theo ngày",
        );
        let mut hydrated = HashMap::new();
        hydrated.insert("only-fts".into(), chunk);
        let lexical = vec![LexicalCandidate {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "only-fts".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 1.2,
        }];
        // Vector leg empty (outage) — FTS-only still returns hydrated text.
        let hits =
            merge_rerank_hydrated(&lexical, &[], &hydrated, &["doi".into(), "soat".into()], 5);
        assert_eq!(hits.len(), 1);
        assert!(!hits[0].body.is_empty());
        assert!(hits[0].rerank_score > 0.0);

        // FTS empty, vector-only still requires hydration.
        let vector = vec![VectorCandidate {
            chunk_identity_sha256: "only-fts".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 0.7,
            payload_is_current: true,
        }];
        let hits = merge_rerank_hydrated(&[], &vector, &hydrated, &["doi".into()], 5);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn uses_frozen_knowledge_vector_weight() {
        assert!((VECTOR_WEIGHT - 0.55).abs() < f32::EPSILON);
        let tokens = vec!["doi".into(), "soat".into(), "giao".into(), "dich".into()];
        let score = hybrid_rerank_score(
            Some(0),
            Some(0),
            0.75,
            &tokens,
            "Đối soát",
            "Đối soát giao theo ngày",
        );
        assert!((score - 1.875).abs() < 0.0001);
    }

    #[test]
    fn deleted_tombstoned_suppressed_in_authorize() {
        let collection = Uuid::new_v4();
        let org = Uuid::new_v4();
        let ctx =
            OrgContext::try_new(org, Uuid::new_v4(), [PERMISSION_QA_QUERY], [collection]).unwrap();
        let mut row = crate::db::search::HydratedChunkRow {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "x".into(),
            org_id: org,
            collection_id: collection,
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            version_number: 1,
            content_sha256: "d".repeat(64),
            heading_path: vec![],
            body: "secret".into(),
            page: None,
            slide: None,
            sheet: None,
            span_start: None,
            span_end: None,
            document_state: crate::db::models::DocumentState::Tombstoned,
            deleted_at: Some(Utc::now()),
            publication_state: crate::db::models::PublicationState::Published,
            is_current: true,
            effective_from: Utc::now(),
            effective_to: None,
            index_metadata_id: Uuid::new_v4(),
            index_generation_active: true,
            index_generation_state: IndexGenerationState::Active,
        };
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_none());
        row.document_state = crate::db::models::DocumentState::Indexed;
        row.deleted_at = None;
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_some());
    }

    #[test]
    fn dual_leg_candidate_emits_single_hit() {
        let collection = Uuid::new_v4();
        let version = Uuid::new_v4();
        let chunk = authorized(
            "shared",
            collection,
            version,
            true,
            "Đối soát giao theo ngày",
        );
        let mut hydrated = HashMap::new();
        hydrated.insert("shared".into(), chunk);
        let lexical = vec![LexicalCandidate {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "shared".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 1.1,
        }];
        let vector = vec![VectorCandidate {
            chunk_identity_sha256: "shared".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 0.8,
            payload_is_current: true,
        }];
        let hits = merge_rerank_hydrated(
            &lexical,
            &vector,
            &hydrated,
            &["doi".into(), "soat".into()],
            10,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_identity_sha256, "shared");
        assert!(hits[0].lexical_score > 0.0);
        assert!(hits[0].vector_score > 0.0);
    }

    #[tokio::test]
    async fn hung_leg_timeout_degrades_instead_of_stall() {
        let result = with_timeout(Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<(), RetrievalError>(())
        })
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn incompatible_generation_is_not_vector_searched() {
        let plan = EmbeddingPlan::local_hash_v1();
        let mut meta = IndexMetadata {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            collection_id: Some(Uuid::new_v4()),
            index_signature_sha256: "a".repeat(64),
            identity_version: 2,
            chunking_version: "heading-chunks-2000-v1".into(),
            body_text_version: "nfc-v1".into(),
            query_normalization_version: "accent-fold-v1".into(),
            embedding_family: "other".into(),
            embedding_revision: "x".into(),
            dimensions: 256,
            normalized: true,
            runtime_path: crate::db::models::EmbeddingRuntimePath::LocalHash,
            generation: 1,
            is_active: true,
            state: IndexGenerationState::Active,
            created_at: Utc::now(),
        };
        assert!(!generation_compatible_with_runtime(&meta, &plan, 256));
        meta.index_signature_sha256 = plan.signature(256).unwrap();
        assert!(generation_compatible_with_runtime(&meta, &plan, 256));
        assert!(
            !generation_compatible_with_runtime(&meta, &plan, 128),
            "must not search a generation with a differently sized query vector"
        );
        meta.state = IndexGenerationState::Shadow;
        assert!(!generation_compatible_with_runtime(&meta, &plan, 256));
        meta.state = IndexGenerationState::Active;
        meta.dimensions = 128;
        meta.index_signature_sha256 = plan.signature(128).unwrap_or_else(|_| "b".repeat(64));
        assert!(
            !generation_compatible_with_runtime(&meta, &plan, 256),
            "dimension mismatch must fail closed even if a signature string is present"
        );
    }

    #[test]
    fn golden_dual_leg_rerank_score_and_latency_budget() {
        let collection = Uuid::new_v4();
        let version = Uuid::new_v4();
        let body = "Đối soát giao dịch theo ngày cho chi nhánh Hà Nội";
        let chunk = authorized("golden", collection, version, true, body);
        let mut hydrated = HashMap::new();
        hydrated.insert("golden".into(), chunk);
        let lexical = vec![LexicalCandidate {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "golden".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 1.4,
        }];
        let vector = vec![VectorCandidate {
            chunk_identity_sha256: "golden".into(),
            document_id: Uuid::new_v4(),
            version_id: version,
            collection_id: collection,
            score: 0.75,
            payload_is_current: true,
        }];
        let tokens = vec!["doi".into(), "soat".into(), "giao".into(), "dich".into()];
        let expected = hybrid_rerank_score(Some(0), Some(0), 0.75, &tokens, "Đối soát", body);
        let started = std::time::Instant::now();
        let hits = merge_rerank_hydrated(&lexical, &vector, &hydrated, &tokens, 10);
        let elapsed = started.elapsed();
        assert_eq!(hits.len(), 1);
        assert!((hits[0].rerank_score - expected).abs() < 0.0001);
        assert!(hits[0].lexical_score > 0.0 && hits[0].vector_score > 0.0);
        assert!(
            elapsed < Duration::from_millis(50),
            "hermetic merge latency budget exceeded: {elapsed:?}"
        );
    }

    #[test]
    fn accent_fold_query_matches_desktop_normalization() {
        let folded = fileconv_core::intelligence::normalize_search_text("Đối soát");
        assert_eq!(folded, "doi soat");
        let prepared = PreparedQuery::new("Đối soát GIAO DỊCH");
        assert_eq!(prepared.tokens, ["doi", "soat", "giao", "dich"]);
    }

    #[test]
    fn validate_request_rejects_bad_compare_and_limits() {
        let request = RetrievalRequest {
            query: " ".into(),
            collection_ids: None,
            mode: VersionMode::Current,
            limit: 10,
            conflict_ids: vec![],
        };
        assert!(validate_request(&request).is_err());
        let v = Uuid::new_v4();
        let request = RetrievalRequest {
            query: "ok".into(),
            collection_ids: None,
            mode: VersionMode::Compare {
                document_id: Uuid::new_v4(),
                version_a: v,
                version_b: v,
            },
            limit: 10,
            conflict_ids: vec![],
        };
        assert!(validate_request(&request).is_err());
    }
}
