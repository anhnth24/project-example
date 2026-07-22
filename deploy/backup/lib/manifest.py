#!/usr/bin/env python3
"""Canonical recovery manifest create/validate/sign (P1B-O03 rebuild)."""

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

_LIB = Path(__file__).resolve().parent
if str(_LIB) not in sys.path:
    sys.path.insert(0, str(_LIB))

from schema_validate import (  # noqa: E402
    SCHEMA_PATH,
    SchemaError,
    load_schema,
    validate_manifest,
)
from strictjson import (  # noqa: E402
    StrictJsonError,
    load_path,
    loads,
    walk,
)


SCHEMA_VERSION = 1
ENCRYPTION_ALGORITHM = "aes-256-ctr-hmac-sha256-v1"
ENCRYPTION_KDF = "hkdf-sha256"
ALLOWED_POSTGRES_METHODS = frozenset({"pg_basebackup_streamed_wal", "pitr_archive"})
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.I,
)
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
LSN_RE = re.compile(r"^[0-9A-F]+/[0-9A-F]+$")
SAFE_REL_RE = re.compile(r"^(?!/)(?!.*\.\./)[A-Za-z0-9._/-]+$")

FORBIDDEN_FRAGMENTS = (
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


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_json(payload: dict[str, Any]) -> bytes:
    return json.dumps(
        payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


def inventory_digest(rows: list[str]) -> tuple[str, int]:
    normalized = sorted(line.strip() for line in rows if line.strip())
    blob = "\n".join(normalized).encode("utf-8")
    return hashlib.sha256(blob).hexdigest(), len(normalized)


def schema_method_enum(schema: dict[str, Any] | None = None) -> set[str]:
    root = schema if schema is not None else load_schema()
    methods = (
        root.get("definitions", {})
        .get("postgres", {})
        .get("properties", {})
        .get("method", {})
        .get("enum")
    )
    if not isinstance(methods, list) or not methods:
        raise ManifestError("schema missing postgres.method enum")
    return set(methods)


def schema_encryption_algorithm(schema: dict[str, Any] | None = None) -> str:
    root = schema if schema is not None else load_schema()
    algo = (
        root.get("definitions", {})
        .get("encryptionObject", {})
        .get("properties", {})
        .get("algorithm", {})
        .get("const")
    )
    if not isinstance(algo, str) or not algo:
        raise ManifestError("schema missing encryption algorithm const")
    return algo


def assert_schema_code_agreement(schema: dict[str, Any] | None = None) -> None:
    """Fail closed when schema enums/consts drift from code constants."""
    root = schema if schema is not None else load_schema()
    methods = schema_method_enum(root)
    if methods != set(ALLOWED_POSTGRES_METHODS):
        raise ManifestError(
            f"schema/code drift on postgres.method: schema={sorted(methods)} "
            f"code={sorted(ALLOWED_POSTGRES_METHODS)}"
        )
    algo = schema_encryption_algorithm(root)
    if algo != ENCRYPTION_ALGORITHM:
        raise ManifestError(
            f"schema/code drift on encryption algorithm: schema={algo!r} "
            f"code={ENCRYPTION_ALGORITHM!r}"
        )
    kdf = (
        root.get("definitions", {})
        .get("encryptionObject", {})
        .get("properties", {})
        .get("kdf", {})
        .get("const")
    )
    if kdf != ENCRYPTION_KDF:
        raise ManifestError(
            f"schema/code drift on encryption kdf: schema={kdf!r} code={ENCRYPTION_KDF!r}"
        )


def signing_key_from_keyring() -> tuple[str, bytes]:
    """Select HMAC key from trusted keyring JSON or single env pair."""
    keyring_path = os.environ.get("MARKHAND_BACKUP_SIGNING_KEYRING", "").strip()
    expected_id = os.environ.get("MARKHAND_BACKUP_SIGNING_KEY_ID", "").strip()
    if not expected_id:
        raise ManifestError("MARKHAND_BACKUP_SIGNING_KEY_ID required")
    if keyring_path:
        data = load_path(keyring_path)
        if not isinstance(data, dict) or not isinstance(data.get("keys"), dict):
            raise ManifestError("signing keyring must be {keys:{id:hex}}")
        if expected_id not in data["keys"]:
            raise ManifestError(f"expected key id {expected_id} not in trusted keyring")
        key_hex = str(data["keys"][expected_id]).strip()
    else:
        key_hex = os.environ.get("MARKHAND_BACKUP_SIGNING_KEY", "").strip()
    if not SHA256_RE.fullmatch(key_hex):
        raise ManifestError("signing key must be 64 lowercase hex chars")
    return expected_id, bytes.fromhex(key_hex)


def sign_payload(payload: dict[str, Any], key_id: str, key: bytes) -> dict[str, str]:
    body = deepcopy(payload)
    # Sign all metadata except the signature bytes themselves.
    sig_meta = body.get("signature")
    if isinstance(sig_meta, dict):
        body["signature"] = {
            k: v for k, v in sig_meta.items() if k != "value"
        }
    else:
        body.pop("signature", None)
        body["signature"] = {"algorithm": "hmac-sha256", "keyId": key_id}
    digest = hmac.new(key, canonical_json(body), hashlib.sha256).hexdigest()
    return {"algorithm": "hmac-sha256", "keyId": key_id, "value": digest}


def verify_signature(payload: dict[str, Any], key: bytes, expected_key_id: str) -> None:
    signature = payload.get("signature")
    if not isinstance(signature, dict):
        raise ManifestError("missing signature")
    if signature.get("algorithm") != "hmac-sha256":
        raise ManifestError("unsupported signature algorithm")
    if signature.get("keyId") != expected_key_id:
        raise ManifestError("signature.keyId does not match trusted/expected key id")
    expected = signature.get("value")
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ManifestError("signature.value invalid")
    actual = sign_payload(payload, expected_key_id, key)["value"]
    if not hmac.compare_digest(actual, expected):
        raise ManifestError("manifest signature mismatch")


def _forbid_secrets(node: Any) -> None:
    def visitor(path: str, value: Any) -> None:
        if isinstance(value, dict):
            for key in value:
                lowered = key.lower()
                if any(frag in lowered for frag in FORBIDDEN_FRAGMENTS):
                    raise ManifestError(f"forbidden field {path}.{key}")
        if isinstance(value, str) and re.search(
            r"(?i)(postgres(?:ql)?://\S+:\S+@|-----BEGIN )", value
        ):
            raise ManifestError(f"secret-like material at {path}")

    walk(node, visitor)


def _check_encryption(enc: Any, *, where: str, errors: list[str]) -> None:
    if not isinstance(enc, dict):
        errors.append(f"{where} must be encryption object")
        return
    if enc.get("algorithm") != ENCRYPTION_ALGORITHM:
        errors.append(f"{where}.algorithm must be {ENCRYPTION_ALGORITHM}")
    if enc.get("kdf") != ENCRYPTION_KDF:
        errors.append(f"{where}.kdf must be {ENCRYPTION_KDF}")
    for field in (
        "keyId",
        "saltHex",
        "ivHex",
        "macHex",
        "aad",
        "plaintextSha256",
        "ciphertextSha256",
    ):
        if field not in enc:
            errors.append(f"{where} missing {field}")


def validate_structure(payload: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    try:
        assert_schema_code_agreement()
        validate_manifest(payload)
        _forbid_secrets(payload)
    except (SchemaError, ManifestError) as error:
        errors.append(str(error))
        return errors

    # Semantic invariants beyond JSON Schema shape.
    fence = payload.get("consistencyFence")
    if isinstance(fence, dict):
        if fence.get("mode") == "strict-write-fence" and fence.get("writesFenced") is not True:
            errors.append("strict-write-fence requires writesFenced=true")
        if fence.get("mode") == "ordered-bounded" and fence.get("writesFenced") is True:
            errors.append("ordered-bounded cannot set writesFenced=true")

    postgres = payload.get("postgres")
    if isinstance(postgres, dict):
        method = postgres.get("method")
        if method not in ALLOWED_POSTGRES_METHODS:
            errors.append("postgres.method unsupported")
        continuous = postgres.get("continuousPitr") is True
        packaged = postgres.get("archiveWalPackaged") is True
        if method == "pitr_archive":
            if not continuous or not packaged:
                errors.append(
                    "pitr_archive requires continuousPitr=true and archiveWalPackaged=true"
                )
            if not postgres.get("archiveWalRequiredThroughLsn"):
                errors.append("pitr_archive requires archiveWalRequiredThroughLsn")
            if not SHA256_RE.fullmatch(str(postgres.get("archiveWalDigestSha256") or "")):
                errors.append("pitr_archive requires archiveWalDigestSha256")
            _check_encryption(
                postgres.get("archiveWalEncryption"),
                where="postgres.archiveWalEncryption",
                errors=errors,
            )
            if postgres.get("recovery", {}).get("kind") != "archive_pitr":
                errors.append("pitr_archive recovery.kind must be archive_pitr")
        if method == "pg_basebackup_streamed_wal":
            if continuous or packaged:
                errors.append(
                    "streamed_wal must keep continuousPitr=false and archiveWalPackaged=false"
                )
            if postgres.get("archiveWalRequiredThroughLsn") is not None:
                errors.append("streamed_wal must set archiveWalRequiredThroughLsn=null")
            if postgres.get("recovery", {}).get("kind") != "streamed_wal_consistent":
                errors.append("streamed_wal recovery.kind must be streamed_wal_consistent")
        for field in ("startWalLsn", "stopWalLsn", "walBoundaryLsn"):
            if not isinstance(postgres.get(field), str) or not LSN_RE.fullmatch(postgres[field]):
                errors.append(f"postgres.{field} invalid")
        _check_encryption(postgres.get("encryption"), where="postgres.encryption", errors=errors)
        _check_encryption(
            postgres.get("walEncryption"), where="postgres.walEncryption", errors=errors
        )

    minio = payload.get("minio")
    if isinstance(minio, dict):
        if minio.get("versioningEnabled") is not True:
            errors.append("minio.versioningEnabled must be true")
        if minio.get("retainsSourceVersionIdsOnRestore") is not False:
            errors.append("minio must declare retainsSourceVersionIdsOnRestore=false")
        _check_encryption(
            minio.get("inventoryEncryption"),
            where="minio.inventoryEncryption",
            errors=errors,
        )

    qdrant = payload.get("qdrant")
    if isinstance(qdrant, dict):
        if not str(qdrant.get("collectionName", "")).startswith("markhand_chunks_"):
            errors.append("qdrant.collectionName must be canonical markhand_chunks_<digest>")
        if qdrant.get("indexSignatureSha256") != payload.get("indexSignatureSha256"):
            errors.append("qdrant signature mismatch vs top-level")
        if qdrant.get("collectionName") != "markhand_chunks_" + str(
            payload.get("indexSignatureSha256")
        ):
            errors.append("qdrant.collectionName must derive from index signature")

    artifacts = payload.get("artifacts") or {}
    checksums = payload.get("checksums") or {}
    rel = artifacts.get("relativePaths") if isinstance(artifacts, dict) else None
    if not isinstance(rel, dict) or not rel:
        errors.append("artifacts.relativePaths required")
    else:
        for role, path in rel.items():
            if not isinstance(path, str) or not SAFE_REL_RE.fullmatch(path):
                errors.append(f"unsafe artifact path {role}")
            elif path not in checksums:
                errors.append(f"missing checksum for {path}")
    return errors


def verify_artifact_checksums(payload: dict[str, Any], backup_root: Path) -> list[str]:
    errors: list[str] = []
    root = backup_root.resolve()
    rel_paths = (payload.get("artifacts") or {}).get("relativePaths") or {}
    checksums = payload.get("checksums") or {}
    for role, rel in rel_paths.items():
        if not isinstance(rel, str) or not SAFE_REL_RE.fullmatch(rel):
            errors.append(f"path traversal/unsafe artifact path ({role}): {rel!r}")
            continue
        candidate = root / rel
        if candidate.is_symlink():
            errors.append(f"symlink artifact rejected ({role}): {rel}")
            continue
        try:
            resolved = candidate.resolve(strict=False)
            resolved.relative_to(root)
        except ValueError:
            errors.append(f"artifact escapes backup root ({role}): {rel}")
            continue
        if not candidate.is_file():
            errors.append(f"missing artifact ({role}): {rel}")
            continue
        if sha256_file(candidate) != checksums.get(rel):
            errors.append(f"checksum mismatch for {rel}")
    return errors


def build_manifest(**kwargs: Any) -> dict[str, Any]:
    key_id = kwargs.pop("key_id")
    key = kwargs.pop("key")
    payload = {
        "schemaVersion": SCHEMA_VERSION,
        "manifestId": kwargs["manifest_id"],
        "createdAt": utc_now(),
        "orgId": kwargs["org_id"],
        "appVersion": kwargs["app_version"],
        "migrationVersion": kwargs["migration_version"],
        "indexSignatureSha256": kwargs["index_signature"],
        "schemaName": kwargs.get("schema_name", "public"),
        "consistencyFence": kwargs["consistency_fence"],
        "postgres": kwargs["postgres"],
        "minio": kwargs["minio"],
        "qdrant": kwargs["qdrant"],
        "artifacts": {"relativePaths": kwargs["relative_paths"]},
        "checksums": kwargs["checksums"],
        "notes": kwargs.get("notes") or [],
    }
    payload["signature"] = sign_payload(payload, key_id, key)
    errors = validate_structure(payload)
    if errors:
        raise ManifestError("; ".join(errors))
    return payload


def load_and_validate_manifest(
    path: Path,
    *,
    backup_root: Path | None,
    require_runtime_expectations: bool,
    check_signature: bool,
) -> dict[str, Any]:
    try:
        payload = load_path(str(path))
    except StrictJsonError as error:
        raise ManifestError(str(error)) from error
    if not isinstance(payload, dict):
        raise ManifestError("manifest must be object")
    errors = validate_structure(payload)
    if require_runtime_expectations:
        for env_name, field in (
            ("MARKHAND_WORKER_ORG_ID", "orgId"),
            ("MARKHAND_BACKUP_SCHEMA_NAME", "schemaName"),
            ("MARKHAND_INDEX_SIGNATURE", "indexSignatureSha256"),
            ("MARKHAND_BACKUP_MIGRATION_VERSION", "migrationVersion"),
        ):
            expected = os.environ.get(env_name)
            if env_name == "MARKHAND_BACKUP_SCHEMA_NAME" and not expected:
                expected = "public"
            if not expected:
                errors.append(f"runtime expectation missing env {env_name}")
            elif payload.get(field, "public" if field == "schemaName" else None) != expected:
                errors.append(f"{field} mismatch vs {env_name}")
    if check_signature:
        key_id, key = signing_key_from_keyring()
        try:
            verify_signature(payload, key, key_id)
        except ManifestError as error:
            errors.append(str(error))
    if backup_root is not None:
        errors.extend(verify_artifact_checksums(payload, backup_root))
    if errors:
        raise ManifestError("; ".join(errors))
    return payload


def redact_env_for_log(mapping: dict[str, str]) -> dict[str, str]:
    redacted: dict[str, str] = {}
    for key, value in mapping.items():
        lowered = key.lower()
        if any(frag in lowered for frag in FORBIDDEN_FRAGMENTS) or (
            "signing_key" in lowered and not lowered.endswith("key_id") and "keyring" not in lowered
        ):
            redacted[key] = "***REDACTED***"
        else:
            redacted[key] = value
    return redacted


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    validate = sub.add_parser("validate")
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--backup-root", type=Path)
    validate.add_argument("--verify-signature", action="store_true")
    digest = sub.add_parser("inventory-digest")
    digest.add_argument("--input", type=Path)
    agree = sub.add_parser("assert-schema-agreement")
    args = parser.parse_args(argv)
    try:
        if args.command == "inventory-digest":
            lines = (
                args.input.read_text(encoding="utf-8").splitlines()
                if args.input
                else sys.stdin.read().splitlines()
            )
            digest_hex, count = inventory_digest(lines)
            json.dump({"digestSha256": digest_hex, "count": count}, sys.stdout)
            sys.stdout.write("\n")
            return 0
        if args.command == "assert-schema-agreement":
            assert_schema_code_agreement()
            print(f"schema/code agreement ok ({SCHEMA_PATH})")
            return 0
        load_and_validate_manifest(
            args.manifest,
            backup_root=args.backup_root,
            require_runtime_expectations=True,
            check_signature=args.verify_signature,
        )
        print("manifest validation ok")
        return 0
    except (ManifestError, StrictJsonError, OSError) as error:
        print(f"manifest error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    _ = loads
    raise SystemExit(main())
