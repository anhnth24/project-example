//! Direct converter subprocess runner (no Linux sandbox isolation).
//!
//! Used on Windows and when `MARKHAND_CONVERTER_DISABLE_SANDBOX=1` on Unix dev hosts.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::limits::ResourceLimits;

const INPUT_PLACEHOLDER: &str = "{input}";
const POLL_INTERVAL: Duration = Duration::from_millis(20);

const MAX_OCR_ARTIFACTS: usize = 512;
const MAX_OCR_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_OCR_ARTIFACTS_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DirectRunConfig {
    pub argv_template: Vec<String>,
    pub limits: ResourceLimits,
    /// When true, argv[0] must be an absolute path (production sandbox parity).
    pub require_absolute_executable: bool,
}

#[derive(Debug)]
pub struct DirectRunInput {
    pub bytes: Vec<u8>,
    pub canonical_extension: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectRunExit {
    Success,
    Exit(i32),
    Signaled(i32),
    TimedOut,
    Cancelled,
}

impl DirectRunExit {
    pub const fn success(self) -> bool {
        matches!(self, Self::Success)
    }
}

#[derive(Debug, Clone)]
pub struct DirectOcrArtifact {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct DirectRunOutput {
    pub exit: DirectRunExit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub workspace_path: PathBuf,
    pub ocr_artifacts: Vec<DirectOcrArtifact>,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectRunError {
    #[error("converter configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("converter subprocess io failed")]
    Io(#[from] std::io::Error),
    #[error("deferred OCR artifacts exceed caps")]
    OcrArtifactsTooLarge,
}

pub fn direct_mode_enabled() -> bool {
    #[cfg(not(unix))]
    {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            std::env::var("MARKHAND_CONVERTER_DISABLE_SANDBOX")
                .ok()
                .map(|value| value.trim().to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    }
}

pub fn validate_direct_config(config: &DirectRunConfig) -> Result<(), DirectRunError> {
    if config.argv_template.is_empty() {
        return Err(DirectRunError::InvalidConfig(
            "converter argv template must not be empty".into(),
        ));
    }
    let executable = config
        .argv_template
        .first()
        .ok_or_else(|| DirectRunError::InvalidConfig("converter argv is empty".into()))?;
    if config.require_absolute_executable && !Path::new(executable).is_absolute() {
        return Err(DirectRunError::InvalidConfig(
            "converter executable must be an absolute path".into(),
        ));
    }
    if !config
        .argv_template
        .iter()
        .any(|arg| arg.contains(INPUT_PLACEHOLDER))
    {
        return Err(DirectRunError::InvalidConfig(
            "converter argv template must contain {input}".into(),
        ));
    }
    config
        .limits
        .validate()
        .map_err(DirectRunError::InvalidConfig)?;
    Ok(())
}

pub fn run_direct(
    config: &DirectRunConfig,
    input: DirectRunInput,
    cancel: &AtomicBool,
) -> Result<DirectRunOutput, DirectRunError> {
    validate_direct_config(config)?;
    let workspace = TempDir::new()?;
    let workspace_path = workspace.path().to_path_buf();
    let input_name = safe_input_name(&input.canonical_extension)?;
    let input_path = workspace.path().join(input_name);
    {
        let mut file = File::create(&input_path)?;
        file.write_all(&input.bytes)?;
        file.sync_all()?;
    }

    let argv = materialize_argv(&config.argv_template, &input_path)?;
    let executable = resolve_executable(&argv[0])?;
    let mut command = Command::new(&executable);
    command
        .args(&argv[1..])
        .current_dir(workspace.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in ["FILECONV_PDFIUM_LIB", "LANG", "PATH"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                command.env(key, value);
            }
        }
    }

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let deadline = Instant::now() + config.limits.wall_timeout;
    let exit = loop {
        if let Some(status) = child.try_wait()? {
            break exit_from_status(status);
        }
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            break DirectRunExit::Cancelled;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break DirectRunExit::TimedOut;
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let mut stdout_capture = CapturedPipe::new(config.limits.stdout_stderr_bytes);
    let mut stderr_capture = CapturedPipe::new(config.limits.stdout_stderr_bytes);
    read_to_end_limited(&mut stdout, &mut stdout_capture)?;
    read_to_end_limited(&mut stderr, &mut stderr_capture)?;

    let ocr_artifacts = if exit == DirectRunExit::Success {
        collect_ocr_artifacts(workspace.path())?
    } else {
        Vec::new()
    };
    drop(workspace);
    Ok(DirectRunOutput {
        exit,
        stdout: stdout_capture.bytes,
        stderr: stderr_capture.bytes,
        stdout_truncated: stdout_capture.truncated,
        stderr_truncated: stderr_capture.truncated,
        workspace_path,
        ocr_artifacts,
    })
}

fn resolve_executable(path: &str) -> Result<PathBuf, DirectRunError> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return Ok(candidate);
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(candidate))
        .map_err(DirectRunError::Io)
}

fn materialize_argv(template: &[String], input_path: &Path) -> Result<Vec<String>, DirectRunError> {
    let input = input_path
        .to_str()
        .ok_or_else(|| DirectRunError::InvalidConfig("input path is not UTF-8".into()))?;
    Ok(template
        .iter()
        .map(|arg| arg.replace(INPUT_PLACEHOLDER, input))
        .collect())
}

fn safe_input_name(extension: &str) -> Result<String, DirectRunError> {
    let ext = extension.trim_start_matches('.').to_ascii_lowercase();
    if ext.is_empty()
        || ext.len() > 16
        || !ext
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(DirectRunError::InvalidConfig(
            "canonical extension is invalid".into(),
        ));
    }
    Ok(format!("input.{ext}"))
}

struct CapturedPipe {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
    eof: bool,
}

impl CapturedPipe {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(8192)),
            limit,
            truncated: false,
            eof: false,
        }
    }
}

