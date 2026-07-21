#!/usr/bin/env python3
"""Download public sources and orchestrate the Rust external RAG pilot.

Conversion, chunking, SQLite FTS/vector indexing, and hybrid ranking all run in
the ``external_rag_pilot`` Rust example. Python is limited to HTTP acquisition,
source locking, process orchestration, and Markdown report rendering.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import html
import json
import re
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
BENCH_DIR = ROOT / "bench/external_rag"
LOCK_PATH = BENCH_DIR / "sources.lock.json"
WORK_DIR = ROOT / "bench/corpus_external"
ORIGINALS_DIR = WORK_DIR / "originals"
RESULTS_DIR = WORK_DIR / "results"
SUMMARY_PATH = RESULTS_DIR / "summary.json"
REPORT_PATH = BENCH_DIR / "reports/pilot.md"
BLIND_SUMMARY_PATH = RESULTS_DIR / "summary-blind.json"
BLIND_REPORT_PATH = BENCH_DIR / "reports/pilot-blind.md"
DISTRACTOR_SUMMARY_PATH = RESULTS_DIR / "summary-blind-200.json"
DISTRACTOR_REPORT_PATH = BENCH_DIR / "reports/pilot-blind-200.md"
DEFAULT_RUNNER = ROOT / "target/release/examples/external_rag_pilot"
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
            headers={"User-Agent": USER_AGENT, "Accept": "*/*"},
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                length = response.headers.get("Content-Length")
                if length and int(length) > MAX_DOWNLOAD_BYTES:
                    raise RuntimeError(f"download too large: {length} bytes: {url}")
                payload = response.read(MAX_DOWNLOAD_BYTES + 1)
                if len(payload) > MAX_DOWNLOAD_BYTES:
                    raise RuntimeError(f"download exceeded size cap: {url}")
                return payload, {
                    "contentType": response.headers.get("Content-Type", ""),
                    "etag": response.headers.get("ETag", ""),
                    "lastModified": response.headers.get("Last-Modified", ""),
                }
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
    for attempt in range(4):
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
                return detail_urls
        if attempt < 3:
            time.sleep(2**attempt)
    raise RuntimeError(
        f"listing yielded {len(detail_urls)} documents; expected {EXPECTED_DOCUMENTS}"
    )


def inspect_source(detail_url: str) -> tuple[dict, bytes]:
    for attempt in range(4):
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
        if candidates and parser.title:
            candidates.sort(key=lambda url: (extension_for(url) != ".pdf", url))
            attachment_url = candidates[0]
            attachment, headers = request_bytes(attachment_url)
            doc_id = document_id(detail_url)
            extension = extension_for(attachment_url)
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
        if attempt < 3:
            time.sleep(2**attempt)
    raise RuntimeError(f"no supported official attachment: {detail_url}")


def refresh_sources() -> dict:
    detail_urls = discover_detail_urls()
    ORIGINALS_DIR.mkdir(parents=True, exist_ok=True)
    rows_by_url: dict[str, tuple[dict, bytes]] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        futures = {executor.submit(inspect_source, url): url for url in detail_urls}
        for future in concurrent.futures.as_completed(futures):
            url = futures[future]
            rows_by_url[url] = future.result()
            print(f"inspected {len(rows_by_url):02d}/{len(detail_urls)} {url}")
    sources = []
    for detail_url in detail_urls:
        source, payload = rows_by_url[detail_url]
        (ORIGINALS_DIR / source["filename"]).write_bytes(payload)
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


def load_lock(path: Path = LOCK_PATH) -> dict:
    if not path.is_file():
        raise SystemExit(
            f"missing {path.relative_to(ROOT)}; run with --refresh-sources"
        )
    lock = json.loads(path.read_text(encoding="utf-8"))
    sources = lock.get("sources") or []
    if not sources or lock.get("documents") != len(sources):
        raise SystemExit("source lock document count does not match its sources")
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


def materialize_sources(sources: list[dict]) -> None:
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        futures = {
            executor.submit(materialize_source, source): source for source in sources
        }
        completed = 0
        for future in concurrent.futures.as_completed(futures):
            source = futures[future]
            future.result()
            completed += 1
            print(f"downloaded {completed:02d}/{len(sources)} {source['id']}")


def validate_query_set(path: Path) -> list[dict]:
    rows = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(rows, list) or len(rows) != EXPECTED_DOCUMENTS * 2:
        raise ValueError("blind query set must contain exactly 100 rows")
    expected_ids = [f"blind-{index:03d}" for index in range(1, len(rows) + 1)]
    if [row.get("question_id") for row in rows] != expected_ids:
        raise ValueError("blind query IDs must be ordered blind-001 through blind-100")
    topics = Counter(row.get("topic_index") for row in rows)
    if topics != Counter({index: 2 for index in range(1, EXPECTED_DOCUMENTS + 1)}):
        raise ValueError("blind query set must contain exactly two rows per topic")
    for row in rows:
        if not isinstance(row.get("question"), str) or not row["question"].strip():
            raise ValueError(f"{row['question_id']} has an empty question")
        if not isinstance(row.get("intent"), str) or not row["intent"].strip():
            raise ValueError(f"{row['question_id']} has an empty intent")
        if re.search(r"\b\d{2,4}/\d{4}\b|NĐ-?CP", row["question"], re.IGNORECASE):
            raise ValueError(f"{row['question_id']} leaks a document identifier")
    return rows


def run_rust_pilot(
    runner: Path,
    limit: int | None,
    queries: Path | None = None,
    lock_path: Path = LOCK_PATH,
) -> dict:
    if not runner.is_file():
        raise SystemExit(
            f"Rust runner not found: {runner}\n"
            "build it with: cargo build --release -p fileconv-knowledge "
            "--features external-rag-pilot --example external_rag_pilot"
        )
    lock = load_lock(lock_path)
    if queries:
        validate_query_set(queries)
    sources = lock["sources"][:limit] if limit else lock["sources"]
    materialize_sources(sources)
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    distractor_run = len(sources) > EXPECTED_DOCUMENTS
    summary_path = (
        DISTRACTOR_SUMMARY_PATH
        if queries and distractor_run
        else BLIND_SUMMARY_PATH
        if queries
        else SUMMARY_PATH
    )
    command = [
        str(runner),
        str(lock_path),
        str(ORIGINALS_DIR),
        str(WORK_DIR),
        str(summary_path),
        str(queries.resolve()) if queries else "-",
    ]
    if limit:
        command.append(str(limit))
    subprocess.run(command, cwd=ROOT, check=True)
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    if not limit:
        write_report(
            summary,
            DISTRACTOR_REPORT_PATH
            if queries and distractor_run
            else BLIND_REPORT_PATH
            if queries
            else REPORT_PATH,
        )
    return summary


def write_report(summary: dict, report_path: Path = REPORT_PATH) -> None:
    conversion = summary["conversion"]
    retrieval = summary["retrieval"]
    overall = retrieval["overall"]
    embedding = summary["embedding"]
    provenance = retrieval["queryProvenance"]
    blind = provenance == "independent-agent-overview-only"
    lines = [
        (
            "# External Vietnamese document blind-query RAG pilot"
            if blind
            else "# External Vietnamese document RAG pilot"
        ),
        "",
        f"- Documents: `{summary['documents']}` official public files",
        f"- Converted: `{conversion['successful']}/{summary['documents']}`",
        f"- Non-empty: `{conversion['nonEmpty']}/{summary['documents']}`",
        f"- Reused converted Markdown: `{conversion.get('cached', 0)}`",
        f"- Production chunks: `{summary['chunks']}`",
        f"- Queries: `{summary['queries']}` (`{provenance}`)",
        f"- Embedding: `{embedding['model']}@{embedding['revision'][:12]}`",
        f"- Runtime path: `{embedding['runtimePath']}`",
        f"- Ranking path: `{retrieval['rankingPath']}`",
        "",
        "> **Non-gating pilot.** Conversion, chunking, SQLite indexing, neural",
        "> embedding calls, and hybrid ranking use the production Rust path.",
        (
            "> Questions were written by an independent agent given only one topic-level"
            if blind
            else "> Queries remain metadata-derived and do not establish production semantic quality."
        ),
        (
            "> overview per document; it saw no titles, identifiers, source text, chunks, or retrieval results."
            if blind
            else ""
        ),
        (
            f"> The first {EXPECTED_DOCUMENTS} documents are query targets; the remaining "
            f"{summary['documents'] - EXPECTED_DOCUMENTS} are chronological distractors."
            if blind and summary["documents"] > EXPECTED_DOCUMENTS
            else ""
        ),
        "",
        "## Conversion",
        "",
        "| Metric | Value |",
        "|---|---:|",
        f"| Success rate | {conversion['successRate']:.4f} |",
        f"| Non-empty rate | {conversion['nonEmptyRate']:.4f} |",
        f"| Median conversion ms | {conversion['medianElapsedMs']:.2f} |",
        f"| P95 conversion ms | {conversion['p95ElapsedMs']:.2f} |",
        f"| Median Markdown chars | {conversion['medianMarkdownChars']:.0f} |",
        "",
        "## Production hybrid retrieval",
        "",
        "| Scope | N | Recall@5 | Recall@10 | MRR | nDCG@10 |",
        "|---|---:|---:|---:|---:|---:|",
        f"| Overall | {overall['queries']} | {overall['recallAt5']:.4f} "
        f"| {overall['recallAt10']:.4f} | {overall['mrr']:.4f} "
        f"| {overall['ndcgAt10']:.4f} |",
    ]
    for category, row in retrieval["byCategory"].items():
        lines.append(
            f"| `{category}` | {row['queries']} | {row['recallAt5']:.4f} "
            f"| {row['recallAt10']:.4f} | {row['mrr']:.4f} "
            f"| {row['ndcgAt10']:.4f} |"
        )
    misses = [
        row for row in retrieval["rows"] if row["recallAt5"] == 0
    ]
    lines.extend(
        [
            "",
            "## Observed misses",
            "",
            f"- `{len(misses)}` queries missed the relevant document in top 5.",
            f"- `{sum(row['recallAt10'] == 0 for row in misses)}` remained absent from top 10.",
            (
                "- Misses are grouped below by independently assigned query intent."
                if blind
                else "- Every top-5 miss was an identifier query. Numeric document codes are"
            ),
            (
                ""
                if blind
                else "  split into common tokens, OCR can alter the code, and repeated chunks"
            ),
            "" if blind else "  from competing decrees can crowd the fixed chunk-level top-k.",
            "",
            "| Query | Relevant rank |",
            "|---|---:|",
        ]
    )
    for row in misses:
        rank = row["firstRelevantRank"] if row["firstRelevantRank"] is not None else ">10"
        lines.append(f"| `{row['queryId']}` | {rank} |")
    lines.extend(
        [
            "",
            "## Interpretation limits",
            "",
            (
                f"- The pool contains {summary['documents']} documents, but all targets remain in the first {EXPECTED_DOCUMENTS}."
                if summary["documents"] > EXPECTED_DOCUMENTS
                else "- Fifty documents remain a small candidate pool."
            ),
            "- Relevance is document-level over the production chunk ranking.",
            (
                "- Questions are overview-derived rather than written after reading source text."
                if blind
                else "- Metadata-derived queries retain lexical overlap."
            ),
            (
                "- Topic-to-document qrels are positional and do not prove that every requested detail is present."
                if blind
                else ""
            ),
            (
                "- Topic overviews remain discriminative in a 50-document corpus, so this score is still optimistic."
                if blind
                else ""
            ),
            "- Recall measures intended-document retrieval, not correct-chunk evidence or answer quality.",
            "- The corpus is government/legal-document heavy.",
            "- No-answer and answer-grounding quality are not scored.",
            "",
        ]
    )
    report_path.parent.mkdir(parents=True, exist_ok=True)
    rendered: list[str] = []
    for line in lines:
        if line or not rendered or rendered[-1]:
            rendered.append(line)
    report_path.write_text("\n".join(rendered), encoding="utf-8")


def self_test() -> None:
    parser = parse_page(
        b'<html><head><title> Nghi dinh 1 </title></head>'
        b'<body><a href="/?docid=123">x</a></body></html>'
    )
    assert parser.title == "Nghi dinh 1"
    assert parser.hrefs == ["/?docid=123"]
    assert document_id("https://example.test/?docid=123") == "cp-123"
    assert extension_for("https://example.test/a.PDF?q=1") == ".pdf"
    validate_query_set(BENCH_DIR / "blind_queries.json")
    print("external RAG acquisition self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--refresh-sources", action="store_true")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--runner", type=Path, default=DEFAULT_RUNNER)
    parser.add_argument("--queries", type=Path)
    parser.add_argument("--lock", type=Path, default=LOCK_PATH)
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
    lock = load_lock(args.lock)
    if args.limit is not None and not 1 <= args.limit <= lock["documents"]:
        raise SystemExit(f"--limit must be between 1 and {lock['documents']}")
    if args.queries and not args.queries.is_file():
        raise SystemExit(f"query set not found: {args.queries}")
    summary = run_rust_pilot(args.runner, args.limit, args.queries, args.lock)
    print(
        json.dumps(
            {
                "documents": summary["documents"],
                "chunks": summary["chunks"],
                "retrieval": summary["retrieval"]["overall"],
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
