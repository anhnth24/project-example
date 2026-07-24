//! Tiny deterministic office fixtures generated in-process (no large binaries).

use std::io::{Cursor, Write};

use image::{ImageBuffer, Luma};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Minimal one-page PDF with extractable Helvetica text.
pub fn tiny_pdf_bytes(marker: &str) -> Vec<u8> {
    let text = format!("BT /F1 12 Tf 40 100 Td ({marker}) Tj ET");
    let stream = format!("<< /Length {} >>stream\n{}\nendstream", text.len(), text);
    let objects = [
        "1 0 obj<< /Type /Catalog /Pages 2 0 R >>endobj\n".to_string(),
        "2 0 obj<< /Type /Pages /Kids [3 0 R] /Count 1 >>endobj\n".to_string(),
        "3 0 obj<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources<< /Font<< /F1 5 0 R >> >> >>endobj\n".to_string(),
        format!("4 0 obj{stream}\nendobj\n"),
        "5 0 obj<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>endobj\n".to_string(),
    ];
    let mut body = String::from("%PDF-1.4\n");
    let mut offsets = vec![0usize];
    for object in &objects {
        offsets.push(body.len());
        body.push_str(object);
    }
    let xref_at = body.len();
    body.push_str(&format!("xref\n0 {}\n", offsets.len()));
    body.push_str("0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        offsets.len()
    ));
    body.into_bytes()
}

fn zip_parts(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, data) in parts {
            zip.start_file(*name, options).expect("zip start");
            zip.write_all(data).expect("zip write");
        }
        zip.finish().expect("zip finish");
    }
    cursor.into_inner()
}

/// Minimal PPTX with one slide containing `marker` text.
pub fn tiny_pptx_bytes(marker: &str) -> Vec<u8> {
    let content_types = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"#;
    let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;
    let presentation = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"#;
    let presentation_rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#;
    let slide = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree>
    <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp>
      <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
      <p:spPr/>
      <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{marker}</a:t></a:r></a:p></p:txBody>
    </p:sp>
  </p:spTree></p:cSld>
</p:sld>"#
    );
    zip_parts(&[
        ("[Content_Types].xml", content_types.as_slice()),
        ("_rels/.rels", rels.as_slice()),
        ("ppt/presentation.xml", presentation.as_slice()),
        (
            "ppt/_rels/presentation.xml.rels",
            presentation_rels.as_slice(),
        ),
        ("ppt/slides/slide1.xml", slide.as_bytes()),
    ])
}

/// Deterministic PNG with high-contrast bitmap text for OCR (Tesseract).
///
/// Renders uppercase A–Z / 0–9 / space from a 5×7 font, scaled for OCR. If
/// Tesseract/`vie+eng` is missing at convert time, the vertical-slice live
/// test must fail (no soft-skip).
pub fn tiny_png_ocr_bytes(marker: &str) -> Vec<u8> {
    if marker.eq_ignore_ascii_case("SOAK15") {
        return include_bytes!(
            "../../../../bench/markhand_web/soak/fixtures/soak-png.png"
        )
        .to_vec();
    }
    const SCALE: u32 = 6;
    const PAD: u32 = 16;
    let text: String = marker
        .chars()
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| matches!(c, 'A'..='Z' | '0'..='9' | ' '))
        .collect();
    let text = if text.is_empty() {
        "OCR15".to_string()
    } else {
        text
    };
    let glyph_w = 6u32; // 5 px + 1 gap
    let glyph_h = 7u32;
    let width = PAD * 2 + glyph_w * text.len() as u32 * SCALE;
    let height = PAD * 2 + glyph_h * SCALE;
    let mut img: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(width, height, Luma([255]));
    for (i, ch) in text.chars().enumerate() {
        let Some(bits) = bitmap_glyph(ch) else {
            continue;
        };
        let ox = PAD + i as u32 * glyph_w * SCALE;
        let oy = PAD;
        for row in 0..7u32 {
            for col in 0..5u32 {
                if bits[row as usize] & (1 << (4 - col)) != 0 {
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            img.put_pixel(ox + col * SCALE + dx, oy + row * SCALE + dy, Luma([0]));
                        }
                    }
                }
            }
        }
    }
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("encode png");
    png.into_inner()
}

fn bitmap_glyph(ch: char) -> Option<[u8; 7]> {
    // 5-bit rows, MSB = left pixel.
    Some(match ch {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        _ => return None,
    })
}

/// Minimal DOCX with one paragraph containing `marker` text.
pub fn tiny_docx_bytes(marker: &str) -> Vec<u8> {
    let content_types = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
    let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{marker}</w:t></w:r></w:p></w:body>
</w:document>"#
    );
    zip_parts(&[
        ("[Content_Types].xml", content_types.as_slice()),
        ("_rels/.rels", rels.as_slice()),
        ("word/document.xml", document.as_bytes()),
    ])
}

/// Minimal XLSX with one sheet named Budget and a marker cell.
pub fn tiny_xlsx_bytes(marker: &str) -> Vec<u8> {
    let content_types = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"#;
    let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    let workbook = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;
    let workbook_rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#;
    let shared = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1">
  <si><t>{marker}</t></si>
</sst>"#
    );
    let sheet = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData>
</worksheet>"#;
    zip_parts(&[
        ("[Content_Types].xml", content_types.as_slice()),
        ("_rels/.rels", rels.as_slice()),
        ("xl/workbook.xml", workbook.as_slice()),
        ("xl/_rels/workbook.xml.rels", workbook_rels.as_slice()),
        ("xl/sharedStrings.xml", shared.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_slice()),
    ])
}

/// Convert source bytes with the production `fileconv-core` converter.
pub fn convert_to_markdown(ext: &str, bytes: &[u8]) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("fixture.{ext}"));
    std::fs::write(&path, bytes).expect("write fixture");
    let result = fileconv_core::Converter::default()
        .convert_path(&path)
        .unwrap_or_else(|error| panic!("convert {ext}: {error}"));
    // Ensure source SHA differs from canonical Markdown SHA for dual-hash assertions.
    if result.markdown.as_bytes() == bytes {
        format!("{}\n\n<!-- markhand-canonical -->\n", result.markdown)
    } else {
        result.markdown
    }
}
