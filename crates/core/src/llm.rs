//! Lớp LLM TUỲ CHỌN (feature `llm`): HTTP API, local endpoint hoặc official
//! Cursor/Codex subscription CLI. Không có provider thì caller dùng fallback local.
//!
//! Cấu hình:
//!   FILECONV_LLM_PROVIDER = openai | anthropic | gemini | openai-compatible
//!   FILECONV_LLM_API_KEY  = <key>
//!   FILECONV_LLM_MODEL    = <model>            (tuỳ chọn, có mặc định)
//!   FILECONV_LLM_BASE_URL = <url>              (tuỳ chọn — ollama/openrouter/local)
//!
//! Lưu ý riêng tư: bật lớp này = gửi nội dung tới nhà cung cấp tương ứng.

use crate::ConvertError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenAi,
    Anthropic,
    Gemini,
    OpenAiCompatible,
    CursorCli,
    CodexCli,
}

const REDACTED_SECRET_PLACEHOLDER: &str = "[REDACTED]";

fn deserialize_api_key<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    if value == REDACTED_SECRET_PLACEHOLDER {
        return Err(serde::de::Error::custom(
            "api_key placeholder [REDACTED] is not a valid secret",
        ));
    }
    Ok(value)
}

/// Conservative endpoint redaction: keep scheme + host[:port] only.
/// Never surface userinfo, path, query, or fragment.
fn redact_endpoint(url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest)
    } else {
        return REDACTED_SECRET_PLACEHOLDER.to_string();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if host_port.is_empty() {
        return REDACTED_SECRET_PLACEHOLDER.to_string();
    }
    format!("{scheme}://{host_port}/[REDACTED]")
}

fn find_url_start(text: &str) -> Option<usize> {
    match (text.find("https://"), text.find("http://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Replace every `http(s)://…` span so path/query/fragment/userinfo cannot leak.
fn redact_urls_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(rel) = find_url_start(&text[cursor..]) {
        let start = cursor + rel;
        out.push_str(&text[cursor..start]);
        let rest = &text[start..];
        let end_rel = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '<' | '>' | '`'))
            .unwrap_or(rest.len());
        out.push_str(&redact_endpoint(&rest[..end_rel]));
        cursor = start + end_rel;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Runtime LLM config — Deserialize only (no Serialize): secrets must not leave
/// process memory via serde. Construct with builders / `from_env`.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub provider: Provider,
    #[serde(deserialize_with = "deserialize_api_key")]
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
    #[serde(default)]
    pub cli_binary: Option<String>,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("provider", &self.provider)
            .field("api_key", &REDACTED_SECRET_PLACEHOLDER)
            .field("model", &self.model)
            .field("base_url", &self.base_url.as_deref().map(redact_endpoint))
            .field("cli_binary", &self.cli_binary)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderPreset {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub base_url: Option<String>,
    pub default_model: String,
    pub models: Vec<String>,
    pub local: bool,
    pub requires_api_key: bool,
    pub subscription: bool,
    pub supports_vision: bool,
    pub supports_embeddings: bool,
    pub description: String,
}

// Re-export ADR 0006 runtime-path constants/helpers from the always-on module so
// `fileconv_core::llm::*` callers keep working without depending on knowledge.
pub use crate::embedding_runtime::{
    infer_embedding_runtime_path, ALLOWED_EMBEDDING_RUNTIME_PATHS,
    EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM, EMBEDDING_RUNTIME_LOCAL_HASH,
    EMBEDDING_RUNTIME_LOCAL_NEURAL, EMBEDDING_RUNTIME_PROVIDER_CLOUD, EMBEDDING_RUNTIME_VLLM_LOCAL,
};

/// Runtime embedding config — Deserialize only (no Serialize); see [`LlmConfig`].
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub provider: Provider,
    #[serde(deserialize_with = "deserialize_api_key")]
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
    pub dimensions: Option<usize>,
    /// Explicit runtime path for index signature (ADR 0006). Prefer preset values
    /// over host/model inference for known deployments (e.g. vLLM → `vllm-local`).
    pub runtime_path: String,
}

impl std::fmt::Debug for EmbeddingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingConfig")
            .field("provider", &self.provider)
            .field("api_key", &REDACTED_SECRET_PLACEHOLDER)
            .field("model", &self.model)
            .field("base_url", &self.base_url.as_deref().map(redact_endpoint))
            .field("dimensions", &self.dimensions)
            .field("runtime_path", &self.runtime_path)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProviderPreset {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub base_url: Option<String>,
    pub default_model: String,
    pub models: Vec<String>,
    pub local: bool,
    pub requires_api_key: bool,
    pub default_dimensions: Option<usize>,
    /// Index-signature runtime path for this preset (must not rely on URL cues).
    pub runtime_path: String,
    pub description: String,
}

impl Provider {
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "anthropic" | "claude" => Self::Anthropic,
            "gemini" | "google" => Self::Gemini,
            "cursor-cli" | "cursor-agent" | "cursor-subscription" => Self::CursorCli,
            "codex-cli" | "codex" | "chatgpt-subscription" => Self::CodexCli,
            "openai-compatible" | "compatible" | "ollama" | "lm-studio" | "lmstudio"
            | "llama.cpp" | "llamacpp" | "vllm" | "openrouter" | "groq" | "mistral"
            | "together" => Self::OpenAiCompatible,
            _ => Self::OpenAi,
        }
    }
}

