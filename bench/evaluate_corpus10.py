#!/usr/bin/env python3
"""Run release conversion on corpus10 and write a reproducible quality report."""

from __future__ import annotations

import json
import os
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "bench/corpus10"
OUTPUTS = CORPUS / "outputs"
REPORT = ROOT / "bench/REPORT_CORPUS10_QUALITY.md"
BIN = ROOT / "target/release/fileconv"


def evaluate(family: str, source: Path, markdown: str) -> list[tuple[str, str]]:
    issues: list[tuple[str, str]] = []
    chars = len(markdown)
    if "\ufffd" in markdown:
        issues.append(("error", "contains Unicode replacement characters"))
    if family == "pdf" and chars < 500:
        issues.append(("error", "unexpectedly short PDF output"))
    if family == "docx" and source.stat().st_size > 20_000 and chars == 0:
        issues.append(("warning", "non-trivial DOCX produced no text"))
    if family == "pptx" and chars == 0:
        issues.append(("info", "shape/image-only deck; inspect visual preview"))
    if family == "spreadsheet" and chars == 0:
        issues.append(("warning", "spreadsheet produced no cells"))
    if family == "csv" and chars and "|" not in markdown:
        issues.append(("error", "CSV output is not a Markdown table"))
    if family == "html":
        lowered = markdown.lower()
        if "<script" in lowered or "<style" in lowered:
            issues.append(("error", "script/style leaked into Markdown"))
        if chars > source.stat().st_size * 8:
            issues.append(("warning", "HTML output expanded more than 8×"))
    if family == "image" and chars == 0:
        issues.append(("info", "no OCR text detected"))
    if family == "audio" and chars > 500:
        issues.append(("warning", "possible hallucination on short audio sample"))
    elif family == "audio" and chars > 0:
        issues.append(("info", "non-empty transcript; codec fixture has no ground-truth speech"))
    if family == "text":
        ratio = chars / max(source.stat().st_size, 1)
        if not 0.85 <= ratio <= 1.2:
            issues.append(("warning", f"text size ratio {ratio:.2f} outside expected range"))
    return issues


