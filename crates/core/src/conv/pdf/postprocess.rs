//! Markdown post-processing: malformed-table detection and repeated margin stripping.

use std::collections::{HashMap, HashSet};

pub(super) fn table_cells(line: &str) -> Option<Vec<&str>> {
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

pub(super) fn is_table_separator(line: &str) -> bool {
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
pub(super) fn markdown_has_malformed_table(markdown: &str) -> bool {
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

/// Normalize a margin candidate for exact-line comparison.
///
/// Leading Markdown heading markers (`#`…`######`) are kept so structural
/// headings never collapse into plain repeated header text. Emphasis markers
/// are still stripped. Cross-line joins are intentionally not used.
pub(super) fn normalized_margin_line(line: &str) -> Option<String> {
    use unicode_normalization::UnicodeNormalization;

    let trimmed = line.trim();
    if trimmed.starts_with('|') || trimmed.starts_with("```") || is_table_separator(trimmed) {
        return None;
    }

    let (heading_prefix, rest) = split_markdown_heading_prefix(trimmed);
    let filtered = rest
        .chars()
        .filter(|ch| !matches!(ch, '*' | '_' | '`'))
        .collect::<String>();
    let body = filtered
        .nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if body.chars().count() < 8 || body.chars().count() > 400 {
        return None;
    }
    let normalized = if heading_prefix.is_empty() {
        body
    } else {
        format!("{heading_prefix} {body}")
    };
    Some(normalized)
}

pub(super) fn split_markdown_heading_prefix(line: &str) -> (&str, &str) {
    let bytes = line.as_bytes();
    let mut hashes = 0usize;
    while hashes < bytes.len() && hashes < 6 && bytes[hashes] == b'#' {
        hashes += 1;
    }
    if hashes == 0 {
        return ("", line);
    }
    if hashes < bytes.len() && !bytes[hashes].is_ascii_whitespace() {
        // `#not-a-heading` — keep as ordinary text (hashes stripped below? no,
        // treat whole line as body; hashes are not a valid MD heading marker).
        return ("", line);
    }
    (&line[..hashes], line[hashes..].trim_start())
}

/// First/last nonempty line indices used for exact per-line margin matching.
pub(super) fn margin_line_indices(lines: &[&str]) -> HashSet<usize> {
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

/// Remove headers/footers repeated on most pages.
///
/// Only exact normalized individual margin-line identity is used. Without PDF
/// geometry we do not equate combined vs split segmentations (joined block
/// signatures erased line boundaries and, with `#` stripped, deleted real
/// Markdown headings). Alternate combined/split forms are retained.
pub(super) fn strip_repeated_marginal_lines(pages: &mut [String]) {
    if pages.len() < 4 {
        return;
    }

    let threshold = (pages.len() * 3).div_ceil(5).max(3);
    let page_margin_lines: Vec<Vec<(usize, String)>> = pages
        .iter()
        .map(|page| {
            let lines: Vec<&str> = page.lines().collect();
            margin_line_indices(&lines)
                .into_iter()
                .filter_map(|index| {
                    normalized_margin_line(lines[index]).map(|normalized| (index, normalized))
                })
                .collect()
        })
        .collect();

    let mut line_page_counts: HashMap<String, usize> = HashMap::new();
    for margins in &page_margin_lines {
        let unique_lines: HashSet<&str> = margins.iter().map(|(_, line)| line.as_str()).collect();
        for line in unique_lines {
            *line_page_counts.entry(line.to_string()).or_default() += 1;
        }
    }

    let repeated_lines: HashSet<String> = line_page_counts
        .into_iter()
        .filter(|(_, count)| *count >= threshold)
        .map(|(line, _)| line)
        .collect();
    if repeated_lines.is_empty() {
        return;
    }

    for (page, margins) in pages.iter_mut().zip(page_margin_lines.iter()) {
        let drop_indices: HashSet<usize> = margins
            .iter()
            .filter(|(_, normalized)| repeated_lines.contains(normalized))
            .map(|(index, _)| *index)
            .collect();
        if drop_indices.is_empty() {
            continue;
        }
        let retained = page
            .lines()
            .enumerate()
            .filter(|(index, _)| !drop_indices.contains(index))
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");
        *page = retained;
    }
}

#[cfg(test)]
mod tests {
    use super::{markdown_has_malformed_table, strip_repeated_marginal_lines};

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
    fn strips_repeated_headers_majority_combined_retains_split_form() {
        let combined = "Mã hiệu: ALPHA/LD/HDCV/FPT **PHƯƠNG PHÁP LUẬN FPT CASAN** \
            Lần ban hành/sửa đổi: 1/0 **TRONG CHUYỂN ĐỔI AI** Ngày hiệu lực: 19/5/2026";
        // Exact line identity only: majority combined lines are stripped.
        // Minority split lines are different strings and are retained — we do
        // not join/split-equate without PDF geometry.
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

        assert!(pages[..4].iter().all(|page| !page.contains("Mã hiệu:")));
        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(
            pages[4].contains("PHƯƠNG PHÁP LUẬN FPT CASAN"),
            "split-form margins must remain without cross-segmentation matching"
        );
        assert!(pages[4].contains("Mã hiệu:"));
        assert!(pages
            .iter()
            .enumerate()
            .all(|(index, page)| page.contains(&format!(
                "trang {}",
                ["một", "hai", "ba", "bốn", "năm"][index]
            ))));
    }

    #[test]
    fn strips_repeated_headers_majority_split_retains_combined_form() {
        let combined = "Mã hiệu: ALPHA/LD/HDCV/FPT **PHƯƠNG PHÁP LUẬN FPT CASAN** \
            Lần ban hành/sửa đổi: 1/0 **TRONG CHUYỂN ĐỔI AI** Ngày hiệu lực: 19/5/2026";
        let split = "PHƯƠNG PHÁP LUẬN FPT CASAN\n\
             TRONG CHUYỂN ĐỔI AI\n\
             Mã hiệu: ALPHA/LD/HDCV/FPT\n\
             Lần ban hành/sửa đổi: 1/0\n\
             Ngày hiệu lực: 19/5/2026";
        let mut pages = vec![
            format!("{split}\n\nNội dung trang một"),
            format!("{split}\n\nNội dung trang hai"),
            format!("{split}\n\nNội dung trang ba"),
            format!("{split}\n\nNội dung trang bốn"),
            format!("{combined}\n\nNội dung trang năm"),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(pages[..4].iter().all(|page| !page.contains("Mã hiệu:")));
        assert!(
            pages[4].contains("PHƯƠNG PHÁP LUẬN FPT CASAN"),
            "combined-form margin must remain without cross-segmentation matching"
        );
        assert!(pages
            .iter()
            .enumerate()
            .all(|(index, page)| page.contains(&format!(
                "trang {}",
                ["một", "hai", "ba", "bốn", "năm"][index]
            ))));
    }

    #[test]
    fn markdown_heading_lines_not_stripped_via_joined_plain_header() {
        // Same-side concatenated plain header vs legitimate Markdown headings
        // that would match if '#' were stripped and lines were joined.
        let plain = "PHƯƠNG PHÁP LUẬN FPT CASAN TRONG CHUYỂN ĐỔI AI";
        let headings = "# PHƯƠNG PHÁP LUẬN FPT CASAN\n## TRONG CHUYỂN ĐỔI AI";
        let mut pages = vec![
            format!("{plain}\n\nNội dung trang một"),
            format!("{plain}\n\nNội dung trang hai"),
            format!("{plain}\n\nNội dung trang ba"),
            format!("{plain}\n\nNội dung trang bốn"),
            format!("{headings}\n\nNội dung trang năm về mục tiêu chuyển đổi"),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(
            pages[4].contains("# PHƯƠNG PHÁP LUẬN FPT CASAN"),
            "AT heading must not be deleted by plain joined-header equivalence"
        );
        assert!(
            pages[4].contains("## TRONG CHUYỂN ĐỔI AI"),
            "nested heading must be preserved as its own structural line"
        );
        assert!(pages[4].contains("Nội dung trang năm về mục tiêu chuyển đổi"));
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
    fn body_line_containing_repeated_header_is_preserved() {
        let header = "PHƯƠNG PHÁP LUẬN FPT CASAN";
        let body = "Theo PHƯƠNG PHÁP LUẬN FPT CASAN, doanh nghiệp phải chuẩn bị dữ liệu.";
        let mut pages = vec![
            format!("{header}\n\nNội dung trang một"),
            format!("{header}\n\nNội dung trang hai"),
            format!("{header}\n\nNội dung trang ba"),
            format!("{header}\n\nNội dung trang bốn"),
            format!("{body}\n\nNội dung trang năm"),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(
            pages[4].contains(body),
            "body line that contains a repeated header must stay intact"
        );
        assert!(pages[4].contains("doanh nghiệp phải chuẩn bị dữ liệu"));
    }

    #[test]
    fn body_line_contained_by_repeated_header_is_preserved() {
        let header = "PHƯƠNG PHÁP LUẬN FPT CASAN TRONG CHUYỂN ĐỔI AI";
        // ≥12 chars and a proper substring of the repeated header; old logic
        // deleted this via candidate.contains(normalized).
        let body_fragment = "PHƯƠNG PHÁP LUẬN FPT";
        let mut pages = vec![
            format!("{header}\n\nNội dung trang một"),
            format!("{header}\n\nNội dung trang hai"),
            format!("{header}\n\nNội dung trang ba"),
            format!("{header}\n\nNội dung trang bốn"),
            format!("{body_fragment}\nphần thân bài độc lập trên trang năm"),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(
            pages[4].contains(body_fragment),
            "short body line must not be dropped just because a longer repeated header contains it"
        );
        assert!(pages[4].contains("phần thân bài độc lập trên trang năm"));
    }

    #[test]
    fn unrelated_top_and_bottom_repeats_do_not_delete_body() {
        let top = "PHƯƠNG PHÁP LUẬN FPT CASAN";
        let bottom = "Tài liệu nội bộ FPT - Chỉ lưu hành nội bộ";
        // Concatenation of unrelated repeated top + bottom strings. Global
        // fragment subtraction would clear this line and drop real body text.
        let body = "PHƯƠNG PHÁP LUẬN FPT CASAN Tài liệu nội bộ FPT - Chỉ lưu hành nội bộ \
            là đoạn thân bài cần giữ lại cho doanh nghiệp.";
        let mut pages = vec![
            format!("{top}\n\nNội dung trang một đủ dài.\n\n{bottom}"),
            format!("{top}\n\nNội dung trang hai đủ dài.\n\n{bottom}"),
            format!("{top}\n\nNội dung trang ba đủ dài.\n\n{bottom}"),
            format!("{top}\n\nNội dung trang bốn đủ dài.\n\n{bottom}"),
            format!("{body}\n\nNội dung trang năm đủ dài.\n\nFooter riêng trang năm"),
        ];

        strip_repeated_marginal_lines(&mut pages);

        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("PHƯƠNG PHÁP LUẬN FPT CASAN")));
        assert!(pages[..4]
            .iter()
            .all(|page| !page.contains("Tài liệu nội bộ FPT")));
        assert!(
            pages[4].contains("là đoạn thân bài cần giữ lại cho doanh nghiệp"),
            "body must survive when it only joins unrelated top/bottom repeats"
        );
        assert!(pages[4].contains("Nội dung trang năm đủ dài"));
    }
}
