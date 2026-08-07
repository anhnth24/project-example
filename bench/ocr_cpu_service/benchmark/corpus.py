"""Verified benchmark pages and deterministic qualitative samples."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlsplit

import pypdfium2 as pdfium
from PIL import Image, ImageDraw, ImageFont

from benchmark.render import bounded_sample_render_limits, render_page
from corpus.download import CorpusSource, load_sources

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_MANIFEST = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_CORPUS = SERVICE_ROOT / ".data" / "corpus"

HISTORICAL_READING_ORDER_ANCHORS = {
    ("wikimedia-dai-nam-1907-804", 4): (
        "NHỜI ĐÀN BÀ",
        "RAO HẸN",
        "TẬP THƠ, PHÚ, CA, RAO",
        "CÁO BẠCH",
        "HIỆN BÁO HOÀN CẦU",
    ),
}


@dataclass(frozen=True, slots=True)
class BenchmarkPage:
    source_id: str
    source_sha256: str
    stratum: str
    page_number: int
    path: Path
    reference: str | None
    gate_included: bool = True
    reading_order_anchors: tuple[str, ...] = ()


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _destination(source: CorpusSource) -> str:
    remote = PurePosixPath(urlsplit(source.url).path).name
    return source.id + "".join(Path(remote).suffixes)


def _verified_path(source: CorpusSource, corpus_dir: Path) -> Path:
    path = corpus_dir / _destination(source)
    if not path.is_file():
        raise FileNotFoundError(f"missing validated corpus asset: {source.id}")
    if _sha256(path) != source.sha256:
        raise ValueError(f"corpus checksum mismatch: {source.id}")
    return path


def load_quantitative_pages(
    manifest: Path = DEFAULT_MANIFEST,
    corpus_dir: Path = DEFAULT_CORPUS,
) -> list[BenchmarkPage]:
    """Load only pinned nrl-ai pages with human-verified reference text."""
    sources = load_sources(manifest)
    by_remote_name = {
        PurePosixPath(urlsplit(source.url).path).name: source
        for source in sources
        if source.kind in {"real-scan", "synthetic-scan"}
        and source.classification != "metadata"
    }
    pages: list[BenchmarkPage] = []
    metadata_sources = [
        source
        for source in sources
        if source.id in {"vnocr-real-metadata", "vnocr-synthetic-scan-metadata"}
    ]
    for metadata_source in sorted(metadata_sources, key=lambda item: item.id):
        metadata_path = _verified_path(metadata_source, corpus_dir)
        for line in metadata_path.read_text(encoding="utf-8").splitlines():
            row = json.loads(line)
            source = by_remote_name.get(row["file_name"])
            if source is None:
                raise ValueError(
                    f"metadata references an unpinned image: {row['file_name']}"
                )
            reference = row.get("text")
            if not isinstance(reference, str) or not reference.strip():
                raise ValueError(f"missing human-verified text: {source.id}")
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum=source.kind,
                    page_number=1,
                    path=_verified_path(source, corpus_dir),
                    reference=reference,
                )
            )
    pages.sort(key=lambda page: (page.stratum, page.source_id, page.page_number))
    if len(pages) != len(by_remote_name):
        raise ValueError("not every pinned quantitative image has verified text")
    return pages


def deterministic_page_sample(page_count: int) -> tuple[int, ...]:
    if page_count <= 0:
        raise ValueError("page_count must be positive")
    return tuple(sorted({1, (page_count + 1) // 2, page_count}))


def historical_reading_order_anchors(
    source_id: str, page_number: int
) -> tuple[str, ...]:
    """Return the small human-reviewed sequence for a pinned historical page."""
    return HISTORICAL_READING_ORDER_ANCHORS.get(
        (source_id, page_number), ()
    )


def generate_reviewed_multicolumn_case(
    work_dir: Path,
) -> tuple[dict[str, Any], BenchmarkPage]:
    """Create a deterministic two-column page with source-ground-truth order."""
    work_dir.mkdir(parents=True, exist_ok=True)
    output = work_dir / "reviewed-multicolumn-v1.png"
    font_path = Path(
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
    )
    font = ImageFont.truetype(str(font_path), 44)
    heading_font = ImageFont.truetype(str(font_path), 54)
    anchors = ("L1", "L2", "L3", "R1", "R2", "R3")
    left = (
        ("L1", "Dòng trái thứ nhất"),
        ("L2", "Dòng trái thứ hai"),
        ("L3", "Dòng trái thứ ba"),
    )
    right = (
        ("R1", "Dòng phải thứ nhất"),
        ("R2", "Dòng phải thứ hai"),
        ("R3", "Dòng phải thứ ba"),
    )
    image = Image.new("RGB", (1600, 1000), "white")
    draw = ImageDraw.Draw(image)
    draw.text(
        (100, 70),
        "Trường hợp kiểm tra thứ tự hai cột",
        fill="black",
        font=heading_font,
    )
    for x, rows in ((100, left), (880, right)):
        for index, (anchor, text) in enumerate(rows):
            draw.text(
                (x, 230 + index * 190),
                f"{anchor} {text}",
                fill="black",
                font=font,
            )
    try:
        image.save(output, format="PNG", optimize=False, compress_level=9)
    finally:
        image.close()
    image_sha256 = _sha256(output)
    metadata = {
        "source_id": "reviewed-multicolumn-v1",
        "classification": "synthetic-scan",
        "layout": "two-column-column-major",
        "ground_truth": "deterministic-source",
        "review_status": "reviewed-fixture-contract",
        "expected_anchors": len(anchors),
        "page_number": 1,
        "expected_sequence": list(anchors),
        "image_sha256": image_sha256,
        "font_sha256": _sha256(font_path),
        "generator": "Pillow fixed canvas, coordinates, font, and PNG settings",
    }
    return (
        metadata,
        BenchmarkPage(
            source_id="reviewed-multicolumn-v1",
            source_sha256=image_sha256,
            stratum="reviewed-multicolumn",
            page_number=1,
            path=output,
            reference=None,
            gate_included=False,
            reading_order_anchors=anchors,
        ),
    )


def inspect_and_render_official(
    source: CorpusSource,
    corpus_dir: Path,
    work_dir: Path,
    *,
    dpi: int = 200,
    benchmark_stratum: str = "mixed",
) -> tuple[dict[str, Any], list[BenchmarkPage]]:
    """Inspect every PDF page, then render one bounded deterministic sample."""
    path = _verified_path(source, corpus_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    document = pdfium.PdfDocument(path)
    try:
        page_count = len(document)
        text_pages = 0
        image_pages = 0
        for index in range(page_count):
            page = document[index]
            try:
                text_page = page.get_textpage()
                try:
                    if text_page.get_text_range().strip():
                        text_pages += 1
                finally:
                    text_page.close()
                if any(
                    True
                    for _ in page.get_objects(
                        filter=(pdfium.raw.FPDF_PAGEOBJ_IMAGE,)
                    )
                ):
                    image_pages += 1
            finally:
                page.close()
        classification = (
            "mixed"
            if text_pages and image_pages
            else "native"
            if text_pages
            else "scan"
            if image_pages
            else "unknown"
        )
        sampled = deterministic_page_sample(page_count)
        pages: list[BenchmarkPage] = []
        sampled_page_classification: dict[str, str] = {}
        sampled_page_dpi: dict[str, int] = {}
        for page_number in sampled:
            output = work_dir / f"{source.id}-p{page_number}.png"
            source_page = document[page_number - 1]
            try:
                text_page = source_page.get_textpage()
                try:
                    has_text = bool(text_page.get_text_range().strip())
                finally:
                    text_page.close()
                has_image = any(
                    True
                    for _ in source_page.get_objects(
                        filter=(pdfium.raw.FPDF_PAGEOBJ_IMAGE,)
                    )
                )
                sampled_page_classification[str(page_number)] = (
                    "mixed"
                    if has_text and has_image
                    else "native"
                    if has_text
                    else "scan"
                    if has_image
                    else "empty"
                )
                width, height = source_page.get_size()
                limits = bounded_sample_render_limits(
                    page_width=width,
                    page_height=height,
                    requested_dpi=dpi,
                )
                sampled_page_dpi[str(page_number)] = limits.dpi
                image = render_page(source_page, limits)
                try:
                    image.save(output, format="PNG")
                finally:
                    image.close()
            finally:
                source_page.close()
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum=benchmark_stratum,
                    page_number=page_number,
                    path=output,
                    reference=None,
                    gate_included=False,
                    reading_order_anchors=historical_reading_order_anchors(
                        source.id, page_number
                    ),
                )
            )
    finally:
        document.close()
    return (
        {
            "source_id": source.id,
            "source_sha256": source.sha256,
            "classification": classification,
            "manifest_classification": source.classification,
            "classification_mismatch": source.classification != classification,
            "classification_evidence": {
                "pages": page_count,
                "text_pages": text_pages,
                "image_pages": image_pages,
            },
            "sampled_pages": list(sampled),
            "sampled_page_classification": sampled_page_classification,
            "requested_render_dpi": dpi,
            "sampled_page_render_dpi": sampled_page_dpi,
            "rendered_page_sha256": {
                str(page.page_number): _sha256(page.path) for page in pages
            },
        },
        pages,
    )


def inspect_and_render_historical(
    sources: list[CorpusSource],
    corpus_dir: Path,
    work_dir: Path,
    *,
    dpi: int = 200,
) -> tuple[list[dict[str, Any]], list[BenchmarkPage]]:
    """Render bounded manifest-pinned historical samples as qualitative evidence."""
    evidence: list[dict[str, Any]] = []
    pages: list[BenchmarkPage] = []
    for source in sorted(sources, key=lambda item: item.id):
        source_evidence, source_pages = inspect_and_render_official(
            source,
            corpus_dir,
            work_dir,
            dpi=dpi,
            benchmark_stratum="historical-scan",
        )
        source_evidence["transcription"] = "none-trustworthy-available"
        source_evidence["evidence_mode"] = "qualitative-only"
        reviewed_pages = [
            page for page in source_pages if page.reading_order_anchors
        ]
        if reviewed_pages:
            if len(reviewed_pages) != 1:
                raise ValueError(
                    f"{source.id}: expected one reviewed reading-order page"
                )
            reviewed_page = reviewed_pages[0]
            source_evidence["reading_order_review"] = {
                "page_number": reviewed_page.page_number,
                "review_status": "human-reviewed-short-anchors",
                "expected_anchors": len(
                    reviewed_page.reading_order_anchors
                ),
                "expected_sequence": list(
                    reviewed_page.reading_order_anchors
                ),
                "matching": (
                    "accent/punctuation folded; <=25% character edits"
                ),
            }
        evidence.append(source_evidence)
        pages.extend(source_pages)
    return evidence, pages
