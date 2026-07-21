use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::llm::{LlmConfig, Provider};
use crate::ConvertError;

const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
const STATUS_TIMEOUT: Duration = Duration::from_secs(15);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSubscriptionStatus {
    pub bridge: String,
    pub authenticated: bool,
    pub account_hint: Option<String>,
    pub message: String,
}

#[derive(Debug)]
struct CliOutput {
    status: ExitStatus,
    stdout: String,
}

fn fail(message: impl Into<String>) -> ConvertError {
    ConvertError::Failed(message.into())
}

fn bridge_name(provider: Provider) -> Result<&'static str, ConvertError> {
    match provider {
        Provider::CursorCli => Ok("Cursor"),
        Provider::CodexCli => Ok("Codex"),
        _ => Err(fail("provider không phải subscription CLI")),
    }
}

fn default_binary(provider: Provider) -> Result<&'static str, ConvertError> {
    match provider {
        Provider::CursorCli => Ok("agent"),
        Provider::CodexCli => Ok("codex"),
        _ => Err(fail("provider không phải subscription CLI")),
    }
}

fn allowed_binary_name(provider: Provider, path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match provider {
        Provider::CursorCli => matches!(stem.as_str(), "agent" | "cursor-agent"),
        Provider::CodexCli => stem == "codex",
        _ => false,
    }
}

fn binary_for(config: &LlmConfig) -> Result<OsString, ConvertError> {
    let Some(override_path) = config
        .cli_binary
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Ok(default_binary(config.provider)?.into());
    };
    let path = Path::new(override_path);
    if !path.is_file() {
        return Err(fail(format!(
            "không tìm thấy CLI binary: {}",
            path.display()
        )));
    }
    if !allowed_binary_name(config.provider, path) {
        return Err(fail(format!(
            "binary không hợp lệ cho {}: {}",
            bridge_name(config.provider)?,
            path.display()
        )));
    }
    Ok(path.as_os_str().to_owned())
}

fn command_for(config: &LlmConfig) -> Result<Command, ConvertError> {
    let mut command = crate::proc::background_command(binary_for(config)?);
    // Subscription bridges must use the official CLI login, not API-key env vars.
    command
        .env_remove("CURSOR_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

fn read_pipe<T: Read + Send + 'static>(mut pipe: T) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// Join a helper thread without exceeding `deadline`. Abandoned joins are reaped
/// in the background so a stuck pipe cannot block the caller past the budget.
fn join_with_deadline<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
    deadline: Instant,
) -> Option<T> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        std::thread::spawn(move || {
            let _ = handle.join();
        });
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle.join());
    });
    match rx.recv_timeout(remaining) {
        Ok(Ok(value)) => Some(value),
        _ => None,
    }
}

#[cfg(unix)]
fn prepare_unix_containment(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // New process group (= child pid) so SIGKILL to -pgid reaps grandchildren.
    command.process_group(0);
}

#[cfg(windows)]
mod win_job {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
    };

    pub struct WindowsJob(HANDLE);

    impl WindowsJob {
        pub fn create() -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                    return None;
                }
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let _ = CloseHandle(handle);
                    return None;
                }
                Some(Self(handle))
            }
        }

        pub fn assign(&self, child: &Child) -> bool {
            unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) != 0 }
        }

        pub fn terminate(&self) {
            unsafe {
                let _ = TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for WindowsJob {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Resume the primary thread of a CREATE_SUSPENDED process (std::process::Child
    /// does not expose PROCESS_INFORMATION.hThread).
    pub fn resume_primary_thread(pid: u32) -> bool {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..std::mem::zeroed()
            };
            let mut found = Thread32First(snap, &mut entry) != 0;
            let mut resumed = false;
            while found {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() && thread != INVALID_HANDLE_VALUE {
                        resumed = ResumeThread(thread) != u32::MAX;
                        let _ = CloseHandle(thread);
                    }
                    break;
                }
                found = Thread32Next(snap, &mut entry) != 0;
            }
            let _ = CloseHandle(snap);
            resumed
        }
    }

    pub fn apply_suspended_flags(command: &mut Command) {
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }
}

/// Containment token kept for the lifetime of the child.
struct ProcessContainment {
    #[cfg(unix)]
    pgid: u32,
    #[cfg(windows)]
    job: win_job::WindowsJob,
}