impl LlmConfig {
    pub fn new(
        provider: Provider,
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
    ) -> Result<Self, ConvertError> {
        if matches!(provider, Provider::CursorCli | Provider::CodexCli) {
            return Err(ConvertError::Failed(
                "subscription CLI phải dùng LlmConfig::new_cli".into(),
            ));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ConvertError::Failed("model LLM không được để trống".into()));
        }
        if let Some(url) = base_url.as_deref() {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ConvertError::Failed(
                    "LLM base URL phải bắt đầu bằng http:// hoặc https://".into(),
                ));
            }
        }
        Ok(Self {
            provider,
            api_key: api_key.into(),
            model,
            base_url,
            cli_binary: None,
        })
    }

    pub fn new_cli(
        provider: Provider,
        model: impl Into<String>,
        cli_binary: Option<String>,
    ) -> Result<Self, ConvertError> {
        if !matches!(provider, Provider::CursorCli | Provider::CodexCli) {
            return Err(ConvertError::Failed(
                "provider không phải subscription CLI".into(),
            ));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ConvertError::Failed("model CLI không được để trống".into()));
        }
        Ok(Self {
            provider,
            api_key: String::new(),
            model,
            base_url: None,
            cli_binary: cli_binary.filter(|path| !path.trim().is_empty()),
        })
    }

    pub fn is_subscription_cli(&self) -> bool {
        matches!(self.provider, Provider::CursorCli | Provider::CodexCli)
    }

    /// Đọc cấu hình từ env. Localhost/OpenAI-compatible local không bắt buộc key.
    pub fn from_env() -> Option<Self> {
        let provider_name =
            std::env::var("FILECONV_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let provider = Provider::from_name(&provider_name);
        if matches!(provider, Provider::CursorCli | Provider::CodexCli) {
            let model = std::env::var("FILECONV_LLM_MODEL").unwrap_or_else(|_| "auto".into());
            let binary = std::env::var("FILECONV_LLM_CLI_BINARY").ok();
            return Self::new_cli(provider, model, binary).ok();
        }
        let api_key = std::env::var("FILECONV_LLM_API_KEY").unwrap_or_default();
        let base_url = std::env::var("FILECONV_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let local_alias = matches!(
            provider_name.to_lowercase().as_str(),
            "ollama" | "lm-studio" | "lmstudio" | "llama.cpp" | "llamacpp" | "vllm"
        );
        let local_url = base_url
            .as_deref()
            .is_some_and(|url| url.contains("localhost") || url.contains("127.0.0.1"));
        if api_key.trim().is_empty() && !local_alias && !local_url {
            return None;
        }
        let model = std::env::var("FILECONV_LLM_MODEL")
            .unwrap_or_else(|_| default_model_for_name(&provider_name, provider));
        Self::new(provider, api_key, model, base_url).ok()
    }
}

impl EmbeddingConfig {
    pub fn new(
        provider: Provider,
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        dimensions: Option<usize>,
        runtime_path: impl Into<String>,
    ) -> Result<Self, ConvertError> {
        if matches!(
            provider,
            Provider::Anthropic | Provider::CursorCli | Provider::CodexCli
        ) {
            return Err(ConvertError::Failed(
                "provider này không có embedding API được Markhand hỗ trợ".into(),
            ));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ConvertError::Failed(
                "model embedding không được để trống".into(),
            ));
        }
        if let Some(url) = base_url.as_deref() {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ConvertError::Failed(
                    "embedding base URL phải bắt đầu bằng http:// hoặc https://".into(),
                ));
            }
        }
        if dimensions.is_some_and(|value| !(32..=4096).contains(&value)) {
            return Err(ConvertError::Failed(
                "số chiều embedding phải nằm trong khoảng 32–4096".into(),
            ));
        }
        let runtime_path = runtime_path.into();
        if !crate::embedding_runtime::is_allowed_embedding_runtime_path(&runtime_path) {
            return Err(ConvertError::Failed(format!(
                "embedding runtime_path không được hỗ trợ: {runtime_path}"
            )));
        }
        Ok(Self {
            provider,
            api_key: api_key.into(),
            model,
            base_url,
            dimensions,
            runtime_path,
        })
    }
}

fn default_model_for_name(name: &str, provider: Provider) -> String {
    match name.trim().to_lowercase().as_str() {
        "ollama" => "qwen2.5:7b",
        "lm-studio" | "lmstudio" | "llama.cpp" | "llamacpp" | "vllm" => "local-model",
        "openrouter" => "openai/gpt-4o-mini",
        "groq" => "llama-3.1-8b-instant",
        "mistral" => "mistral-small-latest",
        "together" => "Qwen/Qwen2.5-72B-Instruct-Turbo",
        _ => match provider {
            Provider::OpenAi | Provider::OpenAiCompatible => "gpt-4o-mini",
            Provider::Anthropic => "claude-3-5-haiku-latest",
            Provider::Gemini => "gemini-2.0-flash",
            Provider::CursorCli | Provider::CodexCli => "auto",
        },
    }
    .to_string()
}

pub fn provider_presets() -> Vec<LlmProviderPreset> {
    let preset = |id: &str,
                  label: &str,
                  provider: Provider,
                  base_url: Option<&str>,
                  default_model: &str,
                  models: &[&str],
                  local: bool,
                  requires_api_key: bool,
                  description: &str| LlmProviderPreset {
        id: id.into(),
        label: label.into(),
        provider,
        base_url: base_url.map(str::to_string),
        default_model: default_model.into(),
        models: models.iter().map(|model| (*model).to_string()).collect(),
        local,
        requires_api_key,
        subscription: matches!(provider, Provider::CursorCli | Provider::CodexCli),
        supports_vision: !matches!(provider, Provider::CursorCli | Provider::CodexCli),
        supports_embeddings: !matches!(
            provider,
            Provider::Anthropic | Provider::CursorCli | Provider::CodexCli
        ),
        description: description.into(),
    };
    vec![
        preset(
            "ollama",
            "Ollama (Local)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:11434"),
            "qwen2.5:7b",
            &["qwen2.5:7b", "qwen2.5:14b", "llama3.1:8b", "gemma3:4b"],
            true,
            false,
            "Khuyến nghị mặc định: dữ liệu và model chạy hoàn toàn trên máy.",
        ),
        preset(
            "lm-studio",
            "LM Studio (Local)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:1234"),
            "local-model",
            &["local-model"],
            true,
            false,
            "Desktop local server, chọn model trong LM Studio.",
        ),
        preset(
            "llama.cpp",
            "llama.cpp Server (Local)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:8080"),
            "local-model",
            &["local-model"],
            true,
            false,
            "Nhẹ, phù hợp self-host CPU/GPU và GGUF.",
        ),
        preset(
            "vllm",
            "vLLM (Self-host)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:8000"),
            "local-model",
            &["local-model"],
            true,
            false,
            "Server OpenAI-compatible cho GPU nội bộ và nhiều người dùng.",
        ),
        preset(
            "local-vlm",
            "Local vision/VLM (Self-host)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:8080"),
            "local-model",
            &["local-model"],
            true,
            false,
            "Endpoint OpenAI-compatible cho VLM local; nhập tên model đã cài, không gửi ảnh ra cloud.",
        ),
        preset(
            "cursor-cli",
            "Cursor subscription",
            Provider::CursorCli,
            None,
            "auto",
            &["auto"],
            false,
            false,
            "Dùng official Cursor Agent CLI/ACP và quota subscription; Markhand không đọc token.",
        ),
        preset(
            "codex-cli",
            "ChatGPT / Codex subscription",
            Provider::CodexCli,
            None,
            "auto",
            &["auto"],
            false,
            false,
            "Dùng official Codex CLI login; chạy ephemeral trong sandbox read-only.",
        ),
        preset(
            "openai",
            "OpenAI",
            Provider::OpenAi,
            Some("https://api.openai.com"),
            "gpt-4o-mini",
            &["gpt-4o-mini", "gpt-4o"],
            false,
            true,
            "Cloud provider; chỉ top citation được gửi khi hỏi đáp.",
        ),
        preset(
            "anthropic",
            "Anthropic Claude",
            Provider::Anthropic,
            Some("https://api.anthropic.com"),
            "claude-3-5-haiku-latest",
            &["claude-3-5-haiku-latest", "claude-3-7-sonnet-latest"],
            false,
            true,
            "Cloud provider mạnh về đọc và tổng hợp tài liệu dài.",
        ),
        preset(
            "gemini",
            "Google Gemini",
            Provider::Gemini,
            Some("https://generativelanguage.googleapis.com"),
            "gemini-2.0-flash",
            &["gemini-2.0-flash", "gemini-2.5-flash"],
            false,
            true,
            "Cloud provider hỗ trợ text và vision.",
        ),
        preset(
            "openrouter",
            "OpenRouter",
            Provider::OpenAiCompatible,
            Some("https://openrouter.ai/api"),
            "openai/gpt-4o-mini",
            &["openai/gpt-4o-mini", "anthropic/claude-3.5-haiku"],
            false,
            true,
            "Một API truy cập nhiều model, OpenAI-compatible.",
        ),
        preset(
            "groq",
            "Groq",
            Provider::OpenAiCompatible,
            Some("https://api.groq.com/openai"),
            "llama-3.1-8b-instant",
            &["llama-3.1-8b-instant", "llama-3.3-70b-versatile"],
            false,
            true,
            "Cloud inference tốc độ cao, OpenAI-compatible.",
        ),
        preset(
            "mistral",
            "Mistral AI",
            Provider::OpenAiCompatible,
            Some("https://api.mistral.ai"),
            "mistral-small-latest",
            &["mistral-small-latest", "mistral-large-latest"],
            false,
            true,
            "Cloud provider OpenAI-compatible, mạnh về multilingual.",
        ),
        preset(
            "together",
            "Together AI",
            Provider::OpenAiCompatible,
            Some("https://api.together.xyz"),
            "Qwen/Qwen2.5-72B-Instruct-Turbo",
            &[
                "Qwen/Qwen2.5-72B-Instruct-Turbo",
                "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            ],
            false,
            true,
            "Cloud inference cho nhiều open model.",
        ),
        preset(
            "custom",
            "Custom OpenAI-compatible",
            Provider::OpenAiCompatible,
            None,
            "local-model",
            &["local-model"],
            false,
            false,
            "Dùng endpoint nội bộ, Together, gateway doanh nghiệp hoặc provider khác.",
        ),
    ]
}

