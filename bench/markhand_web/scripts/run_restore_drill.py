#!/usr/bin/env python3
"""P0-10 offline restore drill smoke.

This harness uses recorded spike lifecycle evidence and deterministic synthetic
timings. It validates restore ordering and marker emission only; it does not
claim Profile B G0-DR evidence.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
WORKLOAD_PROFILE = CORPUS / "workload-profile.yaml"
SPIKE_ENVIRONMENT = CORPUS / "reports/spike-environment.json"
SUMMARY_PATH = CORPUS / "restore/summary.json"
REPORT_PATH = CORPUS / "reports/restore-drill.md"
SEED = 20260718
DOES_NOT_CLAIM = "does NOT claim G0-DR Profile B pass evidence"
IMPLEMENTATION_FILES = (
    "bench/markhand_web/scripts/run_restore_drill.py",
    "docs/adr/0012-backup-recovery-order.md",
)


class HarnessError(RuntimeError):
    """Actionable restore-drill error."""


def load_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{path}: cannot load JSON: {error}") from error
    if not isinstance(payload, dict):
        raise HarnessError(f"{path}: expected JSON object")
    return payload


def git(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_status() -> dict:
    raw = ""
    try:
        raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    except (OSError, subprocess.CalledProcessError):
        pass
    dirty_paths: list[str] = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {
        "commit": git("rev-parse", "HEAD"),
        "branch": git("branch", "--show-current"),
        "dirty": bool(dirty_paths),
        "dirtyPaths": dirty_paths,
    }


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for relative in IMPLEMENTATION_FILES:
        path = ROOT / relative
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes() if path.is_file() else b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def stable_unit(seed: int, *parts: object) -> float:
    data = "|".join([str(seed), *(str(part) for part in parts)]).encode()
    value = int.from_bytes(hashlib.sha256(data).digest()[:8], "big")
    return value / float(2**64 - 1)


def docker_available() -> bool:
    docker = shutil.which("docker")
    if not docker:
        return False
    try:
        completed = subprocess.run(
            [docker, "info"],
            cwd=ROOT,
            text=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=3,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return completed.returncode == 0


def marker(seed: int, store: str, stage: str) -> str:
    digest = hashlib.sha256(f"{seed}:{store}:{stage}".encode()).hexdigest()
    return f"{store}-{stage}-{digest[:12]}"


def checksum_placeholder(seed: int, store: str, marker_value: str) -> dict:
    digest = hashlib.sha256(f"{seed}:{store}:{marker_value}:placeholder".encode()).hexdigest()
    return {
        "store": store,
        "sha256": digest,
        "placeholder": True,
        "verifiedAgainstLiveArtifact": False,
    }


def synthetic_timings(seed: int, profile: dict) -> dict:
    targets = profile["serviceTargets"]
    rpo = round(4.0 + stable_unit(seed, "rpo") * 7.0, 3)
    query_ready = round(28.0 + stable_unit(seed, "query-ready") * 24.0, 3)
    full_vector = round(query_ready + 92.0 + stable_unit(seed, "full-vector") * 72.0, 3)
    return {
        "rpoMinutesSynthetic": rpo,
        "queryReadyRtoMinutesSynthetic": query_ready,
        "fullVectorRtoMinutesSynthetic": full_vector,
        "targets": {
            "rpoMinutes": targets["rpoMinutes"],
            "queryReadyRtoMinutes": targets["queryReadyRtoMinutes"],
            "fullVectorRtoMinutes": targets["fullVectorRtoMinutes"],
        },
        "syntheticWithinTargets": {
            "rpo": rpo <= float(targets["rpoMinutes"]),
            "queryReadyRto": query_ready <= float(targets["queryReadyRtoMinutes"]),
            "fullVectorRto": full_vector <= float(targets["fullVectorRtoMinutes"]),
        },
    }


def recovery_order(seed: int) -> list[dict]:
    stages = [
        ("fence-writes", "Freeze API mutations and workers before recovery-point selection."),
        ("restore-postgres", "Restore PostgreSQL first; it is authority for visibility and auth."),
        ("restore-minio", "Restore MinIO originals and derived artifacts to the PG recovery point."),
        ("restore-or-rebuild-qdrant", "Restore matching Qdrant snapshot or rebuild from PG chunks."),
        ("reconcile", "Reconcile missing/orphan/stale objects and vectors before readiness."),
        ("open-query-ready", "Open authorized read/search path without claiming full vector rebuild."),
        ("complete-full-vector", "Finish active generation restore/rebuild and verification."),
    ]
    rows = []
    for index, (stage, note) in enumerate(stages, start=1):
        rows.append(
            {
                "order": index,
                "stage": stage,
                "marker": marker(seed, "restore", stage),
                "note": note,
            }
        )
    return rows


def build_payload(seed: int) -> dict:
    profile = load_json(WORKLOAD_PROFILE)
    spike = load_json(SPIKE_ENVIRONMENT)
    status = git_status()
    docker_ok = docker_available()
    spike_lifecycle = spike.get("lifecycle", {})
    stores = spike_lifecycle.get("stores") if isinstance(spike_lifecycle, dict) else None
    if not isinstance(stores, list) or not stores:
        stores = ["postgres", "qdrant", "minio"]

    store_markers = {}
    checksums = []
    for store in sorted(str(item) for item in stores):
        backup_marker = marker(seed, store, "backup")
        restore_marker = marker(seed, store, "restore")
        store_markers[store] = {
            "backupMarker": backup_marker,
            "restoreMarker": restore_marker,
            "source": "recorded-spike-lifecycle",
        }
        checksums.append(checksum_placeholder(seed, store, restore_marker))

    timings = synthetic_timings(seed, profile)
    target_match = False
    profile_b_gate_passed = False
    closure = {
        "recordedSpikeEvidenceLoaded": SPIKE_ENVIRONMENT.is_file(),
        "recoveryOrderMarkersEmitted": True,
        "checksumPlaceholdersEmitted": bool(checksums),
        "timingsEmitted": True,
        "honestFlagsSet": target_match is False and profile_b_gate_passed is False,
    }

    return {
        "version": 1,
        "reportId": "p0-10-restore-drill",
        "gateId": "P0-10-RESTORE-SMOKE",
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "command": "python3 bench/markhand_web/scripts/run_restore_drill.py",
        "mode": "offline-synthetic-recorded-spike",
        "seed": seed,
        "git": status,
        "environment": {
            "environmentId": "current-runner-offline-synthetic",
            "targetEnvironmentId": "on-prem-reference",
            "targetMatch": target_match,
            "dockerAvailable": docker_ok,
            "dockerUsed": False,
            "evidenceSource": "bench/markhand_web/reports/spike-environment.json",
            "spikeTargetMatch": bool(spike.get("targetMatch")),
        },
        "implementationSha256": implementation_sha256(),
        "workloadProfile": {
            "profileId": profile["profileId"],
            "recovery": profile["loads"]["recovery"],
            "serviceTargets": {
                "rpoMinutes": profile["serviceTargets"]["rpoMinutes"],
                "queryReadyRtoMinutes": profile["serviceTargets"]["queryReadyRtoMinutes"],
                "fullVectorRtoMinutes": profile["serviceTargets"]["fullVectorRtoMinutes"],
            },
        },
        "recordedSpike": {
            "reportId": spike.get("reportId"),
            "sha256": file_sha256(SPIKE_ENVIRONMENT),
            "targetMatch": bool(spike.get("targetMatch")),
            "lifecycle": spike_lifecycle,
        },
        "authorityOrder": {
            "postgres": "authority for visibility, auth, chunks, jobs and index generation pointers",
            "minio": "durable originals and artifacts; not reconstructable from vectors",
            "qdrant": "rebuildable from PostgreSQL chunks and active index signature",
        },
        "recoveryOrder": recovery_order(seed),
        "storeMarkers": store_markers,
        "checksumPlaceholders": checksums,
        "timings": timings,
        "targetMatch": target_match,
        "profileBDrGatePassed": profile_b_gate_passed,
        "profileBDrBlocked": True,
        "targetResultsValidForGate": False,
        "closure": closure,
        "p0_10_restore_smoke_closed": all(closure.values()),
        "doesNotClaim": [
            "G0-DR-RPO pass",
            "G0-DR-QUERY-READY-RTO pass",
            "G0-DR-FULL-VECTOR-RTO pass",
            "Profile B restore evidence",
        ],
        "notes": [
            "Offline synthetic restore smoke only; no live PostgreSQL, MinIO or Qdrant restore was executed.",
            "Checksum values are deterministic placeholders for runbook shape, not live artifact verification.",
            DOES_NOT_CLAIM,
        ],
    }


def render_report(payload: dict) -> str:
    timings = payload["timings"]
    lines = [
        "# P0-10 restore drill smoke",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Mode: `{payload['mode']}`",
        f"- Seed: `{payload['seed']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty at harness start: `{str(payload['git']['dirty']).lower()}`",
        f"- `targetMatch`: `{str(payload['targetMatch']).lower()}`",
        f"- `profileBDrGatePassed`: `{str(payload['profileBDrGatePassed']).lower()}`",
        f"- `profileBDrBlocked`: `{str(payload['profileBDrBlocked']).lower()}`",
        f"- `p0_10_restore_smoke_closed`: `{str(payload['p0_10_restore_smoke_closed']).lower()}`",
        "",
        "## Scope",
        "",
        "This is an offline synthetic restore drill using recorded spike lifecycle",
        "evidence. It emits recovery-order markers and placeholder checksums only.",
        "",
        f"Explicit note: {DOES_NOT_CLAIM}.",
        "",
        "## Authority order",
        "",
    ]
    for key, value in payload["authorityOrder"].items():
        lines.append(f"- `{key}`: {value}.")
    lines.extend(["", "## Recovery order", "", "| order | stage | marker | note |", "|---:|---|---|---|"])
    for row in payload["recoveryOrder"]:
        lines.append(
            f"| {row['order']} | `{row['stage']}` | `{row['marker']}` | {row['note']} |"
        )
    lines.extend(
        [
            "",
            "## Store markers and checksum placeholders",
            "",
            "| store | backup marker | restore marker | checksum placeholder |",
            "|---|---|---|---|",
        ]
    )
    placeholders = {item["store"]: item["sha256"] for item in payload["checksumPlaceholders"]}
    for store, marker_row in payload["storeMarkers"].items():
        lines.append(
            f"| `{store}` | `{marker_row['backupMarker']}` | "
            f"`{marker_row['restoreMarker']}` | `{placeholders[store]}` |"
        )
    lines.extend(
        [
            "",
            "## Synthetic timings",
            "",
            "| metric | synthetic value min | target min | synthetic within target | gate-valid pass |",
            "|---|---:|---:|---|---|",
            (
                f"| RPO | {timings['rpoMinutesSynthetic']} | "
                f"{timings['targets']['rpoMinutes']} | "
                f"{str(timings['syntheticWithinTargets']['rpo']).lower()} | false |"
            ),
            (
                f"| Query-ready RTO | {timings['queryReadyRtoMinutesSynthetic']} | "
                f"{timings['targets']['queryReadyRtoMinutes']} | "
                f"{str(timings['syntheticWithinTargets']['queryReadyRto']).lower()} | false |"
            ),
            (
                f"| Full-vector RTO | {timings['fullVectorRtoMinutesSynthetic']} | "
                f"{timings['targets']['fullVectorRtoMinutes']} | "
                f"{str(timings['syntheticWithinTargets']['fullVectorRto']).lower()} | false |"
            ),
            "",
            "Synthetic values are not gate-valid because `targetMatch=false`.",
            "",
            "## Closure",
            "",
            "| field | value |",
            "|---|---|",
        ]
    )
    for key, value in payload["closure"].items():
        lines.append(f"| `{key}` | `{str(value).lower()}` |")
    if payload["git"]["dirtyPaths"]:
        lines.extend(["", "Dirty paths at harness start:"])
        for path in payload["git"]["dirtyPaths"][:20]:
            lines.append(f"- `{path}`")
    lines.append("")
    return "\n".join(lines)


def write_outputs(payload: dict, summary: Path, report: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload), encoding="utf-8")


def self_test(seed: int) -> None:
    profile = load_json(WORKLOAD_PROFILE)
    first = synthetic_timings(seed, profile)
    second = synthetic_timings(seed, profile)
    assert first == second
    order = recovery_order(seed)
    assert [row["stage"] for row in order][:3] == [
        "fence-writes",
        "restore-postgres",
        "restore-minio",
    ]
    payload = build_payload(seed)
    assert payload["targetMatch"] is False
    assert payload["profileBDrGatePassed"] is False
    assert payload["p0_10_restore_smoke_closed"] is True
    assert DOES_NOT_CLAIM in payload["notes"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=SEED)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test(args.seed)
        print("self-test ok")
        return 0

    try:
        payload = build_payload(args.seed)
        write_outputs(payload, args.summary.resolve(), args.report.resolve())
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    print(f"wrote {args.summary.resolve().relative_to(ROOT)}")
    print(f"wrote {args.report.resolve().relative_to(ROOT)}")
    print(f"p0_10_restore_smoke_closed={str(payload['p0_10_restore_smoke_closed']).lower()}")
    print(f"profileBDrGatePassed={str(payload['profileBDrGatePassed']).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
