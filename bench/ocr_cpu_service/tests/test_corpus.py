from __future__ import annotations

import hashlib
import json
import socket
import sys
import threading
import time
from io import BytesIO
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

import corpus.download as corpus_download  # noqa: E402
from corpus.download import (  # noqa: E402
    CorpusSource,
    download_sources,
    load_sources,
    validate_source,
)


@pytest.fixture(autouse=True)
def public_dns(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        socket,
        "getaddrinfo",
        lambda *args, **kwargs: [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 443))
        ],
    )


def valid_source(**overrides: object) -> CorpusSource:
    values: dict[str, object] = {
        "id": "fixture",
        "url": "https://huggingface.co/fixture.pdf",
        "license": "CC0-1.0",
        "sha256": "0" * 64,
        "max_bytes": 1024,
        "kind": "real-scan",
        "classification": "scan",
    }
    values.update(overrides)
    return CorpusSource(**values)  # type: ignore[arg-type]


def write_manifest(path: Path, sources: list[CorpusSource]) -> None:
    path.write_text(
        json.dumps(
            [
                {
                    "id": source.id,
                    "url": source.url,
                    "license": source.license,
                    "sha256": source.sha256,
                    "max_bytes": source.max_bytes,
                    "kind": source.kind,
                    "classification": source.classification,
                }
                for source in sources
            ]
        ),
        encoding="utf-8",
    )


def test_rejects_source_without_license(tmp_path: Path) -> None:
    manifest = tmp_path / "sources.json"
    manifest.write_text(
        '[{"id":"bad","url":"https://example.com/a.pdf","sha256":"00"}]',
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="license"):
        load_sources(manifest)


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"url": "http://example.com/a.pdf"}, "HTTPS"),
        ({"url": "https://example.com/a.pdf"}, "approved host"),
        ({"url": "https://127.0.0.1/a.pdf"}, "approved host"),
        ({"max_bytes": 0}, "positive"),
        ({"license": "  "}, "license"),
        ({"sha256": "A" * 64}, "lowercase hex"),
        ({"sha256": "0" * 63}, "lowercase hex"),
        ({"id": "../escape"}, "id"),
        ({"kind": ""}, "kind"),
    ],
)
def test_rejects_invalid_source(
    overrides: dict[str, object], message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        validate_source(valid_source(**overrides))


def test_load_sources_rejects_duplicate_ids(tmp_path: Path) -> None:
    manifest = tmp_path / "sources.json"
    write_manifest(
        manifest,
        [valid_source(), valid_source(url="https://huggingface.co/b.pdf")],
    )

    with pytest.raises(ValueError, match="duplicate"):
        load_sources(manifest)


def test_load_sources_rejects_unknown_fields(tmp_path: Path) -> None:
    manifest = tmp_path / "sources.json"
    source = {
        "id": "fixture",
        "url": "https://example.com/fixture.pdf",
        "license": "CC0-1.0",
        "sha256": "0" * 64,
        "max_bytes": 1024,
        "kind": "real-scan",
        "classification": "scan",
        "redirect_url": "https://example.net/unreviewed.pdf",
    }
    manifest.write_text(json.dumps([source]), encoding="utf-8")

    with pytest.raises(ValueError, match="unexpected"):
        load_sources(manifest)


def test_load_sources_requires_classification(tmp_path: Path) -> None:
    manifest = tmp_path / "sources.json"
    source = {
        "id": "fixture",
        "url": "https://huggingface.co/fixture.pdf",
        "license": "CC0-1.0",
        "sha256": "0" * 64,
        "max_bytes": 1024,
        "kind": "real-scan",
    }
    manifest.write_text(json.dumps([source]), encoding="utf-8")

    with pytest.raises(ValueError, match="classification"):
        load_sources(manifest)


def test_checked_in_manifest_is_valid_and_pinned() -> None:
    manifest = Path(__file__).parents[1] / "corpus" / "sources.json"

    sources = load_sources(manifest)

    assert sources
    assert any(source.kind == "real-scan" for source in sources)
    assert any(source.kind == "synthetic-scan" for source in sources)
    wikimedia = [source for source in sources if source.kind == "wikimedia-scan"]
    official = [source for source in sources if source.kind == "official-government"]
    assert len(wikimedia) >= 2
    assert all(source.classification == "scan" for source in wikimedia)
    assert len(official) == 1
    assert official[0].classification == "mixed"
    assert official[0].url.startswith("https://datafiles.chinhphu.vn/")
    assert all(
        "/resolve/36f3060fd7628937c77c1b1e2a95892f24f562e0/" in source.url
        for source in sources
        if source.url.startswith("https://huggingface.co/")
    )


class FakeResponse(BytesIO):
    def __init__(
        self,
        body: bytes,
        content_length: int | None = None,
        url: str = "https://huggingface.co/fixture.pdf",
    ) -> None:
        super().__init__(body)
        self.headers = (
            {} if content_length is None else {"Content-Length": str(content_length)}
        )
        self.url = url

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()

    def geturl(self) -> str:
        return self.url


def test_download_sources_streams_verifies_and_atomically_installs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    body = b"%PDF-1.4\nreviewed fixture\n"
    source = valid_source(sha256=hashlib.sha256(body).hexdigest())
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(body, len(body)),
    )

    downloaded = download_sources(manifest, output)

    assert len(downloaded) == 1
    assert downloaded[0].source == source
    assert downloaded[0].path == output / "fixture.pdf"
    assert downloaded[0].bytes_downloaded == len(body)
    assert downloaded[0].path.read_bytes() == body
    assert not list(output.glob("*.tmp"))


