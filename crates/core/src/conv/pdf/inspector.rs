//! pdf-inspector structured extraction: fast filtered, parallel full, and OCR-aware paths.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use crate::diagnostics::{ConversionWarning, MarkdownOutput};
use crate::image_ocr::{OcrAttemptError, OcrRunConfig};

use super::native_text::{
    native_text_covers_markdown, native_text_for_pages, native_text_for_requested_pages,
    native_text_is_high_confidence, native_text_is_trustworthy,
};
use super::ocr::{ocr_page_at, ocr_page_images, PageOcr};
use super::pdfium::{pdfium_call_guard, with_pdfium};
use super::postprocess::{markdown_has_malformed_table, strip_repeated_marginal_lines};
use super::recovery::{recover_needs_ocr_page, NeedsOcrPageResult, PDF_UNTRUSTED_INSPECTOR_SOURCE};

const PARALLEL_MIN_PAGES: u32 = 16;
const PARALLEL_MAX_PAGES: u32 = 200;
const PARALLEL_MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
const PARALLEL_MIN_CPUS: usize = 5;

/// Outcome of attempting the structured pdf-inspector path.
pub(super) enum InspectorAttempt {
    Success(MarkdownOutput),
    Abandoned { pages_needing_ocr: HashSet<u32> },
    Unavailable,
}

pub(super) fn probe_pages_needing_ocr(bytes: &[u8], pages: Option<&[u32]>) -> HashSet<u32> {
    let pages0: Option<Vec<u32>> =
        pages.map(|ps| ps.iter().filter(|&&p| p >= 1).map(|&p| p - 1).collect());
    let Some(res) = catch_unwind(AssertUnwindSafe(|| {
        pdf_inspector::extract_pages_markdown_mem(bytes, pages0.as_deref())
    }))
    .ok()
    .and_then(|r| r.ok()) else {
        return HashSet::new();
    };
    res.pages
        .into_iter()
        .filter(|page| page.needs_ocr)
        .map(|page| page.page + 1)
        .collect()
}

pub(super) fn parse_marked_pages(markdown: &str) -> HashMap<u32, String> {
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

pub(super) struct FastPages {
    pub(super) chunks: HashMap<u32, String>,
    pub(super) pages_needing_ocr: HashSet<u32>,
}

pub(super) fn extract_fast_pages_once(bytes: &[u8], selected: &[u32]) -> Option<FastPages> {
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

pub(super) fn extract_fast_pages(bytes: &[u8], selected: &[u32]) -> Option<FastPages> {
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

pub(super) fn finalize_fast_pages(
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
pub(super) fn via_pdf_inspector_filtered_fast(
    path: &Path,
    bytes: &[u8],
    pages: &[u32],
) -> Option<String> {
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
pub(super) fn via_pdf_inspector_parallel_full(path: &Path, bytes: &[u8]) -> Option<String> {
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
pub(super) fn via_pdf_inspector(
    path: &Path,
    bytes: &[u8],
    langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
    ocr_config: &OcrRunConfig,
    last_ocr_error: &mut Option<OcrAttemptError>,
) -> InspectorAttempt {
    // pages 1-indexed từ người dùng → 0-indexed cho pdf-inspector.
    let pages0: Option<Vec<u32>> =
        pages.map(|ps| ps.iter().filter(|&&p| p >= 1).map(|&p| p - 1).collect());
    // lopdf structure extraction and PDFium native-text extraction are
    // independent. Run them concurrently so documents that need native table
    // rescue pay the slower stage, not the sum of both stages.
    let Some((res, native_pages)) = std::thread::scope(|scope| {
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
    }) else {
        return InspectorAttempt::Unavailable;
    };

    let pages_needing_ocr: HashSet<u32> = res
        .pages
        .iter()
        .filter(|page| page.needs_ocr)
        .map(|page| page.page + 1)
        .collect();

    let needs_rendered_ocr = res.pages.iter().any(|page| {
        page.needs_ocr
            && !native_pages.get(&(page.page + 1)).is_some_and(|text| {
                native_text_is_trustworthy(text)
                    && (page.ocr_reason.is_none() || native_text_is_high_confidence(text))
            })
    });
    let need_pdfium = ocr_enabled && (ocr_images || needs_rendered_ocr);

    let _pdfium_guard = pdfium_call_guard();
    with_pdfium(|opt| -> InspectorAttempt {
        // Chỉ mở PDFium khi thật sự cần (OCR trang scan hoặc OCR ảnh nhúng).
        let pdf_doc = if need_pdfium {
            opt.and_then(|p| p.load_pdf_from_file(path, None).ok())
        } else {
            None
        };

        let mut page_chunks: Vec<String> = Vec::with_capacity(res.pages.len());
        let mut page_warnings: Vec<ConversionWarning> = Vec::new();
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
                let ocr_text = ocr_enabled
                    .then(|| {
                        pdf_doc.as_ref().and_then(|d| {
                            match ocr_page_at(d, pm.page, langs, ocr_config) {
                                Ok(PageOcr::Text(text)) => Some(text),
                                Ok(PageOcr::Blank) => Some(String::new()),
                                Err(error) => {
                                    *last_ocr_error = Some(error);
                                    None
                                }
                            }
                        })
                    })
                    .flatten();
                let page_1idx = pm.page + 1;
                let recovered = recover_needs_ocr_page(
                    page_1idx,
                    native_text.map(String::as_str),
                    ocr_text.as_deref(),
                    &pm.markdown,
                    PDF_UNTRUSTED_INSPECTOR_SOURCE,
                );
                match recovered {
                    NeedsOcrPageResult {
                        markdown: Some(text),
                        warning,
                        ..
                    } => {
                        if let Some(warning) = warning {
                            page_warnings.push(warning);
                        }
                        page_out.push_str(&text);
                    }
                    NeedsOcrPageResult { markdown: None, .. } => {
                        unresolved_page = true;
                    }
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
                            if let Some(extra) = ocr_page_images(
                                doc,
                                &page,
                                langs,
                                pm.page as usize + 1,
                                ocr_config,
                                last_ocr_error,
                            ) {
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
            // Abandoned inspector path — return needs_ocr so fallbacks inherit it.
            return InspectorAttempt::Abandoned { pages_needing_ocr };
        }
        strip_repeated_marginal_lines(&mut page_chunks);
        let out = page_chunks
            .into_iter()
            .filter(|page| !page.trim().is_empty())
            .map(|page| page.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n\n");
        if out.trim().is_empty() {
            InspectorAttempt::Abandoned { pages_needing_ocr }
        } else {
            InspectorAttempt::Success(MarkdownOutput::with_warnings(out, page_warnings))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_marked_pages, probe_pages_needing_ocr};
    use std::path::PathBuf;
    fn review_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf")
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
    fn committed_review_fixture_detector_flags_needs_ocr() {
        let fixture = review_fixture_path();
        assert!(
            fixture.is_file(),
            "committed project fixture must exist: {}",
            fixture.display()
        );
        let bytes = std::fs::read(&fixture).expect("read fixture");
        let needs = probe_pages_needing_ocr(&bytes, None);
        assert!(
            needs.contains(&1),
            "pdf-inspector detector must set needs_ocr on committed fixture page 1, got {needs:?}"
        );
    }
}
