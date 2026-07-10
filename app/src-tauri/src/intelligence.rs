use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fileconv_core::intelligence::{
    self, AskResult, CorpusDocument, DiffHunk, DocumentSchema, HandoffMode, HandoffOptions,
    HandoffPack, MarkdownTable, MergeResult, PiiReport, QualityReport, SearchHit, VersionSnapshot,
    WatchMatch, WatchRule,
};
use fileconv_core::FormatKind;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::{atomic_write, data_root, es, rel_of, resolve_within, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRequest {
    pub source_rels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub source_rels: Vec<String>,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRequest {
    pub source_rels: Vec<String>,
    pub question: String,
    pub top_k: Option<usize>,
    pub use_llm: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffRequest {
    pub source_rels: Vec<String>,
    pub product_name: String,
    pub product_slug: String,
    pub mode: HandoffMode,
    pub out_rel_dir: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffResult {
    pub pack: HandoffPack,
    pub out_rel_dir: String,
    pub llm_note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRequest {
    pub rel_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactRequest {
    pub source_rel: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactResult {
    pub report: PiiReport,
    pub redacted_rel_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRequest {
    pub source_rel: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableUpdateRequest {
    pub source_rel: String,
    pub table_id: String,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableUpdateResult {
    pub md_rel_path: String,
    pub markdown: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRequest {
    pub source_rel: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMeta {
    pub id: String,
    pub created_at: u64,
    pub bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionReadRequest {
    pub source_rel: String,
    pub version_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDiffRequest {
    pub source_rel: String,
    pub old_version_id: String,
    pub new_version_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchRulesRequest {
    pub rules: Vec<WatchRule>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPackRequest {
    pub source_rels: Vec<String>,
    pub product_name: String,
    pub product_slug: String,
    pub output_abs: String,
}

fn stable_key(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn markdown_path(root: &Path, source_rel: &str) -> Result<PathBuf, String> {
    let source = resolve_within(root, source_rel)?;
    let is_markdown = source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    let markdown = if is_markdown {
        source
    } else {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or("file nguồn không hợp lệ")?;
        source.with_file_name(format!("{name}.md"))
    };
    if !markdown.is_file() {
        return Err(format!(
            "chưa có Markdown cho '{}'; hãy convert trước",
            source_rel
        ));
    }
    Ok(markdown)
}

fn load_document(root: &Path, source_rel: &str) -> Result<CorpusDocument, String> {
    let source_path = resolve_within(root, source_rel)?;
    let md_path = markdown_path(root, source_rel)?;
    let markdown = fs::read_to_string(&md_path).map_err(es)?;
    Ok(CorpusDocument {
        source_rel: source_rel.to_string(),
        md_rel: rel_of(root, &md_path),
        format: FormatKind::from_path(&source_path).as_str().to_string(),
        markdown,
    })
}

fn load_documents(root: &Path, source_rels: &[String]) -> Result<Vec<CorpusDocument>, String> {
    if source_rels.is_empty() {
        return Err("hãy chọn ít nhất một tài liệu đã convert".into());
    }
    source_rels
        .iter()
        .map(|source_rel| load_document(root, source_rel))
        .collect()
}

fn markhand_root(root: &Path) -> PathBuf {
    root.join(".markhand")
}

fn handoff_dir(root: &Path, pack_id: &str, requested: Option<&str>) -> Result<PathBuf, String> {
    let relative = requested
        .map(str::to_string)
        .unwrap_or_else(|| format!(".markhand/handoff/{pack_id}"));
    resolve_within(root, &relative)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(es)?;
    atomic_write(path, &bytes)
}

fn persist_pack(directory: &Path, pack: &HandoffPack) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(es)?;
    for (name, content) in &pack.artifacts {
        atomic_write(&directory.join(name), content.as_bytes())?;
    }
    write_json(&directory.join("manifest.json"), pack)?;
    write_json(&directory.join("validation.json"), &pack.validation)
}

#[tauri::command]
pub async fn generate_handoff_pack(
    state: State<'_, AppState>,
    req: HandoffRequest,
) -> Result<HandoffResult, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        let options = HandoffOptions {
            product_name: req.product_name,
            product_slug: req.product_slug,
            locale: "vi-VN".into(),
            mode: req.mode.clone(),
            max_chunk_chars: 2_000,
            strict_citations: true,
        };
        let mut pack = intelligence::generate_handoff_pack(&documents, &options);
        let mut llm_note = None;

        if req.mode == HandoffMode::LlmAssisted {
            if let Some(config) = fileconv_core::llm::LlmConfig::from_env() {
                for name in ["01-BRD.md", "02-PRD.md"] {
                    if let Some(deterministic) = pack.artifacts.get(name).cloned() {
                        match intelligence::enhance_handoff_artifact(
                            &config,
                            name,
                            &deterministic,
                            &pack.citations,
                        ) {
                            Ok(enhanced) => {
                                pack.artifacts
                                    .insert(name.replace(".md", "-LLM.md"), enhanced);
                            }
                            Err(error) => {
                                llm_note = Some(format!("LLM lỗi; giữ bản tất định: {}", error));
                            }
                        }
                    }
                }
            } else {
                llm_note =
                    Some("Chưa cấu hình FILECONV_LLM_*; đã sinh bản tất định offline.".into());
            }
        }

        let directory = handoff_dir(&root, &pack.pack_id, req.out_rel_dir.as_deref())?;
        persist_pack(&directory, &pack)?;
        Ok(HandoffResult {
            out_rel_dir: rel_of(&root, &directory),
            pack,
            llm_note,
        })
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub fn read_handoff_artifact(
    state: State<AppState>,
    req: ArtifactRequest,
) -> Result<String, String> {
    let path = resolve_within(&data_root(&state), &req.rel_path)?;
    if !path.is_file() {
        return Err("artifact không tồn tại".into());
    }
    fs::read_to_string(path).map_err(es)
}

#[tauri::command]
pub async fn run_quality_report(
    state: State<'_, AppState>,
    req: ScopeRequest,
) -> Result<QualityReport, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        Ok(intelligence::quality_report(&documents))
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn search_intelligence(
    state: State<'_, AppState>,
    req: SearchRequest,
) -> Result<Vec<SearchHit>, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        Ok(intelligence::search_corpus(
            &documents,
            &req.query,
            req.limit.unwrap_or(20),
        ))
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn ask_intelligence(
    state: State<'_, AppState>,
    req: AskRequest,
) -> Result<AskResult, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        let mut result =
            intelligence::ask_corpus(&documents, &req.question, req.top_k.unwrap_or(6));
        if req.use_llm.unwrap_or(false) && !result.citations.is_empty() {
            if let Some(config) = fileconv_core::llm::LlmConfig::from_env() {
                let sources = result
                    .citations
                    .iter()
                    .map(|citation| format!("[{}] {}", citation.id, citation.quote))
                    .collect::<Vec<_>>()
                    .join("\n");
                let prompt = format!(
                    "Câu hỏi: {}\n\nNguồn:\n{}\n\nChỉ trả lời từ nguồn, trích [CITE-*].",
                    req.question, sources
                );
                if let Ok(answer) = fileconv_core::llm::chat(
                    &config,
                    "Bạn trả lời tài liệu trung thực, không bịa và luôn trích dẫn.",
                    &prompt,
                ) {
                    result.answer = answer;
                }
            }
        }
        Ok(result)
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn scan_pii(state: State<'_, AppState>, req: ScopeRequest) -> Result<PiiReport, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        Ok(intelligence::detect_pii(&documents))
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn redact_pii(
    state: State<'_, AppState>,
    req: RedactRequest,
) -> Result<RedactResult, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let document = load_document(&root, &req.source_rel)?;
        let report = intelligence::detect_pii(std::slice::from_ref(&document));
        let redacted = intelligence::redact_pii(&document.markdown, &report.findings);
        let out_dir = markhand_root(&root).join("redacted");
        fs::create_dir_all(&out_dir).map_err(es)?;
        let out = out_dir.join(format!("{}.md", stable_key(&req.source_rel)));
        atomic_write(&out, redacted.as_bytes())?;
        Ok(RedactResult {
            report,
            redacted_rel_path: rel_of(&root, &out),
        })
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn extract_document_schema(
    state: State<'_, AppState>,
    req: ScopeRequest,
) -> Result<Vec<DocumentSchema>, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        Ok(documents.iter().map(intelligence::extract_schema).collect())
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn list_markdown_tables(
    state: State<'_, AppState>,
    req: TableRequest,
) -> Result<Vec<MarkdownTable>, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let document = load_document(&root, &req.source_rel)?;
        Ok(intelligence::parse_markdown_tables(&document))
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn update_markdown_table(
    state: State<'_, AppState>,
    req: TableUpdateRequest,
) -> Result<TableUpdateResult, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let document = load_document(&root, &req.source_rel)?;
        let table = intelligence::parse_markdown_tables(&document)
            .into_iter()
            .find(|table| table.id == req.table_id)
            .ok_or("không tìm thấy bảng")?;
        let markdown = intelligence::update_markdown_table(&document.markdown, &table, &req.rows)
            .map_err(es)?;
        let md_path = resolve_within(&root, &document.md_rel)?;
        atomic_write(&md_path, markdown.as_bytes())?;
        Ok(TableUpdateResult {
            md_rel_path: document.md_rel,
            markdown,
        })
    })
    .await
    .map_err(es)?
}

fn versions_dir(root: &Path, source_rel: &str) -> PathBuf {
    markhand_root(root)
        .join("versions")
        .join(stable_key(source_rel))
}

fn valid_version_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[tauri::command]
pub async fn snapshot_document_version(
    state: State<'_, AppState>,
    req: VersionRequest,
) -> Result<VersionMeta, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let document = load_document(&root, &req.source_rel)?;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let id = format!("v-{created_at}");
        let directory = versions_dir(&root, &req.source_rel);
        fs::create_dir_all(&directory).map_err(es)?;
        let path = directory.join(format!("{id}.md"));
        atomic_write(&path, document.markdown.as_bytes())?;
        Ok(VersionMeta {
            id,
            created_at,
            bytes: document.markdown.len() as u64,
        })
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub fn list_document_versions(
    state: State<AppState>,
    req: VersionRequest,
) -> Result<Vec<VersionMeta>, String> {
    let root = data_root(&state);
    let directory = versions_dir(&root, &req.source_rel);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut versions = Vec::new();
    for entry in fs::read_dir(directory).map_err(es)? {
        let entry = entry.map_err(es)?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let id = path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let created_at = id
            .strip_prefix("v-")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        versions.push(VersionMeta {
            id,
            created_at,
            bytes: fs::metadata(path).map_err(es)?.len(),
        });
    }
    versions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(versions)
}

fn read_version(root: &Path, source_rel: &str, version_id: &str) -> Result<String, String> {
    if !valid_version_id(version_id) {
        return Err("version id không hợp lệ".into());
    }
    let path = versions_dir(root, source_rel).join(format!("{version_id}.md"));
    fs::read_to_string(path).map_err(es)
}

#[tauri::command]
pub fn read_document_version(
    state: State<AppState>,
    req: VersionReadRequest,
) -> Result<VersionSnapshot, String> {
    let root = data_root(&state);
    let markdown = read_version(&root, &req.source_rel, &req.version_id)?;
    let created_at = req
        .version_id
        .strip_prefix("v-")
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    Ok(VersionSnapshot {
        id: req.version_id,
        source_rel: req.source_rel,
        created_at,
        markdown,
    })
}

#[tauri::command]
pub fn diff_document_versions(
    state: State<AppState>,
    req: VersionDiffRequest,
) -> Result<Vec<DiffHunk>, String> {
    let root = data_root(&state);
    let old = read_version(&root, &req.source_rel, &req.old_version_id)?;
    let new = read_version(&root, &req.source_rel, &req.new_version_id)?;
    Ok(intelligence::diff_markdown(&old, &new))
}

#[tauri::command]
pub fn merge_document_versions(req: MergeRequest) -> MergeResult {
    intelligence::three_way_merge(&req.base, &req.ours, &req.theirs)
}

fn watch_rules_path(root: &Path) -> PathBuf {
    markhand_root(root).join("watch-rules.json")
}

#[tauri::command]
pub fn get_watch_rules(state: State<AppState>) -> Result<Vec<WatchRule>, String> {
    let path = watch_rules_path(&data_root(&state));
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&fs::read(path).map_err(es)?).map_err(es)
}

#[tauri::command]
pub fn set_watch_rules(state: State<AppState>, req: WatchRulesRequest) -> Result<(), String> {
    for rule in &req.rules {
        let watch = fs::canonicalize(&rule.watch_abs).map_err(es)?;
        if !watch.is_dir() {
            return Err(format!("watch path không phải thư mục: {}", rule.watch_abs));
        }
        resolve_within(&data_root(&state), &rule.target_folder_rel)?;
    }
    let path = watch_rules_path(&data_root(&state));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(es)?;
    }
    write_json(&path, &req.rules)
}

#[tauri::command]
pub async fn scan_watch_rules(state: State<'_, AppState>) -> Result<Vec<WatchMatch>, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let path = watch_rules_path(&root);
        let rules: Vec<WatchRule> = if path.exists() {
            serde_json::from_slice(&fs::read(path).map_err(es)?).map_err(es)?
        } else {
            Vec::new()
        };
        let mut matches = Vec::new();
        for rule in rules.into_iter().filter(|rule| rule.enabled) {
            let watch = fs::canonicalize(&rule.watch_abs).map_err(es)?;
            for entry in fs::read_dir(watch).map_err(es)? {
                let entry = entry.map_err(es)?;
                if !entry.file_type().map_err(es)?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if intelligence::watch_pattern_matches(&rule.pattern, &name) {
                    matches.push(WatchMatch {
                        rule_id: rule.id.clone(),
                        source_abs: entry.path().to_string_lossy().to_string(),
                        target_folder_rel: rule.target_folder_rel.clone(),
                        action: rule.action.clone(),
                    });
                }
            }
        }
        Ok(matches)
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub async fn export_knowledge_pack(
    state: State<'_, AppState>,
    req: ExportPackRequest,
) -> Result<String, String> {
    let root = data_root(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let documents = load_documents(&root, &req.source_rels)?;
        let options = HandoffOptions {
            product_name: req.product_name,
            product_slug: req.product_slug,
            ..Default::default()
        };
        let mut pack = intelligence::generate_handoff_pack(&documents, &options);
        for document in &documents {
            pack.artifacts.insert(
                format!("sources/{}.md", stable_key(&document.source_rel)),
                document.markdown.clone(),
            );
        }
        let output = PathBuf::from(&req.output_abs);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(es)?;
        }
        intelligence::export_handoff_zip(&pack, &output).map_err(es)?;
        Ok(output.to_string_lossy().to_string())
    })
    .await
    .map_err(es)?
}
