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

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use super::fail;
use crate::{image_ocr, ConvertError};

thread_local! {
    // PDFium chỉ init MỘT lần/tiến trình → cache một instance mỗi thread.
    static PDFIUM: Option<Pdfium> = load_pdfium();
}

static PDFIUM_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

// libpdfium KHÔNG thread-safe: hai conversion song song (watch worker + lệnh
// convert desktop qua spawn_blocking) gọi FPDF đan xen vào state C toàn cục
// → UB/crash. Feature `thread_safe` của pdfium-render chỉ chia sẻ binding qua
// OnceCell, không khóa từng lời gọi, nên mọi vùng đụng PDFium phải giữ khóa
// này suốt vùng đó. Khóa ôm cả đoạn OCR trang scan cho đơn giản — hai PDF scan
// convert song song sẽ xếp hàng ở đoạn render+OCR; nếu throughput thành vấn đề
// thì tách Tesseract ra ngoài khóa.
static PDFIUM_CALL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Giữ khóa serialize PDFium trong suốt lifetime của guard trả về.
fn pdfium_call_guard() -> std::sync::MutexGuard<'static, ()> {
    PDFIUM_CALL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Trang có ít hơn ngưỡng này ký tự (không tính khoảng trắng) → coi là trang scan → OCR.
/// (Chỉ dùng ở đường fallback PDFium.)
const PAGE_TEXT_MIN_CHARS: usize = 10;
/// Chỉ OCR ảnh nhúng đủ lớn (px²) — bỏ qua logo/icon nhỏ.
const MIN_IMG_AREA: i64 = 200 * 200;
/// DPI render trang khi OCR (cao hơn = OCR tốt hơn, chậm hơn).
const OCR_DPI: f32 = 300.0;
const PARALLEL_MIN_PAGES: u32 = 16;
const PARALLEL_MAX_PAGES: u32 = 200;
const PARALLEL_MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
const PARALLEL_MIN_CPUS: usize = 5;

pub fn to_markdown(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
) -> Result<String, ConvertError> {
    image_ocr::clear_last_ocr_error();
    let bytes = std::fs::read(path).map_err(fail)?;

    // Page-filtered requests are common in the desktop/MCP token-saving flow.
    // The per-page API below intentionally extracts the whole document for
    // cross-page font statistics (~400 ms even for one page). The regular
    // options API honours its page filter during extraction and is ~8× faster.
    // Keep the slower path as fallback for OCR and malformed tables.
    if !ocr_images {
        match pages {
            Some(selected_pages) => {
                if let Some(md) = via_pdf_inspector_filtered_fast(path, &bytes, selected_pages) {
                    return Ok(md);
                }
            }
            None => {
                if let Some(md) = via_pdf_inspector_parallel_full(path, &bytes) {
                    return Ok(md);
                }
            }
        }
    }

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
        if let Some(error) = image_ocr::take_last_ocr_error() {
            return Err(fail(format!("OCR trang PDF đã chọn thất bại: {error}")));
        }
        return Err(fail(
            "không thể trích đúng các trang đã chọn (pdf-inspector/PDFium thất bại)",
        ));
    }
    match extract_with_pdf_extract(&bytes) {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Err(error) => Err(error),
        Ok(_) => {
            if !ocr_enabled {
                return Err(fail(
                    "PDF không có text layer; hãy bật OCR trang scan trong Settings",
                ));
            }
            if !pdfium_available() {
                return Err(fail(
                    "PDF là bản scan nhưng không tìm thấy PDFium để render trang; \
                     hãy cài lại Markhand Desktop hoặc đặt FILECONV_PDFIUM_LIB",
                ));
            }
            if !image_ocr::tesseract_available() {
                return Err(fail(
                    "PDF là bản scan nhưng không tìm thấy Tesseract OCR; \
                     hãy cài lại Markhand Desktop hoặc đặt FILECONV_TESSERACT",
                ));
            }
            if let Some(error) = image_ocr::take_last_ocr_error() {
                return Err(fail(format!("OCR trang PDF thất bại: {error}")));
            }
            Err(fail(
                "PDF không có text layer và OCR không nhận được nội dung",
            ))
        }
    }
}

