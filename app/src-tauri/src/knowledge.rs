use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use fileconv_core::intelligence::{self, CorpusDocument};
use fileconv_core::llm::EmbeddingConfig;
pub use fileconv_knowledge::types::{
    GroundedAnswer, HybridAskRequest, HybridSearchHit, HybridSearchRequest, HybridSearchResponse,
    IndexBuildResult, IndexMetadata, IndexRequest, IndexStats, SourceAnchor,
};
use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;

use super::{data_root, es, resolve_within, AppState};

const LOCAL_VECTOR_DIMENSIONS: usize = 256;
const MAX_VECTOR_CANDIDATES: usize = 100_000;
const LOCAL_EMBEDDING_MODE: &str = "local_hash_v1";
const PROVIDER_EMBEDDING_MODE: &str = "provider_v1";

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
    vector_dims: usize,
}

#[derive(Debug, Clone)]
struct EmbeddingPlan {
    config: Option<EmbeddingConfig>,
    metadata: IndexMetadata,
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

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(es)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(es)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(es)?;
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(es)?;
    }
    Ok(())
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
               chunks INTEGER NOT NULL,
               embedding_signature TEXT NOT NULL DEFAULT ''
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
               vector BLOB NOT NULL,
               vector_dims INTEGER NOT NULL DEFAULT 256
             );
             CREATE TABLE IF NOT EXISTS index_meta (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
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
    ensure_column(
        &connection,
        "documents",
        "embedding_signature",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &connection,
        "chunks",
        "vector_dims",
        "INTEGER NOT NULL DEFAULT 256",
    )?;
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
    let index = (hash as usize) % LOCAL_VECTOR_DIMENSIONS;
    let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
    (index, sign)
}

/// Fully local feature-hashing vector. It is always available and provides a
/// deterministic vector fallback when no embedding provider is configured.
fn local_vector(text: &str) -> Vec<f32> {
    let folded = intelligence::normalize_search_text(text);
    let mut vector = vec![0.0_f32; LOCAL_VECTOR_DIMENSIONS];
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

fn load_vector_points(
    connection: &Connection,
    expected_dimensions: usize,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    let mut statement = connection
        .prepare("SELECT chunk_id, vector, vector_dims FROM chunks ORDER BY chunk_id")
        .map_err(es)?;
    let rows = statement
        .query_map([], |row| {
            let bytes: Vec<u8> = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                vector_from_bytes(&bytes),
                row.get::<_, i64>(2)? as usize,
            ))
        })
        .map_err(es)?;
    let mut points = Vec::new();
    for row in rows {
        let (id, vector, dimensions) = row.map_err(es)?;
        if dimensions != expected_dimensions || vector.len() != expected_dimensions {
            return Err(format!(
                "không build HNSW: chunk {id} có {dimensions}D, metadata {expected_dimensions}D"
            ));
        }
        points.push((id, vector));
    }
    Ok(points)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn provider_name(provider: fileconv_core::llm::Provider) -> String {
    format!("{provider:?}").to_ascii_lowercase()
}

