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
OPENAI_PIN_RE = re.compile(r"^openai-alias-observed-\d{4}-\d{2}-\d{2}$")
MD_DIR = CORPUS / "golden/markdown"
QUERIES_PATH = CORPUS / "golden/queries.tsv"
MANIFEST_LOCK = CORPUS / "manifest.lock.json"
MAX_CHARS = 2000
# Match crates/knowledge/src/desktop/sqlite.rs embedding input construction.
PAYLOAD_FORMAT = "{heading}\\n{text}"
SHA1_FULL = re.compile(r"^[0-9a-f]{40}$")
MIN_GATING_RUNS = 3
MIN_GATING_FAMILIES = 2


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def git_status() -> dict:
    """Capture commit + dirty paths at evaluation start (before writing outputs)."""
    commit = git("rev-parse", "HEAD")
    # Do not .strip() the whole porcelain blob — a leading " M file" would lose
    # the first path character. Parse line-by-line instead.
    raw = subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=ROOT, text=True
    )
    dirty_paths = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {
        "commit": commit,
        "dirty": bool(dirty_paths),
        "dirtyPaths": dirty_paths,
    }


def load_models(path: Path | None = None) -> dict:
    catalog_path = path or MODELS_PATH
    try:
        import yaml
    except ImportError as error:  # pragma: no cover
        raise SystemExit(
            "PyYAML is required to load embedding/models.yaml; "
            "install bench/markhand_web/requirements-embedding.txt"
        ) from error
    data = yaml.safe_load(catalog_path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise SystemExit(f"invalid models catalog: {catalog_path}")
    return data


def load_gate_config(catalog: dict) -> dict:
    """Catalog gates are authoritative; fail if missing or mismatched registry IDs."""
    gates = catalog.get("gates")
    if not isinstance(gates, dict):
        raise SystemExit("models.yaml missing gates block")
    recall = gates.get("recallAt5") or {}
    gap = gates.get("bestModelNdcgGap") or {}
    if recall.get("gateId") != "G0-RET-RECALL-AT-5":
        raise SystemExit("models.yaml recallAt5.gateId must be G0-RET-RECALL-AT-5")
    if gap.get("gateId") != "G0-RET-BEST-MODEL-GAP":
        raise SystemExit("models.yaml bestModelNdcgGap.gateId must be G0-RET-BEST-MODEL-GAP")
    if recall.get("operator") != ">=" or gap.get("operator") != "<=":
        raise SystemExit("models.yaml gate operators must be Recall>= and Gap<=")
    if recall.get("statistic") != "min" or gap.get("statistic") != "max":
        raise SystemExit("models.yaml gate statistics must be Recall=min and Gap=max")
    try:
        recall_threshold = float(recall["value"])
        gap_threshold = float(gap["value"])
    except (KeyError, TypeError, ValueError) as error:
        raise SystemExit(f"models.yaml gate thresholds invalid: {error}") from error
    if catalog.get("chunkingVersion") != "heading-chunks-2000-v1":
        raise SystemExit(
            f"unsupported chunkingVersion: {catalog.get('chunkingVersion')!r}"
        )
    if catalog.get("normalize") != "l2":
        raise SystemExit(f"unsupported normalize: {catalog.get('normalize')!r}")
    if catalog.get("ranking") != "max-pool-chunk-cosine -> document":
        raise SystemExit(f"unsupported ranking: {catalog.get('ranking')!r}")
    if catalog.get("payloadFormat") not in (None, PAYLOAD_FORMAT, "{heading}\\n{text}"):
        raise SystemExit(f"unsupported catalog payloadFormat: {catalog.get('payloadFormat')}")
    return {
        "recallThreshold": recall_threshold,
        "gapThreshold": gap_threshold,
        "recallStatistic": "min",
        "gapStatistic": "max",
        "chunkingVersion": catalog["chunkingVersion"],
        "normalize": catalog["normalize"],
        "ranking": catalog["ranking"],
        "payloadFormat": PAYLOAD_FORMAT,
    }


def verify_fixture_manifest() -> dict:
    """Validate golden markdown + queries against manifest.lock.json hashes."""
    if not MANIFEST_LOCK.is_file():
        raise SystemExit(f"missing fixture lock: {MANIFEST_LOCK}")
    lock = json.loads(MANIFEST_LOCK.read_text(encoding="utf-8"))
    by_path = {entry["path"]: entry["sha256"] for entry in lock.get("files", [])}
    required = [
        path
        for path in by_path
        if path == "golden/queries.tsv" or path.startswith("golden/markdown/")
    ]
    if "golden/queries.tsv" not in by_path:
        raise SystemExit("manifest.lock.json missing golden/queries.tsv")
    mismatches = []
    checked = []
    for rel in sorted(required):
        path = CORPUS / rel
        if not path.is_file():
            mismatches.append(f"missing {rel}")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != by_path[rel]:
            mismatches.append(f"{rel}: got {digest}, expected {by_path[rel]}")
        else:
            checked.append(rel)
    on_disk_md = sorted(
        str(path.relative_to(CORPUS)).replace("\\", "/")
        for path in MD_DIR.glob("*.md")
    )
    locked_md = sorted(path for path in required if path.startswith("golden/markdown/"))
    if on_disk_md != locked_md:
        mismatches.append(
            f"markdown set drift: disk={len(on_disk_md)} lock={len(locked_md)}"
        )
    if mismatches:
        raise SystemExit(
            "fixture manifest validation failed:\n- " + "\n- ".join(mismatches)
        )
    return {
        "manifestLockSha256": hashlib.sha256(MANIFEST_LOCK.read_bytes()).hexdigest(),
        "checkedFiles": len(checked),
        "queriesPath": "golden/queries.tsv",
        "markdownFiles": len(locked_md),
    }


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
    """Desktop convention: always `format!("{{}}\\n{{}}", heading, text)` (sqlite.rs)."""
    return f"{heading}\n{text}"


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


def model_provider(model_cfg: dict, catalog: dict | None = None) -> str:
    return (
        model_cfg.get("provider")
        or (catalog or {}).get("provider")
        or "sentence-transformers"
    )


def validate_model_cfg(model_cfg: dict, catalog: dict | None = None) -> None:
    provider = model_provider(model_cfg, catalog)
    if provider == "openai-compatible":
        required = [
            "id",
            "model",
            "revision",
            "dimensions",
            "maxSeqLength",
            "batchSize",
            "wordSegment",
        ]
        missing = [key for key in required if key not in model_cfg]
        if missing:
            raise SystemExit(f"{model_cfg.get('id', '?')}: missing config keys {missing}")
        revision = model_cfg["revision"]
        if not isinstance(revision, str) or not OPENAI_PIN_RE.match(revision):
            raise SystemExit(
                f"{model_cfg['id']}: openai observation pin must look like "
                f"openai-alias-observed-YYYY-MM-DD, got {revision!r}"
            )
        if model_cfg["wordSegment"]:
            raise SystemExit(f"{model_cfg['id']}: openai models must set wordSegment=false")
        return

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
    revision = model_cfg["revision"]
    if not isinstance(revision, str) or not SHA1_FULL.match(revision):
        raise SystemExit(
            f"{model_cfg['id']}: revision must be a full 40-hex commit SHA, "
            f"got {revision!r}"
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


def openai_client(catalog: dict) -> dict:
    import urllib.request

    api_key_env = catalog.get("apiKeyEnv") or "FILECONV_EMBEDDING_API_KEY"
    api_key = os.environ.get(api_key_env, "").strip()
    if not api_key:
        raise SystemExit(f"missing API key env {api_key_env}")
    base_url = (catalog.get("baseUrl") or "https://api.openai.com").rstrip("/")
    return {"api_key": api_key, "base_url": base_url, "request": urllib.request}


def embed_texts_openai(texts: list[str], model_cfg: dict, catalog: dict):
    import json as json_lib
    import numpy as np
    import urllib.error

    client = openai_client(catalog)
    url = f"{client['base_url']}/v1/embeddings"
    batch_size = max(1, int(model_cfg["batchSize"]))
    dims_request = model_cfg.get("dimensionsRequest")
    vectors: list[list[float]] = []
    started = time.perf_counter()
    for offset in range(0, len(texts), batch_size):
        batch = texts[offset : offset + batch_size]
        payload: dict = {
            "model": model_cfg["model"],
            "input": batch,
            "encoding_format": "float",
        }
        if dims_request is not None:
            payload["dimensions"] = int(dims_request)
        body = json_lib.dumps(payload).encode("utf-8")
        request = client["request"].Request(
            url,
            data=body,
            method="POST",
            headers={
                "Authorization": f"Bearer {client['api_key']}",
                "Content-Type": "application/json",
            },
        )
        try:
            with client["request"].urlopen(request, timeout=120) as response:
                data = json_lib.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")[:400]
            raise SystemExit(
                f"{model_cfg['id']}: OpenAI embeddings HTTP {error.code}: {detail}"
            ) from error
        ordered = sorted(data["data"], key=lambda item: item["index"])
        vectors.extend(item["embedding"] for item in ordered)
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
    gate_config: dict,
    catalog: dict,
) -> dict:
    """Independent run: load model fresh, embed, score."""
    provider = model_provider(model_cfg, catalog)
    revision_requested = model_cfg["revision"]
    observed_at = datetime.now(timezone.utc).isoformat()
    if provider == "openai-compatible":
        device = "openai-api"
        # OpenAI model ids are mutable aliases; keep observation metadata only.
        revision_resolved = revision_requested
        hub_id = model_cfg["model"]
        print(
            f"== {model_cfg['id']} run={run_index} provider=openai "
            f"model={hub_id} observedPin={revision_resolved} =="
        )
        load_ms = 0.0
        chunk_vecs, chunk_ms = embed_texts_openai(
            [chunk["text"] for chunk in chunks], model_cfg, catalog
        )
        query_vecs, query_ms = embed_texts_openai(
            [query["query"] for query in queries], model_cfg, catalog
        )
        capacity_note = "openai-compatible API; VRAM N/A"
        model_mutability = "mutable-alias"
    else:
        device = resolve_device(model_cfg.get("device", "auto"))
        revision_resolved = resolve_revision(model_cfg["hubId"], revision_requested)
        if revision_resolved != revision_requested:
            raise SystemExit(
                f"{model_cfg['id']}: pinned revision {revision_requested} "
                f"resolved to different SHA {revision_resolved}"
            )
        hub_id = model_cfg["hubId"]
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
        capacity_note = "quality-track; VRAM not measured on CPU smoke"
        model_mutability = "immutable-sha"

    if chunk_vecs.shape[1] != int(model_cfg["dimensions"]):
        raise RuntimeError(
            f"{model_cfg['id']} returned dim={chunk_vecs.shape[1]}, "
            f"expected {model_cfg['dimensions']}"
        )

    evaluation = evaluate_rankings(queries, chunks, chunk_vecs, query_vecs)
    total_vectors = len(chunks) + len(queries)
    embed_wall_s = (chunk_ms + query_ms) / 1000
    recall_threshold = gate_config["recallThreshold"]
    recall_metric = evaluation["summary"]["recallAt5"]
    result = {
        "modelId": model_cfg["id"],
        "role": model_cfg["role"],
        "family": model_cfg["family"],
        "provider": provider,
        "hubId": hub_id,
        "revisionRequested": revision_requested,
        "revisionResolved": revision_resolved,
        "modelMutability": model_mutability,
        "observedAt": observed_at,
        "dimensions": int(model_cfg["dimensions"]),
        "dimensionsRequest": model_cfg.get("dimensionsRequest"),
        "maxSeqLength": int(model_cfg["maxSeqLength"]),
        "batchSize": int(model_cfg["batchSize"]),
        "device": device,
        "wordSegment": bool(model_cfg["wordSegment"]),
        "wordSegmenter": model_cfg.get("wordSegmenter"),
        "normalize": gate_config["normalize"],
        "payloadFormat": gate_config["payloadFormat"],
        "chunkingVersion": gate_config["chunkingVersion"],
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
            "note": capacity_note,
        },
        "thresholdObservations": {
            "G0-RET-RECALL-AT-5": {
                "metric": recall_metric,
                "threshold": recall_threshold,
                "statistic": "per-run",
                "meetsThreshold": recall_metric >= recall_threshold,
            }
        },
        "rows": evaluation["rows"],
    }
    print(
        f"  recall@5={result['summary']['recallAt5']:.4f} "
        f"ndcg@10={result['summary']['ndcgAt10']:.4f} "
        f"mrr={result['summary']['mrr']:.4f} "
        f"meetsThreshold={recall_metric >= recall_threshold} "
        f"ranking={result['rankingSha256'][:12]}"
    )
    return result


