#!/usr/bin/env python3
"""Download and validate 10 public files for every converter family."""

from __future__ import annotations

import hashlib
import json
import time
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUTPUT = ROOT / "corpus10"
USER_AGENT = "Markhand public conversion benchmark/1.0"
MAX_BYTES = 20 * 1024 * 1024


def github(repo: str, path: str) -> str:
    return f"https://raw.githubusercontent.com/{repo}/master/{path}"


SOURCES: dict[str, list[tuple[str, str, str]]] = {
    "pdf": [
        (f"arxiv-{paper}.pdf", f"https://arxiv.org/pdf/{paper}", "arXiv public paper")
        for paper in [
            "1706.03762",
            "1810.04805",
            "1512.03385",
            "1409.1556",
            "1412.6980",
            "1506.02640",
            "1505.04597",
            "1810.13243",
            "2010.11929",
            "1707.06347",
        ]
    ],
    "docx": [
        (
            f"{name}.docx",
            github(
                "python-openxml/python-docx",
                f"features/steps/test_files/{name}.docx",
            ),
            "python-docx MIT fixture",
        )
        for name in [
            "blk-containing-table",
            "blk-paras-and-tables",
            "doc-default",
            "doc-coreprops",
            "par-known-paragraphs",
            "par-hyperlinks",
            "hdr-header-footer",
            "run-char-style",
            "par-alignment",
            "doc-odd-even-hdrs",
        ]
    ],
    "pptx": [
        (
            f"{name}.pptx",
            github(
                "scanny/python-pptx",
                f"features/steps/test_files/{name}.pptx",
            ),
            "python-pptx MIT fixture",
        )
        for name in [
            "shp-shapes",
            "shp-autoshape-props",
            "shp-common-props",
            "sld-blank",
            "shp-picture",
            "shp-groupshape",
            "shp-freeform",
            "shp-connector-props",
            "shp-pos-and-size",
            "shp-access-chart",
        ]
    ],
    "spreadsheet": [
        (name, github("tafia/calamine", f"tests/{name}"), "calamine MIT fixture")
        for name in [
            "merged_range.xlsx",
            "inventory-table.xlsx",
            "any_sheets.xlsx",
            "merged_range.xls",
            "date.xls",
            "issue127.xls",
            "any_sheets.xlsb",
            "date.xlsb",
            "merged_cells.ods",
            "date.ods",
        ]
    ],
    "csv": [
        (
            f"{name}.csv",
            f"https://people.sc.fsu.edu/~jburkardt/data/csv/{name}.csv",
            "FSU teaching dataset",
        )
        for name in [
            "airtravel",
            "biostats",
            "cities",
            "deniro",
            "faithful",
            "grades",
            "homes",
            "hw_200",
            "mlb_players",
            "snakes_count_10000",
        ]
    ],
    "html": [
        ("example.html", "https://example.com/", "IANA example"),
        (
            "iana-example-domains.html",
            "https://www.iana.org/help/example-domains",
            "IANA documentation",
        ),
        ("gpl3.html", "https://www.gnu.org/licenses/gpl-3.0.en.html", "GNU GPL text"),
        ("rust-book.html", "https://doc.rust-lang.org/book/title-page.html", "Rust docs"),
        ("w3c-png.html", "https://www.w3.org/TR/PNG/", "W3C specification"),
        ("wiki-markdown.html", "https://en.wikipedia.org/wiki/Markdown", "CC BY-SA"),
        (
            "wiki-rust.html",
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "CC BY-SA",
        ),
        (
            "wiki-vietnam-vi.html",
            "https://vi.wikipedia.org/wiki/Vi%E1%BB%87t_Nam",
            "CC BY-SA",
        ),
        (
            "wiki-markdown-vi.html",
            "https://vi.wikipedia.org/wiki/Markdown",
            "CC BY-SA",
        ),
        (
            "python-tutorial.html",
            "https://docs.python.org/3/tutorial/index.html",
            "Python documentation",
        ),
    ],
    "image": [
        (
            "monty-truth.png",
            github(
                "python-openxml/python-docx",
                "features/steps/test_files/monty-truth.png",
            ),
            "python-docx MIT fixture",
        ),
        (
            "python-powered.png",
            github(
                "python-openxml/python-docx",
                "tests/test_files/python-powered.png",
            ),
            "python-docx MIT fixture",
        ),
        (
            "test.png",
            github("python-openxml/python-docx", "features/steps/test_files/test.png"),
            "python-docx MIT fixture",
        ),
        (
            "jfif-300-dpi.jpg",
            github(
                "python-openxml/python-docx",
                "features/steps/test_files/jfif-300-dpi.jpg",
            ),
            "python-docx MIT fixture",
        ),
        (
            "sample.tif",
            github(
                "python-openxml/python-docx",
                "features/steps/test_files/sample.tif",
            ),
            "python-docx MIT fixture",
        ),
        (
            "tesseract-2col.png",
            github("tesseract-ocr/tessdoc", "images/2col.png").replace(
                "/master/", "/main/"
            ),
            "Tesseract Apache-2.0 docs",
        ),
        (
            "tesseract-bilingual.png",
            github("tesseract-ocr/tessdoc", "images/bilingual.png").replace(
                "/master/", "/main/"
            ),
            "Tesseract Apache-2.0 docs",
        ),
        (
            "tesseract-eurotext.png",
            github("tesseract-ocr/tessdoc", "images/eurotext.png").replace(
                "/master/", "/main/"
            ),
            "Tesseract Apache-2.0 docs",
        ),
        (
            "tesseract-phototest.tif",
            github("tesseract-ocr/tessdoc", "examples/phototest.tif").replace(
                "/master/", "/main/"
            ),
            "Tesseract Apache-2.0 docs",
        ),
        (
            "tesseract-toc.png",
            github("tesseract-ocr/tessdoc", "images/toc.png").replace(
                "/master/", "/main/"
            ),
            "Tesseract Apache-2.0 docs",
        ),
    ],
    "audio": [
        (
            f"sample-{seconds}s.mp3",
            f"https://download.samplelib.com/mp3/sample-{seconds}s.mp3",
            "Samplelib free sample",
        )
        for seconds in [3, 6, 9, 12, 15]
    ]
    + [
        (
            f"sample-{seconds}s.wav",
            f"https://download.samplelib.com/wav/sample-{seconds}s.wav",
            "Samplelib free sample",
        )
        for seconds in [3, 6]
    ]
    + [
        (
            "lena.flac",
            github("audiojs/audio-lena", "lena.flac"),
            "audio-lena public decoder fixture",
        ),
        (
            "lena.ogg",
            github("audiojs/audio-lena", "lena.ogg"),
            "audio-lena public decoder fixture",
        ),
        (
            "lena.m4a",
            github("audiojs/audio-lena", "lena.m4a"),
            "audio-lena public decoder fixture",
        ),
    ],
    "text": [
        (
            f"gutenberg-{book}.txt",
            f"https://www.gutenberg.org/cache/epub/{book}/pg{book}.txt",
            "Project Gutenberg public domain",
        )
        for book in [11, 84, 1342, 1661, 2701, 98, 74, 76, 5200, 345]
    ],
}


