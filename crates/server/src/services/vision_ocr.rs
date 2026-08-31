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
    DEFAULT_VISION_OCR_MODEL,
};
use serde::Deserialize;
use thiserror::Error;

const ENV_API_KEY: &str = "MARKHAND_OCR_API_KEY";
const ENV_BASE_URL: &str = "MARKHAND_OCR_BASE_URL";
const ENV_MODEL: &str = "MARKHAND_OCR_MODEL";
const ENV_SYSTEM_PROMPT: &str = "MARKHAND_OCR_SYSTEM_PROMPT";
const ENV_TIMEOUT_SECS: &str = "MARKHAND_OCR_TIMEOUT_SECS";
const ENV_BATCH_PAGES: &str = "MARKHAND_OCR_BATCH_PAGES";

/// Ngôn ngữ hint cố định của server (khớp `ConverterOptions::default`).
const SERVER_OCR_LANGS: &str = "vie+eng";

/// Số trang/request mặc định. Bench product owner 2026-08-10 (Qwen3.7
/// Flash/Plus, tài liệu scan 839 trang): 10 trang ≈ 60–150s / 16K output
/// token; 5 trang ≈ 30–70s / 12K; 1 trang (chạy bù) ≈ 20–50s / 8K.
/// 5 là điểm cân bằng an toàn; trang thưa chữ có thể tăng lên 10.
const DEFAULT_BATCH_PAGES: usize = 5;
const MAX_BATCH_PAGES: usize = 16;
/// Timeout cap mặc định của server cho MỘT request batch (cao hơn mặc định
/// per-page của core vì một request có thể chứa tới 10 trang).
const DEFAULT_SERVER_TIMEOUT_CAP_SECS: u64 = 300;

/// `max_tokens` theo cỡ batch, nội suy từ bench: 1→8K, 5→~12K, 10→16K.
fn batch_max_output_tokens(pages: usize) -> u64 {
    (8_000 + 900 * (pages.saturating_sub(1)) as u64).min(16_000)
}

/// Marker phân trang trong response batch. Model phải mở đầu mỗi trang bằng
/// dòng này; parse fail-closed (thiếu/sai thứ tự → bisect batch).
fn page_marker(index: usize) -> String {
    format!("<!-- markhand:page {index} -->")
}

