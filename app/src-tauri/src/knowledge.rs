use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use fileconv_core::llm::EmbeddingConfig;
use fileconv_knowledge::ask::{
    extractive_answer, grounded_user_prompt, retrieval_context, valid_citation_ids,
    GROUNDED_SYSTEM_PROMPT,
};
use fileconv_knowledge::citation::{
    extract_snippet as snippet, validate_grounded_answer as answer_is_grounded,
};
use fileconv_knowledge::desktop::sqlite::{SqliteKnowledgeStore, StoredChunk};
use fileconv_knowledge::embedding::{
    local_vector as shared_local_vector, EmbeddingPlan as SharedEmbeddingPlan, ProviderDeployment,
    LOCAL_EMBEDDING_MODE, LOCAL_VECTOR_DIMENSIONS, PROVIDER_EMBEDDING_MODE,
};
use fileconv_knowledge::query::{fts5_prefix_query, normalized_tokens};
use fileconv_knowledge::rank::{
    cosine_similarity as cosine, hybrid_rerank_score, sort_hybrid_hits,
};
pub use fileconv_knowledge::types::{
    GroundedAnswer, HybridAskRequest, HybridSearchHit, HybridSearchRequest, HybridSearchResponse,
    IndexBuildResult, IndexMetadata, IndexRequest, IndexStats, SourceAnchor,
};
use tauri::State;

use super::{data_root, es, resolve_within, AppState};

#[derive(Debug, Clone)]
struct EmbeddingPlan {
    config: Option<EmbeddingConfig>,
    metadata: IndexMetadata,
    shared: Option<SharedEmbeddingPlan>,
}

fn index_path(root: &Path) -> Result<PathBuf, String> {
    let markhand = resolve_within(root, ".markhand")?;
    Ok(markhand.join("knowledge.sqlite"))
}

fn local_vector(text: &str) -> Vec<f32> {
    shared_local_vector(text).into_values()
}

fn provider_name(provider: fileconv_core::llm::Provider) -> String {
    format!("{provider:?}").to_ascii_lowercase()
}

fn embedding_plan(config: Option<EmbeddingConfig>) -> EmbeddingPlan {
    match config {
        Some(config) => {
            let provider = provider_name(config.provider);
            let model = config.model.clone();
            let deployment = ProviderDeployment::from_base_url(config.base_url.as_deref())
                .or_else(|_| ProviderDeployment::from_base_url(None))
                .expect("default deployment identity is valid");
            let shared = SharedEmbeddingPlan::provider(
                provider.clone(),
                model.clone(),
                model.clone(),
                deployment,
                config.dimensions,
            )
            .expect("validated desktop embedding configuration");
            let signature = shared.provisional_signature();
            EmbeddingPlan {
                metadata: IndexMetadata {
                    mode: PROVIDER_EMBEDDING_MODE.into(),
                    provider,
                    model,
                    dimensions: config.dimensions.unwrap_or_default(),
                    signature,
                },
                config: Some(config),
                shared: Some(shared),
            }
        }
        None => EmbeddingPlan {
            config: None,
            metadata: IndexMetadata {
                mode: LOCAL_EMBEDDING_MODE.into(),
                provider: "local".into(),
                model: LOCAL_EMBEDDING_MODE.into(),
                dimensions: LOCAL_VECTOR_DIMENSIONS,
                signature: LOCAL_EMBEDDING_MODE.into(),
            },
            // Existing desktop indexes retain their legacy signature. New server
            // consumers use the durable signature exposed by the shared plan.
            shared: None,
        },
    }
}

