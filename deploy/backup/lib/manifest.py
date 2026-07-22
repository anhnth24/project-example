#!/usr/bin/env python3
"""Canonical recovery manifest create / validate / sign helpers (P1B-O03).

The manifest binds PostgreSQL backup/WAL boundary, MinIO version-inventory
digest, and Qdrant snapshot/generation/signature identities. It must never
embed secrets, object content, or object keys beyond operational digests.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import sys
from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
ROOT = Path(__file__).resolve().parents[3]
SCHEMA_PATH = (
    ROOT / "deploy" / "backup" / "schema" / "recovery-manifest.schema.json"
)
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
LSN_RE = re.compile(r"^[0-9A-F]+/[0-9A-F]+$")
SAFE_REL_RE = re.compile(r"^(?!/)(?!.*\.\./)[A-Za-z0-9._/-]+$")
FORBIDDEN_KEY_FRAGMENTS = (
    "password",
    "secret",
    "api_key",
    "apikey",
    "private_key",
    "access_key",
    "token",
    "authorization",
    "object_key",
    "objectkey",
    "content",
)


class ManifestError(ValueError):
    """Fail-closed manifest error."""


def utc_now() -> str:
    return (
        datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_json(payload: dict[str, Any]) -> bytes:
    """RFC-style canonical JSON: sorted keys, no insignificant whitespace."""
    return json.dumps(
        payload,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ManifestError(f"{path}: cannot load JSON: {error}") from error
    if not isinstance(value, dict):
        raise ManifestError(f"{path}: expected JSON object")
    return value


def signing_key_from_env() -> tuple[str, bytes]:
    key_id = os.environ.get("MARKHAND_BACKUP_SIGNING_KEY_ID", "").strip()
    key_hex = os.environ.get("MARKHAND_BACKUP_SIGNING_KEY", "").strip()
    if not key_id or not key_hex:
        raise ManifestError(
            "MARKHAND_BACKUP_SIGNING_KEY_ID and MARKHAND_BACKUP_SIGNING_KEY "
            "are required (narrow HMAC material; never log the key)"
        )
    if not SHA256_RE.fullmatch(key_hex):
        raise ManifestError(
            "MARKHAND_BACKUP_SIGNING_KEY must be 64 lowercase hex chars (32 bytes)"
        )
    return key_id, bytes.fromhex(key_hex)


def sign_payload(payload: dict[str, Any], key_id: str, key: bytes) -> dict[str, str]:
    body = deepcopy(payload)
    body.pop("signature", None)
    digest = hmac.new(key, canonical_json(body), hashlib.sha256).hexdigest()
    return {"algorithm": "hmac-sha256", "keyId": key_id, "value": digest}


def verify_signature(payload: dict[str, Any], key: bytes) -> None:
    signature = payload.get("signature")
    if not isinstance(signature, dict):
        raise ManifestError("manifest missing signature object")
    if signature.get("algorithm") != "hmac-sha256":
        raise ManifestError("unsupported signature algorithm")
    expected = signature.get("value")
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ManifestError("signature.value must be sha256 hex")
    body = deepcopy(payload)
    body.pop("signature", None)
    actual = hmac.new(key, canonical_json(body), hashlib.sha256).hexdigest()
    if not hmac.compare_digest(actual, expected):
        raise ManifestError("manifest signature mismatch (fail closed)")


def _require_str(obj: dict[str, Any], key: str) -> str:
    value = obj.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ManifestError(f"missing or empty string field: {key}")
    return value


def _walk_forbidden(node: Any, path: str = "$") -> None:
    if isinstance(node, dict):
        for key, value in node.items():
            lowered = str(key).lower()
            if any(fragment in lowered for fragment in FORBIDDEN_KEY_FRAGMENTS):
                raise ManifestError(
                    f"forbidden secret/content field present at {path}.{key}"
                )
            _walk_forbidden(value, f"{path}.{key}")
    elif isinstance(node, list):
        for index, value in enumerate(node):
            _walk_forbidden(value, f"{path}[{index}]")
    elif isinstance(node, str):
        if re.search(
            r"(?i)(postgres(?:ql)?://\S+:\S+@|-----BEGIN [A-Z ]*PRIVATE KEY-----|"
            r"\bAKIA[0-9A-Z]{16}\b|\bghp_[A-Za-z0-9]{20,}\b)",
            node,
        ):
            raise ManifestError(f"secret-like material embedded at {path}")


def validate_structure(payload: dict[str, Any]) -> list[str]:
    """Structural + semantic validation (no external jsonschema dependency)."""
    errors: list[str] = []

    def err(message: str) -> None:
        errors.append(message)

    try:
        _walk_forbidden(payload)
    except ManifestError as error:
        err(str(error))

    if payload.get("schemaVersion") != SCHEMA_VERSION:
        err(f"schemaVersion must be {SCHEMA_VERSION}")

    for key in (
        "manifestId",
        "createdAt",
        "orgId",
        "appVersion",
        "migrationVersion",
        "indexSignatureSha256",
        "consistencyFence",
        "postgres",
        "minio",
        "qdrant",
        "artifacts",
        "checksums",
        "signature",
    ):
        if key not in payload:
            err(f"missing required field: {key}")

    org_id = payload.get("orgId")
    if isinstance(org_id, str) and not UUID_RE.fullmatch(org_id):
        err("orgId must be a UUID")

    index_sig = payload.get("indexSignatureSha256")
    if isinstance(index_sig, str) and not SHA256_RE.fullmatch(index_sig):
        err("indexSignatureSha256 must be 64 hex chars")

    schema_name = payload.get("schemaName", "public")
    if not isinstance(schema_name, str) or not re.fullmatch(
        r"[A-Za-z_][A-Za-z0-9_]*", schema_name
    ):
        err("schemaName must be a simple SQL identifier")

    fence = payload.get("consistencyFence")
    if isinstance(fence, dict):
        mode = fence.get("mode")
        if mode not in {"strict-write-fence", "ordered-bounded"}:
            err("consistencyFence.mode invalid")
        if mode == "strict-write-fence" and fence.get("writesFenced") is not True:
            err("strict-write-fence requires writesFenced=true")
        ordering = fence.get("ordering")
        if not isinstance(ordering, list) or "postgres" not in ordering:
            err("consistencyFence.ordering must include postgres")
        if isinstance(ordering, list) and ordering and ordering[0] != "postgres":
            # Backup capture may order differently, but restore authority starts at PG.
            pass
    else:
        err("consistencyFence must be an object")

    postgres = payload.get("postgres")
    if isinstance(postgres, dict):
        if postgres.get("method") != "pg_basebackup_pitr":
            err("postgres.method must be pg_basebackup_pitr")
        lsn = postgres.get("walBoundaryLsn")
        if not isinstance(lsn, str) or not LSN_RE.fullmatch(lsn):
            err("postgres.walBoundaryLsn must look like WAL LSN")
        digest = postgres.get("baseBackupDigestSha256")
        if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
            err("postgres.baseBackupDigestSha256 invalid")
        if postgres.get("encrypted") is not True:
            err("postgres.encrypted must be true (encrypted narrow credentials path)")
    else:
        err("postgres must be an object")

    minio = payload.get("minio")
    if isinstance(minio, dict):
        if minio.get("versioningEnabled") is not True:
            err("minio.versioningEnabled must be true")
        digest = minio.get("inventoryDigestSha256")
        if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
            err("minio.inventoryDigestSha256 invalid")
        count = minio.get("objectVersionCount")
        if not isinstance(count, int) or count < 0:
            err("minio.objectVersionCount must be >= 0")
    else:
        err("minio must be an object")

    qdrant = payload.get("qdrant")
    if isinstance(qdrant, dict):
        for field in (
            "indexSignatureSha256",
            "snapshotDigestSha256",
        ):
            value = qdrant.get(field)
            if not isinstance(value, str) or not SHA256_RE.fullmatch(value):
                err(f"qdrant.{field} invalid")
        if qdrant.get("indexSignatureSha256") != payload.get("indexSignatureSha256"):
            err("qdrant.indexSignatureSha256 must match top-level indexSignatureSha256")
        generation = qdrant.get("collectionGeneration")
        if not isinstance(generation, int) or generation < 0:
            err("qdrant.collectionGeneration must be >= 0")
    else:
        err("qdrant must be an object")

    artifacts = payload.get("artifacts")
    checksums = payload.get("checksums")
    if isinstance(artifacts, dict) and isinstance(checksums, dict):
        rel = artifacts.get("relativePaths")
        if not isinstance(rel, dict) or not rel:
            err("artifacts.relativePaths must be a non-empty object")
        else:
            for role, path in rel.items():
                if not isinstance(path, str) or not SAFE_REL_RE.fullmatch(path):
                    err(f"unsafe artifact path for {role}: {path!r}")
                elif path not in checksums:
                    err(f"checksums missing entry for artifact path {path}")
                elif not SHA256_RE.fullmatch(str(checksums[path])):
                    err(f"checksums[{path}] invalid")
    else:
        err("artifacts/checksums must be objects")

    signature = payload.get("signature")
    if isinstance(signature, dict):
        if signature.get("algorithm") != "hmac-sha256":
            err("signature.algorithm must be hmac-sha256")
        if not isinstance(signature.get("keyId"), str) or not signature["keyId"]:
            err("signature.keyId required")
        if not isinstance(signature.get("value"), str) or not SHA256_RE.fullmatch(
            signature["value"]
        ):
            err("signature.value invalid")
    else:
        err("signature must be an object")

    return errors


def validate_against_expected(
    payload: dict[str, Any],
    *,
    expected_org_id: str | None = None,
    expected_schema: str | None = None,
    expected_index_signature: str | None = None,
    expected_migration_version: str | None = None,
) -> list[str]:
    errors = validate_structure(payload)
    if expected_org_id and payload.get("orgId") != expected_org_id:
        errors.append(
            f"orgId mismatch: manifest={payload.get('orgId')} expected={expected_org_id}"
        )
    schema_name = payload.get("schemaName", "public")
    if expected_schema and schema_name != expected_schema:
        errors.append(
            f"schemaName mismatch: manifest={schema_name} expected={expected_schema}"
        )
    if expected_index_signature and payload.get("indexSignatureSha256") != expected_index_signature:
        errors.append("indexSignatureSha256 mismatch vs runtime config")
    if expected_migration_version and payload.get("migrationVersion") != expected_migration_version:
        errors.append(
            "migrationVersion mismatch vs expected upgrade compatibility marker"
        )
    return errors


def verify_artifact_checksums(payload: dict[str, Any], backup_root: Path) -> list[str]:
    errors: list[str] = []
    root = backup_root.resolve()
    checksums = payload.get("checksums") or {}
    rel_paths = (payload.get("artifacts") or {}).get("relativePaths") or {}
    if not isinstance(checksums, dict) or not isinstance(rel_paths, dict):
        return ["artifacts/checksums malformed"]
    for role, rel in rel_paths.items():
        if not isinstance(rel, str) or not SAFE_REL_RE.fullmatch(rel):
            errors.append(f"path traversal/unsafe artifact path ({role}): {rel!r}")
            continue
        candidate = root / rel
        # Reject symlinks before resolve() (resolve follows links and can escape).
        if candidate.is_symlink():
            errors.append(f"symlink artifact rejected ({role}): {rel}")
            continue
        try:
            path = candidate.resolve(strict=False)
            path.relative_to(root)
        except ValueError:
            errors.append(f"artifact escapes backup root ({role}): {rel}")
            continue
        if not candidate.is_file():
            errors.append(f"missing artifact ({role}): {rel}")
            continue
        actual = sha256_file(candidate)
        expected = checksums.get(rel)
        if actual != expected:
            errors.append(f"checksum mismatch for {rel}")
    return errors


def build_manifest(
    *,
    manifest_id: str,
    org_id: str,
    app_version: str,
    migration_version: str,
    index_signature: str,
    postgres: dict[str, Any],
    minio: dict[str, Any],
    qdrant: dict[str, Any],
    relative_paths: dict[str, str],
    checksums: dict[str, str],
    consistency_fence: dict[str, Any],
    schema_name: str = "public",
    notes: list[str] | None = None,
    key_id: str,
    key: bytes,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "schemaVersion": SCHEMA_VERSION,
        "manifestId": manifest_id,
        "createdAt": utc_now(),
        "orgId": org_id,
        "appVersion": app_version,
        "migrationVersion": migration_version,
        "indexSignatureSha256": index_signature,
        "schemaName": schema_name,
        "consistencyFence": consistency_fence,
        "postgres": postgres,
        "minio": minio,
        "qdrant": qdrant,
        "artifacts": {"relativePaths": relative_paths},
        "checksums": checksums,
        "notes": notes or [],
    }
    payload["signature"] = sign_payload(payload, key_id, key)
    errors = validate_structure(payload)
    if errors:
        raise ManifestError("; ".join(errors))
    return payload


def inventory_digest(rows: list[str]) -> tuple[str, int]:
    """Digest canonical inventory lines without embedding them in the manifest."""
    normalized = sorted(line.strip() for line in rows if line.strip())
    blob = "\n".join(normalized).encode("utf-8")
    return sha256_bytes(blob), len(normalized)


def redact_env_for_log(mapping: dict[str, str]) -> dict[str, str]:
    redacted: dict[str, str] = {}
    for key, value in mapping.items():
        lowered = key.lower()
        if any(fragment in lowered for fragment in FORBIDDEN_KEY_FRAGMENTS) or (
            "signing_key" in lowered and not lowered.endswith("_key_id")
        ):
            redacted[key] = "***REDACTED***"
        else:
            redacted[key] = value
    return redacted


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser("validate", help="Validate a recovery manifest")
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--backup-root", type=Path)
    validate.add_argument("--org-id")
    validate.add_argument("--schema-name", default="public")
    validate.add_argument("--index-signature")
    validate.add_argument("--migration-version")
    validate.add_argument(
        "--verify-signature",
        action="store_true",
        help="Verify HMAC using MARKHAND_BACKUP_SIGNING_KEY*",
    )

    sign = sub.add_parser("sign", help="(Re)sign a manifest body lacking signature")
    sign.add_argument("--manifest", type=Path, required=True)
    sign.add_argument("--output", type=Path)

    digest = sub.add_parser("inventory-digest", help="Digest stdin inventory lines")
    digest.add_argument("--input", type=Path)

    args = parser.parse_args(argv)

    try:
        if args.command == "inventory-digest":
            if args.input:
                lines = args.input.read_text(encoding="utf-8").splitlines()
            else:
                lines = sys.stdin.read().splitlines()
            digest_hex, count = inventory_digest(lines)
            json.dump({"digestSha256": digest_hex, "count": count}, sys.stdout)
            sys.stdout.write("\n")
            return 0

        if args.command == "sign":
            payload = load_json(args.manifest)
            key_id, key = signing_key_from_env()
            payload["signature"] = sign_payload(payload, key_id, key)
            errors = validate_structure(payload)
            if errors:
                raise ManifestError("; ".join(errors))
            out = args.output or args.manifest
            out.write_text(
                json.dumps(payload, indent=2, ensure_ascii=False) + "\n",
                encoding="utf-8",
            )
            print(f"signed {out}")
            return 0

        payload = load_json(args.manifest)
        errors = validate_against_expected(
            payload,
            expected_org_id=args.org_id,
            expected_schema=args.schema_name,
            expected_index_signature=args.index_signature,
            expected_migration_version=args.migration_version,
        )
        if args.verify_signature:
            _, key = signing_key_from_env()
            try:
                verify_signature(payload, key)
            except ManifestError as error:
                errors.append(str(error))
        if args.backup_root is not None:
            errors.extend(verify_artifact_checksums(payload, args.backup_root))
        if errors:
            print("manifest validation failed:", file=sys.stderr)
            for item in errors:
                print(f"- {item}", file=sys.stderr)
            return 1
        print("manifest validation ok")
        return 0
    except ManifestError as error:
        print(f"manifest error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
