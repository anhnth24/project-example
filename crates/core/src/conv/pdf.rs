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

use std::collections::HashSet;
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
    if pages.is_some() {
        return Err(fail(
            "không thể trích đúng các trang đã chọn (pdf-inspector/PDFium thất bại)",
        ));
    }
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

    let has_malformed_tables = res
        .pages
        .iter()
        .any(|page| markdown_has_malformed_table(&page.markdown));
    let need_pdfium = has_malformed_tables
        || res.pages.iter().any(|page| page.needs_ocr)
        || (ocr_enabled && ocr_images);

    PDFIUM.with(|opt| -> Option<String> {
        // Chỉ mở PDFium khi thật sự cần (OCR trang scan hoặc OCR ảnh nhúng).
        let pdf_doc = if need_pdfium {
            opt.as_ref()
                .and_then(|p| p.load_pdf_from_file(path, None).ok())
        } else {
            None
        };

        let mut page_chunks: Vec<String> = Vec::with_capacity(res.pages.len());
        let mut unresolved_page = false;
        for pm in &res.pages {
            let has_text = !pm.markdown.trim().is_empty();
            let mut page_out = String::new();

            if pm.needs_ocr {
                // `pdf-inspector` intentionally errs on the side of OCR when
                // *any* GID/symbol font is present. Real Word-exported PDFs can
                // therefore be flagged because of one logo/bullet font even
                // though PDFium decodes the main text perfectly. Prefer that
                // native text when it passes a conservative quality gate;
                // only render + OCR genuinely empty/garbled pages.
                let native_text = pm.ocr_reason.is_none().then(|| {
                    pdf_doc
                        .as_ref()
                        .and_then(|d| native_page_text_at(d, pm.page))
                        .filter(|text| native_text_is_trustworthy(text))
                });

                if let Some(text) = native_text.flatten() {
                    page_out.push_str(text.trim_end());
                } else if let Some(text) = ocr_enabled
                    .then(|| {
                        pdf_doc
                            .as_ref()
                            .and_then(|d| ocr_page_at(d, pm.page, langs))
                    })
                    .flatten()
                {
                    page_out.push_str(&format!("<!-- Trang {} (OCR) -->\n\n", pm.page + 1));
                    page_out.push_str(text.trim());
                } else if has_text {
                    // Không OCR được (thiếu lib…) → tạm dùng text-layer dù kém.
                    page_out.push_str(pm.markdown.trim_end());
                } else {
                    unresolved_page = true;
                }
            } else if has_text {
                // Trang có text tốt → dùng markdown cấu trúc của pdf-inspector.
                // Với bảng bị tách sai cột/ô rỗng, ưu tiên native text theo thứ
                // tự đọc: ít đẹp hơn nhưng không làm mất hoặc đảo nội dung.
                let native_table_fallback = markdown_has_malformed_table(&pm.markdown)
                    .then(|| {
                        pdf_doc
                            .as_ref()
                            .and_then(|d| native_page_text_at(d, pm.page))
                            .filter(|text| {
                                native_text_is_trustworthy(text)
                                    && native_text_covers_markdown(text, &pm.markdown)
                            })
                    })
                    .flatten();
                if let Some(text) = native_table_fallback {
                    page_out.push_str(text.trim_end());
                } else {
                    page_out.push_str(pm.markdown.trim_end());
                }

                // Trang trộn: OCR thêm ảnh nhúng lớn (nếu bật).
                if ocr_enabled && ocr_images {
                    if let Some(doc) = pdf_doc.as_ref() {
                        if let Ok(page) = doc.pages().get(pm.page as i32) {
                            if let Some(extra) =
                                ocr_page_images(doc, &page, langs, pm.page as usize + 1)
                            {
                                if !page_out.is_empty() {
                                    page_out.push_str("\n\n");
                                }
                                page_out.push_str(extra.trim_end());
                            }
                        }
                    }
                }
            }
            page_chunks.push(page_out);
        }

        if unresolved_page {
            return None;
        }
        strip_repeated_marginal_lines(&mut page_chunks);
        let out = page_chunks
            .into_iter()
            .filter(|page| !page.trim().is_empty())
            .map(|page| page.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
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
            || ('\u{F0000}'..='\u{FFFFD}').contains(&ch)
            || ('\u{100000}'..='\u{10FFFD}').contains(&ch)
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

fn native_text_covers_markdown(native: &str, markdown: &str) -> bool {
    let native_alnum = native.chars().filter(|ch| ch.is_alphanumeric()).count();
    let markdown_alnum = markdown.chars().filter(|ch| ch.is_alphanumeric()).count();
    markdown_alnum == 0 || native_alnum * 100 >= markdown_alnum * 90
}

fn table_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    Some(inner.split('|').map(str::trim).collect())
}

fn is_table_separator(line: &str) -> bool {
    table_cells(line).is_some_and(|cells| {
        !cells.is_empty()
            && cells.iter().all(|cell| {
                !cell.is_empty()
                    && cell
                        .chars()
                        .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                    && cell.chars().filter(|&ch| ch == '-').count() >= 3
            })
    })
}

/// `pdf-inspector` can over-segment a visually merged/multi-line table into
/// extra empty columns. Such Markdown looks structured but scrambles sentence
/// order. Detect those cases so the caller can preserve content as native text.
fn markdown_has_malformed_table(markdown: &str) -> bool {
    let lines: Vec<&str> = markdown.lines().collect();
    for index in 0..lines.len().saturating_sub(1) {
        let Some(header) = table_cells(lines[index]) else {
            continue;
        };
        if !is_table_separator(lines[index + 1]) {
            continue;
        }
        let separator = table_cells(lines[index + 1]).unwrap_or_default();
        let empty_headers = header.iter().filter(|cell| cell.is_empty()).count();
        let joined_header = header.join(" ").to_lowercase();
        if header.len() < 2
            || header.len() != separator.len()
            || empty_headers >= 2
            || (empty_headers > 0
                && (joined_header.contains("mã hiệu")
                    || joined_header.contains("lần ban hành")
                    || joined_header.contains("ngày hiệu lực")))
        {
            return true;
        }

        for row in lines
            .iter()
            .skip(index + 2)
            .take_while(|line| line.trim().starts_with('|'))
        {
            let Some(cells) = table_cells(row) else {
                break;
            };
            if cells.len() != header.len() {
                return true;
            }
        }
    }
    false
}

fn normalized_margin_line(line: &str) -> Option<String> {
    use unicode_normalization::UnicodeNormalization;

    let trimmed = line.trim();
    if trimmed.starts_with('|') || trimmed.starts_with("```") || is_table_separator(trimmed) {
        return None;
    }
    let filtered = trimmed
        .chars()
        .filter(|ch| !matches!(ch, '#' | '*' | '_' | '`'))
        .collect::<String>();
    let normalized = filtered
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (normalized.chars().count() >= 8 && normalized.chars().count() <= 400).then_some(normalized)
}

fn margin_indices(lines: &[&str]) -> HashSet<usize> {
    let nonempty: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (!line.trim().is_empty()).then_some(index))
        .collect();
    nonempty
        .iter()
        .take(5)
        .chain(nonempty.iter().rev().take(3))
        .copied()
        .collect()
}

/// Remove headers/footers repeated on most pages. Matching is restricted to
/// page margins and also handles a header represented as one combined line on
/// structured pages but several lines on PDFium fallback pages.
fn strip_repeated_marginal_lines(pages: &mut [String]) {
    if pages.len() < 4 {
        return;
    }

    let mut candidates: HashSet<String> = HashSet::new();
    let normalized_margins: Vec<String> = pages
        .iter()
        .map(|page| {
            let lines: Vec<&str> = page.lines().collect();
            margin_indices(&lines)
                .into_iter()
                .filter_map(|index| normalized_margin_line(lines[index]))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    for page in pages.iter() {
        let lines: Vec<&str> = page.lines().collect();
        for index in margin_indices(&lines) {
            if let Some(line) = normalized_margin_line(lines[index]) {
                candidates.insert(line);
            }
        }
    }

    let threshold = (pages.len() * 3).div_ceil(5).max(3);
    let repeated: Vec<String> = candidates
        .into_iter()
        .filter(|line| {
            normalized_margins
                .iter()
                .filter(|margin| margin.contains(line))
                .count()
                >= threshold
        })
        .collect();
    if repeated.is_empty() {
        return;
    }

    for page in pages.iter_mut() {
        let lines: Vec<&str> = page.lines().collect();
        let margins = margin_indices(&lines);
        let retained = lines
            .iter()
            .enumerate()
            .filter(|(index, line)| {
                if !margins.contains(index) {
                    return true;
                }
                let Some(normalized) = normalized_margin_line(line) else {
                    return true;
                };
                !repeated.iter().any(|candidate| {
                    candidate == &normalized
                        || (normalized.chars().count() >= 12 && candidate.contains(&normalized))
                        || (candidate.chars().count() >= 12 && normalized.contains(candidate))
                })
            })
            .map(|(_, line)| *line)
            .collect::<Vec<_>>()
            .join("\n");
        *page = retained;
    }
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
    use super::{
        markdown_has_malformed_table, native_text_covers_markdown, native_text_is_trustworthy,
        strip_repeated_marginal_lines,
    };

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
        let cid_garbage = "(cid:123) readable looking words repeated many times ".repeat(20);
        assert!(!native_text_is_trustworthy(&cid_garbage));
        let private_use = format!(
            "{} {}",
            '\u{F0000}'.to_string().repeat(4),
            "This otherwise readable page contains many normal words and sentences. ".repeat(8)
        );
        assert!(!native_text_is_trustworthy(&private_use));
    }

    #[test]
    fn trusts_long_plain_english_text() {
        let text = "This document contains a complete native text layer with enough readable \
            words to avoid unnecessary optical character recognition. The source remains \
            searchable, selectable, and substantially more accurate than a rendered OCR pass.";
        assert!(native_text_is_trustworthy(text));
    }

    #[test]
    fn detects_malformed_markdown_tables() {
        let valid = "| Tên | Mô tả | Trạng thái |\n\
            | --- | --- | --- |\n\
            | CASAN | Khung năng lực AI | Hoàn tất |";
        assert!(!markdown_has_malformed_table(valid));

        let empty_header = "| Định nghĩa |  | Đặc điểm | Mục tiêu |  |\n\
            | --- | --- | --- | --- | --- |\n\
            | Curious | là cấp | nội dung | chuyển cấp |  |";
        assert!(markdown_has_malformed_table(empty_header));

        let valid_sparse = "|  | Quý 1 | Quý 2 |\n\
            | --- | --- | --- |\n\
            | Doanh thu |  | 100 |";
        assert!(!markdown_has_malformed_table(valid_sparse));

        let mismatched = "| Tên | Mô tả |\n\
            | --- | --- |\n\
            | CASAN | Khung năng lực | Dư cột |";
        assert!(markdown_has_malformed_table(mismatched));
    }

    #[test]
    fn native_fallback_must_cover_structured_content() {
        assert!(native_text_covers_markdown(
            "CASAN là khung năng lực chuyển đổi trí tuệ nhân tạo cho doanh nghiệp.",
            "## CASAN\n\nCASAN là khung năng lực chuyển đổi trí tuệ nhân tạo."
        ));
        assert!(!native_text_covers_markdown(
            "CASAN có nội dung ngắn.",
            "## CASAN\n\nCASAN là khung năng lực chuyển đổi trí tuệ nhân tạo với rất nhiều \
             nội dung chi tiết không được phép biến mất khi fallback."
        ));
    }

    #[test]
    fn strips_repeated_headers_in_combined_and_split_forms() {
        let combined = "Mã hiệu: ALPHA/LD/HDCV/FPT **PHƯƠNG PHÁP LUẬN FPT CASAN** \
            Lần ban hành/sửa đổi: 1/0 **TRONG CHUYỂN ĐỔI AI** Ngày hiệu lực: 19/5/2026";
        let mut pages = vec![
            format!("{combined}\n\nNội dung trang một"),
            format!("{combined}\n\nNội dung trang hai"),
            format!("{combined}\n\nNội dung trang ba"),
            format!("{combined}\n\nNội dung trang bốn"),
            "PHƯƠNG PHÁP LUẬN FPT CASAN\n\
             TRONG CHUYỂN ĐỔI AI\n\
             Mã hiệu: ALPHA/LD/HDCV/FPT\n\
             Lần ban hành/sửa đổi: 1/0\n\
             Ngày hiệu lực: 19/5/2026\n\
             Nội dung trang năm"
                .to_string(),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages.iter().all(|page| !page.contains("Mã hiệu:")));
        assert!(pages
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(pages
            .iter()
            .enumerate()
            .all(|(index, page)| page.contains(&format!(
                "trang {}",
                ["một", "hai", "ba", "bốn", "năm"][index]
            ))));
    }

    #[test]
    fn repeated_table_headers_are_not_stripped() {
        let mut pages = (1..=5)
            .map(|page| {
                format!(
                    "| Chỉ tiêu | Giá trị |\n| --- | --- |\n| Trang {page} | {page}00 |\nNội dung {page}"
                )
            })
            .collect::<Vec<_>>();

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages
            .iter()
            .all(|page| page.contains("| Chỉ tiêu | Giá trị |")));
        assert!(pages.iter().all(|page| page.contains("| --- | --- |")));
    }
}
