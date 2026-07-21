//! Desktop knowledge orchestration without Tauri, settings, or data-root access.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use fileconv_core::intelligence::{CorpusDocument, INTELLIGENCE_ID_SCHEME};

use crate::ask::{
    extractive_answer, grounded_user_prompt, retrieval_context, valid_citation_ids, AnswerMode,
    GROUNDED_SYSTEM_PROMPT,
};
use crate::citation::{extract_snippet, validate_grounded_answer};
use crate::desktop::hnsw;
use crate::desktop::sqlite::{SqliteKnowledgeStore, StoredChunk};
use crate::embedding::{
    infer_runtime_path, local_vector, EmbeddingPlan, ProviderDeployment, LOCAL_EMBEDDING_MODE,
    LOCAL_VECTOR_DIMENSIONS, PROVIDER_EMBEDDING_MODE,
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
        let signature_plan = EmbeddingPlan::local_hash_v1();
        let signature = signature_plan
            .signature(LOCAL_VECTOR_DIMENSIONS)
            .expect("local hash plan has fixed dimensions");
        Self {
            metadata: IndexMetadata {
                mode: LOCAL_EMBEDDING_MODE.into(),
                provider: "local".into(),
                model: LOCAL_EMBEDDING_MODE.into(),
                dimensions: LOCAL_VECTOR_DIMENSIONS,
                signature,
                id_scheme: INTELLIGENCE_ID_SCHEME.into(),
            },
            // Schema-v2 canonical plan; legacy `"local_hash_v1"` string signatures rebuild.
            signature_plan: Some(signature_plan),
        }
    }

    pub fn provider(
        provider: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        dimensions: Option<usize>,
    ) -> Result<Self> {
        Self::provider_with_runtime(provider, model, base_url, dimensions, None)
    }

    pub fn provider_with_runtime(
        provider: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        dimensions: Option<usize>,
        runtime_path: Option<&str>,
    ) -> Result<Self> {
        let model = model.into();
        Self::provider_with_revision(
            provider,
            model.clone(),
            model,
            base_url,
            dimensions,
            runtime_path,
        )
    }

    pub fn provider_with_revision(
        provider: impl Into<String>,
        model: impl Into<String>,
        revision: impl Into<String>,
        base_url: Option<&str>,
        dimensions: Option<usize>,
        runtime_path: Option<&str>,
    ) -> Result<Self> {
        let provider = provider.into();
        let model = model.into();
        let revision = revision.into();
        let runtime = runtime_path
            .unwrap_or_else(|| infer_runtime_path(base_url, &model))
            .to_string();
        let deployment = ProviderDeployment::from_base_url(base_url)
            .or_else(|_| ProviderDeployment::from_base_url(None))?;
        let signature_plan = EmbeddingPlan::provider(
            provider.clone(),
            model.clone(),
            revision,
            deployment,
            dimensions,
            runtime,
        )?;
        Ok(Self {
            metadata: IndexMetadata {
                mode: PROVIDER_EMBEDDING_MODE.into(),
                provider,
                model,
                dimensions: dimensions.unwrap_or_default(),
                signature: signature_plan.provisional_signature(),
                id_scheme: INTELLIGENCE_ID_SCHEME.into(),
            },
            signature_plan: Some(signature_plan),
        })
    }

    pub fn runtime_path(&self) -> Option<&str> {
        self.signature_plan
            .as_ref()
            .map(EmbeddingPlan::runtime_path)
    }

    pub fn metadata(&self) -> &IndexMetadata {
        &self.metadata
    }

    pub fn is_provider(&self) -> bool {
        self.metadata.mode == PROVIDER_EMBEDDING_MODE
    }

    fn matches(&self, stored: &IndexMetadata) -> bool {
        if stored.id_scheme.is_empty() || stored.id_scheme != self.metadata.id_scheme {
            return false;
        }
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
    if stored.replaced_incompatible_index {
        warnings.push(
            "Index compatibility thay đổi (embedding signature hoặc intelligence ID scheme); đã rebuild SQLite/FTS. HNSW được clear/rebuild riêng (không chung transaction)."
                .into(),
        );
    }
    if let Some(error) = stored.hnsw_clear_error.as_ref() {
        warnings.push(format!(
            "HNSW clear lỗi sau SQLite commit ({error}); ANN cũ bị từ chối theo ID scheme, search dùng exact cosine cho đến khi rebuild thành công."
        ));
        // Best-effort retry; failure still leaves scheme-gated exact fallback.
        if let Err(retry_error) = hnsw::clear(&paths.ann_root) {
            warnings.push(format!(
                "HNSW clear retry lỗi ({retry_error}); tiếp tục exact cosine fallback."
            ));
        }
    }
    let id_scheme = stored.metadata.id_scheme.as_str();
    if stored.indexed > 0
        || !hnsw::is_available(
            &paths.ann_root,
            &stored.metadata.signature,
            id_scheme,
            stored.metadata.dimensions,
        )
    {
        match store
            .load_vector_points(stored.metadata.dimensions)
            .and_then(|points| {
                hnsw::rebuild(
                    &paths.ann_root,
                    &stored.metadata.signature,
                    id_scheme,
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
    let query_vector = if !plan.matches(&metadata) {
        warnings.push(
            "Cấu hình index không khớp (embedding signature hoặc intelligence ID scheme); hãy rebuild. Tạm chỉ dùng FTS."
                .into(),
        );
        None
    } else if metadata.mode == PROVIDER_EMBEDDING_MODE {
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
                &metadata.id_scheme,
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
            &metadata.id_scheme,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_paths() -> KnowledgePaths {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("markhand_service_{}_{}", std::process::id(), id));
        KnowledgePaths::new(root.join(".markhand/knowledge.sqlite"), root)
    }

    fn document() -> CorpusDocument {
        CorpusDocument {
            source_rel: "payments.pdf".into(),
            md_rel: "payments.pdf.md".into(),
            format: "pdf".into(),
            markdown: "# Đối soát\n\nGiao dịch được đối soát mỗi ngày.".into(),
        }
    }

    #[test]
    fn provider_signature_change_emits_explicit_rebuild_notice() {
        let paths = temp_paths();
        rebuild_index(
            &paths,
            &[document()],
            &DesktopEmbeddingPlan::local(),
            false,
            |_| Err(KnowledgeError::EmbeddingProviderFailure),
        )
        .unwrap();
        let provider = DesktopEmbeddingPlan::provider(
            "openai-compatible",
            "replacement-model",
            Some("https://embedding.internal/v1"),
            Some(LOCAL_VECTOR_DIMENSIONS),
        )
        .unwrap();
        let result = rebuild_index(&paths, &[document()], &provider, false, |inputs| {
            Ok(inputs
                .iter()
                .map(|input| local_vector(input).into_values())
                .collect())
        })
        .unwrap();
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("rebuild") && warning.contains("ID scheme")));
        let _ = std::fs::remove_dir_all(paths.ann_root);
    }

    #[test]
    fn legacy_local_hash_string_signature_forces_rebuild() {
        let local = DesktopEmbeddingPlan::local();
        assert!(local.signature_plan.is_some());
        assert_ne!(local.metadata().signature, LOCAL_EMBEDDING_MODE);
        // Simulate a pre-schema-v2 store that only stored the mode string.
        let mut legacy = local.metadata().clone();
        legacy.signature = LOCAL_EMBEDDING_MODE.into();
        assert!(!local.matches(&legacy));
    }

    #[test]
    fn missing_or_legacy_id_scheme_forces_rebuild_match_failure() {
        let local = DesktopEmbeddingPlan::local();
        assert_eq!(local.metadata().id_scheme, INTELLIGENCE_ID_SCHEME);
        let mut missing = local.metadata().clone();
        missing.id_scheme.clear();
        assert!(!local.matches(&missing));
        let mut other = local.metadata().clone();
        other.id_scheme = "sip13-v1".into();
        assert!(!local.matches(&other));
        assert!(local.matches(local.metadata()));
    }

    #[test]
    fn hnsw_clear_failure_warns_and_leaves_scheme_gated_exact_fallback() {
        let paths = temp_paths();
        // Seed a legacy (empty-scheme) ANN partition that must become unusable.
        let legacy_points = (0..128)
            .map(|index| {
                let mut vector = vec![0.0; LOCAL_VECTOR_DIMENSIONS];
                vector[index % LOCAL_VECTOR_DIMENSIONS] = 1.0;
                (format!("legacy-chunk-{index}"), vector)
            })
            .collect::<Vec<_>>();
        hnsw::rebuild(
            &paths.ann_root,
            LOCAL_EMBEDDING_MODE,
            "",
            LOCAL_VECTOR_DIMENSIONS,
            &legacy_points,
        )
        .unwrap();
        assert!(hnsw::is_available(
            &paths.ann_root,
            LOCAL_EMBEDDING_MODE,
            "",
            LOCAL_VECTOR_DIMENSIONS
        ));

        let mut store = SqliteKnowledgeStore::open(&paths.database).unwrap();
        let doc = document();
        let metadata = DesktopEmbeddingPlan::local().metadata().clone();
        // Seed SQLite with a legacy empty scheme, then upgrade with failing clear.
        store
            .index_documents(
                &[doc.clone()],
                {
                    let mut legacy = metadata.clone();
                    legacy.id_scheme.clear();
                    legacy.signature = LOCAL_EMBEDDING_MODE.into();
                    legacy
                },
                None,
                |inputs| {
                    Ok(inputs
                        .iter()
                        .map(|input| local_vector(input).into_values())
                        .collect())
                },
                || Ok(()),
            )
            .unwrap();
        let upgraded = store
            .index_documents(
                &[doc],
                metadata.clone(),
                None,
                |inputs| {
                    Ok(inputs
                        .iter()
                        .map(|input| local_vector(input).into_values())
                        .collect())
                },
                || Err(KnowledgeError::AdapterFailure("clear denied".into())),
            )
            .unwrap();
        assert!(upgraded.hnsw_clear_error.is_some());
        assert_eq!(upgraded.metadata.id_scheme, INTELLIGENCE_ID_SCHEME);
        // Stale empty-scheme ANN may still be on disk, but must not be usable
        // under the new SQLite ID scheme (exact cosine self-heals).
        assert!(hnsw::is_available(
            &paths.ann_root,
            LOCAL_EMBEDDING_MODE,
            "",
            LOCAL_VECTOR_DIMENSIONS
        ));
        assert!(!hnsw::is_available(
            &paths.ann_root,
            &metadata.signature,
            INTELLIGENCE_ID_SCHEME,
            metadata.dimensions
        ));
        let _ = std::fs::remove_dir_all(paths.ann_root);
    }

    #[test]
    fn glm_cloud_runtime_is_inferred_from_endpoint_not_provider_enum() {
        let plan = DesktopEmbeddingPlan::provider(
            "openaicompatible",
            "embedding-3",
            Some("https://open.bigmodel.cn/api/paas/v4"),
            Some(1024),
        )
        .unwrap();
        assert_eq!(
            plan.runtime_path(),
            Some(crate::identity::RUNTIME_GLM_CLOUD_INTERIM)
        );
    }

    #[test]
    fn vllm_preset_values_need_explicit_runtime_path() {
        // Real desktop preset: neither host nor model contains "vllm".
        let inferred = DesktopEmbeddingPlan::provider(
            "openaicompatible",
            "BAAI/bge-m3",
            Some("http://127.0.0.1:8000"),
            None,
        )
        .unwrap();
        assert_eq!(
            inferred.runtime_path(),
            Some(crate::identity::RUNTIME_PROVIDER_CLOUD)
        );
        let explicit = DesktopEmbeddingPlan::provider_with_runtime(
            "openaicompatible",
            "BAAI/bge-m3",
            Some("http://127.0.0.1:8000"),
            None,
            Some(crate::identity::RUNTIME_VLLM_LOCAL),
        )
        .unwrap();
        assert_eq!(
            explicit.runtime_path(),
            Some(crate::identity::RUNTIME_VLLM_LOCAL)
        );
    }

    #[test]
    fn provider_revision_is_part_of_index_compatibility() {
        let first = DesktopEmbeddingPlan::provider_with_revision(
            "openaicompatible",
            "AITeamVN/Vietnamese_Embedding",
            "revision-a",
            Some("http://127.0.0.1:8088/v1"),
            Some(1024),
            Some(crate::identity::RUNTIME_LOCAL_NEURAL),
        )
        .unwrap();
        let second = DesktopEmbeddingPlan::provider_with_revision(
            "openaicompatible",
            "AITeamVN/Vietnamese_Embedding",
            "revision-b",
            Some("http://127.0.0.1:8088/v1"),
            Some(1024),
            Some(crate::identity::RUNTIME_LOCAL_NEURAL),
        )
        .unwrap();
        assert_ne!(first.metadata().signature, second.metadata().signature);
    }
}
