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
use std::sync::OnceLock;

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
            "txt" | "log" => Self::Text,
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
}

#[derive(Debug, Clone)]
pub struct ConversionResult {
    pub markdown: String,
    pub title: Option<String>,
    pub format: FormatKind,
}

#[derive(Debug, thiserror::Error)]
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

/// Backend chuyển đổi. Model whisper được **cache** sau lần load đầu (OnceLock),
/// nên convert nhiều file audio không phải load lại model.
pub struct Converter {
    opts: ConverterOptions,
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
            engine: OnceLock::new(),
        }
    }

    /// Lấy AudioEngine, load model một lần rồi cache lại.
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
    pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError> {
        let format = FormatKind::from_path(path);
        let md = image_ocr::with_ocr_engine(self.opts.ocr_engine, || match format {
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
            FormatKind::Image => image_ocr::ocr_image(path, &self.opts.ocr_langs)
                .map_err(|e| ConvertError::Failed(e.to_string())),
            FormatKind::Audio => self
                .engine()?
                .transcribe_file(path, Some(&self.opts.audio_lang))
                .map(|t| t.text),
            FormatKind::Unknown => Err(ConvertError::Unsupported("không rõ đuôi file")),
        })?;

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

        Ok(ConversionResult {
            markdown: md,
            title,
            format,
        })
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
}
