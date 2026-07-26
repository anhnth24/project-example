#!/usr/bin/env python3
"""Independent, secret-free target-state attestation for O03 green restores."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlparse
from urllib.request import Request, urlopen

from manifest import load_authenticated_manifest, verify_artifacts
from pg_session import private_pg_env


class GreenAttestationError(ValueError):
    """Raised when target state is insufficient to clear the restore fence."""


REQUIRED_CHECKS = frozenset(
    {
        "manifestAuthenticated",
        "postgresConsistent",
        "minioConsistent",
        "qdrantConsistent",
        "crossStoreRefsConsistent",
        "restoreFenceMatches",
    }
)


def _canonical_bytes(value: dict[str, Any]) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode("utf-8")


def build_attestation(
    *,
    manifest_sha256: str,
    fence_epoch: str,
    target: dict[str, str],
    checks: dict[str, bool],
    verified_at_epoch: int,
) -> tuple[dict[str, Any], str]:
    """Build a deterministic attestation only when every required check passed."""
    if not re.fullmatch(r"[a-f0-9]{64}", manifest_sha256):
        raise GreenAttestationError("manifest digest invalid")
    if not re.fullmatch(r"[0-9a-f-]{36}", fence_epoch):
        raise GreenAttestationError("fence epoch invalid")
    if not isinstance(verified_at_epoch, int) or verified_at_epoch < 0:
        raise GreenAttestationError("verification epoch invalid")
    if any(checks.get(name) is not True for name in REQUIRED_CHECKS):
        raise GreenAttestationError("independent target-state checks incomplete")
    required_target = {
        "pgDatabase",
        "minioEndpoint",
        "minioBucket",
        "qdrantUrl",
        "qdrantCollection",
    }
    if set(target) != required_target or any(not str(value) for value in target.values()):
        raise GreenAttestationError("green target identity incomplete")

    payload: dict[str, Any] = {
        "schemaVersion": 1,
        "kind": "markhand.green-target-state",
        "manifestSha256": manifest_sha256,
        "fenceEpoch": fence_epoch,
        "target": dict(sorted(target.items())),
        "checks": {name: True for name in sorted(REQUIRED_CHECKS)},
        "verifiedAtEpoch": verified_at_epoch,
    }
    digest = hashlib.sha256(_canonical_bytes(payload)).hexdigest()
    return payload, digest


def clear_fence_sql() -> str:
    """Return the compare-and-clear SQL used after independent verification."""
    return """
UPDATE ops_fences
SET active = false,
    cleared_at = clock_timestamp(),
    attestation_sha256 = :'digest'
WHERE name = 'restore'
  AND active = true
  AND reason LIKE '%fence_epoch=' || :'epoch' || '%'
