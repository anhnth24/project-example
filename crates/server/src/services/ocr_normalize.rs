//! Chuẩn hoá Markdown sau convert/OCR, TRƯỚC khi lưu artifact và chunk.
//!
//! Vì sao ở đây mà không ở converter dùng chung (`fileconv-core`): CLI/desktop
//! cần output trung thực với nguồn; còn pipeline index của server cần Markdown
//! có cấu trúc heading để `chunk_markdown` chia đúng ranh giới Điều/Chương và
//! điền `heading_path` (bằng chứng 2026-08-15: 12/12 chunk của Thông tư
//! 36/2025/TT-BCT có `heading_path = {}` vì OCR không sinh heading nào — FTS
//! mất tín hiệu heading, citation không biết Điều, extractive trả trang bìa).
//!
//! Chạy trước khi tính sha256/lưu markdown nên artifact đã lưu và chunk span
//! luôn nhất quán. Mọi rule đều precision-first: chỉ sửa khi mẫu không thể là
//! nội dung hợp lệ khác.

/// Sửa lỗi ký tự OCR phổ biến của văn bản pháp luật tiếng Việt, nối các dòng
/// bị OCR bẻ giữa câu, rồi thăng cấp các dòng cấu trúc (`Chương`, `Điều N.`,
/// `Phụ lục`) thành heading Markdown.
pub fn normalize_converted_markdown(markdown: &str) -> String {
    let ascii_safe = normalize_extraction_artifacts(markdown);
    let repaired = repair_ocr_characters(&ascii_safe);
    let unwrapped = unwrap_soft_line_breaks(&repaired);
    promote_legal_headings(&unwrapped)
}

/// Font subset trong PDF hay map glyph gạch ngang/space về codepoint typographic
/// (U+2010 HYPHEN, U+2011 NB-HYPHEN, U+00A0 NBSP, U+00AD SOFT HYPHEN) thay vì
/// ASCII. FTS tokenize khác đi nên "TT-BCT" không khớp "TT‐BCT" — chuẩn về
/// ASCII trước mọi rule khác. Không đụng en/em dash (U+2013/U+2014): chúng là
/// dấu câu hợp lệ (khoảng số, ngắt mệnh đề).
fn normalize_extraction_artifacts(text: &str) -> String {
    if !text.contains(['\u{2010}', '\u{2011}', '\u{00A0}', '\u{00AD}']) {
        return text.to_string();
    }
    text.chars()
        .filter_map(|c| match c {
            '\u{2010}' | '\u{2011}' => Some('-'),
            '\u{00A0}' => Some(' '),
            '\u{00AD}' => None,
            _ => Some(c),
        })
        .collect()
}

/// Vision OCR có lúc bẻ dòng cứng giữa câu ("quy định vận\nhành thị trường").
/// Nối dòng sau vào dòng trước khi cả hai rõ ràng là một câu đang chảy tiếp;
/// mọi ranh giới cấu trúc (heading, bảng, list, ALL-CAPS title, comment trang)
/// đều giữ nguyên.
fn unwrap_soft_line_breaks(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(prev) = lines.last_mut() {
            if should_join_lines(prev, trimmed) {
                prev.push(' ');
                prev.push_str(trimmed);
                continue;
            }
        }
        lines.push(line.trim_end().to_string());
    }
    join_preserving_trailing_newline(text, lines)
}

fn should_join_lines(prev: &str, next: &str) -> bool {
    if prev.is_empty() || next.is_empty() {
        return false;
    }
    // Dòng trước đã kết câu/mệnh đề — không nối.
    if prev.ends_with(['.', ':', ';', '!', '?', '”', '"']) {
        return false;
    }
    // Ranh giới cấu trúc không bao giờ nối.
    let structural_start = |line: &str| {
        line.starts_with('#')
            || line.starts_with('|')
            || line.starts_with("<!--")
            || line.starts_with('-')
            || line.starts_with('+')
            || line.starts_with('*')
    };
    if structural_start(prev) || structural_start(next) || prev.ends_with("-->") {
        return false;
    }
    // Dòng heading pháp lý (kể cả khi chưa được thăng cấp) không nhận thêm
    // text và cũng không bị hút vào dòng trước.
    if legal_heading_level(prev).is_some() || legal_heading_level(next).is_some() {
        return false;
    }
    if is_list_item_start(next) {
        return false;
    }
    // Dòng title/letterhead toàn chữ hoa đứng riêng.
    if is_all_caps_line(prev) || is_all_caps_line(next) {
        return false;
    }
    true
}