fn pdfium_available() -> bool {
    PDFIUM.with(|pdfium| pdfium.is_some())
}

fn parse_marked_pages(markdown: &str) -> HashMap<u32, String> {
    let mut pages = HashMap::new();
    let mut current_page = None;
    let mut current_text = String::new();

    for line in markdown.lines() {
        let marker = line
            .trim()
            .strip_prefix("<!-- Page ")
            .and_then(|rest| rest.strip_suffix(" -->"))
            .and_then(|page| page.parse::<u32>().ok());
        if let Some(page) = marker {
            if let Some(previous) = current_page.replace(page) {
                pages.insert(previous, current_text.trim().to_string());
                current_text.clear();
            }
            continue;
        }
        if current_page.is_some() {
            current_text.push_str(line);
            current_text.push('\n');
        }
    }
    if let Some(page) = current_page {
        pages.insert(page, current_text.trim().to_string());
    }
    pages
}

struct FastPages {
    chunks: HashMap<u32, String>,
    pages_needing_ocr: HashSet<u32>,
}

fn extract_fast_pages_once(bytes: &[u8], selected: &[u32]) -> Option<FastPages> {
    let mut markdown_options = pdf_inspector::MarkdownOptions::default();
    markdown_options.include_page_numbers = true;
    // Keep headers in each marked chunk so our page-aware cleanup can also
    // process native-text replacements consistently.
    markdown_options.strip_headers_footers = false;
    let options = pdf_inspector::PdfOptions::new()
        .pages(selected.iter().copied())
        .markdown(markdown_options);
    let result = catch_unwind(AssertUnwindSafe(|| {
        pdf_inspector::process_pdf_mem_with_options(bytes, options)
    }))
    .ok()?
    .ok()?;
    let marked = result.markdown?;
    if marked.trim().is_empty() {
        return None;
    }
    let mut chunks = parse_marked_pages(&marked);
    // Table-only pages in pdf-inspector 0.1.3 can produce Markdown without the
    // requested page marker even when marker output is enabled. A single-page
    // request is still unambiguous.
    if chunks.is_empty() && selected.len() == 1 {
        chunks.insert(selected[0], marked.trim().to_string());
    }
    Some(FastPages {
        chunks,
        pages_needing_ocr: result.pages_needing_ocr.into_iter().collect(),
    })
}

fn extract_fast_pages(bytes: &[u8], selected: &[u32]) -> Option<FastPages> {
    let mut extracted = extract_fast_pages_once(bytes, selected)?;
    let mut recover: HashSet<u32> = selected
        .iter()
        .copied()
        .filter(|page| !extracted.chunks.contains_key(page))
        .collect();
    if selected.len() > 1 {
        recover.extend(selected.iter().copied().filter(|page| {
            extracted
                .chunks
                .get(page)
                .is_some_and(|text| markdown_has_malformed_table(text))
        }));
    }
    // Multi-page table insertion in pdf-inspector 0.1.3 can omit a page marker
    // or duplicate table content across page boundaries. Recover only those
    // pages individually instead of discarding the entire fast path.
    for page in recover {
        let mut single = extract_fast_pages_once(bytes, &[page])?;
        let text = single.chunks.remove(&page)?;
        extracted.chunks.insert(page, text);
        extracted.pages_needing_ocr.extend(single.pages_needing_ocr);
    }
    selected
        .iter()
        .all(|page| extracted.chunks.contains_key(page))
        .then_some(extracted)
}

fn finalize_fast_pages(
    path: &Path,
    selected: &[u32],
    mut extracted: FastPages,
    prefetched_native: Option<HashMap<u32, String>>,
) -> Option<String> {
    for page in &extracted.pages_needing_ocr {
        if selected.contains(page)
            && !extracted
                .chunks
                .get(page)
                .is_some_and(|text| native_text_is_high_confidence(text))
        {
            return None;
        }
    }

    let malformed_pages: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|page| {
            extracted
                .chunks
                .get(page)
                .is_some_and(|text| markdown_has_malformed_table(text))
        })
        .collect();
    let native_pages = match prefetched_native {
        Some(native) => native,
        None if malformed_pages.is_empty() => HashMap::new(),
        None => native_text_for_pages(path, &malformed_pages),
    };
    for page in malformed_pages {
        let native = native_pages.get(&page)?;
        let structured = extracted.chunks.get(&page)?;
        if !native_text_is_trustworthy(native) || !native_text_covers_markdown(native, structured) {
            return None;
        }
        extracted.chunks.insert(page, native.trim().to_string());
    }

    let mut ordered: Vec<String> = selected
        .iter()
        .filter_map(|page| extracted.chunks.remove(page))
        .collect();
    strip_repeated_marginal_lines(&mut ordered);
    let clean = ordered
        .into_iter()
        .filter(|page| !page.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!clean.trim().is_empty()).then_some(clean)
}

