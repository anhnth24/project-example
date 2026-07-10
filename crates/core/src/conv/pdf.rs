//! PDF → Markdown, quyết định **theo từng trang**.
//!
//! Đường chính: **`pdf-inspector`** trích markdown CÓ CẤU TRÚC theo từng trang
//! (heading theo cỡ chữ, bảng, **sắp lại thứ tự đọc đa cột**) và tự gắn cờ
//! `needs_ocr` cho trang scan/ảnh HOẶC trang có **text-layer rác** (font GID,
//! encoding hỏng) — bắt được lỗi mà cách đếm ký tự không thấy.
//!
//! Trang `needs_ocr` → render bằng PDFium ở 300 DPI rồi **OCR Tesseract** (pdf-inspector
//! không OCR). Trang trộn (text + ảnh) có thể OCR thêm ảnh nhúng khi bật `pdf_ocr_images`.
//!
//! Fallback: nếu pdf-inspector lỗi → đường PDFium (đếm ký tự); nếu vẫn không được /
//! thiếu libpdfium → `pdf-extract`.

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
/// (Chỉ dùng ở đường fallback PDFium.)
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
    pages: Option<&[u32]>,
) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;

    // 1) pdf-inspector: markdown có cấu trúc + needs_ocr theo trang.
    if let Some(md) = via_pdf_inspector(path, &bytes, ocr_langs, ocr_enabled, ocr_images, pages) {
        if !md.trim().is_empty() {
            return Ok(md);
        }
    }
    // 2) Fallback: PDFium (đếm ký tự) — giữ cho trường hợp pdf-inspector trượt.
    if let Some(md) = via_pdfium(path, ocr_langs, ocr_enabled, ocr_images, pages) {
        if !md.trim().is_empty() {
            return Ok(md);
        }
    }
    // 3) Cuối cùng: pdf-extract (không hỗ trợ lọc trang).
    extract_with_pdf_extract(&bytes)
}

/// Đường chính: pdf-inspector cho text/cấu trúc + PDFium/Tesseract cho trang scan.
fn via_pdf_inspector(
    path: &Path,
    bytes: &[u8],
    langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
) -> Option<String> {
    // pages 1-indexed từ người dùng → 0-indexed cho pdf-inspector.
    let pages0: Option<Vec<u32>> =
        pages.map(|ps| ps.iter().filter(|&&p| p >= 1).map(|&p| p - 1).collect());
    // pdf-inspector dùng lopdf bên trong → bọc catch_unwind cho chắc.
    let res = catch_unwind(AssertUnwindSafe(|| {
        pdf_inspector::extract_pages_markdown_mem(bytes, pages0.as_deref())
    }))
    .ok()?
    .ok()?;

    let need_pdfium = ocr_enabled && (ocr_images || res.pages.iter().any(|p| p.needs_ocr));

    PDFIUM.with(|opt| -> Option<String> {
        // Chỉ mở PDFium khi thật sự cần (OCR trang scan hoặc OCR ảnh nhúng).
        let pdf_doc = if need_pdfium {
            opt.as_ref()
                .and_then(|p| p.load_pdf_from_file(path, None).ok())
        } else {
            None
        };

        let mut out = String::new();
        for pm in &res.pages {
            let has_text = !pm.markdown.trim().is_empty();

            if pm.needs_ocr && ocr_enabled {
                // `pdf-inspector` intentionally errs on the side of OCR when
                // *any* GID/symbol font is present. Real Word-exported PDFs can
                // therefore be flagged because of one logo/bullet font even
                // though PDFium decodes the main text perfectly. Prefer that
                // native text when it passes a conservative quality gate;
                // only render + OCR genuinely empty/garbled pages.
                let native_text = pdf_doc
                    .as_ref()
                    .and_then(|d| native_page_text_at(d, pm.page))
                    .filter(|text| native_text_is_trustworthy(text));

                if let Some(text) = native_text {
                    out.push_str(text.trim_end());
                    out.push_str("\n\n");
                } else if let Some(text) = pdf_doc
                    .as_ref()
                    .and_then(|d| ocr_page_at(d, pm.page, langs))
                {
                    out.push_str(&format!("<!-- Trang {} (OCR) -->\n\n", pm.page + 1));
                    out.push_str(text.trim());
                    out.push_str("\n\n");
                } else if has_text {
                    // Không OCR được (thiếu lib…) → tạm dùng text-layer dù kém.
                    out.push_str(pm.markdown.trim_end());
                    out.push_str("\n\n");
                }
            } else if has_text {
                // Trang có text tốt → dùng markdown cấu trúc của pdf-inspector.
                out.push_str(pm.markdown.trim_end());
                out.push_str("\n\n");

                // Trang trộn: OCR thêm ảnh nhúng lớn (nếu bật).
                if ocr_enabled && ocr_images {
                    if let Some(doc) = pdf_doc.as_ref() {
                        if let Ok(page) = doc.pages().get(pm.page as i32) {
                            if let Some(extra) =
                                ocr_page_images(doc, &page, langs, pm.page as usize + 1)
                            {
                                out.push_str(&extra);
                            }
                        }
                    }
                }
            }
        }
        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    })
}

