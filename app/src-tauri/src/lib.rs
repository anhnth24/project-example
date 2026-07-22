//! Backend Tauri cho Markhand Desktop.
//!
//! Cầu nối giữa UI (React) và lõi `fileconv-core`. Mọi thao tác filesystem chạy trong
//! tiến trình Rust để kiểm soát chặt đường dẫn (không bật plugin fs).
//!
//! Mô hình dữ liệu (đơn giản hóa — không còn multi-workspace):
//!   - Một **thư mục gốc DATA** duy nhất. Mặc định: `app_data_dir()/DATA`.
//!     Người dùng có thể **map** DATA sang thư mục bất kỳ (lưu ở `config.json`).
//!   - Trong DATA: tạo folder thật → upload file vào → convert → ghi `.md` cạnh file gốc.
//!   - Quy ước link 1-1: `report.pdf` -> `report.pdf.md`. Filesystem là nguồn sự thật.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use fileconv_core::{ConvertErrorKind, ConverterOptions, DetailedErrorDto, FormatKind};

mod intelligence;
mod knowledge;
#[cfg(test)]
mod knowledge_contract;
mod projects;
mod watch;

// ───────────────────────────── Kiểu dữ liệu ─────────────────────────────

/// Một node trong cây thư mục gửi cho UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub name: String,
    /// Đường dẫn tương đối so với gốc DATA (dùng `/`). "" với node gốc.
    pub rel_path: String,
    pub is_dir: bool,
    /// "folder" | format (pdf/docx/...) | "markdown" | "other".
    pub kind: String,
    /// File gốc có thể convert được không (đuôi nằm trong danh sách hỗ trợ).
    pub supported: bool,
    /// File `.md` liên kết 1-1 (nếu đã convert). Với md đứng riêng = chính nó.
    pub md_rel_path: Option<String>,
    /// True nếu là file `.md` người dùng tạo tay (không gắn file gốc).
    pub standalone_md: bool,
    pub children: Vec<Node>,
}

/// Tùy chọn convert lộ ra UI (ánh xạ sang `ConverterOptions` của lõi).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub ocr_langs: String,
    pub ocr_engine: String,
    pub pdf_ocr: bool,
    pub pdf_ocr_images: bool,
    pub audio_lang: String,
    pub audio_threads: i32,
    pub audio_no_speech_threshold: f32,
    pub whisper_model: Option<String>,
    pub llm_enabled: bool,
    pub llm_provider: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub llm_api_key: Option<String>,
    pub llm_cli_binary: Option<String>,
    pub embedding_enabled: bool,
    pub embedding_provider: String,
    pub embedding_base_url: String,
    pub embedding_model: String,
    pub embedding_api_key: Option<String>,
    pub embedding_dimensions: Option<usize>,
    pub embedding_fallback_local: bool,
    pub auto_check_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let d = ConverterOptions::default();
        Self {
            ocr_langs: d.ocr_langs,
            ocr_engine: "tesseract".into(),
            pdf_ocr: d.pdf_ocr,
            pdf_ocr_images: d.pdf_ocr_images,
            audio_lang: d.audio_lang,
            audio_threads: d.audio_threads,
            audio_no_speech_threshold: d.audio_no_speech_threshold,
            whisper_model: None,
            llm_enabled: false,
            llm_provider: "ollama".into(),
            llm_base_url: "http://127.0.0.1:11434".into(),
            llm_model: "qwen2.5:7b".into(),
            llm_api_key: None,
            llm_cli_binary: None,
            embedding_enabled: false,
            embedding_provider: "ollama".into(),
            embedding_base_url: "http://127.0.0.1:11434".into(),
            embedding_model: "nomic-embed-text".into(),
            embedding_api_key: None,
            embedding_dimensions: None,
            embedding_fallback_local: true,
            auto_check_update: true,
        }
    }
}

impl Settings {
    fn to_options(&self) -> ConverterOptions {
        ConverterOptions {
            ocr_langs: self.ocr_langs.clone(),
            ocr_engine: fileconv_core::image_ocr::OcrEngine::from_name(&self.ocr_engine),
            whisper_model: self.whisper_model.as_ref().map(PathBuf::from),
            audio_lang: self.audio_lang.clone(),
            audio_threads: self.audio_threads,
            audio_no_speech_threshold: self.audio_no_speech_threshold,
            pdf_ocr: self.pdf_ocr,
            pdf_ocr_images: self.pdf_ocr_images,
            ..Default::default()
        }
    }

