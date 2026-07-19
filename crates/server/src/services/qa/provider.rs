//! Server-owned OpenAI-compatible streaming chat client for grounded Q&A.

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use serde_json::Value;
use thiserror::Error;
use tokio::time::{timeout_at, Instant};

use crate::config::SecretString;

const DEFAULT_MODEL: &str = "gpt-4o";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_SSE_LINE_BYTES: usize = 64 * 1024;
pub const MAX_DECODED_ANSWER_BYTES: usize = 256 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct LlmChatConfig {
    provider: LlmProvider,
    model: String,
    base_url: String,
    api_key: SecretString,
}

impl fmt::Debug for LlmChatConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmChatConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmProvider {
    OpenAi,
    OpenAiCompatible,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LlmError {
    #[error("LLM provider request timed out")]
    Timeout,
    #[error("LLM provider transport error")]
    Transport,
    #[error("LLM provider returned HTTP {0}")]
    HttpStatus(u16),
    #[error("LLM provider returned an invalid streaming response")]
    InvalidResponse,
    #[error("LLM provider streaming response exceeded size limits")]
    ResponseTooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedSseLine {
    Delta(String),
    Done,
    Ignore,
}

impl LlmChatConfig {
    pub fn from_env() -> Option<Self> {
        let provider_name =
            std::env::var("FILECONV_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let provider = match provider_name.trim().to_ascii_lowercase().as_str() {
            "openai" => LlmProvider::OpenAi,
            "openai-compatible" => LlmProvider::OpenAiCompatible,
            _ => return None,
        };
        let api_key = std::env::var("FILECONV_LLM_API_KEY").ok()?;
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return None;
        }
        let model = env_or_default("FILECONV_LLM_MODEL", DEFAULT_MODEL);
        let base_url = env_or_default("FILECONV_LLM_BASE_URL", DEFAULT_BASE_URL);
        Some(Self {
            provider,
            model,
            base_url,
            api_key: SecretString::new(api_key),
        })
    }
}

fn env_or_default(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub async fn stream_chat(
    cfg: &LlmChatConfig,
    system: &str,
    user: &str,
) -> Result<BoxStream<'static, Result<String, LlmError>>, LlmError> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|_| LlmError::Transport)?;
    let endpoint = chat_completions_endpoint(&cfg.base_url);
    let request = client
        .post(endpoint)
        .bearer_auth(cfg.api_key.expose())
        .json(&serde_json::json!({
            "model": cfg.model,
            "stream": true,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        }));
    let response = timeout_at(deadline, request.send())
        .await
        .map_err(|_| LlmError::Timeout)?
        .map_err(|_| LlmError::Transport)?;
    if !response.status().is_success() {
        return Err(LlmError::HttpStatus(response.status().as_u16()));
    }
    Ok(sse_delta_stream(response.bytes_stream().boxed(), deadline).boxed())
}

fn chat_completions_endpoint(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim().trim_end_matches('/'))
}

fn sse_delta_stream(
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    deadline: Instant,
) -> impl futures::Stream<Item = Result<String, LlmError>> {
    futures::stream::unfold(
        SseState {
            inner,
            deadline,
            buffer: Vec::new(),
            pending: VecDeque::new(),
            decoded_answer_bytes: 0,
            finished: false,
        },
        |mut state| async move {
            loop {
                if let Some(item) = state.pending.pop_front() {
                    return Some((item, state));
                }
                if state.finished {
                    return None;
                }
                match timeout_at(state.deadline, state.inner.next()).await {
                    Ok(Some(Ok(bytes))) => {
                        state.push_bytes(&bytes);
                    }
                    Ok(Some(Err(_))) => {
                        state.finished = true;
                        return Some((Err(LlmError::Transport), state));
                    }
                    Ok(None) => {
                        state.finish_buffer();
                    }
                    Err(_) => {
                        state.finished = true;
                        return Some((Err(LlmError::Timeout), state));
                    }
                }
            }
        },
    )
}

struct SseState {
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    deadline: Instant,
    buffer: Vec<u8>,
    pending: VecDeque<Result<String, LlmError>>,
    decoded_answer_bytes: usize,
    finished: bool,
}

