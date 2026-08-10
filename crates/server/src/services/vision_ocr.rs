//! Vision-OCR runtime cho worker (ngoài converter sandbox).
//!
//! Converter sandbox không có network và không bao giờ nhận API key; nó chỉ ghi
//! JPEG trang scan (`markhand-ocr-*.jpg`) kèm placeholder vào Markdown (deferred
//! OCR — xem `fileconv_core::image_ocr`). Worker tin cậy dùng runtime này để gọi
//! provider vision (OpenRouter mặc định) rồi thay placeholder bằng nội dung.
//!
//! Env:
//!   MARKHAND_OCR_API_KEY        (bắt buộc trừ khi BASE_URL là endpoint local)
//!   MARKHAND_OCR_BASE_URL       (mặc định https://openrouter.ai/api)
//!   MARKHAND_OCR_MODEL          (mặc định qwen/qwen3.7-flash)
//!   MARKHAND_OCR_SYSTEM_PROMPT  (tuỳ chọn — thay prompt "chép trung thực")
//!   MARKHAND_OCR_TIMEOUT_SECS   (mặc định 180, cho một trang)
//!
//! Không log nội dung trang/prompt/key; lỗi chỉ mang metadata.

use std::env;
use std::time::Duration;

use base64::Engine as _;
use fileconv_core::image_ocr::{
    default_vision_ocr_system_prompt, strip_ocr_wrapping_fence, DEFAULT_VISION_OCR_BASE_URL,
    DEFAULT_VISION_OCR_MODEL, DEFAULT_VISION_OCR_TIMEOUT_SECS,
};
use serde::Deserialize;
use thiserror::Error;

const ENV_API_KEY: &str = "MARKHAND_OCR_API_KEY";
const ENV_BASE_URL: &str = "MARKHAND_OCR_BASE_URL";
const ENV_MODEL: &str = "MARKHAND_OCR_MODEL";
const ENV_SYSTEM_PROMPT: &str = "MARKHAND_OCR_SYSTEM_PROMPT";
const ENV_TIMEOUT_SECS: &str = "MARKHAND_OCR_TIMEOUT_SECS";

/// Ngôn ngữ hint cố định của server (khớp `ConverterOptions::default`).
const SERVER_OCR_LANGS: &str = "vie+eng";

#[derive(Clone)]
pub struct VisionOcrRuntime {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    system_prompt: String,
}

