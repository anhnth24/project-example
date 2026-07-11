use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{
    atomic_write, child_case_insensitive, data_root, es, rel_of, resolve_within, AppState,
    SUPPORTED_EXTS,
};

const MAX_IMPORT_FILES: usize = 5_000;
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_rel: String,
    pub created_at: u64,
    pub imported_from: Option<String>,
    pub implicit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptProjectRequest {
    pub folder_rel: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFolderRequest {
    pub project_id: String,
    pub source_abs: String,
    pub target_folder_rel: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFolderResult {
    pub project: Project,
    pub imported: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub convert_rels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectRequest {
    pub project_id: String,
    pub delete_contents: bool,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stable_key(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn projects_path(root: &Path) -> Result<PathBuf, String> {
    let markhand = resolve_within(root, ".markhand")?;
    Ok(markhand.join("projects.json"))
}

fn read_registered(root: &Path) -> Result<Vec<Project>, String> {
    let path = projects_path(root)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&fs::read(path).map_err(es)?).map_err(es)
}

fn write_registered(root: &Path, projects: &[Project]) -> Result<(), String> {
    let path = projects_path(root)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(es)?;
    }
    let json = serde_json::to_vec_pretty(projects).map_err(es)?;
    atomic_write(&path, &json)
}

fn project_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            separator = false;
        } else {
            separator = true;
        }
    }
    slug.trim_matches('-').chars().take(80).collect()
}

fn unique_folder_name(parent: &Path, preferred: &str) -> String {
    if child_case_insensitive(parent, preferred).is_none() {
        return preferred.to_string();
    }
    for suffix in 2..10_000 {
        let candidate = format!("{preferred}-{suffix}");
        if child_case_insensitive(parent, &candidate).is_none() {
            return candidate;
        }
    }
    format!("{preferred}-{}", now_epoch())
}

