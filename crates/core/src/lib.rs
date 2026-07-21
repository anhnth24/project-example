//! fileconv-core: lõi chuyển đổi tài liệu/ảnh sang Markdown — viết lại từ đầu.
//!
//! Khác với phase trước (bọc markitdown-rs), bản này gọi THẲNG các crate gốc và
//! sửa các lỗi đã phát hiện trong benchmark:
//!   - html: dùng `htmd` (html5ever) thay `html2md` để tránh phình output.
//!   - xlsx: đọc TẤT CẢ sheet (calamine), không chỉ sheet đầu.
//!   - docx: phát hiện heading qua style, xuất `#`/bảng Markdown.
//!   - pptx: đọc slide theo ĐÚNG thứ tự số.
//!   - bỏ toàn bộ `println!` debug và dependency LLM nặng (rig-core/tokio).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
#[cfg(feature = "audio")]
use std::sync::OnceLock;

#[cfg(feature = "audio")]
pub mod audio;
pub mod chunk;
mod conv;
pub mod image_ocr;
pub mod intelligence;
#[cfg(test)]
mod intelligence_tests;
#[cfg(feature = "llm")]
pub mod llm;
#[cfg(feature = "llm")]
pub mod llm_cli;
pub mod pptx_preview;
pub mod probe;
mod proc;
pub mod tables;
pub mod viet_legacy;
mod viet_legacy_maps;

pub use probe::{probe, FileInfo};

#[cfg(feature = "audio")]
use audio::AudioEngine;

/// Loại định dạng nhận diện được.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatKind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Csv,
    Html,
    Text,
    Image,
    Audio,
    Unknown,
}

impl FormatKind {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "pdf" => Self::Pdf,
            "docx" => Self::Docx,
            "pptx" => Self::Pptx,
            "xlsx" | "xls" | "xlsb" | "ods" => Self::Xlsx,
            "csv" => Self::Csv,
            "html" | "htm" => Self::Html,
            "txt" | "log" | "md" | "markdown" => Self::Text,
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "gif" => Self::Image,
            "wav" | "mp3" | "m4a" | "flac" | "ogg" => Self::Audio,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
            Self::Xlsx => "xlsx",
            Self::Csv => "csv",
            Self::Html => "html",
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Unknown => "unknown",
        }
    }

    pub fn supported_extensions() -> &'static [&'static str] {
        &[
            "pdf", "docx", "pptx", "xlsx", "xls", "xlsb", "ods", "csv", "html", "htm", "png",
            "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "wav", "mp3", "m4a", "flac", "ogg",
            "txt", "log", "md", "markdown",
        ]
    }
}

/// Stable machine-readable warning codes (`ConversionWarning::code`).
pub mod warning_codes {
    /// `needs_ocr` page kept untrusted pdf-inspector text after OCR/native recovery failed.
    pub const PDF_UNTRUSTED_TEXT_FALLBACK: &str = "pdf_untrusted_text_fallback";
}

/// Structured conversion diagnostic for partial / degraded success paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionWarning {
    /// Stable code for programmatic handling (see [`warning_codes`]).
    pub code: &'static str,
    /// Converter / recovery stage that emitted the warning (stable, not localized).
    pub source: &'static str,
    /// 1-indexed page when the warning is page-scoped.
    pub page: Option<u32>,
    /// Human-readable detail (Vietnamese UI/logs). Preserved as authored — not reclassified.
    pub message: String,
}

/// Outcome of a successful conversion (`Err` is hard failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionOutcome {
    /// Trusted extraction paths only; no degraded recovery warnings.
    FullSuccess,
    /// Markdown produced, but at least one degraded recovery was recorded.
    PartialSuccess,
}

#[derive(Debug, Clone)]
pub struct ConversionResult {
    pub markdown: String,
    pub title: Option<String>,
    pub format: FormatKind,
    /// Machine-readable soft diagnostics. Empty when [`outcome`] is [`FullSuccess`].
    pub warnings: Vec<ConversionWarning>,
    pub outcome: ConversionOutcome,
}

impl ConversionResult {
    /// True when conversion returned markdown with one or more degraded recoveries.
    pub fn is_partial_success(&self) -> bool {
        self.outcome == ConversionOutcome::PartialSuccess
    }