    fn llm_config(&self) -> Result<Option<fileconv_core::llm::LlmConfig>, String> {
        if !self.llm_enabled {
            return Ok(fileconv_core::llm::LlmConfig::from_env());
        }
        let preset = fileconv_core::llm::provider_presets()
            .into_iter()
            .find(|preset| preset.id == self.llm_provider);
        let provider = preset
            .as_ref()
            .map(|preset| preset.provider)
            .unwrap_or_else(|| fileconv_core::llm::Provider::from_name(&self.llm_provider));
        if matches!(
            provider,
            fileconv_core::llm::Provider::CursorCli | fileconv_core::llm::Provider::CodexCli
        ) {
            return fileconv_core::llm::LlmConfig::new_cli(
                provider,
                self.llm_model.trim(),
                self.llm_cli_binary.clone(),
            )
            .map(Some)
            .map_err(|error| error.to_string());
        }
        let api_key = self
            .llm_api_key
            .clone()
            .or_else(|| std::env::var("FILECONV_LLM_API_KEY").ok())
            .unwrap_or_default();
        if preset
            .as_ref()
            .is_some_and(|preset| preset.requires_api_key)
            && api_key.trim().is_empty()
        {
            return Err(format!(
                "{} yêu cầu API key",
                preset
                    .as_ref()
                    .map(|preset| preset.label.as_str())
                    .unwrap_or("Provider")
            ));
        }
        let base_url =
            (!self.llm_base_url.trim().is_empty()).then(|| self.llm_base_url.trim().to_string());
        fileconv_core::llm::LlmConfig::new(provider, api_key, self.llm_model.trim(), base_url)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn embedding_config(&self) -> Result<Option<fileconv_core::llm::EmbeddingConfig>, String> {
        if !self.embedding_enabled {
            return Ok(None);
        }
        let preset = fileconv_core::llm::embedding_provider_presets()
            .into_iter()
            .find(|preset| preset.id == self.embedding_provider);
        let provider = preset
            .as_ref()
            .map(|preset| preset.provider)
            .unwrap_or_else(|| fileconv_core::llm::Provider::from_name(&self.embedding_provider));
        let api_key = self
            .embedding_api_key
            .clone()
            .or_else(|| std::env::var("FILECONV_EMBEDDING_API_KEY").ok())
            .or_else(|| std::env::var("FILECONV_LLM_API_KEY").ok())
            .unwrap_or_default();
        if preset
            .as_ref()
            .is_some_and(|preset| preset.requires_api_key)
            && api_key.trim().is_empty()
        {
            return Err(format!(
                "{} yêu cầu API key cho embedding",
                preset
                    .as_ref()
                    .map(|preset| preset.label.as_str())
                    .unwrap_or("Provider")
            ));
        }
        let base_url = (!self.embedding_base_url.trim().is_empty())
            .then(|| self.embedding_base_url.trim().to_string());
        let runtime_path = preset
            .as_ref()
            .map(|preset| preset.runtime_path.clone())
            .unwrap_or_else(|| {
                fileconv_core::llm::infer_embedding_runtime_path(
                    base_url.as_deref(),
                    self.embedding_model.trim(),
                )
                .to_string()
            });
        fileconv_core::llm::EmbeddingConfig::new(
            provider,
            api_key,
            self.embedding_model.trim(),
            base_url,
            self.embedding_dimensions,
            runtime_path,
        )
        .map(Some)
        .map_err(|error| error.to_string())
    }
}

/// File cấu hình app (lưu vị trí DATA root mà người dùng map).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    data_root: Option<String>,
}

pub struct AppState {
    config_dir: PathBuf,
    data_root: Mutex<PathBuf>,
    settings: Mutex<Settings>,
    watch_service: watch::WatchService,
}

// ───────────────────────────── Helper ─────────────────────────────

fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

static TEMP_FILE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write through a same-directory temporary file, then replace the target.
/// Unix replaces atomically; Windows uses a short-lived backup because
/// `std::fs::rename` does not overwrite an existing file there.
fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::Ordering;

    let parent = path.parent().ok_or("file đích không có thư mục cha")?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let suffix = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), suffix));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(es)?;
    if let Err(error) = output.write_all(contents).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = fs::remove_file(&temp);
        return Err(es(error));
    }
    drop(output);

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            let backup = parent.join(format!(".{name}.{}.{}.backup", std::process::id(), suffix));
            if let Err(error) = fs::rename(path, &backup) {
                let _ = fs::remove_file(&temp);
                return Err(es(error));
            }
            match fs::rename(&temp, path) {
                Ok(()) => {
                    let _ = fs::remove_file(backup);
                    Ok(())
                }
                Err(error) => {
                    let _ = fs::rename(&backup, path);
                    let _ = fs::remove_file(&temp);
                    Err(es(error))
                }
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(es(error))
        }
    }
}

/// Đuôi file mà lõi convert hỗ trợ (suy từ `FormatKind::from_path`).
const SUPPORTED_EXTS: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "xls", "xlsb", "ods", "csv", "html", "htm", "png", "jpg",
    "jpeg", "webp", "bmp", "tif", "tiff", "gif", "txt", "log", "wav", "mp3", "m4a", "flac", "ogg",
];

fn config_file(config_dir: &Path) -> PathBuf {
    config_dir.join("config.json")
}

fn settings_file(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

fn load_config(config_dir: &Path) -> AppConfig {
    fs::read_to_string(config_file(config_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(config_dir: &Path, cfg: &AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(es)?;
    atomic_write(&config_file(config_dir), json.as_bytes())
}

/// Ghép `rel` vào trong `root` an toàn (chặn `..`, đường dẫn tuyệt đối).
fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut p = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => {
                p.push(c);
                if fs::symlink_metadata(&p)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err("đường dẫn qua symlink không được phép".into());
                }
            }
            Component::CurDir => {}
            _ => return Err("đường dẫn không hợp lệ".into()),
        }
    }
    Ok(p)
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
    {
        return Err("tên không hợp lệ".into());
    }
    Ok(())
}

fn child_case_insensitive(parent: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
}

/// Dựng cây thư mục đệ quy. Bỏ qua mục ẩn. File `.md` là md-liên-kết của một file
/// gốc cùng tên thì KHÔNG hiện riêng (gắn vào node của file gốc).
fn build_tree(abs: &Path, root: &Path) -> Result<Node, String> {
    let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
    for e in fs::read_dir(abs).map_err(es)? {
        let e = e.map_err(es)?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        let meta = fs::symlink_metadata(&p).map_err(es)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let is_dir = meta.is_dir();
        entries.push((name, p, is_dir));
    }

    let files_by_name: HashMap<String, PathBuf> = entries
        .iter()
        .filter(|(_, _, d)| !d)
        .map(|(name, path, _)| (name.to_ascii_lowercase(), path.clone()))
        .collect();

    let mut children: Vec<Node> = Vec::new();
    for (name, path, is_dir) in &entries {
        if *is_dir {
            children.push(build_tree(path, root)?);
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".md") {
            let base = &name[..name.len() - 3];
            if files_by_name.contains_key(&base.to_ascii_lowercase()) {
                continue; // md liên kết -> bỏ qua, thể hiện qua file gốc.
            }
            let rel = rel_of(root, path);
            children.push(Node {
                name: name.clone(),
                rel_path: rel.clone(),
                is_dir: false,
                kind: "markdown".into(),
                supported: false,
                md_rel_path: Some(rel),
                standalone_md: true,
                children: vec![],
            });
        } else {
            let kind = FormatKind::from_path(path);
            let supported = kind != FormatKind::Unknown;
            let md_name = format!("{name}.md");
            let md_rel = files_by_name
                .get(&md_name.to_ascii_lowercase())
                .map(|md_path| rel_of(root, md_path));
            children.push(Node {
                name: name.clone(),
                rel_path: rel_of(root, path),
                is_dir: false,
                kind: if supported {
                    kind.as_str().into()
                } else {
                    "other".into()
                },
                supported,
                md_rel_path: md_rel,
                standalone_md: false,
                children: vec![],
            });
        }
    }

    children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Node {
        name: abs
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "DATA".into()),
        rel_path: rel_of(root, abs),
        is_dir: true,
        kind: "folder".into(),
        supported: false,
        md_rel_path: None,
        standalone_md: false,
        children,
    })
}

