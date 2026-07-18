#!/usr/bin/env python3
"""P0-06 retrieval evaluation scaffold (lexical / local-hash vector / hybrid)."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import statistics
import struct
import subprocess
import sys
import unicodedata
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MD_DIR = CORPUS / "golden/markdown"
QUERIES = CORPUS / "golden/queries.tsv"
EXPECTED = CORPUS / "retrieval/expected-chunks.tsv"
REPORT = CORPUS / "reports/retrieval-evaluation.md"
SUMMARY = CORPUS / "retrieval/summary.json"

# Frozen desktop parity constants from crates/knowledge/src/rank.rs
RRF_K = 60.0
RRF_RERANK_SCALE = 30.0
VECTOR_WEIGHT = 0.55
BODY_OVERLAP_WEIGHT = 0.35
HEADING_HIT_WEIGHT = 0.1
LOCAL_DIMS = 256

sys.path.insert(0, str(Path(__file__).resolve().parent))
from knowledge_identity import (  # noqa: E402
    DEFAULT_CHUNKING_VERSION,
    RUNTIME_LOCAL_HASH,
    index_signature,
)


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def normalize_search_text(text: str) -> str:
    """Approximate fileconv_core::intelligence::normalize_search_text for tokens."""
    folded = unicodedata.normalize("NFKD", text)
    folded = "".join(ch for ch in folded if not unicodedata.combining(ch))
    folded = folded.casefold()
    return re.sub(r"\s+", " ", folded).strip()


def tokens(text: str) -> list[str]:
    return [part for part in re.split(r"[^\w]+", normalize_search_text(text), flags=re.UNICODE) if part]


def local_vector(text: str) -> list[float]:
    """Desktop local_hash_v1 feature hashing (SipHash13 zero-key via Python fallback).

    Exact SipHash parity is validated in Rust; for this scaffold we use a stable
    Python blake2b mix that is *deterministic* and sufficient for hybrid ranking
    experiments. Signature still records runtime=local-hash.
    """
    toks = tokens(text)
    vector = [0.0] * LOCAL_DIMS

    def add_feature(feature: str, weight: float) -> None:
        digest = hashlib.blake2b(feature.encode("utf-8"), digest_size=8).digest()
        value = struct.unpack("<Q", digest)[0]
        index = value % LOCAL_DIMS
        sign = 1.0 if (value & (1 << 63)) == 0 else -1.0
        vector[index] += sign * weight

    for token in toks:
        add_feature(token, 1.0)
    for left, right in zip(toks, toks[1:]):
        add_feature(f"{left}:{right}", 0.65)
    compact = [ch for ch in normalize_search_text(text) if not ch.isspace()]
    for index in range(max(0, len(compact) - 2)):
        add_feature("".join(compact[index : index + 3]), 0.15)
    norm = math.sqrt(sum(value * value for value in vector))
    if norm > 0:
        vector = [value / norm for value in vector]
    return vector


def cosine(left: list[float], right: list[float]) -> float:
    if len(left) != len(right) or not left:
        return 0.0
    return sum(a * b for a, b in zip(left, right))


def load_chunks() -> list[dict]:
    with EXPECTED.open(encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle, delimiter="\t"))
    chunks = []
    for row in rows:
        path = MD_DIR / f"{row['documentId']}.md"
        raw = path.read_bytes()
        body = raw[int(row["start"]) : int(row["end"])].decode("utf-8")
        chunks.append(
            {
                **row,
                "body": body,
                "docId": row["documentId"],
                "vector": local_vector(f"{row['headingPath']}\n{body}"),
            }
        )
    return chunks


def load_queries() -> list[dict]:
    with QUERIES.open(encoding="utf-8") as handle:
        return list(csv.DictReader(handle, delimiter="\t"))


def discounted_gain(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(index + 2) for index, grade in enumerate(grades))


def rank_lexical(query: str, chunks: list[dict]) -> list[str]:
    q_tokens = tokens(query)
    scored = []
    for chunk in chunks:
        body_set = set(tokens(chunk["body"]))
        heading_set = set(tokens(chunk["headingPath"]))
        overlap = sum(1 for token in q_tokens if token in body_set)
        heading = sum(1 for token in q_tokens if token in heading_set)
        score = overlap + 0.5 * heading
        scored.append((score, chunk["docId"]))
    # Aggregate max score per document
    best: dict[str, float] = {}
    for score, doc in scored:
        best[doc] = max(best.get(doc, 0.0), score)
    return [doc for doc, _ in sorted(best.items(), key=lambda item: (-item[1], item[0]))]


def rank_vector(query: str, chunks: list[dict]) -> list[str]:
    q_vec = local_vector(query)
    best: dict[str, float] = {}
    for chunk in chunks:
        score = cosine(q_vec, chunk["vector"])
        best[chunk["docId"]] = max(best.get(chunk["docId"], -1.0), score)
    return [doc for doc, _ in sorted(best.items(), key=lambda item: (-item[1], item[0]))]


def rank_hybrid(query: str, chunks: list[dict]) -> list[str]:
    lexical = rank_lexical(query, chunks)
    vector = rank_vector(query, chunks)
    lex_rank = {doc: index for index, doc in enumerate(lexical)}
    vec_rank = {doc: index for index, doc in enumerate(vector)}
    q_tokens = tokens(query)
    q_vec = local_vector(query)
    best: dict[str, float] = {}
    for chunk in chunks:
        doc = chunk["docId"]
        vector_score = cosine(q_vec, chunk["vector"])
        body_overlap = (
            sum(1 for token in q_tokens if token in set(tokens(chunk["body"])))
            / max(1, len(q_tokens))
        )
        heading_hits = sum(
            1 for token in q_tokens if token in normalize_search_text(chunk["headingPath"])
        )
        rrf = 0.0
        if doc in lex_rank:
            rrf += 1.0 / (RRF_K + lex_rank[doc])
        if doc in vec_rank:
            rrf += 1.0 / (RRF_K + vec_rank[doc])
        score = (
            rrf * RRF_RERANK_SCALE
            + max(vector_score, 0.0) * VECTOR_WEIGHT
            + body_overlap * BODY_OVERLAP_WEIGHT
            + heading_hits * HEADING_HIT_WEIGHT
        )
        best[doc] = max(best.get(doc, -1.0), score)
    return [doc for doc, _ in sorted(best.items(), key=lambda item: (-item[1], item[0]))]


def evaluate(ranked: list[str], judgments: dict[str, int]) -> dict:
    relevant = {doc for doc, grade in judgments.items() if grade >= 2}
    if not relevant:
        return {
            "hasRelevant": False,
            "recallAt5": 0.0,
            "recallAt10": 0.0,
            "hitAt5": 0.0,
            "mrr": 0.0,
            "ndcgAt10": 0.0,
        }
    recall5 = len(relevant.intersection(ranked[:5])) / len(relevant)
    recall10 = len(relevant.intersection(ranked[:10])) / len(relevant)
    hit5 = 1.0 if any(doc in relevant for doc in ranked[:5]) else 0.0
    mrr = next((1 / (index + 1) for index, doc in enumerate(ranked) if doc in relevant), 0.0)
    actual = [judgments.get(doc, 0) for doc in ranked[:10]]
    ideal = sorted(judgments.values(), reverse=True)[:10]
    ideal_gain = discounted_gain(ideal)
    ndcg = discounted_gain(actual) / ideal_gain if ideal_gain else 1.0
    return {
        "hasRelevant": True,
        "recallAt5": recall5,
        "recallAt10": recall10,
        "hitAt5": hit5,
        "mrr": mrr,
        "ndcgAt10": ndcg,
    }


def summarize(rows: list[dict]) -> dict:
    relevant = [row for row in rows if row["hasRelevant"]]
    count = max(1, len(relevant))
    return {
        "queries": len(relevant),
        "noAnswerQueries": sum(1 for row in rows if not row["hasRelevant"]),
        "recallAt5": round(sum(row["recallAt5"] for row in relevant) / count, 6),
        "recallAt10": round(sum(row["recallAt10"] for row in relevant) / count, 6),
        "hitAt5": round(sum(row["hitAt5"] for row in relevant) / count, 6),
        "mrr": round(sum(row["mrr"] for row in relevant) / count, 6),
        "ndcgAt10": round(sum(row["ndcgAt10"] for row in relevant) / count, 6),
    }


def version_citation_metrics(queries: list[dict]) -> dict:
    """Stub metrics until version-aware citation payload is filled (chunkId)."""
    total = 0
    with_chunk = 0
    for row in queries:
        for cite in json.loads(row.get("citations") or "[]"):
            total += 1
            if cite.get("chunkId"):
                with_chunk += 1
    return {
        "citations": total,
        "citationsWithChunkId": with_chunk,
        "versionCitationPrecision": 0.0,
        "versionCitationRecall": 0.0,
        "note": "chunkId still null in golden citations; fill after expected-chunks wiring",
    }


def self_test() -> int:
    if not EXPECTED.is_file():
        print("missing expected-chunks.tsv", file=sys.stderr)
        return 1
    sig = index_signature(
        runtime_path=RUNTIME_LOCAL_HASH,
        embedding_family="local/local_hash_v1/provider-default",
        embedding_revision="1",
        dimensions=LOCAL_DIMS,
        normalized=True,
    )
    if len(sig) != 64:
        print("bad signature length", file=sys.stderr)
        return 1
    chunks = load_chunks()
    if not chunks:
        print("no chunks loaded", file=sys.stderr)
        return 1
    ranked = rank_hybrid("Mã hồ sơ HS-2026-001", chunks)
    if "gold-001" not in ranked[:5]:
        print(f"self-test hybrid miss: {ranked[:5]}", file=sys.stderr)
        return 1
    print("self-test OK: expected-chunks + hybrid scaffold")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--report", type=Path, default=REPORT)
    parser.add_argument("--summary", type=Path, default=SUMMARY)
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not EXPECTED.is_file():
        raise SystemExit("run generate_expected_chunks.py first")

    chunks = load_chunks()
    queries = load_queries()
    signature = index_signature(
        runtime_path=RUNTIME_LOCAL_HASH,
        embedding_family="local/local_hash_v1/scaffold",
        embedding_revision="1",
        dimensions=LOCAL_DIMS,
        normalized=True,
    )
    legs = {
        "lexical": rank_lexical,
        "vector_local_hash": rank_vector,
        "hybrid": rank_hybrid,
    }
    leg_summaries = {}
    for name, ranker in legs.items():
        rows = []
        for query in queries:
            judgments = {
                key: int(value)
                for key, value in json.loads(query.get("judgments") or "{}").items()
            }
            ranked = ranker(query["query"], chunks)
            metrics = evaluate(ranked, judgments)
            rows.append(
                {
                    "queryId": query["query_id"],
                    "category": query["category"],
                    "rankedDocuments": ranked[:10],
                    **metrics,
                }
            )
        leg_summaries[name] = summarize(rows)

    citation = version_citation_metrics(queries)
    payload = {
        "version": 1,
        "issue": "P0-06",
        "track": "retrieval-scaffold-local-hash",
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "git": {"commit": git("rev-parse", "HEAD"), "dirty": bool(git("status", "--porcelain"))},
        "chunkingVersion": DEFAULT_CHUNKING_VERSION,
        "indexSignature": signature,
        "runtimePath": RUNTIME_LOCAL_HASH,
        "rrf": {
            "k": RRF_K,
            "rerankScale": RRF_RERANK_SCALE,
            "vectorWeight": VECTOR_WEIGHT,
            "bodyOverlapWeight": BODY_OVERLAP_WEIGHT,
            "headingHitWeight": HEADING_HIT_WEIGHT,
            "tuned": False,
            "note": "frozen desktop parity constants; tuning deferred",
        },
        "legs": leg_summaries,
        "versionCitation": citation,
        "gates": {
            "G0-RET-RECALL-AT-5": {
                "metric": leg_summaries["hybrid"]["recallAt5"],
                "threshold": 0.85,
                "pass": leg_summaries["hybrid"]["recallAt5"] >= 0.85,
                "note": "local-hash scaffold; neural hybrid may differ",
            },
            "G0-RET-VERSION-CITATION-PRECISION": {
                "metric": citation["versionCitationPrecision"],
                "threshold": 1.0,
                "pass": False,
                "note": citation["note"],
            },
        },
        "p0_06_closed": False,
        "reasons": [
            "Expected chunks pinned for heading-chunks-2000-v1 with span resolve.",
            "Hybrid scaffold uses frozen RRF weights + local-hash vectors.",
            "Version-citation gates remain red until chunkId is filled into gold.",
            "Neural embedding hybrid + claim/conflict metrics deferred.",
        ],
    }
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    lines = [
        "# P0-06 retrieval evaluation (scaffold)",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Chunking: `{payload['chunkingVersion']}`",
        f"- Runtime path: `{payload['runtimePath']}`",
        f"- Index signature: `{payload['indexSignature']}`",
        f"- RRF tuned: `{payload['rrf']['tuned']}`",
        "",
        "## Legs (document-level, local-hash scaffold)",
        "",
        "| Leg | Recall@5 | Recall@10 | Hit@5 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for name, stats in leg_summaries.items():
        lines.append(
            f"| `{name}` | {stats['recallAt5']:.4f} | {stats['recallAt10']:.4f} | "
            f"{stats['hitAt5']:.4f} | {stats['mrr']:.4f} | {stats['ndcgAt10']:.4f} |"
        )
    lines += [
        "",
        "## Version citation / temporal",
        "",
        f"- Citations total: `{citation['citations']}`",
        f"- Citations with chunkId: `{citation['citationsWithChunkId']}`",
        f"- Version-citation precision/recall: `{citation['versionCitationPrecision']}` / "
        f"`{citation['versionCitationRecall']}` (not yet measurable)",
        f"- Note: {citation['note']}",
        "",
        "## Verdict",
        "",
        f"- P0-06 closed: **{'YES' if payload['p0_06_closed'] else 'NO'}**",
        "",
    ]
    for reason in payload["reasons"]:
        lines.append(f"- {reason}")
    lines.append("")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {args.summary}")
    print(f"wrote {args.report}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
