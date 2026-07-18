"""Python mirror of crates/knowledge identity digests (schema v2)."""

from __future__ import annotations

import hashlib
import struct

IDENTITY_VERSION = 2
BODY_TEXT_VERSION = "nfc-v1"
QUERY_NORMALIZATION_VERSION = "accent-fold-v1"
DEFAULT_CHUNKING_VERSION = "heading-chunks-2000-v1"
RUNTIME_LOCAL_HASH = "local-hash"
RUNTIME_LOCAL_NEURAL = "local-neural"
RUNTIME_GLM_CLOUD_INTERIM = "glm-cloud-interim"
RUNTIME_VLLM_LOCAL = "vllm-local"
RUNTIME_PROVIDER_CLOUD = "provider-cloud"


def _update_field(hasher, value: bytes) -> None:
    hasher.update(struct.pack(">Q", len(value)))
    hasher.update(value)


def digest(domain: str, fields: list[bytes]) -> str:
    hasher = hashlib.sha256()
    hasher.update(b"markhand-knowledge-identity")
    hasher.update(struct.pack(">H", IDENTITY_VERSION))
    _update_field(hasher, domain.encode("utf-8"))
    for field in fields:
        _update_field(hasher, field)
    return hasher.hexdigest()


def document_identity(source_rel: str, content_sha256: str) -> str:
    return digest("document", [source_rel.encode("utf-8"), content_sha256.encode("utf-8")])


def chunk_identity(
    document_id: str,
    version_id: str,
    ordinal: int,
    heading_path: str,
    body: str,
    body_text_version: str = BODY_TEXT_VERSION,
) -> str:
    return digest(
        "chunk",
        [
            document_id.encode("utf-8"),
            version_id.encode("utf-8"),
            struct.pack(">Q", ordinal),
            heading_path.encode("utf-8"),
            body.encode("utf-8"),
            body_text_version.encode("utf-8"),
        ],
    )


def index_signature(
    *,
    runtime_path: str,
    embedding_family: str,
    embedding_revision: str,
    dimensions: int,
    normalized: bool,
    chunking_version: str = DEFAULT_CHUNKING_VERSION,
    body_text_version: str = BODY_TEXT_VERSION,
    query_normalization_version: str = QUERY_NORMALIZATION_VERSION,
) -> str:
    return digest(
        "index",
        [
            runtime_path.encode("utf-8"),
            embedding_family.encode("utf-8"),
            embedding_revision.encode("utf-8"),
            struct.pack(">Q", dimensions),
            bytes([1 if normalized else 0]),
            chunking_version.encode("utf-8"),
            body_text_version.encode("utf-8"),
            query_normalization_version.encode("utf-8"),
        ],
    )
