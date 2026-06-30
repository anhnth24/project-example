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

pub mod audio;
mod conv;
pub mod image_ocr;

/// Loại định dạng nhận diện được.
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
    /// Đường dẫn model whisper GGML cho audio (None = audio chưa khả dụng).
    pub whisper_model: Option<PathBuf>,
    /// Ngôn ngữ audio (mặc định "vi").
    pub audio_lang: String,
}

impl Default for ConverterOptions {
    fn default() -> Self {
        Self {
            ocr_langs: "vie+eng".to_string(),
            whisper_model: None,
            audio_lang: "vi".to_string(),
        }
    }
}

/// Backend chuyển đổi.
pub struct Converter {
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
        Self { opts }
    }

    /// Chuyển một file sang Markdown.
    pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError> {
        let format = FormatKind::from_path(path);
        let md = match format {
            FormatKind::Pdf => conv::pdf::to_markdown(path),
            FormatKind::Docx => conv::docx::to_markdown(path),
            FormatKind::Pptx => conv::pptx::to_markdown(path),
            FormatKind::Xlsx => conv::xlsx::to_markdown(path),
            FormatKind::Csv => conv::csv_conv::to_markdown(path),
            FormatKind::Html => conv::html::to_markdown(path),
            FormatKind::Image => image_ocr::ocr_image(path, &self.opts.ocr_langs)
                .map_err(|e| ConvertError::Failed(e.to_string())),
            FormatKind::Audio => {
                let model = self
                    .opts
                    .whisper_model
                    .as_ref()
                    .ok_or(ConvertError::Unsupported("audio: chưa cấu hình whisper_model"))?;
                let engine = audio::AudioEngine::load(model)?;
                engine
                    .transcribe_file(path, Some(&self.opts.audio_lang))
                    .map(|t| t.text)
            }
            FormatKind::Unknown => return Err(ConvertError::Unsupported("không rõ đuôi file")),
        }?;

        Ok(ConversionResult {
            markdown: md,
            title: None,
            format,
        })
    }
}
