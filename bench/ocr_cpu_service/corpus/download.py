"""Validated, bounded acquisition of benchmark corpus files."""

from __future__ import annotations

import hashlib
import ipaddress
import json
import os
import re
import socket
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

_SOURCE_FIELDS = (
    "id",
    "url",
    "license",
    "sha256",
    "max_bytes",
    "kind",
    "classification",
)
_CHUNK_BYTES = 64 * 1024
_DOWNLOAD_TIMEOUT_SECONDS = 30.0
_APPROVED_HOSTS = frozenset(
    {
        "cas-bridge.xethub.hf.co",
        "cdn-lfs-us-1.hf.co",
        "datafiles.chinhphu.vn",
        "huggingface.co",
        "upload.wikimedia.org",
        "us.aws.cdn.hf.co",
    }
)


@dataclass(frozen=True)
class CorpusSource:
    id: str
    url: str
    license: str
    sha256: str
    max_bytes: int
    kind: str
    classification: str


@dataclass(frozen=True)
class DownloadedSource:
    source: CorpusSource
    path: Path
    bytes_downloaded: int


class _ValidatedRedirectHandler(HTTPRedirectHandler):
    def redirect_request(
        self,
        req: Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> Request | None:
        absolute_url = urljoin(req.full_url, newurl)
        _validate_download_url(absolute_url)
        return super().redirect_request(req, fp, code, msg, headers, absolute_url)


def urlopen(request: Request, timeout: float) -> Any:
    return build_opener(_ValidatedRedirectHandler()).open(request, timeout=timeout)


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
    if parsed.hostname not in _APPROVED_HOSTS:
        raise ValueError(f"{source.id}: URL must use an approved host")
    if parsed.port not in (None, 443):
        raise ValueError(f"{source.id}: HTTPS URL must use port 443")

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
    if source.classification not in {
        "metadata",
        "mixed",
        "native",
        "scan",
        "synthetic-scan",
    }:
        raise ValueError(f"{source.id}: classification is invalid")


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
        classification=item["classification"],
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

    _validate_download_url(source.url)
    with urlopen(request, timeout=_DOWNLOAD_TIMEOUT_SECONDS) as response:
        final_url = response.geturl()
        _validate_download_url(final_url)

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


def _validate_download_url(url: str) -> None:
    parsed = urlsplit(url)
    if parsed.scheme != "https":
        raise ValueError("download redirected to a non-HTTPS URL")
    if parsed.hostname not in _APPROVED_HOSTS:
        raise ValueError("download URL must use an approved host")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("download URL credentials are not allowed")
    if parsed.port not in (None, 443):
        raise ValueError("download HTTPS URL must use port 443")

    assert parsed.hostname is not None
    try:
        addresses = socket.getaddrinfo(
            parsed.hostname,
            443,
            type=socket.SOCK_STREAM,
        )
    except OSError as error:
        raise ValueError(f"download host could not be resolved: {parsed.hostname}") from error
    if not addresses:
        raise ValueError(f"download host could not be resolved: {parsed.hostname}")

    for address in addresses:
        ip = ipaddress.ip_address(address[4][0])
        if not ip.is_global:
            raise ValueError(
                f"download host must resolve only to public addresses: {parsed.hostname}"
            )