def test_download_sources_rejects_oversized_content_length(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source(max_bytes=4)
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(b"small", 5),
    )

    with pytest.raises(ValueError, match="Content-Length"):
        download_sources(manifest, output)

    assert not (output / "fixture.pdf").exists()


def test_download_sources_enforces_streamed_byte_limit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source(max_bytes=4)
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(b"large"),
    )

    with pytest.raises(ValueError, match="exceeds"):
        download_sources(manifest, output)

    assert not (output / "fixture.pdf").exists()
    assert not list(output.glob("*.tmp"))


def test_download_sources_rejects_redirect_to_non_https(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source()
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(
            b"", url="http://example.com/fixture.pdf"
        ),
    )

    with pytest.raises(ValueError, match="redirected"):
        download_sources(manifest, output)

    assert not (output / "fixture.pdf").exists()


def test_download_sources_rejects_redirect_to_unapproved_host(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    empty_sha256 = hashlib.sha256(b"").hexdigest()
    source = valid_source(sha256=empty_sha256)
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(
            b"", url="https://attacker.example/fixture.pdf"
        ),
    )

    with pytest.raises(ValueError, match="approved host"):
        download_sources(manifest, output)


def test_download_sources_rejects_approved_host_resolving_to_private_address(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    empty_sha256 = hashlib.sha256(b"").hexdigest()
    source = valid_source(sha256=empty_sha256)
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        socket,
        "getaddrinfo",
        lambda *args, **kwargs: [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 443))
        ],
    )
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(b""),
    )

    with pytest.raises(ValueError, match="public address"):
        download_sources(manifest, output)


def test_dns_resolution_is_bounded_by_connection_deadline(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    release = threading.Event()
    monkeypatch.setattr(
        socket,
        "getaddrinfo",
        lambda *args, **kwargs: (
            release.wait(timeout=1)
            or [
                (
                    socket.AF_INET,
                    socket.SOCK_STREAM,
                    6,
                    "",
                    ("93.184.216.34", 443),
                )
            ]
        ),
    )

    started = time.monotonic()
    try:
        with pytest.raises(TimeoutError, match="DNS"):
            corpus_download._validate_download_url(
                "https://huggingface.co/fixture.pdf",
                timeout_seconds=0.01,
            )
    finally:
        release.set()

    assert time.monotonic() - started < 0.5


def test_download_sources_uses_bounded_timeout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    body = b"bounded"
    source = valid_source(sha256=hashlib.sha256(body).hexdigest())
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    observed: dict[str, float | None] = {"timeout": None}

    def open_with_timeout(request: object, timeout: float | None = None) -> FakeResponse:
        observed["timeout"] = timeout
        return FakeResponse(body)

    monkeypatch.setattr("corpus.download.urlopen", open_with_timeout)

    download_sources(manifest, output)

    assert observed["timeout"] is not None
    assert 0 < observed["timeout"] <= 60


def test_download_sources_accepts_explicit_bounded_total_deadline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source()
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])
    observed: list[float] = []

    def stream(
        item: CorpusSource, destination: object, total_deadline_seconds: float
    ) -> int:
        del item
        observed.append(total_deadline_seconds)
        destination.write(b"bounded")  # type: ignore[attr-defined]
        return 7

    monkeypatch.setattr(corpus_download, "_stream_source", stream)

    downloaded = download_sources(
        manifest, output, total_deadline_seconds=600
    )

    assert observed == [600]
    assert downloaded[0].bytes_downloaded == 7


@pytest.mark.parametrize("deadline", [0, -1, 600.01, True])
def test_download_sources_rejects_invalid_total_deadline(
    tmp_path: Path, deadline: object
) -> None:
    manifest = tmp_path / "sources.json"
    write_manifest(manifest, [valid_source()])

    with pytest.raises(ValueError, match="deadline"):
        download_sources(
            manifest,
            tmp_path / "corpus",
            total_deadline_seconds=deadline,  # type: ignore[arg-type]
        )


