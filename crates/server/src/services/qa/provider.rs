//! Optional OpenAI-compatible chat provider for grounded Q&A (GLM / local).
//!
//! Supports non-streaming `complete` and incremental `stream_tokens` with
//! cooperative cancel (P1B-R05). Extractive fallback may chunk locally; the
//! provider path must never precompute-then-tokenize a completed response.

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fileconv_knowledge::ask::AnswerMode;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::services::qa::prompt::GroundedMessages;
use crate::services::qa::stream::tokenize_answer;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ANSWER_CHARS: usize = 16_384;
const MAX_STREAM_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("chat provider is not configured")]
    NotConfigured,
    #[error("chat provider request failed")]
    Transport,
    #[error("chat provider returned an invalid response")]
    InvalidResponse,
    #[error("chat provider timed out")]
    Timeout,
    #[error("chat provider stream cancelled")]
    Cancelled,
}

/// Cooperative cancel flag shared by SSE producers and provider HTTP bodies.
#[derive(Clone, Default)]
pub struct StreamCancel {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for StreamCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl StreamCancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves when cancel is requested.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            self.notify.notified().await;
        }
    }
}

/// Process-local chat backend. Prefer extractive fallback when `None`.
#[derive(Clone)]
pub enum ChatProvider {
    OpenAi(OpenAiCompatibleChat),
    Static(StaticChatProvider),
    /// Emits scripted tokens incrementally (true streaming test double).
    StreamingStatic(StreamingStaticProvider),
    /// Deterministic transport failure for ask outage paths.
    Failing,
    /// Deterministic timeout for ask timeout fallback paths.
    Timeout,
}

impl ChatProvider {
    pub async fn complete(&self, messages: &GroundedMessages) -> Result<String, ProviderError> {
        match self {
            Self::OpenAi(provider) => provider.complete(messages).await,
            Self::Static(provider) => Ok(provider.answer.clone()),
            Self::StreamingStatic(provider) => Ok(provider.tokens.join("")),
            Self::Failing => Err(ProviderError::Transport),
            Self::Timeout => Err(ProviderError::Timeout),
        }
    }

    /// Incremental token stream. Cancel drops the upstream HTTP body/reader.
    pub async fn stream_tokens(
        &self,
        messages: &GroundedMessages,
        cancel: StreamCancel,
    ) -> Result<mpsc::Receiver<Result<String, ProviderError>>, ProviderError> {
        match self {
            Self::OpenAi(provider) => provider.stream_tokens(messages, cancel).await,
            Self::Static(provider) => {
                Ok(spawn_chunk_stream(tokenize_answer(&provider.answer), cancel).await)
            }
            Self::StreamingStatic(provider) => {
                Ok(spawn_chunk_stream(provider.tokens.clone(), cancel).await)
            }
            Self::Failing => Err(ProviderError::Transport),
            Self::Timeout => Err(ProviderError::Timeout),
        }
    }

    pub fn answer_mode(&self) -> AnswerMode {
        match self {
            Self::OpenAi(provider) => provider.mode,
            Self::Static(provider) => provider.mode,
            Self::StreamingStatic(provider) => provider.mode,
            Self::Failing | Self::Timeout => AnswerMode::FallbackExtractive,
        }
    }

    /// True when this backend supports incremental provider streaming (not snapshot tokenize).
    pub fn supports_incremental_stream(&self) -> bool {
        matches!(self, Self::OpenAi(_) | Self::StreamingStatic(_))
    }
}

impl std::fmt::Debug for ChatProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenAi(provider) => provider.fmt(formatter),
            Self::Static(provider) => formatter
                .debug_struct("StaticChatProvider")
                .field("mode", &provider.mode)
                .field("answer", &"[REDACTED_ANSWER]")
                .finish(),
            Self::StreamingStatic(provider) => formatter
                .debug_struct("StreamingStaticProvider")
                .field("mode", &provider.mode)
                .field("token_count", &provider.tokens.len())
                .finish(),
            Self::Failing => formatter.write_str("FailingChatProvider"),
            Self::Timeout => formatter.write_str("TimeoutChatProvider"),
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleChat {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    mode: AnswerMode,
}

