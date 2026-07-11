//! Structured, safe PPTX preview data for the desktop SVG renderer.
//! This is intentionally a fidelity-oriented subset: positioned text, pictures
//! and basic shape fills. Unsupported charts/SmartArt remain visible as labels
//! instead of injecting third-party HTML into the webview.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::Serialize;
use zip::ZipArchive;

use crate::ConvertError;

const DEFAULT_WIDTH_EMU: i64 = 12_192_000;
const DEFAULT_HEIGHT_EMU: i64 = 6_858_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PptxPreviewMeta {
    pub slide_count: usize,
    pub width_emu: i64,
    pub height_emu: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PptxPreviewSlide {
    pub index: usize,
    pub width_emu: i64,
    pub height_emu: i64,
    pub background: String,
    pub shapes: Vec<PptxPreviewShape>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PptxPreviewShape {
    Text {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        text: String,
        font_pt: f32,
        bold: bool,
        color: String,
        fill: Option<String>,
    },
    Image {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        alt: String,
        data_url: String,
    },
    Shape {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        fill: Option<String>,
        stroke: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Text,
    Image,
    Graphic,
}

#[derive(Debug)]
struct ShapeBuilder {
    kind: ShapeKind,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    text: String,
    font_pt: f32,
    bold: bool,
    color: String,
    fill: Option<String>,
    stroke: Option<String>,
    relation_id: Option<String>,
    alt: String,
}

impl ShapeBuilder {
    fn new(kind: ShapeKind) -> Self {
        Self {
            kind,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            text: String::new(),
            font_pt: 18.0,
            bold: false,
            color: "#1f2937".into(),
            fill: None,
            stroke: None,
            relation_id: None,
            alt: String::new(),
        }
    }
}

fn fail(error: impl std::fmt::Display) -> ConvertError {
    ConvertError::Failed(error.to_string())
}

fn read_zip_text(zip: &mut ZipArchive<std::fs::File>, name: &str) -> Result<String, ConvertError> {
    let mut text = String::new();
    zip.by_name(name)
        .map_err(fail)?
        .read_to_string(&mut text)
        .map_err(fail)?;
    Ok(text)
}

fn slide_number(name: &str) -> Option<usize> {
    let value = name
        .strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?;
    value.parse().ok()
}

fn slide_names(zip: &ZipArchive<std::fs::File>) -> Vec<(usize, String)> {
    let mut slides: Vec<_> = zip
        .file_names()
        .filter_map(|name| slide_number(name).map(|number| (number, name.to_string())))
        .collect();
    slides.sort_by_key(|(number, _)| *number);
    slides
}

fn attr_string(element: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .map(|attribute| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
}

fn attr_i64(element: &BytesStart<'_>, key: &[u8]) -> Option<i64> {
    attr_string(element, key)?.parse().ok()
}

fn parse_dimensions(xml: &str) -> Result<(i64, i64), ConvertError> {
    let mut reader = Reader::from_str(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(fail)? {
            Event::Start(element) | Event::Empty(element)
                if element.name().as_ref() == b"p:sldSz" =>
            {
                return Ok((
                    attr_i64(&element, b"cx").unwrap_or(DEFAULT_WIDTH_EMU),
                    attr_i64(&element, b"cy").unwrap_or(DEFAULT_HEIGHT_EMU),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((DEFAULT_WIDTH_EMU, DEFAULT_HEIGHT_EMU))
}

fn color_value(element: &BytesStart<'_>) -> Option<String> {
    let value = attr_string(element, b"val")?;
    if value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(format!("#{value}"))
    } else {
        None
    }
}

fn apply_empty_element(
    element: &BytesStart<'_>,
    shape: &mut ShapeBuilder,
    in_run_properties: bool,
    in_line: bool,
) {
    match element.name().as_ref() {
        b"a:off" => {
            shape.x = attr_i64(element, b"x").unwrap_or(shape.x);
            shape.y = attr_i64(element, b"y").unwrap_or(shape.y);
        }
        b"a:ext" => {
            shape.width = attr_i64(element, b"cx").unwrap_or(shape.width).max(1);
            shape.height = attr_i64(element, b"cy").unwrap_or(shape.height).max(1);
        }
        b"a:blip" => {
            shape.relation_id =
                attr_string(element, b"r:embed").or_else(|| attr_string(element, b"embed"));
        }
        b"p:cNvPr" => {
            shape.alt = attr_string(element, b"descr")
                .or_else(|| attr_string(element, b"name"))
                .unwrap_or_default();
        }
        b"a:srgbClr" => {
            if let Some(color) = color_value(element) {
                if in_run_properties {
                    shape.color = color;
                } else if in_line {
                    shape.stroke = Some(color);
                } else {
                    shape.fill = Some(color);
                }
            }
        }
        b"a:rPr" | b"a:defRPr" => {
            if let Some(size) = attr_i64(element, b"sz") {
                shape.font_pt = (size as f32 / 100.0).clamp(6.0, 96.0);
            }
            shape.bold |= attr_string(element, b"b").as_deref() == Some("1");
        }
        _ => {}
    }
}

fn parse_slide_shapes(xml: &str) -> Result<Vec<ShapeBuilder>, ConvertError> {
    let mut reader = Reader::from_str(xml);
    let mut buffer = Vec::new();
    let mut current: Option<ShapeBuilder> = None;
    let mut shapes = Vec::new();
    let mut in_text = false;
    let mut in_run_properties = false;
    let mut in_line = false;

    loop {
        match reader.read_event_into(&mut buffer).map_err(fail)? {
            Event::Start(element) => match element.name().as_ref() {
                b"p:sp" | b"p:cxnSp" => current = Some(ShapeBuilder::new(ShapeKind::Text)),
                b"p:pic" => current = Some(ShapeBuilder::new(ShapeKind::Image)),
                b"p:graphicFrame" => current = Some(ShapeBuilder::new(ShapeKind::Graphic)),
                b"a:t" => in_text = true,
                b"a:rPr" | b"a:defRPr" => {
                    in_run_properties = true;
                    if let Some(shape) = current.as_mut() {
                        apply_empty_element(&element, shape, true, false);
                    }
                }
                b"a:ln" => in_line = true,
                _ => {
                    if let Some(shape) = current.as_mut() {
                        apply_empty_element(&element, shape, in_run_properties, in_line);
                    }
                }
            },
            Event::Empty(element) => {
                if let Some(shape) = current.as_mut() {
                    apply_empty_element(&element, shape, in_run_properties, in_line);
                }
            }
            Event::Text(text) if in_text => {
                if let Some(shape) = current.as_mut() {
                    shape.text.push_str(&text.unescape().map_err(fail)?);
                }
            }
            Event::End(element) => match element.name().as_ref() {
                b"a:t" => in_text = false,
                b"a:p" => {
                    if let Some(shape) = current.as_mut() {
                        if !shape.text.ends_with('\n') {
                            shape.text.push('\n');
                        }
                    }
                }
                b"a:rPr" | b"a:defRPr" => in_run_properties = false,
                b"a:ln" => in_line = false,
                b"p:sp" | b"p:cxnSp" | b"p:pic" | b"p:graphicFrame" => {
                    if let Some(mut shape) = current.take() {
                        shape.text = shape.text.trim().to_string();
                        shapes.push(shape);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(shapes)
}

fn relationship_map(xml: &str, slide_path: &str) -> Result<HashMap<String, String>, ConvertError> {
    let base = Path::new(slide_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut reader = Reader::from_str(xml);
    let mut buffer = Vec::new();
    let mut relationships = HashMap::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(fail)? {
            Event::Start(element) | Event::Empty(element)
                if element.name().as_ref() == b"Relationship" =>
            {
                if let (Some(id), Some(target)) = (
                    attr_string(&element, b"Id"),
                    attr_string(&element, b"Target"),
                ) {
                    relationships.insert(id, resolve_zip_target(base, &target));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(relationships)
}

fn resolve_zip_target(base: &Path, target: &str) -> String {
    let joined = base.join(target.replace('\\', "/"));
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            _ => {}
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}

fn image_mime(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("tif" | "tiff") => "image/tiff",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

pub fn preview_meta(path: &Path) -> Result<PptxPreviewMeta, ConvertError> {
    let file = std::fs::File::open(path).map_err(fail)?;
    let mut zip = ZipArchive::new(file).map_err(fail)?;
    let slide_count = slide_names(&zip).len();
    let dimensions = read_zip_text(&mut zip, "ppt/presentation.xml")
        .and_then(|xml| parse_dimensions(&xml))
        .unwrap_or((DEFAULT_WIDTH_EMU, DEFAULT_HEIGHT_EMU));
    Ok(PptxPreviewMeta {
        slide_count,
        width_emu: dimensions.0,
        height_emu: dimensions.1,
    })
}

pub fn preview_slide(path: &Path, index: usize) -> Result<PptxPreviewSlide, ConvertError> {
    let file = std::fs::File::open(path).map_err(fail)?;
    let mut zip = ZipArchive::new(file).map_err(fail)?;
    let slides = slide_names(&zip);
    let (_, slide_path) = slides
        .get(index)
        .ok_or_else(|| fail(format!("slide index ngoài phạm vi: {index}")))?
        .clone();
    let dimensions = read_zip_text(&mut zip, "ppt/presentation.xml")
        .and_then(|xml| parse_dimensions(&xml))
        .unwrap_or((DEFAULT_WIDTH_EMU, DEFAULT_HEIGHT_EMU));
    let xml = read_zip_text(&mut zip, &slide_path)?;
    let builders = parse_slide_shapes(&xml)?;
    let file_name = Path::new(&slide_path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail("tên slide không hợp lệ"))?;
    let rels_path = format!("ppt/slides/_rels/{file_name}.rels");
    let relationships = read_zip_text(&mut zip, &rels_path)
        .ok()
        .and_then(|rels| relationship_map(&rels, &slide_path).ok())
        .unwrap_or_default();

    let mut shapes = Vec::new();
    for builder in builders {
        match builder.kind {
            ShapeKind::Image => {
                let Some(target) = builder
                    .relation_id
                    .as_ref()
                    .and_then(|id| relationships.get(id))
                else {
                    continue;
                };
                let mut bytes = Vec::new();
                let Ok(mut media) = zip.by_name(target) else {
                    continue;
                };
                if media.read_to_end(&mut bytes).is_err() {
                    continue;
                }
                let data_url = format!(
                    "data:{};base64,{}",
                    image_mime(target),
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                );
                shapes.push(PptxPreviewShape::Image {
                    x: builder.x,
                    y: builder.y,
                    width: builder.width,
                    height: builder.height,
                    alt: builder.alt,
                    data_url,
                });
            }
            ShapeKind::Text | ShapeKind::Graphic if !builder.text.is_empty() => {
                shapes.push(PptxPreviewShape::Text {
                    x: builder.x,
                    y: builder.y,
                    width: builder.width,
                    height: builder.height,
                    text: builder.text,
                    font_pt: builder.font_pt,
                    bold: builder.bold,
                    color: builder.color,
                    fill: builder.fill,
                });
            }
            ShapeKind::Graphic => {
                shapes.push(PptxPreviewShape::Shape {
                    x: builder.x,
                    y: builder.y,
                    width: builder.width,
                    height: builder.height,
                    fill: builder.fill.or_else(|| Some("#f1f5f9".into())),
                    stroke: builder.stroke.or_else(|| Some("#94a3b8".into())),
                });
            }
            ShapeKind::Text => {
                shapes.push(PptxPreviewShape::Shape {
                    x: builder.x,
                    y: builder.y,
                    width: builder.width,
                    height: builder.height,
                    fill: builder.fill,
                    stroke: builder.stroke.or_else(|| Some("#94a3b8".into())),
                });
            }
            ShapeKind::Image => {}
        }
    }
    Ok(PptxPreviewSlide {
        index,
        width_emu: dimensions.0,
        height_emu: dimensions.1,
        background: "#ffffff".into(),
        shapes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positioned_text_and_style() {
        let xml = r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree>
          <p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/></p:nvSpPr>
          <p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="300" cy="400"/></a:xfrm>
          <a:solidFill><a:srgbClr val="DDEEFF"/></a:solidFill></p:spPr>
          <p:txBody><a:p><a:r><a:rPr sz="2400" b="1"><a:solidFill>
          <a:srgbClr val="112233"/></a:solidFill></a:rPr><a:t>Xin chào</a:t>
          </a:r></a:p></p:txBody></p:sp>
        </p:spTree></p:cSld></p:sld>"#;
        let shapes = parse_slide_shapes(xml).unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].text, "Xin chào");
        assert_eq!((shapes[0].x, shapes[0].y), (100, 200));
        assert_eq!((shapes[0].width, shapes[0].height), (300, 400));
        assert_eq!(shapes[0].font_pt, 24.0);
        assert!(shapes[0].bold);
    }

    #[test]
    fn resolves_relative_media_relationships() {
        assert_eq!(
            resolve_zip_target(Path::new("ppt/slides"), "../media/image1.png"),
            "ppt/media/image1.png"
        );
    }

    #[test]
    fn slide_names_are_numeric_not_lexical() {
        assert_eq!(slide_number("ppt/slides/slide12.xml"), Some(12));
        assert_eq!(slide_number("ppt/slides/slideX.xml"), None);
    }
}
