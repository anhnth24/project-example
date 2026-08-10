//! OCR ảnh bằng **vision-LLM** (OpenRouter mặc định) — đường OCR duy nhất của
//! converter. Tesseract/PaddleOCR local đã bị loại bỏ (quyết định product
//! 2026-08-10): trang scan / ảnh được chuẩn hoá kích thước, encode JPEG rồi gửi
//! provider vision qua API OpenAI-compatible. `pdf-inspector` vẫn là nơi quyết
//! định trang PDF nào cần OCR — trang có text layer tin cậy KHÔNG đi qua đây.
//!
//! Cấu hình (env, override được qua [`OcrRunConfig::vision`]):
//!   FILECONV_OCR_BASE_URL      (mặc định https://openrouter.ai/api)
//!   FILECONV_OCR_MODEL         (mặc định qwen/qwen3.7-flash)
//!   FILECONV_OCR_API_KEY       (fallback FILECONV_LLM_API_KEY)
//!   FILECONV_OCR_SYSTEM_PROMPT (tuỳ chọn — thay system prompt mặc định)
//!   FILECONV_OCR_TIMEOUT_SECS  (mặc định 180)
//!
//! Không có key (và không trỏ endpoint local) → lỗi `DependencyMissing` rõ ràng,
//! KHÔNG âm thầm bỏ trang. Nội dung ảnh được gửi tới provider đã cấu hình.

use std::io;
use std::path::Path;

use image::{DynamicImage, ImageReader, Limits};

use crate::diagnostics::{ConvertErrorKind, DetailedConvertError};

/// Endpoint mặc định: OpenRouter (OpenAI-compatible).
pub const DEFAULT_VISION_OCR_BASE_URL: &str = "https://openrouter.ai/api";
/// Model vision mặc định cho OCR (override qua `FILECONV_OCR_MODEL`).
pub const DEFAULT_VISION_OCR_MODEL: &str = "qwen/qwen3.7-flash";
/// Timeout tổng mặc định cho một trang (giây).
pub const DEFAULT_VISION_OCR_TIMEOUT_SECS: u64 = 180;

/// Cấu hình vision OCR. `Debug` che API key.
#[derive(Clone)]
pub struct VisionOcrConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// `None` → dùng [`default_vision_ocr_system_prompt`].
    pub system_prompt: Option<String>,
    pub timeout_secs: u64,
}

impl std::fmt::Debug for VisionOcrConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisionOcrConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("system_prompt", &self.system_prompt.is_some())
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl Default for VisionOcrConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_VISION_OCR_BASE_URL.to_string(),
            model: DEFAULT_VISION_OCR_MODEL.to_string(),
            api_key: String::new(),
            system_prompt: None,
            timeout_secs: DEFAULT_VISION_OCR_TIMEOUT_SECS,
        }
    }
}

impl VisionOcrConfig {
    /// Đọc cấu hình từ env. API key fallback sang `FILECONV_LLM_API_KEY` để
    /// người dùng một provider (OpenRouter) không phải đặt key hai lần.
    pub fn from_env() -> Self {
        let read = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        Self {
            base_url: read("FILECONV_OCR_BASE_URL")
                .unwrap_or_else(|| DEFAULT_VISION_OCR_BASE_URL.to_string()),
            model: read("FILECONV_OCR_MODEL")
                .unwrap_or_else(|| DEFAULT_VISION_OCR_MODEL.to_string()),
            api_key: read("FILECONV_OCR_API_KEY")
                .or_else(|| read("FILECONV_LLM_API_KEY"))
                .unwrap_or_default(),
            system_prompt: read("FILECONV_OCR_SYSTEM_PROMPT"),
            timeout_secs: read("FILECONV_OCR_TIMEOUT_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_VISION_OCR_TIMEOUT_SECS),
        }
    }

    /// Đã cấu hình = có API key, hoặc chủ đích trỏ endpoint khác mặc định
    /// (server vision local không cần key).
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty() || self.base_url != DEFAULT_VISION_OCR_BASE_URL
    }
}