    /// True when any warning carries the given stable code.
    pub fn has_warning_code(&self, code: &str) -> bool {
        self.warnings.iter().any(|warning| warning.code == code)
    }
}

/// Hard conversion failure.
///
/// Existing variants (`BadPath`, `Unsupported`, `Failed`) stay stable. New typed
/// variants are additive; prefer them only when the call site has clear evidence
/// (do not parse opaque error strings to guess a category). `Failed` remains the
/// compatibility catch-all / migration path for unclassified failures.
///
/// `#[non_exhaustive]` forces a wildcard arm so future taxonomy additions do not
/// break downstream exhaustive matches.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    #[error("đường dẫn không hợp lệ (non-UTF8)")]
    BadPath,
    #[error("định dạng chưa hỗ trợ: {0}")]
    Unsupported(&'static str),
    #[error("chuyển đổi thất bại: {0}")]
    Failed(String),
    #[error("đầu vào hỏng hoặc không hợp lệ: {0}")]
    CorruptInput(String),
    #[error("thiếu phụ thuộc: {0}")]
    DependencyMissing(String),
    #[error("hết thời gian chờ: {0}")]
    Timeout(String),
    #[error("thiếu tài nguyên: {0}")]
    Resource(String),
    #[error("lỗi toàn vẹn dữ liệu: {0}")]
    Integrity(String),
    #[error("lỗi nhà cung cấp: {0}")]
    Provider(String),
    #[error("lỗi OCR: {0}")]
    Ocr(String),
}

impl ConvertError {
    /// Stable machine-readable error code (does not change with message text).
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadPath => "bad_path",
            Self::Unsupported(_) => "unsupported",
            Self::Failed(_) => "failed",
            Self::CorruptInput(_) => "corrupt_input",
            Self::DependencyMissing(_) => "dependency_missing",
            Self::Timeout(_) => "timeout",
            Self::Resource(_) => "resource",
            Self::Integrity(_) => "integrity",
            Self::Provider(_) => "provider",
            Self::Ocr(_) => "ocr",
        }
    }

    /// Map an image-OCR `io::Error` using only `ErrorKind` evidence.
    ///
    /// Opaque / unclassified failures stay [`ConvertError::Ocr`] with the original
    /// display text — never string-match messages into other taxonomy buckets.
    pub(crate) fn from_ocr_io(error: std::io::Error) -> Self {
        let message = error.to_string();
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::DependencyMissing(format!(
                "không tìm thấy Tesseract OCR (hoặc binary FILECONV_TESSERACT): {message}"
            )),
            std::io::ErrorKind::TimedOut => Self::Timeout(message),
            std::io::ErrorKind::OutOfMemory => Self::Resource(message),
            _ => Self::Ocr(message),
        }
    }
}