impl ProcessContainment {
    fn begin(command: &mut Command) -> Result<Self, ConvertError> {
        #[cfg(unix)]
        {
            prepare_unix_containment(command);
            Ok(Self { pgid: 0 })
        }
        #[cfg(windows)]
        {
            let job = win_job::WindowsJob::create().ok_or_else(|| {
                fail("không tạo được Windows Job Object cho subscription CLI (fail-closed)")
            })?;
            win_job::apply_suspended_flags(command);
            Ok(Self { job })
        }
    }

    fn after_spawn(&mut self, child: &std::process::Child) -> Result<(), ConvertError> {
        #[cfg(unix)]
        {
            self.pgid = child.id();
            Ok(())
        }
        #[cfg(windows)]
        {
            if !self.job.assign(child) {
                return Err(fail(
                    "không gán được CLI vào Windows Job Object (fail-closed)",
                ));
            }
            // Process stayed suspended until job assignment — no grandchild race.
            if !win_job::resume_primary_thread(child.id()) {
                self.job.terminate();
                return Err(fail(
                    "không resume được CLI sau khi gán Job Object (fail-closed)",
                ));
            }
            Ok(())
        }
    }

    fn kill_tree(&self) {
        #[cfg(unix)]
        {
            if self.pgid != 0 {
                extern "C" {
                    fn kill(pid: i32, sig: i32) -> i32;
                }
                const SIGKILL: i32 = 9;
                unsafe {
                    let _ = kill(-(self.pgid as i32), SIGKILL);
                }
            }
        }
        #[cfg(windows)]
        {
            self.job.terminate();
        }
    }
}

fn terminate_contained(child: &mut std::process::Child, containment: &ProcessContainment) {
    containment.kill_tree();
    let _ = child.kill();
    let _ = child.wait();
}

fn run_command(
    mut command: Command,
    input: Option<&str>,
    timeout: Duration,
) -> Result<CliOutput, ConvertError> {
    // Absolute deadline covers spawn, stdin write, wait, and pipe joins.
    let deadline = Instant::now() + timeout;
    let mut containment = ProcessContainment::begin(&mut command)?;
    let mut child = command
        .spawn()
        .map_err(|error| fail(format!("không khởi chạy được CLI: {error}")))?;
    if let Err(error) = containment.after_spawn(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let stdout_reader = child.stdout.take().map(read_pipe);
    let stderr_reader = child.stderr.take().map(read_pipe);
    let stdin_writer = if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| fail("không mở được stdin cho CLI"))?;
        let bytes = input.as_bytes().to_vec();
        Some(std::thread::spawn(move || {
            stdin.write_all(&bytes).and_then(|_| stdin.flush())
        }))
    } else {
        drop(child.stdin.take());
        None
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Parent may exit while grandchildren still hold pipes
                // (`sleep 30 & exit 0`). Reap the process group / job first.
                containment.kill_tree();
                break status;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(None) => {
                terminate_contained(&mut child, &containment);
                if let Some(handle) = stdin_writer {
                    let _ = join_with_deadline(handle, deadline);
                }
                if let Some(handle) = stdout_reader {
                    let _ = join_with_deadline(handle, deadline);
                }
                if let Some(handle) = stderr_reader {
                    let _ = join_with_deadline(handle, deadline);
                }
                return Err(fail(format!(
                    "subscription CLI timeout sau {} giây",
                    timeout.as_secs()
                )));
            }
            Err(error) => {
                terminate_contained(&mut child, &containment);
                if let Some(handle) = stdin_writer {
                    let _ = join_with_deadline(handle, deadline);
                }
                if let Some(handle) = stdout_reader {
                    let _ = join_with_deadline(handle, deadline);
                }
                if let Some(handle) = stderr_reader {
                    let _ = join_with_deadline(handle, deadline);
                }
                return Err(fail(format!("không chờ được CLI: {error}")));
            }
        }
    };

    if let Some(handle) = stdin_writer {
        match join_with_deadline(handle, deadline) {
            Some(Err(error))
                if status.success() && error.kind() != std::io::ErrorKind::BrokenPipe =>
            {
                return Err(fail(format!("không gửi được prompt tới CLI: {error}")));
            }
            _ => {}
        }
    }

    let stdout = stdout_reader
        .and_then(|reader| join_with_deadline(reader, deadline))
        .unwrap_or_default();
    // Drain stderr without exposing OAuth URLs, tokens, or local paths to UI logs.
    if let Some(reader) = stderr_reader {
        let _ = join_with_deadline(reader, deadline);
    }
    Ok(CliOutput {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
    })
}