pub fn embedding_provider_presets() -> Vec<EmbeddingProviderPreset> {
    let preset = |id: &str,
                  label: &str,
                  provider: Provider,
                  base_url: Option<&str>,
                  default_model: &str,
                  models: &[&str],
                  local: bool,
                  requires_api_key: bool,
                  default_dimensions: Option<usize>,
                  runtime_path: &str,
                  description: &str| EmbeddingProviderPreset {
        id: id.into(),
        label: label.into(),
        provider,
        base_url: base_url.map(str::to_string),
        default_model: default_model.into(),
        models: models.iter().map(|model| (*model).to_string()).collect(),
        local,
        requires_api_key,
        default_dimensions,
        runtime_path: runtime_path.into(),
        description: description.into(),
    };
    vec![
        preset(
            "ollama",
            "Ollama embeddings (Local)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:11434"),
            "nomic-embed-text",
            &["nomic-embed-text", "mxbai-embed-large", "bge-m3"],
            true,
            false,
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            "Neural embedding chạy local qua OpenAI-compatible /v1/embeddings.",
        ),
        preset(
            "lm-studio",
            "LM Studio embeddings (Local)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:1234"),
            "local-model",
            &["local-model"],
            true,
            false,
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            "Load embedding model trong LM Studio Local Server.",
        ),
        preset(
            "vllm",
            "vLLM embeddings (Self-host)",
            Provider::OpenAiCompatible,
            Some("http://127.0.0.1:8000"),
            "BAAI/bge-m3",
            &["BAAI/bge-m3", "intfloat/multilingual-e5-large"],
            true,
            false,
            None,
            EMBEDDING_RUNTIME_VLLM_LOCAL,
            "Embedding server nội bộ; phù hợp corpus lớn và GPU dùng chung.",
        ),
        preset(
            "glm",
            "GLM embeddings (Zhipu cloud)",
            Provider::OpenAiCompatible,
            Some("https://open.bigmodel.cn/api/paas/v4"),
            "embedding-3",
            &["embedding-3", "embedding-2"],
            false,
            true,
            Some(1024),
            EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
            "Interim cloud embedding cho POC/DEMO (ADR 0004); toàn bộ chunk được gửi khi build index. Target production vẫn là vLLM self-host.",
        ),
        preset(
            "openai",
            "OpenAI embeddings",
            Provider::OpenAi,
            Some("https://api.openai.com"),
            "text-embedding-3-small",
            &["text-embedding-3-small", "text-embedding-3-large"],
            false,
            true,
            Some(1536),
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            "Cloud embedding; toàn bộ chunk được gửi khi build index.",
        ),
        preset(
            "gemini",
            "Google Gemini embeddings",
            Provider::Gemini,
            Some("https://generativelanguage.googleapis.com"),
            "gemini-embedding-001",
            &["gemini-embedding-001"],
            false,
            true,
            Some(768),
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            "Cloud embedding tiếng Việt; gọi embedContent theo từng chunk.",
        ),
        preset(
            "custom",
            "Custom OpenAI-compatible embeddings",
            Provider::OpenAiCompatible,
            None,
            "embedding-model",
            &["embedding-model"],
            false,
            false,
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
            "Endpoint embedding nội bộ hoặc gateway doanh nghiệp.",
        ),
    ]
}

fn fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Failed(e.to_string())
}

/// Bounded HTTP retries for connect failures and explicit 429/503 responses.
const HTTP_MAX_ATTEMPTS: u32 = 3;
const HTTP_BASE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(200);
/// Max wait we will honor for a retry. Larger `Retry-After` values skip retry
/// (never sleep less than the provider requested).
const HTTP_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Redact endpoints and known API-key material from any surfaced error text.
fn redact_surfaced_error(text: &str, secret: Option<&str>) -> String {
    let mut out = redact_urls_in_text(text);
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        out = out.replace(secret, REDACTED_SECRET_PLACEHOLDER);
    }
    out
}

fn fail_redacted(message: impl std::fmt::Display, secret: Option<&str>) -> ConvertError {
    ConvertError::Failed(redact_surfaced_error(&message.to_string(), secret))
}

fn invalid_provider_response(
    provider: &str,
    value: &serde_json::Value,
    secret: Option<&str>,
) -> ConvertError {
    fail_redacted(format!("phản hồi {provider} không hợp lệ: {value}"), secret)
}