/// Per-call OCR configuration.
#[derive(Debug, Clone, Default)]
pub struct OcrRunConfig {
    /// Override vision OCR; `None` = đọc từ env (`FILECONV_OCR_*`).
    pub vision: Option<VisionOcrConfig>,
}

/// Resolve cấu hình hiệu lực cho một lần chạy.
pub fn effective_vision_config(config: &OcrRunConfig) -> VisionOcrConfig {
    config
        .vision
        .clone()
        .unwrap_or_else(VisionOcrConfig::from_env)
}

/// Vision OCR dùng được không (feature `llm` + cấu hình đủ).
pub fn vision_ocr_available(config: &OcrRunConfig) -> bool {
    cfg!(feature = "llm") && effective_vision_config(config).is_configured()
}

/// Pipeline stage where an OCR attempt failed (stable, not localized).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrStage {
    Decode,
    Bounds,
    Encode,
    Render,
    Vision,
}

impl OcrStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decode => "decode",
            Self::Bounds => "bounds",
            Self::Encode => "encode",
            Self::Render => "render",
            Self::Vision => "vision",
        }
    }
}

/// Typed OCR attempt failure returned explicitly to callers (PDF/image).
#[derive(Debug, Clone)]
pub enum OcrAttemptError {
    /// Vision OCR chưa cấu hình (thiếu API key / build thiếu feature `llm`).
    NotConfigured { stage: OcrStage, message: String },
    /// Other OCR/decode/render/API failure.
    Failed {
        stage: OcrStage,
        message: String,
        io_kind: io::ErrorKind,
    },
}

impl std::fmt::Display for OcrAttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured { stage, message } | Self::Failed { stage, message, .. } => {
                write!(f, "OCR {}: {message}", stage.as_str())
            }
        }
    }
}

impl std::error::Error for OcrAttemptError {}

impl OcrAttemptError {
    pub fn stage(&self) -> OcrStage {
        match self {
            Self::NotConfigured { stage, .. } | Self::Failed { stage, .. } => *stage,
        }
    }

    /// Maps to [`ConvertErrorKind`] for detailed convert surfaces.
    pub fn kind(&self) -> ConvertErrorKind {
        match self {
            Self::NotConfigured { .. } => ConvertErrorKind::DependencyMissing,
            Self::Failed { .. } => ConvertErrorKind::Failed,
        }
    }

    pub fn convert_kind(&self) -> ConvertErrorKind {
        self.kind()
    }

    pub fn to_detailed(self) -> DetailedConvertError {
        match self {
            Self::NotConfigured { stage, message } => DetailedConvertError::dependency_missing(
                format!("OCR {}: {message}", stage.as_str()),
            ),
            Self::Failed { stage, message, .. } => {
                DetailedConvertError::failed(format!("OCR {}: {message}", stage.as_str()))
            }
        }
    }

    pub fn into_io(self) -> io::Error {
        match self {
            Self::NotConfigured { stage, message } => io::Error::new(
                io::ErrorKind::NotFound,
                OcrNotConfiguredError {
                    message: format!("OCR {}: {message}", stage.as_str()),
                },
            ),
            Self::Failed {
                stage,
                message,
                io_kind,
            } => io::Error::new(io_kind, format!("OCR {}: {message}", stage.as_str())),
        }
    }

    pub(crate) fn failed(stage: OcrStage, error: impl std::fmt::Display) -> Self {
        Self::Failed {
            stage,
            message: error.to_string(),
            io_kind: io::ErrorKind::Other,
        }
    }

    pub(crate) fn from_io(stage: OcrStage, error: io::Error) -> Self {
        Self::Failed {
            stage,
            message: error.to_string(),
            io_kind: error.kind(),
        }
    }

    fn not_configured(message: impl Into<String>) -> Self {
        Self::NotConfigured {
            stage: OcrStage::Vision,
            message: message.into(),
        }
    }
}

