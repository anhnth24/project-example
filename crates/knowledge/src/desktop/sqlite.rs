//! SQLite authority for the local desktop knowledge index.
//!
//! Callers own path-jailing, document loading, provider HTTP, and ANN cache
//! orchestration. This module owns schema compatibility, incremental writes,
//! FTS hydration, vector decoding, and index metadata.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fileconv_core::intelligence::{self, CorpusDocument};
use rusqlite::{params, Connection, OptionalExtension};

use crate::citation::infer_source_anchor;
use crate::embedding::{
    validate_embedding_batch, EmbeddingPlan, EmbeddingVector, LOCAL_EMBEDDING_MODE,
    LOCAL_VECTOR_DIMENSIONS,
};
use crate::identity::legacy_desktop_hash;
use crate::types::IndexMetadata;
use crate::{KnowledgeError, Result};

pub const MAX_VECTOR_CANDIDATES: usize = 100_000;

#[derive(Debug, Clone)]
pub struct StoredChunk {
    pub id: String,
    pub source_rel: String,
    pub md_rel: String,
    pub heading: String,
    pub body: String,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub vector: Vec<f32>,
    pub vector_dims: usize,
}

#[derive(Debug, Clone)]
pub struct StoreIndexResult {
    pub documents: usize,
    pub chunks: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub metadata: IndexMetadata,
}

pub struct SqliteKnowledgeStore {
    connection: Connection,
    path: PathBuf,
}

fn failure(message: impl Into<String>) -> KnowledgeError {
    KnowledgeError::AdapterFailure(message.into())
}

fn sql(error: rusqlite::Error) -> KnowledgeError {
    failure(format!("SQLite knowledge index failed: {error}"))
}