#[cfg(test)]
thread_local! {
    static TEST_SLEEP_LOG: std::cell::RefCell<Vec<std::time::Duration>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static TEST_NOW_OVERRIDE: std::cell::Cell<Option<std::time::SystemTime>> =
        const { std::cell::Cell::new(None) };
    static TEST_SKIP_REAL_SLEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn sleep_for(duration: std::time::Duration) {
    #[cfg(test)]
    {
        TEST_SLEEP_LOG.with(|log| log.borrow_mut().push(duration));
        if TEST_SKIP_REAL_SLEEP.with(|skip| skip.get()) {
            return;
        }
    }
    if !duration.is_zero() {
        std::thread::sleep(duration);
    }
}

fn current_time() -> std::time::SystemTime {
    #[cfg(test)]
    {
        if let Some(now) = TEST_NOW_OVERRIDE.with(|cell| cell.get()) {
            return now;
        }
    }
    std::time::SystemTime::now()
}

/// Parse `Retry-After` as delta-seconds or HTTP-date (IMF-fixdate / RFC850 / asctime).
/// Malformed values return `None` — never panic.
fn parse_retry_after_header(
    value: Option<&str>,
    now: std::time::SystemTime,
) -> Option<std::time::Duration> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(std::time::Duration::from_secs(seconds));
    }
    match httpdate::parse_http_date(value) {
        Ok(when) => Some(
            when.duration_since(now)
                .unwrap_or(std::time::Duration::ZERO),
        ),
        Err(_) => None,
    }
}

fn exponential_backoff(attempt: u32) -> std::time::Duration {
    let shift = attempt.min(4);
    let millis = HTTP_BASE_BACKOFF.as_millis() as u64;
    std::time::Duration::from_millis(millis.saturating_mul(1u64 << shift)).min(HTTP_RETRY_BUDGET)
}

/// Decide whether/how long to wait before another attempt.
/// `Some(duration)` honors the full wait (never shortened below `Retry-After`).
/// `None` means do not retry (including when `Retry-After` exceeds the budget).
fn plan_retry_wait(
    attempt: u32,
    retry_after: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    if attempt + 1 >= HTTP_MAX_ATTEMPTS {
        return None;
    }
    if let Some(after) = retry_after {
        if after > HTTP_RETRY_BUDGET {
            return None;
        }
        return Some(after);
    }
    Some(exponential_backoff(attempt))
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 503)
}

fn is_retryable_transport(error: &reqwest::Error) -> bool {
    // Generation POSTs are not idempotent: never retry ambiguous read/timeouts
    // after the request may have been consumed. Connect failures are safe.
    error.is_connect()
}

fn openai_chat_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

fn gemini_generate_url(base: &str, model: &str) -> String {
    format!(
        "{}/v1beta/models/{}:generateContent",
        base.trim_end_matches('/'),
        model
    )
}

fn gemini_embed_url(base: &str, model: &str) -> String {
    format!(
        "{}/v1beta/models/{}:embedContent",
        base.trim_end_matches('/'),
        model
    )
}

fn gemini_auth(
    request: reqwest::blocking::RequestBuilder,
    api_key: &str,
) -> reqwest::blocking::RequestBuilder {
    // Prefer header auth so transport/error Display never embeds the key in a URL.
    if api_key.trim().is_empty() {
        request
    } else {
        request.header("x-goog-api-key", api_key)
    }
}

/// Gọi 1 lượt chat (system + user) → trả text.
pub fn chat(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, ConvertError> {
    if cfg.is_subscription_cli() {
        return crate::llm_cli::chat(cfg, system, user);
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(fail)?;

    match cfg.provider {
        Provider::OpenAi | Provider::OpenAiCompatible => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".into());
            let url = openai_chat_url(&base);
            let body = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user}
                ]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |request| {
                if cfg.api_key.trim().is_empty() {
                    request
                } else {
                    request.bearer_auth(&cfg.api_key)
                }
            })?;
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("OpenAI", &v, secret))
        }
        Provider::Anthropic => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            let url = format!("{}/v1/messages", base.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": cfg.model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{"role": "user", "content": user}]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |r| {
                r.header("x-api-key", &cfg.api_key)
                    .header("anthropic-version", "2023-06-01")
            })?;
            v["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("Anthropic", &v, secret))
        }
        Provider::Gemini => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
            let url = gemini_generate_url(&base, &cfg.model);
            let body = serde_json::json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"parts": [{"text": user}]}]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |r| {
                gemini_auth(r, &cfg.api_key)
            })?;
            v["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("Gemini", &v, secret))
        }
        Provider::CursorCli | Provider::CodexCli => unreachable!("handled above"),
    }
}

fn post_json(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &serde_json::Value,
    secret: Option<&str>,
    decorate: impl Fn(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
) -> Result<serde_json::Value, ConvertError> {
    let mut last_error: Option<ConvertError> = None;
    for attempt in 0..HTTP_MAX_ATTEMPTS {
        let request = decorate(client.post(url).json(body));
        let response = match request.send() {
            Ok(response) => response,
            Err(error) => {
                let message = redact_surfaced_error(&error.to_string(), secret);
                if is_retryable_transport(&error) {
                    if let Some(wait) = plan_retry_wait(attempt, None) {
                        sleep_for(wait);
                        last_error = Some(ConvertError::Failed(message));
                        continue;
                    }
                }
                return Err(ConvertError::Failed(message));
            }
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after_header(Some(value), current_time()));
        let text = match response.text() {
            Ok(text) => text,
            Err(error) => return Err(fail_redacted(error, secret)),
        };
        if status.is_success() {
            return serde_json::from_str(&text).map_err(|error| fail_redacted(error, secret));
        }
        let safe_text = redact_surfaced_error(&text, secret);
        let error = ConvertError::Failed(format!("LLM HTTP {status}: {safe_text}"));
        if is_retryable_status(status) {
            if let Some(wait) = plan_retry_wait(attempt, retry_after) {
                sleep_for(wait);
                last_error = Some(error);
                continue;
            }
        }
        return Err(error);
    }
    Err(last_error.unwrap_or_else(|| fail("LLM HTTP retry exhausted")))
}

fn openai_embeddings_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/embeddings")
    } else {
        format!("{base}/v1/embeddings")
    }
}

fn normalize_embedding(vector: &mut [f32]) -> Result<(), ConvertError> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return Err(fail("provider trả embedding rỗng hoặc không hợp lệ"));
    }
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

fn validate_embedding_dimensions(
    cfg: &EmbeddingConfig,
    vectors: &[Vec<f32>],
) -> Result<(), ConvertError> {
    let Some(first) = vectors.first() else {
        return Ok(());
    };
    if first.is_empty() {
        return Err(fail("provider trả embedding rỗng"));
    }
    let dimensions = first.len();
    if vectors.iter().any(|vector| vector.len() != dimensions) {
        return Err(fail("provider trả các vector khác số chiều"));
    }
    if cfg
        .dimensions
        .is_some_and(|expected| expected != dimensions)
    {
        return Err(fail(format!(
            "provider trả {dimensions} chiều, khác cấu hình {}",
            cfg.dimensions.unwrap_or_default()
        )));
    }
    Ok(())
}

