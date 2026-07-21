//! fileconv-core: lõi chuyển đổi tài liệu/ảnh sang Markdown — viết lại từ đầu.
//!
//! Khác với phase trước (bọc markitdown-rs), bản này gọi THẲNG các crate gốc và
//! sửa các lỗi đã phát hiện trong benchmark:
//!   - html: dùng `htmd` (html5ever) thay `html2md` để tránh phình output.
//!   - xlsx: đọc TẤT CẢ sheet (calamine), không chỉ sheet đầu.
//!   - docx: phát hiện heading qua style, xuất `#`/bảng Markdown.
//!   - pptx: đọc slide theo ĐÚNG thứ tự số.
//!   - bỏ toàn bộ `println!` debug và dependency LLM nặng (rig-core/tokio).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "audio")]
pub mod audio;
pub mod chunk;
mod conv;
pub mod diagnostics;
/// Always-on embedding runtime-path helpers (ADR 0006). Not gated by `llm`.
pub mod embedding_runtime;
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

pub use diagnostics::{
    ConversionOutcome, ConversionReport, ConversionWarning, ConversionWarningCode,
    ConvertErrorKind, DetailedConvertError, DetailedErrorDto,
};
pub use image_ocr::OcrRunConfig;
pub use probe::{probe, FileInfo};

#[cfg(feature = "audio")]
use audio::AudioEngine;
use diagnostics::MarkdownOutput;

/// Loại định dạng nhận diện được.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Legacy successful conversion payload (exact fields preserved for callers).
///
/// Soft diagnostics live on [`ConversionReport`] from
/// [`Converter::convert_path_detailed`] — not here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub markdown: String,
    pub title: Option<String>,
    pub format: FormatKind,
}