def main() -> None:
    if not BIN.exists():
        raise SystemExit(f"release binary missing: {BIN}")
    OUTPUTS.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["FILECONV_WHISPER_MODEL"] = str(ROOT / "models/ggml-tiny.bin")
    rows = []
    for family_dir in sorted(path for path in CORPUS.iterdir() if path.is_dir() and path.name != "outputs"):
        family = family_dir.name
        for source in sorted(path for path in family_dir.iterdir() if path.is_file()):
            started = time.perf_counter()
            result = subprocess.run(
                [str(BIN), "one", str(source)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
                timeout=180,
            )
            elapsed_ms = (time.perf_counter() - started) * 1000
            raw_markdown = result.stdout.decode("utf-8", errors="replace")
            markdown = raw_markdown.strip()
            output = OUTPUTS / f"{family}__{source.name}.md"
            output.write_text(raw_markdown, encoding="utf-8")
            issues = evaluate(family, source, markdown) if result.returncode == 0 else [
                ("error", result.stderr.decode("utf-8", errors="replace").strip())
            ]
            rows.append(
                {
                    "family": family,
                    "file": source.name,
                    "ok": result.returncode == 0,
                    "ms": elapsed_ms,
                    "chars": len(markdown),
                    "lines": markdown.count("\n") + bool(markdown),
                    "headings": sum(line.startswith("#") for line in markdown.splitlines()),
                    "tables": sum(line.startswith("|") for line in markdown.splitlines()),
                    "issues": issues,
                }
            )
            print(f"{family:12} {source.name:42} {result.returncode} {len(markdown):7} chars")

    summary = []
    for family in sorted({row["family"] for row in rows}):
        selected = [row for row in rows if row["family"] == family]
        summary.append(
            {
                "family": family,
                "count": len(selected),
                "ok": sum(row["ok"] for row in selected),
                "nonempty": sum(row["chars"] > 0 for row in selected),
                "median_ms": statistics.median(row["ms"] for row in selected),
                "median_chars": statistics.median(row["chars"] for row in selected),
                "errors": sum(
                    severity == "error"
                    for row in selected
                    for severity, _ in row["issues"]
                ),
                "warnings": sum(
                    severity == "warning"
                    for row in selected
                    for severity, _ in row["issues"]
                ),
            }
        )

    pptx_preview = []
    for source in sorted((CORPUS / "pptx").glob("*.pptx")):
        result = subprocess.run(
            [str(BIN), "pptx-preview", str(source)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=60,
        )
        if result.returncode != 0:
            pptx_preview.append(
                {"file": source.name, "ok": False, "error": result.stderr.decode()}
            )
            continue
        payload = json.loads(result.stdout)
        shapes = [
            shape
            for slide in payload["slides"]
            for shape in slide.get("shapes", [])
        ]
        pptx_preview.append(
            {
                "file": source.name,
                "ok": True,
                "slides": payload["meta"]["slideCount"],
                "shapes": len(shapes),
                "text": sum(shape["kind"] == "text" for shape in shapes),
                "images": sum(shape["kind"] == "image" for shape in shapes),
            }
        )

    lines = [
        "# Markhand CORPUS10 — quality report",
        "",
        f"- Files: **{len(rows)}** public internet samples.",
        f"- Successful conversions: **{sum(row['ok'] for row in rows)}/{len(rows)}**.",
        "- Path: desktop-equivalent release `fileconv_core::Converter`.",
        "- Audio model: Whisper tiny; samples are decoder/music fixtures, not Vietnamese WER ground truth.",
        "",
        "## Summary",
        "",
        "| Family | N | OK | Non-empty | Median ms | Median chars | Errors | Warnings |",
        "|---|--:|--:|--:|--:|--:|--:|--:|",
    ]
    for item in summary:
        lines.append(
            f"| {item['family']} | {item['count']} | {item['ok']} | "
            f"{item['nonempty']} | {item['median_ms']:.2f} | {item['median_chars']:.0f} | "
            f"{item['errors']} | {item['warnings']} |"
        )
    lines.extend(
        [
            "",
            "## Per-file",
            "",
            "| Family | File | ms | Chars | Headings | Table rows | Assessment |",
            "|---|---|--:|--:|--:|--:|---|",
        ]
    )
    for row in rows:
        assessment = "; ".join(
            f"{severity}: {message}" for severity, message in row["issues"]
        ) or "pass"
        assessment = assessment.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| {row['family']} | `{row['file']}` | {row['ms']:.2f} | "
            f"{row['chars']} | {row['headings']} | {row['tables']} | {assessment} |"
        )
    lines.extend(
        [
            "",
            "## PPTX visual preview",
            "",
            "| File | Slides | Rendered shapes | Text shapes | Images | Result |",
            "|---|--:|--:|--:|--:|---|",
        ]
    )
    for item in pptx_preview:
        if item["ok"]:
            lines.append(
                f"| `{item['file']}` | {item['slides']} | {item['shapes']} | "
                f"{item['text']} | {item['images']} | pass |"
            )
        else:
            lines.append(
                f"| `{item['file']}` | — | — | — | — | error: {item['error']} |"
            )
    lines.extend(
        [
            "",
            "## Interpretation limits",
            "",
            "- Public format fixtures validate compatibility and structure, not Vietnamese semantic accuracy.",
            "- Image files mix OCR documents and decorative assets; empty decorative-image output is not a failure.",
            "- Audio samples are mostly music/codec fixtures; short or empty output is preferred over hallucinated speech.",
            "- BRD/PRD, citation grounding and Vietnamese OCR accuracy remain covered by their dedicated manifests.",
            "",
        ]
    )
    REPORT.write_text("\n".join(lines), encoding="utf-8")
    (CORPUS / "quality-results.json").write_text(
        json.dumps(
            {"conversions": rows, "pptxPreview": pptx_preview},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