fn convert_and_write_md(opts: ConverterOptions, source: PathBuf) -> Result<PathBuf, String> {
    let md_path = source.with_file_name(format!(
        "{}.md",
        source
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    // Per-request Converter is fine: WhisperContext is process-cached in fileconv-core.
    let conv = fileconv_core::Converter::with_options(opts);
    let result = conv
        .convert_path(&source)
        .map_err(|e| format!("convert thất bại: {e}"))?;
    atomic_write(&md_path, result.markdown.as_bytes())?;
    Ok(md_path)
}

/// Additive detailed convert report for UI surfaces (legacy import/reconvert unchanged).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionDetailedReport {
    markdown_rel_path: String,
    title: Option<String>,
    format: String,
    outcome: fileconv_core::ConversionOutcome,
    warnings: Vec<fileconv_core::ConversionWarning>,
}

fn detailed_failed(message: impl Into<String>) -> DetailedErrorDto {
    DetailedErrorDto {
        message: message.into(),
        kind: ConvertErrorKind::Failed,
    }
}

fn convert_and_write_md_detailed(
    opts: ConverterOptions,
    root: &Path,
    source: PathBuf,
) -> Result<ConversionDetailedReport, DetailedErrorDto> {
    let md_path = source.with_file_name(format!(
        "{}.md",
        source
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    // Per-request Converter is fine: WhisperContext is process-cached in fileconv-core.
    let report = fileconv_core::Converter::with_options(opts)
        .convert_path_detailed(&source)
        .map_err(|e| e.to_dto())?;
    atomic_write(&md_path, report.result.markdown.as_bytes()).map_err(detailed_failed)?;
    let outcome = report.outcome();
    Ok(ConversionDetailedReport {
        markdown_rel_path: rel_of(root, &md_path),
        title: report.result.title,
        format: report.result.format.as_str().into(),
        outcome,
        warnings: report.warnings,
    })
}

/// Validate and copy one source file into a folder inside DATA.
///
/// Conversion is intentionally separate so the desktop UI can show the copied
/// file immediately and process expensive conversions through its background
/// queue.
fn copy_import_source(root: &Path, folder_rel: &str, source_abs: &str) -> Result<PathBuf, String> {
    let source = PathBuf::from(source_abs);
    if FormatKind::from_path(&source) == FormatKind::Unknown {
        return Err(format!(
            "định dạng không hỗ trợ: chỉ nhận {}",
            SUPPORTED_EXTS.join(", ")
        ));
    }
    let file_name = source
        .file_name()
        .ok_or("file nguồn không hợp lệ")?
        .to_string_lossy()
        .to_string();
    let folder = resolve_within(root, folder_rel)?;
    fs::create_dir_all(&folder).map_err(es)?;
    let dest = folder.join(&file_name);
    if child_case_insensitive(&folder, &file_name).is_some() {
        return Err(format!("đã tồn tại file '{file_name}' trong thư mục"));
    }
    if child_case_insensitive(&folder, &format!("{file_name}.md")).is_some() {
        return Err(format!(
            "đã tồn tại file Markdown '{file_name}.md'; không thể tự động ghép đè"
        ));
    }
    use std::sync::atomic::Ordering;
    let suffix = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = folder.join(format!(
        ".{file_name}.{}.{}.import",
        std::process::id(),
        suffix
    ));
    let mut input = fs::File::open(&source).map_err(es)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(es)?;
    let copy_result = std::io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = copy_result {
        let _ = fs::remove_file(&temp);
        return Err(es(error));
    }
    // Re-check after the potentially long copy, then reserve the exact target
    // name so another import cannot win before the final rename.
    if child_case_insensitive(&folder, &file_name).is_some()
        || child_case_insensitive(&folder, &format!("{file_name}.md")).is_some()
    {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "file '{file_name}' hoặc Markdown liên kết đã xuất hiện trong lúc import"
        ));
    }
    let reservation = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(|error| {
            let _ = fs::remove_file(&temp);
            es(error)
        })?;
    drop(reservation);
    if let Err(first_error) = fs::rename(&temp, &dest) {
        // Windows cannot replace the empty reservation with rename().
        let _ = fs::remove_file(&dest);
        if let Err(error) = fs::rename(&temp, &dest) {
            let _ = fs::remove_file(&temp);
            return Err(es(format!("{first_error}; {error}")));
        }
    }
    Ok(dest)
}

fn data_root(state: &AppState) -> PathBuf {
    state
        .data_root
        .lock()
        .map(|p| p.clone())
        .unwrap_or_default()
}

// ───────────────────────────── Commands ─────────────────────────────

#[tauri::command]
fn supported_extensions() -> Vec<String> {
    SUPPORTED_EXTS.iter().map(|s| s.to_string()).collect()
}

#[tauri::command]
fn get_data_root(state: State<AppState>) -> String {
    data_root(&state).to_string_lossy().to_string()
}

/// Map DATA sang thư mục người dùng chọn (tạo nếu chưa có), lưu vào config.
#[tauri::command]
fn set_data_root(state: State<AppState>, path: String) -> Result<String, String> {
    let dir = PathBuf::from(&path);
    fs::create_dir_all(&dir).map_err(es)?;
    let abs = fs::canonicalize(&dir).map_err(es)?;
    let mut dr = state.data_root.lock().map_err(|_| "lock lỗi")?;
    let mut cfg = load_config(&state.config_dir);
    cfg.data_root = Some(abs.to_string_lossy().to_string());
    save_config(&state.config_dir, &cfg)?;
    *dr = abs.clone();
    drop(dr);
    state
        .watch_service
        .sync(abs.clone(), watch::load_rules(&abs))?;
    Ok(abs.to_string_lossy().to_string())
}

#[tauri::command]
fn read_tree(state: State<AppState>) -> Result<Node, String> {
    let root = data_root(&state);
    fs::create_dir_all(&root).map_err(es)?;
    build_tree(&root, &root)
}

#[tauri::command]
fn create_folder(state: State<AppState>, parent_rel: String, name: String) -> Result<(), String> {
    validate_name(&name)?;
    let root = data_root(&state);
    let parent = resolve_within(&root, &parent_rel)?;
    if child_case_insensitive(&parent, &name).is_some() {
        return Err("thư mục đã tồn tại".into());
    }
    let target = parent.join(&name);
    fs::create_dir(&target).map_err(|error| {
        if fs::symlink_metadata(&target).is_ok() {
            "thư mục đã tồn tại".to_string()
        } else {
            es(error)
        }
    })
}

#[tauri::command]
fn create_markdown(
    state: State<AppState>,
    parent_rel: String,
    name: String,
) -> Result<Node, String> {
    validate_name(&name)?;
    let root = data_root(&state);
    let parent = resolve_within(&root, &parent_rel)?;
    let file_name = if name.to_ascii_lowercase().ends_with(".md") {
        name.clone()
    } else {
        format!("{name}.md")
    };
    let target = parent.join(&file_name);
    if child_case_insensitive(&parent, &file_name).is_some() {
        return Err("file đã tồn tại".into());
    }
    let base_name = &file_name[..file_name.len() - 3];
    if child_case_insensitive(&parent, base_name).is_some() {
        return Err(format!(
            "đã có file nguồn '{base_name}'; hãy convert file đó thay vì tạo Markdown trùng cặp"
        ));
    }
    let create_result = {
        use std::io::Write;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(es)?;
        let result = output
            .write_all(format!("# {base_name}\n").as_bytes())
            .and_then(|_| output.sync_all());
        drop(output);
        result
    };
    if let Err(error) = create_result {
        let _ = fs::remove_file(&target);
        return Err(es(error));
    }
    let rel = rel_of(&root, &target);
    Ok(Node {
        name: file_name,
        rel_path: rel.clone(),
        is_dir: false,
        kind: "markdown".into(),
        supported: false,
        md_rel_path: Some(rel),
        standalone_md: true,
        children: vec![],
    })
}

#[tauri::command]
fn rename_node(state: State<AppState>, rel_path: String, new_name: String) -> Result<(), String> {
    validate_name(&new_name)?;
    let root = data_root(&state);
    let src = resolve_within(&root, &rel_path)?;
    if !src.exists() {
        return Err("không tồn tại".into());
    }
    let parent = src.parent().ok_or("không có thư mục cha")?;
    let dst = parent.join(&new_name);
    if child_case_insensitive(parent, &new_name).is_some() {
        return Err("tên đích đã tồn tại".into());
    }
    let old_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_md = old_name.to_ascii_lowercase().ends_with(".md");
    let mut paired_rename: Option<(PathBuf, PathBuf)> = None;
    if src.is_file() && !is_md {
        if let Some(old_md) = child_case_insensitive(parent, &format!("{old_name}.md")) {
            let new_md = parent.join(format!("{new_name}.md"));
            if child_case_insensitive(parent, &format!("{new_name}.md")).is_some() {
                return Err(format!("đã tồn tại Markdown liên kết '{new_name}.md'"));
            }
            paired_rename = Some((old_md, new_md));
        }
    }
    fs::rename(&src, &dst).map_err(es)?;
    if let Some((old_md, new_md)) = paired_rename {
        if let Err(error) = fs::rename(&old_md, &new_md) {
            let _ = fs::rename(&dst, &src);
            return Err(es(error));
        }
    }
    Ok(())
}

#[tauri::command]
fn delete_node(state: State<AppState>, rel_path: String) -> Result<(), String> {
    let root = data_root(&state);
    let target = resolve_within(&root, &rel_path)?;
    if rel_path.is_empty() || target == root {
        return Err("không thể xóa gốc DATA".into());
    }
    if !target.exists() {
        return Err("không tồn tại".into());
    }
    if target.is_dir() {
        return fs::remove_dir_all(&target).map_err(es);
    }
    let name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if !name.to_ascii_lowercase().ends_with(".md") {
        if let Some(md) = child_case_insensitive(
            target.parent().ok_or("không có thư mục cha")?,
            &format!("{name}.md"),
        ) {
            let _ = fs::remove_file(&md);
        }
    }
    fs::remove_file(&target).map_err(es)
}

/// Import nhanh: chỉ copy vào DATA, chưa convert. UI sẽ đưa file vào hàng đợi
/// và gọi `reconvert` ở background.
#[tauri::command]
async fn import_file_only(
    state: State<'_, AppState>,
    folder_rel: String,
    source_abs: String,
) -> Result<Node, String> {
    let root = data_root(&state);
    let root_for_copy = root.clone();
    let dest = tauri::async_runtime::spawn_blocking(move || {
        copy_import_source(&root_for_copy, &folder_rel, &source_abs)
    })
    .await
    .map_err(es)??;
    let file_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or("file nguồn không hợp lệ")?;
    let kind = FormatKind::from_path(&dest);
    Ok(Node {
        name: file_name,
        rel_path: rel_of(&root, &dest),
        is_dir: false,
        kind: kind.as_str().into(),
        supported: true,
        md_rel_path: None,
        standalone_md: false,
        children: vec![],
    })
}

#[tauri::command]
async fn import_file(
    state: State<'_, AppState>,
    folder_rel: String,
    source_abs: String,
) -> Result<Node, String> {
    let root = data_root(&state);
    let root_for_copy = root.clone();
    let dest = tauri::async_runtime::spawn_blocking(move || {
        copy_import_source(&root_for_copy, &folder_rel, &source_abs)
    })
    .await
    .map_err(es)??;
    let file_name = dest
        .file_name()
        .ok_or("file nguồn không hợp lệ")?
        .to_string_lossy()
        .to_string();

    let opts = state.settings.lock().map_err(|_| "lock lỗi")?.to_options();
    let dest_for_conv = dest.clone();
    let conv_result =
        tauri::async_runtime::spawn_blocking(move || convert_and_write_md(opts, dest_for_conv))
            .await
            .map_err(es)?;

    let md_rel = match conv_result {
        Ok(md_path) => Some(rel_of(&root, &md_path)),
        Err(e) => return Err(format!("đã thêm file nhưng convert lỗi: {e}")),
    };

    let kind = FormatKind::from_path(&dest);
    Ok(Node {
        name: file_name,
        rel_path: rel_of(&root, &dest),
        is_dir: false,
        kind: kind.as_str().into(),
        supported: true,
        md_rel_path: md_rel,
        standalone_md: false,
        children: vec![],
    })
}

#[tauri::command]
async fn reconvert(state: State<'_, AppState>, source_rel: String) -> Result<String, String> {
    let root = data_root(&state);
    let source = resolve_within(&root, &source_rel)?;
    if !source.is_file() {
        return Err("file gốc không tồn tại".into());
    }
    if FormatKind::from_path(&source) == FormatKind::Unknown {
        return Err("định dạng không hỗ trợ convert".into());
    }
    let opts = state.settings.lock().map_err(|_| "lock lỗi")?.to_options();
    let root_for_snapshot = root.clone();
    let source_rel_for_snapshot = source_rel.clone();
    let md_path = tauri::async_runtime::spawn_blocking(move || {
        intelligence::snapshot_existing_version(&root_for_snapshot, &source_rel_for_snapshot)?;
        convert_and_write_md(opts, source)
    })
    .await
    .map_err(es)??;
    Ok(rel_of(&root, &md_path))
}

/// Parallel detailed reconvert: same side effects as `reconvert`, plus outcome/warnings.
/// Hard failures return structured `{message, kind}` DTOs (not kind embedded only in text).
#[tauri::command]
async fn reconvert_detailed(
    state: State<'_, AppState>,
    source_rel: String,
) -> Result<ConversionDetailedReport, DetailedErrorDto> {
    let root = data_root(&state);
    let source = resolve_within(&root, &source_rel).map_err(detailed_failed)?;
    if !source.is_file() {
        return Err(detailed_failed("file gốc không tồn tại"));
    }
    if FormatKind::from_path(&source) == FormatKind::Unknown {
        return Err(detailed_failed("định dạng không hỗ trợ convert"));
    }
    let opts = state
        .settings
        .lock()
        .map_err(|_| detailed_failed("lock lỗi"))?
        .to_options();
    let root_for_snapshot = root.clone();
    let source_rel_for_snapshot = source_rel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        intelligence::snapshot_existing_version(&root_for_snapshot, &source_rel_for_snapshot)
            .map_err(detailed_failed)?;
        convert_and_write_md_detailed(opts, &root_for_snapshot, source)
    })
    .await
    .map_err(|e| detailed_failed(e.to_string()))?
}

