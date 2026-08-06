#!/usr/bin/env python3
"""Validated identities, effective config, and runtime JSON for real E2E fixtures."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import string
import tempfile
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

KEY_DOMAIN = b"markhand-object-key-v1"
DEFAULT_OPERATION_TIMEOUT_SECS = 30.0
MAX_RUN_ID_LENGTH = 56

RUN_ID_RE = re.compile(
    r"^(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|"
    r"e2e-[a-f0-9]{12,40}-(?:0|[1-9][0-9]{0,9}))$",
    re.IGNORECASE,
)
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)
IDENTIFIER_RE = re.compile(r"^[a-z][a-z0-9_]{0,62}$")
SIGNATURE_RE = re.compile(r"^[a-f0-9]{64}$")

SENSITIVE_MANIFEST_KEYS = {
    "adminpassword",
    "viewerpassword",
    "password",
    "passwordhash",
    "accesstoken",
    "refreshtoken",
    "objectkeys",
    "objectkey",
    "token",
    "secret",
}

# Mirrors crates/server/src/config.rs ConfigFile (deny_unknown_fields). The fixture
# only consumes a subset, but must still fail closed when the server would reject
# the selected config file.
_CONFIG_STRING_KEYS = {
    "profile",
    "bindAddr",
    "databaseUrl",
    "workerDatabaseUrl",
    "qdrantUrl",
    "qdrantApiKey",
    "minioUrl",
    "minioAccessKey",
    "minioSecretKey",
    "minioBucket",
    "minioRegion",
    "authIssuer",
    "authAudience",
    "authSigningKey",
    "authAlg",
    "authKid",
    "indexSignature",
}
_CONFIG_INTEGER_KEYS = {
    "minioOperationTimeoutSecs",
    "authAccessTokenTtlSecs",
    "authRefreshTokenTtlSecs",
    "authArgon2MemoryKib",
    "authArgon2TimeCost",
    "authArgon2Parallelism",
    "maxUploadBytes",
    "jobLeaseSeconds",
    "maxArchiveEntries",
    "maxArchiveUncompressedBytes",
    "maxArchiveCompressionRatio",
    "maxPdfPages",
    "maxImagePixels",
    "maxAudioDurationSecs",
    "maxMultipartParts",
    "maxPartHeaderBytes",
    "uploadTimeoutSecs",
    "uploadIdleTimeoutSecs",
    "quotaSweepIntervalSecs",
    "quotaSweepBatchSize",
}
_CONFIG_BOOLEAN_KEYS = {"minioPathStyle"}
_CONFIG_KEYS = _CONFIG_STRING_KEYS | _CONFIG_INTEGER_KEYS | _CONFIG_BOOLEAN_KEYS

_ENV_TO_FILE = {
    "MARKHAND_PROFILE": "profile",
    "MARKHAND_QDRANT_URL": "qdrantUrl",
    "MARKHAND_QDRANT_API_KEY": "qdrantApiKey",
    "MARKHAND_MINIO_URL": "minioUrl",
    "MARKHAND_MINIO_ACCESS_KEY": "minioAccessKey",
    "MARKHAND_MINIO_SECRET_KEY": "minioSecretKey",
    "MARKHAND_MINIO_BUCKET": "minioBucket",
    "MARKHAND_MINIO_REGION": "minioRegion",
    "MARKHAND_MINIO_PATH_STYLE": "minioPathStyle",
    "MARKHAND_INDEX_SIGNATURE": "indexSignature",
}


class FixtureError(RuntimeError):
    """Fail-closed fixture error; messages must never contain secrets."""


class FixtureLeakError(FixtureError):
    """A confirmed non-clean state with identifier-only evidence."""

    def __init__(self, message: str, leaks: dict[str, Any]) -> None:
        super().__init__(message)
        self.leaks = sanitize_leaks(leaks)


class FixtureProbeError(FixtureError):
    """A storage probe could not distinguish absence from backend failure."""


@dataclass(frozen=True)
class EffectiveConfig:
    environ: Mapping[str, str]
    file_values: Mapping[str, Any]
    profile: str

    def value(self, env_name: str, default: Any = None) -> Any:
        raw = self.environ.get(env_name)
        if raw is not None and str(raw).strip():
            return raw
        file_name = _ENV_TO_FILE.get(env_name)
        if file_name is not None and file_name in self.file_values:
            return self.file_values[file_name]
        return default


def _load_config_file(path: Path) -> dict[str, Any]:
    try:
        source = path.read_text(encoding="utf-8")
    except OSError as error:
        raise FixtureError("cannot read MARKHAND_CONFIG_FILE") from error
    try:
        payload = json.loads(source)
    except json.JSONDecodeError as error:
        raise FixtureError("MARKHAND_CONFIG_FILE contains invalid JSON") from error
    if not isinstance(payload, dict) or any(key not in _CONFIG_KEYS for key in payload):
        raise FixtureError("MARKHAND_CONFIG_FILE contains invalid JSON")
    for key, value in payload.items():
        valid = (
            (key in _CONFIG_STRING_KEYS and isinstance(value, str))
            or (
                key in _CONFIG_INTEGER_KEYS
                and isinstance(value, int)
                and not isinstance(value, bool)
                and value >= 0
            )
            or (key in _CONFIG_BOOLEAN_KEYS and isinstance(value, bool))
        )
        if not valid:
            raise FixtureError("MARKHAND_CONFIG_FILE contains invalid JSON")
    return payload


def load_effective_config(environ: Mapping[str, str]) -> EffectiveConfig:
    config_path = (environ.get("MARKHAND_CONFIG_FILE") or "").strip()
    file_values = _load_config_file(Path(config_path)) if config_path else {}
    raw_profile = environ.get("MARKHAND_PROFILE")
    if raw_profile is None:
        raw_profile = file_values.get("profile", "dev")
    if not isinstance(raw_profile, str):
        raise FixtureError("invalid MARKHAND_PROFILE")
    profile = raw_profile.strip().lower()
    if profile not in {"dev", "test", "prod"}:
        raise FixtureError("invalid MARKHAND_PROFILE")
    return EffectiveConfig(dict(environ), file_values, profile)


def is_production_profile(environ: Mapping[str, str]) -> bool:
    return load_effective_config(environ).profile == "prod"


def refuse_production(environ: Mapping[str, str]) -> EffectiveConfig:
    config = load_effective_config(environ)
    if config.profile == "prod":
        raise FixtureError("refusing effective production profile")
    return config


def fixture_checksum(ids: list[str]) -> str:
    joined = "\n".join(sorted(ids))
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()


def validate_uuid(value: str, *, field: str) -> str:
    text = (value or "").strip()
    if not UUID_RE.fullmatch(text):
        raise FixtureError(f"invalid {field}")
    return text.lower()


def validate_run_id(run_id: str) -> str:
    value = (run_id or "").strip()
    if len(value) > MAX_RUN_ID_LENGTH or not RUN_ID_RE.fullmatch(value):
        raise FixtureError("invalid run-id")
    return value.lower()


def validate_signature(value: str) -> str:
    text = (value or "").strip()
    if not SIGNATURE_RE.fullmatch(text):
        raise FixtureError("invalid index signature")
    return text


def opaque_identity(kind: str, identity: str) -> str:
    value = validate_uuid(identity, field=kind)
    hasher = hashlib.sha256()
    hasher.update(KEY_DOMAIN)
    hasher.update(kind.encode("utf-8"))
    hasher.update(uuid.UUID(value).bytes)
    return hasher.hexdigest()


def quarantine_object_key(org_id: str, object_id: str) -> str:
    org = validate_uuid(org_id, field="orgId")
    obj = validate_uuid(object_id, field="objectId")
    return f"quarantine/{opaque_identity('org', org)}/{uuid.UUID(obj).hex}"


def _run_suffix(run_id: str) -> str:
    value = validate_run_id(run_id)
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def _slug_for_run(run_id: str, namespace: str = "run") -> str:
    value = validate_run_id(run_id)
    safe_namespace = re.sub(r"[^a-z0-9]+", "-", namespace.lower()).strip("-")
    if not safe_namespace:
        raise FixtureError("invalid slug namespace")
    suffix = _run_suffix(value)
    readable = re.sub(r"[^a-z0-9]+", "-", value).strip("-")
    available = 63 - len("e2e---") - len(safe_namespace) - len(suffix)
    readable = readable[: max(1, available)].rstrip("-") or "run"
    return f"e2e-{safe_namespace}-{readable}-{suffix}"


def _email_for_run(actor: str, run_id: str) -> str:
    if actor not in {"admin", "viewer"}:
        raise FixtureError("invalid fixture actor")
    return f"{actor}+{_run_suffix(run_id)}@example.test"


def _generate_password(length: int = 24) -> str:
    alphabet = string.ascii_letters + string.digits
    return "".join(secrets.choice(alphabet) for _ in range(length))


def _sql_quote_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _sql_uuid(value: str, *, field: str) -> str:
    return _sql_quote_literal(validate_uuid(value, field=field))


def _paths_alias(left: Path, right: Path) -> bool:
    try:
        if left.exists() and right.exists() and os.path.samefile(left, right):
            return True
    except OSError:
        pass
    return left.expanduser().resolve(strict=False) == right.expanduser().resolve(strict=False)


def _atomic_write_json(path: Path, payload: dict[str, Any], *, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=path.name + ".", dir=str(path.parent))
    tmp_path = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(tmp_path, mode)
        os.replace(tmp_path, path)
        os.chmod(path, mode)
    finally:
        tmp_path.unlink(missing_ok=True)


def _remove_credentials(path: Path) -> None:
    try:
        path.unlink(missing_ok=True)
    except OSError as error:
        raise FixtureError("failed to remove credentials file") from error


def _load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise FixtureError("invalid json file") from error
    if not isinstance(payload, dict):
        raise FixtureError("invalid json object")
    return payload


def _assert_manifest_public(payload: dict[str, Any]) -> None:
    for key in payload:
        if key.lower() in SENSITIVE_MANIFEST_KEYS:
            raise FixtureError("manifest contains sensitive key")


def _manifest_ids(manifest: dict[str, Any]) -> dict[str, Any]:
    run_id = validate_run_id(str(manifest.get("runId", "")))
    org_id = validate_uuid(str(manifest.get("orgId", "")), field="orgId")
    admin_user_id = validate_uuid(str(manifest.get("adminUserId", "")), field="adminUserId")
    viewer_user_id = validate_uuid(str(manifest.get("viewerUserId", "")), field="viewerUserId")
    collection_id = validate_uuid(str(manifest.get("collectionId", "")), field="collectionId")
    failed_document_id = validate_uuid(
        str(manifest.get("failedDocumentId", "")), field="failedDocumentId"
    )
    failed_version_id = validate_uuid(
        str(manifest.get("failedVersionId", "")), field="failedVersionId"
    )
    object_ids = [
        validate_uuid(str(item), field="objectId") for item in (manifest.get("objectIds") or [])
    ]
    vector_point_ids = [
        validate_uuid(str(item), field="vectorPointId")
        for item in (manifest.get("vectorPointIds") or [])
    ]
    expected = fixture_checksum(
        [
            org_id,
            admin_user_id,
            viewer_user_id,
            collection_id,
            failed_document_id,
            failed_version_id,
            *object_ids,
            *vector_point_ids,
        ]
    )
    checksum = str(manifest.get("checksum", ""))
    if checksum != expected:
        raise FixtureError("manifest checksum mismatch")
    return {
        "runId": run_id,
        "orgId": org_id,
        "adminUserId": admin_user_id,
        "viewerUserId": viewer_user_id,
        "collectionId": collection_id,
        "failedDocumentId": failed_document_id,
        "failedVersionId": failed_version_id,
        "objectIds": object_ids,
        "vectorPointIds": vector_point_ids,
        "collectionName": str(manifest.get("collectionName", "")),
        "checksum": checksum,
    }


def sanitize_leaks(leaks: dict[str, Any]) -> dict[str, Any]:
    """Allow only structural names and UUID identifier values in leak evidence."""

    def sanitize(value: Any, *, key: str = "") -> Any:
        if isinstance(value, dict):
            clean: dict[str, Any] = {}
            for child_key, child in sorted(value.items()):
                if not isinstance(child_key, str) or not IDENTIFIER_RE.fullmatch(child_key):
                    raise FixtureError("invalid leak evidence key")
                cleaned = sanitize(child, key=child_key)
                if cleaned not in ({}, []):
                    clean[child_key] = cleaned
            return clean
        if isinstance(value, list):
            clean_items = [sanitize(item, key=key) for item in value]
            if all(isinstance(item, str) for item in clean_items):
                return sorted(set(clean_items))
            return clean_items
        if isinstance(value, str):
            return validate_uuid(value, field="leakId")
        raise FixtureError("invalid leak evidence value")

    cleaned = sanitize(leaks)
    if not isinstance(cleaned, dict):
        raise FixtureError("invalid leak evidence")
    return cleaned