fn finalize_provider_signature(plan: &mut EmbeddingPlan) -> Result<(), String> {
    if let Some(shared) = plan.shared.as_ref() {
        plan.metadata.signature = shared
            .signature(plan.metadata.dimensions)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn provider_config_matches(config: &EmbeddingConfig, metadata: &IndexMetadata) -> bool {
    let mut plan = embedding_plan(Some(config.clone()));
    if plan.metadata.dimensions == 0 && metadata.dimensions > 0 {
        plan.metadata.dimensions = metadata.dimensions;
    }
    (plan.metadata.dimensions == 0 || finalize_provider_signature(&mut plan).is_ok())
        && plan.metadata.signature == metadata.signature
}

fn index_documents_with_plan(
    root: &Path,
    source_rels: &[String],
    plan: EmbeddingPlan,
) -> fileconv_knowledge::Result<IndexBuildResult> {
    let documents = super::intelligence::load_documents(root, source_rels)
        .map_err(fileconv_knowledge::KnowledgeError::AdapterFailure)?;
    let path = index_path(root).map_err(fileconv_knowledge::KnowledgeError::AdapterFailure)?;
    let mut store = SqliteKnowledgeStore::open(path)?;
    let config = plan.config;
    let result = store.index_documents(
        &documents,
        plan.metadata,
        plan.shared.as_ref(),
        |inputs| {
            if let Some(config) = config.as_ref() {
                fileconv_core::llm::embed_batch(config, inputs)
                    .map_err(|_| fileconv_knowledge::KnowledgeError::EmbeddingProviderFailure)
            } else {
                Ok(inputs.iter().map(|input| local_vector(input)).collect())
            }
        },
        || {
            super::vector_index::clear(root)
                .map_err(fileconv_knowledge::KnowledgeError::AdapterFailure)
        },
    )?;
    let mut warnings = Vec::new();
    if result.indexed > 0
        || !super::vector_index::is_available(
            root,
            &result.metadata.signature,
            result.metadata.dimensions,
        )
    {
        match store
            .load_vector_points(result.metadata.dimensions)
            .map_err(|error| error.to_string())
            .and_then(|points| {
                super::vector_index::rebuild(
                    root,
                    &result.metadata.signature,
                    result.metadata.dimensions,
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
        documents: result.documents,
        chunks: result.chunks,
        indexed: result.indexed,
        skipped: result.skipped,
        embedding_mode: result.metadata.mode,
        embedding_provider: result.metadata.provider,
        embedding_model: result.metadata.model,
        vector_dimensions: result.metadata.dimensions,
        warnings,
    })
}

fn index_documents_inner(
    root: &Path,
    source_rels: &[String],
    config: Option<EmbeddingConfig>,
    fallback_local: bool,
) -> Result<IndexBuildResult, String> {
    let provider_requested = config.is_some();
    match index_documents_with_plan(root, source_rels, embedding_plan(config)) {
        Ok(result) => Ok(result),
        Err(fileconv_knowledge::KnowledgeError::EmbeddingProviderFailure)
            if provider_requested && fallback_local =>
        {
            let mut result = index_documents_with_plan(root, source_rels, embedding_plan(None))
                .map_err(|error| error.to_string())?;
            result.warnings.push(
                "embedding provider lỗi; đã rebuild toàn bộ scope bằng local hash offline.".into(),
            );
            Ok(result)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn hybrid_search_inner(
    root: &Path,
    source_rels: &[String],
    query: &str,
    limit: usize,
    config: Option<EmbeddingConfig>,
    fallback_local: bool,
) -> Result<HybridSearchResponse, String> {
    if query.trim().is_empty() {
        return Ok(HybridSearchResponse {
            hits: Vec::new(),
            warnings: Vec::new(),
            embedding_mode: LOCAL_EMBEDDING_MODE.into(),
        });
    }
    if !source_rels.is_empty() {
        index_documents_inner(root, source_rels, config.clone(), fallback_local)?;
    }
    let store = SqliteKnowledgeStore::open(index_path(root)?).map_err(|error| error.to_string())?;
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    let scope: HashSet<String> = source_rels.iter().cloned().collect();
    let query_tokens = normalized_tokens(query);
    let fts = fts5_prefix_query(query);
    let lexical_rank = store
        .lexical_ranks(&fts, &scope, 250)
        .map_err(|error| error.to_string())?;
    let chunks = store
        .load_chunks(&scope, metadata.dimensions)
        .map_err(|error| error.to_string())?;
    let mut warnings = Vec::new();
    let query_vector = if metadata.mode == PROVIDER_EMBEDDING_MODE {
        match config {
            Some(config) if provider_config_matches(&config, &metadata) => {
                match fileconv_core::llm::embed_query(&config, query) {
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
                        warnings.push("Embedding provider lỗi; chỉ dùng FTS lexical.".to_string());
                        None
                    }
                }
            }
            _ => {
                warnings.push(
                    "Cấu hình embedding không khớp index; hãy rebuild. Tạm chỉ dùng FTS.".into(),
                );
                None
            }
        }
    } else {
        Some(local_vector(query))
    };
    let scoped_ids: HashSet<&str> = chunks.iter().map(|chunk| chunk.id.as_str()).collect();
    let mut vector_order: Vec<(String, f32)> = if let Some(query_vector) = query_vector.as_ref() {
        if chunks.len() > 1_000 {
            match super::vector_index::search(
                root,
                &metadata.signature,
                metadata.dimensions,
                query_vector,
                (chunks.len() * 4).clamp(500, 5_000),
            ) {
                Ok(candidates) => {
                    let scoped: Vec<_> = candidates
                        .into_iter()
                        .filter(|(id, _)| scoped_ids.contains(id.as_str()))
                        .collect();
                    if scoped.len() >= 20.min(chunks.len()) {
                        scoped
                    } else {
                        warnings.push(
                            "HNSW trả quá ít candidate trong scope; dùng exact cosine.".into(),
                        );
                        chunks
                            .iter()
                            .map(|chunk| (chunk.id.clone(), cosine(query_vector, &chunk.vector)))
                            .collect()
                    }
                }
                Err(error) => {
                    warnings.push(format!("HNSW chưa dùng được ({error}); dùng exact cosine."));
                    chunks
                        .iter()
                        .map(|chunk| (chunk.id.clone(), cosine(query_vector, &chunk.vector)))
                        .collect()
                }
            }
        } else {
            chunks
                .iter()
                .map(|chunk| (chunk.id.clone(), cosine(query_vector, &chunk.vector)))
                .collect()
        }
    } else {
        Vec::new()
    };
    vector_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let vector_rank: HashMap<String, (usize, f32)> = vector_order
        .into_iter()
        .take(500)
        .enumerate()
        .map(|(rank, (id, score))| (id, (rank, score)))
        .collect();

    let by_id: HashMap<&str, &StoredChunk> = chunks
        .iter()
        .map(|chunk| (chunk.id.as_str(), chunk))
        .collect();
    let candidate_ids: HashSet<String> = lexical_rank
        .keys()
        .chain(vector_rank.keys())
        .cloned()
        .collect();
    let mut results = Vec::new();
    for id in candidate_ids {
        let Some(chunk) = by_id.get(id.as_str()) else {
            continue;
        };
        let (lex_rank, lex_score) = lexical_rank.get(&id).copied().unwrap_or((usize::MAX, 0.0));
        let (vec_rank, vec_score) = vector_rank.get(&id).copied().unwrap_or((usize::MAX, 0.0));
        let rerank_score = hybrid_rerank_score(
            (lex_rank != usize::MAX).then_some(lex_rank),
            (vec_rank != usize::MAX).then_some(vec_rank),
            vec_score,
            &query_tokens,
            &chunk.heading,
            &chunk.body,
        );
        results.push(HybridSearchHit {
            chunk_id: chunk.id.clone(),
            source_rel: chunk.source_rel.clone(),
            md_rel: chunk.md_rel.clone(),
            heading: chunk.heading.clone(),
            snippet: snippet(&chunk.body, &query_tokens),
            lexical_score: lex_score,
            vector_score: vec_score,
            rerank_score,
            anchor: SourceAnchor {
                page: chunk.page,
                slide: chunk.slide,
                sheet: chunk.sheet.clone(),
                start: chunk.start,
                end: chunk.end,
            },
        });
    }
    sort_hybrid_hits(&mut results);
    results.truncate(limit.max(1));
    Ok(HybridSearchResponse {
        hits: results,
        warnings,
        embedding_mode: metadata.mode,
    })
}

#[tauri::command]
pub async fn rebuild_knowledge_index(
    state: State<'_, AppState>,
    req: IndexRequest,
) -> Result<IndexBuildResult, String> {
    let root = data_root(&state);
    let settings = state.settings.lock().map_err(|_| "lock lỗi")?.clone();
    let (embedding_config, config_warning) = match settings.embedding_config() {
        Ok(config) => (config, None),
        Err(error) if settings.embedding_fallback_local => (None, Some(error)),
        Err(error) => return Err(error),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut result = index_documents_inner(
            &root,
            &req.source_rels,
            embedding_config,
            settings.embedding_fallback_local,
        )?;
        if let Some(warning) = config_warning {
            result.warnings.push(format!(
                "Cấu hình embedding chưa dùng được ({warning}); đã dùng local hash."
            ));
        }
        Ok(result)
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub fn knowledge_index_stats(state: State<AppState>) -> Result<IndexStats, String> {
    let root = data_root(&state);
    let store =
        SqliteKnowledgeStore::open(index_path(&root)?).map_err(|error| error.to_string())?;
    let documents = store.document_count().map_err(|error| error.to_string())?;
    let chunks = store.chunk_count().map_err(|error| error.to_string())?;
    let database_bytes = store.database_bytes();
    let metadata = store.metadata().map_err(|error| error.to_string())?;
    Ok(IndexStats {
        documents,
        chunks,
        database_bytes,
        vector_dimensions: metadata.dimensions,
        embedding_mode: metadata.mode,
        embedding_provider: metadata.provider,
        embedding_model: metadata.model,
        ann_available: super::vector_index::is_available(
            &root,
            &metadata.signature,
            metadata.dimensions,
        ),
        ann_threshold: 1_000,
    })
}

#[tauri::command]
pub async fn hybrid_search(
    state: State<'_, AppState>,
    req: HybridSearchRequest,
) -> Result<HybridSearchResponse, String> {
    let root = data_root(&state);
    let settings = state.settings.lock().map_err(|_| "lock lỗi")?.clone();
    let (embedding_config, config_warning) = match settings.embedding_config() {
        Ok(config) => (config, None),
        Err(error) if settings.embedding_fallback_local => (None, Some(error)),
        Err(error) => return Err(error),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut response = hybrid_search_inner(
            &root,
            &req.source_rels,
            &req.query,
            req.limit.unwrap_or(20),
            embedding_config,
            settings.embedding_fallback_local,
        )?;
        if let Some(warning) = config_warning {
            response.warnings.push(format!(
                "Cấu hình embedding chưa dùng được ({warning}); đã dùng local hash."
            ));
        }
        Ok(response)
    })
    .await
    .map_err(es)?
}

fn hybrid_ask_inner(
    root: &Path,
    req: HybridAskRequest,
    llm_config: Option<fileconv_core::llm::LlmConfig>,
    config_warning: Option<String>,
    embedding_config: Option<EmbeddingConfig>,
    embedding_fallback_local: bool,
    embedding_warning: Option<String>,
) -> Result<GroundedAnswer, String> {
    let search = hybrid_search_inner(
        root,
        &req.source_rels,
        &req.question,
        req.top_k.unwrap_or(8),
        embedding_config,
        embedding_fallback_local,
    )?;
    let hits = search.hits;
    let mut retrieval_warnings = search.warnings;
    if let Some(warning) = embedding_warning {
        retrieval_warnings.push(format!(
            "Cấu hình embedding chưa dùng được ({warning}); đã dùng local hash."
        ));
    }
    let fallback = extractive_answer(&req.question, &hits);
    if !req.use_llm.unwrap_or(false) {
        return Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "offline_extractive".into(),
            grounded: true,
            warnings: retrieval_warnings,
        });
    }
    let Some(config) = llm_config else {
        let warning = config_warning
            .map(|error| {
                format!("Cấu hình LLM chưa dùng được ({error}); đã fallback extractive local.")
            })
            .unwrap_or_else(|| {
                "Chưa cấu hình LLM provider; đã dùng câu trả lời extractive local.".into()
            });
        return Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "fallback_extractive".into(),
            grounded: true,
            warnings: {
                retrieval_warnings.push(warning);
                retrieval_warnings
            },
        });
    };
    if hits.is_empty() {
        return Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "fallback_extractive".into(),
            grounded: true,
            warnings: {
                retrieval_warnings.push("Không đủ nguồn để gọi LLM.".into());
                retrieval_warnings
            },
        });
    }
    let context = retrieval_context(&hits);
    let prompt = grounded_user_prompt(&req.question, &context);
    let llm_answer = match fileconv_core::llm::chat(&config, GROUNDED_SYSTEM_PROMPT, &prompt) {
        Ok(answer) => answer,
        Err(error) => {
            return Ok(GroundedAnswer {
                answer: fallback,
                citations: hits,
                mode: "fallback_extractive".into(),
                grounded: true,
                warnings: {
                    retrieval_warnings.push(format!(
                        "LLM provider lỗi ({error}); đã fallback extractive local."
                    ));
                    retrieval_warnings
                },
            });
        }
    };
    let valid_ids = valid_citation_ids(hits.len());
    match answer_is_grounded(&llm_answer, &valid_ids) {
        Ok(()) => {
            let local = config
                .base_url
                .as_deref()
                .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));
            Ok(GroundedAnswer {
                answer: llm_answer,
                citations: hits,
                mode: if config.is_subscription_cli() {
                    "subscription_cli".into()
                } else if local {
                    "local_llm".into()
                } else {
                    "cloud_llm".into()
                },
                grounded: true,
                warnings: retrieval_warnings,
            })
        }
        Err(mut warnings) => Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "fallback_extractive".into(),
            grounded: true,
            warnings: {
                retrieval_warnings.append(&mut warnings);
                retrieval_warnings
            },
        }),
    }
}

