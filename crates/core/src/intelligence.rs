//! Local-first document intelligence for Markhand.
//!
//! This module works on canonical Markdown sidecars. It intentionally keeps
//! [`crate::Converter::convert_path`] unchanged and provides deterministic
//! baselines for handoff packs, cited search, quality, PII, tables, schema,
//! versions and automation. Optional LLM enhancement remains behind `llm`.
//!
//! Persisted chunk/table/handoff IDs use [`INTELLIGENCE_ID_SCHEME`] (`sha256-v1`):
//! length-delimited SHA-256 with per-purpose domains. Visible IDs embed the
//! scheme; desktop knowledge stores persist the same scheme in SQLite metadata
//! and HNSW manifests. SQLite/FTS wipe is transactional; HNSW clear/rebuild is
//! separate and best-effort (ADR 0013) — stale ANN is rejected by scheme, not
//! by cross-store atomicity. ADR 0006 server index signatures / knowledge chunk
//! identity remain a separate contract.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::chunk::{chunk_markdown, clamp_to_char_boundary, locate_chunk_span};
use crate::ConvertError;

const DEFAULT_CHUNK_CHARS: usize = 2_000;

/// Durable core-intelligence ID scheme (chunk / table / handoff fingerprints).
///
/// `sha256-v1` = SHA-256 over a fixed framing:
/// length-prefixed (`u64` BE) fields for
/// `markhand-intelligence-id`, scheme, purpose domain, then payload parts.
/// Integers use fixed-width `u64` BE bytes. No `std::hash::Hash` serialization.
///
/// Migration: not compatible with historical `DefaultHasher` or interim
/// `sip13-v1` digests. Desktop indexes missing this scheme wipe SQLite/FTS in
/// one transaction and best-effort clear/rebuild HNSW separately (ADR 0013);
/// scheme-gated ANN + exact cosine cover clear/rebuild failures. ADR 0006
/// server identity is out of scope.
pub const INTELLIGENCE_ID_SCHEME: &str = "sha256-v1";

/// Handoff pack JSON schema that carries [`INTELLIGENCE_ID_SCHEME`].
pub const HANDOFF_SCHEMA_VERSION: u32 = 2;

