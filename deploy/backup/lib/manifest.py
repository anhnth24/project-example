#!/usr/bin/env python3
"""O03 backup manifest: authenticate raw bytes before parse; strict JSON Schema."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import re
import sys
from pathlib import Path
from typing import Any

SAFE_REL_RE = re.compile(r"^(?!/)(?!.*\.\./)[A-Za-z0-9._/-]+$")
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
STAMP_RE = re.compile(r"^[0-9]{8}T[0-9]{6}Z$")
SCHEMA_VERSION = 3
SCHEMA_PATH = Path(__file__).resolve().parents[1] / "schema" / "manifest.schema.json"


class ManifestError(ValueError):
    """Fail-closed manifest error (safe message; no secrets)."""


def require_signing_env() -> tuple[str, str]:
    key = os.environ.get("MARKHAND_BACKUP_SIGNING_KEY", "")
    key_id = os.environ.get("MARKHAND_BACKUP_KEY_ID", "")
    if len(key.encode()) < 32:
        raise ManifestError("MARKHAND_BACKUP_SIGNING_KEY required (>=32 bytes)")
    if not key_id or not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", key_id):
        raise ManifestError("MARKHAND_BACKUP_KEY_ID required (safe token)")
    for arg in sys.argv[1:]:
        if key and key in arg:
            raise ManifestError("signing key must not appear on argv")
    return key, key_id


def canonical_dumps(payload: dict[str, Any]) -> bytes:
    return (
        json.dumps(payload, indent=2, sort_keys=True, separators=(",", ": ")) + "\n"
    ).encode()


def sign_raw(raw: bytes, key: str) -> str:
    return hmac.new(key.encode(), raw, hashlib.sha256).hexdigest()


def verify_raw_signature(raw: bytes, signature_hex: str, key: str) -> None:
    if not SHA256_RE.fullmatch(signature_hex or ""):
        raise ManifestError("manifest signature malformed")
    expected = sign_raw(raw, key)
    if not hmac.compare_digest(expected, signature_hex):
        raise ManifestError("manifest signature mismatch")


def assert_safe_rel(path: str, *, field: str) -> None:
    if not path or not SAFE_REL_RE.fullmatch(path):
        raise ManifestError(f"unsafe relative path in {field}")
    if path.startswith("/") or ".." in path.split("/"):
        raise ManifestError(f"path traversal refused in {field}")


def _load_json_schema() -> dict[str, Any]:
    try:
        return json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ManifestError("manifest JSON Schema unavailable") from exc


def validate_schema(manifest: dict[str, Any]) -> None:
    """Enforce Draft-07 schema including additionalProperties: false."""
    if not isinstance(manifest, dict):
        raise ManifestError("manifest must be object")
    try:
        import jsonschema
        from jsonschema import Draft7Validator
    except ImportError as exc:
        raise ManifestError(
            "jsonschema package required for manifest validation"
        ) from exc

    schema = _load_json_schema()
    validator = Draft7Validator(schema)
    errors = sorted(validator.iter_errors(manifest), key=lambda e: list(e.path))
    if errors:
        err = errors[0]
        path = ".".join(str(p) for p in err.path) or "(root)"
        raise ManifestError(f"schema validation failed at {path}: {err.message}")

    # Keep explicit invariant checks that tests assert by message substring.
    if manifest.get("schemaVersion") != SCHEMA_VERSION:
        raise ManifestError(
            f"unsupported/downgraded schemaVersion={manifest.get('schemaVersion')}"
        )
    if manifest.get("mode") != "blue_green":
        raise ManifestError("mode must be blue_green")
    if manifest.get("opsFenceMandatory") is not True:
        raise ManifestError("opsFenceMandatory must be true")
    if not STAMP_RE.fullmatch(str(manifest.get("capturedAt") or "")):
        raise ManifestError("capturedAt malformed")
    if not UUID_RE.fullmatch(str(manifest.get("fenceEpoch") or "")):
        raise ManifestError("fenceEpoch malformed")
    wm = manifest.get("watermarks") or {}
    if wm.get("jobsDrained") is not True:
        raise ManifestError("watermarks.jobsDrained must be true")


def safe_open_under(root: Path, rel: str) -> Path:
    """Resolve rel under root; reject traversal, symlinks, and escapes."""
    assert_safe_rel(rel, field="artifact")
    root_resolved = root.resolve(strict=True)
    candidate = (root / rel)
    # Reject symlinks at every path component.
    cur = root_resolved
    parts = Path(rel).parts
    for part in parts:
        cur = cur / part
        if cur.is_symlink():
            raise ManifestError(f"symlink refused: {rel}")
        if not cur.exists():
            # Allow missing until caller checks is_file; still refuse if a parent is link.
            break
    try:
        resolved = candidate.resolve(strict=False)
    except OSError as exc:
        raise ManifestError(f"artifact path unresolvable: {rel}") from exc
    try:
        if not resolved.is_relative_to(root_resolved):
            raise ManifestError(f"artifact escapes backup dir: {rel}")
    except AttributeError:
        # Python <3.9 fallback (should not hit on current toolchain).
        if not str(resolved).startswith(str(root_resolved) + os.sep) and resolved != root_resolved:
            raise ManifestError(f"artifact escapes backup dir: {rel}")
    if candidate.is_symlink() or resolved.is_symlink():
        raise ManifestError(f"symlink refused: {rel}")
    return resolved


def load_authenticated_manifest(backup_dir: Path) -> tuple[dict[str, Any], bytes]:
    """Verify HMAC over raw manifest bytes BEFORE json parse/use."""
    key, key_id = require_signing_env()
    if backup_dir.is_symlink():
        raise ManifestError("backup dir must not be a symlink")
    manifest_path = safe_open_under(backup_dir, "manifest.json")
    sig_path = safe_open_under(backup_dir, "manifest.sig")
    if not manifest_path.is_file():
        raise ManifestError("manifest.json missing")
    if not sig_path.is_file():
        raise ManifestError("manifest.sig missing")
    raw = manifest_path.read_bytes()
    sig = sig_path.read_text(encoding="utf-8").strip()
    verify_raw_signature(raw, sig, key)
    try:
        manifest = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ManifestError("manifest JSON malformed after auth") from exc
    validate_schema(manifest)
    tb = manifest.get("trustedBoundary") or {}
    if tb.get("keyId") != key_id:
        raise ManifestError("trustedBoundary.keyId mismatch vs MARKHAND_BACKUP_KEY_ID")
    for rel in (manifest.get("artifactSha256") or {}):
        assert_safe_rel(rel, field="artifactSha256")
    return manifest, raw


def verify_artifacts(backup_dir: Path, manifest: dict[str, Any]) -> None:
    artifacts = manifest["artifactSha256"]
    sizes = manifest["artifactBytes"]
    for rel, digest in artifacts.items():
        path = safe_open_under(backup_dir, str(rel))
        if not path.is_file():
            raise ManifestError(f"artifact missing: {rel}")
        if path.is_symlink():
            raise ManifestError(f"symlink refused: {rel}")
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ManifestError(f"artifact checksum mismatch: {rel}")
        if len(data) != int(sizes[rel]):
            raise ManifestError(f"artifact size mismatch: {rel}")


def write_signed_manifest(backup_dir: Path, payload: dict[str, Any]) -> None:
    key, key_id = require_signing_env()
    validate_schema(payload)
    if payload["trustedBoundary"].get("keyId") != key_id:
        raise ManifestError("payload keyId must match MARKHAND_BACKUP_KEY_ID")
    raw = canonical_dumps(payload)
    old = os.umask(0o077)
    try:
        tmp = backup_dir / "manifest.json.tmp"
        sig_tmp = backup_dir / "manifest.sig.tmp"
        sha_tmp = backup_dir / "manifest.sha256.tmp"
        tmp.write_bytes(raw)
        sig_tmp.write_text(sign_raw(raw, key) + "\n", encoding="utf-8")
        sha_tmp.write_text(hashlib.sha256(raw).hexdigest() + "\n", encoding="utf-8")
        for p in (tmp, sig_tmp, sha_tmp):
            os.chmod(p, 0o600)
        tmp.replace(backup_dir / "manifest.json")
        sig_tmp.replace(backup_dir / "manifest.sig")
        sha_tmp.replace(backup_dir / "manifest.sha256")
        for name in ("manifest.json", "manifest.sig", "manifest.sha256"):
            os.chmod(backup_dir / name, 0o600)
    finally:
        os.umask(old)