#[tauri::command]
pub async fn hybrid_ask(
    state: State<'_, AppState>,
    req: HybridAskRequest,
) -> Result<GroundedAnswer, String> {
    let root = data_root(&state);
    let (llm_config, config_warning) = if req.use_llm.unwrap_or(false) {
        match state.settings.lock().map_err(|_| "lock lỗi")?.llm_config() {
            Ok(config) => (config, None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let settings = state.settings.lock().map_err(|_| "lock lỗi")?.clone();
    let (embedding_config, embedding_warning) = match settings.embedding_config() {
        Ok(config) => (config, None),
        Err(error) if settings.embedding_fallback_local => (None, Some(error)),
        Err(error) => return Err(error),
    };
    tauri::async_runtime::spawn_blocking(move || {
        hybrid_ask_inner(
            &root,
            req,
            llm_config,
            config_warning,
            embedding_config,
            settings.embedding_fallback_local,
            embedding_warning,
        )
    })
    .await
    .map_err(es)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> PathBuf {
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "markhand_knowledge_{}_{}",
            std::process::id(),
            count
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn seed(root: &Path) -> Vec<String> {
        std::fs::write(root.join("payments.pdf"), b"%PDF").unwrap();
        std::fs::write(
            root.join("payments.pdf.md"),
            "# Đối soát\n\nHệ thống phải đối chiếu giao dịch với đối tác mỗi ngày.\n",
        )
        .unwrap();
        std::fs::write(root.join("security.docx"), b"PK").unwrap();
        std::fs::write(
            root.join("security.docx.md"),
            "# Bảo mật\n\nMọi API phải có xác thực và nhật ký kiểm toán.\n",
        )
        .unwrap();
        vec!["payments.pdf".into(), "security.docx".into()]
    }

    fn mock_embedding_server(requests: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut captured = Vec::new();
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0u8; 32 * 1024];
                let size = stream.read(&mut request).unwrap();
                captured.push(String::from_utf8_lossy(&request[..size]).to_string());
                let body = r#"{"data":[{"index":0,"embedding":[1.0,0.5,0.25]}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
            captured
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn local_vectors_are_normalized_and_deterministic() {
        let first = local_vector("đối soát giao dịch");
        let second = local_vector("đối soát giao dịch");
        assert_eq!(first, second);
        let norm = first.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[test]
    fn sqlite_index_is_incremental_and_persistent() {
        let root = temp_root();
        let sources = seed(&root);
        let first = index_documents_inner(&root, &sources, None, true).unwrap();
        let second = index_documents_inner(&root, &sources, None, true).unwrap();
        assert_eq!(first.indexed, 2);
        assert_eq!(second.skipped, 2);
        let store = SqliteKnowledgeStore::open(index_path(&root).unwrap()).unwrap();
        assert_eq!(store.chunk_count().unwrap(), 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hybrid_search_ranks_relevant_document() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources, None, true).unwrap();
        let hits = hybrid_search_inner(&root, &sources, "đối soát giao dịch", 5, None, true)
            .unwrap()
            .hits;
        assert_eq!(hits[0].source_rel, "payments.pdf");
        assert!(hits[0].rerank_score > 0.0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_filters_search_results() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources, None, true).unwrap();
        let hits = hybrid_search_inner(
            &root,
            &["security.docx".into()],
            "giao dịch API",
            10,
            None,
            true,
        )
        .unwrap()
        .hits;
        assert!(hits.iter().all(|hit| hit.source_rel == "security.docx"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn grounded_validator_rejects_missing_and_fake_citations() {
        let valid = HashSet::from(["CITE-0001".to_string()]);
        assert!(answer_is_grounded(
            "Nội dung đủ dài nhưng không hề có citation nào ở cuối đoạn để kiểm tra.",
            &valid
        )
        .is_err());
        assert!(answer_is_grounded(
            "Nội dung factual đủ dài và có citation giả không hợp lệ ở cuối. [CITE-9999]",
            &valid
        )
        .is_err());
        assert!(answer_is_grounded(
            "Nội dung factual đủ dài, được hỗ trợ bởi nguồn đã retrieval. [CITE-0001]",
            &valid
        )
        .is_ok());
    }

    #[test]
    fn extractive_answer_always_cites_hits() {
        let root = temp_root();
        let sources = seed(&root);
        let hits = hybrid_search_inner(&root, &sources, "xác thực API", 3, None, true)
            .unwrap()
            .hits;
        let answer = extractive_answer("API bảo mật thế nào?", &hits);
        assert!(answer.contains("[CITE-0001]"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_ask_never_requires_an_llm() {
        let root = temp_root();
        let sources = seed(&root);
        let result = hybrid_ask_inner(
            &root,
            HybridAskRequest {
                source_rels: sources,
                question: "Đối soát khi nào?".into(),
                top_k: Some(3),
                use_llm: Some(false),
            },
            None,
            None,
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(result.mode, "offline_extractive");
        assert!(result.grounded);
        assert!(result.warnings.is_empty());
        assert!(result.answer.contains("[CITE-0001]"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_llm_configuration_falls_back_instead_of_failing() {
        let root = temp_root();
        let sources = seed(&root);
        let result = hybrid_ask_inner(
            &root,
            HybridAskRequest {
                source_rels: sources,
                question: "API được bảo vệ thế nào?".into(),
                top_k: Some(3),
                use_llm: Some(true),
            },
            None,
            Some("thiếu API key".into()),
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(result.mode, "fallback_extractive");
        assert!(result.grounded);
        assert!(result.warnings[0].contains("thiếu API key"));
        assert!(result.warnings[0].contains("fallback"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unavailable_llm_provider_falls_back_instead_of_failing() {
        let root = temp_root();
        let sources = seed(&root);
        let config = fileconv_core::llm::LlmConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "",
            "local-test",
            Some("http://127.0.0.1:1".into()),
        )
        .unwrap();
        let result = hybrid_ask_inner(
            &root,
            HybridAskRequest {
                source_rels: sources,
                question: "Đối soát giao dịch thế nào?".into(),
                top_k: Some(3),
                use_llm: Some(true),
            },
            Some(config),
            None,
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(result.mode, "fallback_extractive");
        assert!(result.warnings[0].contains("LLM provider lỗi"));
        assert!(result.answer.contains("[CITE-0001]"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn changed_markdown_replaces_old_chunks() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources, None, true).unwrap();
        std::fs::write(
            root.join("payments.pdf.md"),
            "# Hoàn tiền\n\nGiao dịch hoàn tiền phải được duyệt bởi hai người.\n",
        )
        .unwrap();
        let update = index_documents_inner(&root, &["payments.pdf".into()], None, true).unwrap();
        assert_eq!(update.indexed, 1);
        let hits = hybrid_search_inner(
            &root,
            &["payments.pdf".into()],
            "hai người duyệt",
            5,
            None,
            true,
        )
        .unwrap()
        .hits;
        assert!(hits[0].snippet.contains("hai người"));
        let store = SqliteKnowledgeStore::open(index_path(&root).unwrap()).unwrap();
        let scope = HashSet::from(["payments.pdf".to_string()]);
        assert_eq!(
            store
                .load_chunks(&scope, LOCAL_VECTOR_DIMENSIONS)
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn page_comments_become_exact_source_anchors() {
        let root = temp_root();
        std::fs::write(root.join("spec.pdf"), b"%PDF").unwrap();
        std::fs::write(
            root.join("spec.pdf.md"),
            "<!-- Page 7 -->\n\n# Thanh toán\n\nCho phép thanh toán QR.\n",
        )
        .unwrap();
        let hits = hybrid_search_inner(&root, &["spec.pdf".into()], "thanh toán QR", 3, None, true)
            .unwrap()
            .hits;
        assert_eq!(hits[0].anchor.page, Some(7));
        assert!(hits[0].anchor.end > hits[0].anchor.start);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn punctuation_cannot_break_fts_query_syntax() {
        let root = temp_root();
        let sources = seed(&root);
        let hits = hybrid_search_inner(
            &root,
            &sources,
            "API: \"xác thực\" OR (giao dịch)",
            5,
            None,
            true,
        )
        .unwrap()
        .hits;
        assert!(!hits.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unavailable_embedding_provider_rebuilds_whole_scope_locally() {
        let root = temp_root();
        let sources = seed(&root);
        let config = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "provider-secret",
            "missing-model",
            Some("http://user:password@127.0.0.1:1?token=hidden".into()),
            None,
        )
        .unwrap();
        let result = index_documents_inner(&root, &sources, Some(config), true).unwrap();
        assert_eq!(result.embedding_mode, LOCAL_EMBEDDING_MODE);
        assert_eq!(result.vector_dimensions, LOCAL_VECTOR_DIMENSIONS);
        assert_eq!(result.indexed, 2);
        assert!(result.warnings[0].contains("rebuild"));
        assert!(!result.warnings[0].contains("password"));
        assert!(!result.warnings[0].contains("hidden"));
        assert!(!result.warnings[0].contains("provider-secret"));
        let metadata = SqliteKnowledgeStore::open(index_path(&root).unwrap())
            .unwrap()
            .metadata()
            .unwrap();
        assert_eq!(metadata.signature, LOCAL_EMBEDDING_MODE);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_signature_excludes_transport_url_and_credentials() {
        let first = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "first-secret",
            "mock-embedding",
            Some("https://user:password@embedding.example/v1?token=hidden".into()),
            Some(768),
        )
        .unwrap();
        let same_deployment = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "second-secret",
            "mock-embedding",
            Some("https://embedding.example/v1".into()),
            Some(768),
        )
        .unwrap();
        let other_deployment = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "second-secret",
            "mock-embedding",
            Some("https://embedding-two.example/v1".into()),
            Some(768),
        )
        .unwrap();
        let first_signature = embedding_plan(Some(first)).metadata.signature;
        let same_signature = embedding_plan(Some(same_deployment)).metadata.signature;
        let other_signature = embedding_plan(Some(other_deployment)).metadata.signature;
        assert_eq!(first_signature, same_signature);
        assert_ne!(first_signature, other_signature);
        assert!(!first_signature.contains("password"));
        assert!(!first_signature.contains("hidden"));
    }

    #[test]
    fn unknown_provider_dimensions_remain_recoverable_for_empty_documents() {
        let root = temp_root();
        std::fs::write(root.join("empty.txt"), b"").unwrap();
        std::fs::write(root.join("empty.txt.md"), b"").unwrap();
        let config = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "",
            "mock-embedding",
            Some("http://127.0.0.1:1".into()),
            None,
        )
        .unwrap();
        let sources = vec!["empty.txt".to_string()];
        let first = index_documents_inner(&root, &sources, Some(config.clone()), false).unwrap();
        let second = index_documents_inner(&root, &sources, Some(config), false).unwrap();
        assert_eq!(first.vector_dimensions, 0);
        assert_eq!(first.chunks, 0);
        assert_eq!(second.skipped, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_embedding_metadata_persists_and_drives_query_vector() {
        let root = temp_root();
        let sources = seed(&root);
        let (base_url, server) = mock_embedding_server(2);
        let config = EmbeddingConfig::new(
            fileconv_core::llm::Provider::OpenAiCompatible,
            "",
            "mock-embedding",
            Some(base_url),
            None,
        )
        .unwrap();
        let result =
            index_documents_inner(&root, &sources[..1], Some(config.clone()), false).unwrap();
        assert_eq!(result.embedding_mode, PROVIDER_EMBEDDING_MODE);
        assert_eq!(result.vector_dimensions, 3);
        let search =
            hybrid_search_inner(&root, &sources[..1], "đối soát", 5, Some(config), false).unwrap();
        assert_eq!(search.embedding_mode, PROVIDER_EMBEDDING_MODE);
        assert!(search.warnings.is_empty());
        assert!(!search.hits.is_empty());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests
            .iter()
            .all(|request| request.starts_with("POST /v1/embeddings ")));
        std::fs::remove_dir_all(root).ok();
    }
}