#[derive(Clone)]
pub struct VisionOcrRuntime {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    system_prompt: String,
    /// Cap cho timeout mỗi request (timeout thật scale theo số trang).
    timeout_cap: Duration,
    batch_pages: usize,
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
            None => DEFAULT_SERVER_TIMEOUT_CAP_SECS,
        };
        let batch_pages = match read(ENV_BATCH_PAGES) {
            Some(raw) => raw
                .parse::<usize>()
                .ok()
                .filter(|value| (1..=MAX_BATCH_PAGES).contains(value))
                .ok_or(VisionOcrError::InvalidConfiguration(ENV_BATCH_PAGES))?,
            None => DEFAULT_BATCH_PAGES,
        };
        let system_prompt = read(ENV_SYSTEM_PROMPT)
            .unwrap_or_else(|| default_vision_ocr_system_prompt(SERVER_OCR_LANGS));
        Self::new(base_url, api_key, model, system_prompt, timeout_secs)
            .map(|runtime| Some(runtime.with_batch_pages(batch_pages)))
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
            .build()
            .map_err(|_| VisionOcrError::Http)?;
        Ok(Self {
            client,
            endpoint,
            api_key,
            model,
            system_prompt,
            timeout_cap: Duration::from_secs(timeout_secs.max(1)),
            batch_pages: DEFAULT_BATCH_PAGES,
        })
    }

    pub fn with_batch_pages(mut self, batch_pages: usize) -> Self {
        self.batch_pages = batch_pages.clamp(1, MAX_BATCH_PAGES);
        self
    }

    /// Số trang mỗi request mà stage OCR của worker nên gom.
    pub fn batch_pages(&self) -> usize {
        self.batch_pages
    }

    /// Timeout cho một request theo số trang, nội suy từ bench (10 trang tối đa
    /// ~150s, 5 trang ~70s, 1 trang ~50s) + biên an toàn: `60s + 15s × trang`,
    /// chặn trên bởi `MARKHAND_OCR_TIMEOUT_SECS`.
    pub fn batch_timeout(&self, pages: usize) -> Duration {
        // Base 120s (thay vì 60s): OpenRouter lúc tải cao cần >90s cho MỘT
        // trang dày chữ (đo 2026-08-16, 3 job dead-letter chỉ vì trần 75s);
        // cap MARKHAND_OCR_TIMEOUT_SECS vẫn thắng công thức.
        Duration::from_secs(120 + 30 * pages as u64).min(self.timeout_cap)
    }

    /// OCR một ảnh JPEG (artifact từ sandbox). Trả Markdown đã strip fence.
    /// Worker KHÔNG decode ảnh — bytes đi thẳng tới provider.
    pub async fn ocr_jpeg(&self, jpeg: &[u8]) -> Result<String, VisionOcrError> {
        let pages = self.ocr_jpeg_batch(&[jpeg]).await?;
        pages
            .into_iter()
            .next()
            .ok_or(VisionOcrError::InvalidResponse)
    }

    /// OCR một batch trang (cùng tài liệu, theo thứ tự) trong MỘT request.
    /// Batch >1 dùng marker `<!-- markhand:page k -->` để tách kết quả; parse
    /// fail-closed (`BatchParse`) để caller bisect xuống batch nhỏ hơn/1 trang.
    pub async fn ocr_jpeg_batch(&self, pages: &[&[u8]]) -> Result<Vec<String>, VisionOcrError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        let count = pages.len();
        let mut content = Vec::with_capacity(count + 1);
        for jpeg in pages {
            let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
            content.push(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:image/jpeg;base64,{b64}")}
            }));
        }
        if count > 1 {
            content.push(serde_json::json!({
                "type": "text",
                "text": format!(
                    "Có {count} ảnh, là các trang liên tiếp của cùng một tài liệu, đánh số \
                     1..{count} theo thứ tự gửi. Với MỖI trang, mở đầu khối kết quả bằng \
                     dòng chính xác `<!-- markhand:page k -->` (k là số thứ tự trang), \
                     sau đó là nội dung Markdown của trang. Đủ {count} khối, đúng thứ tự, \
                     không gộp trang, không bỏ trang, không thêm nhận xét."
                )
            }));
        }
        let body = serde_json::json!({
            "model": self.model,
            // OCR là chép trung thực — tắt reasoning để giảm latency/cost.
            "reasoning": {"enabled": false},
            // Trần output theo bench (1→8K, 5→~12K, 10→16K token).
            "max_tokens": batch_max_output_tokens(count),
            "messages": [
                {"role": "system", "content": self.system_prompt},
                {"role": "user", "content": content}
            ]
        });
        let mut request = self
            .client
            .post(&self.endpoint)
            .timeout(self.batch_timeout(count))
            .json(&body);
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
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or(VisionOcrError::InvalidResponse)?;
        let truncated = choice.finish_reason.as_deref() == Some("length");
        let text = choice
            .message
            .content
            .ok_or(VisionOcrError::InvalidResponse)?;
        if truncated {
            // Output chạm trần max_tokens — trang cuối có thể mất chữ; batch
            // nhỏ hơn có trần/trang cao hơn (bench "chạy bù" 1 trang / 8K).
            return Err(VisionOcrError::Truncated);
        }
        if count == 1 {
            return Ok(vec![strip_ocr_wrapping_fence(&text)]);
        }
        split_batch_response(&strip_ocr_wrapping_fence(&text), count)
    }
}

/// Tách response batch theo marker; yêu cầu đủ `n` marker đúng thứ tự và không
/// có nội dung lạ trước marker đầu tiên.
fn split_batch_response(text: &str, n: usize) -> Result<Vec<String>, VisionOcrError> {
    let mut boundaries = Vec::with_capacity(n);
    for index in 1..=n {
        let marker = page_marker(index);
        let mut matches = text.match_indices(&marker);
        let (offset, _) = matches.next().ok_or(VisionOcrError::BatchParse)?;
        if matches.next().is_some() {
            return Err(VisionOcrError::BatchParse);
        }
        boundaries.push((offset, marker.len()));
    }
    if boundaries.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(VisionOcrError::BatchParse);
    }
    if !text[..boundaries[0].0].trim().is_empty() {
        return Err(VisionOcrError::BatchParse);
    }
    let mut pages = Vec::with_capacity(n);
    for (position, (offset, marker_len)) in boundaries.iter().enumerate() {
        let start = offset + marker_len;
        let end = boundaries
            .get(position + 1)
            .map(|(next, _)| *next)
            .unwrap_or(text.len());
        pages.push(text[start..end].trim().to_string());
    }
    Ok(pages)
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
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
    #[error("vision OCR batch response is missing page markers")]
    BatchParse,
    #[error("vision OCR output hit the max_tokens ceiling (truncated)")]
    Truncated,
}