impl SseState {
    fn push_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_SSE_LINE_BYTES && !self.buffer.contains(&b'\n') {
            self.pending.push_back(Err(LlmError::ResponseTooLarge));
            self.finished = true;
            self.buffer.clear();
            return;
        }
        self.drain_complete_lines();
    }

    fn finish_buffer(&mut self) {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.process_line(line);
        }
        self.finished = true;
    }

    fn drain_complete_lines(&mut self) {
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=newline).collect::<Vec<_>>();
            self.process_line(line);
            if self.finished {
                self.buffer.clear();
                break;
            }
        }
    }

    fn process_line(&mut self, mut line: Vec<u8>) {
        if line.len() > MAX_SSE_LINE_BYTES {
            self.pending.push_back(Err(LlmError::ResponseTooLarge));
            self.finished = true;
            return;
        }
        if matches!(line.last(), Some(b'\n')) {
            line.pop();
        }
        if matches!(line.last(), Some(b'\r')) {
            line.pop();
        }
        let Ok(line) = std::str::from_utf8(&line) else {
            self.pending.push_back(Err(LlmError::InvalidResponse));
            self.finished = true;
            return;
        };
        match parse_sse_line(line) {
            Ok(ParsedSseLine::Delta(delta)) => {
                match self.decoded_answer_bytes.checked_add(delta.len()) {
                    Some(total) if total <= MAX_DECODED_ANSWER_BYTES => {
                        self.decoded_answer_bytes = total;
                        self.pending.push_back(Ok(delta));
                    }
                    _ => {
                        self.pending.push_back(Err(LlmError::ResponseTooLarge));
                        self.finished = true;
                    }
                }
            }
            Ok(ParsedSseLine::Done) => self.finished = true,
            Ok(ParsedSseLine::Ignore) => {}
            Err(error) => {
                self.pending.push_back(Err(error));
                self.finished = true;
            }
        }
    }
}

pub(crate) fn parse_sse_line(line: &str) -> Result<ParsedSseLine, LlmError> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(ParsedSseLine::Ignore);
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(ParsedSseLine::Ignore);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(ParsedSseLine::Done);
    }
    if data.is_empty() {
        return Ok(ParsedSseLine::Ignore);
    }
    let value: Value = serde_json::from_str(data).map_err(|_| LlmError::InvalidResponse)?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str);
    Ok(match content {
        Some(delta) if !delta.is_empty() => ParsedSseLine::Delta(delta.to_string()),
        _ => ParsedSseLine::Ignore,
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures::stream::{self, StreamExt};
    use tokio::time::{Duration, Instant};

    use super::{
        parse_sse_line, sse_delta_stream, LlmError, ParsedSseLine, MAX_DECODED_ANSWER_BYTES,
        MAX_SSE_LINE_BYTES,
    };

    #[test]
    fn parses_openai_sse_content_deltas_and_done() {
        let line = r#"data: {"choices":[{"delta":{"content":"xin chào"}}]}"#;
        assert_eq!(
            parse_sse_line(line).unwrap(),
            ParsedSseLine::Delta("xin chào".into())
        );
        assert_eq!(parse_sse_line("data: [DONE]").unwrap(), ParsedSseLine::Done);
        assert_eq!(
            parse_sse_line(": keepalive").unwrap(),
            ParsedSseLine::Ignore
        );
    }

    #[tokio::test]
    async fn rejects_oversized_sse_line() {
        let line = format!("data: {}\n", "x".repeat(MAX_SSE_LINE_BYTES));
        let inner = stream::iter([Ok(Bytes::from(line))]).boxed();
        let mut stream = sse_delta_stream(inner, Instant::now() + Duration::from_secs(5)).boxed();
        assert_eq!(stream.next().await, Some(Err(LlmError::ResponseTooLarge)));
    }

    #[tokio::test]
    async fn rejects_oversized_decoded_answer() {
        let delta = "a".repeat(32 * 1024);
        let lines = (0..((MAX_DECODED_ANSWER_BYTES / delta.len()) + 1))
            .map(|_| {
                Ok(Bytes::from(format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices":[{"delta":{"content":delta}}]})
                )))
            })
            .collect::<Vec<Result<Bytes, reqwest::Error>>>();
        let inner = stream::iter(lines).boxed();
        let mut stream = sse_delta_stream(inner, Instant::now() + Duration::from_secs(5)).boxed();
        let mut saw_cap = false;
        while let Some(item) = stream.next().await {
            if item == Err(LlmError::ResponseTooLarge) {
                saw_cap = true;
                break;
            }
        }
        assert!(saw_cap);
    }
}