/// "1.", "19a.", "a)", "đ)", hoặc mở ngoặc kép trích dẫn sửa đổi.
fn is_list_item_start(line: &str) -> bool {
    if line.starts_with('“') {
        return true;
    }
    let mut end = 0usize;
    for (index, c) in line.char_indices() {
        if index == end && c.is_alphanumeric() && end < 6 {
            end = index + c.len_utf8();
        } else {
            break;
        }
    }
    end > 0 && matches!(line[end..].chars().next(), Some(')' | '.'))
}

fn is_all_caps_line(line: &str) -> bool {
    let mut has_letter = false;
    for c in line.chars() {
        if c.is_alphabetic() {
            has_letter = true;
            if c.is_lowercase() {
                return false;
            }
        }
    }
    has_letter
}

/// Các cặp sửa lỗi mất `Đ`/`Ư` mà vision OCR hay mắc trên letterhead công văn.
/// Chỉ chuỗi không thể là tiền tố của từ hợp lệ khác mới được replace thẳng.
const LITERAL_REPAIRS: [(&str, &str); 6] = [
    ("Nghị ịnh", "Nghị định"),
    ("nghị ịnh", "nghị định"),
    ("Quyết ịnh", "Quyết định"),
    ("quyết ịnh", "quyết định"),
    ("Quy ịnh", "Quy định"),
    ("quy ịnh", "quy định"),
];

fn repair_ocr_characters(text: &str) -> String {
    let mut repaired = text.to_string();
    for (from, to) in LITERAL_REPAIRS {
        repaired = repaired.replace(from, to);
    }
    // "THÔNG TU" chỉ sửa khi không phải tiền tố của từ khác (THÔNG TUYẾN…).
    repaired = replace_unless_followed_by_letter(&repaired, "THÔNG TU", "THÔNG TƯ");
    // Đầu dòng "iều <số>" là "Điều" mất Đ (giữa câu luôn viết thường "điều").
    repair_line_start_dieu(&repaired)
}

fn replace_unless_followed_by_letter(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(from) {
        let after = &rest[pos + from.len()..];
        out.push_str(&rest[..pos]);
        if after.chars().next().is_some_and(char::is_alphabetic) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn repair_line_start_dieu(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        match trimmed.strip_prefix("iều ") {
            Some(rest) if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                lines.push(format!("{indent}Điều {rest}"));
            }
            _ => lines.push(line.to_string()),
        }
    }
    join_preserving_trailing_newline(text, lines)
}

/// Heading level Markdown cho một dòng cấu trúc pháp lý, hoặc `None` nếu dòng
/// là nội dung thường. Cross-reference giữa câu ("khoản 2 Điều 3 như sau")
/// không bao giờ khớp vì yêu cầu dấu `.`/`:` ngay sau số Điều ở ĐẦU dòng.
fn legal_heading_level(trimmed: &str) -> Option<usize> {
    if trimmed.starts_with('#') {
        return None;
    }
    let chuong = trimmed
        .strip_prefix("Chương ")
        .or_else(|| trimmed.strip_prefix("CHƯƠNG "));
    if let Some(rest) = chuong {
        let token = rest.split_whitespace().next().unwrap_or("");
        let token = token.trim_end_matches(['.', ':']);
        if !token.is_empty()
            && token
                .chars()
                .all(|c| c.is_ascii_digit() || "IVXLCDM".contains(c))
        {
            return Some(1);
        }
    }
    if let Some(rest) = trimmed.strip_prefix("Điều ") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let mut tail = rest[digits.len()..].chars();
            let mut next = tail.next();
            // Cho phép hậu tố kiểu "Điều 19a."
            if next.is_some_and(|c| c.is_ascii_lowercase()) {
                next = tail.next();
            }
            if matches!(next, Some('.' | ':')) && matches!(tail.next(), None | Some(' ')) {
                return Some(2);
            }
        }
    }
    if (trimmed.starts_with("Phụ lục") || trimmed.starts_with("PHỤ LỤC"))
        && trimmed.chars().count() < 100
    {
        return Some(2);
    }
    None
}

fn promote_legal_headings(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(level) = legal_heading_level(trimmed) {
            if lines.last().is_some_and(|prev| !prev.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("{} {trimmed}", "#".repeat(level)));
            lines.push(String::new());
            continue;
        }
        lines.push(line.to_string());
    }
    join_preserving_trailing_newline(markdown, lines)
}

