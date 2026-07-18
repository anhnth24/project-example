#!/usr/bin/env python3
"""P0-10 query-load smoke stub.

This emits deterministic local smoke metrics for the query-load harness shape.
It documents the Profile B requirement and does not claim G0-SLO pass evidence.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
WORKLOAD_PROFILE = CORPUS / "workload-profile.yaml"
SUMMARY_PATH = CORPUS / "query_load/summary.json"
REPORT_PATH = CORPUS / "reports/query-load-smoke.md"
SEED = 20260718
DOES_NOT_CLAIM = "does NOT claim G0-SLO-QUERY-P95 or G0-SLO-QUERY-P99 pass evidence"
IMPLEMENTATION_FILES = (
    "bench/markhand_web/scripts/run_query_load.py",
    "docs/markhand-web-sla-targets.md",
)


class HarnessError(RuntimeError):
    """Actionable query-load smoke error."""


def load_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{path}: cannot load JSON: {error}") from error
    if not isinstance(payload, dict):
        raise HarnessError(f"{path}: expected JSON object")
    return payload


def git_status() -> dict:
    try:
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        branch = subprocess.check_output(["git", "branch", "--show-current"], cwd=ROOT, text=True).strip()
        raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    except (OSError, subprocess.CalledProcessError):
        return {"commit": "unknown", "branch": "unknown", "dirty": True, "dirtyPaths": ["git-status-unavailable"]}
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
    return {"commit": commit, "branch": branch, "dirty": bool(dirty_paths), "dirtyPaths": dirty_paths}


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


def synthetic_latency(seed: int, concurrent_queries: int, statistic: str) -> float:
    base = 90.0 + concurrent_queries * 3.2
    if statistic == "p95":
        multiplier = 1.7
    elif statistic == "p99":
        multiplier = 2.25
    else:
        multiplier = 1.0
    jitter = 1.0 + stable_unit(seed, statistic, concurrent_queries) * 0.16
    return round(base * multiplier * jitter, 3)


def build_payload(seed: int) -> dict:
    profile = load_json(WORKLOAD_PROFILE)
    status = git_status()
    normal = profile["loads"]["normal"]
    peak = profile["loads"]["peak"]
    targets = profile["serviceTargets"]
    p95 = synthetic_latency(seed, int(normal["concurrentQueries"]), "p95")
    p99 = synthetic_latency(seed, int(peak["concurrentQueries"]), "p99")
    target_match = False
    gate_passed = False
    closure = {
        "stubExecuted": True,
        "smokeMetricsEmitted": True,
        "honestFlagsSet": target_match is False and gate_passed is False,
        "profileBRequirementDocumented": True,
    }
    return {
        "version": 1,
        "reportId": "p0-10-query-load-smoke",
        "gateIds": ["G0-SLO-QUERY-P95", "G0-SLO-QUERY-P99"],
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "command": "python3 bench/markhand_web/scripts/run_query_load.py",
        "mode": "offline-synthetic-query-load-smoke",
        "seed": seed,
        "git": status,
        "environment": {
            "environmentId": "current-runner-offline-synthetic",
            "targetEnvironmentId": "on-prem-reference",
            "targetMatch": target_match,
        },
        "implementationSha256": implementation_sha256(),
        "workloadProfile": {
            "profileId": profile["profileId"],
            "normal": normal,
            "peak": peak,
            "targetFilteredQueryP99Ms": targets["filteredQueryP99Ms"],
        },
        "metrics": {
            "queryLatencyP95MsSynthetic": p95,
            "filteredQueryLatencyP99MsSynthetic": p99,
            "targetQueryP95Ms": 500,
            "targetFilteredQueryP99Ms": targets["filteredQueryP99Ms"],
            "syntheticWithinTargets": {
                "queryP95": p95 <= 500,
                "filteredQueryP99": p99 <= float(targets["filteredQueryP99Ms"]),
            },
        },
        "targetMatch": target_match,
        "profileBSloGatePassed": gate_passed,
        "targetResultsValidForGate": False,
        "productionSloBlocked": True,
        "closure": closure,
        "p0_10_query_load_smoke_closed": all(closure.values()),
        "doesNotClaim": [
            "G0-SLO-QUERY-P95 pass",
            "G0-SLO-QUERY-P99 pass",
            "20M live mixed-load evidence",
            "Profile B latency evidence",
        ],
        "notes": [
            "This stub exists so gate commands are runnable before Profile B is available.",
            "Gate-valid query SLO evidence requires live Postgres/Qdrant with targetMatch=true.",
            DOES_NOT_CLAIM,
        ],
    }


def render_report(payload: dict) -> str:
    metrics = payload["metrics"]
    lines = [
        "# P0-10 query-load smoke",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Mode: `{payload['mode']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty at harness start: `{str(payload['git']['dirty']).lower()}`",
        f"- `targetMatch`: `{str(payload['targetMatch']).lower()}`",
        f"- `profileBSloGatePassed`: `{str(payload['profileBSloGatePassed']).lower()}`",
        f"- `productionSloBlocked`: `{str(payload['productionSloBlocked']).lower()}`",
        f"- `p0_10_query_load_smoke_closed`: `{str(payload['p0_10_query_load_smoke_closed']).lower()}`",
        "",
        "## Scope",
        "",
        "This is a deterministic smoke stub for the query-load command shape. It",
        "does not run live PostgreSQL/Qdrant or 20M aggregate vector load.",
        "",
        f"Explicit note: {DOES_NOT_CLAIM}.",
        "",
        "## Synthetic metrics",
        "",
        "| metric | synthetic value ms | target ms | synthetic within target | gate-valid pass |",
        "|---|---:|---:|---|---|",
        (
            f"| Query P95 | {metrics['queryLatencyP95MsSynthetic']} | "
            f"{metrics['targetQueryP95Ms']} | "
            f"{str(metrics['syntheticWithinTargets']['queryP95']).lower()} | false |"
        ),
        (
            f"| Filtered query P99 | {metrics['filteredQueryLatencyP99MsSynthetic']} | "
            f"{metrics['targetFilteredQueryP99Ms']} | "
            f"{str(metrics['syntheticWithinTargets']['filteredQueryP99']).lower()} | false |"
        ),
        "",
        "Profile B requirement: run against `on-prem-reference` with `targetMatch=true`,",
        "approved workload scale, live services and mixed query/ingest/delete pressure.",
        "",
        "## Closure",
        "",
        "| field | value |",
        "|---|---|",
    ]
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
    p95 = synthetic_latency(seed, 20, "p95")
    assert p95 == synthetic_latency(seed, 20, "p95")
    assert p95 > 0
    payload = build_payload(seed)
    assert payload["targetMatch"] is False
    assert payload["profileBSloGatePassed"] is False
    assert payload["p0_10_query_load_smoke_closed"] is True
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
    print(f"p0_10_query_load_smoke_closed={str(payload['p0_10_query_load_smoke_closed']).lower()}")
    print(f"profileBSloGatePassed={str(payload['profileBSloGatePassed']).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