impl std::fmt::Debug for OpenAiCompatibleChat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleChat")
            .field("endpoint", &"[REDACTED_ENDPOINT]")
            .field("model", &self.model)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleChat {
    pub fn from_env() -> Result<Self, ProviderError> {
        let base = env::var("MARKHAND_GLM_BASE_URL")
            .or_else(|_| env::var("MARKHAND_CHAT_BASE_URL"))
            .map_err(|_| ProviderError::NotConfigured)?;
        let api_key = env::var("MARKHAND_GLM_API_KEY")
            .or_else(|_| env::var("MARKHAND_CHAT_API_KEY"))
            .unwrap_or_default();
        let model = env::var("MARKHAND_GLM_MODEL")
            .or_else(|_| env::var("MARKHAND_CHAT_MODEL"))
            .unwrap_or_else(|_| "glm-4-flash".into());
        let mode = if base.contains("127.0.0.1") || base.contains("localhost") {
            AnswerMode::LocalLlm
        } else {
            AnswerMode::CloudLlm
        };
        Self::new(base, api_key, model, mode)
    }

    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        mode: AnswerMode,
    ) -> Result<Self, ProviderError> {
        let base = base_url.trim_end_matches('/').to_string();
        if base.is_empty() || model.trim().is_empty() {
            return Err(ProviderError::NotConfigured);
        }
        let endpoint = if base.ends_with("/chat/completions") {
            base
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|_| ProviderError::Transport)?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model,
            mode,
        })
    }

    async fn complete(&self, messages: &GroundedMessages) -> Result<String, ProviderError> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: &messages.system,
                },
                ChatMessage {
                    role: "user",
                    content: &messages.user,
                },
            ],
            temperature: 0.1,
            stream: false,
        };
        let mut request = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = match tokio::time::timeout(DEFAULT_TIMEOUT, request.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(ProviderError::Transport),
            Err(_) => return Err(ProviderError::Timeout),
        };
        if !response.status().is_success() {
            return Err(ProviderError::Transport);
        }
        let parsed: ChatResponse = response
            .json()
            .await
            .map_err(|_| ProviderError::InvalidResponse)?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or(ProviderError::InvalidResponse)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(ProviderError::InvalidResponse);
        }
        if trimmed.chars().count() > MAX_ANSWER_CHARS {
            return Ok(trimmed.chars().take(MAX_ANSWER_CHARS).collect());
        }
        Ok(trimmed.to_string())
    }

    async fn stream_tokens(
        &self,
        messages: &GroundedMessages,
        cancel: StreamCancel,
    ) -> Result<mpsc::Receiver<Result<String, ProviderError>>, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: &messages.system,
                },
                ChatMessage {
                    role: "user",
                    content: &messages.user,
                },
            ],
            temperature: 0.1,
            stream: true,
        };
        let mut request = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = match tokio::time::timeout(DEFAULT_TIMEOUT, request.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err(ProviderError::Transport),
            Err(_) => return Err(ProviderError::Timeout),
        };
        if !response.status().is_success() {
            return Err(ProviderError::Transport);
        }
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut framing = SseByteBuffer::new(MAX_STREAM_BYTES);
            let mut total_chars = 0usize;
            let overall = tokio::time::sleep(DEFAULT_TIMEOUT);
            tokio::pin!(overall);
            loop {
                if cancel.is_cancelled() {
                    let _ = tx.send(Err(ProviderError::Cancelled)).await;
                    return;
                }
                let next = tokio::select! {
                    _ = &mut overall => {
                        let _ = tx.send(Err(ProviderError::Timeout)).await;
                        return;
                    }
                    _ = cancel.cancelled() => {
                        let _ = tx.send(Err(ProviderError::Cancelled)).await;
                        return;
                    }
                    chunk = tokio::time::timeout(BODY_IDLE_TIMEOUT, byte_stream.next()) => {
                        match chunk {
                            Ok(Some(Ok(bytes))) => bytes,
                            Ok(Some(Err(_))) => {
                                let _ = tx.send(Err(ProviderError::Transport)).await;
                                return;
                            }
                            Ok(None) => {
                                // EOF without prior [DONE]/finish_reason is invalid.
                                let _ = tx.send(Err(ProviderError::InvalidResponse)).await;
                                return;
                            }
                            Err(_) => {
                                let _ = tx.send(Err(ProviderError::Timeout)).await;
                                return;
                            }
                        }
                    }
                };
                let frames = match framing.push(&next) {
                    Ok(frames) => frames,
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                };
                for frame in frames {
                    match frame {
                        SseFrame::Done | SseFrame::Finished => {
                            return;
                        }
                        SseFrame::Data(data) => match parse_stream_delta(&data) {
                            Ok(StreamDeltaParse::Text(text)) => {
                                if text.is_empty() {
                                    continue;
                                }
                                total_chars = total_chars.saturating_add(text.chars().count());
                                if total_chars > MAX_ANSWER_CHARS {
                                    let _ = tx.send(Err(ProviderError::InvalidResponse)).await;
                                    return;
                                }
                                if tx.send(Ok(text)).await.is_err() {
                                    return;
                                }
                            }
                            Ok(StreamDeltaParse::TextAndFinished(text)) => {
                                if !text.is_empty() {
                                    total_chars = total_chars.saturating_add(text.chars().count());
                                    if total_chars > MAX_ANSWER_CHARS {
                                        let _ = tx.send(Err(ProviderError::InvalidResponse)).await;
                                        return;
                                    }
                                    if tx.send(Ok(text)).await.is_err() {
                                        return;
                                    }
                                }
                                return;
                            }
                            Ok(StreamDeltaParse::Finished) => {
                                return;
                            }
                            Ok(StreamDeltaParse::Ignore) => {}
                            Err(error) => {
                                let _ = tx.send(Err(error)).await;
                                return;
                            }
                        },
                    }
                }
            }
        });
        Ok(rx)
    }
}