thread_local! {
    static CONVERSION_WARNINGS: RefCell<Vec<ConversionWarning>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn clear_conversion_warnings() {
    CONVERSION_WARNINGS.with(|warnings| warnings.borrow_mut().clear());
}

pub(crate) fn push_conversion_warning(warning: ConversionWarning) {
    CONVERSION_WARNINGS.with(|warnings| warnings.borrow_mut().push(warning));
}

pub(crate) fn take_conversion_warnings() -> Vec<ConversionWarning> {
    CONVERSION_WARNINGS.with(|warnings| std::mem::take(&mut *warnings.borrow_mut()))
}

#[derive(Debug, Clone)]
pub struct ConverterOptions {
    /// Ngôn ngữ OCR cho ảnh (mặc định "vie+eng").
    pub ocr_langs: String,
    /// Tesseract mặc định; Paddle/Auto là tier tùy chọn.
    pub ocr_engine: image_ocr::OcrEngine,
    /// Đường dẫn model whisper GGML cho audio (None = audio chưa khả dụng).
    pub whisper_model: Option<PathBuf>,
    /// Ngôn ngữ audio (mặc định "vi").
    pub audio_lang: String,
    /// Số thread cho whisper (mặc định 4).
    pub audio_threads: i32,
    /// Bỏ segment có xác suất không lời >= ngưỡng này (mặc định 0.6).
    pub audio_no_speech_threshold: f32,
    /// Bật OCR cho TRANG scan (không/ít lớp text). Mặc định true.
    pub pdf_ocr: bool,
    /// Bật OCR thêm cho ẢNH NHÚNG lớn trong trang có text (trang trộn).
    /// Mặc định false vì có thể chậm/nhiễu với tài liệu nhiều hình.
    pub pdf_ocr_images: bool,
    /// Chỉ trích các trang PDF này (1-indexed). None = mọi trang. (Giảm token.)
    pub pdf_pages: Option<Vec<u32>>,
    /// Chỉ trích sheet này của xlsx (theo tên). None = mọi sheet.
    pub xlsx_sheet: Option<String>,
    /// Cắt Markdown ở tối đa N ký tự (kèm chú thích phần bị cắt). None = không cắt.
    pub max_chars: Option<usize>,
}

impl Default for ConverterOptions {
    fn default() -> Self {
        Self {
            ocr_langs: "vie+eng".to_string(),
            ocr_engine: image_ocr::OcrEngine::Tesseract,
            whisper_model: None,
            audio_lang: "vi".to_string(),
            audio_threads: 4,
            audio_no_speech_threshold: 0.6,
            pdf_ocr: true,
            pdf_ocr_images: false,
            pdf_pages: None,
            xlsx_sheet: None,
            max_chars: None,
        }
    }
}

/// Backend chuyển đổi. Model whisper được **cache** sau lần load đầu (OnceLock),
/// nên convert nhiều file audio không phải load lại model.
pub struct Converter {
    opts: ConverterOptions,
    #[cfg(feature = "audio")]
    engine: OnceLock<AudioEngine>,
}

impl Default for Converter {
    fn default() -> Self {
        Self::new()
    }
}

impl Converter {
    pub fn new() -> Self {
        Self::with_options(ConverterOptions::default())
    }

    pub fn with_options(opts: ConverterOptions) -> Self {
        Self {
            opts,
            #[cfg(feature = "audio")]
            engine: OnceLock::new(),
        }
    }

    /// Lấy AudioEngine, load model một lần rồi cache lại.
    #[cfg(feature = "audio")]
    fn engine(&self) -> Result<&AudioEngine, ConvertError> {
        if let Some(e) = self.engine.get() {
            return Ok(e);
        }
        let model = self
            .opts
            .whisper_model
            .clone()
            .or_else(audio::discover_whisper_model)
            .ok_or(ConvertError::Unsupported(
                "audio: chưa cài hoặc cấu hình whisper_model",
            ))?;
        let eng = AudioEngine::load(&model)?
            .with_threads(self.opts.audio_threads)
            .with_no_speech_threshold(self.opts.audio_no_speech_threshold);
        // Nếu thread khác set trước, bỏ qua bản của ta (vẫn dùng bản đã cache).
        let _ = self.engine.set(eng);
        Ok(self.engine.get().unwrap())
    }

    /// Chuyển một file sang Markdown.
    ///
    /// Soft degradation (e.g. PDF `needs_ocr` page kept untrusted inspector text)
    /// returns [`Ok`] with [`ConversionOutcome::PartialSuccess`] and structured
    /// [`ConversionWarning`]s. Hard failures remain [`Err`].
    pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError> {
        clear_conversion_warnings();
        let format = FormatKind::from_path(path);
        let md = match image_ocr::with_ocr_engine(self.opts.ocr_engine, || match format {
            FormatKind::Pdf => conv::pdf::to_markdown(
                path,
                &self.opts.ocr_langs,
                self.opts.pdf_ocr,
                self.opts.pdf_ocr_images,
                self.opts.pdf_pages.as_deref(),
            ),
            FormatKind::Docx => conv::docx::to_markdown(path),
            FormatKind::Pptx => conv::pptx::to_markdown(path),
            FormatKind::Xlsx => conv::xlsx::to_markdown(path, self.opts.xlsx_sheet.as_deref()),
            FormatKind::Csv => conv::csv_conv::to_markdown(path),
            FormatKind::Html => conv::html::to_markdown(path),
            FormatKind::Text => conv::text::to_markdown(path),
            FormatKind::Image => {
                image_ocr::ocr_image(path, &self.opts.ocr_langs).map_err(ConvertError::from_ocr_io)
            }
            FormatKind::Audio => {
                #[cfg(feature = "audio")]
                {
                    self.engine()?
                        .transcribe_file(path, Some(&self.opts.audio_lang))
                        .map(|t| t.text)
                }
                #[cfg(not(feature = "audio"))]
                {
                    Err(ConvertError::Unsupported(
                        "audio: build này không bật feature `audio`",
                    ))
                }
            }
            FormatKind::Unknown => Err(ConvertError::Unsupported("không rõ đuôi file")),
        }) {
            Ok(md) => md,
            Err(error) => {
                clear_conversion_warnings();
                return Err(error);
            }
        };

        // Chuẩn hoá Unicode NFC: tài liệu tiếng Việt cũ (nhất là từ macOS/PDF legacy)
        // hay ở dạng NFD (ê + dấu rời) — gây lệch so khớp/tìm kiếm/embedding dù nhìn
        // giống hệt. Không đối thủ nào xử lý (xem bench/RESEARCH_COMPETITORS.md).
        let md = {
            use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};
            match is_nfc_quick(md.chars()) {
                IsNormalized::Yes => md,
                _ => md.nfc().collect::<String>(),
            }
        };

        // Cắt theo max_chars (giảm token cho file lớn).
        let md = match self.opts.max_chars {
            Some(limit) if md.chars().count() > limit => {
                let kept: String = md.chars().take(limit).collect();
                let remaining = md.chars().count() - limit;
                format!("{kept}\n\n<!-- (đã cắt ở {limit} ký tự, còn {remaining} ký tự) -->\n")
            }
            _ => md,
        };

        let title = title_from_markdown(&md).or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        });

        Ok(finish_conversion_result(md, title, format))
    }
}

