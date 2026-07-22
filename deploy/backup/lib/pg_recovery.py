#!/usr/bin/env python3
"""Configure and verify streamed-WAL shadow PostgreSQL recovery."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from pg_wal import BackupLabel, PgWalError


class PgRecoveryError(ValueError):
    """Fail-closed recovery error."""


def configure_streamed_recovery(
    shadow: Path,
    *,
    label: BackupLabel,
    wal_dir: Path,
) -> dict[str, Any]:
    """Write recovery.signal + postgresql.auto.conf for streamed WAL recovery."""
    if not shadow.is_dir():
        raise PgRecoveryError("shadow PGDATA missing")
    restore_script = shadow / "markhand_restore_command.sh"
    restore_script.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        f'WAL_DIR="{wal_dir}"\n'
        'src="$WAL_DIR/$1"\n'
        'if [ ! -f "$src" ]; then exit 1; fi\n'
        'cp -- "$src" "$2"\n',
        encoding="utf-8",
    )
    restore_script.chmod(0o700)
    auto = shadow / "postgresql.auto.conf"
    conf = (
        f"restore_command = '{restore_script} %f %p'\n"
        f"recovery_target_lsn = '{label.stop_lsn}'\n"
        "recovery_target_action = 'promote'\n"
        f"recovery_target_timeline = '{label.timeline_id}'\n"
    )
    auto.write_text(conf, encoding="utf-8")
    (shadow / "recovery.signal").write_text("", encoding="utf-8")
    # Remove stale standby.signal if present.
    standby = shadow / "standby.signal"
    if standby.exists():
        standby.unlink()
    return {
        "restoreCommand": str(restore_script),
        "recoveryTargetLsn": label.stop_lsn,
        "recoveryTargetTimeline": label.timeline_id,
        "recoverySignal": True,
        "postgresqlAutoConf": conf,
    }


def start_and_verify_shadow(
    shadow: Path,
    *,
    label: BackupLabel,
    postgres_image: str,
    state_dir: Path,
) -> dict[str, Any]:
    """Start shadow via pinned postgres tool/container path and verify recovery.

    Prefer ``MARKHAND_BACKUP_PG_CTL`` (stateful fake or real pg_ctl). Else use
    docker run with the pinned image. Fail closed if neither is available.
    """
    state_dir.mkdir(parents=True, exist_ok=True)
    tool = os.environ.get("MARKHAND_BACKUP_PG_CTL", "").strip()
    if tool:
        return _run_pg_ctl(tool, shadow, label, state_dir)
    if shutil.which("docker") is None:
        raise PgRecoveryError(
            "shadow recovery requires MARKHAND_BACKUP_PG_CTL or docker with pinned postgres image"
        )
    # Docker path — only when daemon works.
    info = subprocess.run(
        ["docker", "info"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if info.returncode != 0:
        raise PgRecoveryError("docker unavailable for shadow postgres recovery")
    return _run_docker_postgres(postgres_image, shadow, label, state_dir)


def _run_pg_ctl(
    tool: str,
    shadow: Path,
    label: BackupLabel,
    state_dir: Path,
) -> dict[str, Any]:
    env = os.environ.copy()
    env["MARKHAND_SHADOW_PGDATA"] = str(shadow)
    env["MARKHAND_RECOVERY_TARGET_LSN"] = label.stop_lsn
    env["MARKHAND_RECOVERY_TIMELINE"] = str(label.timeline_id)
    completed = subprocess.run(
        [tool, "start-and-verify", str(shadow)],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise PgRecoveryError(
            f"pg recovery tool failed: {completed.stderr.strip() or completed.returncode}"
        )
    try:
        result = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise PgRecoveryError(f"pg recovery tool returned non-JSON: {error}") from error
    _assert_recovery_result(result, label)
    (state_dir / "pg-ctl-output.json").write_text(
        json.dumps(result, indent=2) + "\n", encoding="utf-8"
    )
    return result


def _run_docker_postgres(
    image: str,
    shadow: Path,
    label: BackupLabel,
    state_dir: Path,
) -> dict[str, Any]:
    name = f"markhand-shadow-{os.getpid()}"
    run = subprocess.run(
        [
            "docker",
            "run",
            "-d",
            "--name",
            name,
            "-v",
            f"{shadow}:/var/lib/postgresql/data",
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            image,
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if run.returncode != 0:
        raise PgRecoveryError(f"docker run postgres failed: {run.stderr.strip()}")
    try:
        # Wait for readiness then query timeline/LSN.
        for _ in range(60):
            probe = subprocess.run(
                [
                    "docker",
                    "exec",
                    name,
                    "psql",
                    "-U",
                    "postgres",
                    "-At",
                    "-c",
                    "SELECT pg_is_in_recovery(), pg_wal_lsn_diff(pg_current_wal_lsn(),"
                    f"'{label.stop_lsn}'::pg_lsn) IS NOT NULL;",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            if probe.returncode == 0 and probe.stdout.strip():
                break
            time.sleep(1)
        else:
            raise PgRecoveryError("shadow postgres did not become ready")
        tl = subprocess.run(
            [
                "docker",
                "exec",
                name,
                "psql",
                "-U",
                "postgres",
                "-At",
                "-c",
                "SELECT timeline_id FROM pg_control_checkpoint();",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if tl.returncode != 0:
            raise PgRecoveryError("failed to read shadow timeline")
        timeline = int(tl.stdout.strip().splitlines()[0])
        result = {
            "recovered": True,
            "timelineId": timeline,
            "recoveryTargetLsn": label.stop_lsn,
            "inRecovery": False,
            "tool": "docker",
            "image": image,
        }
        _assert_recovery_result(result, label)
        return result
    finally:
        subprocess.run(
            ["docker", "rm", "-f", name],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )


def _assert_recovery_result(result: dict[str, Any], label: BackupLabel) -> None:
    if result.get("recovered") is not True:
        raise PgRecoveryError("shadow recovery did not report recovered=true")
    if int(result.get("timelineId") or 0) != label.timeline_id:
        raise PgRecoveryError(
            f"timeline mismatch: got {result.get('timelineId')} want {label.timeline_id}"
        )
    if result.get("recoveryTargetLsn") != label.stop_lsn:
        raise PgRecoveryError("recovery target LSN mismatch")
    if result.get("inRecovery") is True:
        raise PgRecoveryError("shadow still in recovery after promote target")


def assert_recovery_files(shadow: Path) -> None:
    if not (shadow / "recovery.signal").is_file() and not (
        shadow / "postgresql.auto.conf"
    ).is_file():
        raise PgRecoveryError("recovery configuration missing on shadow PGDATA")
    auto = shadow / "postgresql.auto.conf"
    if auto.is_file():
        text = auto.read_text(encoding="utf-8")
        if "restore_command" not in text:
            raise PgWalError("postgresql.auto.conf missing restore_command")
        if "recovery_target_lsn" not in text:
            raise PgWalError("postgresql.auto.conf missing recovery_target_lsn")
