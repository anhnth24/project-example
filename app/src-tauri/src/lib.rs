//! Backend Tauri cho FileConv Docs.
//!
//! Cầu nối giữa UI (React) và lõi `fileconv-core`. Mọi thao tác filesystem đều chạy
//! trong tiến trình Rust (không bật plugin fs) để kiểm soát chặt đường dẫn.
//!
//! Mô hình dữ liệu:
//!   - Workspace = một thư mục THẬT trên đĩa người dùng chọn. Danh sách lưu ở
//!     `app_config_dir()/workspaces.json`.
//!   - Folder = thư mục con thật. Document = cặp (file gốc, file `.md`).
//!   - Quy ước link 1-1: md = "<tên file gốc>.md" đặt cạnh file gốc
//!     (vd `report.pdf` -> `report.pdf.md`). Filesystem là nguồn sự thật.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use fileconv_core::{ConverterOptions, FormatKind};

// ───────────────────────────── Kiểu dữ liệu ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Một node trong cây thư mục gửi cho UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub name: String,
    /// Đường dẫn tương đối so với gốc workspace (dùng `/`). "" với node gốc.
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
    /// Đường dẫn model whisper GGML; None = audio chưa khả dụng.
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

pub struct AppState {
    config_dir: PathBuf,
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

fn workspaces_file(config_dir: &Path) -> PathBuf {
    config_dir.join("workspaces.json")
}

fn settings_file(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

fn load_workspaces(config_dir: &Path) -> Vec<Workspace> {
    let p = workspaces_file(config_dir);
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_workspaces(config_dir: &Path, list: &[Workspace]) -> Result<(), String> {
    let p = workspaces_file(config_dir);
    let s = serde_json::to_string_pretty(list).map_err(es)?;
    fs::write(p, s).map_err(es)
}

fn find_workspace(config_dir: &Path, id: &str) -> Result<Workspace, String> {
    load_workspaces(config_dir)
        .into_iter()
        .find(|w| w.id == id)
        .ok_or_else(|| format!("không tìm thấy workspace: {id}"))
}

/// id ổn định theo đường dẫn (hash) — cùng thư mục => cùng id, tránh trùng.
fn workspace_id(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("ws_{:016x}", h.finish())
}

/// Ghép `rel` vào trong `root` một cách an toàn (chặn `..`, đường dẫn tuyệt đối).
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

/// Tên file phải hợp lệ: không rỗng, không chứa ký tự tách đường dẫn / traversal.
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

/// Dựng cây thư mục đệ quy. Bỏ qua file/thư mục ẩn (vd `.fileconv`).
/// File `.md` là "md liên kết" của một file gốc cùng tên thì KHÔNG hiện riêng.
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

    // Tập tên FILE trong thư mục này (để nhận diện md-liên-kết).
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
                // md liên kết của `base` -> thể hiện qua node của file gốc, bỏ qua ở đây.
                continue;
            }
            // md đứng riêng.
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

    // Sắp xếp: thư mục trước, rồi theo tên (không phân biệt hoa thường).
    children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Node {
        name: abs
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        rel_path: rel_of(root, abs),
        is_dir: true,
        kind: "folder".into(),
        supported: false,
        md_rel_path: None,
        standalone_md: false,
        children,
    })
}

/// Convert một file gốc -> markdown rồi ghi `<file>.md` cạnh nó.
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

// ───────────────────────────── Commands ─────────────────────────────

#[tauri::command]
fn supported_extensions() -> Vec<String> {
    SUPPORTED_EXTS.iter().map(|s| s.to_string()).collect()
}

#[tauri::command]
fn list_workspaces(state: State<AppState>) -> Vec<Workspace> {
    load_workspaces(&state.config_dir)
}