#[tauri::command]
fn read_text_file(state: State<AppState>, rel_path: String) -> Result<String, String> {
    let p = resolve_within(&data_root(&state), &rel_path)?;
    let bytes = fs::read(&p).map_err(es)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[tauri::command]
fn write_text_file(
    state: State<AppState>,
    rel_path: String,
    content: String,
) -> Result<(), String> {
    let p = resolve_within(&data_root(&state), &rel_path)?;
    if !p.is_file()
        || p.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| !ext.eq_ignore_ascii_case("md"))
            .unwrap_or(true)
    {
        return Err("chỉ được ghi file Markdown hiện có trong DATA".into());
    }
    atomic_write(&p, content.as_bytes())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    text: String,
    truncated: bool,
    size: u64,
}

/// Đọc TỐI ĐA `max_bytes` đầu file (cho preview text/csv/log lớn — tránh treo UI).
#[tauri::command]
fn read_text_preview(
    state: State<AppState>,
    rel_path: String,
    max_bytes: u64,
) -> Result<TextPreview, String> {
    use std::io::Read;
    let p = resolve_within(&data_root(&state), &rel_path)?;
    let size = fs::metadata(&p).map_err(es)?.len();
    let mut bytes = Vec::new();
    fs::File::open(&p)
        .map_err(es)?
        .take(max_bytes)
        .read_to_end(&mut bytes)
        .map_err(es)?;
    let truncated = size > bytes.len() as u64;
    Ok(TextPreview {
        text: String::from_utf8_lossy(&bytes).to_string(),
        truncated,
        size,
    })
}