impl std::fmt::Debug for VisionOcrRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VisionOcrRuntime")
            .field("endpoint", &"[REDACTED_ENDPOINT]")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl VisionOcrRuntime {
    /// `Ok(None)` khi chưa cấu hình (không key và không endpoint tuỳ chỉnh) —
    /// caller quyết định fail-closed khi gặp trang cần OCR.
    pub fn from_env() -> Result<Option<Self>, VisionOcrError> {
        let read = |key: &str| env::var(key).ok().filter(|value| !value.trim().is_empty());
        let api_key = read(ENV_API_KEY).unwrap_or_default();
        let base_url = read(ENV_BASE_URL);
        if api_key.is_empty() && base_url.is_none() {
            return Ok(None);
        }
        let base_url = base_url.unwrap_or_else(|| DEFAULT_VISION_OCR_BASE_URL.to_string());
        let model = read(ENV_MODEL).unwrap_or_else(|| DEFAULT_VISION_OCR_MODEL.to_string());
        let timeout_secs = match read(ENV_TIMEOUT_SECS) {
            Some(raw) => raw
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or(VisionOcrError::InvalidConfiguration(ENV_TIMEOUT_SECS))?,
            None => DEFAULT_VISION_OCR_TIMEOUT_SECS,
        };
        let system_prompt = read(ENV_SYSTEM_PROMPT)
            .unwrap_or_else(|| default_vision_ocr_system_prompt(SERVER_OCR_LANGS));
        Self::new(base_url, api_key, model, system_prompt, timeout_secs).map(Some)
    }

    pub fn new(
        base_url: String,
        api_key: String,
        model: String,
        system_prompt: String,
        timeout_secs: u64,
    ) -> Result<Self, VisionOcrError> {
        let base = base_url.trim_end_matches('/');
        if base.is_empty() {
            return Err(VisionOcrError::InvalidConfiguration(ENV_BASE_URL));
        }
        let endpoint = if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .build()
            .map_err(|_| VisionOcrError::Http)?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model,
            system_prompt,
        })
    }

    /// OCR một ảnh JPEG (artifact từ sandbox). Trả Markdown đã strip fence.
    /// Worker KHÔNG decode ảnh — bytes đi thẳng tới provider.
    pub async fn ocr_jpeg(&self, jpeg: &[u8]) -> Result<String, VisionOcrError> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let body = serde_json::json!({
            "model": self.model,
            // OCR là chép trung thực — tắt reasoning để giảm latency/cost.
            "reasoning": {"enabled": false},
            "messages": [
                {"role": "system", "content": self.system_prompt},
                {"role": "user", "content": [
                    {"type": "image_url",
                     "image_url": {"url": format!("data:image/jpeg;base64,{b64}")}}
                ]}
            ]
        });
        let mut request = self.client.post(&self.endpoint).json(&body);
        if !self.api_key.trim().is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = request.send().await.map_err(|_| VisionOcrError::Http)?;
        let status = response.status();
        if !status.is_success() {
            return Err(VisionOcrError::Status(status.as_u16()));
        }
        let parsed = response
            .json::<ChatResponse>()
            .await
            .map_err(|_| VisionOcrError::InvalidResponse)?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or(VisionOcrError::InvalidResponse)?;
        Ok(strip_ocr_wrapping_fence(&content))
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Error)]
pub enum VisionOcrError {
    #[error("vision OCR configuration is missing or invalid: {0}")]
    InvalidConfiguration(&'static str),
    #[error("vision OCR provider request failed")]
    Http,
    #[error("vision OCR provider returned HTTP {0}")]
    Status(u16),
    #[error("vision OCR provider returned an invalid response")]
    InvalidResponse,
}

impl VisionOcrError {
    /// Provider outage/rate-limit là transient — job được retry với backoff.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http => true,
            Self::Status(status) => matches!(status, 408 | 429 | 500..=599),
            Self::InvalidConfiguration(_) | Self::InvalidResponse => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_handles_v1_suffix_and_debug_redacts() {
        let runtime = VisionOcrRuntime::new(
            "https://openrouter.ai/api".into(),
            "sk-or-secret".into(),
            "vision-model".into(),
            "prompt".into(),
            30,
        )
        .expect("runtime");
        assert_eq!(
            runtime.endpoint,
            "https://openrouter.ai/api/v1/chat/completions"
        );
        let with_v1 = VisionOcrRuntime::new(
            "http://127.0.0.1:8000/v1".into(),
            String::new(),
            "m".into(),
            "p".into(),
            30,
        )
        .expect("runtime");
        assert_eq!(
            with_v1.endpoint,
            "http://127.0.0.1:8000/v1/chat/completions"
        );
        let debug = format!("{runtime:?}");
        assert!(!debug.contains("sk-or-secret"));
        assert!(!debug.contains("openrouter.ai"));
    }

    #[test]
    fn retryability_matches_transient_failures_only() {
        assert!(VisionOcrError::Http.is_retryable());
        assert!(VisionOcrError::Status(429).is_retryable());
        assert!(VisionOcrError::Status(503).is_retryable());
        assert!(!VisionOcrError::Status(400).is_retryable());
        assert!(!VisionOcrError::Status(401).is_retryable());
        assert!(!VisionOcrError::InvalidResponse.is_retryable());
        assert!(!VisionOcrError::InvalidConfiguration("X").is_retryable());
    }

    #[tokio::test]
    async fn ocr_jpeg_parses_openai_compatible_response() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = vec![0_u8; 256 * 1024];
            let mut read_total = 0;
            // Đọc tới hết header + body (best effort cho test).
            loop {
                let n = stream.read(&mut buffer[read_total..]).unwrap_or(0);
                if n == 0 {
                    break;
                }
                read_total += n;
                let text = String::from_utf8_lossy(&buffer[..read_total]);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let content_length = text
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if read_total >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&buffer[..read_total]).to_string();
            let body = r#"{"choices":[{"message":{"content":"```markdown\n# Trang OCR\n```"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
            request
        });
        let runtime = VisionOcrRuntime::new(
            format!("http://{address}"),
            "test-key".into(),
            "test-model".into(),
            "system prompt".into(),
            10,
        )
        .expect("runtime");
        let text = runtime
            .ocr_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0])
            .await
            .expect("ocr");
        assert_eq!(text, "# Trang OCR");
        let request = server.join().expect("server thread");
        assert!(
            request.contains("authorization: Bearer test-key")
                || request.contains("Authorization: Bearer test-key")
        );
        assert!(request.contains("test-model"));
        assert!(request.contains("\"reasoning\""));
    }
}
