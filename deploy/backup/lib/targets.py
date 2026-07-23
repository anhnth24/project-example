#!/usr/bin/env python3
"""Immutable green allowlists + restore-owned creation tokens (O03)."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlparse


class TargetError(ValueError):
    pass


_BUCKET_RE = re.compile(r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$")
_COLL_RE = re.compile(r"^[A-Za-z0-9._-]{1,128}$")
_DB_RE = re.compile(r"^[A-Za-z0-9_]{1,63}$")


@dataclass(frozen=True)
class GreenAllowlists:
    """Frozen allowlists — constructed once; never mutated.

    MinIO and Qdrant allowlist env vars are **mandatory** (no single-target fallback).
    """

    pg: tuple[tuple[str, str], ...]  # (system_identifier, database)
    minio_buckets: tuple[str, ...]
    qdrant_collections: tuple[str, ...]
    digest: str

    @staticmethod
    def load_from_env() -> "GreenAllowlists":
        pg_raw = os.environ.get("MARKHAND_GREEN_ALLOWLIST_JSON", "")
        buckets_raw = os.environ.get("MARKHAND_GREEN_MINIO_ALLOWLIST_JSON", "")
        colls_raw = os.environ.get("MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON", "")
        if not pg_raw.strip():
            raise TargetError("MARKHAND_GREEN_ALLOWLIST_JSON required")
        if not buckets_raw.strip():
            raise TargetError(
                "MARKHAND_GREEN_MINIO_ALLOWLIST_JSON required (mandatory allowlist policy)"
            )
        if not colls_raw.strip():
            raise TargetError(
                "MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON required (mandatory allowlist policy)"
            )
        try:
            pg_list = json.loads(pg_raw)
        except json.JSONDecodeError as exc:
            raise TargetError("MARKHAND_GREEN_ALLOWLIST_JSON malformed") from exc
        if not isinstance(pg_list, list) or not pg_list:
            raise TargetError("MARKHAND_GREEN_ALLOWLIST_JSON must be non-empty list")
        pg_entries: list[tuple[str, str]] = []
        for entry in pg_list:
            if not isinstance(entry, dict):
                raise TargetError("pg allowlist entry must be object")
            sys_id = str(entry.get("pgSystemIdentifier") or "")
            db = str(entry.get("pgDatabase") or "")
            if not sys_id.isdigit() or not _DB_RE.fullmatch(db):
                raise TargetError("pg allowlist entry invalid")
            pg_entries.append((sys_id, db))

        try:
            buckets = json.loads(buckets_raw)
        except json.JSONDecodeError as exc:
            raise TargetError("MARKHAND_GREEN_MINIO_ALLOWLIST_JSON malformed") from exc
        if not isinstance(buckets, list) or not buckets:
            raise TargetError("green MinIO bucket allowlist must be non-empty list")
        for b in buckets:
            if not isinstance(b, str) or not _BUCKET_RE.fullmatch(b):
                raise TargetError(f"invalid green bucket allowlist entry: {b!r}")

        try:
            colls = json.loads(colls_raw)
        except json.JSONDecodeError as exc:
            raise TargetError("MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON malformed") from exc
        if not isinstance(colls, list) or not colls:
            raise TargetError("green Qdrant collection allowlist must be non-empty list")
        for c in colls:
            if not isinstance(c, str) or not _COLL_RE.fullmatch(c):
                raise TargetError(f"invalid green collection allowlist entry: {c!r}")

        canonical = json.dumps(
            {
                "pg": sorted(pg_entries),
                "minio": sorted(buckets),
                "qdrant": sorted(colls),
            },
            sort_keys=True,
        )
        digest = hashlib.sha256(canonical.encode()).hexdigest()
        return GreenAllowlists(
            pg=tuple(sorted(pg_entries)),
            minio_buckets=tuple(sorted(str(b) for b in buckets)),
            qdrant_collections=tuple(sorted(str(c) for c in colls)),
            digest=digest,
        )

    def assert_pg(self, system_identifier: str, database: str) -> None:
        if (system_identifier, database) not in self.pg:
            raise TargetError("green PG identity not in immutable allowlist")

    def assert_bucket(self, bucket: str) -> None:
        if bucket not in self.minio_buckets:
            raise TargetError("green MinIO bucket not in immutable allowlist")

    def assert_collection(self, collection: str) -> None:
        if collection not in self.qdrant_collections:
            raise TargetError("green Qdrant collection not in immutable allowlist")


def endpoint_alias(a: str, b: str) -> bool:
    """True when two HTTP(S) endpoints resolve to the same host:port scheme."""
    pa, pb = urlparse(a), urlparse(b)
    ha = (pa.hostname or "").lower()
    hb = (pb.hostname or "").lower()
    if ha in {"127.0.0.1", "localhost"} and hb in {"127.0.0.1", "localhost"}:
        ha = hb = "loopback"
    porta = pa.port or (443 if pa.scheme == "https" else 80)
    portb = pb.port or (443 if pb.scheme == "https" else 80)
    return pa.scheme == pb.scheme and ha == hb and porta == portb


def assert_not_blue_alias(
    *,
    blue_bucket: str,
    green_bucket: str,
    blue_collection: str,
    green_collection: str,
    blue_endpoint: str,
    green_endpoint: str | None,
    blue_qdrant: str,
    green_qdrant: str | None,
) -> None:
    if green_bucket == blue_bucket:
        raise TargetError("green bucket equals blue source bucket")
    if green_collection == blue_collection:
        raise TargetError("green collection equals blue source collection")
    gep = green_endpoint or blue_endpoint
    gq = green_qdrant or blue_qdrant
    if endpoint_alias(gep, blue_endpoint) and green_bucket == blue_bucket:
        raise TargetError("green MinIO is endpoint+bucket alias of blue")
    if endpoint_alias(gq, blue_qdrant) and green_collection == blue_collection:
        raise TargetError("green Qdrant is endpoint+collection alias of blue")


@dataclass(frozen=True)
class CreationToken:
    """Restore-owned token authorizing create/delete of isolated green resources."""

    token: str
    path: Path

    @staticmethod
    def issue(work_dir: Path) -> "CreationToken":
        token = secrets.token_hex(16)
        path = work_dir / f".restore-create-{token}"
        old = os.umask(0o077)
        try:
            path.write_text(token + "\n", encoding="utf-8")
            os.chmod(path, 0o600)
        finally:
            os.umask(old)
        return CreationToken(token=token, path=path)

    def write_resource_marker(self, path: Path, resource: dict[str, Any]) -> None:
        old = os.umask(0o077)
        try:
            path.write_text(
                json.dumps({"token": self.token, "resource": resource}, sort_keys=True)
                + "\n",
                encoding="utf-8",
            )
            os.chmod(path, 0o600)
        finally:
            os.umask(old)

    def assert_resource_marker(self, path: Path) -> dict[str, Any]:
        if not path.is_file():
            raise TargetError("resource marker missing — refuse unrelated delete")
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise TargetError("resource marker malformed") from exc
        if data.get("token") != self.token:
            raise TargetError("resource marker token mismatch")
        return data.get("resource") or {}

    @staticmethod
    def load_marker(path: Path) -> dict[str, Any]:
        """Read a marker without owning the token (for cleanup verification)."""
        if not path.is_file():
            raise TargetError("resource marker missing")
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise TargetError("resource marker malformed") from exc
        if not data.get("token") or not isinstance(data.get("resource"), dict):
            raise TargetError("resource marker incomplete")
        return data