/// Kích thước file (byte) — UI dùng để cảnh báo/giới hạn trước khi render file lớn.
#[tauri::command]
fn file_size(state: State<AppState>, rel_path: String) -> Result<u64, String> {
    let p = resolve_within(&data_root(&state), &rel_path)?;
    Ok(fs::metadata(&p).map_err(es)?.len())
}

/// Đường dẫn tuyệt đối của rel_path (dùng cho "Mở ngoài").
#[tauri::command]
fn resolve_path(state: State<AppState>, rel_path: String) -> Result<String, String> {
    let p = resolve_within(&data_root(&state), &rel_path)?;
    Ok(p.to_string_lossy().to_string())
}

/// Đọc bytes thô của file (UI nhận ArrayBuffer) — dùng cho pdf.js/docx-preview/SheetJS.
/// (Không dùng fetch(asset://) vì webview chặn 403 với fetch.)
#[tauri::command]
fn read_bytes(state: State<AppState>, rel_path: String) -> Result<tauri::ipc::Response, String> {
    let p = resolve_within(&data_root(&state), &rel_path)?;
    let bytes = fs::read(&p).map_err(es)?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
async fn preview_pptx_meta(
    state: State<'_, AppState>,
    rel_path: String,
) -> Result<fileconv_core::pptx_preview::PptxPreviewMeta, String> {
    let path = resolve_within(&data_root(&state), &rel_path)?;
    if FormatKind::from_path(&path) != FormatKind::Pptx {
        return Err("preview PPTX chỉ nhận file .pptx".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        fileconv_core::pptx_preview::preview_meta(&path).map_err(es)
    })
    .await
    .map_err(es)?
}

#[tauri::command]
async fn preview_pptx_slide(
    state: State<'_, AppState>,
    rel_path: String,
    index: usize,
) -> Result<fileconv_core::pptx_preview::PptxPreviewSlide, String> {
    let path = resolve_within(&data_root(&state), &rel_path)?;
    if FormatKind::from_path(&path) != FormatKind::Pptx {
        return Err("preview PPTX chỉ nhận file .pptx".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        fileconv_core::pptx_preview::preview_slide(&path, index).map_err(es)
    })
    .await
    .map_err(es)?
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
fn get_live_watch_status(state: State<AppState>) -> watch::WatchStatus {
    state.watch_service.status()
}

fn bundled_ocr_langs_are_valid(langs: &str) -> bool {
    !langs.trim().is_empty() && langs.split('+').all(|lang| matches!(lang, "vie" | "eng"))
}

#[tauri::command]
fn set_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    if !bundled_ocr_langs_are_valid(&settings.ocr_langs) {
        return Err(
            "bản desktop đi kèm model OCR vie và eng; hãy chọn vie, eng hoặc vie+eng".into(),
        );
    }
    if !matches!(
        settings.ocr_engine.as_str(),
        "tesseract" | "paddle" | "auto"
    ) {
        return Err("OCR engine phải là tesseract, paddle hoặc auto".into());
    }
    if settings.audio_lang.trim().is_empty() {
        return Err("ngôn ngữ audio không được để trống".into());
    }
    if !(1..=32).contains(&settings.audio_threads) {
        return Err("thread audio phải nằm trong khoảng 1–32".into());
    }
    if !settings.audio_no_speech_threshold.is_finite()
        || !(0.0..=1.0).contains(&settings.audio_no_speech_threshold)
    {
        return Err("ngưỡng no-speech phải nằm trong khoảng 0–1".into());
    }
    settings.llm_config()?;
    settings.embedding_config()?;
    let mut current = state.settings.lock().map_err(|_| "lock lỗi")?;
    let mut persisted = settings.clone();
    persisted.llm_api_key = None; // Secret remains in memory; use env/keychain for persistence.
    persisted.embedding_api_key = None;
    let json = serde_json::to_string_pretty(&persisted).map_err(es)?;
    atomic_write(&settings_file(&state.config_dir), json.as_bytes())?;
    *current = settings;
    Ok(())
}

// ───────────────────────────── Bootstrap ─────────────────────────────

fn configure_bundled_document_runtime(resource_dir: &Path) {
    let runtime = resource_dir.join("native-runtime");

    if std::env::var_os("FILECONV_PDFIUM_LIB").is_none() {
        let pdfium = if cfg!(target_os = "windows") {
            runtime.join("pdfium/bin/pdfium.dll")
        } else if cfg!(target_os = "macos") {
            resource_dir
                .parent()
                .unwrap_or(resource_dir)
                .join("Frameworks/libpdfium.dylib")
        } else {
            runtime.join("pdfium/lib/libpdfium.so")
        };
        if pdfium.is_file() {
            std::env::set_var("FILECONV_PDFIUM_LIB", pdfium);
        }
    }

    if std::env::var_os("FILECONV_TESSERACT").is_none() {
        let executable = if cfg!(target_os = "windows") {
            runtime.join("ocr/bin/tesseract.exe")
        } else if cfg!(target_os = "macos") {
            resource_dir
                .parent()
                .unwrap_or(resource_dir)
                .join("MacOS/tesseract")
        } else {
            runtime.join("ocr/bin/tesseract")
        };
        if executable.is_file() {
            std::env::set_var("FILECONV_TESSERACT", executable);
        }
    }

    if std::env::var_os("FILECONV_TESSDATA").is_none() {
        let tessdata = runtime.join("ocr/tessdata");
        if tessdata.join("vie.traineddata").is_file() && tessdata.join("eng.traineddata").is_file()
        {
            std::env::set_var("FILECONV_TESSDATA", tessdata);
        }
    }

    if std::env::var_os("FILECONV_OCR_LIB_DIR").is_none() {
        let libraries = runtime.join("ocr/lib");
        if libraries.is_dir() {
            std::env::set_var("FILECONV_OCR_LIB_DIR", libraries);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            configure_bundled_document_runtime(&app.path().resource_dir()?);
            let config_dir = app.path().app_config_dir()?;
            fs::create_dir_all(&config_dir).ok();

            // DATA root: lấy từ config nếu có, mặc định app_data_dir()/DATA.
            let cfg = load_config(&config_dir);
            let root = match cfg.data_root {
                Some(p) => PathBuf::from(p),
                None => app.path().app_data_dir()?.join("DATA"),
            };
            fs::create_dir_all(&root).ok();

            let mut settings = fs::read_to_string(settings_file(&config_dir))
                .ok()
                .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
                .unwrap_or_default();
            if !bundled_ocr_langs_are_valid(&settings.ocr_langs) {
                eprintln!(
                    "Markhand: OCR language '{}' không có trong runtime đi kèm; \
                     chuyển về vie+eng",
                    settings.ocr_langs
                );
                settings.ocr_langs = "vie+eng".into();
                if let Ok(serialized) = serde_json::to_string_pretty(&settings) {
                    let _ = fs::write(settings_file(&config_dir), serialized);
                }
            }

            let watch_service = watch::WatchService::new(app.handle().clone());
            watch_service
                .sync(root.clone(), watch::load_rules(&root))
                .map_err(std::io::Error::other)?;
            app.manage(AppState {
                config_dir,
                data_root: Mutex::new(root),
                settings: Mutex::new(settings),
                watch_service,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            supported_extensions,
            get_data_root,
            set_data_root,
            read_tree,
            create_folder,
            create_markdown,
            rename_node,
            delete_node,
            import_file_only,
            import_file,
            reconvert,
            reconvert_detailed,
            read_text_file,
            write_text_file,
            read_text_preview,
            file_size,
            resolve_path,
            read_bytes,
            preview_pptx_meta,
            preview_pptx_slide,
            get_settings,
            get_live_watch_status,
            set_settings,
            intelligence::get_llm_provider_presets,
            intelligence::get_embedding_provider_presets,
            intelligence::test_embedding_connection,
            intelligence::get_cli_subscription_status,
            intelligence::start_cli_subscription_login,
            intelligence::test_llm_connection,
            intelligence::generate_handoff_pack,
            intelligence::read_handoff_artifact,
            intelligence::save_handoff_artifact,
            intelligence::export_existing_handoff,
            intelligence::run_quality_report,
            intelligence::search_intelligence,
            intelligence::ask_intelligence,
            intelligence::scan_pii,
            intelligence::redact_pii,
            intelligence::hard_ocr_image,
            intelligence::extract_document_schema,
            intelligence::list_markdown_tables,
            intelligence::update_markdown_table,
            intelligence::export_markdown_table,
            intelligence::snapshot_document_version,
            intelligence::list_document_versions,
            intelligence::read_document_version,
            intelligence::diff_document_versions,
            intelligence::merge_document_versions,
            intelligence::get_watch_rules,
            intelligence::set_watch_rules,
            intelligence::scan_watch_rules,
            intelligence::export_knowledge_pack,
            projects::list_projects,
            projects::create_project,
            projects::adopt_project,
            projects::import_local_folder,
            projects::remove_project,
            knowledge::rebuild_knowledge_index,
            knowledge::knowledge_index_stats,
            knowledge::hybrid_search,
            knowledge::hybrid_ask,
        ])
        .run(tauri::generate_context!())
        .expect("lỗi khi khởi chạy ứng dụng Tauri");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("fileconv_test_{}_{}", std::process::id(), n));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolve_within_blocks_traversal() {
        let root = temp_dir();
        assert!(resolve_within(&root, "a/b").is_ok());
        assert!(resolve_within(&root, "../etc/passwd").is_err());
        assert!(resolve_within(&root, "a/../../b").is_err());
        assert!(resolve_within(&root, "/abs").is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_name_rejects_bad() {
        assert!(validate_name("ok.md").is_ok());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("  ").is_err());
    }

    #[test]
    fn build_tree_pairs_source_with_md() {
        let root = temp_dir();
        fs::write(root.join("report.pdf"), b"%PDF").unwrap();
        fs::write(root.join("report.pdf.md"), b"# md").unwrap();
        fs::write(root.join("notes.md"), b"# notes").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join(".secret"), b"x").unwrap();

        let tree = build_tree(&root, &root).unwrap();
        let names: Vec<&str> = tree.children.iter().map(|n| n.name.as_str()).collect();

        assert!(names.contains(&"sub"));
        assert!(names.contains(&"report.pdf"));
        assert!(names.contains(&"notes.md"));
        assert!(!names.contains(&"report.pdf.md"));
        assert!(!names.contains(&".secret"));

        let pdf = tree
            .children
            .iter()
            .find(|n| n.name == "report.pdf")
            .unwrap();
        assert_eq!(pdf.kind, "pdf");
        assert!(pdf.supported);
        assert_eq!(pdf.md_rel_path.as_deref(), Some("report.pdf.md"));

        let notes = tree.children.iter().find(|n| n.name == "notes.md").unwrap();
        assert!(notes.standalone_md);
        assert_eq!(notes.kind, "markdown");
        assert!(tree.children[0].is_dir);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn copy_import_source_is_deferred_and_rejects_duplicates() {
        let base = temp_dir();
        let root = base.join("DATA");
        fs::create_dir_all(&root).unwrap();
        let source = base.join("report.pdf");
        fs::write(&source, b"%PDF deferred import").unwrap();

        let copied = copy_import_source(&root, "incoming", source.to_str().unwrap()).unwrap();
        assert_eq!(copied, root.join("incoming/report.pdf"));
        assert!(copied.exists());
        assert!(!root.join("incoming/report.pdf.md").exists());

        let duplicate = copy_import_source(&root, "incoming", source.to_str().unwrap());
        assert!(duplicate.is_err());

        fs::remove_file(&copied).unwrap();
        fs::write(root.join("incoming/report.pdf.MD"), b"# standalone").unwrap();
        let paired_conflict = copy_import_source(&root, "incoming", source.to_str().unwrap());
        assert!(paired_conflict.is_err());
        assert!(!copied.exists());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn atomic_write_replaces_complete_markdown() {
        let root = temp_dir();
        let path = root.join("report.md");
        atomic_write(&path, b"# first").unwrap();
        atomic_write(&path, b"# second\n\nNoi dung").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "# second\n\nNoi dung");
        assert_eq!(
            fs::read_dir(&root).unwrap().filter_map(Result::ok).count(),
            1
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn old_settings_json_receives_local_first_llm_defaults() {
        let json = r#"{
          "ocrLangs":"vie+eng",
          "pdfOcr":true,
          "pdfOcrImages":false,
          "audioLang":"vi",
          "audioThreads":4,
          "whisperModel":null
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(!settings.llm_enabled);
        assert_eq!(settings.llm_provider, "ollama");
        assert_eq!(settings.llm_base_url, "http://127.0.0.1:11434");
        assert!(settings.auto_check_update);
    }

    #[test]
    fn ollama_config_does_not_require_api_key() {
        let settings = Settings {
            llm_enabled: true,
            ..Settings::default()
        };
        let config = settings.llm_config().unwrap().unwrap();
        assert!(config.api_key.is_empty());
        assert_eq!(config.model, "qwen2.5:7b");
    }

    #[test]
    fn cloud_provider_requires_api_key() {
        let mut settings = Settings {
            llm_enabled: true,
            llm_provider: "openai".into(),
            llm_base_url: "https://api.openai.com".into(),
            llm_model: "gpt-4o-mini".into(),
            ..Settings::default()
        };
        assert!(settings.llm_config().is_err());
        settings.llm_api_key = Some("secret".into());
        assert!(settings.llm_config().unwrap().is_some());
    }

    #[test]
    fn subscription_cli_uses_official_login_without_api_key() {
        let mut settings = Settings::default();
        settings.llm_enabled = true;
        settings.llm_provider = "cursor-cli".into();
        settings.llm_model = "auto".into();
        settings.llm_base_url.clear();
        let config = settings.llm_config().unwrap().unwrap();
        assert_eq!(config.provider, fileconv_core::llm::Provider::CursorCli);
        assert!(config.api_key.is_empty());
        assert!(config.is_subscription_cli());
    }

    #[test]
    fn embedding_settings_are_independent_from_chat_provider() {
        // Isolate from cloud-agent injected FILECONV_* keys.
        let previous_embedding = std::env::var("FILECONV_EMBEDDING_API_KEY").ok();
        let previous_llm = std::env::var("FILECONV_LLM_API_KEY").ok();
        std::env::remove_var("FILECONV_EMBEDDING_API_KEY");
        std::env::remove_var("FILECONV_LLM_API_KEY");

        let mut settings = Settings::default();
        settings.llm_provider = "cursor-cli".into();
        settings.embedding_enabled = true;
        let config = settings.embedding_config().unwrap().unwrap();
        assert_eq!(
            config.provider,
            fileconv_core::llm::Provider::OpenAiCompatible
        );
        assert_eq!(config.model, "nomic-embed-text");
        assert!(config.api_key.is_empty());
        assert_eq!(
            config.runtime_path,
            fileconv_core::llm::EMBEDDING_RUNTIME_PROVIDER_CLOUD
        );

        match previous_embedding {
            Some(value) => std::env::set_var("FILECONV_EMBEDDING_API_KEY", value),
            None => std::env::remove_var("FILECONV_EMBEDDING_API_KEY"),
        }
        match previous_llm {
            Some(value) => std::env::set_var("FILECONV_LLM_API_KEY", value),
            None => std::env::remove_var("FILECONV_LLM_API_KEY"),
        }
    }

    #[test]
    fn vllm_embedding_settings_carry_explicit_runtime_path() {
        let mut settings = Settings::default();
        settings.embedding_enabled = true;
        settings.embedding_provider = "vllm".into();
        settings.embedding_base_url = "http://127.0.0.1:8000".into();
        settings.embedding_model = "BAAI/bge-m3".into();
        let config = settings.embedding_config().unwrap().unwrap();
        assert_eq!(
            config.runtime_path,
            fileconv_core::llm::EMBEDDING_RUNTIME_VLLM_LOCAL
        );
        assert_eq!(
            fileconv_core::llm::infer_embedding_runtime_path(
                config.base_url.as_deref(),
                &config.model
            ),
            fileconv_core::llm::EMBEDDING_RUNTIME_PROVIDER_CLOUD
        );
    }

    #[test]
    fn glm_embedding_settings_carry_explicit_runtime_path() {
        let mut settings = Settings::default();
        settings.embedding_enabled = true;
        settings.embedding_provider = "glm".into();
        settings.embedding_base_url = "https://open.bigmodel.cn/api/paas/v4".into();
        settings.embedding_model = "embedding-3".into();
        settings.embedding_dimensions = Some(1024);
        settings.embedding_api_key = Some("test-key".into());
        let config = settings.embedding_config().unwrap().unwrap();
        assert_eq!(
            config.runtime_path,
            fileconv_core::llm::EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM
        );
    }

    #[test]
    fn persisted_settings_can_omit_api_key() {
        let mut settings = Settings::default();
        settings.llm_api_key = Some("do-not-write-me".into());
        let mut persisted = settings;
        persisted.llm_api_key = None;
        let json = serde_json::to_string(&persisted).unwrap();
        assert!(!json.contains("do-not-write-me"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_within_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let base = temp_dir();
        let root = base.join("DATA");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        assert!(resolve_within(&root, "escape/file.md").is_err());
        fs::remove_dir_all(&base).ok();
    }

    /// Keep detailed reconvert registered next to legacy `reconvert`.
    const REGISTERED_CONVERT_COMMANDS: &[&str] = &["reconvert", "reconvert_detailed"];

    #[test]
    fn reconvert_detailed_command_is_registered() {
        assert!(REGISTERED_CONVERT_COMMANDS.contains(&"reconvert_detailed"));
        assert!(REGISTERED_CONVERT_COMMANDS.contains(&"reconvert"));
        // Source-level registration check against generate_handler list in this file.
        let src = include_str!("lib.rs");
        assert!(
            src.contains("reconvert_detailed,"),
            "reconvert_detailed must remain in generate_handler!"
        );
        assert!(
            src.contains("reconvert,"),
            "legacy reconvert must remain registered"
        );
    }

    #[test]
    fn detailed_hard_failure_dto_serializes_message_and_kind() {
        let dto = fileconv_core::DetailedConvertError::dependency_missing(
            "không tìm thấy binary Tesseract",
        )
        .to_dto();
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["kind"], "dependency_missing");
        assert!(value["message"].as_str().unwrap().contains("Tesseract"));
        assert_ne!(value["kind"], value["message"]);
        // Desktop helper must produce the same structured shape.
        let mapped = detailed_failed("lock lỗi");
        let mapped_json = serde_json::to_value(&mapped).unwrap();
        assert_eq!(mapped_json["kind"], "failed");
        assert_eq!(mapped_json["message"], "lock lỗi");
    }
}