impl SqliteKnowledgeStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                failure(format!("cannot create knowledge index parent: {error}"))
            })?;
        }
        let connection = Connection::open(&path).map_err(sql)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(sql)?;
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
            .map_err(sql)?;
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
        Ok(Self { connection, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn document_count(&self) -> Result<usize> {
        self.connection
            .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
            .map_err(sql)
    }

    pub fn chunk_count(&self) -> Result<usize> {
        self.connection
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .map_err(sql)
    }

    pub fn database_bytes(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or_default()
    }

    pub fn metadata(&self) -> Result<IndexMetadata> {
        read_metadata(&self.connection)
    }

    pub fn clear(&mut self) -> Result<()> {
        let transaction = self.connection.transaction().map_err(sql)?;
        transaction
            .execute("DELETE FROM chunks_fts", [])
            .map_err(sql)?;
        transaction.execute("DELETE FROM chunks", []).map_err(sql)?;
        transaction
            .execute("DELETE FROM documents", [])
            .map_err(sql)?;
        transaction
            .execute("DELETE FROM index_meta", [])
            .map_err(sql)?;
        transaction.commit().map_err(sql)
    }

    pub fn index_documents<Embed, Cleared>(
        &mut self,
        documents: &[CorpusDocument],
        mut metadata: IndexMetadata,
        signature_plan: Option<&EmbeddingPlan>,
        mut embed: Embed,
        mut on_cleared: Cleared,
    ) -> Result<StoreIndexResult>
    where
        Embed: FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
        Cleared: FnMut(),
    {
        let indexed_documents = self.document_count()?;
        let current_metadata = self.metadata()?;
        if indexed_documents > 0
            && metadata.dimensions == 0
            && current_metadata.mode == metadata.mode
            && current_metadata.provider == metadata.provider
            && current_metadata.model == metadata.model
        {
            metadata.dimensions = current_metadata.dimensions;
            finalize_signature(&mut metadata, signature_plan)?;
        }
        if indexed_documents > 0 && current_metadata.signature != metadata.signature {
            self.clear()?;
            on_cleared();
        }

        let mut indexed = 0;
        let mut skipped = 0;
        let mut total_chunks = 0;
        for document in documents {
            let content_hash = legacy_desktop_hash(&document.markdown);
            let existing: Option<(String, String)> = self
                .connection
                .query_row(
                    "SELECT content_hash, embedding_signature
                     FROM documents WHERE doc_rel = ?1",
                    params![document.source_rel],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(sql)?;
            if existing.as_ref().is_some_and(|(hash, signature)| {
                hash == &content_hash && signature == &metadata.signature
            }) {
                let count: usize = self
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM chunks WHERE doc_rel = ?1",
                        params![document.source_rel],
                        |row| row.get(0),
                    )
                    .map_err(sql)?;
                total_chunks += count;
                skipped += 1;
                continue;
            }

            let chunks = intelligence::build_corpus(std::slice::from_ref(document), 2_000);
            let inputs = chunks
                .iter()
                .map(|chunk| format!("{}\n{}", chunk.heading, chunk.text))
                .collect::<Vec<_>>();
            let vectors = embed(&inputs)?;
            let checked = vectors
                .iter()
                .cloned()
                .map(EmbeddingVector::new)
                .collect::<Result<Vec<_>>>()?;
            let expected = (metadata.dimensions > 0).then_some(metadata.dimensions);
            let dimensions = validate_embedding_batch(&checked, chunks.len(), expected)?;
            if dimensions > 0 && metadata.dimensions == 0 {
                metadata.dimensions = dimensions;
                finalize_signature(&mut metadata, signature_plan)?;
            }

            let transaction = self.connection.transaction().map_err(sql)?;
            transaction
                .execute(
                    "DELETE FROM chunks_fts WHERE doc_rel = ?1",
                    params![document.source_rel],
                )
                .map_err(sql)?;
            transaction
                .execute(
                    "DELETE FROM chunks WHERE doc_rel = ?1",
                    params![document.source_rel],
                )
                .map_err(sql)?;
            for (chunk, vector) in chunks.iter().zip(&vectors) {
                let anchor = infer_source_anchor(
                    &document.format,
                    &chunk.heading,
                    chunk.page,
                    chunk.start,
                    chunk.end,
                );
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
                    .map_err(sql)?;
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
                    .map_err(sql)?;
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
                        metadata.signature,
                    ],
                )
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            total_chunks += chunks.len();
            indexed += 1;
        }

        let transaction = self.connection.transaction().map_err(sql)?;
        write_metadata(&transaction, &metadata)?;
        transaction.commit().map_err(sql)?;
        Ok(StoreIndexResult {
            documents: documents.len(),
            chunks: total_chunks,
            indexed,
            skipped,
            metadata,
        })
    }

    pub fn load_vector_points(
        &self,
        expected_dimensions: usize,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let mut statement = self
            .connection
            .prepare("SELECT chunk_id, vector, vector_dims FROM chunks ORDER BY chunk_id")
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(sql)?;
        let mut points = Vec::new();
        for row in rows {
            let (id, bytes, stored_dimensions) = row.map_err(sql)?;
            let dimensions = usize::try_from(stored_dimensions).map_err(|_| {
                KnowledgeError::IncompatibleIndex("chunk vector dimensions are negative")
            })?;
            let vector = vector_from_bytes(&bytes)?;
            validate_stored_vector(&id, dimensions, &vector, expected_dimensions)?;
            points.push((id, vector));
        }
        Ok(points)
    }

    pub fn lexical_ranks(
        &self,
        fts_query: &str,
        scope: &HashSet<String>,
        limit: usize,
    ) -> Result<HashMap<String, (usize, f32)>> {
        if fts_query.is_empty() || limit == 0 {
            return Ok(HashMap::new());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk_id, doc_rel, bm25(chunks_fts)
                 FROM chunks_fts WHERE chunks_fts MATCH ?1
                 ORDER BY bm25(chunks_fts) LIMIT ?2",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map(params![fts_query, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })
            .map_err(sql)?;
        let mut ranks = HashMap::new();
        for row in rows {
            let (id, source_rel, bm25) = row.map_err(sql)?;
            if !scope.is_empty() && !scope.contains(&source_rel) {
                continue;
            }
            ranks.insert(id, (ranks.len(), (-bm25) as f32));
        }
        Ok(ranks)
    }

    pub fn load_chunks(
        &self,
        scope: &HashSet<String>,
        expected_dimensions: usize,
    ) -> Result<Vec<StoredChunk>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT chunk_id, doc_rel, md_rel, heading, body, start_offset,
                        end_offset, page, slide, sheet, vector, vector_dims
                 FROM chunks",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<u32>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Vec<u8>>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            })
            .map_err(sql)?;
        let mut chunks = Vec::new();
        for row in rows {
            let (
                id,
                source_rel,
                md_rel,
                heading,
                body,
                start,
                end,
                page,
                slide,
                sheet,
                bytes,
                stored_dimensions,
            ) = row.map_err(sql)?;
            let start = usize::try_from(start)
                .map_err(|_| KnowledgeError::IncompatibleIndex("chunk start offset is negative"))?;
            let end = usize::try_from(end)
                .map_err(|_| KnowledgeError::IncompatibleIndex("chunk end offset is negative"))?;
            let vector_dims = usize::try_from(stored_dimensions).map_err(|_| {
                KnowledgeError::IncompatibleIndex("chunk vector dimensions are negative")
            })?;
            let vector = vector_from_bytes(&bytes)?;
            validate_stored_vector(&id, vector_dims, &vector, expected_dimensions)?;
            if scope.is_empty() || scope.contains(&source_rel) {
                chunks.push(StoredChunk {
                    id,
                    source_rel,
                    md_rel,
                    heading,
                    body,
                    start,
                    end,
                    page,
                    slide,
                    sheet,
                    vector,
                    vector_dims,
                });
                if chunks.len() > MAX_VECTOR_CANDIDATES {
                    return Err(KnowledgeError::InvalidInput(
                        "vector candidate count exceeds 100000",
                    ));
                }
            }
        }
        Ok(chunks)
    }
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sql)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sql)?;
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(sql)?;
    }
    Ok(())
}

