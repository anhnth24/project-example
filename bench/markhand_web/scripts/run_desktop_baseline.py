#!/usr/bin/env python3
"""Run release conversion and local desktop retrieval over the Phase 0 corpus."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import re
import shutil
import subprocess
import time
import unicodedata
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
DEFAULT_OUTPUT = CORPUS / "baselines/desktop-v1"


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def normalize(value: str) -> str:
    cleaned = []
    for line in unicodedata.normalize("NFC", value).splitlines():
        stripped = line.strip()
        if stripped and all(character in "|-: " for character in stripped):
            continue
        cleaned.append(line.replace("|", " "))
    return " ".join(
        token
        for token in " ".join(cleaned).split()
        if not all(character == "#" or character == "-" for character in token)
    )


def distance(left: list[str], right: list[str]) -> int:
    if len(left) > len(right):
        left, right = right, left
    previous = list(range(len(left) + 1))
    for row, right_item in enumerate(right, 1):
        current = [row]
        for column, left_item in enumerate(left, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[column] + 1,
                    previous[column - 1] + (left_item != right_item),
                )
            )
        previous = current
    return previous[-1]


def error_rates(expected: str, actual: str) -> tuple[float, float]:
    expected_normal = normalize(expected)
    actual_normal = normalize(actual)
    expected_chars = list(expected_normal)
    actual_chars = list(actual_normal)
    expected_words = expected_normal.split()
    actual_words = actual_normal.split()
    cer = distance(expected_chars, actual_chars) / max(1, len(expected_chars))
    wer = distance(expected_words, actual_words) / max(1, len(expected_words))
    return cer, wer


def hardware_fingerprint() -> dict:
    cpu_model = "unknown"
    cpu_vendor = platform.processor() or "unknown"
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        content = cpuinfo.read_text(errors="replace")
        model = re.search(r"model name\s*:\s*(.+)", content)
        vendor = re.search(r"vendor_id\s*:\s*(.+)", content)
        cpu_model = model.group(1).strip() if model else cpu_model
        cpu_vendor = vendor.group(1).strip() if vendor else cpu_vendor
    memory_kb = 0
    meminfo = Path("/proc/meminfo")
    if meminfo.is_file():
        match = re.search(r"MemTotal:\s*(\d+)", meminfo.read_text())
        memory_kb = int(match.group(1)) if match else 0
    disk = shutil.disk_usage(ROOT)
    gpu = "none"
    gpu_count = 0
    gpu_vram = 0
    nvidia = shutil.which("nvidia-smi")
    if nvidia:
        query = subprocess.run(
            [
                nvidia,
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        lines = [line for line in query.stdout.splitlines() if line.strip()]
        if lines:
            gpu_count = len(lines)
            gpu = lines[0].split(",", 1)[0].strip()
            gpu_vram = int(lines[0].split(",", 1)[1].strip()) // 1024
    return {
        "cpu": {
            "vendor": cpu_vendor,
            "model": cpu_model,
            "cores": os.cpu_count() or 1,
            "threads": os.cpu_count() or 1,
        },
        "ramGb": round(memory_kb / 1024 / 1024, 2),
        "disk": {
            "type": "overlay-or-local",
            "capacityGb": round(disk.total / 1024**3, 2),
            "iopsNote": "not measured in desktop baseline",
        },
        "gpu": {"model": gpu, "vramGb": gpu_vram, "count": gpu_count},
        "network": {"bandwidthGbps": 0, "latencyMsAssumed": 0},
        "os": {"distro": platform.platform(), "arch": platform.machine()},
    }


def file_sha256(path: Path) -> str | None:
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def native_fingerprint() -> dict:
    pdfium_candidates = [
        ROOT / "pdfium/lib/libpdfium.so",
        ROOT / "pdfium/lib/pdfium.dll",
        ROOT / "pdfium/lib/libpdfium.dylib",
    ]
    pdfium = next((path for path in pdfium_candidates if path.is_file()), None)
    tesseract = shutil.which("tesseract")
    tesseract_version = None
    languages: dict[str, str] = {}
    if tesseract:
        version = subprocess.run(
            [tesseract, "--version"],
            capture_output=True,
            text=True,
            check=False,
        )
        tesseract_version = version.stdout.splitlines()[0] if version.stdout else None
        data_roots = [
            ROOT / "tessdata_best",
            Path("/usr/share/tesseract-ocr/5/tessdata"),
        ]
        for language in ("vie", "eng"):
            model = next(
                (
                    root / f"{language}.traineddata"
                    for root in data_roots
                    if (root / f"{language}.traineddata").is_file()
                ),
                None,
            )
            if model:
                languages[language] = file_sha256(model)
    return {
        "pdfium": {
            "present": pdfium is not None,
            "sha256": file_sha256(pdfium) if pdfium else None,
        },
        "tesseract": {
            "present": tesseract is not None,
            "version": tesseract_version,
            "languageModelSha256": languages,
        },
        "whisperModelConfigured": False,
    }


def conversion_baseline(converter: Path, output: Path, manifest: dict) -> tuple[list[dict], list[dict]]:
    raw = output / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    results = []
    retrieval_documents = []
    environment = os.environ.copy()
    pdfium = ROOT / "pdfium/lib"
    if pdfium.is_dir():
        environment["FILECONV_PDFIUM_LIB"] = str(pdfium)
    tessdata = ROOT / "tessdata_best"
    if tessdata.is_dir():
        environment["FILECONV_TESSDATA"] = str(tessdata)
    for item in manifest["documents"]:
        source = CORPUS / "golden" / item["path"]
        expected = (CORPUS / "golden" / item["markdownPath"]).read_text(encoding="utf-8")
        started = time.perf_counter()
        completed = subprocess.run(
            [str(converter), "one", str(source)],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            timeout=180,
        )
        elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
        actual = completed.stdout.decode("utf-8", errors="replace") if completed.returncode == 0 else ""
        stderr = completed.stderr.decode("utf-8", errors="replace")
        stderr = stderr.replace(str(ROOT), "<repo>")
        status = "ok" if completed.returncode == 0 else "error"
        actual_path = raw / f"{item['id']}.md"
        actual_path.write_text(actual, encoding="utf-8")
        cer, wer = error_rates(expected, actual) if status == "ok" else (None, None)
        results.append(
            {
                "documentId": item["id"],
                "logicalDocumentId": item["logicalDocumentId"],
                "versionId": item["versionId"],
                "format": item["format"],
                "status": status,
                "exitCode": completed.returncode,
                "durationMs": elapsed_ms,
                "expectedBehavior": item["expectedBehavior"],
                "expectedChars": len(expected),
                "actualChars": len(actual),
                "cer": round(cer, 6) if cer is not None else None,
                "wer": round(wer, 6) if wer is not None else None,
                "actualSha256": hashlib.sha256(actual.encode()).hexdigest(),
                "error": stderr[-1000:] if status == "error" else "",
            }
        )
        if status == "ok":
            retrieval_documents.append(
                {
                    "sourceRel": item["id"],
                    "mdRel": f"{item['id']}.md",
                    "format": item["format"],
                    "markdown": actual,
                }
            )
    return results, retrieval_documents


def write_retrieval_input(
    output: Path,
    documents: list[dict],
    queries: list[dict],
    runtime_name: str = "runtime",
) -> Path:
    runtime = output / runtime_name
    if runtime.exists():
        shutil.rmtree(runtime)
    runtime.mkdir(parents=True)
    path = runtime / "retrieval-input.json"
    path.write_text(
        json.dumps(
            {
                "database": str(runtime / "knowledge.sqlite"),
                "annRoot": str(runtime),
                "documents": documents,
                "queries": [
                    {
                        "queryId": query["query_id"],
                        "text": query["query"],
                        "answerMode": query["answer_mode"],
                    }
                    for query in queries
                ],
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    return path


def discounted_gain(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(index + 2) for index, grade in enumerate(grades))


def evaluate_retrieval(queries: list[dict], output: dict) -> dict:
    expected = {row["query_id"]: row for row in queries}
    by_category: dict[str, list[dict]] = defaultdict(list)
    rows = []
    for actual in output["queries"]:
        query = expected[actual["queryId"]]
        judgments = json.loads(query["judgments"])
        expected_citations = json.loads(query["citations"])
        ranked_chunks = [hit["sourceRel"] for hit in actual["hits"]]
        ranked = list(dict.fromkeys(ranked_chunks))
        first_hit_by_document = {}
        for hit in actual["hits"]:
            first_hit_by_document.setdefault(hit["sourceRel"], hit)
        expected_citation_docs = {
            citation["documentId"] for citation in expected_citations
        }
        actual_citation_docs = set(ranked)
        citation_precision = (
            len(expected_citation_docs.intersection(actual_citation_docs))
            / len(actual_citation_docs)
            if actual_citation_docs
            else 1.0 if not expected_citation_docs else 0.0
        )
        citation_recall = (
            len(expected_citation_docs.intersection(actual_citation_docs))
            / len(expected_citation_docs)
            if expected_citation_docs
            else 1.0 if not actual_citation_docs else 0.0
        )
        matched_citations = [
            citation
            for citation in expected_citations
            if citation["documentId"] in first_hit_by_document
        ]
        page_accuracy = (
            sum(
                first_hit_by_document[citation["documentId"]]["anchor"]["page"]
                == citation["page"]
                for citation in matched_citations
            )
            / len(matched_citations)
            if matched_citations
            else 0.0
        )
        span_accuracy = (
            sum(
                first_hit_by_document[citation["documentId"]]["anchor"]["start"]
                == citation["start"]
                and first_hit_by_document[citation["documentId"]]["anchor"]["end"]
                == citation["end"]
                for citation in matched_citations
            )
            / len(matched_citations)
            if matched_citations
            else 0.0
        )
        relevant = {doc for doc, grade in judgments.items() if grade >= 2}
        recall5 = (
            len(relevant.intersection(ranked[:5])) / len(relevant)
            if relevant
            else 0.0
        )
        recall10 = (
            len(relevant.intersection(ranked[:10])) / len(relevant)
            if relevant
            else 0.0
        )
        hit5 = 1.0 if relevant and any(doc in relevant for doc in ranked[:5]) else 0.0
        reciprocal = next(
            (1 / (index + 1) for index, doc in enumerate(ranked) if doc in relevant),
            0.0,
        )
        actual_grades = [judgments.get(doc, 0) for doc in ranked[:10]]
        ideal_grades = sorted(judgments.values(), reverse=True)[:10]
        ideal = discounted_gain(ideal_grades)
        ndcg = discounted_gain(actual_grades) / ideal if ideal else 1.0
        row = {
            "queryId": query["query_id"],
            "category": query["category"],
            "versionMode": query["version_mode"],
            "expectedAnswerMode": query["answer_mode"],
            "actualAnswerMode": actual["actualAnswerMode"],
            "rankedDocuments": ranked,
            "recallAt5": recall5,
            "recallAt10": recall10,
            "hitAt5": hit5,
            "reciprocalRank": round(reciprocal, 6),
            "ndcgAt10": round(ndcg, 6),
            "topDocument": ranked[0] if ranked else None,
            "expectedDocument": query["expected_doc"] or None,
            "versionAwareCitationAvailable": False,
            "hasRelevant": bool(relevant),
            "citationDocumentPrecision": round(citation_precision, 6),
            "citationDocumentRecall": round(citation_recall, 6),
            "citationPageAccuracy": round(page_accuracy, 6),
            "citationSpanExactAccuracy": round(span_accuracy, 6),
            "answerExact": normalize(actual["answer"])
            == normalize(query["expected_answer"]),
            "returnedHitCount": len(ranked_chunks),
            "uniqueDocumentCount": len(ranked),
        }
        rows.append(row)
        by_category[query["category"]].append(row)

    def aggregate(items: list[dict]) -> dict:
        relevant_items = [item for item in items if item["hasRelevant"]]
        count = len(relevant_items)
        return {
            "queries": count,
            "recallAt5": round(sum(item["recallAt5"] for item in relevant_items) / max(1, count), 6),
            "recallAt10": round(sum(item["recallAt10"] for item in relevant_items) / max(1, count), 6),
            "hitAt5": round(sum(item["hitAt5"] for item in relevant_items) / max(1, count), 6),
            "mrr": round(sum(item["reciprocalRank"] for item in relevant_items) / max(1, count), 6),
            "ndcgAt10": round(sum(item["ndcgAt10"] for item in relevant_items) / max(1, count), 6),
        }

    ranked_rows = [
        row for row in rows if json.loads(expected[row["queryId"]]["judgments"])
    ]
    no_answer = [
        row for row in rows if not json.loads(expected[row["queryId"]]["judgments"])
    ]
    temporal = [
        row
        for row in ranked_rows
        if row["versionMode"] != "current"
        or row["category"] == "temporal_current"
    ]
    current_temporal = [
        row for row in temporal if row["category"] == "temporal_current"
    ]
    return {
        "summary": aggregate(ranked_rows),
        "citationSummary": {
            "documentPrecision": round(
                sum(row["citationDocumentPrecision"] for row in rows)
                / max(1, len(rows)),
                6,
            ),
            "documentRecall": round(
                sum(row["citationDocumentRecall"] for row in rows)
                / max(1, len(rows)),
                6,
            ),
            "pageAccuracy": round(
                sum(row["citationPageAccuracy"] for row in ranked_rows)
                / max(1, len(ranked_rows)),
                6,
            ),
            "spanExactAccuracy": round(
                sum(row["citationSpanExactAccuracy"] for row in ranked_rows)
                / max(1, len(ranked_rows)),
                6,
            ),
            "answerExactAccuracy": round(
                sum(row["answerExact"] for row in rows) / max(1, len(rows)),
                6,
            ),
        },
        "noAnswerSummary": {
            "queries": len(no_answer),
            "accuracy": round(
                sum(row["returnedHitCount"] == 0 for row in no_answer)
                / max(1, len(no_answer)),
                6,
            ),
        },
        "temporalSummary": {
            **aggregate(temporal),
            "currentVersionTop1Accuracy": round(
                sum(row["topDocument"] == row["expectedDocument"] for row in current_temporal)
                / max(1, len(current_temporal)),
                6,
            ),
            "versionCitationPrecision": 0.0,
            "versionCitationRecall": 0.0,
            "note": "Desktop baseline has no version-aware citation payload yet.",
        },
        "categories": {
            category: aggregate(items) for category, items in sorted(by_category.items())
        },
        "queries": rows,
    }


def markdown_report(
    commit: str,
    hardware: dict,
    conversions: list[dict],
    retrieval: dict,
) -> str:
    formats: dict[str, list[dict]] = defaultdict(list)
    for result in conversions:
        formats[result["format"]].append(result)
    lines = [
        "# P0-03 desktop baseline",
        "",
        f"- Git commit: `{commit}`",
        f"- CPU threads visible: {hardware['cpu']['threads']}",
        f"- RAM: {hardware['ramGb']} GB",
        f"- GPU: {hardware['gpu']['model']} × {hardware['gpu']['count']}",
        "- Environment role: reduced smoke, not approved Profile B target.",
        "",
        "## Conversion",
        "",
        "| Format | Files | Success | Mean CER | Mean WER | Mean ms |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for format_name, items in sorted(formats.items()):
        successful = [item for item in items if item["status"] == "ok"]
        mean_cer = (
            f"{sum(item['cer'] for item in successful) / len(successful):.4f}"
            if successful
            else "n/a"
        )
        mean_wer = (
            f"{sum(item['wer'] for item in successful) / len(successful):.4f}"
            if successful
            else "n/a"
        )
        lines.append(
            "| {} | {} | {} | {} | {} | {:.2f} |".format(
                format_name,
                len(items),
                len(successful),
                mean_cer,
                mean_wer,
                sum(item["durationMs"] for item in items) / len(items),
            )
        )
    audio_items = formats.get("audio", [])
    if audio_items and not any(item["status"] == "ok" for item in audio_items):
        lines.extend(
            [
                "",
                "Audio rows failed dependency admission because no Whisper model was configured;",
                "their durations are failure latency, not transcription performance.",
            ]
        )
    summary = retrieval["summary"]
    temporal = retrieval["temporalSummary"]
    lines.extend(
        [
            "",
            "## Local desktop retrieval",
            "",
            f"- Ranked queries: {summary['queries']}",
            f"- No-answer queries: {retrieval['noAnswerSummary']['queries']}",
            f"- Recall@5: {summary['recallAt5']:.4f}",
            f"- Recall@10: {summary['recallAt10']:.4f}",
            f"- Hit@5: {summary['hitAt5']:.4f}",
            f"- MRR: {summary['mrr']:.4f}",
            f"- nDCG@10: {summary['ndcgAt10']:.4f}",
            f"- Temporal Recall@5: {temporal['recallAt5']:.4f}",
            f"- Current-version Top-1 accuracy: {temporal['currentVersionTop1Accuracy']:.4f}",
            f"- No-answer accuracy: {retrieval['noAnswerSummary']['accuracy']:.4f}",
            f"- Citation document precision: {retrieval['citationSummary']['documentPrecision']:.4f}",
            f"- Citation document recall: {retrieval['citationSummary']['documentRecall']:.4f}",
            f"- Citation exact-span accuracy: {retrieval['citationSummary']['spanExactAccuracy']:.4f}",
            "- Version-citation precision/recall: 0.0 baseline (payload not implemented).",
            "",
            "## Interpretation",
            "",
            "This report freezes current behavior. It does not claim P0 retrieval, temporal,",
            "capacity, or target-hardware gates pass. Version-aware gold intentionally exposes",
            "the desktop baseline gap before P0-06/P1B implementation.",
            "",
        ]
    )
    return "\n".join(lines)


def run(output: Path, converter: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    commit = git("rev-parse", "HEAD")
    dirty = bool(git("status", "--porcelain", "--untracked-files=no"))
    if dirty:
        raise RuntimeError("baseline requires a clean git worktree")
    manifest = json.loads((CORPUS / "golden/manifest.json").read_text())
    with (CORPUS / "golden/queries.tsv").open(encoding="utf-8", newline="") as source:
        queries = list(csv.DictReader(source, delimiter="\t"))
    conversions, documents = conversion_baseline(converter, output, manifest)
    retrieval_input = write_retrieval_input(output, documents, queries)
    retrieval_raw = output / "retrieval-raw.json"
    subprocess.run(
        [
            "cargo",
            "run",
            "--release",
            "-p",
            "fileconv-knowledge",
            "--all-features",
            "--example",
            "p0_desktop_baseline",
            "--",
            str(retrieval_input),
            str(retrieval_raw),
        ],
        cwd=ROOT,
        check=True,
    )
    raw = json.loads(retrieval_raw.read_text())
    rerun_input = write_retrieval_input(
        output, documents, queries, runtime_name="runtime-rerun"
    )
    rerun_raw = output / "retrieval-rerun.json"
    subprocess.run(
        [
            "cargo",
            "run",
            "--release",
            "-p",
            "fileconv-knowledge",
            "--all-features",
            "--example",
            "p0_desktop_baseline",
            "--",
            str(rerun_input),
            str(rerun_raw),
        ],
        cwd=ROOT,
        check=True,
    )
    rerun = json.loads(rerun_raw.read_text())
    retrieval_deterministic = raw == rerun
    retrieval = evaluate_retrieval(queries, raw)
    ranking_fingerprint = hashlib.sha256(
        json.dumps(
            [
                [row["queryId"], row["rankedDocuments"]]
                for row in retrieval["queries"]
            ],
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    raw_fingerprint = hashlib.sha256(
        json.dumps(
            raw,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    hardware = hardware_fingerprint()
    (output / "conversion-results.json").write_text(
        json.dumps({"version": 1, "documents": conversions}, ensure_ascii=False, indent=2) + "\n"
    )
    (output / "retrieval-results.json").write_text(
        json.dumps({"version": 1, **retrieval}, ensure_ascii=False, indent=2) + "\n"
    )
    stale_input = output / "retrieval-input.json"
    if stale_input.exists():
        stale_input.unlink()
    metadata = {
        "version": 1,
        "gitCommit": commit,
        "gitDirty": dirty,
        "workloadProfileId": "on-prem-reference-v1",
        "environmentRole": "reduced-smoke",
        "hardware": hardware,
        "command": "make p0-desktop-baseline",
        "composeFileSha256": file_sha256(ROOT / "deploy/dev/compose.yml"),
        "serviceVersions": {
            "fileconvCore": "0.1.0",
            "fileconvKnowledge": "0.1.0",
        },
        "imageDigests": {"notUsed": "local-process-baseline"},
        "nativeRuntime": native_fingerprint(),
        "fixtureManifestSha256": hashlib.sha256(
            (CORPUS / "manifest.lock.json").read_bytes()
        ).hexdigest(),
        "converterSha256": hashlib.sha256(converter.read_bytes()).hexdigest(),
        "retrievalRankingSha256": ranking_fingerprint,
        "rawRetrievalSha256": raw_fingerprint,
        "retrievalDeterministic": retrieval_deterministic,
    }
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    report = markdown_report(commit, hardware, conversions, retrieval)
    (output / "desktop-baseline.md").write_text(report, encoding="utf-8")
    (CORPUS / "reports/desktop-baseline.md").write_text(report, encoding="utf-8")
    shutil.rmtree(output / "runtime", ignore_errors=True)
    shutil.rmtree(output / "runtime-rerun", ignore_errors=True)
    rerun_raw.unlink(missing_ok=True)
    print(f"wrote desktop baseline to {output}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--converter",
        type=Path,
        default=ROOT / "target/release/fileconv",
    )
    args = parser.parse_args()
    converter = args.converter.resolve()
    if not converter.is_file():
        raise RuntimeError(f"release converter missing: {converter}")
    run(args.output.resolve(), converter)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