/// Marker set when OCR failed because the vision provider is not configured.
#[derive(Debug)]
struct OcrNotConfiguredError {
    message: String,
}

impl std::fmt::Display for OcrNotConfiguredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OcrNotConfiguredError {}

/// True only when the error was tagged as vision-OCR-not-configured.
pub fn error_is_ocr_not_configured(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<OcrNotConfiguredError>())
}

/// Cạnh dài tối đa gửi provider; lớn hơn ⇒ thu xuống (tiết kiệm token/băng thông,
/// không giảm chất lượng OCR đáng kể so với 300 DPI đầy đủ).
const MAX_LONG_SIDE: u32 = 2400;
/// Cạnh tối đa khi decode (strict, qua `image::Limits`). Cho phép trang lớn
/// nhưng chặn decompression bomb. Giữ `Limits::default().max_alloc` (512 MiB).
const MAX_DECODE_SIDE: u32 = 12_000;
/// Chất lượng JPEG gửi provider (fidelity cao cho chữ nhỏ/dấu tiếng Việt).
const VISION_JPEG_QUALITY: u8 = 90;

/// Limits decode OCR: giữ max_alloc mặc định của `image`, thêm trần cạnh.
fn ocr_image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits
}

fn image_error_to_io(error: image::ImageError) -> io::Error {
    match error {
        image::ImageError::IoError(inner) => inner,
        image::ImageError::Limits(_) => {
            io::Error::new(io::ErrorKind::InvalidData, error.to_string())
        }
        other => io::Error::other(other.to_string()),
    }
}

fn is_image_limit_error(error: &image::ImageError) -> bool {
    matches!(error, image::ImageError::Limits(_))
}

fn ensure_ocr_image_bounds(width: u32, height: u32) -> io::Result<()> {
    ocr_image_limits()
        .check_dimensions(width, height)
        .map_err(image_error_to_io)
}

/// Mở ảnh với giới hạn dimension/alloc **trước** khi decode đầy đủ buffer.
fn load_image_for_ocr(path: &Path) -> Result<DynamicImage, image::ImageError> {
    let mut reader = ImageReader::open(path)?;
    reader.limits(ocr_image_limits());
    reader.decode()
}

/// Chuẩn hoá kích thước (giữ màu — con dấu/highlight có ý nghĩa) rồi encode JPEG.
fn encode_for_vision(img: &DynamicImage) -> Result<Vec<u8>, OcrAttemptError> {
    let (w, h) = (img.width(), img.height());
    let long = w.max(h);
    let resized;
    let source = if long > MAX_LONG_SIDE {
        let f = MAX_LONG_SIDE as f32 / long as f32;
        resized = img.resize(
            ((w as f32 * f).round() as u32).max(1),
            ((h as f32 * f).round() as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        );
        &resized
    } else {
        img
    };
    let rgb = source.to_rgb8();
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, VISION_JPEG_QUALITY);
    rgb.write_with_encoder(encoder)
        .map_err(|error| OcrAttemptError::from_io(OcrStage::Encode, image_error_to_io(error)))?;
    Ok(buf)
}

/// System prompt mặc định — tổng quát cho cả văn bản pháp luật lẫn tài liệu
/// dự án phần mềm (BRD/SRS/testcase/API); tham chiếu prompt product owner duyệt.
pub fn default_vision_ocr_system_prompt(langs: &str) -> String {
    format!(
        "Bạn là hệ thống OCR tài liệu, không phải trợ lý tóm tắt. Chép trung thực \
         toàn bộ chữ nhìn thấy trong ảnh theo thứ tự đọc từ trên xuống dưới, trái \
         sang phải; tài liệu nhiều cột phải theo đúng thứ tự đọc từng cột. \
         Không diễn giải, rút gọn, sửa nghĩa hoặc tự bổ sung.\n\
         Bắt buộc giữ: metadata ký số, số trang in, tiêu đề, chương/điều/khoản/điểm, \
         heading, danh sách, mã định danh và số hiệu (ví dụ 89/2026/TT-BTC, \
         BR-PAY-022, REQ-0014, TC-PAY-034, POST /payments), bảng kể cả ô gộp, \
         công thức, code và chú thích. Không bỏ nội dung vì cho rằng không quan \
         trọng. Nếu không chắc một đoạn, ghi `[không rõ: ...]`, không đoán.\n\
         Ngôn ngữ dự kiến của tài liệu: {}. Chỉ xuất Markdown, không thêm nhận \
         xét, không bọc toàn bộ nội dung trong code fence.",
        describe_langs(langs)
    )
}