#[tauri::command]
fn add_workspace(
    state: State<AppState>,
    path: String,
    name: Option<String>,
) -> Result<Workspace, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err("thư mục không tồn tại".into());
    }
    let abs = fs::canonicalize(&dir).map_err(es)?;
    let path_str = abs.to_string_lossy().to_string();
    let id = workspace_id(&path_str);

    let mut list = load_workspaces(&state.config_dir);
    if let Some(existing) = list.iter().find(|w| w.id == id) {
        return Ok(existing.clone()); // đã có -> trả lại, không nhân đôi.
    }
    let ws = Workspace {
        id,
        name: name.unwrap_or_else(|| {
            abs.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.clone())
        }),
        path: path_str,
    };
    list.push(ws.clone());
    save_workspaces(&state.config_dir, &list)?;
    Ok(ws)
}

#[tauri::command]
fn remove_workspace(state: State<AppState>, id: String) -> Result<(), String> {
    // Chỉ gỡ khỏi danh sách — KHÔNG xóa file trên đĩa của người dùng.
    let mut list = load_workspaces(&state.config_dir);
    list.retain(|w| w.id != id);
    save_workspaces(&state.config_dir, &list)
}

#[tauri::command]
fn read_tree(state: State<AppState>, workspace_id: String) -> Result<Node, String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    if !root.is_dir() {
        return Err("thư mục workspace không còn tồn tại".into());
    }
    let mut node = build_tree(&root, &root)?;
    node.name = ws.name; // node gốc lấy tên workspace.
    Ok(node)
}

#[tauri::command]
fn create_folder(
    state: State<AppState>,
    workspace_id: String,
    parent_rel: String,
    name: String,
) -> Result<(), String> {
    validate_name(&name)?;
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let parent = resolve_within(&root, &parent_rel)?;
    let target = parent.join(&name);
    if target.exists() {
        return Err("thư mục/đã tồn tại".into());
    }
    fs::create_dir_all(&target).map_err(es)
}

#[tauri::command]
fn create_markdown(
    state: State<AppState>,
    workspace_id: String,
    parent_rel: String,
    name: String,
) -> Result<Node, String> {
    validate_name(&name)?;
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
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
fn rename_node(
    state: State<AppState>,
    workspace_id: String,
    rel_path: String,
    new_name: String,
) -> Result<(), String> {
    validate_name(&new_name)?;
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let src = resolve_within(&root, &rel_path)?;
    if !src.exists() {
        return Err("không tồn tại".into());
    }
    let parent = src.parent().ok_or("không có thư mục cha")?;
    let dst = parent.join(&new_name);
    if dst.exists() {
        return Err("tên đích đã tồn tại".into());
    }
    // Nếu là file gốc có md liên kết, đổi tên md theo.
    let is_file = src.is_file();
    let old_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_md = old_name.to_ascii_lowercase().ends_with(".md");
    if is_file && !is_md {
        let old_md = parent.join(format!("{old_name}.md"));
        if old_md.exists() {
            let new_md = parent.join(format!("{new_name}.md"));
            fs::rename(&old_md, &new_md).map_err(es)?;
        }
    }
    fs::rename(&src, &dst).map_err(es)
}

#[tauri::command]
fn delete_node(
    state: State<AppState>,
    workspace_id: String,
    rel_path: String,
) -> Result<(), String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let target = resolve_within(&root, &rel_path)?;
    if rel_path.is_empty() || target == root {
        return Err("không thể xóa gốc workspace".into());
    }
    if !target.exists() {
        return Err("không tồn tại".into());
    }
    if target.is_dir() {
        return fs::remove_dir_all(&target).map_err(es);
    }
    // File: nếu là file gốc, xóa kèm md liên kết.
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
    workspace_id: String,
    folder_rel: String,
    source_abs: String,
) -> Result<Node, String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let source = PathBuf::from(&source_abs);

    // Chặn định dạng không hỗ trợ.
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
    if !folder.is_dir() {
        return Err("thư mục đích không tồn tại".into());
    }
    let dest = folder.join(&file_name);
    if dest.exists() {
        return Err(format!("đã tồn tại file '{file_name}' trong thư mục"));
    }
    fs::copy(&source, &dest).map_err(es)?;

    // Convert nặng -> chạy ngoài luồng UI.
    let opts = state.settings.lock().map_err(|_| "lock lỗi")?.to_options();
    let dest_for_conv = dest.clone();
    let conv_result = tauri::async_runtime::spawn_blocking(move || {
        convert_and_write_md(opts, dest_for_conv)
    })
    .await
    .map_err(es)?;

    let md_rel = match conv_result {
        Ok(md_path) => Some(rel_of(&root, &md_path)),
        Err(e) => {
            // Vẫn giữ file gốc đã copy; báo lỗi để UI hiện toast, người dùng có thể Convert lại.
            return Err(format!("đã thêm file nhưng convert lỗi: {e}"));
        }
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
async fn reconvert(
    state: State<'_, AppState>,
    workspace_id: String,
    source_rel: String,
) -> Result<String, String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
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
fn read_text_file(
    state: State<AppState>,
    workspace_id: String,
    rel_path: String,
) -> Result<String, String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let p = resolve_within(&root, &rel_path)?;
    let bytes = fs::read(&p).map_err(es)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[tauri::command]
fn write_text_file(
    state: State<AppState>,
    workspace_id: String,
    rel_path: String,
    content: String,
) -> Result<(), String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let p = resolve_within(&root, &rel_path)?;
    fs::write(&p, content).map_err(es)
}

