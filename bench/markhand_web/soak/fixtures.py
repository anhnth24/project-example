"""Converter-accepted synthetic fixtures modeled on Rust tiny_* helpers.

Generates deterministic PDF/DOCX/PPTX/XLSX/PNG/CSV/HTML/TXT that the production
``fileconv`` converter can extract markers from. Magic-only stubs are rejected
by structural + converter preflight.
"""

from __future__ import annotations

import json
import shutil
import struct
import subprocess
import zlib
from pathlib import Path
from typing import Any
from zipfile import ZIP_STORED, ZipFile


ROOT = Path(__file__).resolve().parents[3]
SOAK_DIR = Path(__file__).resolve().parent
FIXTURE_DIR = SOAK_DIR / "fixtures"

MARKERS: dict[str, str] = {
    "pdf": "SOAKPDF15",
    "docx": "SOAKDOCX15",
    "pptx": "SOAKPPTX15",
    "xlsx": "SOAKXLSX15",
    "csv": "SOAKCSV15",
    "html": "SOAKHTML15",
    "txt": "SOAKTXT15",
    "png": "SOAK15",
}

REQUIRED_OOXML_PARTS: dict[str, tuple[str, ...]] = {
    "docx": ("[Content_Types].xml", "_rels/.rels", "word/document.xml"),
    "pptx": (
        "[Content_Types].xml",
        "_rels/.rels",
        "ppt/presentation.xml",
        "ppt/slides/slide1.xml",
    ),
    "xlsx": (
        "[Content_Types].xml",
        "_rels/.rels",
        "xl/workbook.xml",
        "xl/worksheets/sheet1.xml",
        "xl/sharedStrings.xml",
    ),
}


class FixtureError(RuntimeError):
    """Missing/invalid soak fixture or converter preflight failure."""


def fixture_filename(fmt: str) -> str:
    return f"soak-{fmt.lower()}.{fmt.lower()}"


def fixture_path(fmt: str, *, base: Path | None = None) -> Path:
    return (base or FIXTURE_DIR) / fixture_filename(fmt)


def marker_for(fmt: str) -> str:
    return MARKERS[fmt.lower()]


def _zip_parts(parts: list[tuple[str, bytes]]) -> bytes:
    import io

    buf = io.BytesIO()
    with ZipFile(buf, "w", compression=ZIP_STORED) as zf:
        for name, data in parts:
            zf.writestr(name, data)
    return buf.getvalue()


def tiny_pdf_bytes(marker: str) -> bytes:
    text = f"BT /F1 12 Tf 40 100 Td ({marker}) Tj ET"
    stream = f"<< /Length {len(text)} >>stream\n{text}\nendstream"
    objects = [
        "1 0 obj<< /Type /Catalog /Pages 2 0 R >>endobj\n",
        "2 0 obj<< /Type /Pages /Kids [3 0 R] /Count 1 >>endobj\n",
        "3 0 obj<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources<< /Font<< /F1 5 0 R >> >> >>endobj\n",
        f"4 0 obj{stream}\nendobj\n",
        "5 0 obj<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>endobj\n",
    ]
    body = "%PDF-1.4\n"
    offsets = [0]
    for obj in objects:
        offsets.append(len(body))
        body += obj
    xref_at = len(body)
    body += f"xref\n0 {len(offsets)}\n"
    body += "0000000000 65535 f \n"
    for offset in offsets[1:]:
        body += f"{offset:010d} 00000 n \n"
    body += f"trailer<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
    return body.encode("latin-1")


def tiny_docx_bytes(marker: str) -> bytes:
    content_types = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""
    rels = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""
    document = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{marker}</w:t></w:r></w:p></w:body>
</w:document>""".encode()
    return _zip_parts(
        [
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", rels),
            ("word/document.xml", document),
        ]
    )


def tiny_pptx_bytes(marker: str) -> bytes:
    content_types = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"""
    rels = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"""
    presentation = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"""
    presentation_rels = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"""
    slide = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
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
</p:sld>""".encode()
    return _zip_parts(
        [
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", rels),
            ("ppt/presentation.xml", presentation),
            ("ppt/_rels/presentation.xml.rels", presentation_rels),
            ("ppt/slides/slide1.xml", slide),
        ]
    )


def tiny_xlsx_bytes(marker: str) -> bytes:
    content_types = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"""
    rels = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"""
    workbook = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets>
</workbook>"""
    workbook_rels = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"""
    shared = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1">
  <si><t>{marker}</t></si>
</sst>""".encode()
    sheet = b"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData>
</worksheet>"""
    return _zip_parts(
        [
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", rels),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", workbook_rels),
            ("xl/sharedStrings.xml", shared),
            ("xl/worksheets/sheet1.xml", sheet),
        ]
    )