fn join_preserving_trailing_newline(original: &str, lines: Vec<String>) -> String {
    let mut joined = lines.join("\n");
    if original.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotes_dieu_and_chuong_and_phu_luc_to_headings() {
        let input = "<!-- Trang 1 (OCR) -->\nTHÔNG TU\nĐiều 1. Sửa đổi, bổ sung một số điều\n1. Sửa đổi khoản 2 Điều 3 như sau:\nChương II\nPHỤ LỤC I\n";
        let output = normalize_converted_markdown(input);
        assert!(output.contains("\n## Điều 1. Sửa đổi, bổ sung một số điều\n"));
        assert!(output.contains("\n# Chương II\n"));
        assert!(output.contains("\n## PHỤ LỤC I\n"));
        assert!(output.contains("THÔNG TƯ"));
        // Cross-reference giữa câu không bị thăng cấp.
        assert!(output.contains("1. Sửa đổi khoản 2 Điều 3 như sau:"));
    }

    #[test]
    fn repairs_lost_dau_characters_precisely() {
        let input = "Căn cứ Nghị ịnh số 40/2025/NĐ-CP quy ịnh chức năng;\niều 4. Nội dung\nTHÔNG TUYẾN khám chữa bệnh\n";
        let output = normalize_converted_markdown(input);
        assert!(output.contains("Nghị định số 40/2025/NĐ-CP"));
        assert!(output.contains("quy định chức năng"));
        assert!(output.contains("## Điều 4. Nội dung"));
        assert!(output.contains("THÔNG TUYẾN"), "{output}");
    }

    #[test]
    fn supports_khoan_suffix_and_colon_headings() {
        let input = "Điều 19a. Bổ sung định nghĩa\nĐiều 2: Hiệu lực thi hành\nĐiều 4 Thông tư này tiếp tục áp dụng.\n";
        let output = normalize_converted_markdown(input);
        assert!(output.contains("## Điều 19a. Bổ sung định nghĩa"));
        assert!(output.contains("## Điều 2: Hiệu lực thi hành"));
        assert!(
            output.contains("\nĐiều 4 Thông tư này tiếp tục áp dụng."),
            "reference without dot must stay body text: {output}"
        );
    }

    #[test]
    fn existing_headings_and_plain_text_are_untouched() {
        let input = "# Chương I\n\n## Điều 1. Đã là heading\n\nVăn bản thường không đổi.\n";
        assert_eq!(normalize_converted_markdown(input), input);
    }

    #[test]
    fn unwraps_soft_line_breaks_but_keeps_structure() {
        let input = "Điều 1. Sửa đổi, bổ sung một số điều của Thông tư số 16/2025/TT-BCT\nngày 01 tháng 02 năm 2025 của Bộ trưởng Bộ Công Thương quy định vận\nhành thị trường bán buôn điện cạnh tranh\n1. Sửa đổi khoản 2 Điều 3 như sau:\n“2. Sản lượng điện năng bao tiêu, bao gồm:\na) Sản lượng điện năng cam kết mua tối thiểu\ntrong các Hợp đồng mua bán điện;\n\n<!-- Trang 2 (OCR) -->\n\nTHÔNG TƯ\nSửa đổi một số điều\n";
        let output = normalize_converted_markdown(input);
        assert!(
            output.contains("quy định vận hành thị trường bán buôn điện cạnh tranh"),
            "wrapped heading must be rejoined: {output}"
        );
        assert!(
            output.contains("mua tối thiểu trong các Hợp đồng mua bán điện;"),
            "wrapped list item body must be rejoined: {output}"
        );
        // List items and quoted amendments stay on their own lines.
        assert!(output.contains("\n1. Sửa đổi khoản 2 Điều 3 như sau:"));
        assert!(output.contains("\n“2. Sản lượng điện năng bao tiêu, bao gồm:"));
        // ALL-CAPS title lines are never merged with following text.
        assert!(output.contains("THÔNG TƯ\nSửa đổi một số điều"), "{output}");
    }

    #[test]
    fn normalizes_typographic_hyphens_and_nbsp_to_ascii() {
        let input = "Thông\u{00A0}tư 36/2025/TT\u{2010}BCT và EMS\u{2011}Recon, sổ\u{00AD} tay.\n";
        let output = normalize_converted_markdown(input);
        assert!(output.contains("Thông tư 36/2025/TT-BCT"), "{output}");
        assert!(output.contains("EMS-Recon"));
        assert!(output.contains("sổ tay"));
        // En/em dash là dấu câu hợp lệ — không đổi.
        let dash = "Giai đoạn 2024–2026 — giữ nguyên.\n";
        assert_eq!(normalize_converted_markdown(dash), dash);
    }

    #[test]
    fn idempotent_on_second_pass() {
        let input = "THÔNG TU\nĐiều 1. Phạm vi điều chỉnh\nNội dung.\n";
        let once = normalize_converted_markdown(input);
        assert_eq!(normalize_converted_markdown(&once), once);
    }
}