/// Trả đường dẫn tuyệt đối của một rel_path (để UI gọi `convertFileSrc` hiển thị ảnh/pdf/audio).
#[tauri::command]
fn resolve_path(
    state: State<AppState>,
    workspace_id: String,
    rel_path: String,
) -> Result<String, String> {
    let ws = find_workspace(&state.config_dir, &workspace_id)?;
    let root = PathBuf::from(&ws.path);
    let p = resolve_within(&root, &rel_path)?;
    Ok(p.to_string_lossy().to_string())
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
    let p = settings_file(&state.config_dir);
    fs::write(p, serde_json::to_string_pretty(&settings).map_err(es)?).map_err(es)
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
            // Nạp settings đã lưu (nếu có).
            let settings = fs::read_to_string(settings_file(&config_dir))
                .ok()
                .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
                .unwrap_or_default();
            app.manage(AppState {
                config_dir,
                settings: Mutex::new(settings),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            supported_extensions,
            list_workspaces,
            add_workspace,
            remove_workspace,
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
    fn workspace_id_is_stable() {
        assert_eq!(workspace_id("/home/x"), workspace_id("/home/x"));
        assert_ne!(workspace_id("/home/x"), workspace_id("/home/y"));
    }

    #[test]
    fn build_tree_pairs_source_with_md() {
        let root = temp_dir();
        // report.pdf + report.pdf.md  -> 1 node "report.pdf" có mdRelPath, md không hiện riêng.
        fs::write(root.join("report.pdf"), b"%PDF").unwrap();
        fs::write(root.join("report.pdf.md"), b"# md").unwrap();
        // notes.md đứng riêng (không có file "notes").
        fs::write(root.join("notes.md"), b"# notes").unwrap();
        // thư mục con.
        fs::create_dir_all(root.join("sub")).unwrap();
        // file ẩn bị bỏ qua.
        fs::write(root.join(".secret"), b"x").unwrap();

        let tree = build_tree(&root, &root).unwrap();
        let names: Vec<&str> = tree.children.iter().map(|n| n.name.as_str()).collect();

        // Có "sub", "report.pdf", "notes.md"; KHÔNG có "report.pdf.md" hay ".secret".
        assert!(names.contains(&"sub"));
        assert!(names.contains(&"report.pdf"));
        assert!(names.contains(&"notes.md"));
        assert!(!names.contains(&"report.pdf.md"));
        assert!(!names.contains(&".secret"));

        let pdf = tree.children.iter().find(|n| n.name == "report.pdf").unwrap();
        assert_eq!(pdf.kind, "pdf");
        assert!(pdf.supported);
        assert_eq!(pdf.md_rel_path.as_deref(), Some("report.pdf.md"));
        assert!(!pdf.standalone_md);

        let notes = tree.children.iter().find(|n| n.name == "notes.md").unwrap();
        assert!(notes.standalone_md);
        assert_eq!(notes.kind, "markdown");

        // thư mục đứng trước file (sắp xếp).
        assert!(tree.children[0].is_dir);

        fs::remove_dir_all(&root).ok();
    }
}
