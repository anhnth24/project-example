"""Validated, bounded acquisition of benchmark corpus files."""

from __future__ import annotations

import hashlib
import json
import os
import re
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

_SOURCE_FIELDS = ("id", "url", "license", "sha256", "max_bytes", "kind")
_CHUNK_BYTES = 64 * 1024


@dataclass(frozen=True)
class CorpusSource:
    id: str
    url: str
    license: str
    sha256: str
    max_bytes: int
    kind: str


@dataclass(frozen=True)
class DownloadedSource:
    source: CorpusSource
    path: Path
    bytes_downloaded: int


def validate_source(source: CorpusSource) -> None:
    if not isinstance(source.id, str) or not re.fullmatch(
        r"[a-z0-9][a-z0-9_-]*", source.id
    ):
        raise ValueError("source id must contain only lowercase letters, digits, _ or -")

    if not isinstance(source.url, str):
        raise ValueError(f"{source.id}: url must be a string")
    parsed = urlsplit(source.url)
    if parsed.scheme != "https" or not parsed.netloc:
        raise ValueError(f"{source.id}: only HTTPS sources are allowed")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError(f"{source.id}: URL credentials are not allowed")

    if not isinstance(source.license, str) or not source.license.strip():
        raise ValueError(f"{source.id}: license is required")
    if (
        not isinstance(source.max_bytes, int)
        or isinstance(source.max_bytes, bool)
        or source.max_bytes <= 0
    ):
        raise ValueError(f"{source.id}: max_bytes must be positive")
    if not isinstance(source.sha256, str) or not re.fullmatch(
        r"[0-9a-f]{64}", source.sha256
    ):
        raise ValueError(f"{source.id}: sha256 must be lowercase hex")
    if not isinstance(source.kind, str) or not source.kind.strip():
        raise ValueError(f"{source.id}: kind is required")


def load_sources(path: Path) -> list[CorpusSource]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: invalid source manifest: {error}") from error

    if not isinstance(raw, list):
        raise ValueError(f"{path}: source manifest must be a JSON array")

    sources: list[CorpusSource] = []
    seen_ids: set[str] = set()
    expected_fields = set(_SOURCE_FIELDS)
    for index, item in enumerate(raw):
        if not isinstance(item, dict):
            raise ValueError(f"{path}: source {index} must be an object")

        missing = [field for field in _SOURCE_FIELDS if field not in item]
        if missing:
            raise ValueError(f"{path}: source {index} missing required field {missing[0]}")
        unexpected = sorted(set(item) - expected_fields)
        if unexpected:
            raise ValueError(
                f"{path}: source {index} has unexpected fields: {', '.join(unexpected)}"
            )

        source = _source_from_mapping(item)
        validate_source(source)
        if source.id in seen_ids:
            raise ValueError(f"{path}: duplicate source id {source.id}")
        seen_ids.add(source.id)
        sources.append(source)

    return sources


def download_sources(manifest: Path, output: Path) -> list[DownloadedSource]:
    sources = load_sources(manifest)
    output.mkdir(parents=True, exist_ok=True)
    downloaded: list[DownloadedSource] = []

    for source in sources:
        destination = output / _destination_name(source)
        temporary_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="xb", prefix=f".{source.id}-", suffix=".tmp", dir=output, delete=False
            ) as temporary:
                temporary_path = Path(temporary.name)
                bytes_downloaded = _stream_source(source, temporary)
                temporary.flush()
                os.fsync(temporary.fileno())

            os.replace(temporary_path, destination)
            temporary_path = None
        finally:
            if temporary_path is not None:
                temporary_path.unlink(missing_ok=True)

        downloaded.append(
            DownloadedSource(
                source=source,
                path=destination,
                bytes_downloaded=bytes_downloaded,
            )
        )

    return downloaded


def _source_from_mapping(item: dict[str, Any]) -> CorpusSource:
    return CorpusSource(
        id=item["id"],
        url=item["url"],
        license=item["license"],
        sha256=item["sha256"],
        max_bytes=item["max_bytes"],
        kind=item["kind"],
    )


def _destination_name(source: CorpusSource) -> str:
    remote_name = PurePosixPath(urlsplit(source.url).path).name
    suffixes = "".join(Path(remote_name).suffixes)
    return f"{source.id}{suffixes}"


def _stream_source(source: CorpusSource, destination: Any) -> int:
    request = Request(
        source.url,
        headers={"User-Agent": "Markhand-OCR-corpus-downloader/1"},
    )
    digest = hashlib.sha256()
    bytes_downloaded = 0

    with urlopen(request) as response:
        final_url = response.geturl()
        if urlsplit(final_url).scheme != "https":
            raise ValueError(f"{source.id}: download redirected to a non-HTTPS URL")

        content_length = response.headers.get("Content-Length")
        if content_length is not None:
            try:
                declared_bytes = int(content_length)
            except ValueError as error:
                raise ValueError(
                    f"{source.id}: invalid Content-Length {content_length!r}"
                ) from error
            if declared_bytes < 0 or declared_bytes > source.max_bytes:
                raise ValueError(
                    f"{source.id}: Content-Length exceeds max_bytes"
                )

        while chunk := response.read(_CHUNK_BYTES):
            bytes_downloaded += len(chunk)
            if bytes_downloaded > source.max_bytes:
                raise ValueError(f"{source.id}: streamed content exceeds max_bytes")
            digest.update(chunk)
            destination.write(chunk)

    actual_sha256 = digest.hexdigest()
    if actual_sha256 != source.sha256:
        raise ValueError(
            f"{source.id}: SHA-256 mismatch: expected {source.sha256}, "
            f"received {actual_sha256}"
        )
    return bytes_downloaded
