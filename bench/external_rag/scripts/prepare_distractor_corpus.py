#!/usr/bin/env python3
"""Extend the locked 50-document pilot with 150 chronological distractors."""

from __future__ import annotations

import concurrent.futures
import http.cookiejar
import json
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path

import run_pilot as pilot


TARGET_DOCUMENTS = 200
OUTPUT_LOCK = pilot.BENCH_DIR / "sources-200.lock.json"
POSTBACK_TARGET = "ctrl_191017_163$grvDocument"


class ListingParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.hrefs: list[str] = []
        self.hidden: dict[str, str] = {}

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        values = dict(attrs)
        if tag.lower() == "a" and values.get("href"):
            self.hrefs.append(values["href"] or "")
        if (
            tag.lower() == "input"
            and (values.get("type") or "").lower() == "hidden"
            and values.get("name")
        ):
            self.hidden[values["name"] or ""] = values.get("value") or ""


def listing_urls(parser: ListingParser) -> list[str]:
    urls: list[str] = []
    seen: set[str] = set()
    for href in parser.hrefs:
        absolute = urllib.parse.urljoin(pilot.DETAIL_ORIGIN, href)
        parsed = urllib.parse.urlparse(absolute)
        query = urllib.parse.parse_qs(parsed.query)
        if parsed.hostname != urllib.parse.urlparse(pilot.DETAIL_ORIGIN).hostname:
            continue
        if not (query.get("docid") or [""])[0].isdigit():
            continue
        canonical = urllib.parse.urlunparse(
            (parsed.scheme, parsed.netloc, parsed.path or "/", "", parsed.query, "")
        )
        doc_id = pilot.document_id(canonical)
        if doc_id not in seen:
            seen.add(doc_id)
            urls.append(canonical)
    return urls


def discover_distractors(existing_ids: set[str], count: int) -> list[str]:
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    request = urllib.request.Request(
        pilot.LISTING_URL, headers={"User-Agent": pilot.USER_AGENT}
    )
    payload = opener.open(request, timeout=60).read()
    parser = ListingParser()
    parser.feed(payload.decode("utf-8", errors="replace"))
    candidates: list[str] = []
    page = 1
    while len(candidates) < count:
        for url in listing_urls(parser):
            if pilot.document_id(url) not in existing_ids and url not in candidates:
                candidates.append(url)
                if len(candidates) == count:
                    return candidates
        page += 1
        if page > 20:
            break
        form = dict(parser.hidden)
        form["__EVENTTARGET"] = POSTBACK_TARGET
        form["__EVENTARGUMENT"] = f"Page${page}"
        request = urllib.request.Request(
            pilot.LISTING_URL,
            data=urllib.parse.urlencode(form).encode(),
            headers={
                "Content-Type": "application/x-www-form-urlencoded",
                "User-Agent": pilot.USER_AGENT,
            },
        )
        payload = opener.open(request, timeout=60).read()
        parser = ListingParser()
        parser.feed(payload.decode("utf-8", errors="replace"))
    raise RuntimeError(f"found only {len(candidates)} of {count} distractors")


def main() -> int:
    base = pilot.load_lock()
    sources = list(base["sources"])
    needed = TARGET_DOCUMENTS - len(sources)
    if needed <= 0:
        raise RuntimeError("base lock must contain fewer than 200 sources")
    candidate_count = needed + 50
    urls = discover_distractors(
        {source["id"] for source in sources}, candidate_count
    )
    rows_by_url: dict[str, tuple[dict, bytes]] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        futures = {executor.submit(pilot.inspect_source, url): url for url in urls}
        for future in concurrent.futures.as_completed(futures):
            url = futures[future]
            try:
                source, payload = future.result()
            except RuntimeError as error:
                print(f"skipped {url}: {error}")
                continue
            rows_by_url[url] = (source, payload)
            (pilot.ORIGINALS_DIR / source["filename"]).write_bytes(payload)
            print(f"inspected {len(rows_by_url):03d}/{candidate_count} {url}")
    successful_urls = [url for url in urls if url in rows_by_url][:needed]
    if len(successful_urls) != needed:
        raise RuntimeError(
            f"found only {len(successful_urls)} usable distractors; expected {needed}"
        )
    for url in successful_urls:
        source, payload = rows_by_url[url]
        sources.append(source)
    lock = {
        "schemaVersion": 1,
        "kind": "external-public-document-distractor-pilot",
        "source": pilot.LISTING_URL,
        "generatedAt": datetime.now(timezone.utc).isoformat(),
        "documents": len(sources),
        "targetDocuments": len(base["sources"]),
        "distractorDocuments": needed,
        "rawFilesCommitted": False,
        "licenseNote": base["licenseNote"],
        "sources": sources,
    }
    OUTPUT_LOCK.write_text(
        json.dumps(lock, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {OUTPUT_LOCK.relative_to(pilot.ROOT)} with {len(sources)} sources")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
