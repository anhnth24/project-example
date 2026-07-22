#!/usr/bin/env python3
"""Restore campaign identity, atomic checkpoints, and cutover receipts."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from pathlib import Path
from typing import Any

from strictjson import StrictJsonError, load_path, loads


class CampaignError(ValueError):
    """Fail-closed campaign/checkpoint error."""


STAGE_POSTCONDITIONS = {
    "fenced": ("restore-fence.json",),
    "postgres-restored": ("postgres-postcheck.json", "pg-recovery-state.json"),
    "minio-restored": ("minio-version-mapping.json",),
    "qdrant-restored": ("qdrant-shadow.json",),
    "cutover-complete": ("cutover-receipt.json",),
    "reconciled": ("reconcile.json",),
}


def campaign_identity(
    *,
    manifest: dict[str, Any],
    manifest_path: Path,
    backup_root: Path,
    target_state: Path,
    environment: dict[str, str],
) -> dict[str, Any]:
    env_fp = {
        "orgId": environment.get("MARKHAND_WORKER_ORG_ID", ""),
        "schemaName": environment.get("MARKHAND_BACKUP_SCHEMA_NAME", "public"),
        "indexSignature": environment.get("MARKHAND_INDEX_SIGNATURE", ""),
        "migrationVersion": environment.get("MARKHAND_BACKUP_MIGRATION_VERSION", ""),
        "appVersion": environment.get("MARKHAND_BACKUP_APP_VERSION", ""),
    }
    identity = {
        "manifestId": manifest["manifestId"],
        "manifestSha256": _sha256_file(manifest_path),
        "orgId": manifest["orgId"],
        "appVersion": manifest["appVersion"],
        "migrationVersion": manifest["migrationVersion"],
        "indexSignatureSha256": manifest["indexSignatureSha256"],
        "backupRoot": str(backup_root.resolve()),
        "targetState": str(target_state.resolve()),
        "environment": env_fp,
    }
    blob = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    identity["campaignId"] = hashlib.sha256(blob).hexdigest()
    return identity


def assert_campaign_match(target_state: Path, identity: dict[str, Any]) -> None:
    path = target_state / "campaign-identity.json"
    if not path.is_file():
        return
    try:
        existing = load_path(str(path))
    except StrictJsonError as error:
        raise CampaignError(f"campaign identity unreadable: {error}") from error
    for key in (
        "campaignId",
        "manifestSha256",
        "orgId",
        "appVersion",
        "migrationVersion",
        "indexSignatureSha256",
        "backupRoot",
        "targetState",
    ):
        if existing.get(key) != identity.get(key):
            raise CampaignError(
                f"campaign identity mismatch on {key}: refuse checkpoint trust"
            )


def write_campaign_identity(target_state: Path, identity: dict[str, Any]) -> None:
    target_state.mkdir(parents=True, exist_ok=True)
    _atomic_json(target_state / "campaign-identity.json", identity)


def read_stage(target_state: Path) -> str:
    path = target_state / "stage"
    if not path.is_file():
        return "none"
    return path.read_text(encoding="utf-8").strip() or "none"


def write_stage(target_state: Path, stage: str) -> None:
    """Atomic checkpoint write + postcondition revalidation."""
    if stage != "none":
        required = STAGE_POSTCONDITIONS.get(stage, ())
        missing = [name for name in required if not (target_state / name).is_file()]
        if missing:
            raise CampaignError(
                f"refusing stage={stage}: missing postcondition artifacts {missing}"
            )
    target_state.mkdir(parents=True, exist_ok=True)
    tmp = target_state / f".stage.{os.getpid()}.tmp"
    tmp.write_text(stage + "\n", encoding="utf-8")
    with tmp.open("r+b") as handle:
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp, target_state / "stage")
    # Re-read and revalidate.
    if read_stage(target_state) != stage:
        raise CampaignError("stage checkpoint readback mismatch")
    if stage != "none":
        for name in STAGE_POSTCONDITIONS.get(stage, ()):
            if not (target_state / name).is_file():
                raise CampaignError(f"stage postcondition vanished: {name}")


def write_cutover_receipt(target_state: Path, receipt: dict[str, Any]) -> None:
    if not receipt.get("operations"):
        raise CampaignError("cutover receipt requires actual operations")
    if not receipt.get("reversible"):
        raise CampaignError("cutover receipt must declare reversible strategy")
    _atomic_json(target_state / "cutover-receipt.json", receipt)


def write_rollback_receipt(target_state: Path, receipt: dict[str, Any]) -> None:
    if not receipt.get("operations"):
        raise CampaignError("rollback receipt requires actual operations")
    _atomic_json(target_state / "rollback-receipt.json", receipt)


def _atomic_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=".atomic-", dir=str(path.parent))
    tmp_path = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_path, path)
        path.chmod(0o600)
    finally:
        if tmp_path.exists():
            tmp_path.unlink(missing_ok=True)


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
