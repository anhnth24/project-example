//! PPTX → Markdown.
//!
//! Cải tiến so với markitdown-rs:
//!   - Đọc slide theo ĐÚNG thứ tự số (slide1, slide2, …) thay vì thứ tự file trong zip.
//!   - Bỏ toàn bộ `println!` debug.
//!   - Trích text theo từng đoạn `<a:p>` (xuống dòng đúng), gồm cả text trong bảng.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use zip::ZipArchive;

use super::fail;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let file = std::fs::File::open(path).map_err(fail)?;
    let mut zip = ZipArchive::new(file).map_err(fail)?;

    // Thu thập tên slide rồi sắp theo số thứ tự.
    let mut slides: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| slide_number(n).map(|num| (num, n.to_string())))
        .collect();
    slides.sort_by_key(|(n, _)| *n);

    let mut md = String::new();
    for (num, name) in slides {
        let mut xml = String::new();
        zip.by_name(&name)
            .map_err(fail)?
            .read_to_string(&mut xml)
            .map_err(fail)?;

        let text = extract_slide_text(&xml)?;
        if text.trim().is_empty() {
            continue;
        }
        md.push_str(&format!("## Slide {num}\n\n"));
        md.push_str(text.trim_end());
        md.push_str("\n\n");
    }
    Ok(md)
}

/// Lấy số N từ "ppt/slides/slideN.xml".
fn slide_number(name: &str) -> Option<u32> {
    let base = name.strip_prefix("ppt/slides/slide")?;
    let num = base.strip_suffix(".xml")?;
    num.parse().ok()
}

/// Trích text: gom mọi `<a:t>` theo thứ tự, xuống dòng khi kết thúc `<a:p>`.
fn extract_slide_text(xml: &str) -> Result<String, ConvertError> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_text = false;

    loop {
        match reader.read_event_into(&mut buf).map_err(fail)? {
            Event::Start(e) if e.name().as_ref() == b"a:t" => in_text = true,
            Event::End(e) if e.name().as_ref() == b"a:t" => in_text = false,
            Event::Text(t) if in_text => {
                out.push_str(&t.unescape().map_err(fail)?);
            }
            Event::End(e) if e.name().as_ref() == b"a:p" => out.push('\n'),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}
