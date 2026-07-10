//! Local-first document intelligence for Markhand.
//!
//! This module works on canonical Markdown sidecars. It intentionally keeps
//! [`crate::Converter::convert_path`] unchanged and provides deterministic
//! baselines for handoff packs, cited search, quality, PII, tables, schema,
//! versions and automation. Optional LLM enhancement remains behind `llm`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::chunk::chunk_markdown;
use crate::ConvertError;

const DEFAULT_CHUNK_CHARS: usize = 2_000;

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

fn stable_hash(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        part.as_ref().hash(&mut hasher);
        0xff_u8.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
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

fn tokens(text: &str) -> Vec<String> {
    accent_fold(text)
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

fn page_before(markdown: &str, offset: usize) -> Option<u32> {
    let prefix = &markdown[..offset.min(markdown.len())];
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
            cursor = cursor.min(document.markdown.len());
            while cursor < document.markdown.len() && !document.markdown.is_char_boundary(cursor) {
                cursor += 1;
            }
            let start = document.markdown[cursor..]
                .find(&chunk.text)
                .map(|relative| cursor + relative)
                .unwrap_or(cursor);
            let mut end = (start + chunk.text.len()).min(document.markdown.len());
            while end > start && !document.markdown.is_char_boundary(end) {
                end -= 1;
            }
            cursor = end;
            corpus.push(CorpusChunk {
                id: format!(
                    "chunk-{}",
                    stable_hash([
                        document.source_rel.as_str(),
                        chunk.heading.as_str(),
                        &start.to_string(),
                    ])
                ),
                source_rel: document.source_rel.clone(),
                md_rel: document.md_rel.clone(),
                heading: chunk.heading,
                text: chunk.text,
                start,
                end,
                page: page_before(&document.markdown, start),
            });
        }
    }
    corpus
}

fn citation_from_chunk(chunk: &CorpusChunk, index: usize) -> Citation {
    Citation {
        id: format!("CITE-{:04}", index + 1),
        source_rel: chunk.source_rel.clone(),
        md_rel: chunk.md_rel.clone(),
        heading: chunk.heading.clone(),
        quote: chunk.text.clone(),
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
        .map(|(index, hit)| citation_from_chunk(&hit.chunk, index))
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

pub fn detect_pii(documents: &[CorpusDocument]) -> PiiReport {
    let mut findings = Vec::new();
    for document in documents {
        let mut offset = 0usize;
        for line in document.markdown.split_inclusive('\n') {
            let lower = accent_fold(line);
            let mut search_from = 0usize;
            for token in line.split_whitespace() {
                let token_start = line[search_from..]
                    .find(token)
                    .map(|relative| search_from + relative)
                    .unwrap_or(search_from);
                search_from = (token_start + token.len()).min(line.len());
                let clean = token.trim_matches(|ch: char| {
                    matches!(ch, ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']')
                });
                let digits: String = clean.chars().filter(|ch| ch.is_ascii_digit()).collect();
                let kind = if clean.contains('@')
                    && clean.contains('.')
                    && !clean.starts_with('@')
                    && !clean.ends_with('.')
                {
                    Some((PiiKind::Email, 0.98))
                } else if (digits.len() == 10 && digits.starts_with('0'))
                    || (digits.len() == 11 && digits.starts_with("84"))
                {
                    Some((PiiKind::Phone, 0.9))
                } else if (digits.len() == 9 || digits.len() == 12)
                    && (lower.contains("cccd")
                        || lower.contains("cmnd")
                        || lower.contains("can cuoc"))
                {
                    Some((PiiKind::NationalId, 0.95))
                } else if (8..=19).contains(&digits.len())
                    && (lower.contains("tai khoan") || lower.contains("ngan hang"))
                {
                    Some((PiiKind::BankAccount, 0.75))
                } else {
                    None
                };
                if let Some((kind, confidence)) = kind {
                    if let Some(clean_start) = token.find(clean) {
                        let start = offset + token_start + clean_start;
                        let end = start + clean.len();
                        findings.push(PiiFinding {
                            kind,
                            text: clean.to_string(),
                            source_rel: document.source_rel.clone(),
                            start,
                            end,
                            confidence,
                        });
                    }
                }
            }
            offset += line.len();
        }
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
        .filter(|finding| finding.end <= output.len() && finding.start < finding.end)
        .map(|finding| (finding.start, finding.end, &finding.kind))
        .collect();
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    for (start, end, kind) in spans {
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
            id: format!(
                "table-{}",
                stable_hash([
                    document.source_rel.as_str(),
                    &tables.len().to_string(),
                    &start.to_string(),
                ])
            ),
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
    if table.end > markdown.len() || table.start > table.end {
        return Err(ConvertError::Failed("span bảng không hợp lệ".into()));
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
        .map(|(index, chunk)| citation_from_chunk(chunk, index))
        .collect();
    let (items, traceability) = extract_handoff_items(&chunks, &citations);
    let validation = validate_handoff(&items, &citations, &traceability, options.strict_citations);
    let artifacts = render_handoff_artifacts(options, &items, &traceability, &citations);
    let created_at = now_epoch();
    let nonce = now_nonce();
    let fingerprint: Vec<String> = documents
        .iter()
        .map(|document| {
            format!(
                "{}:{}",
                document.source_rel,
                stable_hash([document.markdown.as_str()])
            )
        })
        .chain(std::iter::once(format!("{:?}", options.mode)))
        .collect();
    HandoffPack {
        schema_version: 1,
        pack_id: format!(
            "handoff-{}-{}-{}",
            options.product_slug,
            nonce,
            stable_hash(fingerprint.iter())
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
