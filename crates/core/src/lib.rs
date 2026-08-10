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
    /// Ngôn ngữ OCR cho ảnh (mặc định "vie+eng") — hint cho prompt vision OCR.
    pub ocr_langs: String,
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
        let output = self.convert_format(path, format)?;

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
    use std::path::Path;

    /// Bảng đuôi → FormatKind; phải khớp `FormatKind::from_path` và `supported_extensions`.
    const FROM_PATH_EXTENSION_CASES: &[(&str, FormatKind)] = &[
        ("pdf", FormatKind::Pdf),
        ("docx", FormatKind::Docx),
        ("pptx", FormatKind::Pptx),
        ("xlsx", FormatKind::Xlsx),
        ("xls", FormatKind::Xlsx),
        ("xlsb", FormatKind::Xlsx),
        ("ods", FormatKind::Xlsx),
        ("csv", FormatKind::Csv),
        ("html", FormatKind::Html),
        ("htm", FormatKind::Html),
        ("txt", FormatKind::Text),
        ("log", FormatKind::Text),
        ("md", FormatKind::Text),
        ("markdown", FormatKind::Text),
        ("png", FormatKind::Image),
        ("jpg", FormatKind::Image),
        ("jpeg", FormatKind::Image),
        ("webp", FormatKind::Image),
        ("bmp", FormatKind::Image),
        ("tif", FormatKind::Image),
        ("tiff", FormatKind::Image),
        ("gif", FormatKind::Image),
        ("wav", FormatKind::Audio),
        ("mp3", FormatKind::Audio),
        ("m4a", FormatKind::Audio),
        ("flac", FormatKind::Audio),
        ("ogg", FormatKind::Audio),
    ];

    fn expected_kind_for_ext(ext: &str) -> FormatKind {
        FROM_PATH_EXTENSION_CASES
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|(_, k)| *k)
            .unwrap_or_else(|| {
                panic!("thiếu entry cho đuôi {ext:?} trong FROM_PATH_EXTENSION_CASES")
            })
    }

    fn assert_format(path: &str, expected: FormatKind) {
        assert_eq!(
            FormatKind::from_path(Path::new(path)),
            expected,
            "đường dẫn {path:?} phải map tới {expected:?}"
        );
    }

    /// from_path phải khớp đuôi file với FormatKind
    #[test]
    fn format_kind_from_path_maps_extensions() {
        for (ext, expected) in FROM_PATH_EXTENSION_CASES {
            assert_format(&format!("file.{ext}"), *expected);
        }

        // Case insensitivity (to_ascii_lowercase trên đuôi)
        assert_format("REPORT.PDF", FormatKind::Pdf);
        assert_format("photo.JPG", FormatKind::Image);

        // Path::extension chỉ nhìn tên file cuối, không phụ thuộc thư mục
        assert_format("docs/a/report.pdf", FormatKind::Pdf);

        // Unknown / đuôi lạ
        assert_format("Makefile", FormatKind::Unknown);
        assert_format("legacy.doc", FormatKind::Unknown); // .doc chưa hỗ trợ — không sniff magic-byte
        assert_format("archive.zip", FormatKind::Unknown);
        assert_format("file.tar.gz", FormatKind::Unknown); // chỉ lấy đuôi cuối "gz"
        assert_format("file.", FormatKind::Unknown); // Path::extension → None
    }

    /// Mọi đuôi trong `supported_extensions` phải map đúng FormatKind (không chỉ ≠ Unknown).
    #[test]
    fn supported_extensions_matches_from_path() {
        for ext in FormatKind::supported_extensions() {
            let expected = expected_kind_for_ext(ext);
            assert_eq!(
                FormatKind::from_path(Path::new(&format!("file.{ext}"))),
                expected,
                "đuôi {ext:?} phải map tới {expected:?}"
            );
        }

        for (ext, _) in FROM_PATH_EXTENSION_CASES {
            assert!(
                FormatKind::supported_extensions().contains(ext),
                "đuôi {ext:?} có trong FROM_PATH_EXTENSION_CASES nhưng thiếu trong supported_extensions"
            );
        }
    }

    #[test]
    fn legacy_converter_options_exhaustive_literal_still_compiles() {
        let _ = ConverterOptions {
            ocr_langs: "vie+eng".to_string(),
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
        use unicode_normalization::{is_nfc_quick, IsNormalized};

        // 1) Chuỗi NFD tường minh (không gõ "tiếng" trực tiếp — editor có thể NFC hoá).
        let nfd = "ti\u{0065}\u{0302}\u{0301}ng Vi\u{0065}\u{0323}\u{0302}t,ok\n";
        let nfc_expected = "tiếng Việt,ok\n"; // literal NFC trong source

        // 2) Chứng minh hai form KHÁC nhau (nếu bằng nhau thì test vô nghĩa).
        assert_ne!(nfd.as_bytes(), nfc_expected.as_bytes());
        assert_ne!(nfd, nfc_expected);

        // 3) Input đi qua đường convert thật (CSV → markdown).
        let dir = std::env::temp_dir().join(format!("fileconv_nfc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("nfd.csv");
        std::fs::write(&f, nfd).unwrap();

        let out = Converter::new().convert_path(&f).unwrap().markdown;

        // 4) Output chứa literal NFC (CSV có thể bọc table — chỉ check substring).
        assert!(
            out.contains("tiếng") && out.contains("Việt"),
            "phải chứa 'tiếng'/'Việt' NFC, got: {out:?}"
        );

        // 5) Toàn bộ markdown đã NFC (gate production chạy).
        assert_eq!(
            is_nfc_quick(out.chars()),
            IsNormalized::Yes,
            "output chưa NFC: {out:?}"
        );

        // 6) Không còn combining mark — fail nếu ai đó xoá gate NFC.
        assert!(
            !out.chars().any(|c| ('\u{0300}'..='\u{036F}').contains(&c)),
            "còn dấu rời NFD trong output: {out:?}"
        );

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
    fn title_from_markdown_edge_cases() {
        assert_eq!(
            title_from_markdown("## Mục phụ\n\nNội dung."),
            Some("Mục phụ".into())
        );
        assert_eq!(title_from_markdown("#\n\nbody"), None);
        assert_eq!(title_from_markdown("##   \n\nbody"), None);

        let ok240 = format!("# {}", "a".repeat(240));
        assert_eq!(title_from_markdown(&ok240), Some("a".repeat(240)));
        let over241 = format!("# {}", "a".repeat(241));
        assert_eq!(title_from_markdown(&over241), None);
    }

    #[test]
    fn convert_path_title_heading_and_stem_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "fileconv_title_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let no_heading = dir.join("báo_cáo.txt");
        std::fs::write(&no_heading, "Chỉ có nội dung, không heading.\n").unwrap();
        let conv = Converter::new();
        let legacy = conv.convert_path(&no_heading).unwrap();
        assert_eq!(legacy.title.as_deref(), Some("báo_cáo"));
        let detailed = conv.convert_path_detailed(&no_heading).unwrap();
        assert_eq!(detailed.result.title.as_deref(), Some("báo_cáo"));

        let with_heading = dir.join("ten_file.txt");
        std::fs::write(&with_heading, "# Tiêu đề thật\n\nNội dung.\n").unwrap();
        let legacy = conv.convert_path(&with_heading).unwrap();
        assert_eq!(legacy.title.as_deref(), Some("Tiêu đề thật"));

        let _ = std::fs::remove_dir_all(&dir);
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
        let dep = DetailedConvertError::dependency_missing("vision OCR chưa cấu hình");
        assert_eq!(dep.kind, ConvertErrorKind::DependencyMissing);
        assert!(matches!(dep.error, ConvertError::Failed(_)));
        let internal = DetailedConvertError::internal("pdf-extract panic");
        assert_eq!(internal.kind, ConvertErrorKind::Internal);
        let failed = DetailedConvertError::failed("opaque");
        assert_eq!(failed.kind, ConvertErrorKind::Failed);
        let dto = dep.to_dto();
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["kind"], "dependency_missing");
        assert!(value["message"].as_str().unwrap().contains("OCR"));
        assert!(value.get("message").is_some() && value.get("kind").is_some());
    }

    #[test]
    fn convert_path_rejects_unknown_extension_without_io() {
        let conv = Converter::new();

        for path in [Path::new("file.xyz"), Path::new("Makefile")] {
            let legacy = conv.convert_path(path).unwrap_err();
            assert!(
                matches!(legacy, ConvertError::Unsupported("không rõ đuôi file")),
                "path {path:?}"
            );

            let detailed = conv.convert_path_detailed(path).unwrap_err();
            assert_eq!(
                detailed.kind,
                ConvertErrorKind::Unsupported,
                "path {path:?}"
            );
            assert!(
                matches!(
                    detailed.error,
                    ConvertError::Unsupported("không rõ đuôi file")
                ),
                "path {path:?}"
            );
        }
    }

    #[test]
    fn convert_path_missing_known_extension_returns_failed() {
        use std::panic;

        let missing = std::env::temp_dir().join(format!(
            "fileconv_missing_{}_{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!missing.exists());

        let conv = Converter::new();
        let legacy = conv.convert_path(&missing).unwrap_err();
        match legacy {
            ConvertError::Failed(ref msg) => assert!(!msg.trim().is_empty()),
            other => panic!("expected Failed for missing file, got {other:?}"),
        }

        let detailed = conv.convert_path_detailed(&missing).unwrap_err();
        assert_eq!(detailed.kind, ConvertErrorKind::Failed);
        assert!(matches!(detailed.error, ConvertError::Failed(_)));

        let no_panic = panic::catch_unwind(panic::AssertUnwindSafe(|| conv.convert_path(&missing)));
        assert!(matches!(no_panic, Ok(Err(_))));
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
