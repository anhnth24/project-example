//! Desktop knowledge orchestration without Tauri, settings, or data-root access.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use fileconv_core::intelligence::CorpusDocument;

use crate::ask::{
    extractive_answer, grounded_user_prompt, retrieval_context, valid_citation_ids, AnswerMode,
    GROUNDED_SYSTEM_PROMPT,
};
use crate::citation::{extract_snippet, validate_grounded_answer};
use crate::desktop::hnsw;
use crate::desktop::sqlite::{SqliteKnowledgeStore, StoredChunk};
use crate::embedding::{
    local_vector, EmbeddingPlan, ProviderDeployment, LOCAL_EMBEDDING_MODE, LOCAL_VECTOR_DIMENSIONS,
    PROVIDER_EMBEDDING_MODE,
};
use crate::query::{fts5_prefix_query, normalized_tokens};
use crate::rank::{cosine_similarity, hybrid_rerank_score, sort_hybrid_hits};
use crate::types::{
    GroundedAnswer, HybridAskRequest, HybridSearchHit, HybridSearchResponse, IndexBuildResult,
    IndexMetadata, IndexStats, SourceAnchor,
};
use crate::{KnowledgeError, Result};

#[derive(Debug, Clone)]
pub struct KnowledgePaths {
    pub database: PathBuf,
    pub ann_root: PathBuf,
}

