//! PDF → Markdown (text).
//!
//! Ưu tiên **PDFium** (thư viện của Google, dùng trong Chrome) qua `pdfium-render`:
//! bền, không panic, trích text tốt. Nếu không tìm thấy libpdfium thì fallback
//! `pdf-extract` (bọc catch_unwind để không sập vì PDF lỗi).
//!
//! Tìm libpdfium theo thứ tự: biến môi trường FILECONV_PDFIUM_LIB → ./pdfium/lib →
//! ./pdfium → thư viện hệ thống.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use pdfium_render::prelude::*;

use super::fail;
use crate::ConvertError;

thread_local! {
    // PDFium chỉ được init MỘT lần/tiến trình; tạo/huỷ nhiều lần sẽ hỏng.
    // Cache một instance cho mỗi thread (Pdfium không Send/Sync).
    static PDFIUM: Option<Pdfium> = load_pdfium();
}

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let via_pdfium = PDFIUM.with(|opt| {
        opt.as_ref().and_then(|p| {
            extract_with_pdfium(p, path)
                .ok()
                .filter(|t| !t.trim().is_empty())
        })
    });
    if let Some(t) = via_pdfium {
        return Ok(t);
    }
    extract_with_pdf_extract(path)
}

/// Bind libpdfium (nếu có).
fn load_pdfium() -> Option<Pdfium> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("FILECONV_PDFIUM_LIB") {
        candidates.push(p);
    }
    candidates.push("pdfium/lib/libpdfium.so".to_string());
    candidates.push("pdfium/libpdfium.so".to_string());

    for c in candidates {
        if let Ok(b) = Pdfium::bind_to_library(&c) {
            return Some(Pdfium::new(b));
        }
    }
    Pdfium::bind_to_system_library().ok().map(Pdfium::new)
}

/// Trích text từng trang bằng PDFium, ngăn cách trang bằng dòng trống.
fn extract_with_pdfium(pdfium: &Pdfium, path: &Path) -> Result<String, ConvertError> {
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| fail(format!("pdfium load: {e}")))?;
    let mut out = String::new();
    for page in doc.pages().iter() {
        if let Ok(text) = page.text() {
            out.push_str(text.all().trim_end());
            out.push_str("\n\n");
        }
    }
    Ok(out)
}

/// Fallback: pdf-extract (có thể panic → bắt bằng catch_unwind).
fn extract_with_pdf_extract(path: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;
    let result = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(&bytes)
    }));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(fail(e)),
        Err(_) => Err(ConvertError::Failed(
            "pdf-extract panic (PDF phức tạp/không chuẩn)".to_string(),
        )),
    }
}