def validate(path: Path, family: str) -> None:
    data = path.read_bytes()
    if not data or len(data) > MAX_BYTES:
        raise ValueError(f"invalid size {len(data)}")
    suffix = path.suffix.lower()
    if family == "pdf" and not data.startswith(b"%PDF"):
        raise ValueError("not PDF")
    if family in {"docx", "pptx"} or suffix in {".xlsx", ".xlsb", ".ods"}:
        if not zipfile.is_zipfile(path):
            raise ValueError("not ZIP-based Office file")
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            required = {
                "docx": "word/document.xml",
                "pptx": "ppt/presentation.xml",
            }.get(family)
            if required and required not in names:
                raise ValueError(f"missing {required}")
            if suffix == ".xlsx" and "xl/workbook.xml" not in names:
                raise ValueError("missing XLSX workbook")
            if suffix == ".xlsb" and "xl/workbook.bin" not in names:
                raise ValueError("missing XLSB workbook")
    if suffix == ".xls" and not data.startswith(bytes.fromhex("D0CF11E0")):
        raise ValueError("not OLE XLS")
    if family == "html" and b"<html" not in data[:20000].lower() and b"<!doctype" not in data[:20000].lower():
        raise ValueError("not HTML")
    if family == "image":
        signatures = (
            b"\x89PNG",
            b"\xff\xd8\xff",
            b"II*\x00",
            b"MM\x00*",
            b"GIF8",
            b"RIFF",
        )
        if not data.startswith(signatures):
            raise ValueError("unknown image signature")
    if family == "audio":
        valid_audio = (
            data.startswith(b"ID3")
            or data.startswith(b"RIFF")
            or data.startswith(b"fLaC")
            or data.startswith(b"OggS")
            or b"ftyp" in data[:32]
            or (data[0] == 0xFF and data[1] & 0xE0 == 0xE0)
        )
        if not valid_audio:
            raise ValueError("unknown audio signature")


def download(url: str, output: Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    last_error: Exception | None = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                content = response.read(MAX_BYTES + 1)
            output.write_bytes(content)
            return
        except Exception as error:  # network retry
            last_error = error
            time.sleep(2**attempt)
    raise RuntimeError(f"download failed: {last_error}")


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    records = []
    failures = []
    for family, sources in SOURCES.items():
        if len(sources) != 10:
            raise RuntimeError(f"{family} must contain exactly 10 sources")
        directory = OUTPUT / family
        directory.mkdir(parents=True, exist_ok=True)
        for name, url, license_note in sources:
            path = directory / name
            try:
                if not path.exists():
                    download(url, path)
                validate(path, family)
                digest = hashlib.sha256(path.read_bytes()).hexdigest()
                records.append(
                    {
                        "family": family,
                        "name": name,
                        "url": url,
                        "license": license_note,
                        "bytes": path.stat().st_size,
                        "sha256": digest,
                    }
                )
                print(f"OK   {family:12} {name}")
            except Exception as error:
                path.unlink(missing_ok=True)
                failures.append(f"{family}/{name}: {error}")
                print(f"FAIL {family:12} {name}: {error}")

    (OUTPUT / "sources.lock.json").write_text(
        json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    (OUTPUT / "SHA256SUMS").write_text(
        "".join(
            f"{record['sha256']}  {record['family']}/{record['name']}\n"
            for record in records
        ),
        encoding="utf-8",
    )
    counts = {
        family: sum(record["family"] == family for record in records)
        for family in SOURCES
    }
    print(json.dumps(counts, indent=2))
    if failures:
        raise SystemExit("\n".join(failures))


if __name__ == "__main__":
    main()