/// Attach TLS diagnostics collected during conversion.
pub(crate) fn finish_conversion_result(
    markdown: String,
    title: Option<String>,
    format: FormatKind,
) -> ConversionResult {
    let warnings = take_conversion_warnings();
    let outcome = if warnings.is_empty() {
        ConversionOutcome::FullSuccess
    } else {
        ConversionOutcome::PartialSuccess
    };
    ConversionResult {
        markdown,
        title,
        format,
        warnings,
        outcome,
    }
}

fn title_from_markdown(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        let trimmed = line.trim();
        let hashes = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if !(1..=6).contains(&hashes) {
            return None;
        }
        let title = trimmed[hashes..].trim().trim_matches('#').trim();
        (!title.is_empty() && title.chars().count() <= 240).then(|| title.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tài liệu chứa tiếng Việt dạng NFD (dấu rời) phải ra NFC sau convert.
    #[test]
    fn output_normalized_to_nfc() {
        // "tiếng Việt" ở dạng NFD: e + U+0302 + U+0301, ê tách dấu.
        let nfd = "ti\u{0065}\u{0302}\u{0301}ng Vi\u{0065}\u{0323}\u{0302}t,ok\n";
        let dir = std::env::temp_dir().join(format!("fileconv_nfc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("nfd.csv");
        std::fs::write(&f, nfd).unwrap();

        let out = Converter::new().convert_path(&f).unwrap().markdown;
        assert!(
            out.contains("tiếng"),
            "phải chứa 'tiếng' dạng NFC, got: {out:?}"
        );
        assert!(out.contains("Việt"), "phải chứa 'Việt' dạng NFC");
        // Không còn combining mark rời nào.
        assert!(!out.chars().any(|c| ('\u{0300}'..='\u{036F}').contains(&c)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversion_result_uses_first_heading_as_title() {
        assert_eq!(
            title_from_markdown("<!-- Page 1 -->\n\n# Báo cáo dự án\n\nNội dung"),
            Some("Báo cáo dự án".into())
        );
        assert_eq!(title_from_markdown("nội dung không heading"), None);
    }

    #[test]
    fn convert_error_codes_are_stable() {
        assert_eq!(ConvertError::BadPath.code(), "bad_path");
        assert_eq!(ConvertError::Unsupported("pdf/x").code(), "unsupported");
        assert_eq!(ConvertError::Failed("x".into()).code(), "failed");
        assert_eq!(
            ConvertError::CorruptInput("x".into()).code(),
            "corrupt_input"
        );
        assert_eq!(
            ConvertError::DependencyMissing("x".into()).code(),
            "dependency_missing"
        );
        assert_eq!(ConvertError::Timeout("x".into()).code(), "timeout");
        assert_eq!(ConvertError::Resource("x".into()).code(), "resource");
        assert_eq!(ConvertError::Integrity("x".into()).code(), "integrity");
        assert_eq!(ConvertError::Provider("x".into()).code(), "provider");
        assert_eq!(ConvertError::Ocr("x".into()).code(), "ocr");
    }

    #[test]
    fn convert_error_display_keeps_legacy_failed_prefix() {
        let err = ConvertError::Failed("không đọc được".into());
        assert_eq!(err.to_string(), "chuyển đổi thất bại: không đọc được");
        assert!(ConvertError::BadPath
            .to_string()
            .contains("đường dẫn không hợp lệ"));
        assert!(ConvertError::Integrity("hash lệch".into())
            .to_string()
            .starts_with("lỗi toàn vẹn dữ liệu:"));
    }

    #[test]
    fn partial_ocr_warning_is_surfaced_as_partial_success() {
        clear_conversion_warnings();
        push_conversion_warning(ConversionWarning {
            code: warning_codes::PDF_UNTRUSTED_TEXT_FALLBACK,
            source: "pdf::needs_ocr_untrusted_fallback",
            page: Some(2),
            message: "trang 2: OCR/khôi phục native thất bại".into(),
        });
        let result = finish_conversion_result(
            "<!-- page fallback -->".into(),
            Some("fallback".into()),
            FormatKind::Pdf,
        );
        assert_eq!(result.outcome, ConversionOutcome::PartialSuccess);
        assert!(result.is_partial_success());
        assert_eq!(result.warnings.len(), 1);
        assert!(result.has_warning_code(warning_codes::PDF_UNTRUSTED_TEXT_FALLBACK));
        assert_eq!(
            result.warnings[0].source,
            "pdf::needs_ocr_untrusted_fallback"
        );
        assert_eq!(result.warnings[0].page, Some(2));
        // Partial path must never look like clean success.
        assert_ne!(result.outcome, ConversionOutcome::FullSuccess);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn trusted_text_convert_emits_no_partial_ocr_warning() {
        let dir = std::env::temp_dir().join(format!("fileconv_warn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ok.txt");
        std::fs::write(&path, "nội dung tin cậy").unwrap();
        let trusted = Converter::new().convert_path(&path).unwrap();
        assert_eq!(trusted.outcome, ConversionOutcome::FullSuccess);
        assert!(!trusted.is_partial_success());
        assert!(trusted.warnings.is_empty());
        assert!(!trusted.has_warning_code(warning_codes::PDF_UNTRUSTED_TEXT_FALLBACK));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_ocr_io_uses_error_kind_evidence_only() {
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "tesseract");
        assert_eq!(
            ConvertError::from_ocr_io(missing).code(),
            "dependency_missing"
        );
        let timed_out = std::io::Error::new(std::io::ErrorKind::TimedOut, "ocr timeout");
        let timed = ConvertError::from_ocr_io(timed_out);
        assert_eq!(timed.code(), "timeout");
        assert!(timed.to_string().contains("ocr timeout"));
        let oom = std::io::Error::new(std::io::ErrorKind::OutOfMemory, "oom");
        assert_eq!(ConvertError::from_ocr_io(oom).code(), "resource");
        // Opaque InvalidData must NOT be string-matched into Resource.
        let opaque = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "image exceeds limit dimensions",
        );
        let classified = ConvertError::from_ocr_io(opaque);
        assert_eq!(classified.code(), "ocr");
        assert!(classified
            .to_string()
            .contains("image exceeds limit dimensions"));
        let other = std::io::Error::new(std::io::ErrorKind::Other, "tesseract lỗi: foo");
        assert_eq!(ConvertError::from_ocr_io(other).code(), "ocr");
    }
}