fn describe_langs(langs: &str) -> String {
    let mapped: Vec<&str> = langs
        .split('+')
        .map(|code| match code.trim() {
            "vie" | "vi" => "tiếng Việt",
            "eng" | "en" => "tiếng Anh",
            other => other,
        })
        .filter(|value| !value.is_empty())
        .collect();
    if mapped.is_empty() {
        "tiếng Việt và tiếng Anh".to_string()
    } else {
        mapped.join(" và ")
    }
}

/// Model hay bọc kết quả trong ```markdown fence dù đã dặn — gỡ fence bao ngoài.
fn strip_wrapping_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(first_line_end) = trimmed.find('\n') else {
        return trimmed.to_string();
    };
    let first_line = trimmed[..first_line_end].trim();
    if !first_line.starts_with("```") {
        return trimmed.to_string();
    }
    let rest = &trimmed[first_line_end + 1..];
    let Some(closing) = rest.rfind("```") else {
        return trimmed.to_string();
    };
    if !rest[closing + 3..].trim().is_empty() {
        return trimmed.to_string();
    }
    rest[..closing].trim().to_string()
}

/// OCR một file ảnh. `langs` ví dụ "vie+eng".
///
/// Legacy `io::Result` surface. Prefer [`ocr_image_detailed`] for typed errors.
pub fn ocr_image(path: &Path, langs: &str) -> io::Result<String> {
    ocr_image_detailed(path, langs, &OcrRunConfig::default()).map_err(OcrAttemptError::into_io)
}

/// Additive detailed image OCR with typed attempt errors.
pub fn ocr_image_detailed(
    path: &Path,
    langs: &str,
    config: &OcrRunConfig,
) -> Result<String, OcrAttemptError> {
    match load_image_for_ocr(path) {
        Ok(img) => ocr_dynimage_detailed(&img, langs, config),
        // Vượt giới hạn kích thước/alloc → fail rõ, KHÔNG gửi bomb cho provider.
        Err(error) if is_image_limit_error(&error) => Err(OcrAttemptError::from_io(
            OcrStage::Bounds,
            image_error_to_io(error),
        )),
        Err(error) => Err(OcrAttemptError::from_io(
            OcrStage::Decode,
            image_error_to_io(error),
        )),
    }
}

/// OCR một ảnh đã có trong bộ nhớ (vd trang PDF render ra).
pub fn ocr_dynimage(img: &DynamicImage, langs: &str) -> io::Result<String> {
    ocr_dynimage_detailed(img, langs, &OcrRunConfig::default()).map_err(OcrAttemptError::into_io)
}

/// Additive detailed in-memory OCR with typed attempt errors.
pub fn ocr_dynimage_detailed(
    img: &DynamicImage,
    langs: &str,
    config: &OcrRunConfig,
) -> Result<String, OcrAttemptError> {
    ensure_ocr_image_bounds(img.width(), img.height())
        .map_err(|error| OcrAttemptError::from_io(OcrStage::Bounds, error))?;
    let vision = effective_vision_config(config);
    if !vision.is_configured() {
        return Err(OcrAttemptError::not_configured(
            "vision OCR chưa cấu hình; đặt FILECONV_OCR_API_KEY (OpenRouter) \
             hoặc FILECONV_OCR_BASE_URL cho server vision local",
        ));
    }
    let jpeg = encode_for_vision(img)?;
    let text = call_vision_api(&vision, &jpeg, langs)?;
    Ok(strip_wrapping_fence(&text))
}