def test_download_sources_enforces_total_monotonic_deadline_against_slow_drip(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source(max_bytes=100)
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    write_manifest(manifest, [source])

    class SlowDripResponse(FakeResponse):
        def __init__(self) -> None:
            super().__init__(b"")
            self.reads = 0

        def read1(self, size: int = -1) -> bytes:
            del size
            self.reads += 1
            return b"x" if self.reads <= 3 else b""

    ticks = iter((0.0, 0.0, 0.6, 1.2, 1.2))
    monkeypatch.setattr(
        corpus_download, "_monotonic", lambda: next(ticks), raising=False
    )
    monkeypatch.setattr(
        corpus_download, "_DOWNLOAD_TOTAL_DEADLINE_SECONDS", 1.0, raising=False
    )
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: SlowDripResponse(),
    )

    with pytest.raises(TimeoutError, match="deadline"):
        download_sources(manifest, output)

    assert not (output / "fixture.pdf").exists()
    assert not list(output.glob("*.tmp"))


def test_urlopen_resolves_relative_redirect_before_validating_next_hop(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    validated: list[str] = []

    class Response:
        def __init__(self, status: int, headers: dict[str, str]) -> None:
            self.status = status
            self.headers = headers

        def close(self) -> None:
            pass

    responses = iter(
        (
            Response(307, {"Location": "/api/resolve-cache/artifact"}),
            Response(200, {}),
        )
    )

    class Connection:
        def __init__(
            self, host: str, addresses: tuple[str, ...], timeout: float
        ) -> None:
            del host, addresses, timeout

        def request(
            self, method: str, target: str, headers: dict[str, str]
        ) -> None:
            del method, target, headers

        def getresponse(self) -> Response:
            return next(responses)

        def close(self) -> None:
            pass

    monkeypatch.setattr(
        corpus_download,
        "_validate_download_url",
        lambda url, **kwargs: validated.append(url) or ("203.0.113.10",),
    )
    monkeypatch.setattr(
        corpus_download, "_PinnedHTTPSConnection", Connection
    )

    response = corpus_download.urlopen(
        corpus_download.Request("https://huggingface.co/original"),
        timeout=2.5,
    )
    response.close()

    assert validated == [
        "https://huggingface.co/original",
        "https://huggingface.co/api/resolve-cache/artifact",
        "https://huggingface.co/api/resolve-cache/artifact",
    ]


def test_hugging_face_regional_cdn_is_an_approved_redirect_host() -> None:
    assert corpus_download._validate_download_url(
        "https://us.aws.cdn.hf.co/artifact"
    ) == ("93.184.216.34",)


def test_urlopen_binds_connection_to_prevalidated_address(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[object, ...]] = []

    class Response:
        status = 200
        headers: dict[str, str] = {}

        def read(self) -> bytes:
            return b""

        def close(self) -> None:
            calls.append(("response-close",))

    class Connection:
        def __init__(
            self, host: str, addresses: tuple[str, ...], timeout: float
        ) -> None:
            calls.append(("connect", host, addresses, timeout))

        def request(
            self, method: str, target: str, headers: dict[str, str]
        ) -> None:
            calls.append(("request", method, target, headers))

        def getresponse(self) -> Response:
            return Response()

        def close(self) -> None:
            calls.append(("connection-close",))

    monkeypatch.setattr(
        corpus_download,
        "_validate_download_url",
        lambda url, **kwargs: ("203.0.113.10",),
    )
    monkeypatch.setattr(
        corpus_download, "_PinnedHTTPSConnection", Connection, raising=False
    )

    response = corpus_download.urlopen(
        corpus_download.Request(
            "https://huggingface.co/path/file.pdf?download=1",
            headers={"User-Agent": "test"},
        ),
        timeout=2.5,
    )
    with response:
        assert response.geturl() == (
            "https://huggingface.co/path/file.pdf?download=1"
        )

    assert calls[0][0:3] == (
        "connect",
        "huggingface.co",
        ("203.0.113.10",),
    )
    assert 0 < float(calls[0][3]) <= 2.5
    assert calls[1][0:3] == (
        "request",
        "GET",
        "/path/file.pdf?download=1",
    )


def test_download_sources_rejects_checksum_mismatch_without_replacing_existing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = valid_source()
    manifest = tmp_path / "sources.json"
    output = tmp_path / "corpus"
    output.mkdir()
    destination = output / "fixture.pdf"
    destination.write_bytes(b"trusted existing file")
    write_manifest(manifest, [source])
    monkeypatch.setattr(
        "corpus.download.urlopen",
        lambda request, timeout=None: FakeResponse(b"wrong"),
    )

    with pytest.raises(ValueError, match="SHA-256"):
        download_sources(manifest, output)

    assert destination.read_bytes() == b"trusted existing file"
    assert not list(output.glob("*.tmp"))