impl KnowledgePaths {
    pub fn new(database: impl Into<PathBuf>, ann_root: impl Into<PathBuf>) -> Self {
        Self {
            database: database.into(),
            ann_root: ann_root.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DesktopEmbeddingPlan {
    metadata: IndexMetadata,
    signature_plan: Option<EmbeddingPlan>,
}

impl DesktopEmbeddingPlan {
    pub fn local() -> Self {
        Self {
            metadata: IndexMetadata {
                mode: LOCAL_EMBEDDING_MODE.into(),
                provider: "local".into(),
                model: LOCAL_EMBEDDING_MODE.into(),
                dimensions: LOCAL_VECTOR_DIMENSIONS,
                signature: LOCAL_EMBEDDING_MODE.into(),
            },
            // Existing desktop indexes retain their legacy signature.
            signature_plan: None,
        }
    }

    pub fn provider(
        provider: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        dimensions: Option<usize>,
    ) -> Result<Self> {
        let provider = provider.into();
        let model = model.into();
        let deployment = ProviderDeployment::from_base_url(base_url)
            .or_else(|_| ProviderDeployment::from_base_url(None))?;
        let signature_plan = EmbeddingPlan::provider(
            provider.clone(),
            model.clone(),
            model.clone(),
            deployment,
            dimensions,
        )?;
        Ok(Self {
            metadata: IndexMetadata {
                mode: PROVIDER_EMBEDDING_MODE.into(),
                provider,
                model,
                dimensions: dimensions.unwrap_or_default(),
                signature: signature_plan.provisional_signature(),
            },
            signature_plan: Some(signature_plan),
        })
    }

    pub fn metadata(&self) -> &IndexMetadata {
        &self.metadata
    }

    pub fn is_provider(&self) -> bool {
        self.signature_plan.is_some()
    }

    fn matches(&self, stored: &IndexMetadata) -> bool {
        let mut metadata = self.metadata.clone();
        if metadata.dimensions == 0 && stored.dimensions > 0 {
            metadata.dimensions = stored.dimensions;
        }
        if let Some(plan) = self.signature_plan.as_ref() {
            if metadata.dimensions > 0 {
                let Ok(signature) = plan.signature(metadata.dimensions) else {
                    return false;
                };
                metadata.signature = signature;
            }
        }
        metadata.signature == stored.signature
    }
}

pub fn rebuild_index<Embed>(
    paths: &KnowledgePaths,
    documents: &[CorpusDocument],
    plan: &DesktopEmbeddingPlan,
    fallback_local: bool,
    mut embed_provider: Embed,
) -> Result<IndexBuildResult>
where
    Embed: FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
{
    match rebuild_once(paths, documents, plan, &mut embed_provider) {
        Ok(result) => Ok(result),
        Err(KnowledgeError::EmbeddingProviderFailure) if plan.is_provider() && fallback_local => {
            let mut result = rebuild_once(
                paths,
                documents,
                &DesktopEmbeddingPlan::local(),
                &mut embed_provider,
            )?;
            result.warnings.push(
                "embedding provider lỗi; đã rebuild toàn bộ scope bằng local hash offline.".into(),
            );
            Ok(result)
        }
        Err(error) => Err(error),
    }
}

fn rebuild_once(
    paths: &KnowledgePaths,
    documents: &[CorpusDocument],
    plan: &DesktopEmbeddingPlan,
    embed_provider: &mut impl FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
) -> Result<IndexBuildResult> {
    let mut store = SqliteKnowledgeStore::open(&paths.database)?;
    let stored = store.index_documents(
        documents,
        plan.metadata.clone(),
        plan.signature_plan.as_ref(),
        |inputs| {
            if plan.is_provider() {
                embed_provider(inputs)
            } else {
                Ok(inputs
                    .iter()
                    .map(|input| local_vector(input).into_values())
                    .collect())
            }
        },
        || hnsw::clear(&paths.ann_root),
    )?;
    let mut warnings = Vec::new();
    if stored.indexed > 0
        || !hnsw::is_available(
            &paths.ann_root,
            &stored.metadata.signature,
            stored.metadata.dimensions,
        )
    {
        match store
            .load_vector_points(stored.metadata.dimensions)
            .and_then(|points| {
                hnsw::rebuild(
                    &paths.ann_root,
                    &stored.metadata.signature,
                    stored.metadata.dimensions,
                    &points,
                )
                .map(|_| ())
            }) {
            Ok(()) => {}
            Err(error) => warnings.push(format!(
                "HNSW cache build lỗi ({error}); search sẽ dùng exact cosine."
            )),
        }
    }
    Ok(IndexBuildResult {
        documents: stored.documents,
        chunks: stored.chunks,
        indexed: stored.indexed,
        skipped: stored.skipped,
        embedding_mode: stored.metadata.mode,
        embedding_provider: stored.metadata.provider,
        embedding_model: stored.metadata.model,
        vector_dimensions: stored.metadata.dimensions,
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn hybrid_search<EmbedBatch, EmbedQuery>(
    paths: &KnowledgePaths,
    documents: &[CorpusDocument],
    source_scope: &[String],
    query: &str,
    limit: usize,
    plan: &DesktopEmbeddingPlan,
    fallback_local: bool,
    mut embed_batch: EmbedBatch,
    mut embed_query: EmbedQuery,
) -> Result<HybridSearchResponse>
where
    EmbedBatch: FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
    EmbedQuery: FnMut(&str) -> Result<Vec<f32>>,
{
    if query.trim().is_empty() {
        return Ok(HybridSearchResponse {
            hits: Vec::new(),
            warnings: Vec::new(),
            embedding_mode: LOCAL_EMBEDDING_MODE.into(),
        });
    }
    if !documents.is_empty() {
        rebuild_index(paths, documents, plan, fallback_local, &mut embed_batch)?;
    }
    let store = SqliteKnowledgeStore::open(&paths.database)?;
    let metadata = store.metadata()?;
    let scope: HashSet<String> = source_scope.iter().cloned().collect();
    let query_tokens = normalized_tokens(query);
    let lexical_rank = store.lexical_ranks(&fts5_prefix_query(query), &scope, 250)?;
    let chunks = store.load_chunks(&scope, metadata.dimensions)?;
    let mut warnings = Vec::new();
    let query_vector = if metadata.mode == PROVIDER_EMBEDDING_MODE {
        if plan.matches(&metadata) {
            match embed_query(query) {
                Ok(vector) if vector.len() == metadata.dimensions => Some(vector),
                Ok(vector) => {
                    warnings.push(format!(
                        "Query embedding {}D không khớp index {}D; chỉ dùng FTS.",
                        vector.len(),
                        metadata.dimensions
                    ));
                    None
                }
                Err(_) => {
                    warnings.push("Embedding provider lỗi; chỉ dùng FTS lexical.".into());
                    None
                }
            }
        } else {
            warnings
                .push("Cấu hình embedding không khớp index; hãy rebuild. Tạm chỉ dùng FTS.".into());
            None
        }
    } else {
        Some(local_vector(query).into_values())
    };
    rank_hits(
        &paths.ann_root,
        chunks,
        lexical_rank,
        query_vector.as_deref(),
        &query_tokens,
        limit,
        &metadata,
        warnings,
    )
}

#[allow(clippy::too_many_arguments)]
fn rank_hits(
    ann_root: &Path,
    chunks: Vec<StoredChunk>,
    lexical_rank: HashMap<String, (usize, f32)>,
    query_vector: Option<&[f32]>,
    query_tokens: &[String],
    limit: usize,
    metadata: &IndexMetadata,
    mut warnings: Vec<String>,
) -> Result<HybridSearchResponse> {
    let scoped_ids: HashSet<&str> = chunks.iter().map(|chunk| chunk.id.as_str()).collect();
    let mut vector_order = if let Some(query_vector) = query_vector {
        if chunks.len() > 1_000 {
            match hnsw::search(
                ann_root,
                &metadata.signature,
                metadata.dimensions,
                query_vector,
                (chunks.len() * 4).clamp(500, 5_000),
            ) {
                Ok(candidates) => {
                    let scoped = candidates
                        .into_iter()
                        .filter(|(id, _)| scoped_ids.contains(id.as_str()))
                        .collect::<Vec<_>>();
                    if scoped.len() >= 20.min(chunks.len()) {
                        scoped
                    } else {
                        warnings.push(
                            "HNSW trả quá ít candidate trong scope; dùng exact cosine.".into(),
                        );
                        exact_scores(&chunks, query_vector)
                    }
                }
                Err(error) => {
                    warnings.push(format!("HNSW chưa dùng được ({error}); dùng exact cosine."));
                    exact_scores(&chunks, query_vector)
                }
            }
        } else {
            exact_scores(&chunks, query_vector)
        }
    } else {
        Vec::new()
    };
    vector_order.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let vector_rank = vector_order
        .into_iter()
        .take(500)
        .enumerate()
        .map(|(rank, (id, score))| (id, (rank, score)))
        .collect::<HashMap<_, _>>();
    let by_id = chunks
        .iter()
        .map(|chunk| (chunk.id.as_str(), chunk))
        .collect::<HashMap<_, _>>();
    let candidate_ids = lexical_rank
        .keys()
        .chain(vector_rank.keys())
        .cloned()
        .collect::<HashSet<_>>();
    let mut hits = Vec::new();
    for id in candidate_ids {
        let Some(chunk) = by_id.get(id.as_str()) else {
            continue;
        };
        let (lexical_position, lexical_score) =
            lexical_rank.get(&id).copied().unwrap_or((usize::MAX, 0.0));
        let (vector_position, vector_score) =
            vector_rank.get(&id).copied().unwrap_or((usize::MAX, 0.0));
        hits.push(HybridSearchHit {
            chunk_id: chunk.id.clone(),
            source_rel: chunk.source_rel.clone(),
            md_rel: chunk.md_rel.clone(),
            heading: chunk.heading.clone(),
            snippet: extract_snippet(&chunk.body, query_tokens),
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
                start: chunk.start,
                end: chunk.end,
            },
        });
    }
    sort_hybrid_hits(&mut hits);
    hits.truncate(limit.max(1));
    Ok(HybridSearchResponse {
        hits,
        warnings,
        embedding_mode: metadata.mode.clone(),
    })
}

fn exact_scores(chunks: &[StoredChunk], query: &[f32]) -> Vec<(String, f32)> {
    chunks
        .iter()
        .map(|chunk| (chunk.id.clone(), cosine_similarity(query, &chunk.vector)))
        .collect()
}

pub fn index_stats(paths: &KnowledgePaths) -> Result<IndexStats> {
    let store = SqliteKnowledgeStore::open(&paths.database)?;
    let metadata = store.metadata()?;
    Ok(IndexStats {
        documents: store.document_count()?,
        chunks: store.chunk_count()?,
        database_bytes: store.database_bytes(),
        vector_dimensions: metadata.dimensions,
        embedding_mode: metadata.mode,
        embedding_provider: metadata.provider,
        embedding_model: metadata.model,
        ann_available: hnsw::is_available(
            &paths.ann_root,
            &metadata.signature,
            metadata.dimensions,
        ),
        ann_threshold: 1_000,
    })
}

pub fn grounded_answer<Chat>(
    request: &HybridAskRequest,
    search: HybridSearchResponse,
    llm_mode: Option<AnswerMode>,
    llm_config_warning: Option<String>,
    embedding_warning: Option<String>,
    mut chat: Chat,
) -> Result<GroundedAnswer>
where
    Chat: FnMut(&str, &str) -> Result<String>,
{
    let hits = search.hits;
    let mut warnings = search.warnings;
    if let Some(warning) = embedding_warning {
        warnings.push(format!(
            "Cấu hình embedding chưa dùng được ({warning}); đã dùng local hash."
        ));
    }
    let fallback = extractive_answer(&request.question, &hits);
    if !request.use_llm.unwrap_or(false) {
        return Ok(answer(
            fallback,
            hits,
            AnswerMode::OfflineExtractive,
            warnings,
        ));
    }
    let Some(llm_mode) = llm_mode else {
        warnings.push(
            llm_config_warning
                .map(|error| {
                    format!("Cấu hình LLM chưa dùng được ({error}); đã fallback extractive local.")
                })
                .unwrap_or_else(|| {
                    "Chưa cấu hình LLM provider; đã dùng câu trả lời extractive local.".into()
                }),
        );
        return Ok(answer(
            fallback,
            hits,
            AnswerMode::FallbackExtractive,
            warnings,
        ));
    };
    if hits.is_empty() {
        warnings.push("Không đủ nguồn để gọi LLM.".into());
        return Ok(answer(
            fallback,
            hits,
            AnswerMode::FallbackExtractive,
            warnings,
        ));
    }
    let prompt = grounded_user_prompt(&request.question, &retrieval_context(&hits));
    let llm_answer = match chat(GROUNDED_SYSTEM_PROMPT, &prompt) {
        Ok(answer) => answer,
        Err(_) => {
            warnings.push("LLM provider lỗi; đã fallback extractive local.".into());
            return Ok(answer(
                fallback,
                hits,
                AnswerMode::FallbackExtractive,
                warnings,
            ));
        }
    };
    match validate_grounded_answer(&llm_answer, &valid_citation_ids(hits.len())) {
        Ok(()) => Ok(answer(llm_answer, hits, llm_mode, warnings)),
        Err(mut grounding_warnings) => {
            warnings.append(&mut grounding_warnings);
            Ok(answer(
                fallback,
                hits,
                AnswerMode::FallbackExtractive,
                warnings,
            ))
        }
    }
}

fn answer(
    answer: String,
    citations: Vec<HybridSearchHit>,
    mode: AnswerMode,
    warnings: Vec<String>,
) -> GroundedAnswer {
    GroundedAnswer {
        answer,
        citations,
        mode: mode.as_str().into(),
        grounded: true,
        warnings,
    }
}
