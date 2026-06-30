//! PDF → Markdown, quyết định **theo từng trang** và xử lý cả **trang trộn**.
//!
//! Với mỗi trang:
//!   - Có **lớp text** → trích trực tiếp bằng PDFium (nhanh, chính xác).
//!   - Gần như **không có text** (trang scan/ảnh) → render trang → **OCR** Tesseract.
//!   - **Trang trộn** (có text + ảnh scan chứa chữ): lấy text layer, và nếu bật
//!     `pdf_ocr_images` thì OCR thêm các **ảnh nhúng lớn** để lấy chữ trong ảnh.
//!
//! `pdf_ocr_images` mặc định TẮT vì OCR mọi ảnh trong tài liệu thường (figure, biểu đồ,
//! ảnh chụp) sẽ chậm và nhiễu; chỉ bật khi PDF có figure scan chứa chữ cần lấy.
//!
//! Không có libpdfium → fallback toàn file `pdf-extract`.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use pdfium_render::prelude::*;

use super::fail;
use crate::{image_ocr, ConvertError};

thread_local! {
    // PDFium chỉ init MỘT lần/tiến trình → cache một instance mỗi thread.
    static PDFIUM: Option<Pdfium> = load_pdfium();
}

/// Trang có ít hơn ngưỡng này ký tự (không tính khoảng trắng) → coi là trang scan → OCR.
const PAGE_TEXT_MIN_CHARS: usize = 10;
/// Chỉ OCR ảnh nhúng đủ lớn (px²) — bỏ qua logo/icon nhỏ.
const MIN_IMG_AREA: i64 = 200 * 200;
/// DPI render trang khi OCR (cao hơn = OCR tốt hơn, chậm hơn).
const OCR_DPI: f32 = 300.0;

pub fn to_markdown(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
) -> Result<String, ConvertError> {
    let via_pdfium = PDFIUM.with(|opt| -> Option<String> {
        let pdfium = opt.as_ref()?;
        let doc = pdfium.load_pdf_from_file(path, None).ok()?;
        let pages = doc.pages();

        let mut out = String::new();
        let mut any = false;
        for (i, page) in pages.iter().enumerate() {
            let text = page.text().map(|t| t.all()).unwrap_or_default();
            let nonspace = text.chars().filter(|c| !c.is_whitespace()).count();

            if nonspace >= PAGE_TEXT_MIN_CHARS {
                // Trang có lớp text → dùng trực tiếp.
                out.push_str(text.trim_end());
                out.push_str("\n\n");
                any = true;

                // Trang trộn: OCR thêm ảnh nhúng lớn (nếu bật).
                if ocr_enabled && ocr_images {
                    if let Some(extra) = ocr_page_images(&doc, &page, ocr_langs, i + 1) {
                        out.push_str(&extra);
                        any = true;
                    }
                }
            } else if ocr_enabled {
                // Trang scan/ảnh → render cả trang + OCR.
                if let Ok(ocr) = ocr_full_page(&page, ocr_langs) {
                    let ocr = ocr.trim();
                    if !ocr.is_empty() {
                        out.push_str(&format!("<!-- Trang {} (OCR) -->\n\n", i + 1));
                        out.push_str(ocr);
                        out.push_str("\n\n");
                        any = true;
                    }
                }
            }
        }
        if any {
            Some(out)
        } else {
            None
        }
    });

    if let Some(t) = via_pdfium {
        if !t.trim().is_empty() {
            return Ok(t);
        }
    }
    extract_with_pdf_extract(path)
}

/// OCR các ảnh nhúng đủ lớn trong một trang (cho trang trộn text + ảnh).
fn ocr_page_images(
    doc: &PdfDocument,
    page: &PdfPage,
    langs: &str,
    page_no: usize,
) -> Option<String> {
    let mut out = String::new();
    for obj in page.objects().iter() {
        let Some(img_obj) = obj.as_image_object() else {
            continue;
        };
        let w = img_obj.width().unwrap_or(0) as i64;
        let h = img_obj.height().unwrap_or(0) as i64;
        if w * h < MIN_IMG_AREA {
            continue; // bỏ ảnh nhỏ (logo/icon)
        }
        let Ok(img) = img_obj.get_processed_image(doc) else {
            continue;
        };
        if let Ok(text) = image_ocr::ocr_dynimage(&img, langs) {
            let text = text.trim();
            // Chỉ thêm nếu có nội dung chữ thực sự.
            if text.chars().filter(|c| c.is_alphanumeric()).count() >= 4 {
                out.push_str(&format!("<!-- Ảnh trong trang {page_no} (OCR) -->\n\n"));
                out.push_str(text);
                out.push_str("\n\n");
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Render cả trang ở OCR_DPI rồi OCR (qua image_ocr có tiền xử lý).
fn ocr_full_page(page: &PdfPage, langs: &str) -> Result<String, ConvertError> {
    let w = (((page.width().value / 72.0) * OCR_DPI).round() as i32).clamp(100, 5000);
    let h = (((page.height().value / 72.0) * OCR_DPI).round() as i32).clamp(100, 7000);
    let bitmap = page.render(w, h, None).map_err(|e| fail(format!("render: {e}")))?;
    let img = bitmap.as_image().map_err(|e| fail(format!("as_image: {e}")))?;
    image_ocr::ocr_dynimage(&img, langs).map_err(fail)
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