/// Incremental UTF-8 / SSE framing buffer for provider streams.
#[derive(Debug, Default)]
pub struct SseByteBuffer {
    raw: Vec<u8>,
    max_bytes: usize,
    pending_data: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseFrame {
    Data(String),
    Done,
    Finished,
}

impl SseByteBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            raw: Vec::new(),
            max_bytes,
            pending_data: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, ProviderError> {
        if self.raw.len().saturating_add(chunk.len()) > self.max_bytes {
            return Err(ProviderError::InvalidResponse);
        }
        self.raw.extend_from_slice(chunk);
        let mut out = Vec::new();
        loop {
            let Some(newline) = self.raw.iter().position(|b| *b == b'\n') else {
                // Keep only a complete UTF-8 prefix in the hold-back window.
                match std::str::from_utf8(&self.raw) {
                    Ok(_) => break,
                    Err(error) if error.error_len().is_some() => {
                        return Err(ProviderError::InvalidResponse);
                    }
                    Err(error) => {
                        let valid = error.valid_up_to();
                        if valid == 0 {
                            break;
                        }
                        // Incomplete trailing sequence — wait for more bytes.
                        break;
                    }
                }
            };
            let mut line_bytes = self.raw.drain(..=newline).collect::<Vec<_>>();
            if line_bytes.last() == Some(&b'\n') {
                line_bytes.pop();
            }
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line =
                std::str::from_utf8(&line_bytes).map_err(|_| ProviderError::InvalidResponse)?;
            if line.is_empty() {
                // Blank line ends an SSE event: join all accumulated data lines.
                if !self.pending_data.is_empty() {
                    let pending = std::mem::take(&mut self.pending_data);
                    if pending.iter().any(|part| part.trim() == "[DONE]") {
                        let content: Vec<String> = pending
                            .into_iter()
                            .filter(|part| part.trim() != "[DONE]")
                            .collect();
                        if !content.is_empty() {
                            out.push(SseFrame::Data(content.join("\n")));
                        }
                        out.push(SseFrame::Done);
                        return Ok(out);
                    }
                    out.push(SseFrame::Data(pending.join("\n")));
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                // Accumulate every data: line until the blank-line delimiter.
                self.pending_data.push(rest.trim_start().to_string());
            }
        }
        Ok(out)
    }
}

enum StreamDeltaParse {
    Text(String),
    TextAndFinished(String),
    Finished,
    Ignore,
}

fn parse_stream_delta(data: &str) -> Result<StreamDeltaParse, ProviderError> {
    let parsed: StreamChatChunk =
        serde_json::from_str(data).map_err(|_| ProviderError::InvalidResponse)?;
    let choice = parsed.choices.into_iter().next();
    let Some(choice) = choice else {
        return Ok(StreamDeltaParse::Ignore);
    };
    let finished = choice
        .finish_reason
        .as_deref()
        .is_some_and(|reason| !reason.is_empty());
    match (
        choice.delta.content.filter(|value| !value.is_empty()),
        finished,
    ) {
        (Some(text), true) => Ok(StreamDeltaParse::TextAndFinished(text)),
        (Some(text), false) => Ok(StreamDeltaParse::Text(text)),
        (None, true) => Ok(StreamDeltaParse::Finished),
        (None, false) => Ok(StreamDeltaParse::Ignore),
    }
}

async fn spawn_chunk_stream(
    tokens: Vec<String>,
    cancel: StreamCancel,
) -> mpsc::Receiver<Result<String, ProviderError>> {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        for token in tokens {
            if cancel.is_cancelled() {
                let _ = tx.send(Err(ProviderError::Cancelled)).await;
                return;
            }
            if tx.send(Ok(token)).await.is_err() {
                return;
            }
            tokio::task::yield_now().await;
        }
    });
    rx
}

