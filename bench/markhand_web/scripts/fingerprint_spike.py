#!/usr/bin/env python3
"""Emit a non-secret environment fingerprint for the disposable benchmark spike."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_REPORT = ROOT / "bench/markhand_web/reports/spike-environment.json"


def run(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def load_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key] = value
    return values


def hardware() -> dict:
    cpuinfo = Path("/proc/cpuinfo").read_text(errors="replace")
    vendor = re.search(r"vendor_id\s*:\s*(.+)", cpuinfo)
    model = re.search(r"model name\s*:\s*(.+)", cpuinfo)
    meminfo = Path("/proc/meminfo").read_text()
    memory = re.search(r"MemTotal:\s*(\d+)", meminfo)
    disk = shutil.disk_usage(ROOT)
    gpu_name = "none"
    gpu_vram = 0.0
    gpu_count = 0
    if shutil.which("nvidia-smi"):
        result = subprocess.run(
            [
                "nvidia-smi",
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        devices = [line for line in result.stdout.splitlines() if line.strip()]
        if devices:
            gpu_count = len(devices)
            gpu_name = devices[0].split(",", 1)[0].strip()
            gpu_vram = round(float(devices[0].split(",", 1)[1]) / 1024, 2)
    speed_path = Path("/sys/class/net/eth0/speed")
    bandwidth = 1.0
    if speed_path.is_file():
        try:
            bandwidth = max(1.0, int(speed_path.read_text().strip()) / 1000)
        except ValueError:
            pass
    return {
        "cpu": {
            "vendor": vendor.group(1).strip() if vendor else "unknown",
            "model": model.group(1).strip() if model else "unknown",
            "cores": os.cpu_count() or 1,
            "threads": os.cpu_count() or 1,
        },
        "ramGb": round(int(memory.group(1)) / 1024 / 1024, 2) if memory else 0.01,
        "disk": {
            "type": "local-or-overlay",
            "capacityGb": round(disk.total / 1024**3, 2),
            "iopsNote": "not measured by P0-04 smoke",
        },
        "gpu": {"model": gpu_name, "vramGb": gpu_vram, "count": gpu_count},
        "network": {
            "bandwidthGbps": bandwidth,
            "latencyMsAssumed": 1,
        },
        "os": {"distro": platform.platform(), "arch": platform.machine()},
    }


def compose(env_file: Path, project: str) -> list[str]:
    return [
        "docker",
        "compose",
        "--env-file",
        str(env_file),
        "--project-name",
        project,
        "-f",
        str(ROOT / "deploy/dev/compose.yml"),
        "-f",
        str(ROOT / "deploy/compose.spike.yml"),
    ]


def meets_reference_target(actual: dict) -> bool:
    target = json.loads(
        (
            ROOT
            / "bench/markhand_web/environments/on-prem-reference.yaml"
        ).read_text()
    )
    return (
        actual["cpu"]["cores"] >= target["cpu"]["cores"]
        and actual["cpu"]["threads"] >= target["cpu"]["threads"]
        and actual["ramGb"] >= target["ramGb"]
        and actual["disk"]["capacityGb"] >= target["disk"]["capacityGb"]
        and actual["gpu"]["count"] >= target["gpu"]["count"]
        and actual["gpu"]["vramGb"] >= target["gpu"]["vramGb"]
        and actual["network"]["bandwidthGbps"]
        >= target["network"]["bandwidthGbps"]
    )


def fingerprint(env_file: Path) -> dict:
    environment = os.environ.copy()
    environment.update(load_env(env_file))
    project = environment.get("MARKHAND_COMPOSE_PROJECT", "markhand-spike")
    command = compose(env_file, project)
    rendered = subprocess.check_output(
        [*command, "config"],
        cwd=ROOT,
        env=environment,
    )
    services = ("postgres", "qdrant", "minio", "otel", "mock-embedding")
    images: dict[str, str] = {}
    versions: dict[str, str] = {}
    for service in services:
        container_id = run(*command, "ps", "-q", service)
        if not container_id:
            raise RuntimeError(f"spike service is not running: {service}")
        image_id = run("docker", "inspect", "--format", "{{.Image}}", container_id)
        image_ref = run(
            "docker",
            "inspect",
            "--format",
            "{{.Config.Image}}",
            container_id,
        )
        repo_digests = run(
            "docker",
            "image",
            "inspect",
            "--format",
            "{{json .RepoDigests}}",
            image_id,
        )
        images[service] = repo_digests
        versions[service] = image_ref
    fixture_hash = hashlib.sha256(
        (ROOT / "bench/markhand_web/manifest.lock.json").read_bytes()
    ).hexdigest()
    git_commit = run("git", "rev-parse", "HEAD")
    git_dirty = bool(run("git", "status", "--porcelain", "--untracked-files=no"))
    actual_hardware = hardware()
    return {
        "version": 1,
        "reportId": "p0-04-spike-smoke",
        "gateId": "P0-04-SPIKE",
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "git": {"commit": git_commit, "dirty": git_dirty},
        "command": "deploy/spike/up.sh",
        "workloadProfileId": "on-prem-reference-v1",
        "environment": {
            "environmentId": "current-runner-spike-smoke",
            "fingerprint": {
                "gitCommit": git_commit,
                "workloadProfileId": "on-prem-reference-v1",
                "composeFileSha256": hashlib.sha256(rendered).hexdigest(),
                "imageDigests": images,
                "serviceVersions": versions,
                "fixtureManifestSha256": fixture_hash,
                "hardware": actual_hardware,
            },
        },
        "fixtures": {"manifestSha256": fixture_hash},
        "result": {
            "metric": "healthy_spike_services",
            "value": len(services),
            "pass": True,
        },
        "targetMatch": meets_reference_target(actual_hardware),
        "notes": "IOPS must be independently verified; target Profile B gates require targetMatch=true.",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--env-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=DEFAULT_REPORT)
    args = parser.parse_args()
    payload = fingerprint(args.env_file.resolve())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n")
    print(f"wrote spike fingerprint to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