fn discover_projects(root: &Path, mut registered: Vec<Project>) -> Result<Vec<Project>, String> {
    let registered_roots: std::collections::HashSet<String> = registered
        .iter()
        .map(|project| project.root_rel.clone())
        .collect();
    for entry in fs::read_dir(root).map_err(es)? {
        let entry = entry.map_err(es)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !entry.file_type().map_err(es)?.is_dir() {
            continue;
        }
        let rel = rel_of(root, &entry.path());
        if !registered_roots.contains(&rel) {
            registered.push(Project {
                id: format!("legacy-{}", stable_key(&rel)),
                name,
                root_rel: rel,
                created_at: 0,
                imported_from: None,
                implicit: true,
            });
        }
    }
    let root_has_files = fs::read_dir(root)
        .map_err(es)?
        .filter_map(Result::ok)
        .any(|entry| {
            !entry.file_name().to_string_lossy().starts_with('.')
                && entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
        });
    if root_has_files && !registered.iter().any(|project| project.root_rel.is_empty()) {
        registered.push(Project {
            id: "legacy-root".into(),
            name: "DATA (Legacy)".into(),
            root_rel: String::new(),
            created_at: 0,
            imported_from: None,
            implicit: true,
        });
    }
    registered.sort_by(|a, b| {
        a.implicit
            .cmp(&b.implicit)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(registered)
}

fn list_projects_inner(root: &Path) -> Result<Vec<Project>, String> {
    discover_projects(root, read_registered(root)?)
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>, String> {
    list_projects_inner(&data_root(&state))
}

#[tauri::command]
pub fn create_project(
    state: State<AppState>,
    req: CreateProjectRequest,
) -> Result<Project, String> {
    let root = data_root(&state);
    let name = req.name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err("tên dự án không hợp lệ".into());
    }
    let preferred = project_slug(name);
    if preferred.is_empty() {
        return Err("tên dự án không tạo được folder hợp lệ".into());
    }
    let folder = unique_folder_name(&root, &preferred);
    let path = root.join(&folder);
    fs::create_dir(&path).map_err(es)?;
    let created_at = now_epoch();
    let project = Project {
        id: format!("project-{created_at}-{}", stable_key(&folder)),
        name: name.to_string(),
        root_rel: folder,
        created_at,
        imported_from: None,
        implicit: false,
    };
    let mut projects = read_registered(&root)?;
    projects.push(project.clone());
    write_registered(&root, &projects)?;
    Ok(project)
}

#[tauri::command]
pub fn adopt_project(state: State<AppState>, req: AdoptProjectRequest) -> Result<Project, String> {
    let root = data_root(&state);
    let folder = resolve_within(&root, &req.folder_rel)?;
    if !folder.is_dir() || req.folder_rel.is_empty() {
        return Err("folder dự án không hợp lệ".into());
    }
    let mut projects = read_registered(&root)?;
    if projects
        .iter()
        .any(|project| project.root_rel.eq_ignore_ascii_case(&req.folder_rel))
    {
        return Err("folder đã được đăng ký làm dự án".into());
    }
    let name = req.name.unwrap_or_else(|| {
        folder
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "Dự án".into())
    });
    let created_at = now_epoch();
    let project = Project {
        id: format!("project-{created_at}-{}", stable_key(&req.folder_rel)),
        name,
        root_rel: req.folder_rel,
        created_at,
        imported_from: None,
        implicit: false,
    };
    projects.push(project.clone());
    write_registered(&root, &projects)?;
    Ok(project)
}

fn staged_copy(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("đích không có parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.import",
        destination
            .file_name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    fs::rename(temp, destination)
}

fn collect_files(
    current: &Path,
    relative: &Path,
    files: &mut Vec<(PathBuf, PathBuf, u64)>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    for entry in fs::read_dir(current).map_err(es)? {
        let entry = entry.map_err(es)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(es)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let rel = relative.join(entry.file_name());
        if metadata.is_dir() {
            collect_files(&entry.path(), &rel, files, total_bytes)?;
        } else if metadata.is_file() {
            if files.len() >= MAX_IMPORT_FILES {
                return Err(format!("folder vượt giới hạn {MAX_IMPORT_FILES} file"));
            }
            *total_bytes = total_bytes.saturating_add(metadata.len());
            if *total_bytes > MAX_IMPORT_BYTES {
                return Err("folder vượt giới hạn import 2GB".into());
            }
            files.push((entry.path(), rel, metadata.len()));
        }
    }
    Ok(())
}

fn supported_source(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    SUPPORTED_EXTS.contains(&extension.as_str())
}

#[tauri::command]
pub async fn import_local_folder(
    state: State<'_, AppState>,
    req: ImportFolderRequest,
) -> Result<ImportFolderResult, String> {
    let root = data_root(&state);
    let projects = list_projects_inner(&root)?;
    let project = projects
        .into_iter()
        .find(|project| project.id == req.project_id)
        .ok_or("không tìm thấy dự án")?;
    let source = fs::canonicalize(&req.source_abs).map_err(es)?;
    let root_canonical = fs::canonicalize(&root).map_err(es)?;
    if !source.is_dir() {
        return Err("đường dẫn local không phải folder".into());
    }
    if source.starts_with(&root_canonical) {
        return Err("không import folder nằm bên trong DATA".into());
    }
    let target_base = if let Some(target) = req.target_folder_rel.as_deref() {
        let target = resolve_within(&root, target)?;
        if !target.starts_with(resolve_within(&root, &project.root_rel)?) {
            return Err("folder đích nằm ngoài dự án".into());
        }
        target
    } else {
        resolve_within(&root, &project.root_rel)?
    };
    let source_name = source
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "imported-folder".into());
    let folder_name = unique_folder_name(&target_base, &source_name);
    let destination_root = target_base.join(folder_name);

    tauri::async_runtime::spawn_blocking(move || {
        let mut files = Vec::new();
        let mut total_bytes = 0u64;
        collect_files(&source, Path::new(""), &mut files, &mut total_bytes)?;
        let source_rel_set: std::collections::HashSet<PathBuf> = files
            .iter()
            .map(|(_, relative, _)| relative.clone())
            .collect();
        let mut imported = 0usize;
        let mut skipped = 0usize;
        let mut convert_rels = Vec::new();
        for (source_file, relative, _) in files {
            let destination = destination_root.join(&relative);
            if destination.exists() {
                skipped += 1;
                continue;
            }
            staged_copy(&source_file, &destination).map_err(es)?;
            imported += 1;
            if supported_source(&destination) {
                let paired = PathBuf::from(format!("{}.md", relative.to_string_lossy()));
                if !source_rel_set.contains(&paired) {
                    convert_rels.push(rel_of(&root, &destination));
                }
            }
        }
        Ok(ImportFolderResult {
            project,
            imported,
            skipped,
            bytes: total_bytes,
            convert_rels,
        })
    })
    .await
    .map_err(es)?
}

#[tauri::command]
pub fn remove_project(state: State<AppState>, req: RemoveProjectRequest) -> Result<(), String> {
    let root = data_root(&state);
    let mut projects = read_registered(&root)?;
    let Some(index) = projects
        .iter()
        .position(|project| project.id == req.project_id)
    else {
        return Err("không tìm thấy dự án đã đăng ký".into());
    };
    let project = projects.remove(index);
    if req.delete_contents {
        let directory = resolve_within(&root, &project.root_rel)?;
        if project.root_rel.is_empty() {
            return Err("không thể xóa DATA root".into());
        }
        fs::remove_dir_all(directory).map_err(es)?;
    }
    write_registered(&root, &projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "markhand_projects_{}_{}_{}",
            label,
            std::process::id(),
            count
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn slug_preserves_vietnamese_letters_and_hierarchy_name() {
        assert_eq!(project_slug("Dự án Thanh toán"), "dự-án-thanh-toán");
        assert_eq!(project_slug("  A / B  "), "a-b");
    }

    #[test]
    fn discovers_legacy_top_level_folders() {
        let root = temp_dir("legacy");
        fs::create_dir(root.join("Project A")).unwrap();
        let projects = list_projects_inner(&root).unwrap();
        assert_eq!(projects.len(), 1);
        assert!(projects[0].implicit);
        assert_eq!(projects[0].root_rel, "Project A");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn root_files_create_legacy_data_project() {
        let root = temp_dir("root");
        fs::write(root.join("notes.md"), "# Notes").unwrap();
        let projects = list_projects_inner(&root).unwrap();
        assert!(projects.iter().any(|project| project.id == "legacy-root"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn collect_files_preserves_nested_relative_paths_and_skips_symlinks() {
        let source = temp_dir("source");
        fs::create_dir_all(source.join("docs/nested")).unwrap();
        fs::write(source.join("docs/nested/a.pdf"), b"pdf").unwrap();
        fs::write(source.join("README.md"), b"md").unwrap();
        let mut files = Vec::new();
        let mut bytes = 0;
        collect_files(&source, Path::new(""), &mut files, &mut bytes).unwrap();
        assert!(files
            .iter()
            .any(|(_, relative, _)| relative == Path::new("docs/nested/a.pdf")));
        assert!(files
            .iter()
            .any(|(_, relative, _)| relative == Path::new("README.md")));
        fs::remove_dir_all(source).ok();
    }

    #[test]
    fn staged_copy_does_not_overwrite_existing_file() {
        let root = temp_dir("copy");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        assert!(staged_copy(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(destination).unwrap(), "old");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn supported_source_matches_converter_extensions() {
        assert!(supported_source(Path::new("report.PDF")));
        assert!(supported_source(Path::new("sheet.xlsx")));
        assert!(!supported_source(Path::new("notes.md")));
        assert!(!supported_source(Path::new("binary.exe")));
    }
}
