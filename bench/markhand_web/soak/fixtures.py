"""Deterministic synthetic fixtures for all phase1b-mixed ingest formats.

Small stdlib-generated files under ``soak/fixtures/`` so the harness does not
depend on optional golden corpus presence. Preflight fails closed if any
profile format is missing or has invalid magic/extension.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path
from typing import Any
from zipfile import ZIP_DEFLATED, ZipFile


ROOT = Path(__file__).resolve().parent
FIXTURE_DIR = ROOT / "fixtures"

# Minimal magic prefixes / OOXML markers per format.
MAGIC_CHECKS: dict[str, tuple[bytes, ...]] = {
    "pdf": (b"%PDF",),
    "docx": (b"PK",),
    "pptx": (b"PK",),
    "xlsx": (b"PK",),
    "csv": (b"id,", b"a,", b"col"),  # text; checked loosely
    "html": (b"<!DOCTYPE", b"<html", b"<HTML"),
    "txt": (b"markhand", b"Markhand", b"soak"),
    "png": (b"\x89PNG\r\n\x1a\n",),
}

CONTENT_TYPES: dict[str, str] = {
    "pdf": "application/pdf",
    "docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "csv": "text/csv",
    "html": "text/html",
    "txt": "text/plain",
    "png": "image/png",
}


class FixtureError(RuntimeError):
    """Missing or invalid soak fixture."""


def fixture_filename(fmt: str) -> str:
    return f"soak-{fmt.lower()}.{fmt.lower()}"


def fixture_path(fmt: str, *, base: Path | None = None) -> Path:
    return (base or FIXTURE_DIR) / fixture_filename(fmt)


def _write_png(path: Path) -> None:
    """1x1 opaque PNG via stdlib only."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    raw = b"\x00\xff\xff\xff"  # filter + RGB
    ihdr = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(
        b"IEND", b""
    )
    path.write_bytes(data)


def _write_ooxml(path: Path, *, kind: str) -> None:
    """Minimal ZIP-shaped OOXML so magic is PK and extension matches."""
    # Content types / package parts are stubbed; upload path only needs bytes+ext.
    content_types = """<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
</Types>
"""
    rels = """<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>
"""
    with ZipFile(path, "w", compression=ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml", content_types)
        zf.writestr("_rels/.rels", rels)
        zf.writestr(f"markhand/soak-{kind}.txt", f"markhand soak {kind} fixture\n")


def ensure_fixtures(formats: list[str], *, base: Path | None = None) -> dict[str, Path]:
    """Generate missing fixtures and return resolved paths for each format."""
    base = base or FIXTURE_DIR
    base.mkdir(parents=True, exist_ok=True)
    resolved: dict[str, Path] = {}
    for fmt in formats:
        key = fmt.lower()
        path = fixture_path(key, base=base)
        if not path.is_file():
            if key == "pdf":
                path.write_bytes(
                    b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n"
                )
            elif key == "docx":
                _write_ooxml(path, kind="docx")
            elif key == "pptx":
                _write_ooxml(path, kind="pptx")
            elif key == "xlsx":
                _write_ooxml(path, kind="xlsx")
            elif key == "csv":
                path.write_text("id,value\n1,markhand-soak\n", encoding="utf-8")
            elif key == "html":
                path.write_text(
                    "<!DOCTYPE html><html><body>markhand soak</body></html>\n",
                    encoding="utf-8",
                )
            elif key == "txt":
                path.write_text("markhand soak fixture\n", encoding="utf-8")
            elif key == "png":
                _write_png(path)
            else:
                raise FixtureError(f"unsupported_format:{key}")
        resolved[key] = path
    return resolved


def validate_magic(fmt: str, path: Path) -> None:
    key = fmt.lower()
    if not path.is_file():
        raise FixtureError(f"missing:{key}:{path}")
    if path.suffix.lstrip(".").lower() != key:
        raise FixtureError(f"extension_mismatch:{key}:{path.name}")
    head = path.read_bytes()[:64]
    expected = MAGIC_CHECKS.get(key)
    if not expected:
        raise FixtureError(f"no_magic_rule:{key}")
    if not any(head.startswith(m) or m in head for m in expected):
        raise FixtureError(f"magic_mismatch:{key}")


def preflight_fixtures(
    formats: list[str],
    *,
    base: Path | None = None,
    generate: bool = True,
) -> dict[str, Any]:
    """Resolve all profile formats with valid magic. Fail closed on any gap.

    When ``generate`` is True (default), missing fixtures are created first.
    When False, missing files raise immediately (no silent skip).
    """
    if generate:
        ensure_fixtures(formats, base=base)
    paths: dict[str, str] = {}
    errors: list[str] = []
    for fmt in formats:
        path = fixture_path(fmt, base=base)
        try:
            if not path.is_file():
                raise FixtureError(f"missing:{fmt.lower()}:{path}")
            validate_magic(fmt, path)
            paths[fmt.lower()] = str(path)
        except FixtureError as exc:
            errors.append(str(exc))
    if errors:
        raise FixtureError("preflight_failed:" + ",".join(errors))
    return {"ok": True, "paths": paths, "formats": sorted({f.lower() for f in formats})}
