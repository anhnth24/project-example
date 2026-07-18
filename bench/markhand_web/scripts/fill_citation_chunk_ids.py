#!/usr/bin/env python3
"""Fill canonical chunkId into golden citations from expected-chunks.tsv (P0-06).

Mechanical annotation only: query text, spans, answers, and judgments are unchanged.
Adjudication stays approved via sampleSemanticSha256 (chunkId-null packet hash).
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_CORPUS = ROOT / "bench/markhand_web"
CORPUS = DEFAULT_CORPUS
EXPECTED = CORPUS / "retrieval/expected-chunks.tsv"
QUERIES = CORPUS / "golden/queries.tsv"
CONFLICTS = CORPUS / "golden/conflicts.json"
REVIEW = CORPUS / "golden/review-sample.tsv"
ADJUDICATION = CORPUS / "golden/adjudication.json"


def configure_corpus(corpus: Path) -> None:
    global CORPUS, EXPECTED, QUERIES, CONFLICTS, REVIEW, ADJUDICATION
    CORPUS = corpus.resolve()
    EXPECTED = CORPUS / "retrieval/expected-chunks.tsv"
    QUERIES = CORPUS / "golden/queries.tsv"
    CONFLICTS = CORPUS / "golden/conflicts.json"
    REVIEW = CORPUS / "golden/review-sample.tsv"
    ADJUDICATION = CORPUS / "golden/adjudication.json"


def load_catalog() -> dict[str, list[dict]]:
    if not EXPECTED.is_file():
        raise SystemExit("missing retrieval/expected-chunks.tsv; generate it first")
    by_doc: dict[str, list[dict]] = {}
    with EXPECTED.open(encoding="utf-8", newline="") as handle:
        for row in csv.DictReader(handle, delimiter="\t"):
            by_doc.setdefault(row["documentId"], []).append(
                {
                    "chunkId": row["chunkId"],
                    "versionId": row["versionId"],
                    "start": int(row["start"]),
                    "end": int(row["end"]),
                }
            )
    return by_doc


def resolve_chunk_id(
    catalog: dict[str, list[dict]],
    *,
    document_id: str,
    version_id: str | None,
    start: int,
    end: int,
    label: str,
) -> str:
    covering = [
        row
        for row in catalog.get(document_id, [])
        if row["start"] <= start and end <= row["end"]
    ]
    if version_id:
        covering = [row for row in covering if row["versionId"] == version_id]
    if len(covering) != 1:
        raise SystemExit(
            f"{label}: expected exactly one covering chunk for "
            f"{document_id}[{start},{end}) version={version_id!r}; got {len(covering)}"
        )
    return covering[0]["chunkId"]


def annotate_anchor(catalog: dict[str, list[dict]], anchor: dict, label: str) -> bool:
    if not isinstance(anchor, dict):
        return False
    if "documentId" not in anchor or "start" not in anchor or "end" not in anchor:
        return False
    chunk_id = resolve_chunk_id(
        catalog,
        document_id=anchor["documentId"],
        version_id=anchor.get("versionId"),
        start=int(anchor["start"]),
        end=int(anchor["end"]),
        label=label,
    )
    changed = anchor.get("chunkId") != chunk_id
    anchor["chunkId"] = chunk_id
    return changed


def null_chunk_ids(value: object) -> object:
    if isinstance(value, dict):
        out = {}
        for key, child in value.items():
            if key == "chunkId":
                out[key] = None
            else:
                out[key] = null_chunk_ids(child)
        return out
    if isinstance(value, list):
        return [null_chunk_ids(item) for item in value]
    return value


def semantic_review_bytes(rows: list[dict], fieldnames: list[str]) -> bytes:
    lines = ["\t".join(fieldnames)]
    for row in rows:
        values = []
        for name in fieldnames:
            value = row.get(name, "")
            if name in {"citations", "version_context", "conflict_context", "judgments"}:
                parsed = json.loads(value) if value else ({} if name != "citations" else [])
                value = json.dumps(
                    null_chunk_ids(parsed),
                    ensure_ascii=False,
                    separators=(",", ":"),
                    sort_keys=True,
                )
            values.append(value)
        lines.append("\t".join(values))
    return ("\n".join(lines) + "\n").encode("utf-8")


def write_tsv(path: Path, fieldnames: list[str], rows: list[dict]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=fieldnames,
            delimiter="\t",
            lineterminator="\n",
            extrasaction="ignore",
        )
        writer.writeheader()
        for row in rows:
            writer.writerow(row)


def fill_queries(catalog: dict[str, list[dict]]) -> tuple[list[str], list[dict], int]:
    with QUERIES.open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        fieldnames = list(reader.fieldnames or [])
        rows = list(reader)
    changed = 0
    for row in rows:
        cites = json.loads(row.get("citations") or "[]")
        for index, cite in enumerate(cites):
            if annotate_anchor(
                catalog,
                cite,
                f"{row['query_id']}/citations[{index}]",
            ):
                changed += 1
        row["citations"] = json.dumps(cites, ensure_ascii=False, separators=(",", ":"))
    write_tsv(QUERIES, fieldnames, rows)
    return fieldnames, rows, changed


def fill_conflicts(catalog: dict[str, list[dict]]) -> int:
    payload = json.loads(CONFLICTS.read_text(encoding="utf-8"))
    changed = 0

    def walk(node: object, label: str) -> None:
        nonlocal changed
        if isinstance(node, dict):
            if annotate_anchor(catalog, node, label):
                changed += 1
            for key, child in node.items():
                walk(child, f"{label}.{key}")
        elif isinstance(node, list):
            for index, child in enumerate(node):
                walk(child, f"{label}[{index}]")

    walk(payload, "conflicts")
    CONFLICTS.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return changed


def row_semantic(row: dict) -> dict:
    out = dict(row)
    for name in ("citations", "version_context", "conflict_context", "judgments"):
        raw = out.get(name) or ("[]" if name == "citations" else "{}")
        out[name] = null_chunk_ids(json.loads(raw))
    return out


def refresh_review_and_adjudication(
    fieldnames: list[str],
    query_rows: list[dict],
) -> None:
    adjudication = json.loads(ADJUDICATION.read_text(encoding="utf-8"))
    sample_ids = adjudication["sampleQueryIds"]
    by_id = {row["query_id"]: row for row in query_rows}
    review_rows = [by_id[query_id] for query_id in sample_ids]

    with REVIEW.open(encoding="utf-8", newline="") as handle:
        previous_rows = list(csv.DictReader(handle, delimiter="\t"))
    previous_by_id = {row["query_id"]: row for row in previous_rows}
    for query_id in sample_ids:
        previous = previous_by_id.get(query_id)
        if previous is None:
            raise SystemExit(f"review sample missing {query_id}")
        if row_semantic(previous) != row_semantic(review_rows[sample_ids.index(query_id)]):
            raise SystemExit(
                f"semantic review packet changed beyond chunkId annotation ({query_id}); "
                "re-adjudication required"
            )

    write_tsv(REVIEW, fieldnames, review_rows)
    full_sha = hashlib.sha256(REVIEW.read_bytes()).hexdigest()
    semantic_sha = hashlib.sha256(semantic_review_bytes(review_rows, fieldnames)).hexdigest()
    # Preserve prior semantic pin when present; otherwise pin the normalized form now.
    adjudication["sampleSemanticSha256"] = (
        adjudication.get("sampleSemanticSha256") or semantic_sha
    )
    if adjudication["sampleSemanticSha256"] != semantic_sha:
        raise SystemExit(
            "sampleSemanticSha256 drifted; re-adjudication required"
        )
    adjudication["sampleSha256"] = full_sha
    adjudication["mechanicalAnnotations"] = [
        {
            "id": "chunkId-from-expected-chunks-v1",
            "description": (
                "Filled citation.chunkId from retrieval/expected-chunks.tsv by unique "
                "span coverage. Query/answer/span/judgment content unchanged."
            ),
        }
    ]
    ADJUDICATION.write_text(
        json.dumps(adjudication, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument(
        "--corpus",
        type=Path,
        default=DEFAULT_CORPUS,
        help="Corpus root (default: bench/markhand_web)",
    )
    args = parser.parse_args()
    configure_corpus(args.corpus)
    catalog = load_catalog()

    if args.check:
        with QUERIES.open(encoding="utf-8", newline="") as handle:
            missing = 0
            total = 0
            for row in csv.DictReader(handle, delimiter="\t"):
                for cite in json.loads(row.get("citations") or "[]"):
                    total += 1
                    if not cite.get("chunkId"):
                        missing += 1
        if missing:
            print(f"chunkId missing on {missing}/{total} query citations", file=sys.stderr)
            return 1
        print(f"chunkId present on all {total} query citations")
        return 0

    fieldnames, query_rows, query_changed = fill_queries(catalog)
    conflict_changed = fill_conflicts(catalog)
    refresh_review_and_adjudication(fieldnames, query_rows)
    print(
        f"filled chunkIds: queries_changed={query_changed} "
        f"conflicts_changed={conflict_changed}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
