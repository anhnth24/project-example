//! Lớp LLM TUỲ CHỌN (feature `llm`). Chỉ hoạt động khi có API key trong env —
//! không key thì `LlmConfig::from_env()` trả None và caller báo lỗi rõ.
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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub provider: Provider,
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
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
    pub description: String,
}

impl Provider {
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "anthropic" | "claude" => Self::Anthropic,
            "gemini" | "google" => Self::Gemini,
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
        })
    }

    /// Đọc cấu hình từ env. Localhost/OpenAI-compatible local không bắt buộc key.
    pub fn from_env() -> Option<Self> {
        let provider_name =
            std::env::var("FILECONV_LLM_PROVIDER").unwrap_or_else(|_| "openai".into());
        let provider = Provider::from_name(&provider_name);
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

fn fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Failed(e.to_string())
}

fn openai_chat_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// Gọi 1 lượt chat (system + user) → trả text.
pub fn chat(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, ConvertError> {
    let client = reqwest::blocking::Client::builder()
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
            let v = post_json(&client, &url, &body, |request| {
                if cfg.api_key.trim().is_empty() {
                    request
                } else {
                    request.bearer_auth(&cfg.api_key)
                }
            })?;
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi OpenAI không hợp lệ: {v}")))
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
            let v = post_json(&client, &url, &body, |r| {
                r.header("x-api-key", &cfg.api_key)
                    .header("anthropic-version", "2023-06-01")
            })?;
            v["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi Anthropic không hợp lệ: {v}")))
        }
        Provider::Gemini => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
            let url = format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base.trim_end_matches('/'),
                cfg.model,
                cfg.api_key
            );
            let body = serde_json::json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"parts": [{"text": user}]}]
            });
            let v = post_json(&client, &url, &body, |r| r)?;
            v["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi Gemini không hợp lệ: {v}")))
        }
    }
}

fn post_json(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &serde_json::Value,
    decorate: impl FnOnce(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
) -> Result<serde_json::Value, ConvertError> {
    let req = decorate(client.post(url).json(body));
    let resp = req.send().map_err(fail)?;
    let status = resp.status();
    let text = resp.text().map_err(fail)?;
    if !status.is_success() {
        return Err(fail(format!("LLM HTTP {status}: {text}")));
    }
    serde_json::from_str(&text).map_err(fail)
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
            let v = post_json(&client, &url, &body, |request| {
                if cfg.api_key.trim().is_empty() {
                    request
                } else {
                    request.bearer_auth(&cfg.api_key)
                }
            })?;
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi OpenAI vision không hợp lệ: {v}")))
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
            let v = post_json(&client, &url, &body, |r| {
                r.header("x-api-key", &cfg.api_key)
                    .header("anthropic-version", "2023-06-01")
            })?;
            v["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi Anthropic vision không hợp lệ: {v}")))
        }
        Provider::Gemini => {
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".into());
            let url = format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base.trim_end_matches('/'),
                cfg.model,
                cfg.api_key
            );
            let body = serde_json::json!({
                "systemInstruction": {"parts": [{"text": system}]},
                "contents": [{"parts": [
                    {"inline_data": {"mime_type": mime, "data": b64}}
                ]}]
            });
            let v = post_json(&client, &url, &body, |r| r)?;
            v["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| fail(format!("phản hồi Gemini vision không hợp lệ: {v}")))
        }
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
}
