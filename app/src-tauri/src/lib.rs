//! Backend Tauri cho FileConv Docs.
//!
//! Cầu nối giữa UI (React) và lõi `fileconv-core`. Mọi thao tác filesystem chạy trong
//! tiến trình Rust để kiểm soát chặt đường dẫn (không bật plugin fs).
//!
//! Mô hình dữ liệu (đơn giản hóa — không còn multi-workspace):
//!   - Một **thư mục gốc DATA** duy nhất. Mặc định: `app_data_dir()/DATA`.
//!     Người dùng có thể **map** DATA sang thư mục bất kỳ (lưu ở `config.json`).
//!   - Trong DATA: tạo folder thật → upload file vào → convert → ghi `.md` cạnh file gốc.
//!   - Quy ước link 1-1: `report.pdf` -> `report.pdf.md`. Filesystem là nguồn sự thật.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use fileconv_core::{ConverterOptions, FormatKind};

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
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub ocr_langs: String,
    pub pdf_ocr: bool,
    pub pdf_ocr_images: bool,
    pub audio_lang: String,
    pub audio_threads: i32,
    pub whisper_model: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let d = ConverterOptions::default();
        Self {
            ocr_langs: d.ocr_langs,
            pdf_ocr: d.pdf_ocr,
            pdf_ocr_images: d.pdf_ocr_images,
            audio_lang: d.audio_lang,
            audio_threads: d.audio_threads,
            whisper_model: None,
        }
    }
}

impl Settings {
    fn to_options(&self) -> ConverterOptions {
        ConverterOptions {
            ocr_langs: self.ocr_langs.clone(),
            whisper_model: self.whisper_model.as_ref().map(PathBuf::from),
            audio_lang: self.audio_lang.clone(),
            audio_threads: self.audio_threads,
            pdf_ocr: self.pdf_ocr,
            pdf_ocr_images: self.pdf_ocr_images,
        }
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
}

// ───────────────────────────── Helper ─────────────────────────────

fn es<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Đuôi file mà lõi convert hỗ trợ (suy từ `FormatKind::from_path`).
const SUPPORTED_EXTS: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "xls", "xlsb", "ods", "csv", "html", "htm", "png", "jpg",
    "jpeg", "webp", "bmp", "tif", "tiff", "gif", "wav", "mp3", "m4a", "flac", "ogg",
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
    fs::write(
        config_file(config_dir),
        serde_json::to_string_pretty(cfg).map_err(es)?,
    )
    .map_err(es)
}

/// Ghép `rel` vào trong `root` an toàn (chặn `..`, đường dẫn tuyệt đối).
fn resolve_within(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut p = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => p.push(c),
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
        let is_dir = p.is_dir();
        entries.push((name, p, is_dir));
    }

    let file_names: HashSet<String> = entries
        .iter()
        .filter(|(_, _, d)| !d)
        .map(|(n, _, _)| n.clone())
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
            if file_names.contains(base) {
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
            let md_rel = if file_names.contains(&md_name) {
                Some(rel_of(root, &path.with_file_name(&md_name)))
            } else {
                None
            };
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
    let conv = fileconv_core::Converter::with_options(opts);
    let result = conv
        .convert_path(&source)
        .map_err(|e| format!("convert thất bại: {e}"))?;
    fs::write(&md_path, result.markdown).map_err(es)?;
    Ok(md_path)
}

fn data_root(state: &AppState) -> PathBuf {
    state.data_root.lock().map(|p| p.clone()).unwrap_or_default()
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
    {
        let mut dr = state.data_root.lock().map_err(|_| "lock lỗi")?;
        *dr = abs.clone();
    }
    let mut cfg = load_config(&state.config_dir);
    cfg.data_root = Some(abs.to_string_lossy().to_string());
    save_config(&state.config_dir, &cfg)?;
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
    let target = resolve_within(&root, &parent_rel)?.join(&name);
    if target.exists() {
        return Err("thư mục đã tồn tại".into());
    }
    fs::create_dir_all(&target).map_err(es)
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
    if target.exists() {
        return Err("file đã tồn tại".into());
    }
    fs::write(&target, format!("# {}\n", name.trim_end_matches(".md"))).map_err(es)?;
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
    if dst.exists() {
        return Err("tên đích đã tồn tại".into());
    }
    let old_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_md = old_name.to_ascii_lowercase().ends_with(".md");
    if src.is_file() && !is_md {
        let old_md = parent.join(format!("{old_name}.md"));
        if old_md.exists() {
            fs::rename(&old_md, parent.join(format!("{new_name}.md"))).map_err(es)?;
        }
    }
    fs::rename(&src, &dst).map_err(es)
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
        let md = target.with_file_name(format!("{name}.md"));
        if md.exists() {
            let _ = fs::remove_file(&md);
        }
    }
    fs::remove_file(&target).map_err(es)
}

#[tauri::command]
async fn import_file(
    state: State<'_, AppState>,
    folder_rel: String,
    source_abs: String,
) -> Result<Node, String> {
    let root = data_root(&state);
    let source = PathBuf::from(&source_abs);

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

    let folder = resolve_within(&root, &folder_rel)?;
    fs::create_dir_all(&folder).map_err(es)?;
    let dest = folder.join(&file_name);
    if dest.exists() {
        return Err(format!("đã tồn tại file '{file_name}' trong thư mục"));
    }
    fs::copy(&source, &dest).map_err(es)?;

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
    let md_path = tauri::async_runtime::spawn_blocking(move || convert_and_write_md(opts, source))
        .await
        .map_err(es)??;
    Ok(rel_of(&root, &md_path))
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
    fs::write(&p, content).map_err(es)
}

/// Đường dẫn tuyệt đối của rel_path (để UI gọi `convertFileSrc` hiển thị ảnh/audio).
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
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
fn set_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    {
        let mut s = state.settings.lock().map_err(|_| "lock lỗi")?;
        *s = settings.clone();
    }
    fs::write(
        settings_file(&state.config_dir),
        serde_json::to_string_pretty(&settings).map_err(es)?,
    )
    .map_err(es)
}

// ───────────────────────────── Bootstrap ─────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            fs::create_dir_all(&config_dir).ok();

            // DATA root: lấy từ config nếu có, mặc định app_data_dir()/DATA.
            let cfg = load_config(&config_dir);
            let root = match cfg.data_root {
                Some(p) => PathBuf::from(p),
                None => app.path().app_data_dir()?.join("DATA"),
            };
            fs::create_dir_all(&root).ok();

            let settings = fs::read_to_string(settings_file(&config_dir))
                .ok()
                .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
                .unwrap_or_default();

            app.manage(AppState {
                config_dir,
                data_root: Mutex::new(root),
                settings: Mutex::new(settings),
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
            import_file,
            reconvert,
            read_text_file,
            write_text_file,
            resolve_path,
            read_bytes,
            get_settings,
            set_settings,
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

        let pdf = tree.children.iter().find(|n| n.name == "report.pdf").unwrap();
        assert_eq!(pdf.kind, "pdf");
        assert!(pdf.supported);
        assert_eq!(pdf.md_rel_path.as_deref(), Some("report.pdf.md"));

        let notes = tree.children.iter().find(|n| n.name == "notes.md").unwrap();
        assert!(notes.standalone_md);
        assert_eq!(notes.kind, "markdown");
        assert!(tree.children[0].is_dir);

        fs::remove_dir_all(&root).ok();
    }
}
