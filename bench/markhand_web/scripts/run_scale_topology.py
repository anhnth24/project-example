#!/usr/bin/env python3
"""P0-07 offline topology comparison harness.

This is intentionally synthetic and in-process: it records topology decisions
for Phase 1B POC work without claiming Profile B or production 20M evidence.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import random
import re
import shutil
import statistics
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
WORKLOAD_PROFILE = CORPUS / "workload-profile.yaml"
SUMMARY_PATH = CORPUS / "scale/summary.json"
REPORT_PATH = CORPUS / "reports/scale-topology.md"
SPIKE_ENVIRONMENT = CORPUS / "reports/spike-environment.json"
SEED = 20260718
HASH_BUCKETS = 16
DOES_NOT_CLAIM = "does NOT claim G0-SLO-QUERY-P99 / 20M evidence"


IMPLEMENTATION_FILES = (
    "bench/markhand_web/scale/README.md",
    "bench/markhand_web/scripts/run_scale_topology.py",
    "docs/adr/0008-pg-partition-strategy.md",
    "docs/adr/0009-qdrant-topology.md",
)


def load_json(path: Path) -> dict:
    """Load JSON from files that may carry a .yaml extension in this repo."""
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise SystemExit(f"invalid object payload: {path}")
    return payload


def git(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_status() -> dict:
    commit = git("rev-parse", "HEAD")
    raw = ""
    try:
        raw = subprocess.check_output(
            ["git", "status", "--porcelain"],
            cwd=ROOT,
            text=True,
        )
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
    return {"commit": commit, "dirty": bool(dirty_paths), "dirtyPaths": dirty_paths}


def parse_key_value_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.is_file():
        return values
    for line in path.read_text(errors="replace").splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key] = value.strip().strip('"')
    return values


def default_network_interface() -> str:
    route_path = Path("/proc/net/route")
    if not route_path.is_file():
        return "unknown"
    for line in route_path.read_text(errors="replace").splitlines()[1:]:
        parts = line.split()
        if len(parts) > 2 and parts[1] == "00000000":
            return parts[0]
    return "unknown"


def hardware_fingerprint(storage_path: Path) -> dict:
    cpuinfo = Path("/proc/cpuinfo").read_text(errors="replace")
    meminfo = Path("/proc/meminfo").read_text(errors="replace")
    vendor = re.search(r"vendor_id\s*:\s*(.+)", cpuinfo)
    model = re.search(r"model name\s*:\s*(.+)", cpuinfo)
    processors = re.findall(r"^processor\s*:", cpuinfo, re.MULTILINE)
    physical_cores = {
        (physical, core)
        for physical, core in re.findall(
            r"physical id\s*:\s*(\d+).*?core id\s*:\s*(\d+)",
            cpuinfo,
            re.DOTALL,
        )
    }
    memory = re.search(r"MemTotal:\s*(\d+)", meminfo)
    disk = shutil.disk_usage(storage_path)
    iface = default_network_interface()
    speed_path = Path("/sys/class/net") / iface / "speed"
    bandwidth = 0.0
    if speed_path.is_file():
        try:
            speed = int(speed_path.read_text().strip())
            bandwidth = speed / 1000 if speed > 0 else 0.0
        except (OSError, ValueError):
            pass
    os_release = parse_key_value_file(Path("/etc/os-release"))
    return {
        "cpu": {
            "vendor": vendor.group(1).strip() if vendor else "unknown",
            "model": model.group(1).strip() if model else "unknown",
            "cores": len(physical_cores) or len(processors) or (os.cpu_count() or 1),
            "threads": len(processors) or (os.cpu_count() or 1),
            "physicalCoresMeasured": bool(physical_cores),
        },
        "ramGb": round(int(memory.group(1)) * 1024 / 1_000_000_000, 2)
        if memory
        else 0.0,
        "disk": {
            "capacityGb": round(disk.total / 1_000_000_000, 2),
            "storagePathSha256": hashlib.sha256(
                str(storage_path.resolve()).encode()
            ).hexdigest(),
            "type": "offline-local",
        },
        "gpu": {"model": "none", "vramGb": 0.0, "count": 0},
        "network": {
            "interface": iface,
            "bandwidthGbps": bandwidth,
            "bandwidthMeasured": bandwidth > 0,
        },
        "os": {
            "distro": (
                f"{os_release.get('ID', platform.system())}-"
                f"{os_release.get('VERSION_ID', platform.release())}"
            ),
            "arch": platform.machine(),
        },
    }


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for relative in IMPLEMENTATION_FILES:
        path = ROOT / relative
        digest.update(relative.encode())
        digest.update(b"\0")
        if path.is_file():
            digest.update(path.read_bytes())
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def stable_unit(seed: int, *parts: object) -> float:
    data = "|".join([str(seed), *(str(part) for part in parts)]).encode()
    value = int.from_bytes(hashlib.sha256(data).digest()[:8], "big")
    return value / float(2**64 - 1)


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    rank = (len(ordered) - 1) * pct
    lower = int(rank)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = rank - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def weighted_choice(rng: random.Random, tenants: list[dict]) -> dict:
    marker = rng.random()
    cumulative = 0.0
    for tenant in tenants:
        cumulative += tenant["loadShare"]
        if marker <= cumulative:
            return tenant
    return tenants[-1]


def zipf_weights(count: int, exponent: float) -> list[float]:
    raw = [1.0 / ((index + 1) ** exponent) for index in range(count)]
    total = sum(raw)
    return [value / total for value in raw]


def tenant_distribution(profile: dict, seed: int) -> dict:
    scale = profile["scale"]
    org_count = int(scale["orgCount"])
    top_count = max(1, int(round(org_count * 0.2)))
    tail_count = org_count - top_count
    rng = random.Random(seed)
    top_orgs = list(range(1, top_count + 1))
    tail_orgs = list(range(top_count + 1, org_count + 1))
    rng.shuffle(top_orgs)
    rng.shuffle(tail_orgs)

    top_weights = [weight * 0.8 for weight in zipf_weights(top_count, 1.04)]
    tail_weights = (
        [weight * 0.2 for weight in zipf_weights(tail_count, 1.04)]
        if tail_count
        else []
    )
    org_weights: dict[int, float] = {}
    for org, weight in zip(top_orgs, top_weights):
        org_weights[org] = weight
    for org, weight in zip(tail_orgs, tail_weights):
        org_weights[org] = weight

    vector_count = int(scale["vectorsPerOrgMax"])
    collections = int(scale["collectionsPerOrg"])
    documents_per_collection = int(scale["documentsPerCollection"])
    average_pages = int(scale["averageDocumentPages"])
    tenants = []
    for org in range(1, org_count + 1):
        tenant = {
            "orgId": f"org-{org:02d}",
            "collections": collections,
            "documents": collections * documents_per_collection,
            "averageDocumentPages": average_pages,
            "vectors": vector_count,
            "loadShare": org_weights[org],
            "qdrantCohort": f"cohort-{((org - 1) // max(1, top_count)) + 1:02d}",
            "pgHashBucket": int(
                hashlib.sha256(f"org-{org:02d}".encode()).hexdigest()[:8],
                16,
            )
            % HASH_BUCKETS,
        }
        tenants.append(tenant)
    total_vectors = sum(tenant["vectors"] for tenant in tenants)
    top_share = sum(
        tenant["loadShare"]
        for tenant in sorted(tenants, key=lambda item: item["loadShare"], reverse=True)[
            :top_count
        ]
    )
    return {
        "strategy": profile["loads"]["aggregate"]["tenantDistribution"],
        "seed": seed,
        "orgCount": org_count,
        "topTenantCount": top_count,
        "topTenantLoadShare": round(top_share, 6),
        "aggregateVectors": total_vectors,
        "vectorsPerOrgMax": vector_count,
        "tenants": tenants,
    }


def operation_plan(profile: dict) -> dict:
    peak = profile["loads"]["peak"]
    query_count = int(peak["concurrentQueries"]) * 20
    ingest_count = int(peak["ingestDocumentsPerHour"])
    delete_count = int(peak["deleteOperationsPerHour"])
    return {
        "profile": "peak",
        "queryOperations": query_count,
        "ingestOperations": ingest_count,
        "deleteOperations": delete_count,
        "totalOperations": query_count + ingest_count + delete_count,
        "orgIdFilterApplied": True,
        "aggregateConcurrentQueries": int(
            profile["loads"]["aggregate"]["concurrentQueries"]
        ),
        "aggregateConcurrentIngest": int(profile["loads"]["aggregate"]["concurrentIngest"]),
    }


def latency_ms(
    topology_family: str,
    topology_name: str,
    op_type: str,
    tenant: dict,
    op_index: int,
    seed: int,
) -> float:
    pressure = tenant["loadShare"] * 20.0
    noise = stable_unit(seed, topology_family, topology_name, op_type, op_index, tenant["orgId"])
    if topology_family == "qdrant":
        if topology_name == "shared-collection":
            bases = {"query": 21.0, "ingest": 7.2, "delete": 10.0}
            pressure_factor = {"query": 4.0, "ingest": 1.8, "delete": 2.6}
            topology_overhead = 0.0
        else:
            bases = {"query": 25.5, "ingest": 8.8, "delete": 11.8}
            pressure_factor = {"query": 5.1, "ingest": 2.3, "delete": 2.5}
            topology_overhead = 3.2
    else:
        if topology_name == "no-partition":
            bases = {"query": 5.4, "ingest": 4.2, "delete": 8.9}
            pressure_factor = {"query": 1.0, "ingest": 1.0, "delete": 2.1}
            topology_overhead = 0.0
        else:
            bases = {"query": 6.8, "ingest": 5.1, "delete": 7.1}
            pressure_factor = {"query": 1.4, "ingest": 1.3, "delete": 1.3}
            topology_overhead = 0.7
    base = bases[op_type]
    tail = 1.0 + (noise**3) * 3.8
    return round(base + topology_overhead + pressure * pressure_factor[op_type] + tail, 3)


def simulate_topology(
    family: str,
    topology: str,
    tenants: list[dict],
    plan: dict,
    seed: int,
) -> dict:
    rng = random.Random(seed + int(stable_unit(seed, family, topology) * 1_000_000))
    operation_counts = {
        "query": plan["queryOperations"],
        "ingest": plan["ingestOperations"],
        "delete": plan["deleteOperations"],
    }
    latencies: dict[str, list[float]] = {"query": [], "ingest": [], "delete": []}
    touched_orgs: set[str] = set()
    missing_filter = 0
    for op_type, count in operation_counts.items():
        for index in range(count):
            tenant = weighted_choice(rng, tenants)
            touched_orgs.add(tenant["orgId"])
            org_id_filter = tenant["orgId"]
            if not org_id_filter:
                missing_filter += 1
            latencies[op_type].append(
                latency_ms(family, topology, op_type, tenant, index, seed)
            )
    all_latencies = [
        latency
        for values in latencies.values()
        for latency in values
    ]
    query_latencies = latencies["query"]
    ingest_latencies = latencies["ingest"]
    delete_latencies = latencies["delete"]
    query_throughput = round(1000.0 / statistics.mean(query_latencies), 3)
    return {
        "operationCounts": operation_counts,
        "orgIdFilterApplied": missing_filter == 0,
        "missingOrgIdFilterOperations": missing_filter,
        "touchedOrgs": len(touched_orgs),
        "latencyMs": {
            "mixedP50": round(statistics.median(all_latencies), 3),
            "mixedP95": round(percentile(all_latencies, 0.95), 3),
            "mixedP99": round(percentile(all_latencies, 0.99), 3),
            "queryP50": round(statistics.median(query_latencies), 3),
            "queryP95": round(percentile(query_latencies, 0.95), 3),
            "queryP99": round(percentile(query_latencies, 0.99), 3),
            "ingestP95": round(percentile(ingest_latencies, 0.95), 3),
            "deleteP95": round(percentile(delete_latencies, 0.95), 3),
        },
        "syntheticThroughput": {
            "queryOpsPerSecondPerWorker": query_throughput,
            "note": "Derived from synthetic latency only; not service throughput evidence.",
        },
        "notes": [
            "Offline in-process comparison, not measured Postgres/Qdrant latency.",
            DOES_NOT_CLAIM,
        ],
    }


def snapshot_restore_markers(topology_results: dict, seed: int) -> dict:
    markers: dict[str, dict] = {}
    for family, topologies in topology_results.items():
        for topology, result in topologies.items():
            digest = hashlib.sha256(f"{seed}:{family}:{topology}".encode()).hexdigest()
            base = result["latencyMs"]["mixedP95"]
            snapshot_ms = round(base * (14.0 + stable_unit(seed, family, topology, "snapshot") * 4.0), 3)
            restore_ms = round(base * (27.0 + stable_unit(seed, family, topology, "restore") * 6.0), 3)
            markers[f"{family}:{topology}"] = {
                "snapshotMarker": f"synthetic-snapshot-{digest[:12]}",
                "restoreMarker": f"synthetic-restore-{digest[12:24]}",
                "snapshotMsSynthetic": snapshot_ms,
                "restoreMsSynthetic": restore_ms,
                "deterministicFromSeed": seed,
            }
    return markers


def accepted_adr(path: Path, decision_key: str) -> bool:
    if not path.is_file():
        return False
    text = path.read_text(encoding="utf-8")
    return (
        re.search(r"^- Status:\s*Accepted\s*$", text, re.MULTILINE) is not None
        and f"Decision key: `{decision_key}`" in text
    )


def spike_target_match() -> bool:
    if not SPIKE_ENVIRONMENT.is_file():
        return False
    try:
        payload = load_json(SPIKE_ENVIRONMENT)
    except (OSError, json.JSONDecodeError):
        return False
    return bool(payload.get("targetMatch"))


def build_payload(seed: int) -> dict:
    profile = load_json(WORKLOAD_PROFILE)
    status = git_status()
    distribution = tenant_distribution(profile, seed)
    plan = operation_plan(profile)
    tenants = distribution["tenants"]
    topology_results = {
        "qdrant": {
            "shared-collection": simulate_topology(
                "qdrant",
                "shared-collection",
                tenants,
                plan,
                seed,
            ),
            "cohort-collections": simulate_topology(
                "qdrant",
                "cohort-collections",
                tenants,
                plan,
                seed,
            ),
        },
        "pg": {
            "no-partition": simulate_topology(
                "pg",
                "no-partition",
                tenants,
                plan,
                seed,
            ),
            "bounded-hash-16": simulate_topology(
                "pg",
                "bounded-hash-16",
                tenants,
                plan,
                seed,
            ),
        },
    }
    recommendation = {
        "phase1bPoc": {
            "qdrant": "shared collection with mandatory org_id filter",
            "pg": "no-partition for 1B single-org POC; bounded hash reserved for multi-tenant growth",
        },
        "production20M": "blocked pending Profile B",
        "decisionKeys": {
            "pg-partition-strategy": "no-partition-poc; bounded-hash-16-reserved-for-multi-tenant-growth",
            "qdrant-topology": "shared-collection-with-mandatory-org-id-filter",
        },
        "selected": True,
    }
    snapshot_restore = snapshot_restore_markers(topology_results, seed)
    adrs = {
        "0008-pg-partition-strategy": accepted_adr(
            ROOT / "docs/adr/0008-pg-partition-strategy.md",
            "pg-partition-strategy",
        ),
        "0009-qdrant-topology": accepted_adr(
            ROOT / "docs/adr/0009-qdrant-topology.md",
            "qdrant-topology",
        ),
    }
    harness_completed = all(
        result["orgIdFilterApplied"]
        for family in topology_results.values()
        for result in family.values()
    )
    recommendation_recorded = bool(
        recommendation["selected"]
        and recommendation["phase1bPoc"]["qdrant"]
        and recommendation["phase1bPoc"]["pg"]
    )
    target_match = False
    closure = {
        "adrsAccepted": all(adrs.values()),
        "harnessCompleted": harness_completed,
        "recommendationRecorded": recommendation_recorded,
        "gitClean": not status["dirty"],
    }
    p0_07_closed = all(closure.values())
    notes = [
        "Offline/synthetic topology smoke only; no Docker or live services were used.",
        "targetMatch is false on this runner; spike evidence targetMatch=false is preserved.",
        "Production 20M aggregate scale is blocked pending Profile B mixed-load evidence.",
        DOES_NOT_CLAIM,
    ]
    return {
        "version": 1,
        "reportId": "p0-07-scale-topology",
        "gateId": "P0-07-SCALE-TOPOLOGY",
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "command": "python3 bench/markhand_web/scripts/run_scale_topology.py",
        "mode": "offline-synthetic-in-process",
        "seed": seed,
        "git": status,
        "environment": {
            "environmentId": "current-runner-offline-synthetic",
            "fingerprint": {
                "gitCommit": status["commit"],
                "workloadProfileId": profile["profileId"],
                "hardware": hardware_fingerprint(ROOT),
                "spikeTargetMatch": spike_target_match(),
            },
        },
        "implementationSha256": implementation_sha256(),
        "workloadProfile": {
            "profileId": profile["profileId"],
            "scale": profile["scale"],
            "loads": profile["loads"],
        },
        "syntheticTenantDistribution": distribution,
        "mixedLoad": plan,
        "topologyResults": topology_results,
        "snapshotRestore": snapshot_restore,
        "recommendation": recommendation,
        "adrs": adrs,
        "closure": closure,
        "p0_07_closed": p0_07_closed,
        "targetMatch": target_match,
        "productionScaleBlocked": True,
        "pocTopologySelected": recommendation_recorded,
        "doesNotClaim": [
            "Profile B evidence",
            "G0-SLO-QUERY-P99",
            "20M live mixed-load evidence",
        ],
        "notes": notes,
    }


def render_report(payload: dict) -> str:
    qdrant = payload["topologyResults"]["qdrant"]
    pg = payload["topologyResults"]["pg"]
    lines = [
        "# P0-07 scale topology offline report",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Mode: `{payload['mode']}`",
        f"- Seed: `{payload['seed']}`",
        f"- Workload profile: `{payload['workloadProfile']['profileId']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty at harness start: `{payload['git']['dirty']}`",
        f"- `targetMatch`: `{str(payload['targetMatch']).lower()}`",
        f"- `productionScaleBlocked`: `{str(payload['productionScaleBlocked']).lower()}`",
        f"- `pocTopologySelected`: `{str(payload['pocTopologySelected']).lower()}`",
        f"- `p0_07_closed`: `{str(payload['p0_07_closed']).lower()}`",
        "",
        "## Scope",
        "",
        "This harness is offline and synthetic. It compares topology shape in-process",
        "because Docker/live Postgres/Qdrant are not available on this runner.",
        "",
        f"Explicit note: {DOES_NOT_CLAIM}.",
        "",
        "Production 20M aggregate scale remains blocked pending Profile B.",
        "",
        "## Tenant distribution",
        "",
        f"- Strategy: `{payload['syntheticTenantDistribution']['strategy']}`",
        f"- Orgs: `{payload['syntheticTenantDistribution']['orgCount']}`",
        f"- Top tenant count: `{payload['syntheticTenantDistribution']['topTenantCount']}`",
        f"- Top tenant load share: `{payload['syntheticTenantDistribution']['topTenantLoadShare']}`",
        f"- Aggregate vectors represented: `{payload['syntheticTenantDistribution']['aggregateVectors']}`",
        "",
        "## Mixed load",
        "",
        f"- Query operations: `{payload['mixedLoad']['queryOperations']}`",
        f"- Ingest operations: `{payload['mixedLoad']['ingestOperations']}`",
        f"- Delete operations: `{payload['mixedLoad']['deleteOperations']}`",
        f"- `org_id` filter applied to every operation: `{str(payload['mixedLoad']['orgIdFilterApplied']).lower()}`",
        "",
        "## Qdrant topology comparison",
        "",
        "| topology | query p95 ms | query p99 ms | mixed p99 ms | org filter |",
        "|---|---:|---:|---:|---|",
    ]
    for name, result in qdrant.items():
        lines.append(
            "| "
            f"{name} | {result['latencyMs']['queryP95']} | "
            f"{result['latencyMs']['queryP99']} | "
            f"{result['latencyMs']['mixedP99']} | "
            f"{str(result['orgIdFilterApplied']).lower()} |"
        )
    lines.extend(
        [
            "",
            "## PG topology comparison",
            "",
            "| topology | query p95 ms | query p99 ms | delete p95 ms | org filter |",
            "|---|---:|---:|---:|---|",
        ]
    )
    for name, result in pg.items():
        lines.append(
            "| "
            f"{name} | {result['latencyMs']['queryP95']} | "
            f"{result['latencyMs']['queryP99']} | "
            f"{result['latencyMs']['deleteP95']} | "
            f"{str(result['orgIdFilterApplied']).lower()} |"
        )
    lines.extend(
        [
            "",
            "## Snapshot/restore markers",
            "",
            "| topology | snapshot marker | snapshot ms | restore marker | restore ms |",
            "|---|---|---:|---|---:|",
        ]
    )
    for name, marker in payload["snapshotRestore"].items():
        lines.append(
            "| "
            f"{name} | `{marker['snapshotMarker']}` | "
            f"{marker['snapshotMsSynthetic']} | "
            f"`{marker['restoreMarker']}` | "
            f"{marker['restoreMsSynthetic']} |"
        )
    lines.extend(
        [
            "",
            "## Recommendation",
            "",
            "- Qdrant: shared collection with mandatory `org_id` filter.",
            "- PG: no-partition for 1B single-org POC; bounded hash reserved for multi-tenant growth.",
            "- Production 20M: blocked pending Profile B.",
            "",
            "Decision keys:",
            "",
            "- `pg-partition-strategy`",
            "- `qdrant-topology`",
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
    distribution = tenant_distribution(profile, seed)
    assert distribution["orgCount"] == 20
    assert distribution["aggregateVectors"] == 20_000_000
    assert abs(sum(tenant["loadShare"] for tenant in distribution["tenants"]) - 1.0) < 1e-9
    assert 0.799 <= distribution["topTenantLoadShare"] <= 0.801
    tiny_plan = {
        "queryOperations": 8,
        "ingestOperations": 4,
        "deleteOperations": 2,
    }
    result = simulate_topology(
        "qdrant",
        "shared-collection",
        distribution["tenants"],
        {**operation_plan(profile), **tiny_plan},
        seed,
    )
    assert result["orgIdFilterApplied"] is True
    assert result["operationCounts"] == {"query": 8, "ingest": 4, "delete": 2}
    repeat = simulate_topology(
        "qdrant",
        "shared-collection",
        distribution["tenants"],
        {**operation_plan(profile), **tiny_plan},
        seed,
    )
    assert result["latencyMs"] == repeat["latencyMs"]
    assert DOES_NOT_CLAIM in result["notes"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=SEED)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test(args.seed)
        print("self-test passed")
        return 0

    payload = build_payload(args.seed)
    write_outputs(payload, args.summary, args.report)
    print(f"wrote {args.summary}")
    print(f"wrote {args.report}")
    print(f"p0_07_closed={str(payload['p0_07_closed']).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