#[derive(Clone)]
pub struct StaticChatProvider {
    answer: String,
    mode: AnswerMode,
}

impl StaticChatProvider {
    pub fn new(answer: impl Into<String>, mode: AnswerMode) -> Self {
        Self {
            answer: answer.into(),
            mode,
        }
    }
}

/// Test/hermetic provider that yields discrete tokens without precompute snapshot tricks.
#[derive(Clone)]
pub struct StreamingStaticProvider {
    tokens: Vec<String>,
    mode: AnswerMode,
}

impl StreamingStaticProvider {
    pub fn new(tokens: Vec<String>, mode: AnswerMode) -> Self {
        Self { tokens, mode }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamChatChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_endpoint_and_key() {
        let provider = OpenAiCompatibleChat::new(
            "https://example.invalid/v1".into(),
            "super-secret-key".into(),
            "glm-4-flash".into(),
            AnswerMode::CloudLlm,
        )
        .unwrap();
        let debug = format!("{provider:?}");
        assert!(!debug.contains("example.invalid"));
        assert!(!debug.contains("super-secret-key"));
        assert!(debug.contains("[REDACTED_ENDPOINT]"));
    }

    #[tokio::test]
    async fn streaming_static_emits_incremental_tokens_and_honors_cancel() {
        let provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
            vec!["Một ".into(), "hai ".into(), "ba".into()],
            AnswerMode::LocalLlm,
        ));
        assert!(provider.supports_incremental_stream());
        let cancel = StreamCancel::new();
        let mut rx = provider
            .stream_tokens(
                &GroundedMessages {
                    system: "s".into(),
                    user: "u".into(),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        let first = rx.recv().await.unwrap().unwrap();
        assert_eq!(first, "Một ");
        cancel.cancel();
        // Remaining items may be Cancelled or absent if the producer exits promptly.
        while let Some(item) = rx.recv().await {
            match item {
                Err(ProviderError::Cancelled) => return,
                Ok(_) => continue,
                Err(other) => panic!("unexpected error {other}"),
            }
        }
    }

    #[test]
    fn parse_stream_delta_reads_content_and_finish() {
        let raw = r#"{"choices":[{"delta":{"content":"xin"}}]}"#;
        assert!(matches!(
            parse_stream_delta(raw).unwrap(),
            StreamDeltaParse::Text(text) if text == "xin"
        ));
        assert!(matches!(
            parse_stream_delta(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#).unwrap(),
            StreamDeltaParse::Finished
        ));
        assert!(matches!(
            parse_stream_delta(
                r#"{"choices":[{"delta":{"content":"end"},"finish_reason":"stop"}]}"#
            )
            .unwrap(),
            StreamDeltaParse::TextAndFinished(text) if text == "end"
        ));
    }

    #[test]
    fn sse_byte_buffer_handles_utf8_crlf_multiline_done_truncate() {
        let mut buf = SseByteBuffer::new(4096);
        // Split multi-byte UTF-8 across chunks ("à" = c3 a0); CRLF; accumulate until blank.
        let part1 = b"data: {\"choices\":[{\"delta\":{\"content\":\"ch\xc3";
        let part2 = b"\xa0o\"}}]}\r\n\n";
        assert!(buf.push(part1).unwrap().is_empty());
        let frames2 = buf.push(part2).unwrap();
        assert_eq!(frames2.len(), 1);
        assert!(matches!(
            &frames2[0],
            SseFrame::Data(d) if d.contains("chào")
        ));

        // Two data lines + blank → one joined frame (proper SSE multiline).
        let mut multi = SseByteBuffer::new(4096);
        let joined = multi.push(b"data: line-a\ndata: line-b\n\n").unwrap();
        assert_eq!(joined, vec![SseFrame::Data("line-a\nline-b".into())]);

        // Incomplete event (no blank) must not emit; [DONE] is its own frame.
        let mut pending = SseByteBuffer::new(4096);
        assert!(pending.push(b"data: waiting\n").unwrap().is_empty());
        let waiting = pending.push(b"\n").unwrap();
        assert_eq!(waiting, vec![SseFrame::Data("waiting".into())]);
        let done = pending.push(b"data: [DONE]\n\n").unwrap();
        assert_eq!(done, vec![SseFrame::Done]);

        let mut tiny = SseByteBuffer::new(8);
        assert!(matches!(
            tiny.push(b"0123456789"),
            Err(ProviderError::InvalidResponse)
        ));
        let mut bad = SseByteBuffer::new(64);
        assert!(matches!(
            bad.push(&[0x80, b'\n']),
            Err(ProviderError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn fake_http_stream_split_stall_cancel_and_require_done() {
        use axum::body::Body;
        use axum::response::IntoResponse;
        use axum::routing::post;
        use axum::Router;
        use bytes::Bytes;
        use futures::stream;
        use std::net::SocketAddr;
        use tokio::net::TcpListener;

        async fn handler() -> impl IntoResponse {
            let chunks = vec![
                Ok::<_, std::io::Error>(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"Xin \"}}]}\n\n",
                )),
                Ok(Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"ch\xc3\xa0o\"}}]}\n\n",
                )),
                Ok(Bytes::from_static(b"data: [DONE]\n\n")),
            ];
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(stream::iter(chunks)),
            )
        }

        let app = Router::new().route("/v1/chat/completions", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = OpenAiCompatibleChat::new(
            format!("http://{addr}"),
            String::new(),
            "test".into(),
            AnswerMode::LocalLlm,
        )
        .unwrap();
        let cancel = StreamCancel::new();
        let mut rx = provider
            .stream_tokens(
                &GroundedMessages {
                    system: "s".into(),
                    user: "u".into(),
                },
                cancel,
            )
            .await
            .unwrap();
        let mut out = String::new();
        while let Some(item) = rx.recv().await {
            out.push_str(&item.unwrap());
        }
        assert!(out.contains("Xin "));
        assert!(out.contains("chào"));

        // Truncated stream without DONE → InvalidResponse.
        async fn truncated() -> impl IntoResponse {
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n"),
            )
        }
        let app = Router::new().route("/v1/chat/completions", post(truncated));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = OpenAiCompatibleChat::new(
            format!("http://{addr}"),
            String::new(),
            "test".into(),
            AnswerMode::LocalLlm,
        )
        .unwrap();
        let mut rx = provider
            .stream_tokens(
                &GroundedMessages {
                    system: "s".into(),
                    user: "u".into(),
                },
                StreamCancel::new(),
            )
            .await
            .unwrap();
        let mut saw_invalid = false;
        while let Some(item) = rx.recv().await {
            match item {
                Ok(_) => {}
                Err(ProviderError::InvalidResponse) => saw_invalid = true,
                Err(other) => panic!("unexpected {other}"),
            }
        }
        assert!(saw_invalid, "truncated stream must require DONE");

        // Cancel mid-stream.
        async fn slow() -> impl IntoResponse {
            let chunks = stream::unfold(0u8, |n| async move {
                if n > 20 {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                Some((
                    Ok::<_, std::io::Error>(Bytes::from(format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{n}\"}}}}]}}\n\n"
                    ))),
                    n + 1,
                ))
            });
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(chunks),
            )
        }
        let app = Router::new().route("/v1/chat/completions", post(slow));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = OpenAiCompatibleChat::new(
            format!("http://{addr}"),
            String::new(),
            "test".into(),
            AnswerMode::LocalLlm,
        )
        .unwrap();
        let cancel = StreamCancel::new();
        let mut rx = provider
            .stream_tokens(
                &GroundedMessages {
                    system: "s".into(),
                    user: "u".into(),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        let _ = rx.recv().await;
        cancel.cancel();
        let mut saw_cancel = false;
        while let Some(item) = rx.recv().await {
            if matches!(item, Err(ProviderError::Cancelled)) {
                saw_cancel = true;
                break;
            }
        }
        assert!(saw_cancel);
    }
}