_GLYPHS: dict[str, list[int]] = {
    " ": [0, 0, 0, 0, 0, 0, 0],
    "0": [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    "1": [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    "5": [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    "A": [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
    "K": [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
    "O": [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
    "S": [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
}


def _png_chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


# Deterministic OCR-accepted PNG for marker SOAK15 (DejaVu Bold 72pt render).
# The Rust 5×7 bitmap in tiny_png_ocr_bytes is the structural model; system /
# tessdata_best Tesseract does not recover that bitmap reliably here, so soak
# ships a citation-producing TrueType render with the same marker contract.
_SOAK15_PNG_B64 = (
    "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"
    "c445J3b6L06r0EyLxIT1AAAAAElFTkSuQmCC"
)


def _bitmap_png_bytes(marker: str, *, scale: int = 6, pad: int = 16) -> bytes:
    """Rust-style 5×7 bitmap PNG (structural model; OCR may fail)."""
    text = "".join(c for c in marker.upper() if c in _GLYPHS)
    if not text:
        text = "SOAK15"
    glyph_w, glyph_h = 6, 7
    width = pad * 2 + glyph_w * len(text) * scale
    height = pad * 2 + glyph_h * scale
    rows = [[255] * width for _ in range(height)]
    for i, ch in enumerate(text):
        bits = _GLYPHS.get(ch)
        if not bits:
            continue
        ox = pad + i * glyph_w * scale
        oy = pad
        for row in range(7):
            for col in range(5):
                if bits[row] & (1 << (4 - col)):
                    for dy in range(scale):
                        for dx in range(scale):
                            rows[oy + row * scale + dy][ox + col * scale + dx] = 0
    raw = b"".join(b"\x00" + bytes(row) for row in rows)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(raw, 9))
        + _png_chunk(b"IEND", b"")
    )


def _truetype_png_bytes(marker: str) -> bytes | None:
    """Optional Pillow render when fonts are available."""
    try:
        from PIL import Image, ImageDraw, ImageFont  # type: ignore
    except ImportError:
        return None
    import io

    font_paths = (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    )
    font = None
    for path in font_paths:
        if Path(path).is_file():
            font = ImageFont.truetype(path, 72)
            break
    if font is None:
        return None
    probe = Image.new("L", (1, 1), 255)
    draw = ImageDraw.Draw(probe)
    bbox = draw.textbbox((0, 0), marker, font=font)
    width = bbox[2] - bbox[0]
    height = bbox[3] - bbox[1]
    pad = 24
    img = Image.new("L", (width + 2 * pad, height + 2 * pad), 255)
    draw = ImageDraw.Draw(img)
    draw.text((pad - bbox[0], pad - bbox[1]), marker, fill=0, font=font)
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def tiny_png_ocr_bytes(marker: str) -> bytes:
    """High-contrast PNG that fileconv/Tesseract recovers ``marker`` from.

    Modeled on Rust ``tiny_png_ocr_bytes`` (same marker contract / greyscale PNG).
    Prefer deterministic golden bytes for SOAK15, then TrueType render, then
    the 5×7 bitmap fallback used for structural negative tests.
    """
    import base64

    if marker == MARKERS["png"]:
        return base64.b64decode(_SOAK15_PNG_B64)
    rendered = _truetype_png_bytes(marker)
    if rendered is not None:
        return rendered
    return _bitmap_png_bytes(marker, scale=20, pad=32)


def invalid_stub_bytes(fmt: str) -> bytes:
    """Intentionally invalid magic-only stubs (for negative tests)."""
    key = fmt.lower()
    if key in {"docx", "pptx", "xlsx"}:
        return _zip_parts(
            [
                ("[Content_Types].xml", b"<Types/>"),
                ("_rels/.rels", b"<Relationships/>"),
                (f"markhand/soak-{key}.txt", b"stub\n"),
            ]
        )
    if key == "pdf":
        return b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n"
    if key == "png":
        # 1x1 white PNG without OCR text
        raw = b"\x00\xff"
        ihdr = struct.pack(">IIBBBBB", 1, 1, 8, 0, 0, 0, 0)
        return (
            b"\x89PNG\r\n\x1a\n"
            + _png_chunk(b"IHDR", ihdr)
            + _png_chunk(b"IDAT", zlib.compress(raw))
            + _png_chunk(b"IEND", b"")
        )
    return b"invalid"


def generate_bytes(fmt: str) -> bytes:
    key = fmt.lower()
    marker = marker_for(key)
    if key == "pdf":
        return tiny_pdf_bytes(marker)
    if key == "docx":
        return tiny_docx_bytes(marker)
    if key == "pptx":
        return tiny_pptx_bytes(marker)
    if key == "xlsx":
        return tiny_xlsx_bytes(marker)
    if key == "png":
        return tiny_png_ocr_bytes(marker)
    if key == "csv":
        return f"id,value\n1,{marker}\n".encode()
    if key == "html":
        return f"<!DOCTYPE html><html><body>{marker}</body></html>\n".encode()
    if key == "txt":
        return f"{marker}\n".encode()
    raise FixtureError(f"unsupported_format:{key}")


def ensure_fixtures(formats: list[str], *, base: Path | None = None, force: bool = False) -> dict[str, Path]:
    base = base or FIXTURE_DIR
    base.mkdir(parents=True, exist_ok=True)
    out: dict[str, Path] = {}
    for fmt in formats:
        path = fixture_path(fmt, base=base)
        if force or not path.is_file():
            path.write_bytes(generate_bytes(fmt))
        out[fmt.lower()] = path
    return out


def validate_structure(fmt: str, path: Path) -> None:
    key = fmt.lower()
    if not path.is_file():
        raise FixtureError(f"missing:{key}:{path}")
    if path.suffix.lstrip(".").lower() != key:
        raise FixtureError(f"extension_mismatch:{key}:{path.name}")
    data = path.read_bytes()
    if key == "pdf":
        if not data.startswith(b"%PDF"):
            raise FixtureError("magic_mismatch:pdf")
        if b"/Type /Catalog" not in data or b"/Font" not in data:
            raise FixtureError("structural_invalid:pdf")
        if marker_for("pdf").encode() not in data:
            raise FixtureError("marker_missing:pdf")
        return
    if key in REQUIRED_OOXML_PARTS:
        if not data.startswith(b"PK"):
            raise FixtureError(f"magic_mismatch:{key}")
        import io

        with ZipFile(io.BytesIO(data)) as zf:
            names = set(zf.namelist())
            for part in REQUIRED_OOXML_PARTS[key]:
                if part not in names:
                    raise FixtureError(f"structural_missing:{key}:{part}")
            # Marker must appear in package bytes.
            blob = b"".join(zf.read(n) for n in zf.namelist())
            if marker_for(key).encode() not in blob:
                raise FixtureError(f"marker_missing:{key}")
        return
    if key == "png":
        if not data.startswith(b"\x89PNG\r\n\x1a\n"):
            raise FixtureError("magic_mismatch:png")
        # Reject 1x1 / tiny stubs: OCR fixtures must carry real bitmap text.
        if len(data) < 200:
            raise FixtureError("structural_invalid:png_too_small")
        if len(data) >= 24:
            # IHDR width/height at bytes 16..24 (after 8-byte sig + 8-byte chunk hdr).
            width = struct.unpack(">I", data[16:20])[0]
            height = struct.unpack(">I", data[20:24])[0]
            if width < 32 or height < 32:
                raise FixtureError(f"structural_invalid:png_dims:{width}x{height}")
        return
    if key in {"csv", "html", "txt"}:
        text = data.decode("utf-8", errors="replace")
        if marker_for(key) not in text:
            raise FixtureError(f"marker_missing:{key}")
        return
    raise FixtureError(f"unsupported_format:{key}")


def resolve_fileconv() -> Path | None:
    for rel in ("target/debug/fileconv", "target/release/fileconv"):
        path = ROOT / rel
        if path.is_file() and os_access_executable(path):
            return path
    which = shutil.which("fileconv")
    return Path(which) if which else None


def os_access_executable(path: Path) -> bool:
    import os

    return os.access(path, os.X_OK)


def convert_with_fileconv(path: Path, *, fileconv: Path) -> str:
    proc = subprocess.run(
        [str(fileconv), "one", str(path)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    if proc.returncode != 0:
        raise FixtureError(
            f"fileconv_failed:{path.name}:rc={proc.returncode}:{(proc.stderr or '')[:200]}"
        )
    return (proc.stdout or "") + (proc.stderr or "")


def preflight_fixtures(
    formats: list[str],
    *,
    base: Path | None = None,
    generate: bool = True,
    require_converter: bool | None = None,
) -> dict[str, Any]:
    """Structural + converter preflight. Fail closed on any gap.

    When ``fileconv`` is available (or require_converter=True), every fixture
    must convert with non-empty marker output. Magic-only stubs fail structure.
    """
    if generate:
        ensure_fixtures(formats, base=base, force=True)
    paths: dict[str, str] = {}
    errors: list[str] = []
    convert_results: dict[str, Any] = {}
    fileconv = resolve_fileconv()
    if require_converter is None:
        require_converter = fileconv is not None
    if require_converter and fileconv is None:
        raise FixtureError("fileconv_binary_missing")

    for fmt in formats:
        path = fixture_path(fmt, base=base)
        try:
            validate_structure(fmt, path)
            paths[fmt.lower()] = str(path)
            if fileconv is not None:
                md = convert_with_fileconv(path, fileconv=fileconv)
                marker = marker_for(fmt)
                if marker not in md and marker.lower() not in md.lower():
                    # PNG OCR may vary casing; accept casefold.
                    if fmt.lower() != "png" or marker.casefold() not in md.casefold():
                        raise FixtureError(f"converter_marker_missing:{fmt}:{marker}")
                if not md.strip():
                    raise FixtureError(f"converter_empty:{fmt}")
                convert_results[fmt.lower()] = {
                    "ok": True,
                    "marker": marker,
                    "outputChars": len(md),
                }
        except FixtureError as exc:
            errors.append(str(exc))
            convert_results[fmt.lower()] = {"ok": False, "error": str(exc)}

    if errors:
        raise FixtureError("preflight_failed:" + ",".join(errors))
    return {
        "ok": True,
        "paths": paths,
        "formats": sorted({f.lower() for f in formats}),
        "fileconv": str(fileconv) if fileconv else None,
        "converterChecked": fileconv is not None,
        "convertResults": convert_results,
        "markers": {f: marker_for(f) for f in formats},
    }