fn embed_openai_batch(
    client: &reqwest::blocking::Client,
    cfg: &EmbeddingConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, ConvertError> {
    let base = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com".into());
    let mut body = serde_json::json!({
        "model": cfg.model,
        "input": texts,
        "encoding_format": "float"
    });
    if let Some(dimensions) = cfg.dimensions {
        body["dimensions"] = serde_json::json!(dimensions);
    }
    let secret = Some(cfg.api_key.as_str());
    let response = post_json(
        client,
        &openai_embeddings_url(&base),
        &body,
        secret,
        |request| {
            if cfg.api_key.trim().is_empty() {
                request
            } else {
                request.bearer_auth(&cfg.api_key)
            }
        },
    )?;
    let data = response["data"]
        .as_array()
        .ok_or_else(|| invalid_provider_response("embedding", &response, secret))?;
    let mut indexed = Vec::with_capacity(data.len());
    for (fallback_index, item) in data.iter().enumerate() {
        let index = item["index"].as_u64().unwrap_or(fallback_index as u64) as usize;
        let values = item["embedding"]
            .as_array()
            .ok_or_else(|| fail("embedding item thiếu mảng số"))?;
        let mut vector = values
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|number| number as f32)
                    .ok_or_else(|| fail("embedding chứa giá trị không phải số"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        normalize_embedding(&mut vector)?;
        indexed.push((index, vector));
    }
    indexed.sort_by_key(|(index, _)| *index);
    let vectors: Vec<Vec<f32>> = indexed.into_iter().map(|(_, vector)| vector).collect();
    if vectors.len() != texts.len() {
        return Err(fail(format!(
            "provider trả {} vector cho {} input",
            vectors.len(),
            texts.len()
        )));
    }
    validate_embedding_dimensions(cfg, &vectors)?;
    Ok(vectors)
}

fn embed_gemini_one(
    client: &reqwest::blocking::Client,
    cfg: &EmbeddingConfig,
    text: &str,
    task_type: &str,
) -> Result<Vec<f32>, ConvertError> {
    let base = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
    let url = gemini_embed_url(&base, &cfg.model);
    let mut body = serde_json::json!({
        "model": format!("models/{}", cfg.model),
        "content": {"parts": [{"text": text}]},
        "taskType": task_type
    });
    if let Some(dimensions) = cfg.dimensions {
        body["outputDimensionality"] = serde_json::json!(dimensions);
    }
    let secret = Some(cfg.api_key.as_str());
    let response = post_json(client, &url, &body, secret, |request| {
        gemini_auth(request, &cfg.api_key)
    })?;
    let values = response["embedding"]["values"]
        .as_array()
        .ok_or_else(|| invalid_provider_response("Gemini embedding", &response, secret))?;
    let mut vector = values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|number| number as f32)
                .ok_or_else(|| fail("Gemini embedding chứa giá trị không phải số"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    normalize_embedding(&mut vector)?;
    Ok(vector)
}

/// Embed text chunks through a configured neural embedding provider.
/// Calls are batched for OpenAI-compatible endpoints; Gemini is sequential.
pub fn embed_batch(cfg: &EmbeddingConfig, texts: &[String]) -> Result<Vec<Vec<f32>>, ConvertError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(fail)?;
    let mut vectors = Vec::with_capacity(texts.len());
    match cfg.provider {
        Provider::OpenAi | Provider::OpenAiCompatible => {
            for batch in texts.chunks(64) {
                vectors.extend(embed_openai_batch(&client, cfg, batch)?);
            }
        }
        Provider::Gemini => {
            for text in texts {
                vectors.push(embed_gemini_one(&client, cfg, text, "RETRIEVAL_DOCUMENT")?);
            }
            validate_embedding_dimensions(cfg, &vectors)?;
        }
        Provider::Anthropic | Provider::CursorCli | Provider::CodexCli => {
            return Err(fail("provider không hỗ trợ neural embeddings"));
        }
    }
    Ok(vectors)
}

pub fn embed_query(cfg: &EmbeddingConfig, query: &str) -> Result<Vec<f32>, ConvertError> {
    if cfg.provider == Provider::Gemini {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(fail)?;
        let vector = embed_gemini_one(&client, cfg, query, "RETRIEVAL_QUERY")?;
        validate_embedding_dimensions(cfg, std::slice::from_ref(&vector))?;
        return Ok(vector);
    }
    embed_batch(cfg, &[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| fail("provider không trả query embedding"))
}

/// Tóm tắt văn bản (Markdown tiếng Việt).
pub fn summarize(cfg: &LlmConfig, text: &str) -> Result<String, ConvertError> {
    let system = "Bạn là trợ lý tóm tắt tài liệu. Tóm tắt trung thực, giữ ý chính và số liệu \
                  quan trọng, không bịa. Trả về Markdown tiếng Việt, ngắn gọn.";
    chat(cfg, system, &format!("Tóm tắt tài liệu sau:\n\n{text}"))
}

/// Trích dữ liệu có cấu trúc theo yêu cầu; trả JSON (chuỗi).
pub fn extract_json(
    cfg: &LlmConfig,
    text: &str,
    instruction: &str,
) -> Result<String, ConvertError> {
    let system = "Bạn trích xuất dữ liệu có cấu trúc từ tài liệu. CHỈ trả về JSON hợp lệ \
                  (không giải thích, không code fence). Nếu thiếu dữ liệu, dùng null.";
    chat(
        cfg,
        system,
        &format!("Yêu cầu: {instruction}\n\nTài liệu:\n{text}"),
    )
}

/// OCR/đọc tài liệu KHÓ bằng vision-LLM (đa cột, IN HOA, chữ viết tay, con dấu…):
/// gửi ảnh cho model vision của provider, nhận Markdown. Đây là "tier chất lượng cao"
/// cho các ca Tesseract yếu — cần API key, nội dung ảnh được gửi tới provider.
pub fn vision_ocr(cfg: &LlmConfig, image_path: &std::path::Path) -> Result<String, ConvertError> {
    use base64::Engine as _;
    if cfg.is_subscription_cli() {
        return Err(fail(
            "subscription CLI hiện chỉ hỗ trợ text; vision OCR cần API/local vision provider",
        ));
    }
    let bytes = std::fs::read(image_path).map_err(fail)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mime = match image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/jpeg",
    };
    let system = "Bạn là công cụ OCR chất lượng cao cho tài liệu tiếng Việt. Chép lại TOÀN BỘ \
                  chữ trong ảnh thành Markdown, đúng thứ tự đọc (xử lý đa cột), giữ bảng thành \
                  bảng Markdown, giữ nguyên dấu tiếng Việt kể cả chữ IN HOA. Không bịa, không \
                  bình luận — chỉ trả nội dung.";

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(fail)?;

    match cfg.provider {
        Provider::OpenAi | Provider::OpenAiCompatible => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".into());
            let url = openai_chat_url(&base);
            let body = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": [
                        {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{b64}")}}
                    ]}
                ]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |request| {
                if cfg.api_key.trim().is_empty() {
                    request
                } else {
                    request.bearer_auth(&cfg.api_key)
                }
            })?;
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("OpenAI vision", &v, secret))
        }
        Provider::Anthropic => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            let url = format!("{}/v1/messages", base.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": cfg.model,
                "max_tokens": 8192,
                "system": system,
                "messages": [{"role": "user", "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": mime, "data": b64}}
                ]}]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |r| {
                r.header("x-api-key", &cfg.api_key)
                    .header("anthropic-version", "2023-06-01")
            })?;
            v["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("Anthropic vision", &v, secret))
        }
        Provider::Gemini => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
            let url = gemini_generate_url(&base, &cfg.model);
            let body = serde_json::json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"parts": [
                    {"inline_data": {"mime_type": mime, "data": b64}}
                ]}]
            });
            let secret = Some(cfg.api_key.as_str());
            let v = post_json(&client, &url, &body, secret, |r| {
                gemini_auth(r, &cfg.api_key)
            })?;
            v["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_provider_response("Gemini vision", &v, secret))
        }
        Provider::CursorCli | Provider::CodexCli => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn mock_openai_server() -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0u8; 16 * 1024];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            let body = r#"{"choices":[{"message":{"content":"OK"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    fn mock_json_server(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0u8; 32 * 1024];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn provider_aliases_map_to_expected_protocols() {
        assert_eq!(Provider::from_name("openai"), Provider::OpenAi);
        assert_eq!(Provider::from_name("claude"), Provider::Anthropic);
        assert_eq!(Provider::from_name("google"), Provider::Gemini);
        assert_eq!(Provider::from_name("ollama"), Provider::OpenAiCompatible);
        assert_eq!(Provider::from_name("vllm"), Provider::OpenAiCompatible);
    }

    #[test]
    fn presets_put_local_self_hosted_options_first() {
        let presets = provider_presets();
        assert!(presets.len() >= 9);
        assert!(presets[0].local);
        assert_eq!(presets[0].id, "ollama");
        assert!(presets.iter().any(|preset| preset.id == "openai"));
        assert!(presets.iter().any(|preset| preset.id == "anthropic"));
        assert!(presets.iter().any(|preset| preset.id == "gemini"));
        assert!(presets.iter().any(|preset| preset.id == "openrouter"));
    }

    #[test]
    fn embedding_presets_include_glm_interim_and_vllm_target() {
        let presets = embedding_provider_presets();
        let glm = presets
            .iter()
            .find(|preset| preset.id == "glm")
            .expect("glm embedding preset");
        assert_eq!(glm.default_model, "embedding-3");
        assert!(glm.requires_api_key);
        assert!(!glm.local);
        assert_eq!(glm.runtime_path, EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM);
        let vllm = presets
            .iter()
            .find(|preset| preset.id == "vllm")
            .expect("vllm embedding preset");
        assert!(vllm.local);
        assert_eq!(vllm.base_url.as_deref(), Some("http://127.0.0.1:8000"));
        assert_eq!(vllm.default_model, "BAAI/bge-m3");
        assert_eq!(vllm.runtime_path, EMBEDDING_RUNTIME_VLLM_LOCAL);
        // Real preset values must not need host/model cues containing "vllm".
        assert_eq!(
            infer_embedding_runtime_path(vllm.base_url.as_deref(), &vllm.default_model),
            EMBEDDING_RUNTIME_PROVIDER_CLOUD
        );
    }

    #[test]
    fn embedding_preset_runtime_paths_are_explicit() {
        let expected = [
            ("ollama", EMBEDDING_RUNTIME_PROVIDER_CLOUD),
            ("lm-studio", EMBEDDING_RUNTIME_PROVIDER_CLOUD),
            ("vllm", EMBEDDING_RUNTIME_VLLM_LOCAL),
            ("glm", EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM),
            ("openai", EMBEDDING_RUNTIME_PROVIDER_CLOUD),
            ("gemini", EMBEDDING_RUNTIME_PROVIDER_CLOUD),
            ("custom", EMBEDDING_RUNTIME_PROVIDER_CLOUD),
        ];
        let presets = embedding_provider_presets();
        for (id, runtime) in expected {
            let preset = presets
                .iter()
                .find(|preset| preset.id == id)
                .unwrap_or_else(|| panic!("missing preset {id}"));
            assert_eq!(preset.runtime_path, runtime, "preset {id}");
        }
    }

    #[test]
    fn local_config_accepts_empty_api_key() {
        let config = LlmConfig::new(
            Provider::OpenAiCompatible,
            "",
            "qwen2.5:7b",
            Some("http://127.0.0.1:11434".into()),
        )
        .unwrap();
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn config_rejects_empty_model_and_invalid_url() {
        assert!(LlmConfig::new(Provider::OpenAi, "key", "", None).is_err());
        assert!(LlmConfig::new(
            Provider::OpenAiCompatible,
            "",
            "model",
            Some("localhost:11434".into())
        )
        .is_err());
    }

    #[test]
    fn openai_compatible_url_accepts_base_with_or_without_v1() {
        assert_eq!(
            openai_chat_url("http://127.0.0.1:11434"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            openai_chat_url("http://127.0.0.1:1234/v1/"),
            "http://127.0.0.1:1234/v1/chat/completions"
        );
        assert_eq!(
            openai_embeddings_url("http://127.0.0.1:1234/v1/"),
            "http://127.0.0.1:1234/v1/embeddings"
        );
    }

    #[test]
    fn local_openai_compatible_chat_works_without_api_key() {
        let (base_url, server) = mock_openai_server();
        let config = LlmConfig::new(
            Provider::OpenAiCompatible,
            "",
            "local-model",
            Some(base_url),
        )
        .unwrap();
        assert_eq!(chat(&config, "system", "ping").unwrap(), "OK");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(!request.to_lowercase().contains("authorization:"));
    }

    #[test]
    fn cloud_style_openai_chat_sends_bearer_key() {
        let (base_url, server) = mock_openai_server();
        let config =
            LlmConfig::new(Provider::OpenAi, "secret-key", "model", Some(base_url)).unwrap();
        assert_eq!(chat(&config, "system", "ping").unwrap(), "OK");
        let request = server.join().unwrap().to_lowercase();
        assert!(request.contains("authorization: bearer secret-key"));
    }

    #[test]
    fn openai_compatible_embeddings_are_batched_and_normalized() {
        let response = r#"{"data":[
          {"index":1,"embedding":[0.0,2.0,0.0]},
          {"index":0,"embedding":[3.0,0.0,0.0]}
        ]}"#;
        let (base_url, server) = mock_json_server(response);
        let config = EmbeddingConfig::new(
            Provider::OpenAiCompatible,
            "",
            "nomic-embed-text",
            Some(base_url),
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
        )
        .unwrap();
        let vectors = embed_batch(&config, &["một".into(), "hai".into()]).unwrap();
        assert_eq!(vectors, vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]]);
        let request = server.join().unwrap().to_lowercase();
        assert!(request.starts_with("post /v1/embeddings "));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn embedding_config_rejects_chat_only_provider() {
        assert!(EmbeddingConfig::new(
            Provider::Anthropic,
            "key",
            "model",
            Some("https://api.anthropic.com".into()),
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
        )
        .is_err());
        assert!(EmbeddingConfig::new(
            Provider::OpenAi,
            "key",
            "model",
            Some("invalid".into()),
            None,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
        )
        .is_err());
        assert!(EmbeddingConfig::new(
            Provider::OpenAi,
            "key",
            "model",
            Some("https://api.openai.com".into()),
            None,
            "not-a-runtime",
        )
        .is_err());
    }

    #[test]
    fn config_debug_redacts_entire_endpoint_canary() {
        let secret = "super-secret-key-canary";
        let canary = "https://host/secret?sig=abc#access_token=def";
        let llm = LlmConfig::new(Provider::OpenAi, secret, "model", Some(canary.into())).unwrap();
        let llm_debug = format!("{llm:?}");
        assert!(!llm_debug.contains(secret));
        assert!(!llm_debug.contains("/secret"));
        assert!(!llm_debug.contains("sig=abc"));
        assert!(!llm_debug.contains("access_token=def"));
        assert!(!llm_debug.contains('?'));
        assert!(!llm_debug.contains('#'));
        assert!(llm_debug.contains("https://host/[REDACTED]"));

        let with_userinfo = LlmConfig::new(
            Provider::OpenAi,
            secret,
            "model",
            Some("https://user:pass@api.example.com/v1/path?q=1#frag".into()),
        )
        .unwrap();
        let debug = format!("{with_userinfo:?}");
        assert!(!debug.contains("user:pass"));
        assert!(!debug.contains("/v1/path"));
        assert!(debug.contains("https://api.example.com/[REDACTED]"));

        let restored: LlmConfig = serde_json::from_str(
            r#"{"provider":"open_ai","apiKey":"restored-from-disk","model":"m","baseUrl":null}"#,
        )
        .unwrap();
        assert_eq!(restored.api_key, "restored-from-disk");
        assert!(serde_json::from_str::<LlmConfig>(
            r#"{"provider":"open_ai","apiKey":"[REDACTED]","model":"m"}"#
        )
        .is_err());

        let embedding = EmbeddingConfig::new(
            Provider::OpenAi,
            "embed-secret-key-canary",
            "text-embedding-3-small",
            Some(canary.into()),
            Some(1536),
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
        )
        .unwrap();
        let embedding_debug = format!("{embedding:?}");
        assert!(!embedding_debug.contains("embed-secret-key-canary"));
        assert!(!embedding_debug.contains("/secret"));
        assert!(embedding_debug.contains("https://host/[REDACTED]"));
        assert!(serde_json::from_str::<EmbeddingConfig>(
            r#"{"provider":"open_ai","apiKey":"[REDACTED]","model":"m","runtimePath":"provider-cloud"}"#
        )
        .is_err());
    }

    #[test]
    fn retry_after_malformed_http_date_never_panics() {
        let now = std::time::SystemTime::now();
        for bad in [
            "",
            "not-a-date",
            "Sun Nov 10 08*00:00 2000",
            "999999999999999999999",
            "Thu, 40 Jan 1970 99:99:99 GMT",
        ] {
            let _ = parse_retry_after_header(Some(bad), now);
        }
        // Legacy RFC850 + asctime accepted by httpdate when well-formed.
        let epoch = std::time::UNIX_EPOCH + std::time::Duration::from_secs(784_111_777);
        assert_eq!(
            parse_retry_after_header(Some("Sunday, 06-Nov-94 08:49:37 GMT"), epoch),
            Some(std::time::Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after_header(Some("Sun Nov  6 08:49:37 1994"), epoch),
            Some(std::time::Duration::ZERO)
        );
    }

    #[test]
    fn transport_error_redacts_entire_endpoint_path_query_fragment() {
        let canary = "https://host/secret?sig=abc#access_token=def";
        let err = format!("error sending request for url ({canary})");
        let redacted = redact_surfaced_error(&err, Some("abc123XYZ"));
        assert!(!redacted.contains("/secret"));
        assert!(!redacted.contains("sig=abc"));
        assert!(!redacted.contains("access_token=def"));
        assert!(!redacted.contains('?'));
        assert!(!redacted.contains('#'));
        assert!(redacted.contains("https://host/[REDACTED]"));
        assert_eq!(redact_endpoint(canary), "https://host/[REDACTED]");

        let with_key = format!(
            "error sending request for url (https://generativelanguage.googleapis.com/v1beta/models/x?key=abc123XYZ)"
        );
        let redacted_key = redact_surfaced_error(&with_key, Some("abc123XYZ"));
        assert!(!redacted_key.contains("abc123XYZ"));
        assert!(!redacted_key.contains("/v1beta"));
        assert!(redacted_key.contains("https://generativelanguage.googleapis.com/[REDACTED]"));
    }

    #[test]
    fn malformed_response_error_redacts_secret_canary() {
        let secret = "malformed-body-secret-canary";
        let body = format!(r#"{{"error":"echo {secret}","leak":"{secret}"}}"#);
        let leaked: &'static str = Box::leak(body.into_boxed_str());
        let (base_url, server) = mock_json_server(leaked);
        let config = LlmConfig::new(Provider::OpenAi, secret, "model", Some(base_url)).unwrap();
        let error = chat(&config, "system", "ping").unwrap_err().to_string();
        assert!(
            !error.contains(secret),
            "malformed response leaked: {error}"
        );
        assert!(error.contains("[REDACTED]") || error.contains("không hợp lệ"));
        let _ = server.join();
    }

    #[test]
    fn retry_after_delta_and_http_date_respect_budget_without_shortening() {
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_609_459_200); // 2021-01-01
        assert_eq!(
            parse_retry_after_header(Some("2"), now),
            Some(std::time::Duration::from_secs(2))
        );
        // Past HTTP-date → zero wait (retry immediately).
        assert_eq!(
            parse_retry_after_header(Some("Wed, 21 Oct 2015 07:28:00 GMT"), now),
            Some(std::time::Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after_header(Some("Fri, 01 Jan 2021 00:00:04 GMT"), now),
            Some(std::time::Duration::from_secs(4))
        );

        assert_eq!(
            plan_retry_wait(0, Some(std::time::Duration::from_secs(1))),
            Some(std::time::Duration::from_secs(1))
        );
        // Beyond budget → no retry (never sleep less than requested).
        assert_eq!(
            plan_retry_wait(0, Some(std::time::Duration::from_secs(100))),
            None
        );
        assert_eq!(
            plan_retry_wait(0, None),
            Some(std::time::Duration::from_millis(200))
        );
        assert_eq!(
            plan_retry_wait(1, None),
            Some(std::time::Duration::from_millis(400))
        );
        assert_eq!(plan_retry_wait(2, None), None);
    }

    #[test]
    fn retry_after_beyond_budget_returns_without_sleeping() {
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(true));
        TEST_SLEEP_LOG.with(|log| log.borrow_mut().clear());
        let (base_url, server) =
            mock_status_sequence_server(vec![(429, r#"{"error":"rate"}"#, Some("120"))]);
        let config =
            LlmConfig::new(Provider::OpenAi, "secret-key", "model", Some(base_url)).unwrap();
        let error = chat(&config, "system", "ping").unwrap_err();
        assert!(error.to_string().contains("LLM HTTP 429"));
        assert!(
            TEST_SLEEP_LOG.with(|log| log.borrow().is_empty()),
            "must not sleep when Retry-After exceeds budget"
        );
        assert_eq!(server.join().unwrap(), 1);
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(false));
    }

    fn mock_status_sequence_server(
        responses: Vec<(u16, &'static str, Option<&'static str>)>,
    ) -> (String, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut count = 0usize;
            for (status, body, retry_after) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0u8; 32 * 1024];
                let _ = stream.read(&mut request);
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let retry_header = retry_after
                    .map(|value| format!("Retry-After: {value}\r\n"))
                    .unwrap_or_default();
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry_header}Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                count += 1;
            }
            count
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn retries_429_with_retry_after_zero_then_succeeds() {
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(true));
        TEST_SLEEP_LOG.with(|log| log.borrow_mut().clear());
        let ok = r#"{"choices":[{"message":{"content":"OK"}}]}"#;
        let (base_url, server) = mock_status_sequence_server(vec![
            (429, r#"{"error":"rate"}"#, Some("0")),
            (200, ok, None),
        ]);
        let config =
            LlmConfig::new(Provider::OpenAi, "secret-key", "model", Some(base_url)).unwrap();
        assert_eq!(chat(&config, "system", "ping").unwrap(), "OK");
        assert_eq!(server.join().unwrap(), 2);
        assert_eq!(
            TEST_SLEEP_LOG.with(|log| log.borrow().clone()),
            vec![std::time::Duration::ZERO]
        );
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(false));
    }

    #[test]
    fn gives_up_after_bounded_503_retries() {
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(true));
        let (base_url, server) = mock_status_sequence_server(vec![
            (503, "no", Some("0")),
            (503, "no", Some("0")),
            (503, "no", Some("0")),
        ]);
        let config =
            LlmConfig::new(Provider::OpenAi, "secret-key", "model", Some(base_url)).unwrap();
        let error = chat(&config, "system", "ping").unwrap_err();
        assert!(error.to_string().contains("LLM HTTP 503"));
        assert_eq!(server.join().unwrap(), 3);
        TEST_SKIP_REAL_SLEEP.with(|skip| skip.set(false));
    }

    #[test]
    fn read_timeout_after_server_consumes_request_is_not_retried() {
        // Server accepts + fully reads the generation POST, then delays the
        // response past the client timeout. Must not issue a second request.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut count = 0usize;
            listener.set_nonblocking(false).unwrap();
            if let Ok((mut stream, _)) = listener.accept() {
                count += 1;
                let mut request = vec![0u8; 64 * 1024];
                let _ = stream.read(&mut request);
                std::thread::sleep(std::time::Duration::from_millis(400));
                let body = r#"{"choices":[{"message":{"content":"late"}}]}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            // Brief window for a hypothetical retry to arrive.
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while std::time::Instant::now() < deadline {
                if listener.accept().is_ok() {
                    count += 1;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            count
        });
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(1))
            .timeout(std::time::Duration::from_millis(150))
            .build()
            .unwrap();
        let url = format!("http://{address}/v1/chat/completions");
        let body = serde_json::json!({"model":"m","messages":[]});
        let error = post_json(&client, &url, &body, Some("k"), |request| request);
        assert!(
            error.is_err(),
            "expected timeout/transport error, got {error:?}"
        );
        assert_eq!(
            server.join().unwrap(),
            1,
            "ambiguous read/timeout after server consumed the POST must not retry"
        );
    }

    #[test]
    fn gemini_chat_uses_header_not_query_key() {
        let body = r#"{"candidates":[{"content":{"parts":[{"text":"gemini-ok"}]}}]}"#;
        let (base_url, server) = mock_json_server(body);
        let config = LlmConfig::new(
            Provider::Gemini,
            "secret-gemini-key",
            "gemini-2.0-flash",
            Some(base_url),
        )
        .unwrap();
        assert_eq!(chat(&config, "system", "ping").unwrap(), "gemini-ok");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1beta/models/gemini-2.0-flash:generateContent "));
        assert!(!request.contains("key=secret-gemini-key"));
        assert!(request
            .to_lowercase()
            .contains("x-goog-api-key: secret-gemini-key"));
    }

    #[test]
    fn transport_connect_error_redacts_endpoint_and_api_key() {
        // Closed port → connect error. Path/query/fragment/userinfo must not surface.
        let secret = "gemini-leak-canary-key";
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(100))
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();
        let url =
            format!("http://user:pass@127.0.0.1:9/secret?sig=abc&key={secret}#access_token=def");
        let body = serde_json::json!({});
        let error = post_json(&client, &url, &body, Some(secret), |request| request).unwrap_err();
        let message = error.to_string();
        assert!(!message.contains(secret), "leaked key in: {message}");
        assert!(
            !message.contains("user:pass"),
            "leaked userinfo in: {message}"
        );
        assert!(!message.contains("/secret"), "leaked path in: {message}");
        assert!(!message.contains("sig=abc"), "leaked query in: {message}");
        assert!(
            !message.contains("access_token=def"),
            "leaked fragment in: {message}"
        );
        assert!(
            message.contains("http://127.0.0.1:9/[REDACTED]"),
            "{message}"
        );
    }
}