fn embedding_plan(config: Option<EmbeddingConfig>) -> EmbeddingPlan {
    match config {
        Some(config) => {
            let provider = provider_name(config.provider);
            let model = config.model.clone();
            let signature = stable_hash(&format!(
                "{PROVIDER_EMBEDDING_MODE}|{provider}|{model}|{}|{}",
                config.base_url.as_deref().unwrap_or_default(),
                config
                    .dimensions
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            ));
            EmbeddingPlan {
                metadata: IndexMetadata {
                    mode: PROVIDER_EMBEDDING_MODE.into(),
                    provider,
                    model,
                    dimensions: config.dimensions.unwrap_or_default(),
                    signature,
                },
                config: Some(config),
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
        },
    }
}

fn read_metadata(connection: &Connection) -> Result<IndexMetadata, String> {
    let read = |key: &str| -> Result<Option<String>, String> {
        connection
            .query_row(
                "SELECT value FROM index_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(es)
    };
    let mode = read("embedding_mode")?.unwrap_or_else(|| LOCAL_EMBEDDING_MODE.into());
    let provider = read("embedding_provider")?.unwrap_or_else(|| "local".into());
    let model = read("embedding_model")?.unwrap_or_else(|| LOCAL_EMBEDDING_MODE.into());
    let dimensions = read("embedding_dimensions")?
        .and_then(|value| value.parse().ok())
        .unwrap_or(LOCAL_VECTOR_DIMENSIONS);
    let signature = read("embedding_signature")?.unwrap_or_else(|| LOCAL_EMBEDDING_MODE.into());
    Ok(IndexMetadata {
        mode,
        provider,
        model,
        dimensions,
        signature,
    })
}

fn write_metadata(
    transaction: &rusqlite::Transaction<'_>,
    metadata: &IndexMetadata,
) -> Result<(), String> {
    for (key, value) in [
        ("embedding_mode", metadata.mode.clone()),
        ("embedding_provider", metadata.provider.clone()),
        ("embedding_model", metadata.model.clone()),
        ("embedding_dimensions", metadata.dimensions.to_string()),
        ("embedding_signature", metadata.signature.clone()),
    ] {
        transaction
            .execute(
                "INSERT INTO index_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(es)?;
    }
    Ok(())
}

fn clear_index(root: &Path, connection: &mut Connection) -> Result<(), String> {
    let transaction = connection.transaction().map_err(es)?;
    transaction
        .execute("DELETE FROM chunks_fts", [])
        .map_err(es)?;
    transaction.execute("DELETE FROM chunks", []).map_err(es)?;
    transaction
        .execute("DELETE FROM documents", [])
        .map_err(es)?;
    transaction
        .execute("DELETE FROM index_meta", [])
        .map_err(es)?;
    transaction.commit().map_err(es)?;
    super::vector_index::clear(root);
    Ok(())
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

fn index_documents_with_plan(
    root: &Path,
    source_rels: &[String],
    mut plan: EmbeddingPlan,
) -> Result<IndexBuildResult, String> {
    let documents = super::intelligence::load_documents(root, source_rels)?;
    let mut connection = open_index(root)?;
    let indexed_documents: usize = connection
        .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
        .map_err(es)?;
    let current_metadata = read_metadata(&connection)?;
    if indexed_documents > 0 && current_metadata.signature != plan.metadata.signature {
        clear_index(root, &mut connection)?;
    } else if indexed_documents > 0
        && current_metadata.signature == plan.metadata.signature
        && plan.metadata.dimensions == 0
    {
        plan.metadata.dimensions = current_metadata.dimensions;
    }
    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut total_chunks = 0usize;

    for document in &documents {
        let content_hash = stable_hash(&document.markdown);
        let existing: Option<(String, String)> = connection
            .query_row(
                "SELECT content_hash, embedding_signature
                 FROM documents WHERE doc_rel = ?1",
                params![document.source_rel],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(es)?;
        if existing.as_ref().is_some_and(|(hash, signature)| {
            hash == &content_hash && signature == &plan.metadata.signature
        }) {
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
        let embedding_inputs: Vec<String> = chunks
            .iter()
            .map(|chunk| format!("{}\n{}", chunk.heading, chunk.text))
            .collect();
        let vectors = if let Some(config) = plan.config.as_ref() {
            fileconv_core::llm::embed_batch(config, &embedding_inputs)
                .map_err(|error| format!("embedding provider lỗi: {error}"))?
        } else {
            embedding_inputs
                .iter()
                .map(|input| local_vector(input))
                .collect()
        };
        if vectors.len() != chunks.len() {
            return Err("số vector không khớp số chunk".into());
        }
        if let Some(vector) = vectors.first() {
            if vector.is_empty() || vectors.iter().any(|item| item.len() != vector.len()) {
                return Err("embedding index có vector khác số chiều".into());
            }
            if plan.metadata.dimensions == 0 {
                plan.metadata.dimensions = vector.len();
            } else if plan.metadata.dimensions != vector.len() {
                return Err(format!(
                    "embedding trả {} chiều, index yêu cầu {}",
                    vector.len(),
                    plan.metadata.dimensions
                ));
            }
        }
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
        for (chunk, vector) in chunks.iter().zip(vectors.iter()) {
            let anchor = infer_anchor(document, chunk);
            transaction
                .execute(
                    "INSERT INTO chunks (
                       chunk_id, doc_rel, md_rel, heading, body, start_offset,
                       end_offset, page, slide, sheet, vector, vector_dims
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                        vector_bytes(vector),
                        vector.len() as i64,
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
                "INSERT INTO documents (
                   doc_rel, md_rel, content_hash, format, chunks, embedding_signature
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(doc_rel) DO UPDATE SET
                   md_rel=excluded.md_rel,
                   content_hash=excluded.content_hash,
                   format=excluded.format,
                   chunks=excluded.chunks,
                   embedding_signature=excluded.embedding_signature",
                params![
                    document.source_rel,
                    document.md_rel,
                    content_hash,
                    document.format,
                    chunks.len() as i64,
                    plan.metadata.signature,
                ],
            )
            .map_err(es)?;
        transaction.commit().map_err(es)?;
        total_chunks += chunks.len();
        indexed += 1;
    }

    let transaction = connection.transaction().map_err(es)?;
    write_metadata(&transaction, &plan.metadata)?;
    transaction.commit().map_err(es)?;
    let mut warnings = Vec::new();
    if indexed > 0
        || !super::vector_index::is_available(
            root,
            &plan.metadata.signature,
            plan.metadata.dimensions,
        )
    {
        match load_vector_points(&connection, plan.metadata.dimensions).and_then(|points| {
            super::vector_index::rebuild(
                root,
                &plan.metadata.signature,
                plan.metadata.dimensions,
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
        documents: documents.len(),
        chunks: total_chunks,
        indexed,
        skipped,
        embedding_mode: plan.metadata.mode,
        embedding_provider: plan.metadata.provider,
        embedding_model: plan.metadata.model,
        vector_dimensions: plan.metadata.dimensions,
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
        Err(error)
            if provider_requested && fallback_local && error.contains("embedding provider") =>
        {
            let mut connection = open_index(root)?;
            clear_index(root, &mut connection)?;
            let mut result = index_documents_with_plan(root, source_rels, embedding_plan(None))?;
            result.warnings.push(format!(
                "{error}; đã rebuild toàn bộ scope bằng local hash offline."
            ));
            Ok(result)
        }
        Err(error) => Err(error),
    }
}

fn load_all_chunks(
    connection: &Connection,
    scope: &HashSet<String>,
    expected_dimensions: usize,
) -> Result<Vec<IndexedChunk>, String> {
    let mut statement = connection
        .prepare(
            "SELECT chunk_id, doc_rel, md_rel, heading, body, start_offset,
                    end_offset, page, slide, sheet, vector, vector_dims
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
                vector_dims: row.get::<_, i64>(11)? as usize,
            })
        })
        .map_err(es)?;
    let mut chunks = Vec::new();
    for row in rows {
        let chunk = row.map_err(es)?;
        if chunk.vector_dims != expected_dimensions || chunk.vector.len() != expected_dimensions {
            return Err(format!(
                "index vector không nhất quán: chunk {} có {}D, metadata {}D",
                chunk.id, chunk.vector_dims, expected_dimensions
            ));
        }
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
    let connection = open_index(root)?;
    let metadata = read_metadata(&connection)?;
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

    let chunks = load_all_chunks(&connection, &scope, metadata.dimensions)?;
    let mut warnings = Vec::new();
    let query_vector = if metadata.mode == PROVIDER_EMBEDDING_MODE {
        match config {
            Some(config)
                if embedding_plan(Some(config.clone())).metadata.signature
                    == metadata.signature =>
            {
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
                    Err(error) => {
                        warnings.push(format!(
                            "Embedding provider lỗi ({error}); chỉ dùng FTS lexical."
                        ));
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
    Ok(HybridSearchResponse {
        hits: results,
        warnings,
        embedding_mode: metadata.mode,
    })
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
    let metadata = read_metadata(&connection)?;
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
                warnings: {
                    retrieval_warnings.push(format!(
                        "LLM provider lỗi ({error}); đã fallback extractive local."
                    ));
                    retrieval_warnings
                },
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
            "",
            "missing-model",
            Some("http://127.0.0.1:1".into()),
            None,
        )
        .unwrap();
        let result = index_documents_inner(&root, &sources, Some(config), true).unwrap();
        assert_eq!(result.embedding_mode, LOCAL_EMBEDDING_MODE);
        assert_eq!(result.vector_dimensions, LOCAL_VECTOR_DIMENSIONS);
        assert_eq!(result.indexed, 2);
        assert!(result.warnings[0].contains("rebuild"));
        let metadata = read_metadata(&open_index(&root).unwrap()).unwrap();
        assert_eq!(metadata.signature, LOCAL_EMBEDDING_MODE);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn mixed_vector_dimensions_are_rejected() {
        let root = temp_root();
        let sources = seed(&root);
        index_documents_inner(&root, &sources, None, true).unwrap();
        let connection = open_index(&root).unwrap();
        connection
            .execute(
                "UPDATE chunks SET vector_dims = 3 WHERE chunk_id = (
                   SELECT chunk_id FROM chunks LIMIT 1
                 )",
                [],
            )
            .unwrap();
        let error = hybrid_search_inner(&root, &sources, "giao dịch", 5, None, true).unwrap_err();
        assert!(error.contains("không nhất quán"));
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
