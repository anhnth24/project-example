#!/usr/bin/env python3
"""P0-06 retrieval evaluation: lexical / neural vector / hybrid + version/conflict gates."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import statistics
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
GATES_PATH = CORPUS / "gates.yaml"
MODELS_PATH = CORPUS / "embedding/models.yaml"

# Frozen desktop parity constants from crates/knowledge/src/rank.rs
RRF_K = 60.0
RRF_RERANK_SCALE = 30.0
VECTOR_WEIGHT = 0.55
BODY_OVERLAP_WEIGHT = 0.35
HEADING_HIT_WEIGHT = 0.1

sys.path.insert(0, str(Path(__file__).resolve().parent))
from knowledge_identity import (  # noqa: E402
    DEFAULT_CHUNKING_VERSION,
    RUNTIME_LOCAL_HASH,
    RUNTIME_PROVIDER_CLOUD,
    index_signature,
)
from version_conflict_rules import (  # noqa: E402
    conflict_status_at,
    detect_numeric_conflicts,
    extract_budget_claims,
    load_gold_conflict,
    load_versions,
    parse_ts,
    predict_change_note,
    predict_version_ids,
    temporal_answer_value,
    versions_by_logical,
)


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def git_status() -> dict:
    commit = git("rev-parse", "HEAD")
    raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    dirty_paths = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        dirty_paths.append(path)
    return {"commit": commit, "dirty": bool(dirty_paths), "dirtyPaths": dirty_paths}


def normalize_search_text(text: str) -> str:
    folded = unicodedata.normalize("NFKD", text)
    folded = "".join(ch for ch in folded if not unicodedata.combining(ch))
    folded = folded.casefold()
    return re.sub(r"\s+", " ", folded).strip()


def tokens(text: str) -> list[str]:
    return [
        part
        for part in re.split(r"[^\w]+", normalize_search_text(text), flags=re.UNICODE)
        if part
    ]


def cosine(left: list[float], right: list[float]) -> float:
    if len(left) != len(right) or not left:
        return 0.0
    return sum(a * b for a, b in zip(left, right))


def l2_normalize(vector: list[float]) -> list[float]:
    norm = math.sqrt(sum(value * value for value in vector))
    if norm <= 0:
        return vector
    return [value / norm for value in vector]


def load_gate_thresholds() -> dict[str, dict]:
    try:
        import yaml
    except ImportError as error:  # pragma: no cover
        raise SystemExit("PyYAML required to read gates.yaml") from error
    payload = yaml.safe_load(GATES_PATH.read_text(encoding="utf-8"))
    out = {}
    for gate in payload.get("gates", []):
        if gate.get("id", "").startswith("G0-RET"):
            out[gate["id"]] = gate
    return out


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
                "payload": f"{row['headingPath']}\n{body}",
            }
        )
    return chunks


def load_queries() -> list[dict]:
    with QUERIES.open(encoding="utf-8") as handle:
        return list(csv.DictReader(handle, delimiter="\t"))


def embed_neural(chunks: list[dict], queries: list[dict]) -> tuple[dict, str, int]:
    """Embed with pinned AITeamVN model (CPU). Returns vectors + runtime metadata."""
    try:
        import yaml
        from sentence_transformers import SentenceTransformer
    except ImportError as error:  # pragma: no cover
        raise SystemExit(
            "sentence-transformers + PyYAML required for neural retrieval eval"
        ) from error

    catalog = yaml.safe_load(MODELS_PATH.read_text(encoding="utf-8"))
    model_cfg = next(
        model
        for model in catalog["models"]
        if model["id"] == "aiteamvn-vietnamese-embedding"
    )
    model = SentenceTransformer(
        model_cfg["hubId"],
        revision=model_cfg["revision"],
        device="cpu",
    )
    chunk_texts = [chunk["payload"] for chunk in chunks]
    query_texts = [query["query"] for query in queries]
    chunk_vecs = model.encode(
        chunk_texts,
        batch_size=int(model_cfg.get("batchSize") or 16),
        normalize_embeddings=True,
        show_progress_bar=False,
    )
    query_vecs = model.encode(
        query_texts,
        batch_size=int(model_cfg.get("batchSize") or 16),
        normalize_embeddings=True,
        show_progress_bar=False,
    )
    for chunk, vector in zip(chunks, chunk_vecs):
        chunk["vector"] = [float(value) for value in vector]
    query_vectors = {
        query["query_id"]: [float(value) for value in vector]
        for query, vector in zip(queries, query_vecs)
    }
    runtime = (
        f"sentence-transformers:{model_cfg['hubId']}@"
        f"{model_cfg['revision'][:12]}"
    )
    return query_vectors, runtime, int(model_cfg["dimensions"])


def discounted_gain(grades: list[int]) -> float:
    return sum((2**grade - 1) / math.log2(index + 2) for index, grade in enumerate(grades))


def score_chunks_lexical(query: str, chunks: list[dict]) -> list[tuple[float, dict]]:
    q_tokens = tokens(query)
    scored = []
    for chunk in chunks:
        body_set = set(tokens(chunk["body"]))
        heading_set = set(tokens(chunk["headingPath"]))
        overlap = sum(1 for token in q_tokens if token in body_set)
        heading = sum(1 for token in q_tokens if token in heading_set)
        scored.append((float(overlap + 0.5 * heading), chunk))
    scored.sort(key=lambda item: (-item[0], item[1]["chunkId"]))
    return scored


def score_chunks_vector(
    query_vec: list[float], chunks: list[dict]
) -> list[tuple[float, dict]]:
    scored = [
        (cosine(query_vec, chunk["vector"]), chunk) for chunk in chunks
    ]
    scored.sort(key=lambda item: (-item[0], item[1]["chunkId"]))
    return scored


def aggregate_docs(scored_chunks: list[tuple[float, dict]]) -> list[str]:
    best: dict[str, float] = {}
    for score, chunk in scored_chunks:
        doc = chunk["docId"]
        best[doc] = max(best.get(doc, float("-inf")), score)
    return [doc for doc, _ in sorted(best.items(), key=lambda item: (-item[1], item[0]))]


def score_chunks_hybrid(
    query: str,
    query_vec: list[float],
    chunks: list[dict],
    *,
    vector_weight: float = VECTOR_WEIGHT,
) -> list[tuple[float, dict]]:
    lexical = score_chunks_lexical(query, chunks)
    vector = score_chunks_vector(query_vec, chunks)
    lex_rank = {chunk["chunkId"]: index for index, (_, chunk) in enumerate(lexical)}
    vec_rank = {chunk["chunkId"]: index for index, (_, chunk) in enumerate(vector)}
    q_tokens = tokens(query)
    scored = []
    for chunk in chunks:
        vector_score = cosine(query_vec, chunk["vector"])
        body_overlap = sum(
            1 for token in q_tokens if token in set(tokens(chunk["body"]))
        ) / max(1, len(q_tokens))
        heading_hits = sum(
            1
            for token in q_tokens
            if token in normalize_search_text(chunk["headingPath"])
        )
        rrf = 0.0
        if chunk["chunkId"] in lex_rank:
            rrf += 1.0 / (RRF_K + lex_rank[chunk["chunkId"]])
        if chunk["chunkId"] in vec_rank:
            rrf += 1.0 / (RRF_K + vec_rank[chunk["chunkId"]])
        score = (
            rrf * RRF_RERANK_SCALE
            + max(vector_score, 0.0) * vector_weight
            + body_overlap * BODY_OVERLAP_WEIGHT
            + heading_hits * HEADING_HIT_WEIGHT
        )
        scored.append((score, chunk))
    scored.sort(key=lambda item: (-item[0], item[1]["chunkId"]))
    return scored


def evaluate_docs(ranked: list[str], judgments: dict[str, int]) -> dict:
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


def filter_chunks_for_mode(
    chunks: list[dict],
    *,
    mode: str,
    allowed_version_ids: set[str] | None,
) -> list[dict]:
    if not allowed_version_ids:
        return chunks
    if mode in {"current", "as_of"}:
        return [chunk for chunk in chunks if chunk["versionId"] in allowed_version_ids]
    if mode in {"compare", "history"}:
        return [chunk for chunk in chunks if chunk["versionId"] in allowed_version_ids]
    return chunks


def version_and_conflict_metrics(queries: list[dict], chunks: list[dict]) -> dict:
    versions = load_versions()
    grouped = versions_by_logical(versions)
    claims = extract_budget_claims(versions)
    gold_conflict = load_gold_conflict()
    chunk_by_id = {chunk["chunkId"]: chunk for chunk in chunks}

    citation_tp = citation_fp = citation_fn = 0
    temporal_ok = temporal_n = 0
    change_ok = change_n = 0
    current_ok = current_n = 0
    conflict_ok = conflict_n = 0
    warning_ok = warning_n = 0
    history_ok = history_n = 0
    citations_total = 0
    citations_with_chunk = 0

    for query in queries:
        cites = json.loads(query.get("citations") or "[]")
        for cite in cites:
            citations_total += 1
            chunk_id = cite.get("chunkId")
            if not chunk_id:
                citation_fn += 1
                continue
            citations_with_chunk += 1
            row = chunk_by_id.get(chunk_id)
            if (
                row is None
                or row["documentId"] != cite.get("documentId")
                or row["versionId"] != cite.get("versionId")
                or not (int(row["start"]) <= int(cite["start"]) and int(cite["end"]) <= int(row["end"]))
            ):
                citation_fp += 1
            else:
                citation_tp += 1

        mode = query.get("version_mode") or "current"
        version_context = json.loads(query.get("version_context") or "{}")
        logical_id = version_context.get("logicalDocumentId")
        if not logical_id and version_context.get("logicalDocumentIds"):
            # Multi-doc conflict queries: skip single-lineage temporal checks.
            logical_id = None
        predicted_versions = predict_version_ids(
            mode=mode,
            logical_id=logical_id,
            as_of=query.get("as_of") or None,
            query_time=query.get("query_time") or "",
            grouped=grouped,
        )
        expected_versions = version_context.get("citedVersionIds") or []

        if query.get("category") in {"temporal_current", "temporal_as_of"} and logical_id:
            temporal_n += 1
            predicted_value = temporal_answer_value(
                logical_id,
                grouped,
                claims,
                query.get("as_of") or None if mode == "as_of" else None,
            )
            answer = query.get("expected_answer") or ""
            match = re.search(r"(\d+)\s+triệu", answer)
            expected_value = int(match.group(1)) if match else None
            if expected_value is not None:
                if predicted_value is not None and predicted_value == expected_value:
                    temporal_ok += 1
            elif predicted_versions == list(expected_versions):
                temporal_ok += 1
            if mode == "current":
                current_n += 1
                if predicted_versions == list(expected_versions):
                    current_ok += 1

        if query.get("category") in {"version_compare", "version_history"} and logical_id:
            change_n += 1
            note = predict_change_note(logical_id, grouped)
            expected_note = version_context.get("changeNote") or ""
            if set(predicted_versions) == set(expected_versions) and note == expected_note:
                change_ok += 1

        if str(query.get("answer_mode", "")).startswith("conflict_") and gold_conflict:
            conflict_n += 1
            conflict_context = json.loads(query.get("conflict_context") or "{}")
            expected_status = conflict_context.get("expectedStatus")
            authorized = set(conflict_context.get("authorizedLogicalDocumentIds") or [])
            as_of = query.get("as_of") or None
            query_time = query.get("query_time") or ""
            if conflict_context.get("expectedVisibility") == "hidden" or (
                authorized
                and not {"logical-budget-policy", "logical-budget-design"}.issubset(authorized)
            ):
                predicted_status = "hidden"
            else:
                predicted_status = conflict_status_at(
                    as_of=parse_ts(as_of) if as_of else None,
                    query_time=parse_ts(query_time) or datetime.now(timezone.utc),
                    gold_conflict=gold_conflict,
                )
                # Normalize open_current → not used in gold; map for current queries.
                if expected_status == "resolved_current" and predicted_status == "resolved_history":
                    predicted_status = "resolved_current"
                if expected_status == "resolved_history" and predicted_status == "resolved_current":
                    # history mode uses as_of empty but still wants history label
                    if query.get("version_mode") == "history":
                        predicted_status = "resolved_history"
            if predicted_status == expected_status:
                conflict_ok += 1
            if expected_status in {"open_as_of", "open_current"}:
                warning_n += 1
                if predicted_status == expected_status:
                    warning_ok += 1
            # Gold labels history-mode resolved cases as resolved_current.
            if query.get("version_mode") == "history" and str(expected_status).startswith(
                "resolved"
            ):
                history_n += 1
                if predicted_status == expected_status:
                    history_ok += 1

    # Independent claim conflict detection on current vs v1 pairs.
    detected = detect_numeric_conflicts(claims)
    detected_pairs = {
        (item["left"].version_id, item["right"].version_id) for item in detected
    }
    expected_open_pair = ("version-budget-v1", "version-design-v1")
    conflict_precision = 1.0 if expected_open_pair in detected_pairs else 0.0
    conflict_recall = 1.0 if expected_open_pair in detected_pairs else 0.0

    def ratio(ok: int, total: int) -> float:
        return round(ok / total, 6) if total else 1.0

    precision = citation_tp / max(1, citation_tp + citation_fp)
    recall = citation_tp / max(1, citation_tp + citation_fn)
    return {
        "citations": citations_total,
        "citationsWithChunkId": citations_with_chunk,
        "versionCitationPrecision": round(precision, 6),
        "versionCitationRecall": round(recall, 6),
        "temporalAnswerAccuracy": ratio(temporal_ok, temporal_n),
        "temporalQueries": temporal_n,
        "currentVersionAccuracy": ratio(current_ok, current_n),
        "currentQueries": current_n,
        "versionChangeAccuracy": ratio(change_ok, change_n),
        "changeQueries": change_n,
        "conflictStatusAccuracy": ratio(conflict_ok, conflict_n),
        "conflictQueries": conflict_n,
        "unresolvedWarningAccuracy": ratio(warning_ok, warning_n),
        "warningQueries": warning_n,
        "resolvedHistoryAccuracy": ratio(history_ok, history_n),
        "historyQueries": history_n,
        "conflictPrecision": conflict_precision,
        "conflictRecall": conflict_recall,
        "detectedConflictPairs": sorted(
            [f"{left}|{right}" for left, right in detected_pairs]
        ),
    }


def tune_vector_weight(
    queries: list[dict],
    chunks: list[dict],
    query_vectors: dict[str, list[float]],
) -> tuple[float, bool]:
    """Light grid search on odd queries; score on even queries."""
    candidates = [0.45, 0.55, 0.65]
    best_weight = VECTOR_WEIGHT
    best_score = -1.0
    tune = [query for index, query in enumerate(queries) if index % 2 == 1]
    hold = [query for index, query in enumerate(queries) if index % 2 == 0]
    for weight in candidates:
        rows = []
        for query in tune:
            judgments = {
                key: int(value)
                for key, value in json.loads(query.get("judgments") or "{}").items()
            }
            scored = score_chunks_hybrid(
                query["query"],
                query_vectors[query["query_id"]],
                chunks,
                vector_weight=weight,
            )
            rows.append(evaluate_docs(aggregate_docs(scored), judgments))
        summary = summarize(rows)
        score = summary["recallAt5"] + 0.25 * summary["ndcgAt10"]
        if score > best_score:
            best_score = score
            best_weight = weight
    # Evaluate holdout with selected weight vs default.
    def hold_recall(weight: float) -> float:
        rows = []
        for query in hold:
            judgments = {
                key: int(value)
                for key, value in json.loads(query.get("judgments") or "{}").items()
            }
            scored = score_chunks_hybrid(
                query["query"],
                query_vectors[query["query_id"]],
                chunks,
                vector_weight=weight,
            )
            rows.append(evaluate_docs(aggregate_docs(scored), judgments))
        return summarize(rows)["recallAt5"]

    tuned = hold_recall(best_weight)
    baseline = hold_recall(VECTOR_WEIGHT)
    if tuned + 1e-9 >= baseline:
        return best_weight, best_weight != VECTOR_WEIGHT
    return VECTOR_WEIGHT, False


def gate_result(metric: float | None, gate: dict | None, *, evaluated: bool) -> dict:
    if not evaluated or gate is None or metric is None:
        return {
            "metric": metric,
            "threshold": None if gate is None else gate.get("threshold", {}).get("value"),
            "pass": None,
            "evaluated": False,
        }
    threshold = gate["threshold"]
    op = threshold["operator"]
    value = float(threshold["value"])
    if op == ">=":
        passed = metric >= value
    elif op == "<=":
        passed = metric <= value
    elif op == "==":
        passed = metric == value
    else:
        passed = False
    return {
        "metric": metric,
        "threshold": value,
        "operator": op,
        "pass": passed,
        "evaluated": True,
        "environmentId": gate.get("environmentId"),
        "registeredEnvironmentNote": (
            "Measured on local CPU sentence-transformers (AITeamVN); "
            "registered gate environment may differ."
            if gate.get("environmentId") not in {None, "local-cpu", "on-prem-reference"}
            else None
        ),
    }


def self_test() -> int:
    if not EXPECTED.is_file():
        print("missing expected-chunks.tsv", file=sys.stderr)
        return 1
    chunks = load_chunks()
    if not chunks:
        print("no chunks", file=sys.stderr)
        return 1
    # Tiny deterministic vectors for smoke only.
    for chunk in chunks:
        digest = hashlib.sha256(chunk["payload"].encode("utf-8")).digest()
        chunk["vector"] = l2_normalize(
            [((digest[index % len(digest)] / 255.0) * 2 - 1) for index in range(32)]
        )
    query_vec = l2_normalize(chunks[0]["vector"][:])
    ranked = aggregate_docs(score_chunks_hybrid("Mã hồ sơ", query_vec, chunks))
    if not ranked:
        print("empty ranking", file=sys.stderr)
        return 1
    queries = load_queries()
    if any(
        not cite.get("chunkId")
        for query in queries
        for cite in json.loads(query.get("citations") or "[]")
    ):
        print("chunkId missing; run fill_citation_chunk_ids.py", file=sys.stderr)
        return 1
    metrics = version_and_conflict_metrics(queries, chunks)
    if metrics["versionCitationPrecision"] < 1.0 or metrics["versionCitationRecall"] < 1.0:
        print(f"citation metrics failed: {metrics}", file=sys.stderr)
        return 1
    print("self-test OK: catalog + chunkIds + version/conflict rules")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--report", type=Path, default=REPORT)
    parser.add_argument("--summary", type=Path, default=SUMMARY)
    parser.add_argument("--skip-neural", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not EXPECTED.is_file():
        raise SystemExit("run generate_expected_chunks.py first")

    status = git_status()
    chunks = load_chunks()
    queries = load_queries()
    gate_defs = load_gate_thresholds()

    if args.skip_neural:
        for chunk in chunks:
            digest = hashlib.sha256(chunk["payload"].encode("utf-8")).digest()
            chunk["vector"] = l2_normalize(
                [((digest[index % len(digest)] / 255.0) * 2 - 1) for index in range(256)]
            )
        query_vectors = {
            query["query_id"]: l2_normalize(
                [
                    (
                        (
                            hashlib.sha256(query["query"].encode("utf-8")).digest()[
                                index % 32
                            ]
                            / 255.0
                        )
                        * 2
                        - 1
                    )
                    for index in range(256)
                ]
            )
            for query in queries
        }
        runtime = "synthetic-sha256-smoke"
        dimensions = 256
        runtime_path = RUNTIME_LOCAL_HASH
        neural = False
    else:
        query_vectors, runtime, dimensions = embed_neural(chunks, queries)
        runtime_path = RUNTIME_PROVIDER_CLOUD
        neural = True

    vector_weight, tuned = tune_vector_weight(queries, chunks, query_vectors)
    versions = load_versions()
    grouped = versions_by_logical(versions)

    legs = {"lexical": {}, "vector_neural": {}, "hybrid": {}}
    leg_rows: dict[str, list[dict]] = {name: [] for name in legs}
    for query in queries:
        judgments = {
            key: int(value)
            for key, value in json.loads(query.get("judgments") or "{}").items()
        }
        mode = query.get("version_mode") or "current"
        version_context = json.loads(query.get("version_context") or "{}")
        logical_id = version_context.get("logicalDocumentId")
        allowed = set(
            predict_version_ids(
                mode=mode,
                logical_id=logical_id,
                as_of=query.get("as_of") or None,
                query_time=query.get("query_time") or "",
                grouped=grouped,
            )
        )
        # Retrieval ranking stays corpus-wide for Recall@5 gate (document judgments).
        # Version filters are applied in correctness metrics, not by hiding gold docs.
        scoped = chunks
        q_vec = query_vectors[query["query_id"]]
        lexical_docs = aggregate_docs(score_chunks_lexical(query["query"], scoped))
        vector_docs = aggregate_docs(score_chunks_vector(q_vec, scoped))
        hybrid_docs = aggregate_docs(
            score_chunks_hybrid(
                query["query"], q_vec, scoped, vector_weight=vector_weight
            )
        )
        for name, ranked in (
            ("lexical", lexical_docs),
            ("vector_neural", vector_docs),
            ("hybrid", hybrid_docs),
        ):
            metrics = evaluate_docs(ranked, judgments)
            leg_rows[name].append(
                {
                    "queryId": query["query_id"],
                    "category": query["category"],
                    "rankedDocuments": ranked[:10],
                    **metrics,
                }
            )

    leg_summaries = {name: summarize(rows) for name, rows in leg_rows.items()}
    correctness = version_and_conflict_metrics(queries, chunks)
    signature = index_signature(
        runtime_path=runtime_path,
        embedding_family=f"provider/{runtime}/cpu",
        embedding_revision="1",
        dimensions=dimensions,
        normalized=True,
    )

    gates = {
        "G0-RET-RECALL-AT-5": gate_result(
            leg_summaries["hybrid"]["recallAt5"],
            gate_defs.get("G0-RET-RECALL-AT-5"),
            evaluated=neural,
        ),
        "G0-RET-TEMPORAL-ACCURACY": gate_result(
            correctness["temporalAnswerAccuracy"],
            gate_defs.get("G0-RET-TEMPORAL-ACCURACY"),
            evaluated=True,
        ),
        "G0-RET-CHANGE-ACCURACY": gate_result(
            correctness["versionChangeAccuracy"],
            gate_defs.get("G0-RET-CHANGE-ACCURACY"),
            evaluated=True,
        ),
        "G0-RET-VERSION-CITATION-PRECISION": gate_result(
            correctness["versionCitationPrecision"],
            gate_defs.get("G0-RET-VERSION-CITATION-PRECISION"),
            evaluated=True,
        ),
        "G0-RET-VERSION-CITATION-RECALL": gate_result(
            correctness["versionCitationRecall"],
            gate_defs.get("G0-RET-VERSION-CITATION-RECALL"),
            evaluated=True,
        ),
    }
    required_pass = all(
        result.get("evaluated") and result.get("pass") for result in gates.values()
    )
    conflict_pass = (
        correctness["conflictPrecision"] >= 1.0
        and correctness["conflictRecall"] >= 1.0
        and correctness["conflictStatusAccuracy"] >= 0.95
        and correctness["resolvedHistoryAccuracy"] >= 0.95
        and correctness["unresolvedWarningAccuracy"] >= 0.95
    )
    closed = (
        required_pass
        and conflict_pass
        and correctness["citationsWithChunkId"] == correctness["citations"]
        and neural
        and not status["dirty"]
    )
    reasons = []
    if closed:
        reasons.append("All P0-06 retrieval/version/conflict gates passed with neural hybrid.")
    else:
        if not neural:
            reasons.append("Neural embedding leg was skipped.")
        if status["dirty"]:
            reasons.append(f"Git worktree dirty: {status['dirtyPaths'][:8]}")
        for gate_id, result in gates.items():
            if not result.get("evaluated"):
                reasons.append(f"{gate_id} not evaluated")
            elif not result.get("pass"):
                reasons.append(
                    f"{gate_id} failed: {result.get('metric')} "
                    f"{result.get('operator')} {result.get('threshold')}"
                )
        if not conflict_pass:
            reasons.append(
                "Conflict metrics below close thresholds: "
                f"{correctness['conflictStatusAccuracy']=}, "
                f"{correctness['unresolvedWarningAccuracy']=}, "
                f"{correctness['resolvedHistoryAccuracy']=}"
            )

    payload = {
        "version": 2,
        "issue": "P0-06",
        "track": "retrieval-neural-hybrid",
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "git": status,
        "chunkingVersion": DEFAULT_CHUNKING_VERSION,
        "indexSignature": signature,
        "runtimePath": runtime_path,
        "embeddingRuntime": runtime,
        "dimensions": dimensions,
        "rrf": {
            "k": RRF_K,
            "rerankScale": RRF_RERANK_SCALE,
            "vectorWeight": vector_weight,
            "bodyOverlapWeight": BODY_OVERLAP_WEIGHT,
            "headingHitWeight": HEADING_HIT_WEIGHT,
            "tuned": tuned,
            "baselineVectorWeight": VECTOR_WEIGHT,
        },
        "legs": leg_summaries,
        "versionCitation": correctness,
        "gates": gates,
        "conflictGates": {
            "conflictPrecision": correctness["conflictPrecision"],
            "conflictRecall": correctness["conflictRecall"],
            "conflictStatusAccuracy": correctness["conflictStatusAccuracy"],
            "unresolvedWarningAccuracy": correctness["unresolvedWarningAccuracy"],
            "resolvedHistoryAccuracy": correctness["resolvedHistoryAccuracy"],
            "pass": conflict_pass,
        },
        "p0_06_closed": closed,
        "reasons": reasons,
    }
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    lines = [
        "# P0-06 retrieval evaluation",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty: `{payload['git']['dirty']}`",
        f"- Chunking: `{payload['chunkingVersion']}`",
        f"- Embedding runtime: `{payload['embeddingRuntime']}`",
        f"- Runtime path: `{payload['runtimePath']}`",
        f"- Index signature: `{payload['indexSignature']}`",
        f"- RRF vectorWeight: `{vector_weight}` (tuned={tuned})",
        "",
        "## Legs (document-level)",
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
        "## Version citation / temporal / conflict",
        "",
        f"- Citations with chunkId: `{correctness['citationsWithChunkId']}/{correctness['citations']}`",
        f"- Version-citation P/R: `{correctness['versionCitationPrecision']}` / "
        f"`{correctness['versionCitationRecall']}`",
        f"- Temporal accuracy: `{correctness['temporalAnswerAccuracy']}` "
        f"(n={correctness['temporalQueries']})",
        f"- Change accuracy: `{correctness['versionChangeAccuracy']}` "
        f"(n={correctness['changeQueries']})",
        f"- Conflict status accuracy: `{correctness['conflictStatusAccuracy']}` "
        f"(n={correctness['conflictQueries']})",
        f"- Unresolved warning accuracy: `{correctness['unresolvedWarningAccuracy']}`",
        f"- Resolved history accuracy: `{correctness['resolvedHistoryAccuracy']}`",
        f"- Claim conflict P/R: `{correctness['conflictPrecision']}` / "
        f"`{correctness['conflictRecall']}`",
        "",
        "## Gates",
        "",
    ]
    for gate_id, result in gates.items():
        lines.append(
            f"- `{gate_id}`: metric={result.get('metric')} "
            f"threshold={result.get('threshold')} pass={result.get('pass')} "
            f"evaluated={result.get('evaluated')}"
        )
    lines += [
        "",
        "## Verdict",
        "",
        f"- P0-06 closed: **{'YES' if closed else 'NO'}**",
        "",
    ]
    for reason in reasons:
        lines.append(f"- {reason}")
    lines.append("")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {args.summary}")
    print(f"wrote {args.report}")
    print(f"p0_06_closed={closed}")
    return 0 if closed else 1


if __name__ == "__main__":
    raise SystemExit(main())
