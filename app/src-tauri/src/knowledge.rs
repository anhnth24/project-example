use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use fileconv_core::intelligence::{self, CorpusDocument};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::{data_root, es, resolve_within, AppState};

const VECTOR_DIMENSIONS: usize = 256;
const MAX_VECTOR_CANDIDATES: usize = 100_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRequest {
    pub source_rels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexBuildResult {
    pub documents: usize,
    pub chunks: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub embedding_mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub documents: usize,
    pub chunks: usize,
    pub database_bytes: u64,
    pub vector_dimensions: usize,
    pub embedding_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchRequest {
    pub source_rels: Vec<String>,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAnchor {
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchHit {
    pub chunk_id: String,
    pub source_rel: String,
    pub md_rel: String,
    pub heading: String,
    pub snippet: String,
    pub lexical_score: f32,
    pub vector_score: f32,
    pub rerank_score: f32,
    pub anchor: SourceAnchor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridAskRequest {
    pub source_rels: Vec<String>,
    pub question: String,
    pub top_k: Option<usize>,
    pub use_llm: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundedAnswer {
    pub answer: String,
    pub citations: Vec<HybridSearchHit>,
    pub mode: String,
    pub grounded: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedChunk {
    id: String,
    source_rel: String,
    md_rel: String,
    heading: String,
    body: String,
    start: usize,
    end: usize,
    page: Option<u32>,
    slide: Option<u32>,
    sheet: Option<String>,
    vector: Vec<f32>,
}

fn stable_hash(value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn index_path(root: &Path) -> Result<PathBuf, String> {
    let markhand = resolve_within(root, ".markhand")?;
    Ok(markhand.join("knowledge.sqlite"))
}

fn open_index(root: &Path) -> Result<Connection, String> {
    let path = index_path(root)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(es)?;
    }
    let connection = Connection::open(path).map_err(es)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(es)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS documents (
               doc_rel TEXT PRIMARY KEY,
               md_rel TEXT NOT NULL,
               content_hash TEXT NOT NULL,
               format TEXT NOT NULL,
               chunks INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS chunks (
               chunk_id TEXT PRIMARY KEY,
               doc_rel TEXT NOT NULL,
               md_rel TEXT NOT NULL,
               heading TEXT NOT NULL,
               body TEXT NOT NULL,
               start_offset INTEGER NOT NULL,
               end_offset INTEGER NOT NULL,
               page INTEGER,
               slide INTEGER,
               sheet TEXT,
               vector BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS chunks_doc_rel ON chunks(doc_rel);
             CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
               chunk_id UNINDEXED,
               doc_rel UNINDEXED,
               heading,
               body,
               folded,
               tokenize='unicode61 remove_diacritics 2'
             );",
        )
        .map_err(es)?;
    Ok(connection)
}

fn normalized_tokens(text: &str) -> Vec<String> {
    intelligence::normalize_search_text(text)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

fn hash_index(value: &str) -> (usize, f32) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    let hash = hasher.finish();
    let index = (hash as usize) % VECTOR_DIMENSIONS;
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    (index, sign)
}

/// Fully local feature-hashing vector. It is always available and provides a
/// deterministic vector fallback when no embedding provider is configured.
fn local_vector(text: &str) -> Vec<f32> {
    let folded = intelligence::normalize_search_text(text);
    let mut vector = vec![0.0_f32; VECTOR_DIMENSIONS];
    let words = normalized_tokens(&folded);
    for token in &words {
        let (index, sign) = hash_index(token);
        vector[index] += sign;
    }
    for pair in words.windows(2) {
        let bigram = format!("{}:{}", pair[0], pair[1]);
        let (index, sign) = hash_index(&bigram);
        vector[index] += sign * 0.65;
    }
    let compact: Vec<char> = folded
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    for trigram in compact.windows(3) {
        let feature: String = trigram.iter().collect();
        let (index, sign) = hash_index(&feature);
        vector[index] += sign * 0.15;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn vector_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn infer_anchor(document: &CorpusDocument, chunk: &intelligence::CorpusChunk) -> SourceAnchor {
    let folded = intelligence::normalize_search_text(&chunk.heading);
    let slide = folded
        .split("slide ")
        .nth(1)
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok());
    let sheet = (document.format == "xlsx")
        .then(|| chunk.heading.split(" > ").last().unwrap_or("").to_string())
        .filter(|value| !value.is_empty());
    SourceAnchor {
        page: chunk.page,
        slide,
        sheet,
        start: chunk.start,
        end: chunk.end,
    }
}

fn index_documents_inner(root: &Path, source_rels: &[String]) -> Result<IndexBuildResult, String> {
    let documents = super::intelligence::load_documents(root, source_rels)?;
    let mut connection = open_index(root)?;
    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut total_chunks = 0usize;

    for document in &documents {
        let content_hash = stable_hash(&document.markdown);
        let existing: Option<String> = connection
            .query_row(
                "SELECT content_hash FROM documents WHERE doc_rel = ?1",
                params![document.source_rel],
                |row| row.get(0),
            )
            .optional()
            .map_err(es)?;
        if existing.as_deref() == Some(content_hash.as_str()) {
            let count: usize = connection
                .query_row(
                    "SELECT COUNT(*) FROM chunks WHERE doc_rel = ?1",
                    params![document.source_rel],
                    |row| row.get(0),
                )
                .map_err(es)?;
            total_chunks += count;
            skipped += 1;
            continue;
        }

        let chunks = intelligence::build_corpus(std::slice::from_ref(document), 2_000);
        let transaction = connection.transaction().map_err(es)?;
        transaction
            .execute(
                "DELETE FROM chunks_fts WHERE doc_rel = ?1",
                params![document.source_rel],
            )
            .map_err(es)?;
        transaction
            .execute(
                "DELETE FROM chunks WHERE doc_rel = ?1",
                params![document.source_rel],
            )
            .map_err(es)?;
        for chunk in &chunks {
            let vector = local_vector(&format!("{}\n{}", chunk.heading, chunk.text));
            let anchor = infer_anchor(document, chunk);
            transaction
                .execute(
                    "INSERT INTO chunks (
                       chunk_id, doc_rel, md_rel, heading, body, start_offset,
                       end_offset, page, slide, sheet, vector
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        chunk.id,
                        chunk.source_rel,
                        chunk.md_rel,
                        chunk.heading,
                        chunk.text,
                        chunk.start as i64,
                        chunk.end as i64,
                        anchor.page,
                        anchor.slide,
                        anchor.sheet,
                        vector_bytes(&vector),
                    ],
                )
                .map_err(es)?;
            transaction
                .execute(
                    "INSERT INTO chunks_fts (chunk_id, doc_rel, heading, body, folded)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        chunk.id,
                        chunk.source_rel,
                        chunk.heading,
                        chunk.text,
                        intelligence::normalize_search_text(&format!(
                            "{} {}",
                            chunk.heading, chunk.text
                        )),
                    ],
                )
                .map_err(es)?;
        }
        transaction
            .execute(
                "INSERT INTO documents (doc_rel, md_rel, content_hash, format, chunks)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(doc_rel) DO UPDATE SET
                   md_rel=excluded.md_rel,
                   content_hash=excluded.content_hash,
                   format=excluded.format,
                   chunks=excluded.chunks",
                params![
                    document.source_rel,
                    document.md_rel,
                    content_hash,
                    document.format,
                    chunks.len() as i64,
                ],
            )
            .map_err(es)?;
        transaction.commit().map_err(es)?;
        total_chunks += chunks.len();
        indexed += 1;
    }

    Ok(IndexBuildResult {
        documents: documents.len(),
        chunks: total_chunks,
        indexed,
        skipped,
        embedding_mode: "local_hash_v1".into(),
    })
}

fn load_all_chunks(
    connection: &Connection,
    scope: &HashSet<String>,
) -> Result<Vec<IndexedChunk>, String> {
    let mut statement = connection
        .prepare(
            "SELECT chunk_id, doc_rel, md_rel, heading, body, start_offset,
                    end_offset, page, slide, sheet, vector
             FROM chunks",
        )
        .map_err(es)?;
    let rows = statement
        .query_map([], |row| {
            let vector: Vec<u8> = row.get(10)?;
            Ok(IndexedChunk {
                id: row.get(0)?,
                source_rel: row.get(1)?,
                md_rel: row.get(2)?,
                heading: row.get(3)?,
                body: row.get(4)?,
                start: row.get::<_, i64>(5)? as usize,
                end: row.get::<_, i64>(6)? as usize,
                page: row.get(7)?,
                slide: row.get(8)?,
                sheet: row.get(9)?,
                vector: vector_from_bytes(&vector),
            })
        })
        .map_err(es)?;
    let mut chunks = Vec::new();
    for row in rows {
        let chunk = row.map_err(es)?;
        if scope.is_empty() || scope.contains(&chunk.source_rel) {
            chunks.push(chunk);
            if chunks.len() > MAX_VECTOR_CANDIDATES {
                return Err(format!(
                    "vector candidate vượt giới hạn {MAX_VECTOR_CANDIDATES}; hãy scope theo project/folder"
                ));
            }
        }
    }
    Ok(chunks)
}

fn fts_query(query: &str) -> String {
    normalized_tokens(query)
        .into_iter()
        .map(|token| format!("\"{}\"*", token.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn snippet(body: &str, query_tokens: &[String]) -> String {
    let words: Vec<&str> = body.split_whitespace().collect();
    let folded_words: Vec<String> = words
        .iter()
        .map(|word| intelligence::normalize_search_text(word))
        .collect();
    let match_index = folded_words
        .iter()
        .position(|word| query_tokens.iter().any(|token| word.contains(token)))
        .unwrap_or(0);
    let start = match_index.saturating_sub(12);
    let end = (start + 56).min(words.len());
    words[start..end].join(" ")
}

fn hybrid_search_inner(
    root: &Path,
    source_rels: &[String],
    query: &str,
    limit: usize,
) -> Result<Vec<HybridSearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    if !source_rels.is_empty() {
        index_documents_inner(root, source_rels)?;
    }
    let connection = open_index(root)?;
    let scope: HashSet<String> = source_rels.iter().cloned().collect();
    let query_tokens = normalized_tokens(query);
    let fts = fts_query(query);
    let mut lexical_rank: HashMap<String, (usize, f32)> = HashMap::new();
    if !fts.is_empty() {
        let mut statement = connection
            .prepare(
                "SELECT chunk_id, doc_rel, bm25(chunks_fts)
                 FROM chunks_fts WHERE chunks_fts MATCH ?1
                 ORDER BY bm25(chunks_fts) LIMIT 250",
            )
            .map_err(es)?;
        let rows = statement
            .query_map(params![fts], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })
            .map_err(es)?;
        let mut rank = 0usize;
        for row in rows {
            let (id, doc_rel, bm25) = row.map_err(es)?;
            if !scope.is_empty() && !scope.contains(&doc_rel) {
                continue;
            }
            lexical_rank.insert(id, (rank, (-bm25) as f32));
            rank += 1;
        }
    }

    let chunks = load_all_chunks(&connection, &scope)?;
    let query_vector = local_vector(query);
    let mut vector_order: Vec<(String, f32)> = chunks
        .iter()
        .map(|chunk| (chunk.id.clone(), cosine(&query_vector, &chunk.vector)))
        .collect();
    vector_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let vector_rank: HashMap<String, (usize, f32)> = vector_order
        .into_iter()
        .take(500)
        .enumerate()
        .map(|(rank, (id, score))| (id, (rank, score)))
        .collect();

    let by_id: HashMap<&str, &IndexedChunk> = chunks
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
        let mut rrf = 0.0_f32;
        if lex_rank != usize::MAX {
            rrf += 1.0 / (60.0 + lex_rank as f32);
        }
        if vec_rank != usize::MAX {
            rrf += 1.0 / (60.0 + vec_rank as f32);
        }
        let folded_heading = intelligence::normalize_search_text(&chunk.heading);
        let heading_hits = query_tokens
            .iter()
            .filter(|token| folded_heading.contains(*token))
            .count() as f32;
        let body_tokens: HashSet<String> = normalized_tokens(&chunk.body).into_iter().collect();
        let overlap = query_tokens
            .iter()
            .filter(|token| body_tokens.contains(*token))
            .count() as f32
            / query_tokens.len().max(1) as f32;
        let rerank_score =
            rrf * 30.0 + vec_score.max(0.0) * 0.55 + overlap * 0.35 + heading_hits * 0.1;
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
    results.sort_by(|a, b| {
        b.rerank_score
            .partial_cmp(&a.rerank_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit.max(1));
    Ok(results)
}

fn extractive_answer(question: &str, hits: &[HybridSearchHit]) -> String {
    if hits.is_empty() {
        return "Không tìm thấy bằng chứng phù hợp trong kho tri thức.".into();
    }
    let mut answer = format!(
        "## Trả lời trích xuất\n\nCâu hỏi: **{}**\n\n",
        question.trim()
    );
    for (index, hit) in hits.iter().enumerate() {
        answer.push_str(&format!(
            "{}. {} [CITE-{:04}]\n\n",
            index + 1,
            hit.snippet,
            index + 1
        ));
    }
    answer
}

fn answer_is_grounded(answer: &str, valid_ids: &HashSet<String>) -> Result<(), Vec<String>> {
    let mut warnings = Vec::new();
    let cited: HashSet<String> = answer
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '[' | ']' | '(' | ')' | ',' | '.')
        })
        .filter(|part| part.starts_with("CITE-"))
        .map(str::to_string)
        .collect();
    if cited.is_empty() {
        warnings.push("LLM không trả citation; đã fallback extractive.".into());
    }
    for citation in cited {
        if !valid_ids.contains(&citation) {
            warnings.push(format!("LLM dùng citation không tồn tại: {citation}"));
        }
    }
    for paragraph in answer.split("\n\n") {
        let factual = paragraph.chars().count() >= 60
            && !paragraph.starts_with('#')
            && !paragraph.starts_with("Câu hỏi:");
        if factual && !paragraph.contains("[CITE-") {
            warnings.push("Có đoạn trả lời dài không gắn citation.".into());
            break;
        }
    }
    if warnings.is_empty() {
        Ok(())
    } else {
        Err(warnings)
    }
}

#[tauri::command]
pub async fn rebuild_knowledge_index(
    state: State<'_, AppState>,
    req: IndexRequest,
) -> Result<IndexBuildResult, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || index_documents_inner(&root, &req.source_rels))
        .await
        .map_err(es)?
}

#[tauri::command]
pub fn knowledge_index_stats(state: State<AppState>) -> Result<IndexStats, String> {
    let root = data_root(&state);
    let connection = open_index(&root)?;
    let documents: usize = connection
        .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
        .map_err(es)?;
    let chunks: usize = connection
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .map_err(es)?;
    let database_bytes = index_path(&root)
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    Ok(IndexStats {
        documents,
        chunks,
        database_bytes,
        vector_dimensions: VECTOR_DIMENSIONS,
        embedding_mode: "local_hash_v1".into(),
    })
}

#[tauri::command]
pub async fn hybrid_search(
    state: State<'_, AppState>,
    req: HybridSearchRequest,
) -> Result<Vec<HybridSearchHit>, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        hybrid_search_inner(&root, &req.source_rels, &req.query, req.limit.unwrap_or(20))
    })
    .await
    .map_err(es)?
}

fn hybrid_ask_inner(
    root: &Path,
    req: HybridAskRequest,
    llm_config: Option<fileconv_core::llm::LlmConfig>,
    config_warning: Option<String>,
) -> Result<GroundedAnswer, String> {
    let hits = hybrid_search_inner(
        root,
        &req.source_rels,
        &req.question,
        req.top_k.unwrap_or(8),
    )?;
    let fallback = extractive_answer(&req.question, &hits);
    if !req.use_llm.unwrap_or(false) {
        return Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "offline_extractive".into(),
            grounded: true,
            warnings: Vec::new(),
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
            warnings: vec![warning],
        });
    };
    if hits.is_empty() {
        return Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "fallback_extractive".into(),
            grounded: true,
            warnings: vec!["Không đủ nguồn để gọi LLM.".into()],
        });
    }
    let context = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            format!(
                "[CITE-{:04}] {} > {}\n{}",
                index + 1,
                hit.source_rel,
                hit.heading,
                hit.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let prompt = format!(
        "Câu hỏi: {}\n\nNguồn:\n{}\n\n\
         Chỉ trả lời từ nguồn. Mỗi đoạn factual phải kết thúc bằng [CITE-xxxx]. \
         Nếu nguồn thiếu, nói rõ không đủ dữ liệu.",
        req.question, context
    );
    let llm_answer = match fileconv_core::llm::chat(
        &config,
        "Bạn là trợ lý kho tri thức trung thực. Không bịa và luôn trích citation.",
        &prompt,
    ) {
        Ok(answer) => answer,
        Err(error) => {
            return Ok(GroundedAnswer {
                answer: fallback,
                citations: hits,
                mode: "fallback_extractive".into(),
                grounded: true,
                warnings: vec![format!(
                    "LLM provider lỗi ({error}); đã fallback extractive local."
                )],
            });
        }
    };
    let valid_ids: HashSet<String> = (0..hits.len())
        .map(|index| format!("CITE-{:04}", index + 1))
        .collect();
    match answer_is_grounded(&llm_answer, &valid_ids) {
        Ok(()) => {
            let local = config
                .base_url
                .as_deref()
                .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));
            Ok(GroundedAnswer {
                answer: llm_answer,
                citations: hits,
                mode: if local {
                    "local_llm".into()
                } else {
                    "cloud_llm".into()
                },
                grounded: true,
                warnings: Vec::new(),
            })
        }
        Err(warnings) => Ok(GroundedAnswer {
            answer: fallback,
            citations: hits,
            mode: "fallback_extractive".into(),
            grounded: true,
            warnings,
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
    tauri::async_runtime::spawn_blocking(move || {
        hybrid_ask_inner(&root, req, llm_config, config_warning)
    })
    .await
    .map_err(es)?
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let first = index_documents_inner(&root, &sources).unwrap();
        let second = index_documents_inner(&root, &sources).unwrap();
        assert_eq!(first.indexed, 2);
        assert_eq!(second.skipped, 2);
        let connection = open_index(&root).unwrap();
        let count: usize = connection
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hybrid_search_ranks_relevant_document() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources).unwrap();
        let hits = hybrid_search_inner(&root, &sources, "đối soát giao dịch", 5).unwrap();
        assert_eq!(hits[0].source_rel, "payments.pdf");
        assert!(hits[0].rerank_score > 0.0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_filters_search_results() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources).unwrap();
        let hits =
            hybrid_search_inner(&root, &["security.docx".into()], "giao dịch API", 10).unwrap();
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
        let hits = hybrid_search_inner(&root, &sources, "xác thực API", 3).unwrap();
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
        index_documents_inner(&root, &sources).unwrap();
        std::fs::write(
            root.join("payments.pdf.md"),
            "# Hoàn tiền\n\nGiao dịch hoàn tiền phải được duyệt bởi hai người.\n",
        )
        .unwrap();
        let update = index_documents_inner(&root, &["payments.pdf".into()]).unwrap();
        assert_eq!(update.indexed, 1);
        let hits =
            hybrid_search_inner(&root, &["payments.pdf".into()], "hai người duyệt", 5).unwrap();
        assert!(hits[0].snippet.contains("hai người"));
        let connection = open_index(&root).unwrap();
        let count: usize = connection
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE doc_rel = 'payments.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
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
        let hits = hybrid_search_inner(&root, &["spec.pdf".into()], "thanh toán QR", 3).unwrap();
        assert_eq!(hits[0].anchor.page, Some(7));
        assert!(hits[0].anchor.end > hits[0].anchor.start);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn punctuation_cannot_break_fts_query_syntax() {
        let root = temp_root();
        let sources = seed(&root);
        let hits =
            hybrid_search_inner(&root, &sources, "API: \"xác thực\" OR (giao dịch)", 5).unwrap();
        assert!(!hits.is_empty());
        std::fs::remove_dir_all(root).ok();
    }
}
