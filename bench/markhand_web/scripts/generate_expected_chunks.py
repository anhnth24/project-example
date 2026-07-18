#!/usr/bin/env python3
"""Generate retrieval/expected-chunks.tsv for heading-chunks-2000-v1 (P0-06)."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MD_DIR = CORPUS / "golden/markdown"
QUERIES = CORPUS / "golden/queries.tsv"
OUT_TSV = CORPUS / "retrieval/expected-chunks.tsv"
OUT_META = CORPUS / "retrieval/expected-chunks.meta.json"
MAX_CHARS = 2000

sys.path.insert(0, str(Path(__file__).resolve().parent))
from knowledge_identity import (  # noqa: E402
    BODY_TEXT_VERSION,
    DEFAULT_CHUNKING_VERSION,
    chunk_identity,
    document_identity,
)


def version_id_for(doc_stem: str) -> str:
    special = {
        "gold-budget-v1": "version-budget-v1",
        "gold-budget-v2": "version-budget-v2",
        "gold-design-v1": "version-design-v1",
        "gold-design-v2": "version-design-v2",
    }
    return special.get(doc_stem, f"version-{doc_stem}-v1")


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


def locate_chunk_bytes(md: str, chunk_text: str, search_from: int) -> tuple[int, int, int]:
    """Return (start_byte, end_byte, next_search_char) for chunk_text in md."""
    idx = md.find(chunk_text, search_from)
    if idx < 0:
        idx = md.find(chunk_text)
        if idx < 0:
            raise ValueError(f"chunk text not found in markdown: {chunk_text[:60]!r}")
    start_b = len(md[:idx].encode("utf-8"))
    end_b = start_b + len(chunk_text.encode("utf-8"))
    return start_b, end_b, idx + len(chunk_text)


def resolve_version_map() -> dict[str, str]:
    mapping: dict[str, str] = {}
    with QUERIES.open(encoding="utf-8") as handle:
        for row in csv.DictReader(handle, delimiter="\t"):
            for cite in json.loads(row.get("citations") or "[]"):
                doc = cite.get("documentId")
                ver = cite.get("versionId")
                if doc and ver:
                    mapping[doc] = ver
    for path in MD_DIR.glob("*.md"):
        mapping.setdefault(path.stem, version_id_for(path.stem))
    return mapping


def verify_citation_spans(chunks_by_doc: dict[str, list[dict]]) -> list[str]:
    errors = []
    with QUERIES.open(encoding="utf-8") as handle:
        for row in csv.DictReader(handle, delimiter="\t"):
            qid = row["query_id"]
            for cite in json.loads(row.get("citations") or "[]"):
                doc = cite.get("documentId")
                start = cite.get("start")
                end = cite.get("end")
                quote = cite.get("quote") or ""
                if doc is None or start is None or end is None:
                    continue
                covering = [
                    chunk
                    for chunk in chunks_by_doc.get(doc, [])
                    if chunk["start"] <= int(start) and int(end) <= chunk["end"]
                ]
                if not covering:
                    errors.append(
                        f"{qid}/{doc}: span [{start},{end}) not covered by any chunk"
                    )
                    continue
                md_bytes = (MD_DIR / f"{doc}.md").read_bytes()
                actual = md_bytes[int(start) : int(end)].decode("utf-8")
                if actual != quote:
                    errors.append(f"{qid}/{doc}: quote mismatch vs markdown bytes")
    return errors


def build_rows() -> tuple[list[dict], dict, list[str]]:
    version_map = resolve_version_map()
    rows: list[dict] = []
    chunks_by_doc: dict[str, list[dict]] = {}
    for path in sorted(MD_DIR.glob("*.md")):
        raw = path.read_bytes()
        md = raw.decode("utf-8")
        content_sha = hashlib.sha256(raw).hexdigest()
        source_rel = f"golden/markdown/{path.name}"
        doc_identity = document_identity(source_rel, content_sha)
        version_id = version_map[path.stem]
        located = []
        search_from = 0
        for ordinal, chunk in enumerate(chunk_markdown(md)):
            body = unicodedata.normalize("NFC", chunk["text"])
            start_b, end_b, search_from = locate_chunk_bytes(md, chunk["text"], search_from)
            cid = chunk_identity(
                doc_identity, version_id, ordinal, chunk["heading"], body
            )
            located.append(
                {
                    "heading": chunk["heading"],
                    "text": chunk["text"],
                    "start": start_b,
                    "end": end_b,
                    "chunkId": cid,
                    "ordinal": ordinal,
                }
            )
            rows.append(
                {
                    "documentId": path.stem,
                    "versionId": version_id,
                    "documentIdentity": doc_identity,
                    "ordinal": ordinal,
                    "chunkId": cid,
                    "headingPath": chunk["heading"],
                    "start": start_b,
                    "end": end_b,
                    "chars": len(body),
                    "contentSha256": hashlib.sha256(body.encode("utf-8")).hexdigest(),
                    "chunkingVersion": DEFAULT_CHUNKING_VERSION,
                    "bodyTextVersion": BODY_TEXT_VERSION,
                }
            )
        chunks_by_doc[path.stem] = located
    return rows, chunks_by_doc, verify_citation_spans(chunks_by_doc)


def render_tsv(rows: list[dict]) -> str:
    fieldnames = [
        "documentId",
        "versionId",
        "documentIdentity",
        "ordinal",
        "chunkId",
        "headingPath",
        "start",
        "end",
        "chars",
        "contentSha256",
        "chunkingVersion",
        "bodyTextVersion",
    ]
    lines = ["\t".join(fieldnames)]
    for row in rows:
        lines.append("\t".join(str(row[name]) for name in fieldnames))
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    rows, chunks_by_doc, span_errors = build_rows()
    if span_errors:
        print(f"span resolve errors: {len(span_errors)}", file=sys.stderr)
        for error in span_errors[:30]:
            print(f"  - {error}", file=sys.stderr)
        return 1
    text = render_tsv(rows)
    meta = {
        "chunkingVersion": DEFAULT_CHUNKING_VERSION,
        "bodyTextVersion": BODY_TEXT_VERSION,
        "documents": len(chunks_by_doc),
        "chunks": len(rows),
        "sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
        "citationSpansResolved": True,
    }
    if args.check:
        if not OUT_TSV.is_file():
            print("missing expected-chunks.tsv", file=sys.stderr)
            return 1
        if OUT_TSV.read_text(encoding="utf-8") != text:
            print("expected-chunks.tsv is stale; regenerate", file=sys.stderr)
            return 1
        print("expected-chunks.tsv up to date")
        return 0
    OUT_TSV.parent.mkdir(parents=True, exist_ok=True)
    OUT_TSV.write_text(text, encoding="utf-8")
    OUT_META.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {OUT_TSV} chunks={len(rows)} docs={len(chunks_by_doc)}")
    print(f"wrote {OUT_META}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
