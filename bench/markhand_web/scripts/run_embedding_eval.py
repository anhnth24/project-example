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


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def load_models() -> dict:
    return json.loads(MODELS_PATH.read_text(encoding="utf-8"))


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


def load_chunks() -> list[dict]:
    chunks: list[dict] = []
    for path in sorted(MD_DIR.glob("*.md")):
        for index, chunk in enumerate(chunk_markdown(path.read_text(encoding="utf-8"))):
            payload = chunk["text"]
            if chunk["heading"]:
                payload = f"# {chunk['heading']}\n\n{payload}"
            chunks.append(
                {
                    "docId": path.stem,
                    "chunkId": f"{path.stem}#{index}",
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


def maybe_segment(texts: list[str], enabled: bool) -> list[str]:
    if not enabled:
        return texts
    from pyvi.ViTokenizer import tokenize

    return [tokenize(text) for text in texts]


def l2_normalize(matrix):
    import numpy as np

    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-12)
    return matrix / norms


def embed_texts(model, texts: list[str], batch_size: int, word_segment: bool):
    import numpy as np

    prepared = maybe_segment(texts, word_segment)
    started = time.perf_counter()
    vectors = model.encode(
        prepared,
        batch_size=batch_size,
        show_progress_bar=False,
        convert_to_numpy=True,
        normalize_embeddings=False,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    matrix = l2_normalize(np.asarray(vectors, dtype=np.float32))
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
    return {"summary": summary, "byCategory": category_summary, "rows": rows}


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
        revision=model_cfg.get("revision", "main"),
        device=device,
    )
    if model_cfg.get("maxSeqLength"):
        model.max_seq_length = int(model_cfg["maxSeqLength"])
    load_ms = (time.perf_counter() - load_started) * 1000
    return model, load_ms


def run_model(
    model_cfg: dict,
    chunks: list[dict],
    queries: list[dict],
    run_index: int,
    model=None,
    load_ms: float = 0.0,
    revision: str | None = None,
) -> dict:
    device = resolve_device(model_cfg.get("device", "auto"))
    revision = revision or resolve_revision(model_cfg["hubId"], model_cfg.get("revision", "main"))
    print(
        f"== {model_cfg['id']} run={run_index} device={device} revision={revision[:12]} =="
    )
    if model is None:
        model, load_ms = load_sentence_transformer(model_cfg, device)

    chunk_texts = [chunk["text"] for chunk in chunks]
    query_texts = [query["query"] for query in queries]
    chunk_vecs, chunk_ms = embed_texts(
        model,
        chunk_texts,
        int(model_cfg.get("batchSize", 16)),
        bool(model_cfg.get("wordSegment")),
    )
    query_vecs, query_ms = embed_texts(
        model,
        query_texts,
        int(model_cfg.get("batchSize", 16)),
        bool(model_cfg.get("wordSegment")),
    )
    if chunk_vecs.shape[1] != int(model_cfg["dimensions"]):
        raise RuntimeError(
            f"{model_cfg['id']} returned dim={chunk_vecs.shape[1]}, "
            f"expected {model_cfg['dimensions']}"
        )

    evaluation = evaluate_rankings(queries, chunks, chunk_vecs, query_vecs)
    total_vectors = len(chunks) + len(queries)
    embed_wall_s = (chunk_ms + query_ms) / 1000
    capacity = {
        "device": device,
        "loadMs": round(load_ms, 2),
        "embedChunksMs": round(chunk_ms, 2),
        "embedQueriesMs": round(query_ms, 2),
        "vectorsPerSecond": round(total_vectors / max(embed_wall_s, 1e-6), 3),
        "vramGb": None,
        "note": "quality-track; VRAM not measured on CPU smoke",
    }
    gates = {
        "G0-RET-RECALL-AT-5": {
            "metric": evaluation["summary"]["recallAt5"],
            "threshold": 0.85,
            "pass": evaluation["summary"]["recallAt5"] >= 0.85,
        }
    }
    result = {
        "modelId": model_cfg["id"],
        "role": model_cfg["role"],
        "family": model_cfg["family"],
        "hubId": model_cfg["hubId"],
        "revisionRequested": model_cfg.get("revision", "main"),
        "revisionResolved": revision,
        "dimensions": int(model_cfg["dimensions"]),
        "maxSeqLength": int(model_cfg.get("maxSeqLength") or 0),
        "wordSegment": bool(model_cfg.get("wordSegment")),
        "normalize": "l2",
        "chunkingVersion": "heading-chunks-2000-v1",
        "runIndex": run_index,
        "summary": evaluation["summary"],
        "byCategory": evaluation["byCategory"],
        "capacity": capacity,
        "gates": gates,
        "rows": evaluation["rows"],
    }
    print(
        f"  recall@5={result['summary']['recallAt5']:.4f} "
        f"ndcg@10={result['summary']['ndcgAt10']:.4f} "
        f"mrr={result['summary']['mrr']:.4f} "
        f"pass={result['gates']['G0-RET-RECALL-AT-5']['pass']}"
    )
    return result


def aggregate_runs(runs: list[dict]) -> dict:
    summaries = [run["summary"] for run in runs]
    def mean_key(key: str) -> float:
        return round(statistics.fmean(summary[key] for summary in summaries), 6)

    aggregate = {
        "runs": len(runs),
        "recallAt5": mean_key("recallAt5"),
        "recallAt10": mean_key("recallAt10"),
        "hitAt5": mean_key("hitAt5"),
        "mrr": mean_key("mrr"),
        "ndcgAt10": mean_key("ndcgAt10"),
        "recallAt5Stdev": round(
            statistics.pstdev(summary["recallAt5"] for summary in summaries), 6
        )
        if len(summaries) > 1
        else 0.0,
    }
    aggregate["gates"] = {
        "G0-RET-RECALL-AT-5": {
            "metric": aggregate["recallAt5"],
            "threshold": 0.85,
            "pass": aggregate["recallAt5"] >= 0.85,
        }
    }
    return aggregate


def write_report(summary: dict, path: Path) -> None:
    lines = [
        "# P0-05 embedding evaluation (quality track)",
        "",
        f"- Generated: `{summary['generatedAt']}`",
        f"- Git commit: `{summary['git']['commit']}`",
        f"- Environment role: `{summary['hardware']['role']}`",
        f"- Device: `{summary['device']}`",
        f"- Chunking: `{summary['chunkingVersion']}`",
        f"- Runs per model: `{summary['runsPerModel']}`",
        f"- Fixture manifest: `{summary['fixtureManifestSha256'][:16]}…`",
        "",
        "## Quality vs gates",
        "",
        "| Model | Family | Dims | Recall@5 | Hit@5 | MRR | nDCG@10 | Recall gate | Gap to best nDCG |",
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
        "## Category breakdown (best candidate mean)",
        "",
        "| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    best = next(model for model in summary["models"] if model["role"] == "best-candidate")
    # use last run categories for detail
    last_run = best["runDetails"][-1]
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
        "## Verdict",
        "",
        f"- Quality gate satisfied by at least one model: "
        f"**{'YES' if summary['verdict']['anyRecallGatePass'] else 'NO'}**",
        f"- Selected draft (quality-only): `{summary['verdict']['selectedDraft']}`",
        f"- P0-05 fully closed: **{'YES' if summary['verdict']['p0_05_closed'] else 'NO'}**",
        "",
    ]
    for reason in summary["verdict"]["reasons"]:
        lines.append(f"- {reason}")
    lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=3, help="runs per model (>=1)")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument(
        "--models",
        nargs="*",
        default=None,
        help="optional model ids from models.yaml",
    )
    args = parser.parse_args()
    if args.runs < 1:
        raise SystemExit("--runs must be >= 1")

    catalog = load_models()
    selected = catalog["models"]
    if args.models:
        wanted = set(args.models)
        selected = [model for model in catalog["models"] if model["id"] in wanted]
        missing = wanted.difference(model["id"] for model in selected)
        if missing:
            raise SystemExit(f"unknown model ids: {sorted(missing)}")

    chunks = load_chunks()
    queries = load_queries()
    print(f"loaded chunks={len(chunks)} queries={len(queries)}")
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
        device = resolve_device(model_cfg.get("device", "auto"))
        revision = resolve_revision(model_cfg["hubId"], model_cfg.get("revision", "main"))
        model, load_ms = load_sentence_transformer(model_cfg, device)
        for run_index in range(1, args.runs + 1):
            result = run_model(
                model_cfg,
                chunks,
                queries,
                run_index,
                model=model,
                load_ms=load_ms,
                revision=revision,
            )
            run_path = model_dir / f"run-{run_index}.json"
            # keep rows in machine-readable output
            run_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            slim = {key: value for key, value in result.items() if key != "rows"}
            runs.append(slim)
        del model
        aggregate = aggregate_runs(runs)
        model_summaries.append(
            {
                "modelId": model_cfg["id"],
                "role": model_cfg["role"],
                "family": model_cfg["family"],
                "hubId": model_cfg["hubId"],
                "revisionResolved": runs[-1]["revisionResolved"],
                "dimensions": model_cfg["dimensions"],
                "wordSegment": model_cfg.get("wordSegment", False),
                "aggregate": aggregate,
                "gates": dict(aggregate["gates"]),
                "runDetails": runs,
            }
        )

    best_ndcg = max(model["aggregate"]["ndcgAt10"] for model in model_summaries)
    for model in model_summaries:
        gap = round(best_ndcg - model["aggregate"]["ndcgAt10"], 6)
        model["gates"]["G0-RET-BEST-MODEL-GAP"] = {
            "metric": gap,
            "threshold": 0.02,
            "pass": gap <= 0.02,
            "bestNdcgAt10": best_ndcg,
        }

    passing = [model for model in model_summaries if model["gates"]["G0-RET-RECALL-AT-5"]["pass"]]
    if passing:
        selected_draft = min(
            passing,
            key=lambda model: (
                0 if model["role"] == "best-candidate" else 1,
                -model["aggregate"]["ndcgAt10"],
            ),
        )["hubId"]
    else:
        selected_draft = max(
            model_summaries, key=lambda model: model["aggregate"]["ndcgAt10"]
        )["hubId"]

    immutable = {
        "chunkingVersion": catalog["chunkingVersion"],
        "normalize": catalog["normalize"],
        "ranking": catalog["ranking"],
        "models": [
            {
                "id": model["modelId"],
                "hubId": model["hubId"],
                "revision": model["revisionResolved"],
                "dimensions": model["dimensions"],
                "wordSegment": model["wordSegment"],
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
        "runsPerModel": args.runs,
        "fixtureManifestSha256": fixture_manifest_sha256,
        "chunkCount": len(chunks),
        "queryCount": len(queries),
        "models": model_summaries,
        "immutableConfig": immutable,
        "verdict": {
            "anyRecallGatePass": bool(passing),
            "selectedDraft": selected_draft,
            "p0_05_closed": False,
            "reasons": [
                "Quality track executed on reduced-smoke hardware (CPU unless CUDA present).",
                "Capacity evidence (VRAM, saturation, queue depth, target GPU fingerprint) still required.",
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
