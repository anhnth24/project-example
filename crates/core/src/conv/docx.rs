//! DOCX → Markdown.
//!
//! Cải tiến so với markitdown-rs (chỉ nối text trơn bằng iter_text):
//!   - Duyệt từng run, xử lý `<w:br>`/`<w:cr>` → xuống dòng, `<w:tab>` → khoảng trắng
//!     → KHÔNG còn dính chữ kiểu "Chương INHỮNG QUY ĐỊNH CHUNG".
//!   - Lấy text trong hyperlink.
//!   - Phát hiện **heading** qua style ("Heading1".."Heading6", "Title") → `#`..`######`.
//!   - Đoạn thuộc danh sách (numbering) → `- `.
//!   - Bảng → bảng Markdown đúng chuẩn (escape `|`).

use std::path::Path;
use std::{collections::HashMap, io::Read};

use docx_rust::document::{
    BodyContent, Paragraph, ParagraphContent, Run, RunContent, TableCellContent, TableRowContent,
};
use docx_rust::DocxFile;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::ZipArchive;

use super::{esc_cell, fail};
use crate::conv::csv_conv::{rows_to_html_table, rows_to_md_table, MergeRange};
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let docx = DocxFile::from_file(path).map_err(|e| fail(format!("{e:?}")))?;
    let doc = docx.parse().map_err(|e| fail(format!("{e:?}")))?;
    let ooxml_tables = extract_ooxml_tables(path).unwrap_or_default();
    let mut table_index = 0usize;

    let mut md = String::new();
    for content in &doc.document.body.content {
        match content {
            BodyContent::Paragraph(p) => {
                let text = para_text(p);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let style = p
                    .property
                    .as_ref()
                    .and_then(|pr| pr.style_id.as_ref())
                    .map(|s| s.value.as_ref());
                let is_list = p
                    .property
                    .as_ref()
                    .map(|pr| pr.numbering.is_some())
                    .unwrap_or(false);

                if let Some(level) = style.and_then(heading_level) {
                    // Heading nằm trên một dòng: gộp các break thành khoảng trắng.
                    let line = text
                        .split('\n')
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join(" ");
                    md.push_str(&"#".repeat(level));
                    md.push(' ');
                    md.push_str(line.trim());
                    md.push_str("\n\n");
                } else if is_list {
                    md.push_str("- ");
                    md.push_str(&text.replace('\n', " "));
                    md.push('\n');
                } else {
                    // Giữ các break trong đoạn thành dòng riêng.
                    for line in text.split('\n') {
                        let line = line.trim();
                        if !line.is_empty() {
                            md.push_str(line);
                            md.push('\n');
                        }
                    }
                    md.push('\n');
                }
            }
            BodyContent::Table(table) => {
                if let Some(raw) = ooxml_tables.get(table_index).filter(|table| {
                    !table.merges.is_empty()
                        || table.rows.iter().flatten().any(|cell| cell.contains('\n'))
                }) {
                    md.push_str(&rows_to_html_table(&raw.rows, &raw.merges));
                    md.push('\n');
                    table_index += 1;
                    continue;
                }
                let mut rows: Vec<Vec<String>> = Vec::new();
                for row in &table.rows {
                    let mut cells: Vec<String> = Vec::new();
                    for cell in &row.cells {
                        if let TableRowContent::TableCell(tc) = cell {
                            let mut s = String::new();
                            for c in &tc.content {
                                let TableCellContent::Paragraph(p) = c;
                                s.push_str(&para_text(p));
                                s.push(' ');
                            }
                            cells.push(esc_cell(&s));
                        }
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                }
                md.push_str(&rows_to_md_table(&rows));
                md.push('\n');
                table_index += 1;
            }
            _ => {}
        }
    }
    Ok(md)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerticalMerge {
    None,
    Restart,
    Continue,
}

#[derive(Debug, Default)]
struct RawCell {
    text: String,
    colspan: usize,
    vertical: VerticalMerge,
}

impl Default for VerticalMerge {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Default)]
struct RawTable {
    rows: Vec<Vec<String>>,
    merges: Vec<MergeRange>,
}

