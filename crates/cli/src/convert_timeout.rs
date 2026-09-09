//! Wall-clock timeout for `fileconv one` / `one-detailed`.
//!
//! macOS often lacks GNU `timeout`; this wraps convert in a worker thread and
//! waits with `mpsc::recv_timeout` (same idea as `fileconv_core::llm_cli`).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use fileconv_core::{
    ConversionReport, ConversionResult, ConvertError, Converter, DetailedConvertError,
};

pub const TIMEOUT_ENV: &str = "FILECONV_CONVERT_TIMEOUT_SEC";

/// Parse `30`, `30s`, `30sec`, or `30seconds` into a positive second count.
pub fn parse_timeout_secs(raw: &str) -> Result<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("--timeout / {TIMEOUT_ENV} rỗng — dùng số giây > 0 (vd: 30 hoặc 30s)");
    }
    let lower = trimmed.to_ascii_lowercase();
    let digits = lower
        .strip_suffix("seconds")
        .or_else(|| lower.strip_suffix("secs"))
        .or_else(|| lower.strip_suffix("sec"))
        .or_else(|| lower.strip_suffix('s'))
        .unwrap_or(lower.as_str())
        .trim();
    let secs: u64 = digits
        .parse()
        .with_context(|| format!("giá trị timeout không hợp lệ: {raw:?}"))?;
    if secs == 0 {
        bail!("timeout phải > 0 giây (nhận được 0)");
    }
    Ok(secs)
}

/// Flag `--timeout` wins over `{TIMEOUT_ENV}`; neither set → no timeout.
pub fn resolve_timeout_from_args(rest: &[String]) -> Result<Option<Duration>> {
    if let Some(index) = rest.iter().position(|argument| argument == "--timeout") {
        let raw = rest
            .get(index + 1)
            .context("thiếu giá trị sau --timeout (vd: --timeout 30s)")?;
        return Ok(Some(Duration::from_secs(parse_timeout_secs(raw)?)));
    }
    match std::env::var(TIMEOUT_ENV) {
        Ok(value) if !value.trim().is_empty() => {
            Ok(Some(Duration::from_secs(parse_timeout_secs(&value)?)))
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).context(format!("đọc {TIMEOUT_ENV}"))?,
    }
}

fn timeout_message(timeout: Duration) -> String {
    format!("convert timeout sau {}s", timeout.as_secs().max(1))
}

/// Run `work` on a helper thread; abandon the join if `timeout` elapses.
pub fn run_with_timeout<T, E, F>(timeout: Duration, work: F) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static + FromTimeout,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(timeout_error(timeout)),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(E::from_timeout("convert worker ended unexpectedly".into()))
        }
    }
}

fn timeout_error<E>(timeout: Duration) -> E
where
    E: FromTimeout,
{
    E::from_timeout(timeout_message(timeout))
}

pub trait FromTimeout {
    fn from_timeout(message: String) -> Self;
}

impl FromTimeout for ConvertError {
    fn from_timeout(message: String) -> Self {
        ConvertError::Failed(message)
    }
}

impl FromTimeout for DetailedConvertError {
    fn from_timeout(message: String) -> Self {
        DetailedConvertError::failed(message)
    }
}

pub fn convert_path_with_timeout(
    conv: Converter,
    path: &Path,
    timeout: Duration,
) -> Result<ConversionResult, ConvertError> {
    let path = PathBuf::from(path);
    run_with_timeout(timeout, move || conv.convert_path(&path))
}

pub fn convert_path_detailed_with_timeout(
    conv: Converter,
    path: &Path,
    timeout: Duration,
) -> Result<ConversionReport, DetailedConvertError> {
    let path = PathBuf::from(path);
    run_with_timeout(timeout, move || conv.convert_path_detailed(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn parse_timeout_secs_accepts_plain_and_suffix() {
        assert_eq!(parse_timeout_secs("30").unwrap(), 30);
        assert_eq!(parse_timeout_secs("30s").unwrap(), 30);
        assert_eq!(parse_timeout_secs("30sec").unwrap(), 30);
        assert_eq!(parse_timeout_secs(" 45Seconds ").unwrap(), 45);
    }

    #[test]
    fn parse_timeout_secs_rejects_zero_and_garbage() {
        assert!(parse_timeout_secs("0").is_err());
        assert!(parse_timeout_secs("abc").is_err());
        assert!(parse_timeout_secs("").is_err());
    }

    #[test]
    fn resolve_timeout_prefers_flag_over_env() {
        let rest = vec!["--timeout".into(), "12s".into()];
        // Even if env is set, flag wins (set env then restore).
        let previous = std::env::var_os(TIMEOUT_ENV);
        std::env::set_var(TIMEOUT_ENV, "99");
        let got = resolve_timeout_from_args(&rest).unwrap();
        match previous {
            Some(value) => std::env::set_var(TIMEOUT_ENV, value),
            None => std::env::remove_var(TIMEOUT_ENV),
        }
        assert_eq!(got, Some(Duration::from_secs(12)));
    }

    #[test]
    fn run_with_timeout_breaks_slow_work() {
        let started = Instant::now();
        let result: Result<(), ConvertError> = run_with_timeout(Duration::from_millis(50), || {
            thread::sleep(Duration::from_millis(500));
            Ok(())
        });
        let elapsed = started.elapsed();
        let err = result.expect_err("slow work must time out");
        let message = err.to_string();
        assert!(
            message.contains("timeout"),
            "expected timeout message, got {message}"
        );
        assert!(
            elapsed < Duration::from_millis(400),
            "wall clock should return near budget, got {elapsed:?}"
        );
    }

    #[test]
    fn run_with_timeout_allows_fast_work() {
        let result: Result<&'static str, ConvertError> =
            run_with_timeout(Duration::from_secs(2), || {
                thread::sleep(Duration::from_millis(10));
                Ok("ok")
            });
        assert_eq!(result.unwrap(), "ok");
    }

    #[test]
    fn detailed_timeout_uses_failed_kind() {
        let err: DetailedConvertError = run_with_timeout(Duration::from_millis(40), || {
            thread::sleep(Duration::from_millis(400));
            Ok::<(), DetailedConvertError>(())
        })
        .expect_err("must time out");
        let dto = err.to_dto();
        assert_eq!(dto.kind.as_str(), "failed");
        assert!(dto.message.contains("timeout"));
    }
}