/// Fast structured extraction for a sorted, page-filtered request.
///
/// Page markers let us independently validate every page that the detector
/// considers suspicious. A genuine scan has an empty/low-confidence section
/// and falls through to the normal PDFium/Tesseract path.
fn via_pdf_inspector_filtered_fast(path: &Path, bytes: &[u8], pages: &[u32]) -> Option<String> {
    let selected: Vec<u32> = pages.iter().copied().filter(|&page| page >= 1).collect();
    if selected.is_empty() || selected.windows(2).any(|pair| pair[0] >= pair[1]) {
        return None;
    }

    let extracted = extract_fast_pages(bytes, &selected)?;
    finalize_fast_pages(path, &selected, extracted, None)
}

/// Split a larger full-document extraction into a few page-filtered workers.
///
/// `pdf-inspector`'s filtered pipeline parses font/layout data only for its
/// requested range. Four medium-sized ranges preserve enough context for
/// heading classification while reducing wall time substantially on normal
/// multicore desktops. Every page is validated and merged in document order;
/// any suspicious/missing page falls back to the conservative sequential path.
fn via_pdf_inspector_parallel_full(path: &Path, bytes: &[u8]) -> Option<String> {
    if bytes.len() > PARALLEL_MAX_PDF_BYTES {
        return None;
    }
    let page_count = catch_unwind(AssertUnwindSafe(|| pdf_inspector::detect_pdf_mem(bytes)))
        .ok()?
        .ok()?
        .page_count;
    if !(PARALLEL_MIN_PAGES..=PARALLEL_MAX_PAGES).contains(&page_count) {
        return None;
    }
    let available = std::thread::available_parallelism().ok()?.get();
    if available < PARALLEL_MIN_CPUS {
        return None;
    }
    let workers = 4.min(page_count.div_ceil(8) as usize);
    if workers < 2 {
        return None;
    }

    let selected: Vec<u32> = (1..=page_count).collect();
    let chunk_size = page_count.div_ceil(workers as u32) as usize;
    let ranges: Vec<&[u32]> = selected.chunks(chunk_size).collect();
    let (parts, native_pages) = std::thread::scope(|scope| {
        let handles: Vec<_> = ranges
            .iter()
            .map(|range| scope.spawn(|| extract_fast_pages_once(bytes, range)))
            .collect();
        // Run PDFium on the caller thread so its thread-local instance remains
        // cached for subsequent desktop conversions.
        let native_pages = native_text_for_requested_pages(path, None);
        let parts = handles
            .into_iter()
            .map(|handle| handle.join().ok().flatten())
            .collect::<Option<Vec<_>>>()?;
        Some((parts, native_pages))
    })?;

    let mut merged = FastPages {
        chunks: HashMap::new(),
        pages_needing_ocr: HashSet::new(),
    };
    for part in parts {
        for (page, text) in part.chunks {
            if merged.chunks.insert(page, text).is_some() {
                return None;
            }
        }
        merged.pages_needing_ocr.extend(part.pages_needing_ocr);
    }

    let missing: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|page| !merged.chunks.contains_key(page))
        .collect();
    let missing_set: HashSet<u32> = missing.iter().copied().collect();
    for page in missing {
        let native = native_pages.get(&page)?;
        if !native_text_is_high_confidence(native) {
            return None;
        }
        merged.chunks.insert(page, native.trim().to_string());

        // pdf-inspector 0.1.3 appends an unmarked table-only page to the
        // preceding marked chunk. Replace that predecessor with its native
        // page text as well, preventing duplicate/misattributed content.
        if let Some(previous) = page.checked_sub(1).filter(|page| *page >= 1) {
            if !missing_set.contains(&previous) && merged.chunks.contains_key(&previous) {
                let previous_native = native_pages.get(&previous)?;
                if !native_text_is_high_confidence(previous_native) {
                    return None;
                }
                merged
                    .chunks
                    .insert(previous, previous_native.trim().to_string());
            }
        }
    }

    if merged.chunks.len() != selected.len() {
        return None;
    }
    finalize_fast_pages(path, &selected, merged, Some(native_pages))
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
    // lopdf structure extraction and PDFium native-text extraction are
    // independent. Run them concurrently so documents that need native table
    // rescue pay the slower stage, not the sum of both stages.
    let (res, native_pages) = std::thread::scope(|scope| {
        let inspector = scope.spawn(|| {
            catch_unwind(AssertUnwindSafe(|| {
                pdf_inspector::extract_pages_markdown_mem(bytes, pages0.as_deref())
            }))
            .ok()?
            .ok()
        });
        let native_pages = native_text_for_requested_pages(path, pages);
        let res = inspector.join().ok().flatten()?;
        Some((res, native_pages))
    })?;

    let needs_rendered_ocr = res.pages.iter().any(|page| {
        page.needs_ocr
            && !native_pages.get(&(page.page + 1)).is_some_and(|text| {
                native_text_is_trustworthy(text)
                    && (page.ocr_reason.is_none() || native_text_is_high_confidence(text))
            })
    });
    let need_pdfium = ocr_enabled && (ocr_images || needs_rendered_ocr);

    let _pdfium_guard = pdfium_call_guard();
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
                let native_text = native_pages.get(&(pm.page + 1)).filter(|text| {
                    native_text_is_trustworthy(text)
                        && (pm.ocr_reason.is_none() || native_text_is_high_confidence(text))
                });

                if let Some(text) = native_text {
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
                        native_pages.get(&(pm.page + 1)).filter(|text| {
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
    match ocr_full_page(&page, langs) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => None,
        Err(error) => {
            image_ocr::record_ocr_error(format!("trang {}: {error}", page_0idx + 1));
            None
        }
    }
}

/// Extract the page's native text layer through PDFium.
fn native_page_text_at(doc: &PdfDocument, page_0idx: u32) -> Option<String> {
    let page = doc.pages().get(page_0idx as i32).ok()?;
    page.text()
        .ok()
        .map(|text| text.all())
        .filter(|text| !text.trim().is_empty())
}

fn native_text_for_requested_pages(
    path: &Path,
    pages_1idx: Option<&[u32]>,
) -> HashMap<u32, String> {
    let _pdfium_guard = pdfium_call_guard();
    PDFIUM.with(|opt| {
        let Some(doc) = opt
            .as_ref()
            .and_then(|pdfium| pdfium.load_pdf_from_file(path, None).ok())
        else {
            return HashMap::new();
        };
        match pages_1idx {
            Some(pages) => pages
                .iter()
                .filter_map(|&page| {
                    page.checked_sub(1)
                        .and_then(|page_0idx| native_page_text_at(&doc, page_0idx))
                        .map(|text| (page, text))
                })
                .collect(),
            None => doc
                .pages()
                .iter()
                .enumerate()
                .filter_map(|(page_0idx, _)| {
                    native_page_text_at(&doc, page_0idx as u32)
                        .map(|text| (page_0idx as u32 + 1, text))
                })
                .collect(),
        }
    })
}

fn native_text_for_pages(path: &Path, pages_1idx: &[u32]) -> HashMap<u32, String> {
    native_text_for_requested_pages(path, Some(pages_1idx))
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

/// Stricter semantic-looking gate used when `pdf-inspector` explicitly reports
/// garbled text. Printable GID noise often passes basic character checks but
/// lacks natural vowel-bearing words and contains long repeated letter runs.
fn native_text_is_high_confidence(text: &str) -> bool {
    if !native_text_is_trustworthy(text) {
        return false;
    }

    let vowels = "aeiouyAEIOUYăâêôơưĂÂÊÔƠƯ\
        áàảãạắằẳẵặấầẩẫậéèẻẽẹếềểễệíìỉĩịóòỏõọốồổỗộớờởỡợúùủũụứừửữựýỳỷỹỵ\
        ÁÀẢÃẠẮẰẲẴẶẤẦẨẪẬÉÈẺẼẸẾỀỂỄỆÍÌỈĨỊÓÒỎÕỌỐỒỔỖỘỚỜỞỠỢÚÙỦŨỤỨỪỬỮỰÝỲỶỸỴ";
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|token| token.chars().filter(|ch| ch.is_alphabetic()).count() >= 2)
        .collect();
    let vowel_words = words
        .iter()
        .filter(|token| token.chars().any(|ch| vowels.contains(ch)))
        .count();
    let alphabetic = text.chars().filter(|ch| ch.is_alphabetic()).count();

    let mut repeated_alnum_runs = 0usize;
    let mut previous = None;
    let mut run = 0usize;
    for ch in text.chars().map(|ch| ch.to_ascii_lowercase()) {
        if ch.is_alphanumeric() && Some(ch) == previous {
            run += 1;
            if run == 4 {
                repeated_alnum_runs += 1;
            }
        } else {
            run = 1;
        }
        previous = Some(ch);
    }

    alphabetic >= 250
        && words.len() >= 40
        && vowel_words * 100 >= words.len() * 70
        && repeated_alnum_runs <= 3
}

fn native_text_covers_markdown(native: &str, markdown: &str) -> bool {
    fn capped_tokens(text: &str) -> HashMap<String, u8> {
        let mut counts = HashMap::new();
        let mut token = String::new();
        let flush = |token: &mut String, counts: &mut HashMap<String, u8>| {
            if !token.is_empty() {
                let count = counts.entry(std::mem::take(token)).or_default();
                *count = (*count + 1).min(2);
            }
        };
        for ch in text.chars() {
            if ch.is_alphanumeric() {
                token.extend(ch.to_lowercase());
            } else {
                flush(&mut token, &mut counts);
            }
        }
        flush(&mut token, &mut counts);
        counts
    }

    let native_tokens = capped_tokens(native);
    let markdown_tokens = capped_tokens(markdown);
    let expected: usize = markdown_tokens.values().map(|&count| count as usize).sum();
    if expected == 0 {
        return true;
    }
    let overlap: usize = markdown_tokens
        .iter()
        .map(|(token, &count)| count.min(native_tokens.get(token).copied().unwrap_or(0)) as usize)
        .sum();
    overlap * 100 >= expected * 90
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
    let _pdfium_guard = pdfium_call_guard();
    PDFIUM.with(|opt| -> Option<String> {
        let pdfium = opt.as_ref()?;
        let doc = pdfium.load_pdf_from_file(path, None).ok()?;
        let mut out = String::new();
        let mut unresolved_pages = Vec::new();
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
                match ocr_full_page(&page, ocr_langs) {
                    Ok(ocr) => {
                        let ocr = ocr.trim();
                        if !ocr.is_empty() {
                            out.push_str(&format!("<!-- Trang {} (OCR) -->\n\n", i + 1));
                            out.push_str(ocr);
                            out.push_str("\n\n");
                        } else {
                            unresolved_pages.push(i + 1);
                            image_ocr::record_ocr_error(format!(
                                "trang {}: Tesseract không trả nội dung",
                                i + 1
                            ));
                        }
                    }
                    Err(error) => {
                        unresolved_pages.push(i + 1);
                        image_ocr::record_ocr_error(format!("trang {}: {error}", i + 1));
                    }
                }
            } else {
                unresolved_pages.push(i + 1);
            }
        }
        if !unresolved_pages.is_empty() || out.trim().is_empty() {
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
    let _init_guard = PDFIUM_INIT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("FILECONV_PDFIUM_LIB") {
        let path = PathBuf::from(p);
        candidates.push(path.clone());
        if path.is_dir() {
            candidates.push(Pdfium::pdfium_platform_library_name_at_path(&path));
        }
    }

    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().take(4).map(Path::to_path_buf));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            roots.extend(parent.ancestors().take(4).map(Path::to_path_buf));
        }
    }
    roots.extend(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .take(4)
            .map(Path::to_path_buf),
    );
    for root in roots {
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(
            &root.join("pdfium/lib"),
        ));
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(
            &root.join("pdfium"),
        ));
    }
    let mut seen = HashSet::new();
    candidates.retain(|path| seen.insert(path.clone()));

    for c in candidates {
        match Pdfium::bind_to_library(c) {
            Ok(bindings) => return Some(Pdfium::new(bindings)),
            Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => {
                return Some(Pdfium::default());
            }
            Err(_) => {}
        }
    }
    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Some(Pdfium::new(bindings)),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Some(Pdfium::default()),
        Err(_) => None,
    }
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
        load_pdfium, markdown_has_malformed_table, native_text_covers_markdown,
        native_text_for_pages, native_text_is_high_confidence, native_text_is_trustworthy,
        parse_marked_pages, strip_repeated_marginal_lines,
    };

    /// PDF một trang tối giản, tự tính offset xref để PDFium load được thật.
    fn minimal_pdf_bytes() -> Vec<u8> {
        let stream = "BT /F1 24 Tf 72 720 Td (Xin chao PDFium) Tj ET";
        let objects = [
            "<</Type/Catalog/Pages 2 0 R>>".to_string(),
            "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(),
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R\
             /Resources<</Font<</F1 5 0 R>>>>>>"
                .to_string(),
            format!("<</Length {}>>\nstream\n{stream}\nendstream", stream.len()),
            "<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_string(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for off in offsets {
            out.push_str(&format!("{off:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

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
        assert!(native_text_is_high_confidence(&text));
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

        let printable_gid_noise = "bcdfg hjklm npqrs tvwxyz BCDFG HJKLM NPQRS TVWXYZ ".repeat(20);
        assert!(native_text_is_trustworthy(&printable_gid_noise));
        assert!(!native_text_is_high_confidence(&printable_gid_noise));
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

    #[test]
    fn parses_fast_path_page_markers() {
        let marked = "<!-- Page 2 -->\n\n## Hai\n\nNội dung\n\
            <!-- Page 5 -->\n\n## Năm\n\nNội dung khác";
        let pages = parse_marked_pages(marked);

        assert_eq!(
            pages.get(&2).map(String::as_str),
            Some("## Hai\n\nNội dung")
        );
        assert_eq!(
            pages.get(&5).map(String::as_str),
            Some("## Năm\n\nNội dung khác")
        );
    }

    #[test]
    fn concurrent_pdf_text_extraction_completes_without_deadlock() {
        // Chống regression cho khóa serialize PDFium: nếu có nesting/lock-order
        // sai thì test này treo; nếu thiếu khóa thì đường chạy này chính là
        // kịch bản UB (watch-convert + convert tay đồng thời).
        let dir = std::env::temp_dir();
        let a_path = dir.join("fileconv_pdfium_lock_a.pdf");
        let b_path = dir.join("fileconv_pdfium_lock_b.pdf");
        std::fs::write(&a_path, minimal_pdf_bytes()).unwrap();
        std::fs::write(&b_path, minimal_pdf_bytes()).unwrap();

        let texts = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                (0..8)
                    .map(|_| native_text_for_pages(&a_path, &[1]))
                    .collect::<Vec<_>>()
            });
            let main: Vec<_> = (0..8)
                .map(|_| native_text_for_pages(&b_path, &[1]))
                .collect();
            (worker.join().unwrap(), main)
        });

        if load_pdfium().is_some() {
            // Có libpdfium: mọi lần trích phải ra đúng nội dung trang.
            for pages in texts.0.iter().chain(texts.1.iter()) {
                assert!(pages.get(&1).is_some_and(|t| t.contains("Xin chao PDFium")));
            }
        } else {
            eprintln!("libpdfium không có — chỉ kiểm tra không deadlock, bỏ qua assert nội dung");
        }
        let _ = std::fs::remove_file(&a_path);
        let _ = std::fs::remove_file(&b_path);
    }

    #[test]
    fn reuses_initialized_pdfium_bindings_across_threads() {
        if load_pdfium().is_none() {
            return; // PDFium is an optional runtime dependency.
        }
        let handles: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| load_pdfium().is_some()))
            .collect();
        assert!(handles
            .into_iter()
            .all(|handle| handle.join().unwrap_or(false)));
    }
}
