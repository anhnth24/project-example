"""Validated, bounded acquisition of benchmark corpus files."""

from __future__ import annotations

import hashlib
import http.client
import ipaddress
import json
import os
import queue
import re
import socket
import ssl
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urljoin, urlsplit
from urllib.request import Request

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
_DOWNLOAD_TOTAL_DEADLINE_SECONDS = 120.0
_monotonic = time.monotonic
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


class _PinnedHTTPSConnection(http.client.HTTPSConnection):
    """Connect to one validated IP while retaining hostname TLS verification."""

    def __init__(
        self, host: str, addresses: tuple[str, ...], timeout: float
    ) -> None:
        super().__init__(
            host,
            port=443,
            timeout=timeout,
            context=ssl.create_default_context(),
        )
        self._validated_addresses = addresses

    def connect(self) -> None:
        last_error: OSError | None = None
        for address in self._validated_addresses:
            raw_socket: socket.socket | None = None
            try:
                raw_socket = socket.create_connection(
                    (address, 443),
                    self.timeout,
                    self.source_address,
                )
                self.sock = self._context.wrap_socket(
                    raw_socket, server_hostname=self.host
                )
                return
            except OSError as error:
                last_error = error
                if raw_socket is not None:
                    raw_socket.close()
        if last_error is None:
            raise OSError("no validated download address")
        raise last_error


class _PinnedResponse:
    def __init__(
        self,
        response: Any,
        connection: Any,
        url: str,
    ) -> None:
        self._response = response
        self._connection = connection
        self._url = url
        self.headers = response.headers

    def geturl(self) -> str:
        return self._url

    def read(self, size: int = -1) -> bytes:
        return self._response.read(size)

    def read1(self, size: int = -1) -> bytes:
        method = getattr(self._response, "read1", self._response.read)
        return method(size)

    def close(self) -> None:
        try:
            self._response.close()
        finally:
            self._connection.close()

    def __enter__(self) -> _PinnedResponse:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()


def urlopen(request: Request, timeout: float) -> _PinnedResponse:
    """Open one allowlisted GET using only the addresses validated per hop."""
    if request.get_method() != "GET" or request.data is not None:
        raise ValueError("corpus downloader supports only GET")
    current_url = request.full_url
    headers = dict(request.header_items())
    started = _monotonic()
    for _ in range(6):
        remaining = timeout - (_monotonic() - started)
        if remaining <= 0:
            raise TimeoutError("download connection deadline exceeded")
        parsed = urlsplit(current_url)
        assert parsed.hostname is not None
        addresses = _validate_download_url(
            current_url, timeout_seconds=remaining
        )
        connection = _PinnedHTTPSConnection(
            parsed.hostname, addresses, remaining
        )
        target = parsed.path or "/"
        if parsed.query:
            target = f"{target}?{parsed.query}"
        try:
            connection.request("GET", target, headers=headers)
            response = connection.getresponse()
        except BaseException:
            connection.close()
            raise
        if response.status not in {301, 302, 303, 307, 308}:
            if not 200 <= response.status < 300:
                response.close()
                connection.close()
                raise ValueError("download server returned an unsuccessful status")
            return _PinnedResponse(response, connection, current_url)
        location = response.headers.get("Location")
        response.close()
        connection.close()
        if not location:
            raise ValueError("download redirect is missing Location")
        current_url = urljoin(current_url, location)
        remaining = timeout - (_monotonic() - started)
        if remaining <= 0:
            raise TimeoutError("download connection deadline exceeded")
        _validate_download_url(current_url, timeout_seconds=remaining)
    raise ValueError("download redirect limit exceeded")


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
    started = _monotonic()
    request = Request(
        source.url,
        headers={"User-Agent": "Markhand-OCR-corpus-downloader/1"},
    )
    digest = hashlib.sha256()
    bytes_downloaded = 0

    initial_remaining = _DOWNLOAD_TOTAL_DEADLINE_SECONDS - (
        _monotonic() - started
    )
    if initial_remaining <= 0:
        raise TimeoutError(f"{source.id}: download deadline exceeded")
    _validate_download_url(
        source.url, timeout_seconds=initial_remaining
    )
    remaining = _DOWNLOAD_TOTAL_DEADLINE_SECONDS - (_monotonic() - started)
    if remaining <= 0:
        raise TimeoutError(f"{source.id}: download deadline exceeded")
    with urlopen(
        request, timeout=min(_DOWNLOAD_TIMEOUT_SECONDS, remaining)
    ) as response:
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

        read_chunk = getattr(response, "read1", response.read)
        while True:
            if _monotonic() - started >= _DOWNLOAD_TOTAL_DEADLINE_SECONDS:
                raise TimeoutError(f"{source.id}: download deadline exceeded")
            chunk = read_chunk(_CHUNK_BYTES)
            if not chunk:
                break
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


def _resolve_host(hostname: str, timeout_seconds: float) -> list[Any]:
    results: queue.Queue[tuple[bool, object]] = queue.Queue(maxsize=1)

    def resolve() -> None:
        try:
            addresses = socket.getaddrinfo(
                hostname,
                443,
                type=socket.SOCK_STREAM,
            )
        except OSError as error:
            results.put((False, error))
        else:
            results.put((True, addresses))

    threading.Thread(
        target=resolve,
        name="markhand-corpus-dns",
        daemon=True,
    ).start()
    try:
        success, value = results.get(timeout=timeout_seconds)
    except queue.Empty as error:
        raise TimeoutError(
            f"download DNS resolution deadline exceeded: {hostname}"
        ) from error
    if not success:
        assert isinstance(value, OSError)
        raise ValueError(f"download host could not be resolved: {hostname}") from value
    assert isinstance(value, list)
    return value


def _validate_download_url(
    url: str,
    *,
    timeout_seconds: float = _DOWNLOAD_TIMEOUT_SECONDS,
) -> tuple[str, ...]:
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
    addresses = _resolve_host(parsed.hostname, timeout_seconds)
    if not addresses:
        raise ValueError(f"download host could not be resolved: {parsed.hostname}")

    validated: list[str] = []
    for address in addresses:
        address_text = address[4][0]
        ip = ipaddress.ip_address(address_text)
        if not ip.is_global:
            raise ValueError(
                f"download host must resolve only to public addresses: {parsed.hostname}"
            )
        if address_text not in validated:
            validated.append(address_text)
    return tuple(validated)