/// Render + OCR một trang theo chỉ số 0-based.
fn ocr_page_at(doc: &PdfDocument, page_0idx: u32, langs: &str) -> Option<String> {
    let page = doc.pages().get(page_0idx as i32).ok()?;
    ocr_full_page(&page, langs)
        .ok()
        .filter(|t| !t.trim().is_empty())
}

/// Extract the page's native text layer through PDFium.
fn native_page_text_at(doc: &PdfDocument, page_0idx: u32) -> Option<String> {
    let page = doc.pages().get(page_0idx as i32).ok()?;
    page.text()
        .ok()
        .map(|text| text.all())
        .filter(|text| !text.trim().is_empty())
}

/// Conservative trust gate for a native PDF text layer.
///
/// A useful page must contain enough word-like/alphanumeric content and almost
/// no decoding sentinels or control/private-use characters. This deliberately
/// accepts punctuation-heavy tables of contents while rejecting empty scans,
/// `(cid:123)` output and broken font mappings.
fn native_text_is_trustworthy(text: &str) -> bool {
    let mut nonspace = 0usize;
    let mut alphanumeric = 0usize;
    let mut bad = 0usize;

    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        nonspace += 1;
        if ch.is_alphanumeric() {
            alphanumeric += 1;
        }
        if ch == '\u{FFFD}'
            || ch == '\0'
            || ch.is_control()
            || ('\u{E000}'..='\u{F8FF}').contains(&ch)
        {
            bad += 1;
        }
    }

    if nonspace < 80 || alphanumeric < 40 || bad * 200 > nonspace {
        return false;
    }

    let lower = text.to_ascii_lowercase();
    if lower.contains("(cid:")
        || lower.contains("/gid")
        || lower.contains("<gid")
        || lower.contains("uni+")
    {
        return false;
    }

    let word_like = text
        .split_whitespace()
        .filter(|token| token.chars().filter(|ch| ch.is_alphabetic()).count() >= 2)
        .count();
    // TOC pages can legitimately be dominated by dotted leaders; 20% still
    // requires substantial readable content while allowing those pages.
    word_like >= 8 && alphanumeric * 100 >= nonspace * 20
}

/// Đường fallback cũ: PDFium đếm ký tự để quyết text vs OCR.
fn via_pdfium(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
) -> Option<String> {
    PDFIUM.with(|opt| -> Option<String> {
        let pdfium = opt.as_ref()?;
        let doc = pdfium.load_pdf_from_file(path, None).ok()?;
        let mut out = String::new();
        for (i, page) in doc.pages().iter().enumerate() {
            // Lọc trang (1-indexed) nếu người dùng chỉ định.
            if let Some(ps) = pages {
                if !ps.contains(&(i as u32 + 1)) {
                    continue;
                }
            }
            let text = page.text().map(|t| t.all()).unwrap_or_default();
            let nonspace = text.chars().filter(|c| !c.is_whitespace()).count();
            if nonspace >= PAGE_TEXT_MIN_CHARS {
                out.push_str(text.trim_end());
                out.push_str("\n\n");
                if ocr_enabled && ocr_images {
                    if let Some(extra) = ocr_page_images(&doc, &page, ocr_langs, i + 1) {
                        out.push_str(&extra);
                    }
                }
            } else if ocr_enabled {
                if let Ok(ocr) = ocr_full_page(&page, ocr_langs) {
                    let ocr = ocr.trim();
                    if !ocr.is_empty() {
                        out.push_str(&format!("<!-- Trang {} (OCR) -->\n\n", i + 1));
                        out.push_str(ocr);
                        out.push_str("\n\n");
                    }
                }
            }
        }
        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    })
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
            continue;
        }
        let Ok(img) = img_obj.get_processed_image(doc) else {
            continue;
        };
        if let Ok(text) = image_ocr::ocr_dynimage(&img, langs) {
            let text = text.trim();
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
    let bitmap = page
        .render(w, h, None)
        .map_err(|e| fail(format!("render: {e}")))?;
    let img = bitmap
        .as_image()
        .map_err(|e| fail(format!("as_image: {e}")))?;
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

/// Fallback cuối: pdf-extract (có thể panic → bắt bằng catch_unwind).
fn extract_with_pdf_extract(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(fail(e)),
        Err(_) => Err(ConvertError::Failed(
            "pdf-extract panic (PDF phức tạp/không chuẩn)".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::native_text_is_trustworthy;

    #[test]
    fn trusts_native_vietnamese_table_of_contents() {
        let mut text = String::from("MỤC LỤC\n");
        for page in 1..=35 {
            text.push_str(&format!(
                "{page}. Nội dung phương pháp luận chuyển đổi AI{} {page}\n",
                ".".repeat(90)
            ));
        }
        assert!(native_text_is_trustworthy(&text));
    }

    #[test]
    fn rejects_short_or_broken_native_text() {
        assert!(!native_text_is_trustworthy("Mã hiệu: 123"));
        assert!(!native_text_is_trustworthy(
            "(cid:123) (cid:99) /GID12 \u{FFFD}\u{FFFD}\u{FFFD} broken text"
        ));
    }

    #[test]
    fn trusts_long_plain_english_text() {
        let text = "This document contains a complete native text layer with enough readable \
            words to avoid unnecessary optical character recognition. The source remains \
            searchable, selectable, and substantially more accurate than a rendered OCR pass.";
        assert!(native_text_is_trustworthy(text));
    }
}