RETURNING name;
""".strip()


def _psql(database_url: str, sql: str, variables: dict[str, str] | None = None) -> str:
    with private_pg_env(database_url) as (safe_url, env):
        argv = ["psql", safe_url, "-v", "ON_ERROR_STOP=1", "-Atq"]
        for name, value in sorted((variables or {}).items()):
            argv.extend(["-v", f"{name}={value}"])
        proc = subprocess.run(
            argv,
            input=sql.rstrip() + "\n",
            check=True,
            capture_output=True,
            text=True,
            env=env,
        )
    return proc.stdout.strip()


def _qdrant_payloads(base_url: str, collection: str) -> list[dict[str, Any]]:
    payloads: list[dict[str, Any]] = []
    offset: Any = None
    while True:
        body: dict[str, Any] = {
            "limit": 100,
            "with_payload": True,
            "with_vector": False,
        }
        if offset is not None:
            body["offset"] = offset
        request = Request(
            f"{base_url}/collections/{collection}/points/scroll",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        response = json.loads(urlopen(request, timeout=60).read())
        result = response.get("result") or {}
        batch = result.get("points") or []
        payloads.extend(
            {"id": point.get("id"), "payload": point.get("payload") or {}}
            for point in batch
        )
        offset = result.get("next_page_offset")
        if not offset or not batch:
            break
    return payloads


def _safe_endpoint(value: str) -> str:
    parsed = urlparse(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise GreenAttestationError("green endpoint invalid")
    if parsed.username or parsed.password:
        raise GreenAttestationError("green endpoint must not contain credentials")
    return value.rstrip("/")


def verify_and_clear(backup_dir: Path, output: Path) -> tuple[dict[str, Any], str]:
    """Independently re-read green stores, attest them, then compare-and-clear the fence."""
    manifest, raw_manifest = load_authenticated_manifest(backup_dir)
    verify_artifacts(backup_dir, manifest)
    manifest_sha = hashlib.sha256(raw_manifest).hexdigest()
    fence_epoch = str(manifest.get("fenceEpoch") or "")

    database_url = os.environ["MARKHAND_GREEN_DATABASE_URL"]
    minio_endpoint = _safe_endpoint(
        os.environ.get("MARKHAND_GREEN_MINIO_ENDPOINT")
        or os.environ["MINIO_ENDPOINT"]
    )
    minio_bucket = os.environ["MARKHAND_GREEN_MINIO_BUCKET"]
    qdrant_url = _safe_endpoint(
        os.environ.get("MARKHAND_GREEN_QDRANT_URL") or os.environ["QDRANT_URL"]
    )
    qdrant_collection = os.environ["MARKHAND_GREEN_QDRANT_COLLECTION"]
    pg_database = (urlparse(database_url).path or "").lstrip("/")

    if qdrant_collection != (manifest.get("qdrant") or {}).get("collection"):
        raise GreenAttestationError(
            "green Qdrant collection must preserve the source runtime identity"
        )

    migrations = _psql(
        database_url,
        "SELECT coalesce(string_agg(name, ',' ORDER BY name), '') "
        "FROM markhand_schema_migrations;",
    )
    expected_migrations = ",".join((manifest.get("postgres") or {}).get("migrations") or [])
    if migrations != expected_migrations:
        raise GreenAttestationError("green migration set differs from manifest")

    fence = _psql(
        database_url,
        "SELECT active::text || '|' || reason FROM ops_fences WHERE name='restore';",
    )
    restore_fence_matches = fence.startswith("true|") and f"fence_epoch={fence_epoch}" in fence
    if not restore_fence_matches:
        raise GreenAttestationError("green restore fence does not match backup epoch")

    signature = qdrant_collection.removeprefix("markhand_chunks_")
    if not re.fullmatch(r"[a-f0-9]{64}", signature):
        raise GreenAttestationError("green Qdrant collection identity invalid")
    active_generations = _psql(
        database_url,
        "SELECT count(*) FROM index_metadata "
        "WHERE is_active=true AND state='active' "
        "AND index_signature_sha256=:'signature';",
        {"signature": signature},
    )
    if int(active_generations or "0") < 1:
        raise GreenAttestationError("green database lacks matching active index generation")

    qdrant_info = json.loads(
        urlopen(
            f"{qdrant_url}/collections/{qdrant_collection}",
            timeout=60,
        ).read()
    )
    qdrant_points = int((qdrant_info.get("result") or {}).get("points_count") or 0)
    expected_points = int((manifest.get("qdrant") or {}).get("pointsCount") or 0)
    payloads = _qdrant_payloads(qdrant_url, qdrant_collection)
    payload_sha = hashlib.sha256(
        json.dumps(payloads, sort_keys=True).encode()
    ).hexdigest()
    qdrant_consistent = (
        qdrant_points == expected_points
        and len(payloads) == expected_points
        and payload_sha == (manifest.get("qdrant") or {}).get("payloadRefSha256")
    )
    if not qdrant_consistent:
        raise GreenAttestationError("green Qdrant state differs from manifest")

    for point in payloads:
        payload = point.get("payload") or {}
        org_id = str(payload.get("org_id") or "")
        document_id = str(payload.get("document_id") or "")
        version_id = str(payload.get("version_id") or "")
        chunk_id = str(payload.get("chunk_id") or "")
        if not all(
            re.fullmatch(r"[0-9a-f-]{36}", value)
            for value in (org_id, document_id, version_id)
        ) or not re.fullmatch(r"[a-f0-9]{64}", chunk_id):
            raise GreenAttestationError("Qdrant point payload identity malformed")
        count = _psql(
            database_url,
            "SELECT count(*) FROM chunks "
            "WHERE org_id=:'org'::uuid AND document_id=:'doc'::uuid "
            "AND version_id=:'version'::uuid AND chunk_identity_sha256=:'chunk';",
            {
                "org": org_id,
                "doc": document_id,
                "version": version_id,
                "chunk": chunk_id,
            },
        )
        if count != "1":
            raise GreenAttestationError("Qdrant point lacks matching green chunk")

    from pipeline import green_mc_env_for, run

    mc_env = green_mc_env_for(minio_endpoint)
    history = json.loads(
        (backup_dir / "minio-normalized-history.json").read_text(encoding="utf-8")
    ).get("keys") or []
    live_object_hashes: dict[str, str] = {}
    for key_entry in history:
        events = key_entry.get("events") or []
        if not events or events[-1].get("type") != "put":
            continue
        key = str(key_entry.get("key") or "")
        data = run(["mc", "cat", f"markhand/{minio_bucket}/{key}"], env=mc_env)
        digest = hashlib.sha256(data).hexdigest()
        if digest != events[-1].get("contentSha256"):
            raise GreenAttestationError("green MinIO live object differs from manifest")
        live_object_hashes[key] = digest

    cross_store_consistent = True
    for ref in manifest.get("crossStoreRefs") or []:
        document_id = str(ref.get("documentId") or "")
        version_id = str(ref.get("versionId") or "")
        object_key = str(ref.get("objectKey") or "")
        object_sha = str(ref.get("objectSha256") or "")
        if (
            not re.fullmatch(r"[0-9a-f-]{36}", document_id)
            or not re.fullmatch(r"[0-9a-f-]{36}", version_id)
            or not re.fullmatch(r"[A-Za-z0-9._/-]+", object_key)
            or not re.fullmatch(r"[a-f0-9]{64}", object_sha)
        ):
            cross_store_consistent = False
            break
        row = _psql(
            database_url,
            "SELECT count(*) FROM document_versions "
            "WHERE document_id=:'doc'::uuid AND id=:'version'::uuid "
            "AND original_object_key=:'key' AND content_sha256=:'sha';",
            {
                "doc": document_id,
                "version": version_id,
                "key": object_key,
                "sha": object_sha,
            },
        )
        if row != "1" or live_object_hashes.get(object_key) != object_sha:
            cross_store_consistent = False
            break
    if not cross_store_consistent:
        raise GreenAttestationError("cross-store reference verification failed")

    target = {
        "pgDatabase": pg_database,
        "minioEndpoint": minio_endpoint,
        "minioBucket": minio_bucket,
        "qdrantUrl": qdrant_url,
        "qdrantCollection": qdrant_collection,
    }
    checks = {
        "manifestAuthenticated": True,
        "postgresConsistent": True,
        "minioConsistent": True,
        "qdrantConsistent": True,
        "crossStoreRefsConsistent": True,
        "restoreFenceMatches": True,
    }
    attestation, digest = build_attestation(
        manifest_sha256=manifest_sha,
        fence_epoch=fence_epoch,
        target=target,
        checks=checks,
        verified_at_epoch=int(time.time()),
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(_canonical_bytes(attestation))
    os.chmod(output, 0o600)

    cleared = _psql(
        database_url,
        clear_fence_sql(),
        {"epoch": fence_epoch, "digest": digest},
    )
    if cleared != "restore":
        raise GreenAttestationError("restore fence compare-and-clear failed")
    return attestation, digest


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(
            "usage: green_attestation.py <backup-dir> <attestation-output>",
            file=sys.stderr,
        )
        return 2
    try:
        _attestation, digest = verify_and_clear(Path(argv[1]), Path(argv[2]))
    except (GreenAttestationError, KeyError, OSError, ValueError, subprocess.SubprocessError) as exc:
        print(f"green_attestation_error: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({"status": "GREEN_TARGET_ATTESTED", "sha256": digest}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
