#!/usr/bin/env python3
"""P1B-F02 boot evidence helpers: sanitize, validate, report, --self-test.

Machine-verifiable report schema is consumed by O04 (`composeProject` + `imageIds`).
Never dumps Config.Env or other secret-bearing docker inspect fields into committed
raw artifacts. Nested/no-limit/vfs hosts may generate evidence but cannot reach
`passed=true` / Done qualification.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
ROOT = SCRIPT_DIR.parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import redact_secrets as rs  # noqa: E402

ISSUE = "P1B-F02"
DEFAULT_COMPOSE_PROJECT = "markhand-poc"
COMPOSE_FILE = ROOT / "deploy" / "compose.poc.yml"

# Align with O04 expected POC services for imageId matching.
EXPECTED_POC_SERVICES = [
    "api",
    "minio",
    "postgres",
    "qdrant",
    "worker-convert",
    "worker-index",
]

# Resource-limit checks (hardened app/worker surfaces).
LIMIT_SERVICES = [
    "api",
    "worker-convert",
    "worker-index",
    "worker-embedding",
]

NONSTANDARD_STORAGE_DRIVERS = frozenset({"vfs", "fuse-overlayfs"})

# Pinned alpine already used by POC mock-embedding — available after poc-up without
# inventing a new third-party pin. Probe runs on the convert network only.
DEFAULT_EGRESS_PROBE_IMAGE = (
    "python:3.12.12-alpine@sha256:"
    "2d91681153dd4b8cdb52d4fd34a17b9edbafa4dd3086143cfd4b6c3a84c1acb0"
)

INSPECT_ALLOWLIST_TOP = ("Id", "Name", "Created", "Image", "Config", "HostConfig", "NetworkSettings", "State")
CONFIG_ALLOWLIST = ("User", "Image")
HOST_ALLOWLIST = (
    "ReadonlyRootfs",
    "SecurityOpt",
    "CapDrop",
    "Memory",
    "NanoCpus",
    "CpuPeriod",
    "CpuQuota",
    "PidsLimit",
)
STATE_ALLOWLIST = ("Status", "Running", "ExitCode", "Health", "OOMKilled", "Pid")
NETWORK_ENDPOINT_ALLOWLIST = (
    "IPAddress",
    "Gateway",
    "NetworkID",
    "EndpointID",
    "MacAddress",
    "Aliases",
)


def sanitize_inspect(raw: Any) -> list[dict[str, Any]]:
    """Return allowlisted docker inspect objects (no Env / Mounts / secrets)."""
    if isinstance(raw, dict):
        items = [raw]
    elif isinstance(raw, list):
        items = [x for x in raw if isinstance(x, dict)]
    else:
        raise TypeError("inspect payload must be object or array")

    out: list[dict[str, Any]] = []
    for item in items:
        cleaned: dict[str, Any] = {}
        for key in INSPECT_ALLOWLIST_TOP:
            if key not in item:
                continue
            if key == "Config" and isinstance(item[key], dict):
                cleaned[key] = {
                    k: item[key][k] for k in CONFIG_ALLOWLIST if k in item[key]
                }
            elif key == "HostConfig" and isinstance(item[key], dict):
                cleaned[key] = {
                    k: item[key][k] for k in HOST_ALLOWLIST if k in item[key]
                }
            elif key == "State" and isinstance(item[key], dict):
                state = {
                    k: item[key][k] for k in STATE_ALLOWLIST if k in item[key]
                }
                health = item[key].get("Health")
                if isinstance(health, dict):
                    state["Health"] = {
                        "Status": health.get("Status"),
                        "FailingStreak": health.get("FailingStreak"),
                    }
                cleaned[key] = state
            elif key == "NetworkSettings" and isinstance(item[key], dict):
                nets_in = item[key].get("Networks") or {}
                nets_out: dict[str, Any] = {}
                if isinstance(nets_in, dict):
                    for net_name, endpoint in nets_in.items():
                        if not isinstance(endpoint, dict):
                            nets_out[str(net_name)] = endpoint
                            continue
                        nets_out[str(net_name)] = {
                            k: endpoint[k]
                            for k in NETWORK_ENDPOINT_ALLOWLIST
                            if k in endpoint
                        }
                cleaned[key] = {"Networks": nets_out}
            else:
                cleaned[key] = item[key]
        out.append(cleaned)
    return out


def scan_committed_text(text: str) -> list[str]:
    """Return secret-finding labels (never values) for committed raw text/json."""
    return rs.broad_secret_scan(text)


def repo_relative_raw_dir(raw_dir: Path | str, root: Path | str = ROOT) -> str:
    raw = Path(raw_dir).resolve()
    base = Path(root).resolve()
    try:
        return str(raw.relative_to(base))
    except ValueError:
        return str(raw)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _nonzero_cpu(limits: dict[str, Any]) -> bool:
    nano = limits.get("nanoCpus")
    if isinstance(nano, (int, float)) and nano > 0:
        return True
    quota = limits.get("cpuQuota")
    period = limits.get("cpuPeriod")
    if isinstance(quota, (int, float)) and quota > 0:
        if period is None or (isinstance(period, (int, float)) and period > 0):
            return True
    return False


def evaluate_report(
    report: dict[str, Any],
    *,
    raw_root: Path | None = None,
) -> tuple[str, list[str]]:
    """Return (status, blockers). ``pass`` only when structural + security gates hold."""
    blockers: list[str] = []

    if report.get("issue") != ISSUE:
        blockers.append("issue_mismatch")

    for key in (
        "composeProject",
        "containerIds",
        "imageIds",
        "gitShaFull",
        "composeFileSha256",
        "dockerVersion",
        "composeVersion",
        "generatedAt",
        "pass_count",
        "fail_count",
        "storageDriver",
        "egressProbe",
        "resourceLimits",
        "rawDir",
        "redactionScan",
    ):
        if key not in report or report.get(key) in (None, ""):
            blockers.append(f"missing:{key}")

    project = report.get("composeProject")
    if not isinstance(project, str) or not project.strip():
        blockers.append("missing:composeProject")

    container_ids = report.get("containerIds") if isinstance(report.get("containerIds"), dict) else {}
    image_ids = report.get("imageIds") if isinstance(report.get("imageIds"), dict) else {}
    for svc in EXPECTED_POC_SERVICES:
        if svc not in container_ids or not container_ids.get(svc):
            blockers.append(f"missing_container:{svc}")
        if svc not in image_ids or not image_ids.get(svc):
            blockers.append(f"missing_image:{svc}")

    digests = report.get("imageDigests") if isinstance(report.get("imageDigests"), dict) else {}
    for svc, digest in digests.items():
        if not isinstance(digest, str) or "@sha256:" not in digest or digest.startswith("[]"):
            blockers.append(f"fake_digest:{svc}")

    egress = report.get("egressProbe") if isinstance(report.get("egressProbe"), dict) else {}
    if egress.get("executed") is not True:
        blockers.append("egress_not_executed")
    elif egress.get("toolMissing") is True:
        blockers.append("egress_tool_missing")
    elif egress.get("blocked") is not True:
        blockers.append("egress_not_blocked")

    if report.get("nolimitComposeUsed") is True:
        blockers.append("nolimit_compose")

    limits = report.get("resourceLimits") if isinstance(report.get("resourceLimits"), dict) else {}
    for svc in LIMIT_SERVICES:
        svc_limits = limits.get(svc) if isinstance(limits.get(svc), dict) else {}
        mem = svc_limits.get("memory")
        pids = svc_limits.get("pidsLimit")
        if not isinstance(mem, (int, float)) or mem <= 0:
            blockers.append(f"resource_limit_zero:{svc}:memory")
        if not _nonzero_cpu(svc_limits):
            blockers.append(f"resource_limit_zero:{svc}:cpu")
        if not isinstance(pids, (int, float)) or pids <= 0:
            blockers.append(f"resource_limit_zero:{svc}:pids")

    driver = str(report.get("storageDriver") or "").strip().lower()
    if not driver:
        blockers.append("missing:storageDriver")
    elif driver in NONSTANDARD_STORAGE_DRIVERS:
        # Nested/cloud DinD (vfs) cannot qualify as standard-host Done.
        blockers.append(f"nonstandard_storage:{driver}")

    if report.get("fail_count") not in (0, 0.0):
        blockers.append("fail_count_nonzero")

    if report.get("passed") is not True:
        blockers.append("passed_false")

    redaction = report.get("redactionScan") if isinstance(report.get("redactionScan"), dict) else {}
    if redaction.get("passed") is not True:
        blockers.append("redaction_failed")

    scan_root = raw_root
    if scan_root is None and report.get("rawDir"):
        candidate = Path(str(report["rawDir"]))
        if not candidate.is_absolute():
            candidate = ROOT / candidate
        if candidate.is_dir():
            scan_root = candidate
    if scan_root is not None and scan_root.is_dir():
        findings: list[str] = []
        for path in sorted(scan_root.rglob("*")):
            if not path.is_file():
                continue
            if path.suffix.lower() not in {".json", ".txt", ".md", ".err", ".out", ".log", ""}:
                # Still scan common text-like evidence files without extension.
                if path.suffix:
                    continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            labels = scan_committed_text(text)
            if labels:
                findings.extend(f"{path.name}:{label}" for label in labels)
        if findings:
            blockers.append("secret_in_raw:" + ",".join(findings[:8]))

    status = "pass" if not blockers else "fail"
    return status, blockers


def build_report_payload(
    *,
    stamp: str,
    fail: int,
    passes: list[str],
    fails: list[str],
    notes: list[str],
    compose_project: str,
    container_ids: dict[str, str],
    image_ids: dict[str, str],
    image_digests: dict[str, str],
    git_sha: str,
    git_sha_full: str,
    docker_version: str | None,
    compose_version: str | None,
    compose_file_sha256: str,
    storage_driver: str,
    nolimit_compose_used: bool,
    egress_probe: dict[str, Any],
    resource_limits: dict[str, dict[str, Any]],
    raw_dir: str,
    redaction_scan: dict[str, Any],
) -> dict[str, Any]:
    cgroup_ok = (not nolimit_compose_used) and all(
        isinstance(resource_limits.get(svc, {}).get("memory"), (int, float))
        and resource_limits[svc]["memory"] > 0
        and _nonzero_cpu(resource_limits.get(svc, {}))
        and isinstance(resource_limits.get(svc, {}).get("pidsLimit"), (int, float))
        and resource_limits[svc]["pidsLimit"] > 0
        for svc in LIMIT_SERVICES
        if svc in resource_limits
    )
    standard = (
        fail == 0
        and cgroup_ok
        and not nolimit_compose_used
        and storage_driver.strip().lower() not in NONSTANDARD_STORAGE_DRIVERS
        and egress_probe.get("executed") is True
        and egress_probe.get("blocked") is True
        and redaction_scan.get("passed") is True
        and all(svc in container_ids and svc in image_ids for svc in EXPECTED_POC_SERVICES)
    )
    payload = {
        "issue": ISSUE,
        "stamp_utc": stamp,
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat(),
        "passed": bool(standard),
        "pass_count": len(passes),
        "fail_count": len(fails),
        "passes": passes,
        "fails": fails,
        "notes": notes,
        "composeProject": compose_project,
        "containerIds": container_ids,
        "imageIds": image_ids,
        "imageDigests": image_digests,
        "gitSha": git_sha,
        "gitShaFull": git_sha_full,
        "dockerVersion": docker_version,
        "composeVersion": compose_version,
        "composeFileSha256": compose_file_sha256,
        "storageDriver": storage_driver,
        "nolimitComposeUsed": nolimit_compose_used,
        "cgroupLimitsEnforced": bool(cgroup_ok),
        "standardHostQualification": bool(standard),
        "egressProbe": egress_probe,
        "resourceLimits": resource_limits,
        "rawDir": raw_dir,
        "redactionScan": redaction_scan,
    }
    # Re-evaluate so `passed` cannot drift from structural gates.
    status, blockers = evaluate_report(payload)
    payload["passed"] = status == "pass"
    payload["standardHostQualification"] = payload["passed"]
    payload["evaluationBlockers"] = blockers
    return payload


def render_markdown(payload: dict[str, Any]) -> str:
    result = "PASS" if payload.get("passed") else "FAIL"
    lines = [
        "# P1B-F02 POC Docker boot evidence",
        "",
        f"- Stamp (UTC): `{payload.get('stamp_utc')}`",
        f"- Generated: `{payload.get('generatedAt')}`",
        f"- Result: `{result}`",
        f"- Passes: `{payload.get('pass_count')}` / Fails: `{payload.get('fail_count')}`",
        f"- Compose project: `{payload.get('composeProject')}`",
        f"- Git: `{payload.get('gitShaFull')}`",
        f"- Compose file SHA256: `{payload.get('composeFileSha256')}`",
        f"- Docker: `{payload.get('dockerVersion')}` / Compose: `{payload.get('composeVersion')}`",
        f"- Storage driver: `{payload.get('storageDriver')}`",
        f"- Standard-host qualification: `{payload.get('standardHostQualification')}`",
        f"- Raw artifacts: `{payload.get('rawDir')}`",
        "",
        "## Checks",
        "",
    ]
    for item in payload.get("passes") or []:
        lines.append(f"- PASS: {item}")
    for item in payload.get("fails") or []:
        lines.append(f"- FAIL: {item}")
    for item in payload.get("notes") or []:
        lines.append(f"- NOTE: {item}")
    blockers = payload.get("evaluationBlockers") or []
    if blockers:
        lines += ["", "## Evaluation blockers", ""]
        for b in blockers:
            lines.append(f"- `{b}`")
    lines += [
        "",
        "## Commands",
        "",
        "```bash",
        "cp deploy/.env.example deploy/.env",
        "deploy/scripts/poc-up.sh",
        "POC_EVIDENCE_RAW_DIR=bench/markhand_web/reports/phase-1b-gate/raw/f02-$(git rev-parse --short HEAD) \\",
        "  deploy/scripts/poc-boot-evidence.sh",
        "# Hermetic validator:",
        "deploy/scripts/poc-boot-evidence.sh --self-test",
        "```",
        "",
        "## Acceptance mapping",
        "",
        "| Criterion | Evidence |",
        "|---|---|",
        "| Clean host boot | `poc-up.sh` + `poc-health` |",
        "| API/worker images separated | distinct image refs + binary presence checks |",
        "| Isolation UID/cap/read_only/no-new-privileges | sanitized `inspect-*.json` / `isolation-*.txt` |",
        "| Convert no egress | convert `Internal=true` + executable network probe |",
        "| Resource limits nonzero | `resourceLimits` memory/cpu/pids |",
        "| Sandbox preflight | `sandbox-preflight.txt` |",
        "| Native format smoke | `format-*.md` |",
        "| O04 consumable metadata | `composeProject` + `imageIds` (+ digests when present) |",
        "",
    ]
    return "\n".join(lines) + "\n"


def write_report_files(json_path: Path, md_path: Path, payload: dict[str, Any]) -> None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    md_path.write_text(render_markdown(payload), encoding="utf-8")


def run_self_test() -> int:
    """Hermetic validator covering reject/accept fixtures (no Docker required)."""
    errors: list[str] = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            errors.append(f"{name}: {detail}" if detail else name)

    # 1) secret-bearing inspect rejected
    raw_inspect = [
        {
            "Id": "c1",
            "Name": "/x",
            "Image": "sha256:" + ("a" * 64),
            "Config": {
                "User": "10001:10001",
                "Image": "markhand-api:poc",
                "Env": ["MARKHAND_AUTH_SIGNING_KEY=supersecretvalue"],
            },
            "HostConfig": {
                "ReadonlyRootfs": True,
                "SecurityOpt": ["no-new-privileges:true"],
                "CapDrop": ["ALL"],
                "Memory": 1,
                "NanoCpus": 1,
                "PidsLimit": 1,
            },
            "NetworkSettings": {"Networks": {}},
            "State": {"Status": "running", "Running": True, "ExitCode": 0},
        }
    ]
    cleaned = sanitize_inspect(raw_inspect)
    blob = json.dumps(cleaned)
    check("sanitize_drops_env", "Env" not in blob and "supersecretvalue" not in blob, blob[:200])

    secret_text = (
        'POSTGRES_PASSWORD=hunter2\n'
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaaa.bbbb\n"
        "postgres://user:s3cret@db:5432/app\n"
        "MINIO_ROOT_PASSWORD=minio-secret\n"
    )
    findings = scan_committed_text(secret_text)
    check("secret_scan_finds", bool(findings), str(findings))

    services = list(EXPECTED_POC_SERVICES)

    def good() -> dict[str, Any]:
        return {
            "issue": ISSUE,
            "stamp_utc": "20260724T000000Z",
            "generatedAt": "2026-07-24T00:00:00+00:00",
            "passed": True,
            "pass_count": 10,
            "fail_count": 0,
            "passes": ["ok"],
            "fails": [],
            "notes": [],
            "composeProject": DEFAULT_COMPOSE_PROJECT,
            "containerIds": {svc: f"cid-{svc}" for svc in services},
            "imageIds": {svc: f"sha256:{i:064d}" for i, svc in enumerate(services)},
            "imageDigests": {"postgres": "postgres@sha256:" + ("e" * 64)},
            "gitSha": "abc1234",
            "gitShaFull": "abc1234deadbeef0000000000000000000000000",
            "dockerVersion": "24.0.0",
            "composeVersion": "2.24.0",
            "composeFileSha256": "f" * 64,
            "storageDriver": "overlay2",
            "nolimitComposeUsed": False,
            "cgroupLimitsEnforced": True,
            "standardHostQualification": True,
            "egressProbe": {
                "executed": True,
                "toolMissing": False,
                "blocked": True,
                "exitCode": 1,
                "raw": "wget: can't connect",
            },
            "resourceLimits": {
                svc: {
                    "memory": 268435456,
                    "nanoCpus": 500000000,
                    "pidsLimit": 128,
                }
                for svc in LIMIT_SERVICES
            },
            "rawDir": "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc1234",
            "redactionScan": {"passed": True, "findings": []},
        }

    # 2) missing service/image metadata rejected
    missing = good()
    missing["imageIds"] = {}
    missing["containerIds"] = {}
    status, blockers = evaluate_report(missing)
    check(
        "missing_metadata_rejected",
        status != "pass"
        and any(b.startswith("missing_image:") for b in blockers)
        and any(b.startswith("missing_container:") for b in blockers),
        str(blockers),
    )

    # 3) missing egress execution rejected
    no_egress = good()
    no_egress["egressProbe"] = {
        "executed": False,
        "toolMissing": True,
        "blocked": None,
        "raw": "curl absent",
    }
    status, blockers = evaluate_report(no_egress)
    check("egress_missing_rejected", status != "pass" and "egress_not_executed" in blockers, str(blockers))

    # 4) resource limit zero rejected
    zero = good()
    zero["resourceLimits"]["api"]["memory"] = 0
    status, blockers = evaluate_report(zero)
    check(
        "zero_limit_rejected",
        status != "pass" and any("resource_limit_zero:api:memory" == b for b in blockers),
        str(blockers),
    )

    # 5) complete fixture accepted
    status, blockers = evaluate_report(good())
    check("complete_accepted", status == "pass" and blockers == [], str(blockers))

    # bonus: nolimit + vfs honesty
    nested = good()
    nested["nolimitComposeUsed"] = True
    nested["storageDriver"] = "vfs"
    status, blockers = evaluate_report(nested)
    check(
        "nested_nolimit_rejected",
        status != "pass"
        and "nolimit_compose" in blockers
        and any(b.startswith("nonstandard_storage:") for b in blockers),
        str(blockers),
    )

    if errors:
        for err in errors:
            print(f"SELF-TEST FAIL: {err}", file=sys.stderr)
        return 1
    print("P1B-F02 self-test OK")
    return 0


def _cmd_text(args: list[str]) -> str | None:
    try:
        proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return None
    out = (proc.stdout or proc.stderr or "").strip()
    return out or None


def finalize_from_raw(
    *,
    json_path: Path,
    md_path: Path,
    raw_dir: Path,
    stamp: str,
    fail: int,
    compose_project: str,
    nolimit_compose_used: bool,
) -> dict[str, Any]:
    """Assemble report JSON/MD from shell-collected raw artifacts."""
    summary_path = raw_dir / "summary.txt"
    summary = (
        summary_path.read_text(encoding="utf-8", errors="replace").splitlines()
        if summary_path.is_file()
        else []
    )
    passes = [line[6:] for line in summary if line.startswith("PASS: ")]
    fails = [line[6:] for line in summary if line.startswith("FAIL: ")]
    notes = [line[6:] for line in summary if line.startswith("NOTE: ")]

    meta_path = raw_dir / "meta.json"
    meta: dict[str, Any] = {}
    if meta_path.is_file():
        meta = json.loads(meta_path.read_text(encoding="utf-8"))

    container_ids = dict(meta.get("containerIds") or {})
    image_ids = dict(meta.get("imageIds") or {})
    image_digests = dict(meta.get("imageDigests") or {})
    resource_limits = dict(meta.get("resourceLimits") or {})
    egress_probe = dict(meta.get("egressProbe") or {"executed": False})
    storage_driver = str(meta.get("storageDriver") or "")

    # Sanitize any inspect-*.json that still looks like full docker inspect.
    for path in sorted(raw_dir.glob("inspect-*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            fails.append(f"inspect unreadable: {path.name}")
            fail = 1
            continue
        cleaned = sanitize_inspect(data)
        path.write_text(json.dumps(cleaned, indent=2) + "\n", encoding="utf-8")

    findings: list[str] = []
    for path in sorted(raw_dir.rglob("*")):
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        labels = scan_committed_text(text)
        if labels:
            findings.extend(f"{path.name}:{label}" for label in labels)
    redaction_scan = {"passed": not findings, "findings": findings}
    if findings:
        fails.append("redaction scan found secret-like material in raw artifacts")
        fail = 1
        notes.append("redaction findings: " + ",".join(findings[:12]))

    try:
        git_sha_full = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
        git_sha = subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, text=True
        ).strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        git_sha_full = "unknown"
        git_sha = "unknown"

    docker_version = _cmd_text(["docker", "version", "--format", "{{.Server.Version}}"])
    compose_version = _cmd_text(["docker", "compose", "version", "--short"])
    if not compose_version:
        compose_version = _cmd_text(["docker", "compose", "version"])

    compose_sha = sha256_file(COMPOSE_FILE)
    raw_rel = repo_relative_raw_dir(raw_dir, ROOT)

    payload = build_report_payload(
        stamp=stamp,
        fail=fail,
        passes=passes,
        fails=fails,
        notes=notes,
        compose_project=compose_project,
        container_ids=container_ids,
        image_ids=image_ids,
        image_digests=image_digests,
        git_sha=git_sha,
        git_sha_full=git_sha_full,
        docker_version=docker_version,
        compose_version=compose_version,
        compose_file_sha256=compose_sha,
        storage_driver=storage_driver,
        nolimit_compose_used=nolimit_compose_used,
        egress_probe=egress_probe,
        resource_limits=resource_limits,
        raw_dir=raw_rel,
        redaction_scan=redaction_scan,
    )
    # Keep shell FAIL bits visible even when evaluation adds blockers.
    if fail != 0:
        payload["passed"] = False
        payload["standardHostQualification"] = False
        if "fail_count_nonzero" not in (payload.get("evaluationBlockers") or []):
            payload.setdefault("evaluationBlockers", []).append("shell_fail")
    write_report_files(json_path, md_path, payload)
    return payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run hermetic validator")
    parser.add_argument("--finalize", action="store_true", help="build report from raw dir")
    parser.add_argument("--json", type=Path, help="output poc-f02-boot.json path")
    parser.add_argument("--md", type=Path, help="output poc-f02-boot.md path")
    parser.add_argument("--raw-dir", type=Path, help="evidence raw directory")
    parser.add_argument("--stamp", default="")
    parser.add_argument("--fail", type=int, default=0)
    parser.add_argument(
        "--compose-project",
        default=os.environ.get("MARKHAND_COMPOSE_PROJECT", DEFAULT_COMPOSE_PROJECT),
    )
    parser.add_argument("--nolimit-compose", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if args.finalize:
        if not args.json or not args.md or not args.raw_dir:
            parser.error("--finalize requires --json --md --raw-dir")
        payload = finalize_from_raw(
            json_path=args.json,
            md_path=args.md,
            raw_dir=args.raw_dir,
            stamp=args.stamp or dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ"),
            fail=args.fail,
            compose_project=args.compose_project,
            nolimit_compose_used=bool(args.nolimit_compose),
        )
        print(f"wrote {args.json}")
        print(f"wrote {args.md}")
        return 0 if payload.get("passed") else 1

    parser.error("specify --self-test or --finalize")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