fn read_to_end_limited(
    reader: &mut impl std::io::Read,
    capture: &mut CapturedPipe,
) -> Result<(), DirectRunError> {
    let mut buf = [0_u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                capture.eof = true;
                return Ok(());
            }
            Ok(n) => {
                let remaining = capture.limit.saturating_sub(capture.bytes.len());
                if remaining == 0 {
                    capture.truncated = true;
                    continue;
                }
                let take = n.min(remaining);
                capture.bytes.extend_from_slice(&buf[..take]);
                if take < n {
                    capture.truncated = true;
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn exit_from_status(status: ExitStatus) -> DirectRunExit {
    if status.success() {
        DirectRunExit::Success
    } else if let Some(code) = status.code() {
        DirectRunExit::Exit(code)
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            DirectRunExit::Signaled(status.signal().unwrap_or(0))
        }
        #[cfg(not(unix))]
        {
            DirectRunExit::Signaled(0)
        }
    }
}

fn collect_ocr_artifacts(workspace: &Path) -> Result<Vec<DirectOcrArtifact>, DirectRunError> {
    let mut artifacts = Vec::new();
    let mut total: u64 = 0;
    let entries = match fs::read_dir(workspace) {
        Ok(entries) => entries,
        Err(_) => return Ok(artifacts),
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with("markhand-ocr-") || !name.ends_with(".jpg") {
            continue;
        }
        let metadata = entry.metadata().map_err(DirectRunError::Io)?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > MAX_OCR_ARTIFACT_BYTES {
            return Err(DirectRunError::OcrArtifactsTooLarge);
        }
        total = total.saturating_add(metadata.len());
        if total > MAX_OCR_ARTIFACTS_TOTAL_BYTES || artifacts.len() >= MAX_OCR_ARTIFACTS {
            return Err(DirectRunError::OcrArtifactsTooLarge);
        }
        let bytes = fs::read(entry.path()).map_err(DirectRunError::Io)?;
        artifacts.push(DirectOcrArtifact {
            name: name.to_string(),
            bytes,
        });
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn direct_mode_enabled_on_non_unix() {
        #[cfg(not(unix))]
        assert!(direct_mode_enabled());
    }

    #[test]
    fn validate_rejects_missing_input_placeholder() {
        let config = DirectRunConfig {
            argv_template: vec!["fileconv".into(), "one".into()],
            limits: ResourceLimits::default(),
            require_absolute_executable: false,
        };
        assert!(validate_direct_config(&config).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn run_direct_echoes_txt() {
        let cancel = AtomicBool::new(false);
        let output = run_direct(
            &DirectRunConfig {
                argv_template: if cfg!(windows) {
                    vec![
                        "cmd".into(),
                        "/C".into(),
                        "type".into(),
                        INPUT_PLACEHOLDER.into(),
                    ]
                } else {
                    vec!["/bin/cat".into(), INPUT_PLACEHOLDER.into()]
                },
                limits: ResourceLimits {
                    wall_timeout: Duration::from_secs(10),
                    ..ResourceLimits::default()
                },
                require_absolute_executable: false,
            },
            DirectRunInput {
                bytes: b"hello direct".to_vec(),
                canonical_extension: "txt".into(),
            },
            &cancel,
        )
        .expect("direct run");
        assert!(output.exit.success());
        assert_eq!(output.stdout, b"hello direct");
    }
}
