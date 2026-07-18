#!/usr/bin/env python3
"""P0-05 Vietnamese embedding quality evaluation on the Phase 0 golden corpus."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import re
import statistics
import subprocess
import time
import unicodedata
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MODELS_PATH = CORPUS / "embedding/models.yaml"
DEFAULT_OUTPUT = CORPUS / "embedding/results"
REPORT_PATH = CORPUS / "reports/embedding-evaluation.md"
MD_DIR = CORPUS / "golden/markdown"
QUERIES_PATH = CORPUS / "golden/queries.tsv"
MANIFEST_LOCK = CORPUS / "manifest.lock.json"
MAX_CHARS = 2000
# Match crates/knowledge/src/desktop/sqlite.rs embedding input construction.
PAYLOAD_FORMAT = "{heading}\\n{text}"
RECALL_GATE = 0.85
GAP_GATE = 0.02


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def load_models() -> dict:
    try:
        import yaml
    except ImportError as error:  # pragma: no cover
        raise SystemExit(
            "PyYAML is required to load embedding/models.yaml; "
            "install bench/markhand_web/requirements-embedding.txt"
        ) from error
    data = yaml.safe_load(MODELS_PATH.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise SystemExit(f"invalid models catalog: {MODELS_PATH}")
    return data


def chunk_markdown(md: str, max_chars: int = MAX_CHARS) -> list[dict]:
    """Mirror crates/core/src/chunk.rs heading-chunks-2000-v1."""
    max_chars = max(max_chars, 200)
    sections: list[tuple[list[str], str]] = []
    path: list[tuple[int, str]] = []
    body = ""

    def flush() -> None:
        nonlocal body
        if body.strip():
            sections.append(([title for _, title in path], body))
        body = ""

    for line in md.splitlines():
        trimmed = line.lstrip()
        hashes = 0
        for character in trimmed:
            if character == "#":
                hashes += 1
            else:
                break
        if 1 <= hashes <= 6 and len(trimmed) > hashes and trimmed[hashes] == " ":
            flush()
            title = trimmed[hashes + 1 :].strip()
            while path and path[-1][0] >= hashes:
                path.pop()
            path.append((hashes, title))
        else:
            body += line + "\n"
    flush()

    chunks: list[dict] = []
    for heading_path, text in sections:
        heading = " > ".join(heading_path)
        text = text.strip()
        if not text:
            continue
        if len(text) <= max_chars:
            chunks.append({"heading": heading, "text": text})
            continue
        current = ""
        for para in text.split("\n\n"):
            if current and len(current) + len(para) + 2 > max_chars:
                chunks.append({"heading": heading, "text": current.strip()})
                current = ""
            if len(para) > max_chars:
                for index in range(0, len(para), max_chars):
                    piece = para[index : index + max_chars].strip()
                    if piece:
                        chunks.append({"heading": heading, "text": piece})
            else:
                current = f"{current}\n\n{para}" if current else para
        if current.strip():
            chunks.append({"heading": heading, "text": current.strip()})
    return chunks


def embedding_payload(heading: str, text: str) -> str:
    """Desktop convention: `{heading}\\n{text}` (sqlite.rs)."""
    if heading:
        return f"{heading}\n{text}"
    return text


def load_chunks() -> list[dict]:
    chunks: list[dict] = []
    for path in sorted(MD_DIR.glob("*.md")):
        for index, chunk in enumerate(chunk_markdown(path.read_text(encoding="utf-8"))):
            payload = embedding_payload(chunk["heading"], chunk["text"])
            chunks.append(
                {
                    "docId": path.stem,
                    "chunkId": f"{path.stem}#{index}",
                    "heading": chunk["heading"],
                    "body": chunk["text"],
                    "text": unicodedata.normalize("NFC", payload),
                    "chars": len(payload),
                }
            )
    return chunks


def load_queries() -> list[dict]:
    with QUERIES_PATH.open(encoding="utf-8") as handle:
        return list(csv.DictReader(handle, delimiter="\t"))


def discounted_gain(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(index + 2) for index, grade in enumerate(grades))


def ranking_fingerprint(rows: list[dict]) -> str:
    digest = hashlib.sha256()
    for row in sorted(rows, key=lambda item: item["queryId"]):
        digest.update(row["queryId"].encode())
        digest.update(b"\0")
        digest.update(",".join(row["rankedDocuments"]).encode())
        digest.update(b"\0")
    return digest.hexdigest()


def hardware_fingerprint() -> dict:
    cpu_model = platform.processor() or "unknown"
    cpu_vendor = "unknown"
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
    gpu_name = "none"
    gpu_vram_gb = 0
    gpu_count = 0
    try:
        query = subprocess.run(
            [
                "nvidia-smi",
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
            gpu_name = lines[0].split(",", 1)[0].strip()
            gpu_vram_gb = int(float(lines[0].split(",", 1)[1].strip()) // 1024)
    except FileNotFoundError:
        pass
    return {
        "cpu": {
            "vendor": cpu_vendor,
            "model": cpu_model,
            "cores": os.cpu_count() or 1,
            "threads": os.cpu_count() or 1,
        },
        "ramGb": round(memory_kb / 1024 / 1024, 2) if memory_kb else None,
        "gpu": {"model": gpu_name, "vramGb": gpu_vram_gb, "count": gpu_count},
        "os": {"distro": platform.platform(), "arch": platform.machine()},
        "role": "reduced-smoke-cpu" if gpu_count == 0 else "gpu-host",
    }


def resolve_device(requested: str) -> str:
    if requested != "auto":
        return requested
    try:
        import torch

        if torch.cuda.is_available():
            return "cuda"
        if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
            return "mps"
    except Exception:
        pass
    return "cpu"


def validate_model_cfg(model_cfg: dict) -> None:
    required = [
        "id",
        "hubId",
        "revision",
        "dimensions",
        "maxSeqLength",
        "batchSize",
        "device",
        "wordSegment",
    ]
    missing = [key for key in required if key not in model_cfg]
    if missing:
        raise SystemExit(f"{model_cfg.get('id', '?')}: missing config keys {missing}")
    if not isinstance(model_cfg["revision"], str) or model_cfg["revision"] in {
        "",
        "main",
        "master",
    }:
        raise SystemExit(
            f"{model_cfg['id']}: revision must be an immutable commit SHA, "
            f"got {model_cfg['revision']!r}"
        )
    if model_cfg["wordSegment"]:
        segmenter = model_cfg.get("wordSegmenter")
        if segmenter != "pyvi":
            raise SystemExit(
                f"{model_cfg['id']}: wordSegment=true requires wordSegmenter='pyvi', "
                f"got {segmenter!r}"
            )


def prepare_texts(texts: list[str], model_cfg: dict) -> list[str]:
    if not model_cfg["wordSegment"]:
        return texts
    try:
        from pyvi.ViTokenizer import tokenize
    except Exception as error:  # pragma: no cover - fail closed
        raise SystemExit(
            f"{model_cfg['id']}: required wordSegmenter pyvi failed to import: {error}"
        ) from error
    return [tokenize(text) for text in texts]


def l2_normalize(matrix):
    import numpy as np

    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-12)
    return matrix / norms


def embed_texts(model, texts: list[str], model_cfg: dict):
    prepared = prepare_texts(texts, model_cfg)
    started = time.perf_counter()
    vectors = model.encode(
        prepared,
        batch_size=int(model_cfg["batchSize"]),
        show_progress_bar=False,
        convert_to_numpy=True,
        normalize_embeddings=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    matrix = l2_normalize(vectors)
    return matrix, elapsed_ms


def evaluate_rankings(queries: list[dict], chunks: list[dict], chunk_vecs, query_vecs) -> dict:
    import numpy as np

    rows = []
    by_category: dict[str, list[dict]] = defaultdict(list)
    doc_ids = sorted({chunk["docId"] for chunk in chunks})
    doc_to_indices: dict[str, list[int]] = defaultdict(list)
    for index, chunk in enumerate(chunks):
        doc_to_indices[chunk["docId"]].append(index)

    for query, query_vec in zip(queries, query_vecs):
        judgments = json.loads(query["judgments"] or "{}")
        scores = {}
        for doc_id in doc_ids:
            indices = doc_to_indices[doc_id]
            chunk_scores = chunk_vecs[indices] @ query_vec
            scores[doc_id] = float(np.max(chunk_scores))
        ranked = [doc for doc, _ in sorted(scores.items(), key=lambda item: item[1], reverse=True)]
        relevant = {doc for doc, grade in judgments.items() if grade >= 2}
        has_relevant = bool(relevant)
        if has_relevant:
            recall5 = len(relevant.intersection(ranked[:5])) / len(relevant)
            recall10 = len(relevant.intersection(ranked[:10])) / len(relevant)
            hit5 = 1.0 if any(doc in relevant for doc in ranked[:5]) else 0.0
            reciprocal = next(
                (1 / (index + 1) for index, doc in enumerate(ranked) if doc in relevant),
                0.0,
            )
            actual_grades = [judgments.get(doc, 0) for doc in ranked[:10]]
            ideal_grades = sorted(judgments.values(), reverse=True)[:10]
            ideal = discounted_gain(ideal_grades)
            ndcg = discounted_gain(actual_grades) / ideal if ideal else 1.0
        else:
            recall5 = recall10 = hit5 = reciprocal = ndcg = 0.0
        row = {
            "queryId": query["query_id"],
            "category": query["category"],
            "hasRelevant": has_relevant,
            "recallAt5": recall5,
            "recallAt10": recall10,
            "hitAt5": hit5,
            "reciprocalRank": round(reciprocal, 6),
            "ndcgAt10": round(ndcg, 6),
            "topDocument": ranked[0] if ranked else None,
            "expectedDocument": query["expected_doc"] or None,
            "rankedDocuments": ranked[:10],
        }
        rows.append(row)
        by_category[query["category"]].append(row)

    ranked_rows = [row for row in rows if row["hasRelevant"]]
    count = max(1, len(ranked_rows))
    summary = {
        "queries": len(ranked_rows),
        "noAnswerQueries": sum(1 for row in rows if not row["hasRelevant"]),
        "recallAt5": round(sum(row["recallAt5"] for row in ranked_rows) / count, 6),
        "recallAt10": round(sum(row["recallAt10"] for row in ranked_rows) / count, 6),
        "hitAt5": round(sum(row["hitAt5"] for row in ranked_rows) / count, 6),
        "mrr": round(sum(row["reciprocalRank"] for row in ranked_rows) / count, 6),
        "ndcgAt10": round(sum(row["ndcgAt10"] for row in ranked_rows) / count, 6),
    }
    category_summary = {}
    for category, items in sorted(by_category.items()):
        relevant_items = [item for item in items if item["hasRelevant"]]
        if not relevant_items:
            category_summary[category] = {"queries": 0}
            continue
        size = len(relevant_items)
        category_summary[category] = {
            "queries": size,
            "recallAt5": round(sum(item["recallAt5"] for item in relevant_items) / size, 6),
            "hitAt5": round(sum(item["hitAt5"] for item in relevant_items) / size, 6),
            "mrr": round(sum(item["reciprocalRank"] for item in relevant_items) / size, 6),
            "ndcgAt10": round(sum(item["ndcgAt10"] for item in relevant_items) / size, 6),
        }
    return {
        "summary": summary,
        "byCategory": category_summary,
        "rows": rows,
        "rankingSha256": ranking_fingerprint(rows),
    }


def resolve_revision(hub_id: str, revision: str) -> str:
    try:
        from huggingface_hub import model_info

        info = model_info(hub_id, revision=revision)
        return getattr(info, "sha", None) or revision
    except Exception:
        return revision


def load_sentence_transformer(model_cfg: dict, device: str):
    from sentence_transformers import SentenceTransformer

    load_started = time.perf_counter()
    model = SentenceTransformer(
        model_cfg["hubId"],
        revision=model_cfg["revision"],
        device=device,
    )
    model.max_seq_length = int(model_cfg["maxSeqLength"])
    load_ms = (time.perf_counter() - load_started) * 1000
    return model, load_ms


def run_model(
    model_cfg: dict,
    chunks: list[dict],
    queries: list[dict],
    run_index: int,
) -> dict:
    """Independent run: load model fresh, embed, score."""
    device = resolve_device(model_cfg.get("device", "auto"))
    revision_requested = model_cfg["revision"]
    revision_resolved = resolve_revision(model_cfg["hubId"], revision_requested)
    if revision_resolved != revision_requested and not revision_requested.startswith(
        revision_resolved[:12]
    ):
        # Allow short prefix pins only if they match resolved SHA.
        if not revision_resolved.startswith(revision_requested):
            raise SystemExit(
                f"{model_cfg['id']}: pinned revision {revision_requested} "
                f"resolved to different SHA {revision_resolved}"
            )
    print(
        f"== {model_cfg['id']} run={run_index} device={device} "
        f"revision={revision_resolved[:12]} =="
    )
    model, load_ms = load_sentence_transformer(model_cfg, device)
    try:
        chunk_vecs, chunk_ms = embed_texts(
            model, [chunk["text"] for chunk in chunks], model_cfg
        )
        query_vecs, query_ms = embed_texts(
            model, [query["query"] for query in queries], model_cfg
        )
    finally:
        del model

    if chunk_vecs.shape[1] != int(model_cfg["dimensions"]):
        raise RuntimeError(
            f"{model_cfg['id']} returned dim={chunk_vecs.shape[1]}, "
            f"expected {model_cfg['dimensions']}"
        )

    evaluation = evaluate_rankings(queries, chunks, chunk_vecs, query_vecs)
    total_vectors = len(chunks) + len(queries)
    embed_wall_s = (chunk_ms + query_ms) / 1000
    recall_pass = evaluation["summary"]["recallAt5"] >= RECALL_GATE
    result = {
        "modelId": model_cfg["id"],
        "role": model_cfg["role"],
        "family": model_cfg["family"],
        "hubId": model_cfg["hubId"],
        "revisionRequested": revision_requested,
        "revisionResolved": revision_resolved,
        "dimensions": int(model_cfg["dimensions"]),
        "maxSeqLength": int(model_cfg["maxSeqLength"]),
        "batchSize": int(model_cfg["batchSize"]),
        "device": device,
        "wordSegment": bool(model_cfg["wordSegment"]),
        "wordSegmenter": model_cfg.get("wordSegmenter"),
        "normalize": "l2",
        "payloadFormat": PAYLOAD_FORMAT,
        "chunkingVersion": "heading-chunks-2000-v1",
        "runIndex": run_index,
        "summary": evaluation["summary"],
        "byCategory": evaluation["byCategory"],
        "rankingSha256": evaluation["rankingSha256"],
        "capacity": {
            "device": device,
            "loadMs": round(load_ms, 2),
            "embedChunksMs": round(chunk_ms, 2),
            "embedQueriesMs": round(query_ms, 2),
            "vectorsPerSecond": round(total_vectors / max(embed_wall_s, 1e-6), 3),
            "vramGb": None,
            "note": "quality-track; VRAM not measured on CPU smoke",
        },
        "gates": {
            "G0-RET-RECALL-AT-5": {
                "metric": evaluation["summary"]["recallAt5"],
                "threshold": RECALL_GATE,
                "statistic": "per-run",
                "pass": recall_pass,
            }
        },
        "rows": evaluation["rows"],
    }
    print(
        f"  recall@5={result['summary']['recallAt5']:.4f} "
        f"ndcg@10={result['summary']['ndcgAt10']:.4f} "
        f"mrr={result['summary']['mrr']:.4f} "
        f"pass={recall_pass} ranking={result['rankingSha256'][:12]}"
    )
    return result


def aggregate_runs(runs: list[dict]) -> dict:
    """Apply registry statistics: recall uses min; report mean/stdev for diagnostics."""
    recalls = [run["summary"]["recallAt5"] for run in runs]
    ndcgs = [run["summary"]["ndcgAt10"] for run in runs]
    recall_min = min(recalls)
    ndcg_min = min(ndcgs)
    aggregate = {
        "runs": len(runs),
        "recallAt5": round(recall_min, 6),
        "recallAt5Mean": round(statistics.fmean(recalls), 6),
        "recallAt10": round(min(run["summary"]["recallAt10"] for run in runs), 6),
        "hitAt5": round(min(run["summary"]["hitAt5"] for run in runs), 6),
        "mrr": round(min(run["summary"]["mrr"] for run in runs), 6),
        "ndcgAt10": round(ndcg_min, 6),
        "ndcgAt10Mean": round(statistics.fmean(ndcgs), 6),
        "recallAt5Stdev": round(statistics.pstdev(recalls), 6) if len(recalls) > 1 else 0.0,
        "rankingSha256Set": sorted({run["rankingSha256"] for run in runs}),
        "independentLoads": True,
    }
    aggregate["gates"] = {
        "G0-RET-RECALL-AT-5": {
            "metric": aggregate["recallAt5"],
            "threshold": RECALL_GATE,
            "statistic": "min",
            "pass": aggregate["recallAt5"] >= RECALL_GATE,
        }
    }
    return aggregate


def write_report(summary: dict, path: Path) -> None:
    selected = summary["verdict"]["selectedDraft"]
    selected_model = next(
        (
            model
            for model in summary["models"]
            if model["hubId"] == selected
        ),
        summary["models"][0],
    )
    lines = [
        "# P0-05 embedding evaluation (quality track)",
        "",
        f"- Generated: `{summary['generatedAt']}`",
        f"- Git commit: `{summary['git']['commit']}`",
        f"- Dirty worktree: `{summary['git']['dirty']}`",
        f"- Environment role: `{summary['hardware']['role']}`",
        f"- Device: `{summary['device']}`",
        f"- Chunking: `{summary['chunkingVersion']}`",
        f"- Payload format: `{summary['payloadFormat']}`",
        f"- Runs per model: `{summary['runsPerModel']}` (independent loads)",
        f"- Gate stats: Recall@5=`min`, best-model nDCG gap=`max`",
        f"- Fixture manifest: `{summary['fixtureManifestSha256'][:16]}…`",
        "",
        "## Quality vs gates",
        "",
        "| Model | Family | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall gate | Gap to best nDCG |",
        "|---|---|---:|---:|---:|---:|---:|---|---|",
    ]
    for model in summary["models"]:
        gap = model["gates"]["G0-RET-BEST-MODEL-GAP"]
        recall_gate = model["gates"]["G0-RET-RECALL-AT-5"]
        lines.append(
            f"| `{model['hubId']}` | {model['family']} | {model['dimensions']} | "
            f"{model['aggregate']['recallAt5']:.4f} | {model['aggregate']['hitAt5']:.4f} | "
            f"{model['aggregate']['mrr']:.4f} | {model['aggregate']['ndcgAt10']:.4f} | "
            f"{'PASS' if recall_gate['pass'] else 'FAIL'} | "
            f"{'PASS' if gap['pass'] else 'FAIL'} ({gap['metric']:.4f}) |"
        )
    lines += [
        "",
        "## Capacity note",
        "",
        "- This track is CPU/GPU-auto quality only.",
        "- VRAM/saturation/queue-depth evidence remains blocked on target NVIDIA GPU.",
        "",
        "## Category breakdown (selected draft, last run)",
        "",
        "| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    last_run = selected_model["runDetails"][-1]
    for category, stats in last_run["byCategory"].items():
        if not stats.get("queries"):
            continue
        lines.append(
            f"| {category} | {stats['queries']} | {stats['recallAt5']:.4f} | "
            f"{stats['hitAt5']:.4f} | {stats['mrr']:.4f} | {stats['ndcgAt10']:.4f} |"
        )
    lines += [
        "",
        "## Immutable config snapshot",
        "",
        "```json",
        json.dumps(summary["immutableConfig"], ensure_ascii=False, indent=2),
        "```",
        "",
        "## Ranking fingerprints",
        "",
    ]
    for model in summary["models"]:
        lines.append(
            f"- `{model['hubId']}`: {', '.join(model['aggregate']['rankingSha256Set'])}"
        )
    lines += [
        "",
        "## Verdict",
        "",
        f"- Both quality gates satisfied by selected draft: "
        f"**{'YES' if summary['verdict']['selectedPassesBothGates'] else 'NO'}**",
        f"- Selected draft (quality-only): `{summary['verdict']['selectedDraft']}`",
        f"- P0-05 fully closed: **{'YES' if summary['verdict']['p0_05_closed'] else 'NO'}**",
        "",
    ]
    for reason in summary["verdict"]["reasons"]:
        lines.append(f"- {reason}")
    lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")


def self_test() -> int:
    """Parity checks for chunking + desktop embedding payload."""
    errors: list[str] = []
    basic = chunk_markdown(
        "# Chương I\n\nMở đầu.\n\n## Điều 1\n\nNội dung điều 1.\n\n## Điều 2\n\nNội dung điều 2.\n",
        1000,
    )
    if len(basic) != 3:
        errors.append(f"expected 3 chunks, got {len(basic)}")
    elif basic[1]["heading"] != "Chương I > Điều 1":
        errors.append(f"bad heading path: {basic[1]['heading']}")
    payload = embedding_payload(basic[1]["heading"], basic[1]["text"])
    expected = f"{basic[1]['heading']}\n{basic[1]['text']}"
    if payload != expected:
        errors.append("payload format mismatch vs desktop `{heading}\\n{text}`")
    if payload.startswith("# "):
        errors.append("payload must not prefix markdown heading markers")

    para = "x" * 150
    long_md = f"# A\n\n{para}\n\n{para}\n\n{para}\n"
    long_chunks = chunk_markdown(long_md, 320)
    if len(long_chunks) < 2:
        errors.append("long section must split at paragraph boundaries")
    if any(len(chunk["text"]) > 320 for chunk in long_chunks):
        errors.append("split chunks exceeded max_chars")

    pop = chunk_markdown("# A\n\nbody a\n\n## B\n\nbody b\n\n# C\n\nbody c\n", 1000)
    if pop[2]["heading"] != "C":
        errors.append("heading level pop failed")

    # Fail-closed segmenter validation
    try:
        validate_model_cfg(
            {
                "id": "bad",
                "hubId": "x",
                "revision": "main",
                "dimensions": 1,
                "maxSeqLength": 1,
                "batchSize": 1,
                "device": "cpu",
                "wordSegment": True,
            }
        )
        errors.append("validate_model_cfg accepted mutable revision/main + missing segmenter")
    except SystemExit:
        pass

    if errors:
        for error in errors:
            print(f"self-test FAIL: {error}", file=os.sys.stderr)
        return 1
    print("self-test OK: chunking, payload, and config validation")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=3, help="independent runs per model")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--models", nargs="*", default=None)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.runs < 1:
        raise SystemExit("--runs must be >= 1")

    catalog = load_models()
    if catalog.get("payloadFormat") not in (None, PAYLOAD_FORMAT, "{heading}\\n{text}"):
        raise SystemExit(f"unsupported catalog payloadFormat: {catalog.get('payloadFormat')}")
    selected = catalog["models"]
    if args.models:
        wanted = set(args.models)
        selected = [model for model in catalog["models"] if model["id"] in wanted]
        missing = wanted.difference(model["id"] for model in selected)
        if missing:
            raise SystemExit(f"unknown model ids: {sorted(missing)}")
    for model_cfg in selected:
        validate_model_cfg(model_cfg)

    chunks = load_chunks()
    queries = load_queries()
    print(f"loaded chunks={len(chunks)} queries={len(queries)} payload={PAYLOAD_FORMAT}")
    args.output.mkdir(parents=True, exist_ok=True)

    hardware = hardware_fingerprint()
    device = resolve_device("auto")
    fixture_sha = (
        MANIFEST_LOCK.read_bytes()
        if MANIFEST_LOCK.is_file()
        else QUERIES_PATH.read_bytes()
    )
    fixture_manifest_sha256 = hashlib.sha256(fixture_sha).hexdigest()
    commit = git("rev-parse", "HEAD")
    dirty = bool(git("status", "--porcelain"))

    model_summaries = []
    for model_cfg in selected:
        model_dir = args.output / model_cfg["id"]
        model_dir.mkdir(parents=True, exist_ok=True)
        runs = []
        for run_index in range(1, args.runs + 1):
            result = run_model(model_cfg, chunks, queries, run_index)
            run_path = model_dir / f"run-{run_index}.json"
            # Keep auditable per-query rankings in committed evidence.
            run_path.write_text(
                json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            runs.append(result)
        aggregate = aggregate_runs(runs)
        # Slim in-memory summary copy for summary.json (rows remain on disk).
        slim_runs = [{key: value for key, value in run.items() if key != "rows"} for run in runs]
        model_summaries.append(
            {
                "modelId": model_cfg["id"],
                "role": model_cfg["role"],
                "family": model_cfg["family"],
                "hubId": model_cfg["hubId"],
                "revisionRequested": model_cfg["revision"],
                "revisionResolved": runs[-1]["revisionResolved"],
                "dimensions": model_cfg["dimensions"],
                "maxSeqLength": model_cfg["maxSeqLength"],
                "batchSize": model_cfg["batchSize"],
                "deviceResolved": runs[-1]["device"],
                "wordSegment": model_cfg["wordSegment"],
                "wordSegmenter": model_cfg.get("wordSegmenter"),
                "aggregate": aggregate,
                "gates": dict(aggregate["gates"]),
                "runDetails": slim_runs,
            }
        )

    # Gap gate uses registry statistic=max: worst per-run gap to the best
    # model in that same run (not gap-of-mins across aggregates).
    run_count = len(model_summaries[0]["runDetails"]) if model_summaries else 0
    for model in model_summaries:
        per_run_gaps: list[float] = []
        per_run_best: list[float] = []
        for run_index in range(run_count):
            best_ndcg = max(
                other["runDetails"][run_index]["summary"]["ndcgAt10"]
                for other in model_summaries
            )
            this_ndcg = model["runDetails"][run_index]["summary"]["ndcgAt10"]
            per_run_gaps.append(best_ndcg - this_ndcg)
            per_run_best.append(best_ndcg)
        gap = round(max(per_run_gaps) if per_run_gaps else 0.0, 6)
        model["gates"]["G0-RET-BEST-MODEL-GAP"] = {
            "metric": gap,
            "threshold": GAP_GATE,
            "statistic": "max",
            "pass": gap <= GAP_GATE,
            "bestNdcgAt10": round(max(per_run_best) if per_run_best else 0.0, 6),
            "perRunGaps": [round(value, 6) for value in per_run_gaps],
        }

    eligible = [
        model
        for model in model_summaries
        if model["gates"]["G0-RET-RECALL-AT-5"]["pass"]
        and model["gates"]["G0-RET-BEST-MODEL-GAP"]["pass"]
    ]
    if eligible:
        selected_draft = max(
            eligible, key=lambda model: model["aggregate"]["ndcgAt10"]
        )["hubId"]
        selected_passes = True
    else:
        selected_draft = max(
            model_summaries, key=lambda model: model["aggregate"]["ndcgAt10"]
        )["hubId"]
        selected_passes = False

    immutable = {
        "chunkingVersion": catalog["chunkingVersion"],
        "normalize": catalog["normalize"],
        "ranking": catalog["ranking"],
        "payloadFormat": PAYLOAD_FORMAT,
        "gateStatistics": {
            "G0-RET-RECALL-AT-5": "min",
            "G0-RET-BEST-MODEL-GAP": "max",
        },
        "models": [
            {
                "id": model["modelId"],
                "hubId": model["hubId"],
                "revision": model["revisionResolved"],
                "revisionRequested": model["revisionRequested"],
                "dimensions": model["dimensions"],
                "maxSeqLength": model["maxSeqLength"],
                "batchSize": model["batchSize"],
                "device": model["deviceResolved"],
                "wordSegment": model["wordSegment"],
                "wordSegmenter": model["wordSegmenter"],
                "normalize": "l2",
            }
            for model in model_summaries
        ],
    }
    summary = {
        "version": 1,
        "issue": "P0-05",
        "track": "quality-cpu-smoke",
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "git": {"commit": commit, "dirty": dirty},
        "hardware": hardware,
        "device": device,
        "chunkingVersion": catalog["chunkingVersion"],
        "payloadFormat": PAYLOAD_FORMAT,
        "runsPerModel": args.runs,
        "fixtureManifestSha256": fixture_manifest_sha256,
        "chunkCount": len(chunks),
        "queryCount": len(queries),
        "models": model_summaries,
        "immutableConfig": immutable,
        "verdict": {
            "anyRecallGatePass": any(
                model["gates"]["G0-RET-RECALL-AT-5"]["pass"] for model in model_summaries
            ),
            "selectedPassesBothGates": selected_passes,
            "selectedDraft": selected_draft,
            "p0_05_closed": False,
            "reasons": [
                "Quality track executed with independent model loads per run.",
                "Gate statistics follow registry: Recall@5=min, best-model nDCG gap=max.",
                "Selection requires both Recall@5 and best-model-gap gates.",
                "Per-query rankings retained in run-*.json with rankingSha256 fingerprints.",
                "Capacity evidence (VRAM, saturation, queue depth, target GPU) still required.",
                "ADR remains Proposed until capacity + approver sign-off.",
                "Restricted corpus must not leave to cloud providers; local/self-host only.",
            ],
        },
    }
    summary_path = args.output / "summary.json"
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")
    write_report(summary, args.report)
    print(f"wrote {summary_path}")
    print(f"wrote {args.report}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
