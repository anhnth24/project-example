#!/usr/bin/env python3
"""P0-10 Phase 0 close harness.

Aggregates the accepted decision gate, registry/license checkers and existing
smoke summaries. `p0_10_closed` is true only for local Phase 0 close semantics;
Profile B production exit may still be blocked.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
SUMMARY_PATH = CORPUS / "phase0/summary.json"
REPORT_PATH = CORPUS / "reports/phase0-gate.md"
RESTORE_SUMMARY = CORPUS / "restore/summary.json"
QUERY_LOAD_SUMMARY = CORPUS / "query_load/summary.json"
SECURITY_SUMMARY = CORPUS / "security/summary.json"

COMMANDS = {
    "phase0Decisions": [sys.executable, "scripts/check-phase0-decisions.py"],
    "markhandGates": [sys.executable, "scripts/check-markhand-gates.py"],
    "runtimeLicenseInventory": [sys.executable, "scripts/check-runtime-license-inventory.py"],
}

PROFILE_B_BLOCKERS = [
    {
        "id": "G0-SLO-QUERY-P95",
        "owner": "operations-owner",
        "reason": "Query P95 requires live mixed-load measurement on on-prem-reference.",
    },
    {
        "id": "G0-SLO-QUERY-P99",
        "owner": "operations-owner",
        "reason": "Filtered query P99 requires live 20M aggregate vector and tenant-filter measurement.",
    },
    {
        "id": "G0-CAP-INGEST-THROUGHPUT",
        "owner": "worker-owner",
        "reason": "Ingest throughput/headroom remains local-cpu smoke until Profile B run.",
    },
    {
        "id": "G0-DR-RPO",
        "owner": "operations-owner",
        "reason": "RPO requires real component-loss backup/restore evidence.",
    },
    {
        "id": "G0-DR-QUERY-READY-RTO",
        "owner": "operations-owner",
        "reason": "Query-ready RTO requires live PG/MinIO/Qdrant restore drill.",
    },
    {
        "id": "G0-DR-FULL-VECTOR-RTO",
        "owner": "operations-owner",
        "reason": "Full-vector RTO requires live snapshot/rebuild timing on target hardware.",
    },
    {
        "id": "G0-RET-VLLM-CUTOVER",
        "owner": "retrieval-owner",
        "reason": "Production embedding cutover requires on-prem vLLM evidence.",
    },
]


class HarnessError(RuntimeError):
    """Actionable Phase 0 gate harness error."""


def git_status() -> dict:
    try:
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        branch = subprocess.check_output(["git", "branch", "--show-current"], cwd=ROOT, text=True).strip()
        raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    except (OSError, subprocess.CalledProcessError):
        return {"commit": "unknown", "branch": "unknown", "clean": False, "dirtyPaths": ["git-status-unavailable"]}
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
    return {"commit": commit, "branch": branch, "clean": not dirty_paths, "dirtyPaths": dirty_paths}


def load_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{path}: cannot load JSON: {error}") from error
    if not isinstance(payload, dict):
        raise HarnessError(f"{path}: expected JSON object")
    return payload


def run_command(name: str, command: list[str]) -> dict:
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            text=True,
            capture_output=True,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return {
            "name": name,
            "command": " ".join(command),
            "pass": False,
            "returnCode": None,
            "stdout": "",
            "stderr": str(error),
        }
    return {
        "name": name,
        "command": " ".join(command),
        "pass": completed.returncode == 0,
        "returnCode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }


def security_smoke_status() -> dict:
    try:
        payload = load_json(SECURITY_SUMMARY)
    except HarnessError as error:
        return {"pass": False, "path": str(SECURITY_SUMMARY.relative_to(ROOT)), "error": str(error)}
    return {
        "pass": bool(payload.get("p0_09_closed")),
        "path": str(SECURITY_SUMMARY.relative_to(ROOT)),
        "p0_09_closed": bool(payload.get("p0_09_closed")),
        "measurementScope": payload.get("measurementScope"),
    }


def restore_smoke_status() -> dict:
    try:
        payload = load_json(RESTORE_SUMMARY)
    except HarnessError as error:
        return {"pass": False, "path": str(RESTORE_SUMMARY.relative_to(ROOT)), "error": str(error)}
    pass_value = (
        bool(payload.get("p0_10_restore_smoke_closed"))
        and payload.get("targetMatch") is False
        and payload.get("profileBDrGatePassed") is False
    )
    return {
        "pass": pass_value,
        "path": str(RESTORE_SUMMARY.relative_to(ROOT)),
        "p0_10_restore_smoke_closed": bool(payload.get("p0_10_restore_smoke_closed")),
        "targetMatch": payload.get("targetMatch"),
        "profileBDrGatePassed": payload.get("profileBDrGatePassed"),
        "profileBDrBlocked": payload.get("profileBDrBlocked"),
    }


def query_load_smoke_status() -> dict:
    if not QUERY_LOAD_SUMMARY.is_file():
        return {"pass": False, "path": str(QUERY_LOAD_SUMMARY.relative_to(ROOT)), "error": "query load smoke missing"}
    try:
        payload = load_json(QUERY_LOAD_SUMMARY)
    except HarnessError as error:
        return {"pass": False, "path": str(QUERY_LOAD_SUMMARY.relative_to(ROOT)), "error": str(error)}
    pass_value = (
        bool(payload.get("p0_10_query_load_smoke_closed"))
        and payload.get("targetMatch") is False
        and payload.get("profileBSloGatePassed") is False
    )
    return {
        "pass": pass_value,
        "path": str(QUERY_LOAD_SUMMARY.relative_to(ROOT)),
        "p0_10_query_load_smoke_closed": bool(payload.get("p0_10_query_load_smoke_closed")),
        "targetMatch": payload.get("targetMatch"),
        "profileBSloGatePassed": payload.get("profileBSloGatePassed"),
        "productionSloBlocked": payload.get("productionSloBlocked"),
    }


def close_flags(
    command_results: dict[str, dict],
    security_smoke: dict,
    restore_smoke: dict,
    git: dict,
) -> dict:
    decisions = bool(command_results["phase0Decisions"]["pass"])
    gates = bool(command_results["markhandGates"]["pass"])
    license_ok = bool(command_results["runtimeLicenseInventory"]["pass"])
    security_ok = bool(security_smoke["pass"])
    restore_ok = bool(restore_smoke["pass"])
    git_clean = bool(git["clean"])
    return {
        "decisionsAccepted": decisions,
        "gateRegistryValid": gates,
        "runtimeLicenseInventoryPassed": license_ok,
        "securitySmokeClosed": security_ok,
        "restoreSmokeClosed": restore_ok,
        "gitClean": git_clean,
        "p0_10_closed": all([decisions, gates, license_ok, security_ok, restore_ok, git_clean]),
        "productionPhase0ExitBlocked": bool(PROFILE_B_BLOCKERS),
    }


def build_payload() -> dict:
    git = git_status()
    command_results = {
        name: run_command(name, command)
        for name, command in COMMANDS.items()
    }
    security = security_smoke_status()
    restore = restore_smoke_status()
    query_load = query_load_smoke_status()
    flags = close_flags(command_results, security, restore, git)
    return {
        "version": 1,
        "reportId": "p0-10-phase0-gate",
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "command": "python3 bench/markhand_web/scripts/run_phase0_gate.py",
        "git": git,
        "commands": command_results,
        "smoke": {
            "security": security,
            "restore": restore,
            "queryLoad": query_load,
        },
        "closure": flags,
        "p0_10_closed": flags["p0_10_closed"],
        "productionPhase0ExitBlocked": flags["productionPhase0ExitBlocked"],
        "remainingProfileBBlockers": PROFILE_B_BLOCKERS,
        "notes": [
            "P0-10 close means accepted decisions, license/security checks and restore smoke are closed on a clean tree.",
            "Production Phase 0 exit remains blocked while Profile B SLO, capacity and DR gates lack targetMatch=true evidence.",
            "G0-DR evidence remains null in gates.yaml; restore summary is smoke evidence only.",
        ],
    }


def render_report(payload: dict) -> str:
    lines = [
        "# P0-10 Phase 0 gate",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Git clean at harness start: `{str(payload['git']['clean']).lower()}`",
        f"- `p0_10_closed`: `{str(payload['p0_10_closed']).lower()}`",
        f"- `productionPhase0ExitBlocked`: `{str(payload['productionPhase0ExitBlocked']).lower()}`",
        "",
        "## Checker results",
        "",
        "| check | pass | stdout | stderr |",
        "|---|---|---|---|",
    ]
    for result in payload["commands"].values():
        stdout = result["stdout"].replace("|", "\\|")
        stderr = result["stderr"].replace("|", "\\|")
        lines.append(
            f"| `{result['name']}` | `{str(result['pass']).lower()}` | "
            f"`{stdout}` | `{stderr}` |"
        )
    lines.extend(["", "## Smoke results", "", "| smoke | pass | path | notes |", "|---|---|---|---|"])
    for name, smoke in payload["smoke"].items():
        notes = []
        for key in (
            "p0_09_closed",
            "p0_10_restore_smoke_closed",
            "p0_10_query_load_smoke_closed",
            "targetMatch",
            "profileBDrGatePassed",
            "profileBSloGatePassed",
        ):
            if key in smoke:
                notes.append(f"{key}={str(smoke[key]).lower()}")
        if "error" in smoke:
            notes.append(f"error={smoke['error']}")
        lines.append(
            f"| `{name}` | `{str(smoke.get('pass')).lower()}` | "
            f"`{smoke.get('path')}` | {'; '.join(notes)} |"
        )
    lines.extend(["", "## Closure flags", "", "| flag | value |", "|---|---|"])
    for key, value in payload["closure"].items():
        lines.append(f"| `{key}` | `{str(value).lower()}` |")
    lines.extend(["", "## Remaining Profile B blockers", "", "| gate/item | owner | reason |", "|---|---|---|"])
    for blocker in payload["remainingProfileBBlockers"]:
        lines.append(f"| `{blocker['id']}` | `{blocker['owner']}` | {blocker['reason']} |")
    if payload["git"]["dirtyPaths"]:
        lines.extend(["", "Dirty paths at harness start:"])
        for path in payload["git"]["dirtyPaths"][:30]:
            lines.append(f"- `{path}`")
    lines.append("")
    return "\n".join(lines)


def write_outputs(payload: dict, summary: Path, report: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload), encoding="utf-8")


def self_test() -> None:
    ok_commands = {
        name: {"pass": True}
        for name in ("phase0Decisions", "markhandGates", "runtimeLicenseInventory")
    }
    flags = close_flags(ok_commands, {"pass": True}, {"pass": True}, {"clean": True})
    assert flags["p0_10_closed"] is True
    assert flags["productionPhase0ExitBlocked"] is True
    dirty_flags = close_flags(ok_commands, {"pass": True}, {"pass": True}, {"clean": False})
    assert dirty_flags["p0_10_closed"] is False
    failed_license = dict(ok_commands)
    failed_license["runtimeLicenseInventory"] = {"pass": False}
    assert close_flags(failed_license, {"pass": True}, {"pass": True}, {"clean": True})["p0_10_closed"] is False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        print("self-test ok")
        return 0

    try:
        payload = build_payload()
        write_outputs(payload, args.summary.resolve(), args.report.resolve())
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    print(f"wrote {args.summary.resolve().relative_to(ROOT)}")
    print(f"wrote {args.report.resolve().relative_to(ROOT)}")
    print(f"p0_10_closed={str(payload['p0_10_closed']).lower()}")
    print(f"productionPhase0ExitBlocked={str(payload['productionPhase0ExitBlocked']).lower()}")
    return 0 if payload["p0_10_closed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