const ID_NAMESPACE: &[u8] = b"markhand-intelligence-id";
const DOMAIN_CHUNK: &str = "chunk";
const DOMAIN_TABLE: &str = "table";
const DOMAIN_HANDOFF_DOCUMENT: &str = "handoff-document";
const DOMAIN_HANDOFF_PACK: &str = "handoff-pack";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusDocument {
    pub source_rel: String,
    pub md_rel: String,
    pub format: String,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub id: String,
    pub source_rel: String,
    pub md_rel: String,
    pub heading: String,
    pub quote: String,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusChunk {
    pub id: String,
    pub source_rel: String,
    pub md_rel: String,
    pub heading: String,
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk: CorpusChunk,
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskResult {
    pub answer: String,
    pub citations: Vec<Citation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualitySeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityIssue {
    pub code: String,
    pub message: String,
    pub severity: QualitySeverity,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentQuality {
    pub source_rel: String,
    pub score: f32,
    pub chars: usize,
    pub headings: usize,
    pub tables: usize,
    pub issues: Vec<QualityIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub score: f32,
    pub documents: Vec<DocumentQuality>,
    pub issue_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    Email,
    Phone,
    NationalId,
    BankAccount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiiFinding {
    pub kind: PiiKind,
    pub text: String,
    pub source_rel: String,
    pub start: usize,
    pub end: usize,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiiReport {
    pub findings: Vec<PiiFinding>,
    pub counts: BTreeMap<PiiKind, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownTable {
    pub id: String,
    pub source_rel: String,
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Number,
    Date,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaField {
    pub name: String,
    pub field_type: FieldType,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSchema {
    pub source_rel: String,
    pub headings: Vec<String>,
    pub fields: Vec<SchemaField>,
    pub tables: Vec<MarkdownTable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSnapshot {
    pub id: String,
    pub source_rel: String,
    pub created_at: u64,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Added,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub kind: DiffKind,
    pub old_start: usize,
    pub new_start: usize,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflict {
    pub index: usize,
    pub ours: String,
    pub theirs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub markdown: String,
    pub conflicts: Vec<MergeConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchAction {
    ImportOnly,
    ImportAndConvert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchRule {
    pub id: String,
    pub watch_abs: String,
    pub target_folder_rel: String,
    pub pattern: String,
    pub action: WatchAction,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchMatch {
    pub rule_id: String,
    pub source_abs: String,
    pub target_folder_rel: String,
    pub action: WatchAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffMode {
    Deterministic,
    LlmAssisted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffOptions {
    pub product_name: String,
    pub product_slug: String,
    pub locale: String,
    pub mode: HandoffMode,
    pub max_chunk_chars: usize,
    pub strict_citations: bool,
}

impl Default for HandoffOptions {
    fn default() -> Self {
        Self {
            product_name: "Sản phẩm".into(),
            product_slug: "san-pham".into(),
            locale: "vi-VN".into(),
            mode: HandoffMode::Deterministic,
            max_chunk_chars: DEFAULT_CHUNK_CHARS,
            strict_citations: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffItemKind {
    BusinessRequirement,
    FunctionalRequirement,
    UserStory,
    AcceptanceCriterion,
    TestCase,
    Glossary,
    Assumption,
    OpenQuestion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffItem {
    pub id: String,
    pub kind: HandoffItemKind,
    pub text: String,
    pub citations: Vec<String>,
    pub status: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceabilityRow {
    pub br: Option<String>,
    pub fr: Option<String>,
    pub user_story: Option<String>,
    pub acceptance_criterion: Option<String>,
    pub test_case: Option<String>,
    pub citations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationMessage {
    pub code: String,
    pub item_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffValidation {
    pub ok: bool,
    pub errors: Vec<ValidationMessage>,
    pub warnings: Vec<ValidationMessage>,
    pub citation_coverage: f32,
    pub traceability_coverage: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPack {
    pub schema_version: u32,
    /// Mirrors [`INTELLIGENCE_ID_SCHEME`]; present so consumers can refuse mixed packs.
    pub id_scheme: String,
    pub pack_id: String,
    pub product_name: String,
    pub product_slug: String,
    pub locale: String,
    pub mode: HandoffMode,
    pub created_at: u64,
    pub sources: Vec<String>,
    pub citations: Vec<Citation>,
    pub items: Vec<HandoffItem>,
    pub traceability: Vec<TraceabilityRow>,
    pub artifacts: BTreeMap<String, String>,
    pub validation: HandoffValidation,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn update_id_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Length-delimited SHA-256 digest for a purpose domain + raw byte fields.
pub(crate) fn intelligence_digest(domain: &str, fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    update_id_field(&mut hasher, ID_NAMESPACE);
    update_id_field(&mut hasher, INTELLIGENCE_ID_SCHEME.as_bytes());
    update_id_field(&mut hasher, domain.as_bytes());
    for field in fields {
        update_id_field(&mut hasher, field);
    }
    hex_encode(&hasher.finalize())
}

fn visible_id(kind: &str, digest: &str) -> String {
    format!("{kind}-{INTELLIGENCE_ID_SCHEME}-{digest}")
}

pub(crate) fn corpus_chunk_id(source_rel: &str, heading: &str, start: usize) -> String {
    let start_be = (start as u64).to_be_bytes();
    visible_id(
        "chunk",
        &intelligence_digest(
            DOMAIN_CHUNK,
            &[source_rel.as_bytes(), heading.as_bytes(), &start_be],
        ),
    )
}

pub(crate) fn markdown_table_id(source_rel: &str, index: usize, start: usize) -> String {
    let index_be = (index as u64).to_be_bytes();
    let start_be = (start as u64).to_be_bytes();
    visible_id(
        "table",
        &intelligence_digest(DOMAIN_TABLE, &[source_rel.as_bytes(), &index_be, &start_be]),
    )
}

pub(crate) fn handoff_document_digest(source_rel: &str, markdown: &str) -> String {
    intelligence_digest(
        DOMAIN_HANDOFF_DOCUMENT,
        &[source_rel.as_bytes(), markdown.as_bytes()],
    )
}

fn handoff_mode_tag(mode: &HandoffMode) -> &'static str {
    match mode {
        HandoffMode::Deterministic => "deterministic",
        HandoffMode::LlmAssisted => "llm_assisted",
    }
}

pub(crate) fn handoff_pack_digest(
    product_slug: &str,
    mode: &HandoffMode,
    document_digests: &[String],
) -> String {
    let mut hasher = Sha256::new();
    update_id_field(&mut hasher, ID_NAMESPACE);
    update_id_field(&mut hasher, INTELLIGENCE_ID_SCHEME.as_bytes());
    update_id_field(&mut hasher, DOMAIN_HANDOFF_PACK.as_bytes());
    update_id_field(&mut hasher, product_slug.as_bytes());
    update_id_field(&mut hasher, handoff_mode_tag(mode).as_bytes());
    for digest in document_digests {
        update_id_field(&mut hasher, digest.as_bytes());
    }
    hex_encode(&hasher.finalize())
}

fn accent_fold(text: &str) -> String {
    text.nfd()
        .filter(|ch| !('\u{0300}'..='\u{036f}').contains(ch))
        .map(|ch| match ch {
            'đ' => 'd',
            'Đ' => 'D',
            _ => ch,
        })
        .collect::<String>()
        .to_lowercase()
}

pub fn normalize_search_text(text: &str) -> String {
    accent_fold(text)
}

fn tokens(text: &str) -> Vec<String> {
    accent_fold(text)
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

/// Trang gần nhất trước `offset`, suy từ marker `<!-- Page N -->` hoặc
/// `<!-- Trang N (OCR) -->` mà converter chèn cho mỗi trang PDF. Dùng chung cho
/// citation anchor ở cả desktop lẫn index server.
///
/// `offset` không cần là char boundary: hàm clamp về boundary gần nhất bên trái
/// trước khi slice (tránh panic khi caller truyền offset thô giữa glyph UTF-8).
pub fn page_before(markdown: &str, offset: usize) -> Option<u32> {
    let end = clamp_to_char_boundary(markdown, offset.min(markdown.len()));
    let prefix = &markdown[..end];
    prefix.lines().rev().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("<!-- Trang ")
            .and_then(|rest| rest.strip_suffix(" (OCR) -->"))
            .or_else(|| {
                line.strip_prefix("<!-- Page ")
                    .and_then(|rest| rest.strip_suffix(" -->"))
            })
            .and_then(|page| page.parse().ok())
    })
}

pub fn build_corpus(documents: &[CorpusDocument], max_chars: usize) -> Vec<CorpusChunk> {
    let mut corpus = Vec::new();
    for document in documents {
        let chunks = chunk_markdown(&document.markdown, max_chars.max(200));
        let mut cursor = 0usize;
        for chunk in chunks {
            let marker_only = chunk.text.lines().all(|line| {
                let line = line.trim();
                line.is_empty()
                    || (line.starts_with("<!-- Trang ") && line.ends_with(" (OCR) -->"))
                    || (line.starts_with("<!-- Page ") && line.ends_with(" -->"))
            });
            if marker_only {
                continue;
            }
            // Cùng `locate_chunk_span` với server: giữ chunk khi không khớp; body luôn LF.
            let (start, end) = locate_chunk_span(&document.markdown, cursor, &chunk.text);
            cursor = end;
            corpus.push(CorpusChunk {
                id: corpus_chunk_id(&document.source_rel, &chunk.heading, start),
                source_rel: document.source_rel.clone(),
                md_rel: document.md_rel.clone(),
                heading: chunk.heading,
                // Canonical LF body — parity với server indexing/identity.
                text: chunk.text,
                start,
                end,
                page: page_before(&document.markdown, start),
            });
        }
    }
    corpus
}

/// Quote citation = đúng byte trên nguồn tại span (CRLF giữ nguyên); fallback body LF.
fn citation_quote_from_source(chunk: &CorpusChunk, source_markdown: &str) -> String {
    if chunk.start < chunk.end
        && chunk.end <= source_markdown.len()
        && source_markdown.is_char_boundary(chunk.start)
        && source_markdown.is_char_boundary(chunk.end)
    {
        source_markdown[chunk.start..chunk.end].to_string()
    } else {
        chunk.text.clone()
    }
}

fn citation_from_chunk(chunk: &CorpusChunk, index: usize, source_markdown: &str) -> Citation {
    let quote = citation_quote_from_source(chunk, source_markdown);
    Citation {
        id: format!("CITE-{:04}", index + 1),
        source_rel: chunk.source_rel.clone(),
        md_rel: chunk.md_rel.clone(),
        heading: chunk.heading.clone(),
        quote,
        start: chunk.start,
        end: chunk.end,
        page: chunk.page,
        confidence: if chunk.text.contains("(OCR)") {
            0.7
        } else {
            1.0
        },
    }
}

fn markdown_for_source<'a>(documents: &'a [CorpusDocument], source_rel: &str) -> &'a str {
    documents
        .iter()
        .find(|document| document.source_rel == source_rel)
        .map(|document| document.markdown.as_str())
        .unwrap_or("")
}

pub fn search_corpus(documents: &[CorpusDocument], query: &str, limit: usize) -> Vec<SearchHit> {
    let query_tokens = tokens(query);
    if query_tokens.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<SearchHit> = build_corpus(documents, DEFAULT_CHUNK_CHARS)
        .into_iter()
        .filter_map(|chunk| {
            let body_tokens = tokens(&chunk.text);
            let heading = accent_fold(&chunk.heading);
            let mut score = 0.0_f32;
            for token in &query_tokens {
                let count = body_tokens
                    .iter()
                    .filter(|candidate| *candidate == token)
                    .count();
                score += (count.min(5) as f32) * 1.2;
                if heading.contains(token) {
                    score += 3.0;
                }
            }
            (score > 0.0).then(|| SearchHit {
                snippet: chunk
                    .text
                    .split_whitespace()
                    .take(58)
                    .collect::<Vec<_>>()
                    .join(" "),
                chunk,
                score,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk.source_rel.cmp(&b.chunk.source_rel))
    });
    hits.truncate(limit.max(1));
    hits
}

pub fn ask_corpus(documents: &[CorpusDocument], question: &str, top_k: usize) -> AskResult {
    let hits = search_corpus(documents, question, top_k.max(1));
    let citations: Vec<Citation> = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            citation_from_chunk(
                &hit.chunk,
                index,
                markdown_for_source(documents, &hit.chunk.source_rel),
            )
        })
        .collect();
    let answer = if hits.is_empty() {
        "Không tìm thấy nội dung phù hợp trong phạm vi đã chọn.".to_string()
    } else {
        let mut answer = String::from(
            "## Trả lời trích xuất\n\nCác đoạn nguồn liên quan nhất (chưa suy diễn ngoài tài liệu):\n\n",
        );
        for (index, hit) in hits.iter().enumerate() {
            answer.push_str(&format!(
                "{}. {} [{}]\n\n",
                index + 1,
                hit.snippet,
                citations[index].id
            ));
        }
        answer
    };
    AskResult { answer, citations }
}

fn repeated_run(text: &str) -> bool {
    let mut previous = None;
    let mut run = 0usize;
    for ch in text.chars() {
        if Some(ch) == previous && ch.is_alphanumeric() {
            run += 1;
            if run >= 5 {
                return true;
            }
        } else {
            run = 1;
        }
        previous = Some(ch);
    }
    false
}

pub fn quality_report(documents: &[CorpusDocument]) -> QualityReport {
    let mut reports = Vec::new();
    let mut issue_count = 0usize;
    for document in documents {
        let mut issues = Vec::new();
        let chars = document.markdown.chars().count();
        if chars < 80 {
            issues.push(QualityIssue {
                code: "SHORT_CONTENT".into(),
                message: "Nội dung quá ngắn so với một tài liệu hoàn chỉnh.".into(),
                severity: QualitySeverity::Warning,
                start: None,
                end: None,
                recommendation: "Kiểm tra file nguồn hoặc chạy lại OCR.".into(),
            });
        }
        if document.markdown.contains('\u{FFFD}') {
            issues.push(QualityIssue {
                code: "REPLACEMENT_CHARACTER".into(),
                message: "Có ký tự thay thế Unicode, khả năng lỗi font/encoding.".into(),
                severity: QualitySeverity::Error,
                start: document.markdown.find('\u{FFFD}'),
                end: document.markdown.find('\u{FFFD}').map(|start| start + 3),
                recommendation: "Reprocess bằng native text hoặc OCR tier khác.".into(),
            });
        }
        if document.markdown.contains("<!-- Trang") && document.markdown.contains("(OCR)") {
            issues.push(QualityIssue {
                code: "OCR_CONTENT".into(),
                message: "Tài liệu có trang được OCR; cần rà soát thủ công.".into(),
                severity: QualitySeverity::Info,
                start: None,
                end: None,
                recommendation: "Mở Đối chiếu và kiểm tra các trang OCR.".into(),
            });
        }
        if repeated_run(&document.markdown) {
            issues.push(QualityIssue {
                code: "REPEATED_RUN".into(),
                message: "Có chuỗi ký tự lặp bất thường.".into(),
                severity: QualitySeverity::Warning,
                start: None,
                end: None,
                recommendation: "Chạy lại block bằng OCR hoặc native text.".into(),
            });
        }
        let headings = document
            .markdown
            .lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .count();
        let tables = parse_markdown_tables(document).len();
        if tables == 0 && document.markdown.contains("|---") {
            issues.push(QualityIssue {
                code: "MALFORMED_TABLE".into(),
                message: "Phát hiện cú pháp bảng chưa hoàn chỉnh.".into(),
                severity: QualitySeverity::Warning,
                start: document.markdown.find("|---"),
                end: None,
                recommendation: "Mở trình chỉnh bảng hoặc dùng native-text fallback.".into(),
            });
        }
        let penalty: f32 = issues
            .iter()
            .map(|issue| match issue.severity {
                QualitySeverity::Info => 0.04,
                QualitySeverity::Warning => 0.12,
                QualitySeverity::Error => 0.3,
            })
            .sum();
        issue_count += issues.len();
        reports.push(DocumentQuality {
            source_rel: document.source_rel.clone(),
            score: (1.0 - penalty).max(0.0),
            chars,
            headings,
            tables,
            issues,
        });
    }
    let score = if reports.is_empty() {
        0.0
    } else {
        reports.iter().map(|report| report.score).sum::<f32>() / reports.len() as f32
    };
    QualityReport {
        score,
        documents: reports,
        issue_count,
    }
}

/// PII recall policy (conservative, Vietnamese-document oriented):
///
/// - **Spans** are exact candidate bytes (email / phone / number run), never the
///   surrounding whitespace token, Markdown table pipes, link wrappers, or labels.
/// - **Email**: dot-atom local (incl. `'` / numeric); reject `price@100.00`, local-dot
///   abuse, domain-label hyphen abuse; strict delimiter boundaries on both sides
///   (no partial carve-outs from unsupported adjacent characters).
/// - **Phone**: maintained active VN mobile prefixes (incl. `055`/`087`) + exact
///   landline area/length tables; leading `+` preserved and must be `+84`; optional
///   `(0)` trunk; grouped separators; Unicode alphanumeric boundaries; consecutive
///   separated phones detected independently; failed `+…` runs are skipped whole
///   (no retry inside); reject `030…` and arbitrary bare digit runs.
/// - **Labels**: nearest explicit label cannot cross newline, `|`, comma/`，`/`、`,
///   or sentence/clause boundary, and must field-link within bounded distance;
///   Markdown table body cells inherit the column header (`Tài khoản`/`SĐT`).
/// - **Email wrappers**: recursively peel balanced `***`/`**`/`~~`/`__`/`*`/`_`/
///   `` ` `` (nested combinations); redact the inner address only; unwrapped locals
///   may still contain those marker characters.
/// - **Bank/CCCD**: nearest scoped label or table header only; bare `ngân hàng` prose
///   does not classify transaction counts. Explicit phone header/label beats bank.
/// - Bare valid VN mobiles/landlines remain in recall; exotic/foreign numbers,
///   unlabeled account-like runs, and invalid prefixes are out of scope (precision).
///
/// Redaction keeps UTF-8 boundary checks, stale `finding.text` equality, and
/// overlapping/crossing span coalescing.

/// Intended dot-atom atext (RFC 5322 subset) plus `.` with separate dot rules.
/// `|` is excluded so Markdown table delimiters stay outside the email span.
fn is_email_local_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '/'
                | '='
                | '?'
                | '^'
                | '_'
                | '`'
                | '{'
                | '}'
                | '~'
                | '.'
        )
}

fn is_email_domain_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-')
}

fn is_email_boundary_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '(' | ')'
                | '<'
                | '>'
                | '['
                | ']'
                | ':'
                | ';'
                | ','
                | '.'
                | '"'
                | '\''
                | '|'
                | '{'
                | '}'
                | '/'
                | '\\'
                | '!'
                | '?'
        )
}

/// Conservative email shape on an exact `local@domain` candidate.
fn looks_like_email(token: &str) -> bool {
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || domain.is_empty()
        || local.contains('@')
        || !domain.contains('.')
        || token.starts_with('@')
        || token.ends_with('.')
    {
        return false;
    }
    if !(local.chars().all(is_email_local_char)
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains(".."))
    {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let tld = *labels.last().unwrap_or(&"");
    if tld.len() < 2 || !tld.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            && label.chars().any(|ch| ch.is_ascii_alphabetic())
    })
}

fn is_markdown_wrapper_char(ch: char) -> bool {
    matches!(ch, '*' | '_' | '~' | '`')
}

/// Recursively peel balanced Markdown wrappers (`***` / `**` / `~~` / `__` / `*` /
/// `_` / `` ` `` and nested combinations). Only paired closers; unwrapped locals
/// keep legitimate marker characters inside the address.
fn peel_markdown_email_wrappers(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    // Longest markers first so `***` wins over `**`/`*`.
    const MARKERS: &[&str] = &["***", "**", "~~", "__", "`", "*", "_"];

    // Include leading/trailing wrapper markers around the expanded candidate so
    // nested forms like `**_email_**` / `~~**email**~~` peel outside-in.
    let mut region_start = start;
    let mut region_end = end;
    loop {
        let mut extended = false;
        for &marker in MARKERS {
            let m = marker.len();
            if region_start >= m && text.get(region_start - m..region_start) == Some(marker) {
                region_start -= m;
                extended = true;
                break;
            }
        }
        if !extended {
            break;
        }
    }
    loop {
        let mut extended = false;
        for &marker in MARKERS {
            let m = marker.len();
            if region_end + m <= text.len() && text.get(region_end..region_end + m) == Some(marker)
            {
                region_end += m;
                extended = true;
                break;
            }
        }
        if !extended {
            break;
        }
    }

    // Do not invent wrappers from ordinary prose — region must actually grow or
    // the expanded candidate itself must begin/end with wrapper markers.
    let expanded_has_wrapper_edge = text
        .get(start..end)
        .map(|s| {
            s.chars()
                .next()
                .map(is_markdown_wrapper_char)
                .unwrap_or(false)
                || s.chars()
                    .next_back()
                    .map(is_markdown_wrapper_char)
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if region_start == start && region_end == end && !expanded_has_wrapper_edge {
        return None;
    }

    let mut s = region_start;
    let mut e = region_end;
    let mut peeled = false;
    loop {
        let mut progress = false;
        for &marker in MARKERS {
            let m = marker.len();
            if e >= s + 2 * m + 3
                && text.get(s..s + m) == Some(marker)
                && text.get(e - m..e) == Some(marker)
            {
                let inner = text.get(s + m..e - m)?;
                // Allow intermediate nested wrappers; final span must be an email.
                if inner.contains('@') {
                    s += m;
                    e -= m;
                    peeled = true;
                    progress = true;
                    break;
                }
            }
        }
        if !progress {
            break;
        }
    }

    if peeled && looks_like_email(text.get(s..e)?) {
        Some((s, e))
    } else {
        None
    }
}

fn email_candidate_at(text: &str, at: usize) -> Option<(usize, usize)> {
    if !text.is_char_boundary(at) || !text[at..].starts_with('@') {
        return None;
    }
    let mut start = at;
    for (idx, ch) in text[..at].char_indices().rev() {
        if is_email_local_char(ch) {
            start = idx;
        } else {
            break;
        }
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    for (rel, ch) in text[at + 1..].char_indices() {
        if is_email_domain_char(ch) {
            end = at + 1 + rel + ch.len_utf8();
        } else {
            break;
        }
    }
    while end > at + 1 && text[..end].ends_with('.') {
        end -= 1;
    }
    let candidate = text.get(start..end)?;
    if !looks_like_email(candidate) {
        return None;
    }
    if let Some((inner_start, inner_end)) = peel_markdown_email_wrappers(text, start, end) {
        return Some((inner_start, inner_end));
    }
    // Strict both-side delimiters — reject partial carve-outs.
    let left_ok = start == 0
        || text[..start]
            .chars()
            .last()
            .map(is_email_boundary_char)
            .unwrap_or(false);
    let right_ok = end >= text.len()
        || text[end..]
            .chars()
            .next()
            .map(is_email_boundary_char)
            .unwrap_or(false);
    if left_ok && right_ok {
        Some((start, end))
    } else {
        None
    }
}

fn scan_emails(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == b'@' {
            if let Some((start, end)) = email_candidate_at(text, i) {
                if out
                    .last()
                    .map(|&(_, prev_end)| start >= prev_end)
                    .unwrap_or(true)
                {
                    out.push((start, end));
                }
                i = end;
                continue;
            }
        }
        i += 1;
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
    }
    out
}

/// Maintained active VN mobile prefixes (national form with leading `0`, length 10).
/// Includes MVNO `055` (Wintel) and `087` (iTel); omits obsolete `095`.
const VN_MOBILE_PREFIXES: &[&str] = &[
    "032", "033", "034", "035", "036", "037", "038", "039", "052", "055", "056", "058", "059",
    "070", "076", "077", "078", "079", "081", "082", "083", "084", "085", "086", "087", "088",
    "089", "090", "091", "092", "093", "094", "096", "097", "098", "099",
];

/// 2-digit geographic area codes (after trunk `0`): Hanoi / HCMC → subscriber len 8.
const VN_LANDLINE_AREA_2: &[&str] = &["24", "28"];

/// 3-digit geographic area codes (after trunk `0`) → subscriber len 7. `030` absent.
const VN_LANDLINE_AREA_3: &[&str] = &[
    "203", "204", "205", "206", "207", "208", "209", "210", "211", "212", "213", "214", "215",
    "216", "218", "219", "220", "221", "222", "225", "226", "227", "228", "229", "232", "233",
    "234", "235", "236", "237", "238", "239", "251", "252", "254", "255", "256", "257", "258",
    "259", "260", "261", "262", "263", "269", "270", "271", "272", "273", "274", "275", "276",
    "277", "290", "291", "292", "293", "294", "296", "297", "299",
];

fn normalize_vn_phone_digits(digits: &str) -> Option<String> {
    if !digits.chars().all(|ch| ch.is_ascii_digit()) || digits.is_empty() {
        return None;
    }
    if digits.starts_with("840") && digits.len() >= 12 {
        Some(format!("0{}", &digits[3..]))
    } else if digits.starts_with("84") && digits.len() >= 11 {
        Some(format!("0{}", &digits[2..]))
    } else if digits.starts_with('0') {
        Some(digits.to_string())
    } else {
        None
    }
}

fn is_vn_mobile_national0(national0: &str) -> bool {
    national0.len() == 10
        && VN_MOBILE_PREFIXES
            .iter()
            .any(|prefix| national0.starts_with(prefix))
}

fn is_vn_landline_national0(national0: &str) -> bool {
    if !national0.starts_with('0') || national0.starts_with("030") || national0.len() != 11 {
        return false;
    }
    let rest = &national0[1..];
    for area in VN_LANDLINE_AREA_2 {
        if let Some(subscriber) = rest.strip_prefix(area) {
            return subscriber.len() == 8 && subscriber.chars().all(|ch| ch.is_ascii_digit());
        }
    }
    for area in VN_LANDLINE_AREA_3 {
        if let Some(subscriber) = rest.strip_prefix(area) {
            return subscriber.len() == 7 && subscriber.chars().all(|ch| ch.is_ascii_digit());
        }
    }
    false
}

fn looks_like_vn_phone_digits(digits: &str) -> bool {
    let Some(national0) = normalize_vn_phone_digits(digits) else {
        return false;
    };
    is_vn_mobile_national0(&national0) || is_vn_landline_national0(&national0)
}

fn is_phone_separator(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '-' | '.' | '(' | ')')
}

fn first_valid_phone_prefix_len(digits: &str, require_plus84: bool) -> Option<usize> {
    if require_plus84 && !digits.starts_with("84") {
        return None;
    }
    // Shortest-first so consecutive phones split (`0912… 0987…`).
    for len in 10..=digits.len().min(13) {
        if looks_like_vn_phone_digits(&digits[..len]) {
            return Some(len);
        }
    }
    None
}

/// True when text after `ws_idx` (at whitespace) begins an independent valid VN phone.
fn lookahead_begins_independent_vn_phone(chars: &[(usize, char)], ws_idx: usize) -> bool {
    let mut idx = ws_idx;
    while idx < chars.len() && matches!(chars[idx].1, ' ' | '\t') {
        idx += 1;
    }
    if idx >= chars.len() || !chars[idx].1.is_ascii_digit() {
        return false;
    }
    let mut digits = String::new();
    while idx < chars.len() {
        let ch = chars[idx].1;
        if ch.is_ascii_digit() {
            digits.push(ch);
            idx += 1;
            if digits.len() >= 13 {
                break;
            }
        } else if matches!(ch, '-' | '.' | '(' | ')') {
            idx += 1;
        } else if matches!(ch, ' ' | '\t') {
            if digits.len() >= 9 {
                break;
            }
            idx += 1;
        } else {
            break;
        }
    }
    first_valid_phone_prefix_len(&digits, false).is_some()
}

/// Skip one plausible phone-shaped number group after a failed leading `+`.
/// At whitespace, stop when collected digits cannot form `+84` **or** the
/// lookahead begins an independent valid VN phone — so
/// `+12345678 0987654321` skips only the invalid group.
fn skip_one_phone_shaped_group(chars: &[(usize, char)], start_idx: usize) -> usize {
    let mut idx = start_idx;
    if chars.get(idx).map(|(_, ch)| *ch) == Some('+') {
        idx += 1;
    }
    let mut digits = String::new();
    let mut last_digit_end = idx;
    while idx < chars.len() {
        let ch = chars[idx].1;
        if ch.is_ascii_digit() {
            digits.push(ch);
            idx += 1;
            last_digit_end = idx;
            if digits.len() >= 13 {
                break;
            }
        } else if matches!(ch, '-' | '.' | '(' | ')') {
            idx += 1;
        } else if matches!(ch, ' ' | '\t') {
            let can_form_plus84 = digits.starts_with("84");
            if !can_form_plus84 || lookahead_begins_independent_vn_phone(chars, idx) {
                break;
            }
            idx += 1;
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return (start_idx + 1).min(chars.len());
    }
    last_digit_end
}

/// Consume one phone candidate starting at `chars[start_idx]`.
/// Preserves leading `+` and requires `+84` when `+` is present.
fn consume_phone_candidate(chars: &[(usize, char)], start_idx: usize) -> Option<(usize, String)> {
    let mut idx = start_idx;
    let require_plus84 = chars.get(idx).map(|(_, ch)| *ch) == Some('+');
    if require_plus84 {
        idx += 1;
    }
    let mut digits = String::new();
    let mut digit_end_idx = Vec::new();
    while idx < chars.len() {
        let ch = chars[idx].1;
        if ch.is_ascii_digit() {
            digits.push(ch);
            digit_end_idx.push(idx + 1);
            idx += 1;
            if digits.len() > 13 {
                break;
            }
        } else if is_phone_separator(ch) {
            idx += 1;
        } else {
            break;
        }
    }
    let len = first_valid_phone_prefix_len(&digits, require_plus84)?;
    let end_idx = digit_end_idx[len - 1];
    // Unicode alphanumeric boundaries on both sides of the full span.
    if start_idx > 0 && chars[start_idx - 1].1.is_alphanumeric() {
        return None;
    }
    if chars
        .get(end_idx)
        .map(|(_, ch)| ch.is_alphanumeric())
        .unwrap_or(false)
    {
        return None;
    }
    Some((end_idx, digits[..len].to_string()))
}

fn scan_phone_spans(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i].1;
        let left_ok = i == 0 || !chars[i - 1].1.is_alphanumeric();
        let can_start = left_ok && (ch == '+' || ch == '(' || ch.is_ascii_digit());
        if can_start {
            if let Some((end_idx, _)) = consume_phone_candidate(&chars, i) {
                let start = chars[i].0;
                let end = chars[end_idx - 1].0 + chars[end_idx - 1].1.len_utf8();
                out.push((start, end));
                i = end_idx;
                continue;
            }
            // Failed `+…`: skip one plausible group only (no retry inside), then
            // continue so a later whitespace-separated valid phone can match.
            if ch == '+' {
                i = skip_one_phone_shaped_group(&chars, i);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Digit group with internal spaces/dashes (bank / CCCD).
fn scan_digit_group_spans(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].1.is_ascii_digit() {
            i += 1;
            continue;
        }
        if i > 0 && chars[i - 1].1.is_alphanumeric() {
            i += 1;
            continue;
        }
        let start_idx = i;
        let mut digits = String::new();
        let mut end_idx = i;
        while i < chars.len() {
            let ch = chars[i].1;
            if ch.is_ascii_digit() {
                digits.push(ch);
                end_idx = i + 1;
                i += 1;
            } else if matches!(ch, ' ' | '\t' | '-')
                && i + 1 < chars.len()
                && chars[i + 1].1.is_ascii_digit()
            {
                i += 1;
            } else {
                break;
            }
        }
        if (8..=19).contains(&digits.len())
            && !chars
                .get(end_idx)
                .map(|(_, ch)| ch.is_alphanumeric())
                .unwrap_or(false)
        {
            let start = chars[start_idx].0;
            let end = chars[end_idx - 1].0 + chars[end_idx - 1].1.len_utf8();
            out.push((start, end, digits));
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExplicitLabel {
    Phone,
    Bank,
    NationalId,
}

fn label_char_boundary(folded: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0
        || !folded
            .get(..start)
            .and_then(|s| s.chars().last())
            .map(|ch| ch.is_alphanumeric())
            .unwrap_or(false);
    let after_ok = end >= folded.len()
        || !folded
            .get(end..)
            .and_then(|s| s.chars().next())
            .map(|ch| ch.is_alphanumeric())
            .unwrap_or(false);
    before_ok && after_ok
}

/// Longer compound phrases first so `tai khoan ngan hang` wins over `tai khoan`.
const LABEL_PATTERNS: &[(&str, ExplicitLabel)] = &[
    ("can cuoc cong dan so", ExplicitLabel::NationalId),
    ("so tai khoan ngan hang", ExplicitLabel::Bank),
    ("tai khoan ngan hang", ExplicitLabel::Bank),
    ("so tk ngan hang", ExplicitLabel::Bank),
    ("stk ngan hang", ExplicitLabel::Bank),
    ("so dien thoai", ExplicitLabel::Phone),
    ("can cuoc cong dan", ExplicitLabel::NationalId),
    ("so tai khoan", ExplicitLabel::Bank),
    ("dien thoai", ExplicitLabel::Phone),
    ("cccd so", ExplicitLabel::NationalId),
    ("cmnd so", ExplicitLabel::NationalId),
    ("can cuoc so", ExplicitLabel::NationalId),
    ("can cuoc", ExplicitLabel::NationalId),
    ("tai khoan", ExplicitLabel::Bank),
    ("hotline", ExplicitLabel::Phone),
    ("mobile", ExplicitLabel::Phone),
    ("phone", ExplicitLabel::Phone),
    ("so tk", ExplicitLabel::Bank),
    ("cccd", ExplicitLabel::NationalId),
    ("cmnd", ExplicitLabel::NationalId),
    ("sdt", ExplicitLabel::Phone),
    ("tel", ExplicitLabel::Phone),
    ("stk", ExplicitLabel::Bank),
];

/// Competing field keys (folded) — not bare conjunctions like `và` (joint owners).
const COMPETING_FIELD_KEYS: &[&str] = &[
    "ma giao dich",
    "ma gd",
    "so tham chieu",
    "ma tham chieu",
    "so dien thoai",
    "dien thoai",
    "hotline",
    "mobile",
    "phone",
    "sdt",
    "tel",
    "cccd",
    "cmnd",
    "stk",
    "so tk",
];

/// Match `needle` in `haystack` with Unicode alphanumeric token borders so
/// `(mã giao dịch)`, `/mã giao dịch`, and `—mã giao dịch` all count.
fn contains_token_phrase(haystack: &str, needle: &str) -> bool {
    let mut base = 0usize;
    while base < haystack.len() {
        let Some(rel) = haystack[base..].find(needle) else {
            break;
        };
        let start = base + rel;
        let end = start + needle.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .last()
                .map(|ch| ch.is_alphanumeric())
                .unwrap_or(false);
        let after_ok = end >= haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .map(|ch| ch.is_alphanumeric())
                .unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        base = start + 1;
        while base < haystack.len() && !haystack.is_char_boundary(base) {
            base += 1;
        }
    }
    false
}

/// Competing field keys in the qualifier before `:/=` break the field link
/// (e.g. closed-account prose + `mã giao dịch`). Bare `và` alone does not.
fn between_has_soft_clause_break(before_sep: &str) -> bool {
    COMPETING_FIELD_KEYS
        .iter()
        .any(|key| contains_token_phrase(before_sep, key))
}

fn classify_label_text(folded: &str) -> Option<ExplicitLabel> {
    let mut best: Option<(usize, usize, ExplicitLabel)> = None;
    for &(pat, kind) in LABEL_PATTERNS {
        let mut base = 0usize;
        while base < folded.len() {
            let Some(rel) = folded[base..].find(pat) else {
                break;
            };
            let start = base + rel;
            let end = start + pat.len();
            if label_char_boundary(folded, start, end) {
                let take = match best {
                    None => true,
                    Some((prev_start, prev_end, _)) => {
                        end > prev_end || (end == prev_end && start >= prev_start)
                    }
                };
                if take {
                    best = Some((start, end, kind));
                }
            }
            base = start + 1;
            while base < folded.len() && !folded.is_char_boundary(base) {
                base += 1;
            }
        }
    }
    best.map(|(_, _, kind)| kind)
}

/// Field-value link between a label and the candidate at the end of `folded_scope`.
/// - No-colon compound labels: tight whitespace only (`Tài khoản ngân hàng 123…`).
/// - Terminal `:/=/：` strongly binds and allows a longer qualifier before the sep,
///   unless a conjunction / competing field key breaks the clause.
fn field_value_link(between: &str) -> bool {
    const MAX_BETWEEN_CHARS: usize = 64;
    if between.chars().count() > MAX_BETWEEN_CHARS {
        return false;
    }
    if between.chars().any(is_label_scope_boundary) {
        return false;
    }
    let trimmed = between.trim();
    if trimmed.is_empty() {
        // Tight `Label 123…` / compound no-colon — only a few spaces.
        return between.chars().count() <= 3;
    }
    let sep = trimmed.rfind(|ch| matches!(ch, ':' | '=' | '：'));
    let Some(sep_at) = sep else {
        // Without a terminal binder, letters between label and value are prose.
        return false;
    };
    let sep_ch = trimmed[sep_at..].chars().next().unwrap_or(':');
    let after = trimmed[sep_at + sep_ch.len_utf8()..].trim();
    if !after.is_empty() {
        return false;
    }
    let before = trimmed[..sep_at].trim();
    // Strong `:/=` bind: allow a longer qualifier, still no nested clause punctuation
    // or competing field keys (`mã giao dịch`, `SĐT`, `STK`, …). Bare `và` is fine.
    before.chars().count() <= 48
        && !before
            .chars()
            .any(|ch| is_label_scope_boundary(ch) || matches!(ch, ',' | '，' | '、'))
        && !between_has_soft_clause_break(before)
}

/// Nearest explicit label in scoped folded prefix that field-links to the value.
fn nearest_explicit_label(folded_prefix: &str) -> Option<ExplicitLabel> {
    let value_at = folded_prefix.len();
    let mut best: Option<(usize, ExplicitLabel)> = None;
    for &(pat, kind) in LABEL_PATTERNS {
        let mut base = 0usize;
        while base < folded_prefix.len() {
            let Some(rel) = folded_prefix[base..].find(pat) else {
                break;
            };
            let start = base + rel;
            let end = start + pat.len();
            if label_char_boundary(folded_prefix, start, end)
                && field_value_link(&folded_prefix[end..value_at])
            {
                let take = best.map(|(prev_end, _)| end >= prev_end).unwrap_or(true);
                if take {
                    best = Some((end, kind));
                }
            }
            base = start + 1;
            while base < folded_prefix.len() && !folded_prefix.is_char_boundary(base) {
                base += 1;
            }
        }
    }
    best.map(|(_, kind)| kind)
}

fn spans_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

fn is_label_scope_boundary(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '|' | '.' | '!' | '?' | ';' | ',' | '，' | '、' | '…' | '。' | '！' | '？'
    )
}

/// Prefix for label association: cannot cross newline, table pipe, or clause boundary.
fn label_scope_before<'a>(markdown: &'a str, start: usize) -> &'a str {
    let prefix_end = start.min(markdown.len());
    let prefix = markdown.get(..prefix_end).unwrap_or("");
    let mut cut = 0usize;
    for (idx, ch) in prefix.char_indices() {
        if is_label_scope_boundary(ch) {
            cut = idx + ch.len_utf8();
        }
    }
    &prefix[cut..]
}

fn label_window_before(markdown: &str, start: usize) -> String {
    accent_fold(label_scope_before(markdown, start))
}

/// Map a byte offset inside a Markdown table body cell to its column-header label.
fn table_column_label_at(markdown: &str, offset: usize) -> Option<ExplicitLabel> {
    let doc = CorpusDocument {
        source_rel: "_pii_table_".into(),
        md_rel: "_pii_table_.md".into(),
        format: "markdown".into(),
        markdown: markdown.to_string(),
    };
    for table in parse_markdown_tables(&doc) {
        if offset < table.start || offset >= table.end || table.rows.len() < 2 {
            continue;
        }
        let headers = &table.rows[0];
        let region = &markdown[table.start..table.end];
        let mut line_start = table.start;
        let mut row_idx = 0usize;
        for line in region.split_inclusive('\n') {
            let line_end = line_start + line.len();
            if offset >= line_start && offset < line_end {
                // rows[0]=header, rows[1]=separator; data starts at row_idx>=2 in line walk
                // when separator consumed as line 1.
                if row_idx < 2 {
                    return None;
                }
                let content = line.trim_end_matches('\n').trim_end_matches('\r');
                let line_body_start = line_start + line.len() - line.trim_start().len();
                if !content.trim_start().starts_with('|') {
                    return None;
                }
                let mut i = line_body_start;
                if markdown.as_bytes().get(i) == Some(&b'|') {
                    i += 1;
                }
                let line_content_end = line_start + content.len();
                let mut scan_end = line_content_end;
                if content.trim_start().ends_with('|') {
                    scan_end = line_content_end - 1;
                }
                let mut col = 0usize;
                let mut cell_start = i;
                let mut escaped = false;
                let bytes = markdown.as_bytes();
                while i < scan_end {
                    let ch = bytes[i];
                    if escaped {
                        escaped = false;
                        i += 1;
                        continue;
                    }
                    if ch == b'\\' {
                        escaped = true;
                        i += 1;
                        continue;
                    }
                    if ch == b'|' {
                        if offset >= cell_start && offset < i {
                            return classify_label_text(&accent_fold(headers.get(col)?.as_str()));
                        }
                        col += 1;
                        cell_start = i + 1;
                    }
                    i += 1;
                }
                if offset >= cell_start && offset < line_end {
                    return classify_label_text(&accent_fold(headers.get(col)?.as_str()));
                }
                return None;
            }
            line_start = line_end;
            row_idx += 1;
        }
    }
    None
}

fn resolve_label(markdown: &str, start: usize) -> Option<ExplicitLabel> {
    table_column_label_at(markdown, start)
        .or_else(|| nearest_explicit_label(&label_window_before(markdown, start)))
}

fn detect_pii_in_markdown(markdown: &str, source_rel: &str) -> Vec<PiiFinding> {
    let emails = scan_emails(markdown);
    let mut occupied: Vec<(usize, usize)> = emails.clone();
    let mut findings = Vec::new();

    for (start, end) in emails {
        findings.push(PiiFinding {
            kind: PiiKind::Email,
            text: markdown[start..end].to_string(),
            source_rel: source_rel.into(),
            start,
            end,
            confidence: 0.98,
        });
    }

    for (start, end) in scan_phone_spans(markdown) {
        if occupied
            .iter()
            .any(|&span| spans_overlap(span, (start, end)))
        {
            continue;
        }
        let label = resolve_label(markdown, start);
        let (kind, confidence) = if label == Some(ExplicitLabel::Bank) {
            (PiiKind::BankAccount, 0.8)
        } else {
            let confidence = if label == Some(ExplicitLabel::Phone) {
                0.92
            } else {
                0.9
            };
            (PiiKind::Phone, confidence)
        };
        findings.push(PiiFinding {
            kind,
            text: markdown[start..end].to_string(),
            source_rel: source_rel.into(),
            start,
            end,
            confidence,
        });
        occupied.push((start, end));
    }

    for (start, end, digits) in scan_digit_group_spans(markdown) {
        if occupied
            .iter()
            .any(|&span| spans_overlap(span, (start, end)))
        {
            continue;
        }
        let label = resolve_label(markdown, start);
        let kind = match label {
            Some(ExplicitLabel::Bank) if (8..=19).contains(&digits.len()) => {
                Some((PiiKind::BankAccount, 0.8))
            }
            Some(ExplicitLabel::NationalId) if digits.len() == 9 || digits.len() == 12 => {
                Some((PiiKind::NationalId, 0.95))
            }
            _ => None,
        };
        if let Some((kind, confidence)) = kind {
            findings.push(PiiFinding {
                kind,
                text: markdown[start..end].to_string(),
                source_rel: source_rel.into(),
                start,
                end,
                confidence,
            });
            occupied.push((start, end));
        }
    }

    findings.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    findings
}

pub fn detect_pii(documents: &[CorpusDocument]) -> PiiReport {
    let mut findings = Vec::new();
    for document in documents {
        findings.extend(detect_pii_in_markdown(
            &document.markdown,
            &document.source_rel,
        ));
    }
    let mut counts = BTreeMap::new();
    for finding in &findings {
        *counts.entry(finding.kind.clone()).or_default() += 1;
    }
    PiiReport { findings, counts }
}

pub fn redact_pii(markdown: &str, findings: &[PiiFinding]) -> String {
    let mut output = markdown.to_string();
    let mut spans: Vec<(usize, usize, &PiiKind)> = findings
        .iter()
        .filter(|finding| {
            finding.end <= output.len()
                && finding.start < finding.end
                && output.is_char_boundary(finding.start)
                && output.is_char_boundary(finding.end)
                // Stale findings (edited markdown / shifted spans) must not punch holes.
                && output.get(finding.start..finding.end) == Some(finding.text.as_str())
        })
        .map(|finding| (finding.start, finding.end, &finding.kind))
        .collect();
    // Coalesce overlapping / crossing / nested ranges — tránh lộ suffix nhạy cảm.
    spans.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut merged: Vec<(usize, usize, &PiiKind)> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut() {
            if span.0 < last.1 {
                last.1 = last.1.max(span.1);
                continue;
            }
        }
        merged.push(span);
    }
    // Áp từ phải sang trái để offset bên trái không lệch.
    for (start, end, kind) in merged.into_iter().rev() {
        output.replace_range(start..end, &format!("[REDACTED_{kind:?}]"));
    }
    output
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            cell.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            cells.push(cell.trim().to_string());
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().to_string());
    Some(cells)
}

fn separator_row(row: &[String]) -> bool {
    !row.is_empty()
        && row.iter().all(|cell| {
            cell.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                && cell.chars().filter(|&ch| ch == '-').count() >= 3
        })
}

pub fn parse_markdown_tables(document: &CorpusDocument) -> Vec<MarkdownTable> {
    let mut tables = Vec::new();
    let lines: Vec<&str> = document.markdown.split_inclusive('\n').collect();
    let mut offsets = Vec::with_capacity(lines.len());
    let mut offset = 0usize;
    for line in &lines {
        offsets.push(offset);
        offset += line.len();
    }
    let mut index = 0usize;
    while index + 1 < lines.len() {
        let Some(header) = parse_table_row(lines[index]) else {
            index += 1;
            continue;
        };
        let Some(separator) = parse_table_row(lines[index + 1]) else {
            index += 1;
            continue;
        };
        if header.len() != separator.len() || !separator_row(&separator) {
            index += 1;
            continue;
        }
        let start_line = index;
        let mut rows = vec![header];
        index += 2;
        while index < lines.len() {
            let Some(row) = parse_table_row(lines[index]) else {
                break;
            };
            if row.len() != rows[0].len() {
                break;
            }
            rows.push(row);
            index += 1;
        }
        let start = offsets[start_line];
        let end = if index < offsets.len() {
            offsets[index]
        } else {
            document.markdown.len()
        };
        tables.push(MarkdownTable {
            id: markdown_table_id(&document.source_rel, tables.len(), start),
            source_rel: document.source_rel.clone(),
            index: tables.len(),
            start,
            end,
            rows,
        });
    }
    tables
}

fn escape_cell(cell: &str) -> String {
    cell.replace('|', "\\|").replace('\n', "<br>")
}

pub fn render_markdown_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() || rows[0].is_empty() {
        return String::new();
    }
    let cols = rows[0].len();
    let mut output = String::new();
    let write_row = |output: &mut String, row: &[String]| {
        output.push('|');
        for column in 0..cols {
            output.push_str(&escape_cell(
                row.get(column).map(String::as_str).unwrap_or(""),
            ));
            output.push('|');
        }
        output.push('\n');
    };
    write_row(&mut output, &rows[0]);
    output.push('|');
    for _ in 0..cols {
        output.push_str("---|");
    }
    output.push('\n');
    for row in rows.iter().skip(1) {
        write_row(&mut output, row);
    }
    output
}

pub fn update_markdown_table(
    markdown: &str,
    table: &MarkdownTable,
    rows: &[Vec<String>],
) -> Result<String, ConvertError> {
    if table.end > markdown.len()
        || table.start >= table.end
        || !markdown.is_char_boundary(table.start)
        || !markdown.is_char_boundary(table.end)
    {
        return Err(ConvertError::Failed("span bảng không hợp lệ".into()));
    }
    // Reparse current markdown and require an exact table match (id/start/end/rows).
    let doc = CorpusDocument {
        source_rel: table.source_rel.clone(),
        md_rel: table.source_rel.clone(),
        format: "markdown".into(),
        markdown: markdown.to_string(),
    };
    let matched = parse_markdown_tables(&doc).into_iter().any(|current| {
        current.id == table.id
            && current.start == table.start
            && current.end == table.end
            && current.rows == table.rows
    });
    if !matched {
        return Err(ConvertError::Failed(
            "conflict: bảng đã thay đổi hoặc span không khớp".into(),
        ));
    }
    let rendered = render_markdown_table(rows);
    let mut updated = markdown.to_string();
    updated.replace_range(table.start..table.end, &rendered);
    Ok(updated)
}

pub fn table_to_csv(rows: &[Vec<String>]) -> Result<Vec<u8>, ConvertError> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    for row in rows {
        let safe: Vec<String> = row
            .iter()
            .map(|cell| {
                if cell.trim_start().starts_with(['=', '+', '-', '@']) {
                    format!("'{cell}")
                } else {
                    cell.clone()
                }
            })
            .collect();
        writer
            .write_record(&safe)
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
    }
    writer
        .into_inner()
        .map_err(|error| ConvertError::Failed(error.to_string()))
}

fn infer_type(values: &[String]) -> FieldType {
    let meaningful: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .collect();
    if meaningful.is_empty() {
        return FieldType::String;
    }
    if meaningful
        .iter()
        .all(|value| value.replace(['.', ',', ' '], "").parse::<f64>().is_ok())
    {
        return FieldType::Number;
    }
    if meaningful.iter().all(|value| {
        let pieces: Vec<&str> = value.split(['/', '-']).collect();
        pieces.len() == 3 && pieces.iter().all(|piece| piece.parse::<u32>().is_ok())
    }) {
        return FieldType::Date;
    }
    if meaningful.iter().all(|value| {
        matches!(
            accent_fold(value).as_str(),
            "true" | "false" | "co" | "khong" | "yes" | "no"
        )
    }) {
        return FieldType::Boolean;
    }
    FieldType::String
}

pub fn extract_schema(document: &CorpusDocument) -> DocumentSchema {
    let headings = document
        .markdown
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix('#')
                .map(|heading| heading.trim_start_matches('#').trim().to_string())
        })
        .filter(|heading| !heading.is_empty())
        .collect();
    let tables = parse_markdown_tables(document);
    let mut fields = Vec::new();
    for table in &tables {
        if let Some(header) = table.rows.first() {
            for (column, name) in header.iter().enumerate() {
                let examples: Vec<String> = table
                    .rows
                    .iter()
                    .skip(1)
                    .filter_map(|row| row.get(column))
                    .filter(|value| !value.is_empty())
                    .take(3)
                    .cloned()
                    .collect();
                fields.push(SchemaField {
                    name: name.clone(),
                    field_type: infer_type(&examples),
                    examples,
                });
            }
        }
    }
    let existing: HashSet<String> = fields
        .iter()
        .map(|field| accent_fold(&field.name))
        .collect();
    for line in document.markdown.lines() {
        if let Some((label, value)) = line.split_once(':') {
            let label = label.trim().trim_start_matches(['-', '*', '#', ' ']);
            let value = value.trim();
            if (3..=60).contains(&label.chars().count())
                && !value.is_empty()
                && !existing.contains(&accent_fold(label))
            {
                fields.push(SchemaField {
                    name: label.to_string(),
                    field_type: infer_type(&[value.to_string()]),
                    examples: vec![value.to_string()],
                });
            }
        }
    }
    DocumentSchema {
        source_rel: document.source_rel.clone(),
        headings,
        fields,
        tables,
    }
}

pub fn diff_markdown(old: &str, new: &str) -> Vec<DiffHunk> {
    if old == new {
        return vec![DiffHunk {
            kind: DiffKind::Unchanged,
            old_start: 0,
            new_start: 0,
            old_text: old.to_string(),
            new_text: new.to_string(),
        }];
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut old_suffix = old_lines.len();
    let mut new_suffix = new_lines.len();
    while old_suffix > prefix
        && new_suffix > prefix
        && old_lines[old_suffix - 1] == new_lines[new_suffix - 1]
    {
        old_suffix -= 1;
        new_suffix -= 1;
    }
    let old_text = old_lines[prefix..old_suffix].join("\n");
    let new_text = new_lines[prefix..new_suffix].join("\n");
    let kind = if old_text.is_empty() {
        DiffKind::Added
    } else if new_text.is_empty() {
        DiffKind::Removed
    } else {
        DiffKind::Modified
    };
    vec![DiffHunk {
        kind,
        old_start: prefix,
        new_start: prefix,
        old_text,
        new_text,
    }]
}

pub fn three_way_merge(base: &str, ours: &str, theirs: &str) -> MergeResult {
    if ours == theirs {
        return MergeResult {
            markdown: ours.to_string(),
            conflicts: Vec::new(),
        };
    }
    if ours == base {
        return MergeResult {
            markdown: theirs.to_string(),
            conflicts: Vec::new(),
        };
    }
    if theirs == base {
        return MergeResult {
            markdown: ours.to_string(),
            conflicts: Vec::new(),
        };
    }
    let markdown =
        format!("<<<<<<< BẢN ĐANG SỬA\n{ours}\n=======\n{theirs}\n>>>>>>> BẢN CONVERT MỚI\n");
    MergeResult {
        markdown,
        conflicts: vec![MergeConflict {
            index: 0,
            ours: ours.to_string(),
            theirs: theirs.to_string(),
        }],
    }
}

pub fn watch_pattern_matches(pattern: &str, file_name: &str) -> bool {
    fn matches(pattern: &[u8], text: &[u8]) -> bool {
        if pattern.is_empty() {
            return text.is_empty();
        }
        match pattern[0] {
            b'*' => {
                matches(&pattern[1..], text) || (!text.is_empty() && matches(pattern, &text[1..]))
            }
            b'?' => !text.is_empty() && matches(&pattern[1..], &text[1..]),
            ch => {
                !text.is_empty()
                    && ch.to_ascii_lowercase() == text[0].to_ascii_lowercase()
                    && matches(&pattern[1..], &text[1..])
            }
        }
    }
    matches(pattern.as_bytes(), file_name.as_bytes())
}

fn source_quote(chunk: &CorpusChunk) -> String {
    chunk
        .text
        .split_whitespace()
        .take(55)
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    let folded = accent_fold(haystack);
    needles.iter().any(|needle| folded.contains(needle))
}

fn push_item(
    items: &mut Vec<HandoffItem>,
    counters: &mut HashMap<&'static str, usize>,
    prefix: &'static str,
    kind: HandoffItemKind,
    text: String,
    citation: String,
    status: &str,
    parent_id: Option<String>,
) -> String {
    let count = counters.entry(prefix).or_default();
    *count += 1;
    let id = format!("{prefix}-{:03}", *count);
    items.push(HandoffItem {
        id: id.clone(),
        kind,
        text,
        citations: vec![citation],
        status: status.to_string(),
        parent_id,
    });
    id
}

fn extract_handoff_items(
    chunks: &[CorpusChunk],
    citations: &[Citation],
) -> (Vec<HandoffItem>, Vec<TraceabilityRow>) {
    let mut items = Vec::new();
    let mut counters = HashMap::new();
    let mut traceability = Vec::new();
    let cite_by_chunk: HashMap<&str, &Citation> = chunks
        .iter()
        .zip(citations)
        .map(|(chunk, citation)| (chunk.id.as_str(), citation))
        .collect();

    for chunk in chunks {
        let Some(citation) = cite_by_chunk.get(chunk.id.as_str()) else {
            continue;
        };
        let mut chunk_requirement_ids = Vec::new();
        for line in chunk.text.lines() {
            let clean = line
                .trim()
                .trim_start_matches(['-', '*', '•', '▪', ' ', '\t']);
            if clean.chars().count() < 12 {
                continue;
            }
            if contains_any(
                clean,
                &[
                    "phai",
                    "can ",
                    "bat buoc",
                    "khong duoc",
                    "yeu cau",
                    "shall",
                    "must",
                ],
            ) {
                let functional = contains_any(
                    clean,
                    &[
                        "he thong",
                        "san pham",
                        "ung dung",
                        "nguoi dung",
                        "api",
                        "chuc nang",
                    ],
                );
                let (prefix, kind) = if functional {
                    ("FR", HandoffItemKind::FunctionalRequirement)
                } else {
                    ("BR", HandoffItemKind::BusinessRequirement)
                };
                let id = push_item(
                    &mut items,
                    &mut counters,
                    prefix,
                    kind,
                    clean.to_string(),
                    citation.id.clone(),
                    "draft",
                    None,
                );
                chunk_requirement_ids.push(id);
            }
            if contains_any(clean, &["gia dinh", "tam thoi", "uoc luong"]) {
                push_item(
                    &mut items,
                    &mut counters,
                    "AS",
                    HandoffItemKind::Assumption,
                    clean.to_string(),
                    citation.id.clone(),
                    "needs_confirmation",
                    None,
                );
            }
            if clean.contains('?') || contains_any(clean, &["tbd", "todo", "can lam ro", "chua ro"])
            {
                push_item(
                    &mut items,
                    &mut counters,
                    "Q",
                    HandoffItemKind::OpenQuestion,
                    clean.to_string(),
                    citation.id.clone(),
                    "open",
                    None,
                );
            }
            if contains_any(clean, &["la ", "toi muon", "de "])
                && clean.to_lowercase().contains("tôi muốn")
            {
                push_item(
                    &mut items,
                    &mut counters,
                    "US",
                    HandoffItemKind::UserStory,
                    clean.to_string(),
                    citation.id.clone(),
                    "draft",
                    chunk_requirement_ids.last().cloned(),
                );
            }
            if (contains_any(clean, &["given", "when", "then"])
                || contains_any(clean, &["cho truoc", "khi ", "thi "]))
                && clean.len() > 20
            {
                push_item(
                    &mut items,
                    &mut counters,
                    "AC",
                    HandoffItemKind::AcceptanceCriterion,
                    clean.to_string(),
                    citation.id.clone(),
                    "draft",
                    None,
                );
            }
        }

        if chunk.heading.chars().count() >= 3 {
            push_item(
                &mut items,
                &mut counters,
                "TERM",
                HandoffItemKind::Glossary,
                format!(
                    "{} — {}",
                    chunk.heading,
                    source_quote(chunk).chars().take(180).collect::<String>()
                ),
                citation.id.clone(),
                "draft",
                None,
            );
        }
    }

    // Produce reviewable skeletons for requirements that do not already have a
    // user story/AC/test case. They are marked needs_elaboration, never approved.
    let requirements: Vec<HandoffItem> = items
        .iter()
        .filter(|item| {
            matches!(
                item.kind,
                HandoffItemKind::BusinessRequirement | HandoffItemKind::FunctionalRequirement
            )
        })
        .cloned()
        .collect();
    for requirement in requirements {
        let citation = requirement.citations[0].clone();
        let user_story = push_item(
            &mut items,
            &mut counters,
            "US",
            HandoffItemKind::UserStory,
            format!(
                "Là người dùng liên quan, tôi muốn {}, để đáp ứng yêu cầu nghiệp vụ.",
                requirement.text.trim_end_matches('.').to_lowercase()
            ),
            citation.clone(),
            "needs_elaboration",
            Some(requirement.id.clone()),
        );
        let acceptance = push_item(
            &mut items,
            &mut counters,
            "AC",
            HandoffItemKind::AcceptanceCriterion,
            format!(
                "Given bối cảnh nguồn, When thực hiện yêu cầu {}, Then kết quả phải khớp trích dẫn.",
                requirement.id
            ),
            citation.clone(),
            "needs_elaboration",
            Some(user_story.clone()),
        );
        let test_case = push_item(
            &mut items,
            &mut counters,
            "TC",
            HandoffItemKind::TestCase,
            format!(
                "Xác minh {} bằng dữ liệu và kết quả mong đợi trong nguồn.",
                acceptance
            ),
            citation.clone(),
            "needs_elaboration",
            Some(acceptance.clone()),
        );
        traceability.push(TraceabilityRow {
            br: matches!(requirement.kind, HandoffItemKind::BusinessRequirement)
                .then(|| requirement.id.clone()),
            fr: matches!(requirement.kind, HandoffItemKind::FunctionalRequirement)
                .then(|| requirement.id.clone()),
            user_story: Some(user_story),
            acceptance_criterion: Some(acceptance),
            test_case: Some(test_case),
            citations: vec![citation],
        });
    }
    (items, traceability)
}

fn items_of<'a>(items: &'a [HandoffItem], kind: HandoffItemKind) -> Vec<&'a HandoffItem> {
    items.iter().filter(|item| item.kind == kind).collect()
}

fn citation_refs(item: &HandoffItem) -> String {
    item.citations
        .iter()
        .map(|citation| format!("[{citation}]"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_requirements(items: &[&HandoffItem]) -> String {
    if items.is_empty() {
        return "_Chưa trích được yêu cầu có căn cứ; xem Câu hỏi mở._\n".into();
    }
    let mut output = String::from("| ID | Mô tả | Trạng thái | Trích dẫn |\n|---|---|---|---|\n");
    for item in items {
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            item.id,
            escape_cell(&item.text),
            item.status,
            citation_refs(item)
        ));
    }
    output
}

fn render_handoff_artifacts(
    options: &HandoffOptions,
    items: &[HandoffItem],
    traceability: &[TraceabilityRow],
    citations: &[Citation],
) -> BTreeMap<String, String> {
    let brs = items_of(items, HandoffItemKind::BusinessRequirement);
    let frs = items_of(items, HandoffItemKind::FunctionalRequirement);
    let stories = items_of(items, HandoffItemKind::UserStory);
    let criteria = items_of(items, HandoffItemKind::AcceptanceCriterion);
    let tests = items_of(items, HandoffItemKind::TestCase);
    let glossary = items_of(items, HandoffItemKind::Glossary);
    let assumptions = items_of(items, HandoffItemKind::Assumption);
    let questions = items_of(items, HandoffItemKind::OpenQuestion);

    let mut artifacts = BTreeMap::new();
    artifacts.insert(
        "00-README.md".into(),
        format!(
            "# Handoff Pack — {}\n\n- Chế độ: `{:?}`\n- Ngôn ngữ: {}\n- Trích dẫn: {}\n\nMọi mục `needs_elaboration` cần BA/PM rà soát trước khi phê duyệt.\n",
            options.product_name,
            options.mode,
            options.locale,
            citations.len()
        ),
    );
    artifacts.insert(
        "01-BRD.md".into(),
        format!(
            "# Tài liệu yêu cầu nghiệp vụ (BRD) — {}\n\n## 1. Bối cảnh và mục tiêu\n\nCác mục dưới đây được trích từ corpus, không bổ sung dữ kiện ngoài nguồn.\n\n## 2. Yêu cầu nghiệp vụ\n\n{}\n## 3. Giả định\n\n{}\n## 4. Câu hỏi mở\n\n{}\n",
            options.product_name,
            render_requirements(&brs),
            render_requirements(&assumptions),
            render_requirements(&questions)
        ),
    );
    artifacts.insert(
        "02-PRD.md".into(),
        format!(
            "# Tài liệu yêu cầu sản phẩm (PRD) — {}\n\n## 1. Tóm tắt sản phẩm\n\nBản nháp tất định dựa trên tài liệu đã chọn.\n\n## 2. Yêu cầu chức năng\n\n{}\n## 3. User stories\n\n{}\n## 4. Tiêu chí chấp nhận\n\n{}\n",
            options.product_name,
            render_requirements(&frs),
            render_requirements(&stories),
            render_requirements(&criteria)
        ),
    );
    artifacts.insert(
        "03-USER-STORIES.md".into(),
        format!("# User stories\n\n{}", render_requirements(&stories)),
    );
    artifacts.insert(
        "04-ACCEPTANCE-CRITERIA.md".into(),
        format!("# Tiêu chí chấp nhận\n\n{}", render_requirements(&criteria)),
    );
    artifacts.insert(
        "05-GLOSSARY.md".into(),
        format!("# Thuật ngữ\n\n{}", render_requirements(&glossary)),
    );
    artifacts.insert(
        "06-TEST-CASES.md".into(),
        format!("# Kịch bản kiểm thử\n\n{}", render_requirements(&tests)),
    );

    let mut trace = String::from(
        "# Ma trận truy vết\n\n| BR | FR | US | AC | TC | Trích dẫn |\n|---|---|---|---|---|---|\n",
    );
    for row in traceability {
        trace.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            row.br.as_deref().unwrap_or("—"),
            row.fr.as_deref().unwrap_or("—"),
            row.user_story.as_deref().unwrap_or("—"),
            row.acceptance_criterion.as_deref().unwrap_or("—"),
            row.test_case.as_deref().unwrap_or("—"),
            row.citations
                .iter()
                .map(|citation| format!("[{citation}]"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    artifacts.insert("07-TRACEABILITY.md".into(), trace);
    artifacts.insert(
        "08-ASSUMPTIONS-QUESTIONS.md".into(),
        format!(
            "# Giả định và câu hỏi mở\n\n## Giả định\n\n{}\n## Câu hỏi mở\n\n{}",
            render_requirements(&assumptions),
            render_requirements(&questions)
        ),
    );

    let mut jira = String::from("Summary,Description,Issue Type,Labels\n");
    for story in &stories {
        let summary = story.text.replace('"', "\"\"");
        jira.push_str(&format!(
            "\"{}\",\"{} {}\",\"Story\",\"markhand-handoff\"\n",
            summary,
            citation_refs(story),
            story.status
        ));
    }
    artifacts.insert("09-JIRA-IMPORT.csv".into(), jira);

    let mut github = format!(
        "# GitHub issue drafts — {}\n\n> Import/review manually; generated items remain draft until approved.\n\n",
        options.product_name
    );
    for story in &stories {
        github.push_str(&format!(
            "## {} — {}\n\n- Trạng thái: `{}`\n- Trích dẫn: {}\n\n",
            story.id,
            story.text,
            story.status,
            citation_refs(story)
        ));
    }
    artifacts.insert("10-GITHUB-ISSUES.md".into(), github);

    artifacts.insert(
        "11-CONFLUENCE.md".into(),
        format!(
            "# {} — BRD/PRD\n\n## Yêu cầu nghiệp vụ\n\n{}\n## Yêu cầu chức năng\n\n{}\n## User stories\n\n{}",
            options.product_name,
            render_requirements(&brs),
            render_requirements(&frs),
            render_requirements(&stories)
        ),
    );
    artifacts.insert(
        "12-OBSIDIAN-MOC.md".into(),
        "# Mục lục bàn giao\n\n\
         - [[01-BRD]]\n- [[02-PRD]]\n- [[03-USER-STORIES]]\n\
         - [[04-ACCEPTANCE-CRITERIA]]\n- [[05-GLOSSARY]]\n\
         - [[06-TEST-CASES]]\n- [[07-TRACEABILITY]]\n\
         - [[08-ASSUMPTIONS-QUESTIONS]]\n"
            .into(),
    );
    artifacts
}

pub fn validate_handoff(
    items: &[HandoffItem],
    citations: &[Citation],
    traceability: &[TraceabilityRow],
    strict: bool,
) -> HandoffValidation {
    let citation_map: HashMap<&str, &Citation> = citations
        .iter()
        .map(|citation| (citation.id.as_str(), citation))
        .collect();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut cited = 0usize;
    let mut ids = HashSet::new();
    for item in items {
        if !ids.insert(&item.id) {
            errors.push(ValidationMessage {
                code: "DUPLICATE_ID".into(),
                item_id: Some(item.id.clone()),
                message: "ID bị trùng.".into(),
            });
        }
        let valid_citations = item
            .citations
            .iter()
            .filter(|citation| citation_map.contains_key(citation.as_str()))
            .count();
        if valid_citations > 0 {
            cited += 1;
        } else {
            let message = ValidationMessage {
                code: "MISSING_CITATION".into(),
                item_id: Some(item.id.clone()),
                message: "Mục không có trích dẫn hợp lệ.".into(),
            };
            if strict {
                errors.push(message);
            } else {
                warnings.push(message);
            }
        }
        if item.status == "needs_elaboration" {
            warnings.push(ValidationMessage {
                code: "NEEDS_ELABORATION".into(),
                item_id: Some(item.id.clone()),
                message: "Cần BA/PM hoàn thiện và phê duyệt.".into(),
            });
        }
        let factual = matches!(
            item.kind,
            HandoffItemKind::BusinessRequirement
                | HandoffItemKind::FunctionalRequirement
                | HandoffItemKind::Assumption
                | HandoffItemKind::OpenQuestion
        );
        if factual && item.status != "needs_elaboration" {
            let item_tokens: HashSet<String> = tokens(&item.text).into_iter().collect();
            let source_tokens: HashSet<String> = item
                .citations
                .iter()
                .filter_map(|id| citation_map.get(id.as_str()))
                .flat_map(|citation| tokens(&citation.quote))
                .collect();
            let grounded = item_tokens.is_empty()
                || item_tokens
                    .iter()
                    .filter(|token| source_tokens.contains(*token))
                    .count()
                    * 100
                    >= item_tokens.len() * 70;
            if !grounded {
                let message = ValidationMessage {
                    code: "CITATION_GROUNDING_WEAK".into(),
                    item_id: Some(item.id.clone()),
                    message: "Nội dung không khớp đủ với đoạn nguồn được trích.".into(),
                };
                if strict {
                    errors.push(message);
                } else {
                    warnings.push(message);
                }
            }
        }
    }
    if !items.iter().any(|item| {
        matches!(
            item.kind,
            HandoffItemKind::BusinessRequirement | HandoffItemKind::FunctionalRequirement
        )
    }) {
        let message = ValidationMessage {
            code: "NO_REQUIREMENTS".into(),
            item_id: None,
            message: "Không trích được BR/FR có căn cứ từ corpus.".into(),
        };
        if strict {
            errors.push(message);
        } else {
            warnings.push(message);
        }
    }
    let citation_coverage = if items.is_empty() {
        0.0
    } else {
        cited as f32 / items.len() as f32
    };
    let traced = traceability
        .iter()
        .filter(|row| row.user_story.is_some() && row.acceptance_criterion.is_some())
        .count();
    let traceability_coverage = if traceability.is_empty() {
        0.0
    } else {
        traced as f32 / traceability.len() as f32
    };
    HandoffValidation {
        ok: errors.is_empty(),
        errors,
        warnings,
        citation_coverage,
        traceability_coverage,
    }
}

pub fn generate_handoff_pack(
    documents: &[CorpusDocument],
    options: &HandoffOptions,
) -> HandoffPack {
    let chunks = build_corpus(documents, options.max_chunk_chars);
    let citations: Vec<Citation> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            citation_from_chunk(
                chunk,
                index,
                markdown_for_source(documents, &chunk.source_rel),
            )
        })
        .collect();
    let (items, traceability) = extract_handoff_items(&chunks, &citations);
    let validation = validate_handoff(&items, &citations, &traceability, options.strict_citations);
    let artifacts = render_handoff_artifacts(options, &items, &traceability, &citations);
    let created_at = now_epoch();
    let nonce = now_nonce();
    let document_digests: Vec<String> = documents
        .iter()
        .map(|document| handoff_document_digest(&document.source_rel, &document.markdown))
        .collect();
    let pack_digest = handoff_pack_digest(&options.product_slug, &options.mode, &document_digests);
    HandoffPack {
        schema_version: HANDOFF_SCHEMA_VERSION,
        id_scheme: INTELLIGENCE_ID_SCHEME.into(),
        pack_id: format!(
            "handoff-{INTELLIGENCE_ID_SCHEME}-{}-{nonce}-{pack_digest}",
            options.product_slug,
        ),
        product_name: options.product_name.clone(),
        product_slug: options.product_slug.clone(),
        locale: options.locale.clone(),
        mode: options.mode.clone(),
        created_at,
        sources: documents
            .iter()
            .map(|document| document.source_rel.clone())
            .collect(),
        citations,
        items,
        traceability,
        artifacts,
        validation,
    }
}

#[cfg(feature = "llm")]
pub fn enhance_handoff_artifact(
    cfg: &crate::llm::LlmConfig,
    artifact_name: &str,
    deterministic_markdown: &str,
    citations: &[Citation],
) -> Result<String, ConvertError> {
    let citation_text = citations
        .iter()
        .take(40)
        .map(|citation| {
            format!(
                "[{}] {}",
                citation.id,
                citation.quote.chars().take(600).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Hoàn thiện tài liệu {artifact_name} dưới đây cho BA/PM bằng tiếng Việt.\n\
         Quy tắc: không thêm dữ kiện; giữ nguyên mọi ID và [CITE-*]; mục không đủ \
         thông tin phải ghi CẦN LÀM RÕ; chỉ trả Markdown.\n\n\
         TRÍCH DẪN:\n{citation_text}\n\nBẢN TẤT ĐỊNH:\n{deterministic_markdown}"
    );
    crate::llm::chat(
        cfg,
        "Bạn là BA lead tạo BRD/PRD trung thực, truy vết được và không bịa.",
        &prompt,
    )
}

pub fn export_handoff_zip(pack: &HandoffPack, output: &Path) -> Result<(), ConvertError> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| ConvertError::Failed(error.to_string()))?;
    let name = output
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        now_nonce()
    ));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| ConvertError::Failed(error.to_string()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in &pack.artifacts {
        zip.start_file(name, options)
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
        zip.write_all(content.as_bytes())
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
    }
    for (name, value) in [
        (
            "manifest.json",
            serde_json::to_vec_pretty(pack)
                .map_err(|error| ConvertError::Failed(error.to_string()))?,
        ),
        (
            "validation.json",
            serde_json::to_vec_pretty(&pack.validation)
                .map_err(|error| ConvertError::Failed(error.to_string()))?,
        ),
    ] {
        zip.start_file(name, options)
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
        zip.write_all(&value)
            .map_err(|error| ConvertError::Failed(error.to_string()))?;
    }
    zip.finish()
        .map_err(|error| ConvertError::Failed(error.to_string()))?;
    if output.exists() {
        std::fs::remove_file(output).map_err(|error| ConvertError::Failed(error.to_string()))?;
    }
    if let Err(error) = std::fs::rename(&temp, output) {
        let _ = std::fs::remove_file(&temp);
        return Err(ConvertError::Failed(error.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::normalize_newlines;

    fn sample_document() -> CorpusDocument {
        CorpusDocument {
            source_rel: "requirements.md".into(),
            md_rel: "requirements.md".into(),
            format: "markdown".into(),
            markdown: "# Yêu cầu\n\nHệ thống phải lưu nhật ký trong 5 năm.\n\
                Người dùng không được xem dữ liệu ngoài phạm vi.\n\n\
                ## Câu hỏi\n\nTBD: SLA cần làm rõ?\n\n\
                | Trường | Giá trị |\n|---|---|\n| Số lượng | 10 |\n"
                .into(),
        }
    }

    #[test]
    fn search_is_accent_insensitive_and_cited() {
        let docs = [sample_document()];
        let hits = search_corpus(&docs, "luu nhat ky", 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("lưu nhật ký"));
        let answer = ask_corpus(&docs, "nhật ký", 3);
        assert!(!answer.citations.is_empty());
        assert!(answer.answer.contains("CITE-"));
    }

    #[test]
    fn deterministic_handoff_has_traceability_and_no_uncited_items() {
        let pack = generate_handoff_pack(
            &[sample_document()],
            &HandoffOptions {
                product_name: "Nhật ký".into(),
                product_slug: "nhat-ky".into(),
                ..Default::default()
            },
        );
        assert!(pack.validation.ok);
        assert!(pack.validation.citation_coverage >= 0.99);
        assert!(pack.artifacts.contains_key("01-BRD.md"));
        assert!(pack.artifacts.contains_key("02-PRD.md"));
        assert!(!pack.traceability.is_empty());
    }

    #[test]
    fn empty_input_never_invents_requirements() {
        let pack = generate_handoff_pack(&[], &HandoffOptions::default());
        assert!(pack.items.is_empty());
        assert!(!pack.validation.ok);
        assert!(pack.artifacts["01-BRD.md"].contains("Chưa trích"));
    }

    #[test]
    fn pii_detection_and_redaction_are_non_destructive() {
        let doc = CorpusDocument {
            source_rel: "contact.md".into(),
            md_rel: "contact.md".into(),
            format: "markdown".into(),
            markdown: "Email: lan@example.com và lan@example.com\nĐiện thoại: 0912345678".into(),
        };
        let report = detect_pii(std::slice::from_ref(&doc));
        assert_eq!(report.findings.len(), 3);
        let redacted = redact_pii(&doc.markdown, &report.findings);
        assert!(!redacted.contains("lan@example.com"));
        assert!(doc.markdown.contains("lan@example.com"));
    }

    #[test]
    fn table_round_trip_preserves_surrounding_text() {
        let doc = sample_document();
        let table = parse_markdown_tables(&doc)[0].clone();
        let updated = update_markdown_table(
            &doc.markdown,
            &table,
            &[
                vec!["Trường".into(), "Giá trị".into()],
                vec!["Số lượng".into(), "20".into()],
            ],
        )
        .unwrap();
        assert!(updated.contains("|Số lượng|20|"));
        assert!(updated.contains("Hệ thống phải"));
    }

    #[test]
    fn escaped_table_cells_and_csv_formulas_are_safe() {
        let doc = CorpusDocument {
            source_rel: "table.md".into(),
            md_rel: "table.md".into(),
            format: "markdown".into(),
            markdown: "| Tên | Công thức |\n|---|---|\n| A\\|B | =1+1 |\n".into(),
        };
        let table = parse_markdown_tables(&doc)[0].clone();
        assert_eq!(table.rows[1][0], "A|B");
        let csv = String::from_utf8(table_to_csv(&table.rows).unwrap()).unwrap();
        assert!(csv.contains("'=1+1"));
    }

    #[test]
    fn corpus_offsets_do_not_panic_on_crlf_vietnamese() {
        let doc = CorpusDocument {
            source_rel: "crlf.md".into(),
            md_rel: "crlf.md".into(),
            format: "markdown".into(),
            markdown: "# Tiếng Việt\r\n\r\nHệ thống phải giữ dấu tiếng Việt.\r\n".into(),
        };
        let chunks = build_corpus(&[doc], 2_000);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].end >= chunks[0].start);
    }

    #[test]
    fn corpus_multiline_crlf_spans_match_exact_quoted_content() {
        let markdown =
            "# Tiếng Việt\r\n\r\nHệ thống phải giữ dấu.\r\nDòng hai vẫn khớp.\r\n".to_string();
        let doc = CorpusDocument {
            source_rel: "crlf-multi.md".into(),
            md_rel: "crlf-multi.md".into(),
            format: "markdown".into(),
            markdown: markdown.clone(),
        };
        let chunks = build_corpus(&[doc], 2_000);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert!(markdown.is_char_boundary(chunk.start));
        assert!(markdown.is_char_boundary(chunk.end));
        // Body canonical LF (indexing/identity parity with server).
        assert_eq!(
            chunk.text.as_str(),
            "Hệ thống phải giữ dấu.\nDòng hai vẫn khớp."
        );
        // Source span quote exact (CRLF preserved).
        assert_eq!(
            &markdown[chunk.start..chunk.end],
            "Hệ thống phải giữ dấu.\r\nDòng hai vẫn khớp."
        );
        assert_eq!(
            normalize_newlines(&markdown[chunk.start..chunk.end]).as_ref(),
            chunk.text.as_str()
        );
        let cite = citation_from_chunk(chunk, 0, &markdown);
        assert_eq!(cite.quote, markdown[chunk.start..chunk.end]);
        assert_eq!(
            page_before(&markdown, chunk.start),
            None,
            "no page marker before body"
        );
    }

    #[test]
    fn corpus_standalone_cr_before_crlf_keeps_nonempty_exact_span() {
        let markdown = "a\r\r\nb".to_string();
        let doc = CorpusDocument {
            source_rel: "cr-crlf.md".into(),
            md_rel: "cr-crlf.md".into(),
            format: "markdown".into(),
            markdown: markdown.clone(),
        };
        let chunks = build_corpus(&[doc], 2_000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "a\r\nb");
        assert!(chunks[0].end > chunks[0].start);
        assert_eq!(&markdown[chunks[0].start..chunks[0].end], "a\r\r\nb");
        assert!(markdown.as_bytes().windows(3).any(|w| w == b"\r\r\n"));
        assert_eq!(
            normalize_newlines(&markdown[chunks[0].start..chunks[0].end]).as_ref(),
            chunks[0].text.as_str()
        );
    }

    #[test]
    fn corpus_keeps_duplicate_mixed_newline_chunks_with_earliest_anchors() {
        let markdown =
            "# A\r\n\r\nLine one.\r\nLine two.\r\n\r\n# B\n\nLine one.\nLine two.\n".to_string();
        let doc = CorpusDocument {
            source_rel: "dup.md".into(),
            md_rel: "dup.md".into(),
            format: "markdown".into(),
            markdown: markdown.clone(),
        };
        let chunks = build_corpus(&[doc], 2_000);
        assert_eq!(chunks.len(), 2, "duplicate bodies must not be dropped");
        assert_eq!(chunks[0].text, "Line one.\nLine two.");
        assert_eq!(chunks[1].text, "Line one.\nLine two.");
        assert_eq!(
            &markdown[chunks[0].start..chunks[0].end],
            "Line one.\r\nLine two."
        );
        assert_eq!(
            &markdown[chunks[1].start..chunks[1].end],
            "Line one.\nLine two."
        );
        assert!(chunks[1].start > chunks[0].end);
    }

    #[test]
    fn corpus_crlf_page_marker_anchors_correct_span() {
        let markdown =
            "<!-- Page 3 -->\r\n\r\n# Mục\r\n\r\nNội dung trang ba.\r\nDòng kế.\r\n".to_string();
        let doc = CorpusDocument {
            source_rel: "page.md".into(),
            md_rel: "page.md".into(),
            format: "markdown".into(),
            markdown: markdown.clone(),
        };
        let chunks = build_corpus(&[doc], 2_000);
        let body = chunks
            .iter()
            .find(|c| c.text.contains("Nội dung trang ba"))
            .expect("body chunk");
        assert_eq!(body.text, "Nội dung trang ba.\nDòng kế.");
        assert_eq!(
            &markdown[body.start..body.end],
            "Nội dung trang ba.\r\nDòng kế."
        );
        assert_eq!(body.page, Some(3));
        assert_eq!(page_before(&markdown, body.start), Some(3));
    }

    #[test]
    fn page_before_tolerates_non_char_boundary_offset() {
        let markdown = "<!-- Page 2 -->\nệ chữ Việt";
        // Byte 1 of "ệ" (U+ệ is 3 bytes after the marker prefix).
        let ye_start = markdown.find('ệ').expect("glyph");
        assert!(!markdown.is_char_boundary(ye_start + 1));
        assert_eq!(page_before(markdown, ye_start + 1), Some(2));
        assert_eq!(page_before(markdown, usize::MAX), Some(2));
    }

    #[test]
    fn redact_pii_ignores_non_boundary_and_malformed_spans() {
        let markdown = "Liên hệ: a@b.co và ệ";
        let email_start = markdown.find("a@b.co").unwrap();
        let email_end = email_start + "a@b.co".len();
        let ye = markdown.find('ệ').unwrap();
        assert!(!markdown.is_char_boundary(ye + 1));
        let findings = [
            PiiFinding {
                kind: PiiKind::Email,
                text: "a@b.co".into(),
                source_rel: "a.md".into(),
                start: email_start,
                end: email_end,
                confidence: 1.0,
            },
            // Non-boundary span — must be ignored (no panic).
            PiiFinding {
                kind: PiiKind::Phone,
                text: "x".into(),
                source_rel: "a.md".into(),
                start: ye + 1,
                end: ye + 2,
                confidence: 1.0,
            },
            // Malformed start >= end — ignored.
            PiiFinding {
                kind: PiiKind::Phone,
                text: "x".into(),
                source_rel: "a.md".into(),
                start: 5,
                end: 5,
                confidence: 1.0,
            },
        ];
        let redacted = redact_pii(markdown, &findings);
        assert!(redacted.contains("[REDACTED_Email]"));
        assert!(redacted.contains('ệ'));
        assert!(!redacted.contains("a@b.co"));
    }

    #[test]
    fn redact_pii_coalesces_crossing_nested_duplicate_and_reversed_spans() {
        //                    012345678901234567890123
        let markdown = "secret=ABCDEFGHtail and more";
        let outer = (7, 15); // ABCDEFGH
        let nested = (9, 12); // CDE
        let crossing = (12, 19); // FGHtail — crosses outer; suffix "tail" must not leak
        let duplicate = outer;
        let findings_reversed = [
            PiiFinding {
                kind: PiiKind::Phone,
                text: "FGHtail".into(),
                source_rel: "a.md".into(),
                start: crossing.0,
                end: crossing.1,
                confidence: 1.0,
            },
            PiiFinding {
                kind: PiiKind::Email,
                text: "CDE".into(),
                source_rel: "a.md".into(),
                start: nested.0,
                end: nested.1,
                confidence: 1.0,
            },
            PiiFinding {
                kind: PiiKind::BankAccount,
                text: "ABCDEFGH".into(),
                source_rel: "a.md".into(),
                start: duplicate.0,
                end: duplicate.1,
                confidence: 1.0,
            },
            PiiFinding {
                kind: PiiKind::NationalId,
                text: "ABCDEFGH".into(),
                source_rel: "a.md".into(),
                start: outer.0,
                end: outer.1,
                confidence: 1.0,
            },
        ];
        let redacted = redact_pii(markdown, &findings_reversed);
        assert!(!redacted.contains("ABCDEFGH"));
        assert!(
            !redacted.contains("tail"),
            "crossing suffix must be redacted: {redacted}"
        );
        assert!(redacted.contains("secret="));
        assert!(redacted.contains(" and more"));
        assert!(redacted.contains("[REDACTED_"));
        // Single coalesced hole — not multiple adjacent redaction tokens for the cluster.
        assert_eq!(redacted.matches("[REDACTED_").count(), 1);
    }

    #[test]
    fn diff_and_merge_cover_clean_and_conflict_cases() {
        assert_eq!(diff_markdown("a\nb", "a\nc")[0].kind, DiffKind::Modified);
        let clean = three_way_merge("base", "ours", "base");
        assert_eq!(clean.markdown, "ours");
        assert!(clean.conflicts.is_empty());
        let conflict = three_way_merge("base", "ours", "theirs");
        assert_eq!(conflict.conflicts.len(), 1);
        assert!(conflict.markdown.contains("<<<<<<<"));
    }

    #[test]
    fn watch_glob_matches_cross_platform_names() {
        assert!(watch_pattern_matches("*.pdf", "BaoCao.PDF"));
        assert!(watch_pattern_matches("hop-??.docx", "hop-01.docx"));
        assert!(!watch_pattern_matches("*.pdf", "note.md"));
    }

    #[test]
    fn quality_flags_replacement_characters() {
        let mut doc = sample_document();
        doc.markdown.push('\u{FFFD}');
        let report = quality_report(&[doc]);
        assert!(report.documents[0]
            .issues
            .iter()
            .any(|issue| issue.code == "REPLACEMENT_CHARACTER"));
    }
}

#[cfg(test)]
mod intelligence_id_tests {
    use super::{
        corpus_chunk_id, handoff_document_digest, handoff_pack_digest, intelligence_digest,
        markdown_table_id, HandoffMode, DOMAIN_CHUNK, DOMAIN_HANDOFF_DOCUMENT, DOMAIN_TABLE,
        HANDOFF_SCHEMA_VERSION, INTELLIGENCE_ID_SCHEME,
    };

    #[test]
    fn sha256_v1_domain_vectors_are_independent_and_pinned() {
        assert_eq!(INTELLIGENCE_ID_SCHEME, "sha256-v1");
        assert_eq!(HANDOFF_SCHEMA_VERSION, 2);
        assert_eq!(
            intelligence_digest(DOMAIN_CHUNK, &[]),
            "3d209f021406f03ded91cc145d3505de3d811d8c84407450fd0103e9c8ac762e"
        );
        assert_eq!(
            intelligence_digest(DOMAIN_CHUNK, &[b"alpha"]),
            "035a5386d593c4334c79fe2c76dc8d0855bbb4406c02a06bba42d69b06f032dd"
        );
        assert_eq!(
            intelligence_digest(DOMAIN_CHUNK, &[b"a", b"b", b"c"]),
            "ab08d8cb024ca089b89c89d26a2bad0c4f8b9c1a56177526dabffbb0b9b5564e"
        );
        let zero = 0u64.to_be_bytes();
        assert_eq!(
            intelligence_digest(DOMAIN_CHUNK, &["yêu cầu".as_bytes(), &zero]),
            "1c7d0e2597701f6b5f510444c92557f01d5161482986d582bf47da32bc50cc9d"
        );
        assert_eq!(
            intelligence_digest(DOMAIN_TABLE, &[b"sheet.md", &zero, &12u64.to_be_bytes()]),
            "890c11209f0ade11d5307d3344bbbb8f37b10775ab4d806b5b9c24c8a36bdb7a"
        );
        assert_eq!(
            intelligence_digest(
                DOMAIN_HANDOFF_DOCUMENT,
                &[b"doc.md", b"# Title\n\nBody text.\n"]
            ),
            "d847104aef03239f6e19f30d970ee9cf47817f5de8c85d6d66cf020456f6cfb9"
        );
    }

    #[test]
    fn sha256_v1_length_prefix_and_domain_separate_collisions() {
        assert_ne!(
            intelligence_digest(DOMAIN_CHUNK, &[b"ab", b"c"]),
            intelligence_digest(DOMAIN_CHUNK, &[b"a", b"bc"])
        );
        assert_ne!(
            intelligence_digest(DOMAIN_CHUNK, &[b"alpha"]),
            intelligence_digest(DOMAIN_TABLE, &[b"alpha"])
        );
        assert_eq!(
            intelligence_digest(DOMAIN_CHUNK, &[b"x", b"y"]),
            intelligence_digest(DOMAIN_CHUNK, &[b"x", b"y"])
        );
        assert_ne!(
            intelligence_digest(DOMAIN_CHUNK, &[b"x", b"y"]),
            intelligence_digest(DOMAIN_CHUNK, &[b"y", b"x"])
        );
    }

    #[test]
    fn visible_ids_encode_scheme_and_are_stable() {
        assert_eq!(
            corpus_chunk_id("doc.md", "Heading", 0),
            "chunk-sha256-v1-f243a448d7403a66a04ddb2f8505673c3b938e710b374df23ed189b70c85614f"
        );
        assert_eq!(
            markdown_table_id("sheet.md", 0, 12),
            "table-sha256-v1-890c11209f0ade11d5307d3344bbbb8f37b10775ab4d806b5b9c24c8a36bdb7a"
        );
        let doc = handoff_document_digest("doc.md", "# Title\n\nBody text.\n");
        assert_eq!(
            doc,
            "d847104aef03239f6e19f30d970ee9cf47817f5de8c85d6d66cf020456f6cfb9"
        );
        assert_eq!(
            handoff_pack_digest("probe", &HandoffMode::Deterministic, &[doc]),
            "0e15d4a58dc945c6f96037d3fbafedb0a7df4d0f0cf49ec62f820be0bd6768a2"
        );
    }
}