fn temporary_working_directory() -> Result<PathBuf, ConvertError> {
    let suffix = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "markhand-subscription-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path)
        .map_err(|error| fail(format!("không tạo được thư mục CLI tạm: {error}")))?;
    Ok(path)
}

fn extract_text(value: &serde_json::Value) -> Option<String> {
    value
        .get("result")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("text").and_then(serde_json::Value::as_str))
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            let item = value.get("item")?;
            (item.get("type")?.as_str()? == "agent_message")
                .then(|| item.get("text").and_then(serde_json::Value::as_str))
                .flatten()
        })
        .map(str::to_string)
}

fn parse_cli_answer(stdout: &str) -> Result<String, ConvertError> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout) {
        if let Some(text) = extract_text(&value) {
            return Ok(text);
        }
    }
    for line in stdout.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(text) = extract_text(&value) {
            return Ok(text);
        }
    }
    Err(fail("subscription CLI không trả assistant message hợp lệ"))
}

fn chat_args(config: &LlmConfig) -> Result<Vec<String>, ConvertError> {
    let model = config.model.trim();
    match config.provider {
        Provider::CursorCli => {
            let mut args = vec![
                "-p".into(),
                "--mode".into(),
                "ask".into(),
                "--sandbox".into(),
                "enabled".into(),
                "--output-format".into(),
                "json".into(),
                "--trust".into(),
            ];
            if !model.is_empty() && model != "auto" {
                args.extend(["--model".into(), model.into()]);
            }
            Ok(args)
        }
        Provider::CodexCli => {
            let mut args = vec![
                "exec".into(),
                "--ephemeral".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--skip-git-repo-check".into(),
                "--json".into(),
            ];
            if !model.is_empty() && model != "auto" {
                args.extend(["--model".into(), model.into()]);
            }
            args.push("-".into());
            Ok(args)
        }
        _ => Err(fail("provider không phải subscription CLI")),
    }
}

pub fn chat(config: &LlmConfig, system: &str, user: &str) -> Result<String, ConvertError> {
    let working_directory = temporary_working_directory()?;
    let result = (|| {
        let mut command = command_for(config)?;
        command
            .args(chat_args(config)?)
            .current_dir(&working_directory);
        let prompt = format!(
            "SYSTEM INSTRUCTION:\n{system}\n\nUSER REQUEST:\n{user}\n\n\
             Return only the requested answer. Do not inspect or modify local files."
        );
        let output = run_command(command, Some(&prompt), CHAT_TIMEOUT)?;
        if !output.status.success() {
            return Err(fail(format!(
                "{} CLI thất bại; hãy kiểm tra login và quota subscription",
                bridge_name(config.provider)?
            )));
        }
        parse_cli_answer(&output.stdout)
    })();
    let _ = std::fs::remove_dir_all(working_directory);
    result
}