def threshold_observation(metric: float, threshold: float, statistic: str, *, formal: bool) -> dict:
    observation = {
        "metric": metric,
        "threshold": threshold,
        "statistic": statistic,
        "meetsThreshold": metric >= threshold
        if statistic != "max"
        else metric <= threshold,
    }
    if formal:
        observation["pass"] = observation["meetsThreshold"]
    else:
        observation["pass"] = None
        observation["note"] = "non-gating: threshold observation only"
    return observation


def aggregate_runs(runs: list[dict], gate_config: dict, *, formal_gates: bool) -> dict:
    """Apply registry statistics: recall uses min; report mean/stdev for diagnostics."""
    recalls = [run["summary"]["recallAt5"] for run in runs]
    ndcgs = [run["summary"]["ndcgAt10"] for run in runs]
    recall_min = min(recalls)
    ndcg_min = min(ndcgs)
    recall_threshold = gate_config["recallThreshold"]
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
        "G0-RET-RECALL-AT-5": threshold_observation(
            aggregate["recallAt5"],
            recall_threshold,
            gate_config["recallStatistic"],
            formal=formal_gates,
        )
    }
    return aggregate


def write_report(summary: dict, path: Path) -> None:
    gating = bool(summary["verdict"].get("gatingProtocol"))
    reject_track = summary.get("track") == "openai-cloud-reject"
    title = (
        "# P0-05 OpenAI embedding rejection (non-gating)"
        if reject_track
        else "# P0-05 embedding evaluation (quality track)"
    )
    focus_id = summary["verdict"].get("selectedDraft") or summary["verdict"].get(
        "bestObservedModel"
    )
    focus_model = next(
        (model for model in summary["models"] if model["hubId"] == focus_id),
        summary["models"][0],
    )
    lines = [
        title,
        "",
        f"- Generated: `{summary['generatedAt']}`",
        f"- Track: `{summary.get('track')}`",
        f"- Git commit: `{summary['git']['commit']}`",
        f"- Dirty worktree: `{summary['git']['dirty']}`",
        f"- Dirty paths: `{', '.join(summary['git'].get('dirtyPaths') or []) or '(none)'}`",
        f"- Gating protocol: `{'YES' if gating else 'NO'}`",
        f"- Environment role: `{summary['hardware']['role']}`",
        f"- Device: `{summary['device']}`",
        f"- Chunking: `{summary['chunkingVersion']}`",
        f"- Payload format: `{summary['payloadFormat']}`",
        f"- Runs per model: `{summary['runsPerModel']}` (independent loads)",
        f"- Gate stats: Recall@5=`min`, best-model nDCG gap=`max`",
        f"- Fixture manifest: `{summary['fixtureManifestSha256'][:16]}…`",
        f"- Fixture files checked: `{summary.get('fixtureValidation', {}).get('checkedFiles', '?')}`",
        "",
        "## Quality vs thresholds",
        "",
        "| Model | Family | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall≥0.85 | Gap≤0.02 |",
        "|---|---|---:|---:|---:|---:|---:|---|---|",
    ]
    for model in summary["models"]:
        gap = model["gates"]["G0-RET-BEST-MODEL-GAP"]
        recall_gate = model["gates"]["G0-RET-RECALL-AT-5"]
        recall_cell = (
            ("PASS" if recall_gate.get("pass") else "FAIL")
            if gating and recall_gate.get("pass") is not None
            else ("yes" if recall_gate.get("meetsThreshold") else "no")
        )
        gap_cell = (
            ("PASS" if gap.get("pass") else "FAIL")
            if gating and gap.get("pass") is not None
            else ("yes" if gap.get("meetsThreshold") else "no")
        )
        lines.append(
            f"| `{model['hubId']}` | {model['family']} | {model['dimensions']} | "
            f"{model['aggregate']['recallAt5']:.4f} | {model['aggregate']['hitAt5']:.4f} | "
            f"{model['aggregate']['mrr']:.4f} | {model['aggregate']['ndcgAt10']:.4f} | "
            f"{recall_cell} | {gap_cell} ({gap['metric']:.4f}) |"
        )
    lines += ["", "## Capacity note", ""]
    if reject_track:
        lines += [
            "- Cloud OpenAI `/v1/embeddings` reject track; local GPU capacity N/A.",
            "- OpenAI model ids are mutable aliases; pins are observation dates only.",
            "",
            "## Category breakdown (best observed model, last run)",
            "",
        ]
    else:
        lines += [
            "- This track is CPU/GPU-auto quality only.",
            "- VRAM/saturation/queue-depth evidence remains blocked on target NVIDIA GPU.",
            "",
            "## Category breakdown (selected draft, last run)"
            if gating
            else "## Category breakdown (best observed model, last run)",
            "",
        ]
    lines += [
        "| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    last_run = focus_model["runDetails"][-1]
    for category, stats in last_run["byCategory"].items():
        if not stats.get("queries"):
            continue
        lines.append(
            f"| {category} | {stats['queries']} | {stats['recallAt5']:.4f} | "
            f"{stats['hitAt5']:.4f} | {stats['mrr']:.4f} | {stats['ndcgAt10']:.4f} |"
        )
    config_heading = (
        "## Config snapshot (OpenAI aliases are mutable)"
        if reject_track
        else "## Immutable config snapshot"
    )
    lines += [
        "",
        config_heading,
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
    lines += ["", "## Verdict", ""]
    if gating:
        lines += [
            f"- Gating protocol (≥{MIN_GATING_FAMILIES} families / ≥{MIN_GATING_RUNS} runs): **YES**",
            f"- Both quality gates satisfied by selected draft: "
            f"**{'YES' if summary['verdict']['selectedPassesBothGates'] else 'NO'}**",
            f"- Selected draft (quality-only): `{summary['verdict']['selectedDraft']}`",
        ]
    else:
        lines += [
            "- Gating protocol: **NO** (threshold observations only; no formal PASS/FAIL).",
            f"- Best observed model (non-draft): `{summary['verdict'].get('bestObservedModel')}`",
            "- Selected draft: `null`",
        ]
    lines += [
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
    empty_heading = embedding_payload("", "body only")
    if empty_heading != "\nbody only":
        errors.append(
            "empty heading must still emit leading newline like desktop format!"
        )

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

    # Fail-closed segmenter + full SHA validation
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
    try:
        validate_model_cfg(
            {
                "id": "short",
                "hubId": "x",
                "revision": "dea33aa1ab33",
                "dimensions": 1,
                "maxSeqLength": 1,
                "batchSize": 1,
                "device": "cpu",
                "wordSegment": False,
            }
        )
        errors.append("validate_model_cfg accepted short revision prefix")
    except SystemExit:
        pass

    catalog = load_models()
    try:
        load_gate_config(catalog)
    except SystemExit as error:
        errors.append(f"catalog gate config invalid: {error}")
    try:
        verify_fixture_manifest()
    except SystemExit as error:
        errors.append(f"fixture validation failed in self-test: {error}")

    if errors:
        for error in errors:
            print(f"self-test FAIL: {error}", file=os.sys.stderr)
        return 1
    print("self-test OK: chunking, payload, catalog gates, fixtures")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--runs",
        type=int,
        default=None,
        help="independent runs per model (default 3 local / 1 openai-reject)",
    )
    parser.add_argument("--catalog", type=Path, default=MODELS_PATH)
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument("--models", nargs="*", default=None)
    parser.add_argument(
        "--allow-nongating",
        action="store_true",
        help="allow <2 families or <3 runs; marks verdict as non-gating",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    catalog_path = args.catalog.resolve()
    catalog = load_models(catalog_path)
    track = catalog.get("track") or "quality-cpu-smoke"
    reject_track = track == "openai-cloud-reject"
    if args.runs is None:
        args.runs = 1 if reject_track else 3
    if args.output is None:
        args.output = (
            CORPUS / "embedding/results/openai-rejected"
            if reject_track
            else DEFAULT_OUTPUT
        )
    if args.report is None:
        args.report = (
            CORPUS / "reports/openai-embedding-rejection.md"
            if reject_track
            else REPORT_PATH
        )
    if reject_track:
        args.allow_nongating = True

    gate_config = load_gate_config(catalog)
    selected = catalog["models"]
    if args.models:
        wanted = set(args.models)
        selected = [model for model in catalog["models"] if model["id"] in wanted]
        missing = wanted.difference(model["id"] for model in selected)
        if missing:
            raise SystemExit(f"unknown model ids: {sorted(missing)}")
    for model_cfg in selected:
        validate_model_cfg(model_cfg, catalog)

    families = {model.get("family") for model in selected}
    gating_protocol = (
        (not reject_track)
        and args.runs >= MIN_GATING_RUNS
        and len(families) >= MIN_GATING_FAMILIES
    )
    if not gating_protocol and not args.allow_nongating:
        raise SystemExit(
            f"gating protocol requires >= {MIN_GATING_RUNS} runs and "
            f">= {MIN_GATING_FAMILIES} model families (got runs={args.runs}, "
            f"families={sorted(families)}). Pass --allow-nongating for smoke."
        )
    if args.runs < 1:
        raise SystemExit("--runs must be >= 1")

    fixture_validation = verify_fixture_manifest()
    git_meta = git_status()
    if git_meta["dirty"]:
        print(
            "WARNING: dirty worktree at eval start; "
            f"paths={git_meta['dirtyPaths']}",
            file=os.sys.stderr,
        )

    chunks = load_chunks()
    queries = load_queries()
    print(
        f"loaded chunks={len(chunks)} queries={len(queries)} "
        f"payload={gate_config['payloadFormat']} gating={gating_protocol} "
        f"track={track}"
    )
    args.output.mkdir(parents=True, exist_ok=True)

    hardware = hardware_fingerprint()
    device = (
        "openai-api"
        if any(model_provider(model, catalog) == "openai-compatible" for model in selected)
        else resolve_device("auto")
    )

    model_summaries = []
    for model_cfg in selected:
        model_dir = args.output / model_cfg["id"]
        model_dir.mkdir(parents=True, exist_ok=True)
        runs = []
        for run_index in range(1, args.runs + 1):
            result = run_model(
                model_cfg, chunks, queries, run_index, gate_config, catalog
            )
            run_path = model_dir / f"run-{run_index}.json"
            # Keep auditable per-query rankings in committed evidence.
            run_path.write_text(
                json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            runs.append(result)
        aggregate = aggregate_runs(runs, gate_config, formal_gates=gating_protocol)
        # Slim in-memory summary copy for summary.json (rows remain on disk).
        slim_runs = [{key: value for key, value in run.items() if key != "rows"} for run in runs]
        model_summaries.append(
            {
                "modelId": model_cfg["id"],
                "role": model_cfg["role"],
                "family": model_cfg["family"],
                "provider": model_provider(model_cfg, catalog),
                "hubId": runs[-1]["hubId"],
                "revisionRequested": model_cfg["revision"],
                "revisionResolved": runs[-1]["revisionResolved"],
                "modelMutability": runs[-1]["modelMutability"],
                "observedAt": runs[-1]["observedAt"],
                "dimensions": model_cfg["dimensions"],
                "dimensionsRequest": model_cfg.get("dimensionsRequest"),
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
    gap_threshold = gate_config["gapThreshold"]
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
        gap_obs = threshold_observation(
            gap,
            gap_threshold,
            gate_config["gapStatistic"],
            formal=gating_protocol,
        )
        gap_obs["bestNdcgAt10"] = round(max(per_run_best) if per_run_best else 0.0, 6)
        gap_obs["perRunGaps"] = [round(value, 6) for value in per_run_gaps]
        model["gates"]["G0-RET-BEST-MODEL-GAP"] = gap_obs

    best_observed = max(
        model_summaries, key=lambda model: model["aggregate"]["ndcgAt10"]
    )["hubId"]
    # Single-family / non-gating runs must not claim formal selection.
    if gating_protocol:
        eligible = [
            model
            for model in model_summaries
            if model["gates"]["G0-RET-RECALL-AT-5"].get("pass")
            and model["gates"]["G0-RET-BEST-MODEL-GAP"].get("pass")
        ]
        if eligible:
            selected_draft = max(
                eligible, key=lambda model: model["aggregate"]["ndcgAt10"]
            )["hubId"]
            selected_passes = True
        else:
            selected_draft = best_observed
            selected_passes = False
    else:
        selected_draft = None
        selected_passes = False

    config_snapshot = {
        "chunkingVersion": gate_config["chunkingVersion"],
        "normalize": gate_config["normalize"],
        "ranking": gate_config["ranking"],
        "payloadFormat": gate_config["payloadFormat"],
        "gates": {
            "G0-RET-RECALL-AT-5": {
                "threshold": gate_config["recallThreshold"],
                "statistic": gate_config["recallStatistic"],
            },
            "G0-RET-BEST-MODEL-GAP": {
                "threshold": gate_config["gapThreshold"],
                "statistic": gate_config["gapStatistic"],
            },
        },
        "models": [
            {
                "id": model["modelId"],
                "hubId": model["hubId"],
                "provider": model["provider"],
                "revision": model["revisionResolved"],
                "revisionRequested": model["revisionRequested"],
                "modelMutability": model["modelMutability"],
                "observedAt": model["observedAt"],
                "dimensions": model["dimensions"],
                "maxSeqLength": model["maxSeqLength"],
                "batchSize": model["batchSize"],
                "device": model["deviceResolved"],
                "wordSegment": model["wordSegment"],
                "wordSegmenter": model["wordSegmenter"],
                "normalize": gate_config["normalize"],
            }
            for model in model_summaries
        ],
    }
    reasons = [
        "Quality track executed with independent model loads per run.",
        "Gate thresholds/statistics loaded from catalog YAML.",
        "Selection requires both Recall@5 and best-model-gap gates under gating protocol.",
        "Per-query rankings retained in run-*.json with rankingSha256 fingerprints.",
        "Golden markdown/queries validated against manifest.lock.json.",
        "Capacity evidence (VRAM, saturation, queue depth, target GPU) still required.",
        "ADR remains Proposed until capacity + approver sign-off.",
        "Restricted corpus must not leave to cloud providers; local/self-host only.",
    ]
    if not gating_protocol:
        reasons.insert(
            0,
            "NON-GATING run: threshold observations only; no formal PASS/FAIL or selectedDraft.",
        )
    if git_meta["dirty"]:
        reasons.insert(
            0,
            "Worktree was dirty at eval start; see git.dirtyPaths for exact inputs drift.",
        )
    if reject_track:
        reasons.insert(
            0,
            "OpenAI cloud reject track: same desktop payload/chunking/ranking as local harness; "
            "OpenAI model ids are mutable aliases (observation pin only).",
        )
    summary = {
        "version": 1,
        "issue": "P0-05",
        "track": track,
        "catalogPath": str(catalog_path.relative_to(ROOT)),
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "git": git_meta,
        "hardware": hardware,
        "device": device,
        "chunkingVersion": gate_config["chunkingVersion"],
        "payloadFormat": gate_config["payloadFormat"],
        "runsPerModel": args.runs,
        "fixtureManifestSha256": fixture_validation["manifestLockSha256"],
        "fixtureValidation": fixture_validation,
        "chunkCount": len(chunks),
        "queryCount": len(queries),
        "models": model_summaries,
        "immutableConfig": config_snapshot,
        "verdict": {
            "gatingProtocol": gating_protocol,
            "familyCount": len(families),
            "anyRecallGatePass": (
                any(model["gates"]["G0-RET-RECALL-AT-5"].get("pass") for model in model_summaries)
                if gating_protocol
                else False
            ),
            "anyMeetsRecallThreshold": any(
                model["gates"]["G0-RET-RECALL-AT-5"].get("meetsThreshold")
                for model in model_summaries
            ),
            "selectedPassesBothGates": selected_passes and gating_protocol,
            "selectedDraft": selected_draft,
            "bestObservedModel": best_observed,
            "p0_05_closed": False,
            "reasons": reasons,
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