impl VisionOcrError {
    /// Provider outage/rate-limit là transient — job được retry với backoff.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http => true,
            Self::Status(status) => matches!(status, 408 | 429 | 500..=599),
            Self::InvalidConfiguration(_)
            | Self::InvalidResponse
            | Self::BatchParse
            | Self::Truncated => false,
        }
    }

    /// Lỗi mà batch NHỎ HƠN có cơ hội khắc phục (bench "chạy bù"): parse/
    /// truncation chắc chắn; timeout/transient cũng đáng thử trước khi trả
    /// job về retry-backoff. Lỗi auth/config thì không.
    pub fn is_batch_splittable(&self) -> bool {
        match self {
            Self::BatchParse | Self::Truncated | Self::Http => true,
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
    fn batch_token_and_timeout_formulas_match_owner_bench() {
        // Bench 2026-08-10: 1 trang → 8K token / ≤50s; 5 → 12K / ≤70s; 10 → 16K / ≤150s.
        assert_eq!(batch_max_output_tokens(1), 8_000);
        assert_eq!(batch_max_output_tokens(5), 11_600);
        assert_eq!(batch_max_output_tokens(10), 16_000);
        assert_eq!(batch_max_output_tokens(16), 16_000);
        let runtime = VisionOcrRuntime::new(
            "https://openrouter.ai/api".into(),
            "k".into(),
            "m".into(),
            "p".into(),
            300,
        )
        .expect("runtime");
        assert_eq!(runtime.batch_timeout(1), Duration::from_secs(150));
        assert_eq!(runtime.batch_timeout(5), Duration::from_secs(270));
        assert_eq!(runtime.batch_timeout(10), Duration::from_secs(300));
        // Cap từ MARKHAND_OCR_TIMEOUT_SECS thắng công thức.
        let capped = VisionOcrRuntime::new(
            "https://openrouter.ai/api".into(),
            "k".into(),
            "m".into(),
            "p".into(),
            90,
        )
        .expect("runtime");
        assert_eq!(capped.batch_timeout(10), Duration::from_secs(90));
    }

    #[test]
    fn split_batch_response_requires_all_markers_in_order() {
        let ok = format!(
            "{}\n# Trang 1\nnội dung 1\n{}\nnội dung 2",
            page_marker(1),
            page_marker(2)
        );
        assert_eq!(
            split_batch_response(&ok, 2).unwrap(),
            vec![
                "# Trang 1\nnội dung 1".to_string(),
                "nội dung 2".to_string()
            ]
        );
        // Thiếu marker → BatchParse.
        assert!(matches!(
            split_batch_response("chỉ có text", 2),
            Err(VisionOcrError::BatchParse)
        ));
        // Sai thứ tự → BatchParse.
        let swapped = format!("{}\nb\n{}\na", page_marker(2), page_marker(1));
        assert!(matches!(
            split_batch_response(&swapped, 2),
            Err(VisionOcrError::BatchParse)
        ));
        // Nội dung lạ trước marker đầu (model chatter) → BatchParse.
        let preamble = format!(
            "Đây là kết quả:\n{}\nx\n{}\ny",
            page_marker(1),
            page_marker(2)
        );
        assert!(matches!(
            split_batch_response(&preamble, 2),
            Err(VisionOcrError::BatchParse)
        ));
        // Marker trùng lặp → BatchParse.
        let duplicated = format!("{m1}\na\n{m1}\nb", m1 = page_marker(1));
        assert!(matches!(
            split_batch_response(&duplicated, 1),
            Err(VisionOcrError::BatchParse)
        ));
    }

    #[test]
    fn batch_splittable_covers_parse_truncation_and_transient_errors() {
        assert!(VisionOcrError::BatchParse.is_batch_splittable());
        assert!(VisionOcrError::Truncated.is_batch_splittable());
        assert!(VisionOcrError::Http.is_batch_splittable());
        assert!(VisionOcrError::Status(429).is_batch_splittable());
        assert!(!VisionOcrError::Status(401).is_batch_splittable());
        assert!(!VisionOcrError::InvalidConfiguration("X").is_batch_splittable());
        assert!(!VisionOcrError::InvalidResponse.is_batch_splittable());
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
