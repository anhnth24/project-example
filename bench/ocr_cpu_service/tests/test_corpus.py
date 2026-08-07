from __future__ import annotations

import hashlib
import json
import sys
from io import BytesIO
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from corpus.download import (  # noqa: E402
    CorpusSource,
    download_sources,
    load_sources,
    validate_source,
)


def valid_source(**overrides: object) -> CorpusSource:
    values: dict[str, object] = {
        "id": "fixture",
        "url": "https://example.com/fixture.pdf",
        "license": "CC0-1.0",
        "sha256": "0" * 64,
        "max_bytes": 1024,
        "kind": "real-scan",
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
    write_manifest(manifest, [valid_source(), valid_source(url="https://example.com/b.pdf")])

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
        "redirect_url": "https://example.net/unreviewed.pdf",
    }
    manifest.write_text(json.dumps([source]), encoding="utf-8")

    with pytest.raises(ValueError, match="unexpected"):
        load_sources(manifest)


def test_checked_in_manifest_is_valid_and_pinned() -> None:
    manifest = Path(__file__).parents[1] / "corpus" / "sources.json"

    sources = load_sources(manifest)

    assert sources
    assert any(source.kind == "real-scan" for source in sources)
    assert any(source.kind == "synthetic-scan" for source in sources)
    assert all(
        "/resolve/36f3060fd7628937c77c1b1e2a95892f24f562e0/" in source.url
        for source in sources
    )


class FakeResponse(BytesIO):
    def __init__(
        self,
        body: bytes,
        content_length: int | None = None,
        url: str = "https://example.com/fixture.pdf",
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
        lambda request: FakeResponse(body, len(body)),
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
        lambda request: FakeResponse(b"small", 5),
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
        lambda request: FakeResponse(b"large"),
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
        lambda request: FakeResponse(b"", url="http://example.com/fixture.pdf"),
    )

    with pytest.raises(ValueError, match="redirected"):
        download_sources(manifest, output)

    assert not (output / "fixture.pdf").exists()


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
        lambda request: FakeResponse(b"wrong"),
    )

    with pytest.raises(ValueError, match="SHA-256"):
        download_sources(manifest, output)

    assert destination.read_bytes() == b"trusted existing file"
    assert not list(output.glob("*.tmp"))
