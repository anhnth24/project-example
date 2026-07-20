#!/usr/bin/env python3
"""Print MARKHAND_INDEX_SIGNATURE for a dev embedding runtime (mirrors fileconv-knowledge)."""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
from urllib.parse import urlparse, urlunparse

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(SCRIPT_DIR, "..", "..", "bench", "markhand_web", "scripts"))

from knowledge_identity import index_signature  # noqa: E402


def provider_deployment_digest(base_url: str) -> str:
    parsed = urlparse(base_url.strip())
    if parsed.scheme not in {"http", "https"}:
        raise SystemExit(f"unsupported embedding base URL scheme: {parsed.scheme!r}")
    path = parsed.path.rstrip("/") or "/"
    canonical = urlunparse((parsed.scheme, parsed.netloc, path, "", "", ""))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def embedding_family(provider: str, model: str, base_url: str) -> str:
    deployment = provider_deployment_digest(base_url)
    return f"{provider}/{model}/{deployment}"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Compute MARKHAND_INDEX_SIGNATURE for an approved embedding runtime."
    )
    parser.add_argument(
        "--base-url",
        default=os.environ.get("MARKHAND_EMBEDDING_BASE_URL", "http://127.0.0.1:8088/v1"),
    )
    parser.add_argument(
        "--provider",
        default=os.environ.get("MARKHAND_EMBEDDING_PROVIDER", "openai-compatible"),
    )
    parser.add_argument(
        "--model",
        default=os.environ.get(
            "MARKHAND_EMBEDDING_MODEL", "AITeamVN/Vietnamese_Embedding"
        ),
    )
    parser.add_argument(
        "--revision",
        default=os.environ.get(
            "MARKHAND_EMBEDDING_REVISION",
            "dea33aa1ab339f38d66ae0a40e6c40e0a9249568",
        ),
    )
    parser.add_argument(
        "--dimensions",
        type=int,
        default=int(os.environ.get("MARKHAND_EMBEDDING_DIMENSIONS", "1024")),
    )
    parser.add_argument(
        "--runtime-path",
        default=os.environ.get("MARKHAND_EMBEDDING_RUNTIME_PATH", "local-neural"),
    )
    args = parser.parse_args()

    family = embedding_family(args.provider, args.model, args.base_url)
    signature = index_signature(
        runtime_path=args.runtime_path,
        embedding_family=family,
        embedding_revision=args.revision,
        dimensions=args.dimensions,
        normalized=True,
    )
    print(signature)


if __name__ == "__main__":
    main()
