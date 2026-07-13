use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use fileconv_core::intelligence::{watch_pattern_matches, WatchMatch, WatchRule};
use fileconv_core::FormatKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const DEBOUNCE: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub state: String,
    pub rules: usize,
    pub paths: usize,
    pub last_error: Option<String>,
}

impl Default for WatchStatus {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            rules: 0,
            paths: 0,
            last_error: None,
        }
    }
}

enum WatchCommand {
    Sync {
        data_root: PathBuf,
        rules: Vec<WatchRule>,
    },
    Shutdown,
}

#[derive(Clone)]
struct ActiveRule {
    rule: WatchRule,
    root: PathBuf,
}

pub struct WatchService {
    sender: mpsc::Sender<WatchCommand>,
    status: Arc<Mutex<WatchStatus>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl WatchService {
    pub fn new(app: AppHandle) -> Self {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(WatchStatus::default()));
        let worker_status = status.clone();
        let worker = std::thread::spawn(move || worker_loop(app, receiver, worker_status));
        Self {
            sender,
            status,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn sync(&self, data_root: PathBuf, rules: Vec<WatchRule>) -> Result<(), String> {
        if let Ok(mut status) = self.status.lock() {
            status.state = "starting".into();
            status.rules = rules.iter().filter(|rule| rule.enabled).count();
            status.last_error = None;
        }
        self.sender
            .send(WatchCommand::Sync { data_root, rules })
            .map_err(|_| "watch service đã dừng".to_string())
    }

    pub fn status(&self) -> WatchStatus {
        self.status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| WatchStatus {
                state: "error".into(),
                last_error: Some("watch status lock lỗi".into()),
                ..Default::default()
            })
    }
}

impl Drop for WatchService {
    fn drop(&mut self) {
        let _ = self.sender.send(WatchCommand::Shutdown);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

pub fn load_rules(data_root: &Path) -> Vec<WatchRule> {
    let path = data_root.join(".markhand/watch-rules.json");
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn update_status(app: &AppHandle, shared: &Arc<Mutex<WatchStatus>>, value: WatchStatus) {
    if let Ok(mut status) = shared.lock() {
        *status = value.clone();
    }
    let _ = app.emit("watch:status", value);
}

fn configure_watcher(
    event_sender: mpsc::Sender<notify::Result<notify::Event>>,
    data_root: &Path,
    rules: Vec<WatchRule>,
) -> Result<(RecommendedWatcher, Vec<ActiveRule>, usize), String> {
    let data_root = std::fs::canonicalize(data_root).map_err(|error| error.to_string())?;
    let mut active_rules = Vec::new();
    for rule in rules.into_iter().filter(|rule| rule.enabled) {
        let root = std::fs::canonicalize(&rule.watch_abs)
            .map_err(|error| format!("{}: {error}", rule.watch_abs))?;
        if !root.is_dir() {
            return Err(format!("watch path không phải thư mục: {}", root.display()));
        }
        if root.starts_with(&data_root) {
            return Err(format!(
                "không được watch bên trong DATA (tránh import loop): {}",
                root.display()
            ));
        }
        active_rules.push(ActiveRule { rule, root });
    }

    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_sender.send(event);
    })
    .map_err(|error| error.to_string())?;
    let paths: HashSet<PathBuf> = active_rules.iter().map(|rule| rule.root.clone()).collect();
    for path in &paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|error| format!("watch {}: {error}", path.display()))?;
    }
    Ok((watcher, active_rules, paths.len()))
}

fn should_ignore(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.starts_with('.')
        || name.starts_with("~$")
        || name.ends_with(".tmp")
        || name.ends_with(".part")
        || name.ends_with('~')
}