#[cfg(feature = "llm")]
fn call_vision_api(
    cfg: &VisionOcrConfig,
    jpeg: &[u8],
    langs: &str,
) -> Result<String, OcrAttemptError> {
    crate::llm::vision_ocr_jpeg(cfg, jpeg, langs)
        .map_err(|error| OcrAttemptError::failed(OcrStage::Vision, error))
}

#[cfg(not(feature = "llm"))]
fn call_vision_api(
    _cfg: &VisionOcrConfig,
    _jpeg: &[u8],
    _langs: &str,
) -> Result<String, OcrAttemptError> {
    Err(OcrAttemptError::not_configured(
        "fileconv-core được build không có feature `llm`; vision OCR cần feature này",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GrayImage;

    fn unconfigured() -> OcrRunConfig {
        OcrRunConfig {
            vision: Some(VisionOcrConfig::default()),
        }
    }

    #[test]
    fn default_config_is_not_configured_until_key_or_custom_endpoint() {
        let mut cfg = VisionOcrConfig::default();
        assert!(!cfg.is_configured());
        cfg.api_key = "sk-test".into();
        assert!(cfg.is_configured());
        let local = VisionOcrConfig {
            api_key: String::new(),
            base_url: "http://127.0.0.1:11434".into(),
            ..VisionOcrConfig::default()
        };
        assert!(
            local.is_configured(),
            "endpoint local chủ đích không cần key"
        );
    }

    #[test]
    fn debug_never_prints_api_key() {
        let cfg = VisionOcrConfig {
            api_key: "sk-or-super-secret".into(),
            ..VisionOcrConfig::default()
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("sk-or-super-secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn unconfigured_ocr_is_typed_dependency_missing() {
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(32, 32, image::Luma([40])));
        let err = ocr_dynimage_detailed(&img, "vie+eng", &unconfigured())
            .expect_err("missing key must fail");
        assert!(matches!(
            err,
            OcrAttemptError::NotConfigured {
                stage: OcrStage::Vision,
                ..
            }
        ));
        assert_eq!(err.kind(), ConvertErrorKind::DependencyMissing);
        let detailed = err.clone().to_detailed();
        assert_eq!(detailed.kind, ConvertErrorKind::DependencyMissing);
        let io_err = err.into_io();
        assert_eq!(io_err.kind(), io::ErrorKind::NotFound);
        assert!(error_is_ocr_not_configured(&io_err));
    }

    #[test]
    fn system_prompt_is_faithful_transcription_contract() {
        let prompt = default_vision_ocr_system_prompt("vie+eng");
        for required in [
            "không phải trợ lý tóm tắt",
            "Không diễn giải",
            "[không rõ: ...]",
            "chương/điều/khoản/điểm",
            "ô gộp",
            "tiếng Việt và tiếng Anh",
            "Chỉ xuất Markdown",
        ] {
            assert!(prompt.contains(required), "prompt thiếu: {required}");
        }
        assert!(default_vision_ocr_system_prompt("eng").contains("tiếng Anh"));
        assert!(default_vision_ocr_system_prompt("").contains("tiếng Việt và tiếng Anh"));
    }

    #[test]
    fn strips_wrapping_code_fence_only() {
        assert_eq!(
            strip_wrapping_fence("```markdown\n# Tiêu đề\nNội dung\n```"),
            "# Tiêu đề\nNội dung"
        );
        assert_eq!(strip_wrapping_fence("```\ntext\n```"), "text");
        let inner_fence = "Đoạn văn\n\n```rust\nfn main() {}\n```\n\nKết";
        assert_eq!(strip_wrapping_fence(inner_fence), inner_fence);
        assert_eq!(strip_wrapping_fence("một dòng"), "một dòng");
    }

    #[test]
    fn encode_for_vision_downscales_and_produces_jpeg() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4800,
            2400,
            image::Rgba([200, 10, 10, 255]),
        ));
        let jpeg = encode_for_vision(&img).expect("encode");
        // JPEG SOI marker; alpha phải được bỏ (JPEG không hỗ trợ RGBA).
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let decoded = image::load_from_memory(&jpeg).expect("decode lại");
        assert!(decoded.width() <= MAX_LONG_SIDE && decoded.height() <= MAX_LONG_SIDE);
    }

    #[test]
    fn decode_limits_keep_image_default_alloc_and_allow_scanned_pages() {
        let limits = ocr_image_limits();
        assert_eq!(limits.max_alloc, Limits::default().max_alloc);
        assert_eq!(limits.max_image_width, Some(MAX_DECODE_SIDE));
        assert_eq!(limits.max_image_height, Some(MAX_DECODE_SIDE));
        // A4 @ 300 DPI and A3 @ ~600 DPI must remain acceptable.
        assert!(ensure_ocr_image_bounds(2480, 3508).is_ok());
        assert!(ensure_ocr_image_bounds(4961, 7016).is_ok());
        assert!(ensure_ocr_image_bounds(MAX_DECODE_SIDE, MAX_DECODE_SIDE).is_ok());
        assert!(ensure_ocr_image_bounds(MAX_DECODE_SIDE + 1, 100).is_err());
        assert!(ensure_ocr_image_bounds(100, MAX_DECODE_SIDE + 1).is_err());
    }

    #[test]
    fn load_image_rejects_oversized_png_header_before_full_decode() {
        // IHDR-only PNG claiming 20000×20000 — dimension check fails before pixel decode.
        let bytes = hex_literal_png_ihdr(20_000, 20_000);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bomb.png");
        std::fs::write(&path, bytes).expect("write bomb png");
        let err = load_image_for_ocr(&path).expect_err("oversized header must fail");
        assert!(is_image_limit_error(&err), "got {err:?}");
    }

    #[test]
    fn ocr_image_rejects_dimension_bomb_before_any_network_call() {
        // Limit path must fail before provider call (no config needed).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bomb.png");
        std::fs::write(&path, hex_literal_png_ihdr(20_000, 20_000)).expect("write");
        let err = ocr_image(&path, "eng").expect_err("limit must surface");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let detailed = ocr_image_detailed(&path, "eng", &unconfigured())
            .expect_err("limit must surface as typed OCR error");
        assert!(
            matches!(
                detailed,
                OcrAttemptError::Failed {
                    stage: OcrStage::Bounds,
                    io_kind: io::ErrorKind::InvalidData,
                    ..
                }
            ),
            "got {detailed:?}"
        );
        assert_eq!(detailed.kind(), ConvertErrorKind::Failed);
        assert_eq!(detailed.stage(), OcrStage::Bounds);
        assert!(
            detailed.to_string().to_ascii_lowercase().contains("limit")
                || detailed.to_string().contains("dimension"),
            "limit detail preserved: {detailed}"
        );
    }

    #[test]
    fn undecodable_image_is_typed_decode_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("corrupt.png");
        std::fs::write(&path, b"not an image at all").expect("write");
        let err = ocr_image_detailed(&path, "vie", &unconfigured()).expect_err("must fail");
        assert_eq!(err.stage(), OcrStage::Decode);
    }

    /// Minimal PNG with only an IHDR chunk (precomputed CRC for given size).
    fn hex_literal_png_ihdr(width: u32, height: u32) -> Vec<u8> {
        // Signature + IHDR length/type/data/CRC for 20000×20000 grayscale.
        match (width, height) {
            (20_000, 20_000) => vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
                0x44, 0x52, 0x00, 0x00, 0x4e, 0x20, 0x00, 0x00, 0x4e, 0x20, 0x08, 0x00, 0x00, 0x00,
                0x00, 0xc6, 0x1b, 0x19, 0xe5,
            ],
            _ => panic!("add CRC fixture for {width}x{height}"),
        }
    }
}