fn xml_attr(element: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .map(|attribute| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
}

fn apply_cell_property(element: &BytesStart<'_>, cell: &mut RawCell) {
    match element.name().as_ref() {
        b"w:gridSpan" => {
            cell.colspan = xml_attr(element, b"w:val")
                .or_else(|| xml_attr(element, b"val"))
                .and_then(|value| value.parse().ok())
                .unwrap_or(1)
                .max(1);
        }
        b"w:vMerge" => {
            cell.vertical = match xml_attr(element, b"w:val")
                .or_else(|| xml_attr(element, b"val"))
                .as_deref()
            {
                Some("restart") => VerticalMerge::Restart,
                _ => VerticalMerge::Continue,
            };
        }
        _ => {}
    }
}

fn layout_raw_table(raw_rows: Vec<Vec<RawCell>>) -> RawTable {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut merges: Vec<MergeRange> = Vec::new();
    let mut active_vertical: HashMap<usize, usize> = HashMap::new();

    for (row_index, raw_row) in raw_rows.into_iter().enumerate() {
        let mut output_row = Vec::new();
        let mut column = 0usize;
        for cell in raw_row {
            let span = cell.colspan.max(1);
            output_row.resize(output_row.len().max(column + span), String::new());
            match cell.vertical {
                VerticalMerge::Continue => {
                    let mut extended = None;
                    for current in column..column + span {
                        if let Some(index) = active_vertical.get(&current).copied() {
                            merges[index].end_row = row_index;
                            extended = Some(index);
                        }
                    }
                    if let Some(index) = extended {
                        merges[index].end_col = merges[index].end_col.max(column + span - 1);
                    }
                }
                VerticalMerge::Restart => {
                    output_row[column] = cell.text.trim().to_string();
                    let index = merges.len();
                    merges.push(MergeRange {
                        start_row: row_index,
                        start_col: column,
                        end_row: row_index,
                        end_col: column + span - 1,
                    });
                    for current in column..column + span {
                        active_vertical.insert(current, index);
                    }
                }
                VerticalMerge::None => {
                    for current in column..column + span {
                        active_vertical.remove(&current);
                    }
                    output_row[column] = cell.text.trim().to_string();
                    if span > 1 {
                        merges.push(MergeRange {
                            start_row: row_index,
                            start_col: column,
                            end_row: row_index,
                            end_col: column + span - 1,
                        });
                    }
                }
            }
            column += span;
        }
        rows.push(output_row);
    }
    RawTable { rows, merges }
}

fn extract_ooxml_tables(path: &Path) -> Result<Vec<RawTable>, ConvertError> {
    let file = std::fs::File::open(path).map_err(fail)?;
    let mut archive = ZipArchive::new(file).map_err(fail)?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(fail)?
        .read_to_string(&mut xml)
        .map_err(fail)?;

    let mut reader = Reader::from_str(&xml);
    let mut buffer = Vec::new();
    let mut tables = Vec::new();
    let mut current_table: Option<Vec<Vec<RawCell>>> = None;
    let mut current_row: Option<Vec<RawCell>> = None;
    let mut current_cell: Option<RawCell> = None;
    let mut in_text = false;
    let mut table_depth = 0usize;

    loop {
        match reader.read_event_into(&mut buffer).map_err(fail)? {
            Event::Start(element) => match element.name().as_ref() {
                b"w:tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        current_table = Some(Vec::new());
                    }
                }
                b"w:tr" if table_depth == 1 && current_table.is_some() => {
                    current_row = Some(Vec::new())
                }
                b"w:tc" if table_depth == 1 && current_row.is_some() => {
                    current_cell = Some(RawCell {
                        colspan: 1,
                        ..Default::default()
                    })
                }
                b"w:t" if table_depth == 1 && current_cell.is_some() => in_text = true,
                b"w:gridSpan" | b"w:vMerge" if table_depth == 1 => {
                    if let Some(cell) = current_cell.as_mut() {
                        apply_cell_property(&element, cell);
                    }
                }
                _ => {}
            },
            Event::Empty(element) => match element.name().as_ref() {
                b"w:gridSpan" | b"w:vMerge" if table_depth == 1 => {
                    if let Some(cell) = current_cell.as_mut() {
                        apply_cell_property(&element, cell);
                    }
                }
                b"w:br" | b"w:cr" if table_depth == 1 => {
                    if let Some(cell) = current_cell.as_mut() {
                        cell.text.push('\n');
                    }
                }
                b"w:tab" if table_depth == 1 => {
                    if let Some(cell) = current_cell.as_mut() {
                        cell.text.push(' ');
                    }
                }
                _ => {}
            },
            Event::Text(text) if table_depth == 1 && in_text => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.text.push_str(&text.unescape().map_err(fail)?);
                }
            }
            Event::End(element) => match element.name().as_ref() {
                b"w:t" if table_depth == 1 => in_text = false,
                b"w:p" if table_depth == 1 => {
                    if let Some(cell) = current_cell.as_mut() {
                        if !cell.text.ends_with('\n') {
                            cell.text.push('\n');
                        }
                    }
                }
                b"w:tc" if table_depth == 1 => {
                    if let (Some(row), Some(cell)) = (current_row.as_mut(), current_cell.take()) {
                        row.push(cell);
                    }
                }
                b"w:tr" if table_depth == 1 => {
                    if let (Some(table), Some(row)) = (current_table.as_mut(), current_row.take()) {
                        table.push(row);
                    }
                }
                b"w:tbl" => {
                    if table_depth == 1 {
                        if let Some(rows) = current_table.take() {
                            tables.push(layout_raw_table(rows));
                        }
                    }
                    table_depth = table_depth.saturating_sub(1);
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(tables)
}

/// Trích text của một đoạn: gồm run trực tiếp và run trong hyperlink.
/// Gộp các run liền kề CÙNG kiểu định dạng trước khi bọc markdown — Word hay tách
/// một câu/chữ thành nhiều run dù định dạng giống nhau (vd gắn `w:lang` khác cho
/// một ký tự có dấu), bọc `**`/`*` theo từng run riêng sẽ dính dấu lưng chừng chữ
/// (kiểu "một s**ố* *điều"). Gộp theo (bold, italic) trước rồi mới bọc 1 lần.
fn para_text(p: &Paragraph) -> String {
    let mut spans: Vec<(String, bool, bool)> = Vec::new();
    for c in &p.content {
        let run = match c {
            ParagraphContent::Run(r) => Some(r),
            ParagraphContent::Link(h) => h.content.as_ref(),
            _ => None,
        };
        let Some(run) = run else { continue };
        let (text, bold, italic) = run_raw(run);
        if text.is_empty() {
            continue;
        }
        match spans.last_mut() {
            Some(last) if last.1 == bold && last.2 == italic => last.0.push_str(&text),
            _ => spans.push((text, bold, italic)),
        }
    }

    let mut s = String::new();
    for (text, bold, italic) in spans {
        if !bold && !italic {
            s.push_str(&text);
            continue;
        }
        // Bọc từng dòng riêng (tránh dấu nhấn mạnh tràn qua xuống dòng).
        let wrapped = text
            .split('\n')
            .map(|line| wrap_emphasis(line, bold, italic))
            .collect::<Vec<_>>()
            .join("\n");
        s.push_str(&wrapped);
    }
    s
}

/// Trích text THÔ (chưa bọc markdown) + cờ bold/italic của một run.
/// Break → xuống dòng, tab → khoảng trắng (trước đây đẩy ký tự tab thô vào
/// markdown, gây lỗi hiển thị như "tên:␉………").
fn run_raw(run: &Run) -> (String, bool, bool) {
    let mut raw = String::new();
    for c in &run.content {
        match c {
            RunContent::Text(t) => raw.push_str(t.text.as_ref()),
            RunContent::Break(_) | RunContent::CarriageReturn(_) => raw.push('\n'),
            RunContent::Tab(_) | RunContent::PTab(_) => raw.push(' '),
            RunContent::NoBreakHyphen(_) => raw.push('-'),
            _ => {}
        }
    }

    let prop = run.property.as_ref();
    let bold = prop
        .and_then(|p| p.bold.as_ref())
        .map(|b| b.value != Some(false))
        .unwrap_or(false);
    let italic = prop
        .and_then(|p| p.italics.as_ref())
        .map(|i| i.value != Some(false))
        .unwrap_or(false);
    (raw, bold, italic)
}

/// Bọc `**`/`*`/`***` quanh phần chữ thật, giữ khoảng trắng đầu/cuối ở NGOÀI dấu
/// nhấn mạnh (CommonMark coi `** text**` là không hợp lệ nếu có khoảng trắng sát dấu).
fn wrap_emphasis(text: &str, bold: bool, italic: bool) -> String {
    let core = text.trim();
    if core.is_empty() {
        return text.to_string();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    let marker = match (bold, italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        (false, false) => return text.to_string(),
    };
    format!("{lead}{marker}{core}{marker}{trail}")
}

/// Map style id → cấp heading. "Title"→1, "Heading1".."Heading6"→1..6.
fn heading_level(style: &str) -> Option<usize> {
    let s = style.to_ascii_lowercase().replace([' ', '-', '_'], "");
    if s == "title" {
        return Some(1);
    }
    s.strip_prefix("heading")
        .and_then(|rest| rest.parse::<usize>().ok())
        .map(|n| n.clamp(1, 6))
}

#[cfg(test)]
mod tests {
    use super::{layout_raw_table, wrap_emphasis, RawCell, VerticalMerge};

    #[test]
    fn wraps_bold_only() {
        assert_eq!(wrap_emphasis("Điều 1.", true, false), "**Điều 1.**");
    }

    #[test]
    fn wraps_italic_only() {
        assert_eq!(wrap_emphasis("Căn cứ Luật…", false, true), "*Căn cứ Luật…*");
    }

    #[test]
    fn wraps_bold_italic() {
        assert_eq!(wrap_emphasis("abc", true, true), "***abc***");
    }

    #[test]
    fn keeps_surrounding_whitespace_outside_markers() {
        assert_eq!(
            wrap_emphasis("  Họ và tên  ", true, false),
            "  **Họ và tên**  "
        );
    }

    #[test]
    fn empty_or_whitespace_unchanged() {
        assert_eq!(wrap_emphasis("   ", true, true), "   ");
        assert_eq!(wrap_emphasis("", true, false), "");
    }

    #[test]
    fn no_emphasis_passthrough() {
        assert_eq!(wrap_emphasis("plain", false, false), "plain");
    }

    #[test]
    fn layouts_horizontal_and_vertical_docx_merges() {
        let table = layout_raw_table(vec![
            vec![
                RawCell {
                    text: "Nhóm".into(),
                    colspan: 2,
                    vertical: VerticalMerge::Restart,
                },
                RawCell {
                    text: "Khác".into(),
                    colspan: 1,
                    vertical: VerticalMerge::None,
                },
            ],
            vec![
                RawCell {
                    text: String::new(),
                    colspan: 2,
                    vertical: VerticalMerge::Continue,
                },
                RawCell {
                    text: "Dữ liệu".into(),
                    colspan: 1,
                    vertical: VerticalMerge::None,
                },
            ],
        ]);
        assert_eq!(table.rows[0][0], "Nhóm");
        assert_eq!(table.merges.len(), 1);
        assert_eq!(table.merges[0].end_row, 1);
        assert_eq!(table.merges[0].end_col, 1);
    }
}