fn fingerprint(path: &Path) -> Option<(u64, SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

fn process_path(
    app: &AppHandle,
    path: &Path,
    data_root: &Path,
    rules: &[ActiveRule],
    seen: &mut HashMap<(String, PathBuf), (u64, SystemTime)>,
) {
    if should_ignore(path) || !path.is_file() || FormatKind::from_path(path) == FormatKind::Unknown
    {
        return;
    }
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return;
    };
    if canonical.starts_with(data_root) {
        return;
    }
    let Some(name) = canonical.file_name().map(|name| name.to_string_lossy()) else {
        return;
    };
    let Some(current) = fingerprint(&canonical) else {
        return;
    };
    for active in rules {
        if !canonical.starts_with(&active.root)
            || !watch_pattern_matches(&active.rule.pattern, &name)
        {
            continue;
        }
        let key = (active.rule.id.clone(), canonical.clone());
        if seen.get(&key).is_some_and(|previous| *previous == current) {
            continue;
        }
        seen.insert(key, current);
        let payload = WatchMatch {
            rule_id: active.rule.id.clone(),
            source_abs: canonical.to_string_lossy().into_owned(),
            target_folder_rel: active.rule.target_folder_rel.clone(),
            action: active.rule.action.clone(),
        };
        let _ = app.emit("watch:match", payload);
    }
}

fn worker_loop(
    app: AppHandle,
    commands: mpsc::Receiver<WatchCommand>,
    status: Arc<Mutex<WatchStatus>>,
) {
    let (event_sender, events) = mpsc::channel();
    let mut watcher: Option<RecommendedWatcher> = None;
    let mut rules = Vec::new();
    let mut data_root = PathBuf::new();
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let mut seen = HashMap::new();

    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                WatchCommand::Shutdown => return,
                WatchCommand::Sync {
                    data_root: next_root,
                    rules: next_rules,
                } => match configure_watcher(event_sender.clone(), &next_root, next_rules) {
                    Ok((next_watcher, next_active, path_count)) => {
                        watcher = Some(next_watcher);
                        rules = next_active;
                        data_root = std::fs::canonicalize(next_root).unwrap_or_default();
                        pending.clear();
                        seen.clear();
                        update_status(
                            &app,
                            &status,
                            WatchStatus {
                                state: if rules.is_empty() {
                                    "idle".into()
                                } else {
                                    "watching".into()
                                },
                                rules: rules.len(),
                                paths: path_count,
                                last_error: None,
                            },
                        );
                    }
                    Err(error) => {
                        watcher = None;
                        rules.clear();
                        update_status(
                            &app,
                            &status,
                            WatchStatus {
                                state: "error".into(),
                                last_error: Some(error),
                                ..Default::default()
                            },
                        );
                    }
                },
            }
        }

        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) => {
                for path in event.paths {
                    pending.insert(path, Instant::now());
                }
            }
            Ok(Err(error)) => {
                let mut value = status.lock().map(|value| value.clone()).unwrap_or_default();
                value.state = "error".into();
                value.last_error = Some(error.to_string());
                update_status(&app, &status, value);
            }
            _ => {}
        }

        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, queued)| queued.elapsed() >= DEBOUNCE)
            .map(|(path, _)| path.clone())
            .collect();
        for path in ready {
            pending.remove(&path);
            process_path(&app, &path, &data_root, &rules, &mut seen);
        }

        // Keep the watcher alive for the lifetime of this loop.
        std::hint::black_box(&watcher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fileconv_core::intelligence::WatchAction;

    #[test]
    fn ignores_editor_and_atomic_temp_files() {
        assert!(should_ignore(Path::new(".report.pdf.1.tmp")));
        assert!(should_ignore(Path::new("~$report.docx")));
        assert!(should_ignore(Path::new("report.pdf.part")));
        assert!(!should_ignore(Path::new("report.pdf")));
    }

    #[test]
    fn fingerprint_changes_with_size_or_mtime() {
        let path =
            std::env::temp_dir().join(format!("markhand_watch_fingerprint_{}", std::process::id()));
        std::fs::write(&path, b"a").unwrap();
        let first = fingerprint(&path).unwrap();
        std::fs::write(&path, b"longer").unwrap();
        let second = fingerprint(&path).unwrap();
        assert_ne!(first.0, second.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_watching_inside_data_root() {
        let root = std::env::temp_dir().join(format!("markhand_watch_loop_{}", std::process::id()));
        let watched = root.join("incoming");
        std::fs::create_dir_all(&watched).unwrap();
        let (sender, _receiver) = mpsc::channel();
        let result = configure_watcher(
            sender,
            &root,
            vec![WatchRule {
                id: "loop".into(),
                watch_abs: watched.to_string_lossy().into_owned(),
                target_folder_rel: String::new(),
                pattern: "*.pdf".into(),
                action: WatchAction::ImportAndConvert,
                enabled: true,
            }],
        );
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