/// Legacy hard conversion failure.
///
/// Variants and exhaustive matching stay stable. Additive kinds live on
/// [`DetailedConvertError`] / [`ConvertErrorKind`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConvertError {
    #[error("đường dẫn không hợp lệ (non-UTF8)")]
    BadPath,
    #[error("định dạng chưa hỗ trợ: {0}")]
    Unsupported(&'static str),
    #[error("chuyển đổi thất bại: {0}")]
    Failed(String),
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

/// Backend chuyển đổi.
///
/// Với feature `audio`, `WhisperContext` được cache **process-wide** theo
/// [`audio::WhisperModelKey`] (canonical path + immutable load knobs). Mỗi
/// `Converter`/request (MCP, desktop) lấy `Arc` từ cache — không reload model.
pub struct Converter {
    opts: ConverterOptions,
    ocr_config: OcrRunConfig,
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
        Self::with_options_and_ocr_config(opts, OcrRunConfig::default())
    }

    /// Build a converter with additive, explicitly threaded OCR process overrides.
    ///
    /// Keeping this configuration separate preserves the exact legacy
    /// [`ConverterOptions`] shape for exhaustive downstream struct literals.
    pub fn with_options_and_ocr_config(opts: ConverterOptions, ocr_config: OcrRunConfig) -> Self {
        Self { opts, ocr_config }
    }

    /// Lấy AudioEngine từ process-wide Whisper cache (cheap `Arc` clone).
    #[cfg(feature = "audio")]
    fn engine(&self) -> Result<AudioEngine, ConvertError> {
        let model = self
            .opts
            .whisper_model
            .clone()
            .or_else(audio::discover_whisper_model)
            .ok_or(ConvertError::Unsupported(
                "audio: chưa cài hoặc cấu hình whisper_model",
            ))?;
        Ok(AudioEngine::load(&model)?
            .with_threads(self.opts.audio_threads)
            .with_no_speech_threshold(self.opts.audio_no_speech_threshold))
    }

    /// Legacy convert: identical `ConversionResult` / `ConvertError` surface.
    ///
    /// Soft diagnostics are available via [`Self::convert_path_detailed`].
    pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError> {
        self.convert_path_detailed(path)
            .map(|report| report.result)
            .map_err(|error| error.error)
    }

    /// Additive detailed convert: explicit warnings + derived outcome.
    ///
    /// Diagnostics are returned on the report — no thread-local collector.
    pub fn convert_path_detailed(
        &self,
        path: &Path,
    ) -> Result<ConversionReport, DetailedConvertError> {
        let format = FormatKind::from_path(path);
        let output =
            image_ocr::with_ocr_engine(self.opts.ocr_engine, || self.convert_format(path, format))?;

        // Chuẩn hoá Unicode NFC: tài liệu tiếng Việt cũ (nhất là từ macOS/PDF legacy)
        // hay ở dạng NFD (ê + dấu rời) — gây lệch so khớp/tìm kiếm/embedding dù nhìn
        // giống hệt. Không đối thủ nào xử lý (xem bench/RESEARCH_COMPETITORS.md).
        let md = {
            use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};
            match is_nfc_quick(output.markdown.chars()) {
                IsNormalized::Yes => output.markdown,
                _ => output.markdown.nfc().collect::<String>(),
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

        Ok(ConversionReport::new(
            ConversionResult {
                markdown: md,
                title,
                format,
            },
            output.warnings,
        ))
    }

    fn convert_format(
        &self,
        path: &Path,
        format: FormatKind,
    ) -> Result<MarkdownOutput, DetailedConvertError> {
        match format {
            FormatKind::Pdf => conv::pdf::to_markdown_detailed(
                path,
                &self.opts.ocr_langs,
                self.opts.pdf_ocr,
                self.opts.pdf_ocr_images,
                self.opts.pdf_pages.as_deref(),
                &self.ocr_config,
            ),
            FormatKind::Docx => conv::docx::to_markdown(path)
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Pptx => conv::pptx::to_markdown(path)
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Xlsx => conv::xlsx::to_markdown(path, self.opts.xlsx_sheet.as_deref())
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Csv => conv::csv_conv::to_markdown(path)
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Html => conv::html::to_markdown(path)
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Text => conv::text::to_markdown(path)
                .map(MarkdownOutput::clean)
                .map_err(DetailedConvertError::from_convert),
            FormatKind::Image => {
                image_ocr::ocr_image_detailed(path, &self.opts.ocr_langs, &self.ocr_config)
                    .map(MarkdownOutput::clean)
                    .map_err(image_ocr::OcrAttemptError::to_detailed)
            }
            FormatKind::Audio => {
                #[cfg(feature = "audio")]
                {
                    self.engine()
                        .map_err(DetailedConvertError::from_convert)?
                        .transcribe_file(path, Some(&self.opts.audio_lang))
                        .map(|t| MarkdownOutput::clean(t.text))
                        .map_err(DetailedConvertError::from_convert)
                }
                #[cfg(not(feature = "audio"))]
                {
                    Err(DetailedConvertError::unsupported(
                        "audio: build này không bật feature `audio`",
                    ))
                }
            }
            FormatKind::Unknown => Err(DetailedConvertError::unsupported("không rõ đuôi file")),
        }
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

    #[test]
    fn legacy_converter_options_exhaustive_literal_still_compiles() {
        let _ = ConverterOptions {
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
        };
    }

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
    fn legacy_convert_error_remains_exhaustive() {
        fn classify(error: &ConvertError) -> &'static str {
            match error {
                ConvertError::BadPath => "bad_path",
                ConvertError::Unsupported(_) => "unsupported",
                ConvertError::Failed(_) => "failed",
            }
        }
        assert_eq!(classify(&ConvertError::BadPath), "bad_path");
        assert_eq!(classify(&ConvertError::Unsupported("x")), "unsupported");
        assert_eq!(classify(&ConvertError::Failed("x".into())), "failed");
        assert_eq!(
            ConvertError::Failed("không đọc được".into()).to_string(),
            "chuyển đổi thất bại: không đọc được"
        );
    }

    #[test]
    fn conversion_report_outcome_is_derived_from_warnings() {
        let result = ConversionResult {
            markdown: "x".into(),
            title: None,
            format: FormatKind::Pdf,
        };
        let clean = ConversionReport::new(result.clone(), vec![]);
        assert_eq!(clean.outcome(), ConversionOutcome::FullSuccess);
        assert!(!clean.is_partial_success());

        let partial = ConversionReport::new(
            result,
            vec![ConversionWarning::pdf_untrusted_text_fallback(
                2,
                "pdf::needs_ocr_untrusted_fallback",
            )],
        );
        assert_eq!(partial.outcome(), ConversionOutcome::PartialSuccess);
        assert!(partial.is_partial_success());
        assert!(partial.has_warning_code(ConversionWarningCode::PdfUntrustedTextFallback));
        assert_ne!(partial.outcome(), ConversionOutcome::FullSuccess);

        let code = serde_json::to_string(&ConversionWarningCode::PdfUntrustedTextFallback).unwrap();
        assert_eq!(code, "\"pdf_untrusted_text_fallback\"");
    }

    #[test]
    fn convert_path_detailed_matches_legacy_markdown_without_warnings_on_trusted_text() {
        let dir = std::env::temp_dir().join(format!("fileconv_warn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ok.txt");
        std::fs::write(&path, "nội dung tin cậy").unwrap();
        let legacy = Converter::new().convert_path(&path).unwrap();
        let detailed = Converter::new().convert_path_detailed(&path).unwrap();
        assert_eq!(legacy.markdown, detailed.result.markdown);
        assert_eq!(legacy.title, detailed.result.title);
        assert_eq!(legacy.format, detailed.result.format);
        assert!(detailed.warnings.is_empty());
        assert_eq!(detailed.outcome(), ConversionOutcome::FullSuccess);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detailed_error_kinds_only_at_exact_stages() {
        let dep = DetailedConvertError::dependency_missing("tesseract spawn NotFound");
        assert_eq!(dep.kind, ConvertErrorKind::DependencyMissing);
        assert!(matches!(dep.error, ConvertError::Failed(_)));
        let internal = DetailedConvertError::internal("pdf-extract panic");
        assert_eq!(internal.kind, ConvertErrorKind::Internal);
        let failed = DetailedConvertError::failed("opaque");
        assert_eq!(failed.kind, ConvertErrorKind::Failed);
        let dto = dep.to_dto();
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["kind"], "dependency_missing");
        assert!(value["message"].as_str().unwrap().contains("tesseract"));
        assert!(value.get("message").is_some() && value.get("kind").is_some());
    }

    #[test]
    fn concurrent_detailed_converts_do_not_leak_warnings() {
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!(
            "fileconv_concurrent_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let trusted = dir.join("trusted.txt");
        std::fs::write(&trusted, "trusted content for concurrent test").unwrap();
        let trusted = Arc::new(trusted);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = Arc::clone(&trusted);
                std::thread::spawn(move || {
                    Converter::new()
                        .convert_path_detailed(path.as_path())
                        .expect("trusted text should convert")
                })
            })
            .collect();
        for handle in handles {
            let report = handle.join().expect("thread");
            assert!(
                report.warnings.is_empty(),
                "no TLS leakage: trusted convert must stay FullSuccess"
            );
            assert_eq!(report.outcome(), ConversionOutcome::FullSuccess);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
