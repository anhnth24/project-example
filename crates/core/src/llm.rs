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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
    Gemini,
    OpenAiCompatible,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: Provider,
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
}

impl LlmConfig {
    /// Đọc cấu hình từ env. None nếu thiếu API key (⇒ chạy chức năng mặc định).
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("FILECONV_LLM_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let provider = match std::env::var("FILECONV_LLM_PROVIDER")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "anthropic" | "claude" => Provider::Anthropic,
            "gemini" | "google" => Provider::Gemini,
            "openai-compatible" | "compatible" | "ollama" | "openrouter" | "groq" => {
                Provider::OpenAiCompatible
            }
            _ => Provider::OpenAi,
        };
        let model = std::env::var("FILECONV_LLM_MODEL").unwrap_or_else(|_| default_model(provider));
        let base_url = std::env::var("FILECONV_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        Some(Self {
            provider,
            api_key,
            model,
            base_url,
        })
    }
}

fn default_model(p: Provider) -> String {
    match p {
        Provider::OpenAi | Provider::OpenAiCompatible => "gpt-4o-mini",
        Provider::Anthropic => "claude-3-5-haiku-latest",
        Provider::Gemini => "gemini-2.0-flash",
    }
    .to_string()
}

fn fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Failed(e.to_string())
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
            let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user}
                ]
            });
            let v = post_json(&client, &url, &body, |r| r.bearer_auth(&cfg.api_key))?;
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
            let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": cfg.model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": [
                        {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{b64}")}}
                    ]}
                ]
            });
            let v = post_json(&client, &url, &body, |r| r.bearer_auth(&cfg.api_key))?;
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