pub fn subscription_status(config: &LlmConfig) -> Result<CliSubscriptionStatus, ConvertError> {
    let mut command = command_for(config)?;
    match config.provider {
        Provider::CursorCli => {
            command.args(["status", "--format", "json"]);
        }
        Provider::CodexCli => {
            command.args(["login", "status"]);
        }
        _ => return Err(fail("provider không phải subscription CLI")),
    }
    let output = run_command(command, None, STATUS_TIMEOUT)?;
    let authenticated = output.status.success();
    let account_hint = if authenticated {
        serde_json::from_str::<serde_json::Value>(&output.stdout)
            .ok()
            .and_then(|value| {
                value
                    .get("email")
                    .or_else(|| value.get("account"))
                    .or_else(|| value.get("user"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
    } else {
        None
    };
    Ok(CliSubscriptionStatus {
        bridge: bridge_name(config.provider)?.into(),
        authenticated,
        account_hint,
        message: if authenticated {
            "Đã đăng nhập bằng subscription CLI.".into()
        } else {
            format!(
                "Chưa đăng nhập. Hãy chạy `{}` login.",
                default_binary(config.provider)?
            )
        },
    })
}

pub fn start_login(config: &LlmConfig) -> Result<(), ConvertError> {
    let binary = binary_for(config)?;
    let mut command = crate::proc::background_command(binary);
    command
        .arg("login")
        .env_remove("CURSOR_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| fail(format!("không mở được luồng đăng nhập CLI: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mock_cli(name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "markhand_cli_bridge_{}_{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        std::fs::write(
            &path,
            r#"#!/bin/sh
if [ "$1" = "status" ] || [ "$1" = "login" ]; then
  printf '{"email":"user@example.com"}\n'
  exit 0
fi
while IFS= read -r line; do
  :
done
printf '{"type":"result","result":"Grounded mock answer"}\n'
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn parses_cursor_and_codex_output() {
        let cursor = r#"{"type":"result","subtype":"success","result":"Cursor answer"}"#;
        assert_eq!(parse_cli_answer(cursor).unwrap(), "Cursor answer");
        let codex = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"1\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",",
            "\"text\":\"Codex answer\"}}\n"
        );
        assert_eq!(parse_cli_answer(codex).unwrap(), "Codex answer");
    }

    #[test]
    fn rejects_unstructured_cli_output() {
        assert!(parse_cli_answer("login required").is_err());
    }

    #[test]
    fn cli_arguments_enforce_read_only_modes() {
        let cursor = LlmConfig::new_cli(Provider::CursorCli, "auto", None).unwrap();
        let cursor_args = chat_args(&cursor).unwrap();
        assert!(cursor_args.windows(2).any(|args| args == ["--mode", "ask"]));
        assert!(cursor_args
            .windows(2)
            .any(|args| args == ["--sandbox", "enabled"]));

        let codex = LlmConfig::new_cli(Provider::CodexCli, "auto", None).unwrap();
        let codex_args = chat_args(&codex).unwrap();
        assert!(codex_args
            .windows(2)
            .any(|args| args == ["--sandbox", "read-only"]));
        assert!(codex_args.iter().any(|arg| arg == "--ephemeral"));
    }

    #[test]
    fn override_binary_must_match_bridge() {
        let config =
            LlmConfig::new_cli(Provider::CursorCli, "auto", Some("/tmp/codex".into())).unwrap();
        assert!(binary_for(&config).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn official_cli_transport_uses_status_and_stdin_chat() {
        let binary = mock_cli("agent");
        let config = LlmConfig::new_cli(
            Provider::CursorCli,
            "auto",
            Some(binary.to_string_lossy().into_owned()),
        )
        .unwrap();
        let status = subscription_status(&config).unwrap();
        assert!(status.authenticated);
        assert_eq!(status.account_hint.as_deref(), Some("user@example.com"));
        assert_eq!(
            chat(&config, "cite sources", "question").unwrap(),
            "Grounded mock answer"
        );
        std::fs::remove_dir_all(binary.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_timeout_kills_hung_bridge() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 5"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let error = run_command(command, None, Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("timeout"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_timeout_covers_blocking_large_stdin_write() {
        // Child never reads stdin; a large write fills the pipe and would block
        // forever if timeout only started after write_all returned.
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let input = "x".repeat(512 * 1024);
        let started = Instant::now();
        let error = run_command(command, Some(&input), Duration::from_millis(200)).unwrap_err();
        assert!(error.to_string().contains("timeout"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_nested_grandchildren_via_process_group() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sh -c 'sh -c \"sleep 30\"'"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let error = run_command(command, None, Duration::from_millis(200)).unwrap_err();
        assert!(error.to_string().contains("timeout"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn early_parent_exit_reaps_orphans_without_hanging_pipes() {
        // Parent exits immediately while a background grandchild keeps pipes open
        // unless the process group is reaped.
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & exit 0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let input = "x".repeat(256 * 1024);
        let started = Instant::now();
        let result = run_command(command, Some(&input), Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(result.is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_timeout_kills_nested_cmd_tree_via_job_object() {
        // Nested `cmd /C` grandchildren must be reaped by the Job Object.
        let mut command = Command::new("cmd");
        command
            .args(["/C", "cmd /C ping -n 30 127.0.0.1 >NUL"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let error = run_command(command, None, Duration::from_millis(300)).unwrap_err();
        assert!(error.to_string().contains("timeout"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
