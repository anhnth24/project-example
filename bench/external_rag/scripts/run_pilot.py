#!/usr/bin/env python3
"""Run a non-gating RAG pilot over 50 public Vietnamese government documents.

The raw files and converted Markdown live under ``bench/corpus_external`` and
are intentionally gitignored. The committed source lock contains provenance
and content hashes, but does not redistribute the documents.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import html
import json
import re
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
BENCH_DIR = ROOT / "bench/external_rag"
LOCK_PATH = BENCH_DIR / "sources.lock.json"
WORK_DIR = ROOT / "bench/corpus_external"
ORIGINALS_DIR = WORK_DIR / "originals"
MARKDOWN_DIR = WORK_DIR / "markdown"
RESULTS_DIR = WORK_DIR / "results"
SUMMARY_PATH = RESULTS_DIR / "summary.json"
REPORT_PATH = BENCH_DIR / "reports/pilot.md"
LISTING_URL = (
    "https://vanban.chinhphu.vn/"
    "he-thong-van-ban?classid=1&mode=1&typegroupid=4"
)
DETAIL_ORIGIN = "https://vanban.chinhphu.vn"
ALLOWED_ATTACHMENT_HOST = "datafiles.chinhphu.vn"
EXPECTED_DOCUMENTS = 50
MAX_DOWNLOAD_BYTES = 100 * 1024 * 1024
SUPPORTED_EXTENSIONS = {
    ".pdf",
    ".docx",
    ".pptx",
    ".xlsx",
    ".csv",
    ".html",
    ".htm",
    ".txt",
    ".png",
    ".jpg",
    ".jpeg",
    ".tif",
    ".tiff",
}
USER_AGENT = "MarkhandExternalRagPilot/1.0 (+benchmark; public-documents)"

SCRIPTS_DIR = ROOT / "bench/markhand_web/scripts"
sys.path.insert(0, str(SCRIPTS_DIR))
from run_embedding_eval import chunk_markdown  # noqa: E402
from run_retrieval_eval import (  # noqa: E402
    aggregate_docs,
    build_fts_index,
    embed_neural,
    evaluate_docs,
    score_chunks_hybrid,
    score_chunks_lexical,
    score_chunks_vector,
    summarize,
)


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.hrefs: list[str] = []
        self._in_title = False
        self._title_parts: list[str] = []

    @property
    def title(self) -> str:
        return re.sub(r"\s+", " ", " ".join(self._title_parts)).strip()

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if tag.lower() == "a" and values.get("href"):
            self.hrefs.append(html.unescape(values["href"] or ""))
        if tag.lower() == "title":
            self._in_title = True

    def handle_endtag(self, tag: str) -> None:
        if tag.lower() == "title":
            self._in_title = False

    def handle_data(self, data: str) -> None:
        if self._in_title:
            self._title_parts.append(data)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def request_bytes(url: str, *, attempts: int = 4) -> tuple[bytes, dict[str, str]]:
    error: Exception | None = None
    for attempt in range(attempts):
        request = urllib.request.Request(
            url,
            headers={
                "User-Agent": USER_AGENT,
                "Accept": "*/*",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                length = response.headers.get("Content-Length")
                if length and int(length) > MAX_DOWNLOAD_BYTES:
                    raise RuntimeError(f"download too large: {length} bytes: {url}")
                payload = response.read(MAX_DOWNLOAD_BYTES + 1)
                if len(payload) > MAX_DOWNLOAD_BYTES:
                    raise RuntimeError(f"download exceeded size cap: {url}")
                headers = {
                    "contentType": response.headers.get("Content-Type", ""),
                    "etag": response.headers.get("ETag", ""),
                    "lastModified": response.headers.get("Last-Modified", ""),
                }
                return payload, headers
        except (OSError, urllib.error.URLError, RuntimeError) as caught:
            error = caught
            if attempt + 1 < attempts:
                time.sleep(2**attempt)
    raise RuntimeError(f"failed to download {url}: {error}") from error


def parse_page(payload: bytes) -> PageParser:
    parser = PageParser()
    parser.feed(payload.decode("utf-8", errors="replace"))
    return parser


def document_id(detail_url: str) -> str:
    query = urllib.parse.parse_qs(urllib.parse.urlparse(detail_url).query)
    values = query.get("docid") or []
    if not values or not values[0].isdigit():
        raise ValueError(f"detail URL has no numeric docid: {detail_url}")
    return f"cp-{values[0]}"


def extension_for(url: str) -> str:
    return Path(urllib.parse.urlparse(url).path).suffix.lower()


def discover_detail_urls() -> list[str]:
    payload, _ = request_bytes(LISTING_URL)
    parser = parse_page(payload)
    detail_urls: list[str] = []
    seen: set[str] = set()
    for href in parser.hrefs:
        absolute = urllib.parse.urljoin(DETAIL_ORIGIN, href)
        parsed = urllib.parse.urlparse(absolute)
        query = urllib.parse.parse_qs(parsed.query)
        if parsed.netloc != urllib.parse.urlparse(DETAIL_ORIGIN).netloc:
            continue
        if not (query.get("docid") or [""])[0].isdigit():
            continue
        canonical = urllib.parse.urlunparse(
            (parsed.scheme, parsed.netloc, parsed.path or "/", "", parsed.query, "")
        )
        doc_id = document_id(canonical)
        if doc_id not in seen:
            seen.add(doc_id)
            detail_urls.append(canonical)
        if len(detail_urls) == EXPECTED_DOCUMENTS:
            break
    if len(detail_urls) != EXPECTED_DOCUMENTS:
        raise RuntimeError(
            f"listing yielded {len(detail_urls)} documents; "
            f"expected {EXPECTED_DOCUMENTS}"
        )
    return detail_urls


def inspect_source(detail_url: str) -> tuple[dict, bytes]:
    page_payload, _ = request_bytes(detail_url)
    parser = parse_page(page_payload)
    candidates: list[str] = []
    for href in parser.hrefs:
        absolute = urllib.parse.urljoin(detail_url, href)
        parsed = urllib.parse.urlparse(absolute)
        if parsed.hostname != ALLOWED_ATTACHMENT_HOST:
            continue
        if extension_for(absolute) not in SUPPORTED_EXTENSIONS:
            continue
        if absolute not in candidates:
            candidates.append(absolute)
    if not candidates:
        raise RuntimeError(f"no supported official attachment: {detail_url}")
    candidates.sort(key=lambda url: (extension_for(url) != ".pdf", url))
    attachment_url = candidates[0]
    attachment, headers = request_bytes(attachment_url)
    doc_id = document_id(detail_url)
    extension = extension_for(attachment_url)
    if not parser.title:
        raise RuntimeError(f"missing document title: {detail_url}")
    return (
        {
            "id": doc_id,
            "title": parser.title,
            "detailUrl": detail_url,
            "attachmentUrl": attachment_url,
            "filename": f"{doc_id}{extension}",
            "sha256": sha256_bytes(attachment),
            "bytes": len(attachment),
            "contentType": headers["contentType"],
            "etag": headers["etag"],
            "lastModified": headers["lastModified"],
        },
        attachment,
    )


def refresh_sources() -> dict:
    detail_urls = discover_detail_urls()
    ORIGINALS_DIR.mkdir(parents=True, exist_ok=True)
    rows_by_url: dict[str, tuple[dict, bytes]] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
        futures = {executor.submit(inspect_source, url): url for url in detail_urls}
        for future in concurrent.futures.as_completed(futures):
            url = futures[future]
            rows_by_url[url] = future.result()
            print(f"inspected {len(rows_by_url):02d}/{len(detail_urls)} {url}")
    sources = []
    for detail_url in detail_urls:
        source, payload = rows_by_url[detail_url]
        destination = ORIGINALS_DIR / source["filename"]
        destination.write_bytes(payload)
        sources.append(source)
    lock = {
        "schemaVersion": 1,
        "kind": "external-public-document-pilot",
        "source": LISTING_URL,
        "generatedAt": utc_now(),
        "documents": len(sources),
        "rawFilesCommitted": False,
        "licenseNote": (
            "Officially published government documents; raw files are benchmark-only "
            "and are not redistributed by this repository."
        ),
        "sources": sources,
    }
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    LOCK_PATH.write_text(
        json.dumps(lock, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return lock


def load_lock() -> dict:
    if not LOCK_PATH.is_file():
        raise SystemExit(
            f"missing {LOCK_PATH.relative_to(ROOT)}; run with --refresh-sources"
        )
    lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    sources = lock.get("sources") or []
    if lock.get("documents") != EXPECTED_DOCUMENTS or len(sources) != EXPECTED_DOCUMENTS:
        raise SystemExit(
            f"source lock must contain exactly {EXPECTED_DOCUMENTS} documents"
        )
    ids = [source["id"] for source in sources]
    if len(set(ids)) != len(ids):
        raise SystemExit("source lock contains duplicate document IDs")
    return lock


def materialize_source(source: dict) -> Path:
    destination = ORIGINALS_DIR / source["filename"]
    if destination.is_file():
        payload = destination.read_bytes()
        if sha256_bytes(payload) == source["sha256"]:
            return destination
    payload, _ = request_bytes(source["attachmentUrl"])
    digest = sha256_bytes(payload)
    if digest != source["sha256"]:
        raise RuntimeError(
            f"upstream drift for {source['id']}: got {digest}, "
            f"expected {source['sha256']}"
        )
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(payload)
    return destination


def materialize_sources(sources: list[dict]) -> dict[str, Path]:
    paths: dict[str, Path] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
        futures = {
            executor.submit(materialize_source, source): source for source in sources
        }
        for future in concurrent.futures.as_completed(futures):
            source = futures[future]
            paths[source["id"]] = future.result()
            print(f"downloaded {len(paths):02d}/{len(sources)} {source['id']}")
    return paths


def convert_documents(
    sources: list[dict],
    source_paths: dict[str, Path],
    converter: Path,
) -> tuple[list[dict], list[dict]]:
    if not converter.is_file():
        raise SystemExit(
            f"converter not found: {converter}; run cargo build --release first"
        )
    MARKDOWN_DIR.mkdir(parents=True, exist_ok=True)
    conversions: list[dict] = []
    chunks: list[dict] = []
    for index, source in enumerate(sources, start=1):
        started = time.perf_counter()
        process = subprocess.run(
            [str(converter), "one", str(source_paths[source["id"]])],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=600,
            check=False,
        )
        elapsed_ms = round((time.perf_counter() - started) * 1000, 2)
        markdown = process.stdout
        markdown_path = MARKDOWN_DIR / f"{source['id']}.md"
        if process.returncode == 0:
            markdown_path.write_text(markdown, encoding="utf-8")
        doc_chunks = chunk_markdown(markdown) if process.returncode == 0 else []
        for chunk_index, chunk in enumerate(doc_chunks):
            chunk_id = f"{source['id']}#{chunk_index}"
            chunks.append(
                {
                    "chunkId": chunk_id,
                    "docId": source["id"],
                    "headingPath": chunk["heading"],
                    "body": chunk["text"],
                    "payload": f"{chunk['heading']}\n{chunk['text']}",
                }
            )
        conversions.append(
            {
                "documentId": source["id"],
                "filename": source["filename"],
                "success": process.returncode == 0,
                "returnCode": process.returncode,
                "elapsedMs": elapsed_ms,
                "markdownChars": len(markdown),
                "chunks": len(doc_chunks),
                "stderr": process.stderr[-1000:],
            }
        )
        print(
            f"converted {index:02d}/{len(sources)} {source['id']} "
            f"chars={len(markdown)} chunks={len(doc_chunks)}"
        )
    return conversions, chunks


def query_subject(title: str) -> str:
    subject = title.split(":", 1)[1].strip() if ":" in title else title
    subject = re.sub(r"\s+", " ", subject).rstrip(".")
    if subject:
        subject = subject[0].lower() + subject[1:]
    return f"Văn bản nào {subject}?"


def query_identifier(title: str) -> str:
    prefix = title.split(":", 1)[0].strip()
    prefix = re.sub(r"\s+của\s+.+$", "", prefix, flags=re.IGNORECASE)
    return f"Nội dung chính của {prefix} là gì?"


def build_queries(sources: list[dict]) -> list[dict]:
    queries: list[dict] = []
    for source in sources:
        for category, query in (
            ("official_subject", query_subject(source["title"])),
            ("identifier", query_identifier(source["title"])),
        ):
            queries.append(
                {
                    "query_id": f"{source['id']}-{category}",
                    "query": query,
                    "category": category,
                    "judgments": {source["id"]: 3},
                    "provenance": "official-detail-metadata-derived",
                }
            )
    return queries


def evaluate_leg(
    name: str,
    queries: list[dict],
    chunks: list[dict],
    query_vectors: dict[str, list[float]],
    fts_conn,
) -> tuple[dict, list[dict]]:
    rows: list[dict] = []
    for query in queries:
        if name == "lexical":
            scored = score_chunks_lexical(
                query["query"], chunks, fts_conn=fts_conn
            )
        elif name == "vector_neural":
            scored = score_chunks_vector(
                query_vectors[query["query_id"]], chunks
            )
        elif name == "hybrid":
            scored = score_chunks_hybrid(
                query["query"],
                query_vectors[query["query_id"]],
                chunks,
                fts_conn=fts_conn,
            )
        else:  # pragma: no cover
            raise ValueError(name)
        ranked = aggregate_docs(scored)
        metrics = evaluate_docs(ranked, query["judgments"])
        rows.append(
            {
                "queryId": query["query_id"],
                "category": query["category"],
                "query": query["query"],
                "rankedDocuments": ranked[:10],
                **metrics,
            }
        )
    overall = summarize(rows)
    overall["byCategory"] = {
        category: summarize([row for row in rows if row["category"] == category])
        for category in sorted({row["category"] for row in rows})
    }
    return overall, rows


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = round((len(ordered) - 1) * fraction)
    return ordered[index]


def write_report(summary: dict) -> None:
    quality = summary["conversion"]
    retrieval = summary["retrieval"]
    lines = [
        "# External Vietnamese document RAG pilot",
        "",
        f"- Generated: `{summary['generatedAt']}`",
        f"- Git commit: `{summary['gitCommit']}`",
        f"- Documents: `{summary['documents']}` official public files",
        f"- Converted: `{quality['successful']}/{summary['documents']}`",
        f"- Non-empty: `{quality['nonEmpty']}/{summary['documents']}`",
        f"- Chunks: `{summary['chunks']}`",
        f"- Queries: `{summary['queries']}` metadata-derived, document-level",
        f"- Embedding: `{summary['embedding']['runtime']}`",
        "",
        "> **Non-gating pilot.** Queries are derived from independently published",
        "> official detail metadata, not organic user traffic. This track validates",
        "> real conversion/indexing and exposes scale/noise, but does not establish",
        "> production semantic retrieval quality.",
        "",
        "## Conversion",
        "",
        "| Metric | Value |",
        "|---|---:|",
        f"| Success rate | {quality['successRate']:.4f} |",
        f"| Non-empty rate | {quality['nonEmptyRate']:.4f} |",
        f"| Median conversion ms | {quality['medianElapsedMs']:.2f} |",
        f"| P95 conversion ms | {quality['p95ElapsedMs']:.2f} |",
        f"| Median Markdown chars | {quality['medianMarkdownChars']:.0f} |",
        "",
        "## Retrieval legs — document level",
        "",
        "| Leg | Recall@5 | Recall@10 | Hit@5 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for name in ("lexical", "vector_neural", "hybrid"):
        row = retrieval[name]
        lines.append(
            f"| `{name}` | {row['recallAt5']:.4f} | {row['recallAt10']:.4f} "
            f"| {row['hitAt5']:.4f} | {row['mrr']:.4f} "
            f"| {row['ndcgAt10']:.4f} |"
        )
    lines.extend(
        [
            "",
            "## Hybrid by query category",
            "",
            "| Category | N | Recall@5 | MRR | nDCG@10 |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for category, row in retrieval["hybrid"]["byCategory"].items():
        lines.append(
            f"| `{category}` | {row['queries']} | {row['recallAt5']:.4f} "
            f"| {row['mrr']:.4f} | {row['ndcgAt10']:.4f} |"
        )
    lines.extend(
        [
            "",
            "## Interpretation limits",
            "",
            "- Fifty documents are still a small candidate pool; top-5 covers 10%.",
            "- The corpus is legal/government PDF-heavy, not representative of every format.",
            "- Relevance is document-level; page/chunk labels require manual adjudication.",
            "- Metadata-derived queries retain lexical overlap and must not replace a blind test set.",
            "- No-answer and answer-grounding quality are not scored by this pilot.",
            "",
        ]
    )
    REPORT_PATH.parent.mkdir(parents=True, exist_ok=True)
    REPORT_PATH.write_text("\n".join(lines), encoding="utf-8")


def git_commit() -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()


def run_pilot(limit: int | None, converter: Path) -> dict:
    lock = load_lock()
    sources = lock["sources"][:limit] if limit else lock["sources"]
    source_paths = materialize_sources(sources)
    conversions, chunks = convert_documents(sources, source_paths, converter)
    if not chunks:
        raise SystemExit("conversion produced zero chunks")
    queries = build_queries(sources)
    query_vectors, runtime, dimensions, family, revision = embed_neural(
        chunks, queries
    )
    fts_conn = build_fts_index(chunks)
    retrieval: dict[str, dict] = {}
    details: dict[str, list[dict]] = {}
    for name in ("lexical", "vector_neural", "hybrid"):
        retrieval[name], details[name] = evaluate_leg(
            name, queries, chunks, query_vectors, fts_conn
        )
    elapsed = [row["elapsedMs"] for row in conversions]
    markdown_chars = [row["markdownChars"] for row in conversions]
    successful = sum(1 for row in conversions if row["success"])
    non_empty = sum(
        1
        for row in conversions
        if row["success"] and row["markdownChars"] >= 80 and row["chunks"] > 0
    )
    summary = {
        "schemaVersion": 1,
        "kind": "external-public-document-pilot",
        "nonGating": True,
        "generatedAt": utc_now(),
        "gitCommit": git_commit(),
        "sourceLockSha256": sha256_bytes(LOCK_PATH.read_bytes()),
        "documents": len(sources),
        "chunks": len(chunks),
        "queries": len(queries),
        "conversion": {
            "successful": successful,
            "nonEmpty": non_empty,
            "successRate": successful / len(sources),
            "nonEmptyRate": non_empty / len(sources),
            "medianElapsedMs": statistics.median(elapsed),
            "p95ElapsedMs": percentile(elapsed, 0.95),
            "medianMarkdownChars": statistics.median(markdown_chars),
            "rows": conversions,
        },
        "embedding": {
            "runtime": runtime,
            "family": family,
            "revision": revision,
            "dimensions": dimensions,
        },
        "queryProvenance": "official-detail-metadata-derived",
        "retrieval": retrieval,
        "details": details,
    }
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    SUMMARY_PATH.write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    if not limit:
        write_report(summary)
    return summary


def self_test() -> None:
    parser = parse_page(
        b'<html><head><title> Nghi dinh 1 </title></head>'
        b'<body><a href="/?docid=123">x</a></body></html>'
    )
    assert parser.title == "Nghi dinh 1"
    assert parser.hrefs == ["/?docid=123"]
    assert document_id("https://example.test/?docid=123") == "cp-123"
    assert extension_for("https://example.test/a.PDF?q=1") == ".pdf"
    title = "Nghị định số 1/2026/NĐ-CP của Chính phủ: Quy định thử nghiệm"
    assert query_identifier(title) == "Nội dung chính của Nghị định số 1/2026/NĐ-CP là gì?"
    assert query_subject(title) == "Văn bản nào quy định thử nghiệm?"
    print("external RAG pilot self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--refresh-sources",
        action="store_true",
        help="discover and lock the latest 50 official documents",
    )
    parser.add_argument(
        "--limit",
        type=int,
        help="run only the first N locked documents (smoke only)",
    )
    parser.add_argument(
        "--converter",
        type=Path,
        default=ROOT / "target/release/fileconv",
    )
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.refresh_sources:
        lock = refresh_sources()
        print(f"wrote {LOCK_PATH.relative_to(ROOT)} with {lock['documents']} sources")
        return 0
    if args.limit is not None and not 1 <= args.limit <= EXPECTED_DOCUMENTS:
        raise SystemExit(f"--limit must be between 1 and {EXPECTED_DOCUMENTS}")
    summary = run_pilot(args.limit, args.converter)
    print(
        json.dumps(
            {
                "documents": summary["documents"],
                "chunks": summary["chunks"],
                "hybrid": summary["retrieval"]["hybrid"],
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
