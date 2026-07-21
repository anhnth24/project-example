//! Native PDFium text-layer extraction and trust/coverage gates.

use std::collections::HashMap;
use std::path::Path;

use pdfium_render::prelude::*;

use super::pdfium::{pdfium_call_guard, with_pdfium};

/// Extract the page's native text layer through PDFium.
pub(super) fn native_page_text_at(doc: &PdfDocument, page_0idx: u32) -> Option<String> {
    let page = doc.pages().get(page_0idx as i32).ok()?;
    page.text()
        .ok()
        .map(|text| text.all())
        .filter(|text| !text.trim().is_empty())
}

pub(super) fn native_text_for_requested_pages(
    path: &Path,
    pages_1idx: Option<&[u32]>,
) -> HashMap<u32, String> {
    let _pdfium_guard = pdfium_call_guard();
    with_pdfium(|opt| {
        let Some(doc) = opt.and_then(|pdfium| pdfium.load_pdf_from_file(path, None).ok()) else {
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

pub(super) fn native_text_for_pages(path: &Path, pages_1idx: &[u32]) -> HashMap<u32, String> {
    native_text_for_requested_pages(path, Some(pages_1idx))
}

/// Conservative trust gate for a native PDF text layer.
///
/// A useful page must contain enough word-like/alphanumeric content and almost
/// no decoding sentinels or control/private-use characters. This deliberately
/// accepts punctuation-heavy tables of contents while rejecting empty scans,
/// `(cid:123)` output and broken font mappings.
pub(super) fn native_text_is_trustworthy(text: &str) -> bool {
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
pub(super) fn native_text_is_high_confidence(text: &str) -> bool {
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

pub(super) fn native_text_covers_markdown(native: &str, markdown: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{
        native_text_covers_markdown, native_text_for_pages, native_text_is_high_confidence,
        native_text_is_trustworthy,
    };
    use crate::conv::pdf::pdfium::load_pdfium;
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
            "xref\n0 {}\n0000000000 65535 f\r\n",
            objects.len() + 1
        ));
        for off in offsets {
            out.push_str(&format!("{off:010} 00000 n\r\n"));
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
    fn concurrent_pdf_text_extraction_completes_without_deadlock() {
        // Chống regression cho khóa serialize PDFium: nếu có nesting/lock-order
        // sai thì test này treo; nếu thiếu khóa thì đường chạy này chính là
        // kịch bản UB (watch-convert + convert tay đồng thời).
        let dir = tempfile::tempdir().expect("exclusive PDF fixture tempdir");
        let a_path = dir.path().join("a.pdf");
        let b_path = dir.path().join("b.pdf");
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
    }
}
