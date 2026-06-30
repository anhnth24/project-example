//! fileconv-core: lõi chuyển đổi tài liệu/ảnh/âm thanh sang Markdown.
//!
//! Phase 1 bọc crate `markitdown` (markitdown-rs) cho các định dạng tài liệu
//! (pdf, docx, pptx, xlsx, csv, html). OCR ảnh (Tesseract) và audio (whisper)
//! được bổ sung ở module `image_ocr` (phase sau).

use std::path::Path;

use markitdown::model::ConversionOptions;
use markitdown::MarkItDown;

pub mod image_ocr;

/// Loại định dạng nhận diện được, dùng cho báo cáo & định tuyến.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatKind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Csv,
    Html,
    Image,
    Audio,
    Unknown,
}

impl FormatKind {
    /// Suy ra loại từ phần mở rộng của đường dẫn.
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
            "xlsx" | "xls" => Self::Xlsx,
            "csv" => Self::Csv,
            "html" | "htm" => Self::Html,
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
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Unknown => "unknown",
        }
    }
}

/// Kết quả chuyển đổi.
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
    #[error("không nhận diện được hoặc không hỗ trợ định dạng")]
    Unsupported,
    #[error("chuyển đổi thất bại: {0}")]
    Failed(String),
}

/// Tuỳ chọn cho backend.
#[derive(Debug, Clone)]
pub struct ConverterOptions {
    /// Ngôn ngữ OCR cho ảnh (mặc định "vie+eng" cho tiếng Việt).
    pub ocr_langs: String,
}

impl Default for ConverterOptions {
    fn default() -> Self {
        Self {
            ocr_langs: "vie+eng".to_string(),
        }
    }
}

/// Backend chuyển đổi. Tạo một lần rồi tái sử dụng cho nhiều file.
pub struct Converter {
    md: MarkItDown,
    opts: ConverterOptions,
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
            md: MarkItDown::new(),
            opts,
        }
    }

    /// Chuyển một file sang Markdown.
    pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError> {
        let format = FormatKind::from_path(path);

        // Ảnh: dùng OCR Tesseract (markitdown chỉ đọc EXIF), audio chưa hỗ trợ.
        if format == FormatKind::Image {
            let text = image_ocr::ocr_image(path, &self.opts.ocr_langs)
                .map_err(|e| ConvertError::Failed(e.to_string()))?;
            return Ok(ConversionResult {
                markdown: text,
                title: None,
                format,
            });
        }

        let src = path.to_str().ok_or(ConvertError::BadPath)?;
        // Ép file_extension theo đuôi file để định tuyến đúng converter.
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_ascii_lowercase()));
        let opts = ConversionOptions {
            file_extension: ext,
            url: None,
            llm_client: None,
            llm_model: None,
        };
        let res = self
            .md
            .convert(src, Some(opts))
            .map_err(|e| ConvertError::Failed(e.to_string()))?;
        match res {
            Some(r) => Ok(ConversionResult {
                markdown: r.text_content,
                title: r.title,
                format,
            }),
            None => Err(ConvertError::Unsupported),
        }
    }
}