fn read_metadata(connection: &Connection) -> Result<IndexMetadata> {
    let read = |key: &str| -> Result<Option<String>> {
        connection
            .query_row(
                "SELECT value FROM index_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
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

fn write_metadata(transaction: &rusqlite::Transaction<'_>, metadata: &IndexMetadata) -> Result<()> {
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
            .map_err(sql)?;
    }
    Ok(())
}

fn finalize_signature(
    metadata: &mut IndexMetadata,
    signature_plan: Option<&EmbeddingPlan>,
) -> Result<()> {
    if let Some(plan) = signature_plan {
        if metadata.dimensions > 0 {
            metadata.signature = plan.signature(metadata.dimensions)?;
        }
    }
    Ok(())
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn vector_from_bytes(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(KnowledgeError::IncompatibleIndex(
            "chunk vector byte length is invalid",
        ));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(KnowledgeError::IncompatibleIndex(
            "chunk vector contains non-finite values",
        ));
    }
    Ok(vector)
}

fn validate_stored_vector(
    id: &str,
    stored_dimensions: usize,
    vector: &[f32],
    expected_dimensions: usize,
) -> Result<()> {
    if stored_dimensions != expected_dimensions || vector.len() != expected_dimensions {
        return Err(failure(format!(
            "chunk {id} has {stored_dimensions}D, metadata requires {expected_dimensions}D"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::embedding::local_vector;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(label: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "markhand_sqlite_{label}_{}_{}.sqlite",
            std::process::id(),
            id
        ))
    }

    fn document(markdown: &str) -> CorpusDocument {
        CorpusDocument {
            source_rel: "payments.pdf".into(),
            md_rel: "payments.pdf.md".into(),
            format: "pdf".into(),
            markdown: markdown.into(),
        }
    }

    fn local_metadata() -> IndexMetadata {
        IndexMetadata {
            mode: LOCAL_EMBEDDING_MODE.into(),
            provider: "local".into(),
            model: LOCAL_EMBEDDING_MODE.into(),
            dimensions: LOCAL_VECTOR_DIMENSIONS,
            signature: LOCAL_EMBEDDING_MODE.into(),
        }
    }

    fn index(store: &mut SqliteKnowledgeStore, documents: &[CorpusDocument]) -> StoreIndexResult {
        store
            .index_documents(
                documents,
                local_metadata(),
                None,
                |inputs| {
                    Ok(inputs
                        .iter()
                        .map(|input| local_vector(input).into_values())
                        .collect())
                },
                || {},
            )
            .unwrap()
    }

    #[test]
    fn empty_incremental_changed_and_persistent_index() {
        let path = temp_path("incremental");
        let mut store = SqliteKnowledgeStore::open(&path).unwrap();
        let empty = index(&mut store, &[]);
        assert_eq!((empty.documents, empty.chunks), (0, 0));

        let original = document("# Đối soát\n\nGiao dịch được đối soát mỗi ngày.");
        assert_eq!(
            index(&mut store, std::slice::from_ref(&original)).indexed,
            1
        );
        assert_eq!(
            index(&mut store, std::slice::from_ref(&original)).skipped,
            1
        );
        drop(store);

        let mut reopened = SqliteKnowledgeStore::open(&path).unwrap();
        assert_eq!(reopened.document_count().unwrap(), 1);
        assert_eq!(reopened.chunk_count().unwrap(), 1);
        let changed = document("# Hoàn tiền\n\nHai người phải duyệt hoàn tiền.");
        assert_eq!(index(&mut reopened, &[changed]).indexed, 1);
        assert_eq!(reopened.chunk_count().unwrap(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lexical_scope_and_hydration_are_preserved() {
        let path = temp_path("scope");
        let mut store = SqliteKnowledgeStore::open(&path).unwrap();
        let documents = [
            document("# Đối soát\n\nGiao dịch được đối soát mỗi ngày."),
            CorpusDocument {
                source_rel: "security.docx".into(),
                md_rel: "security.docx.md".into(),
                format: "docx".into(),
                markdown: "# Bảo mật\n\nAPI yêu cầu xác thực.".into(),
            },
        ];
        index(&mut store, &documents);
        let scope = HashSet::from(["security.docx".to_string()]);
        let ranks = store
            .lexical_ranks(r#""giao"* OR "xac"*"#, &scope, 250)
            .unwrap();
        assert_eq!(ranks.len(), 1);
        let chunks = store.load_chunks(&scope, LOCAL_VECTOR_DIMENSIONS).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].source_rel, "security.docx");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn corrupt_dimensions_and_vector_bytes_are_rejected() {
        let path = temp_path("corrupt");
        let mut store = SqliteKnowledgeStore::open(&path).unwrap();
        index(
            &mut store,
            &[document("# Đối soát\n\nGiao dịch được đối soát mỗi ngày.")],
        );
        store
            .connection
            .execute("UPDATE chunks SET vector_dims = 3", [])
            .unwrap();
        assert!(store
            .load_chunks(&HashSet::new(), LOCAL_VECTOR_DIMENSIONS)
            .is_err());
        store
            .connection
            .execute(
                "UPDATE chunks SET vector_dims = ?1, vector = x'00'",
                [LOCAL_VECTOR_DIMENSIONS as i64],
            )
            .unwrap();
        assert!(store.load_vector_points(LOCAL_VECTOR_DIMENSIONS).is_err());
        let _ = std::fs::remove_file(path);
    }
}
