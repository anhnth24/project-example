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

fn run_command(
    mut command: Command,
    input: Option<&str>,
    timeout: Duration,
) -> Result<CliOutput, ConvertError> {
    let mut child = command
        .spawn()
        .map_err(|error| fail(format!("không khởi chạy được CLI: {error}")))?;
    let stdout_reader = child.stdout.take().map(read_pipe);
    let stderr_reader = child.stderr.take().map(read_pipe);
    if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| fail("không mở được stdin cho CLI"))?;
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| fail(format!("không gửi được prompt tới CLI: {error}")))?;
    }
    drop(child.stdin.take());

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!(
                    "subscription CLI timeout sau {} giây",
                    timeout.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!("không chờ được CLI: {error}")));
            }
        }
    };
    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    // Drain stderr without exposing OAuth URLs, tokens, or local paths to UI logs.
    let _ = stderr_reader.and_then(|reader| reader.join().ok());
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
}
