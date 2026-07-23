//! Optional OpenAI-compatible chat provider for grounded Q&A (GLM / local).

use std::env;
use std::time::Duration;

use fileconv_knowledge::ask::AnswerMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::services::qa::prompt::GroundedMessages;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ANSWER_CHARS: usize = 16_384;

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
}

/// Process-local chat backend. Prefer extractive fallback when `None`.
#[derive(Clone)]
pub enum ChatProvider {
    OpenAi(OpenAiCompatibleChat),
    Static(StaticChatProvider),
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
            Self::Failing => Err(ProviderError::Transport),
            Self::Timeout => Err(ProviderError::Timeout),
        }
    }

    pub fn answer_mode(&self) -> AnswerMode {
        match self {
            Self::OpenAi(provider) => provider.mode,
            Self::Static(provider) => provider.mode,
            Self::Failing | Self::Timeout => AnswerMode::FallbackExtractive,
        }
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
}
