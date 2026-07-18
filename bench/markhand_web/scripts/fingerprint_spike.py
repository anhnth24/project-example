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


def hardware(storage_path: Path) -> dict:
    cpuinfo = Path("/proc/cpuinfo").read_text(errors="replace")
    vendor = re.search(r"vendor_id\s*:\s*(.+)", cpuinfo)
    model = re.search(r"model name\s*:\s*(.+)", cpuinfo)
    meminfo = Path("/proc/meminfo").read_text()
    memory = re.search(r"MemTotal:\s*(\d+)", meminfo)
    disk = shutil.disk_usage(storage_path)
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
    physical_cores = {
        (physical, core)
        for physical, core in re.findall(
            r"physical id\s*:\s*(\d+).*?core id\s*:\s*(\d+)",
            cpuinfo,
            re.DOTALL,
        )
    }
    os_release = {}
    release_path = Path("/etc/os-release")
    if release_path.is_file():
        for line in release_path.read_text().splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                os_release[key] = value.strip('"')
    mount = subprocess.run(
        ["findmnt", "-T", str(storage_path), "-n", "-o", "SOURCE,FSTYPE"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    backing_source, _, filesystem = mount.partition(" ")
    disk_type = "nvme" if "nvme" in backing_source.lower() else filesystem or "unknown"
    storage_hash = hashlib.sha256(str(storage_path.resolve()).encode()).hexdigest()
    measured_iops = 0
    iops_evidence_sha256 = None
    iops_report_path = os.environ.get("MARKHAND_SPIKE_IOPS_REPORT")
    if iops_report_path and Path(iops_report_path).is_file():
        report_path = Path(iops_report_path)
        report = json.loads(report_path.read_text())
        if report.get("storagePathSha256") == storage_hash:
            measured_iops = int(report.get("randomReadIops", 0))
            iops_evidence_sha256 = hashlib.sha256(
                report_path.read_bytes()
            ).hexdigest()
    return {
        "cpu": {
            "vendor": vendor.group(1).strip() if vendor else "unknown",
            "model": model.group(1).strip() if model else "unknown",
            "cores": len(physical_cores) or (os.cpu_count() or 1),
            "threads": os.cpu_count() or 1,
            "physicalCoresMeasured": bool(physical_cores),
        },
        "ramGb": round(int(memory.group(1)) / 1024 / 1024, 2) if memory else 0.01,
        "disk": {
            "type": disk_type,
            "capacityGb": round(disk.total / 1024**3, 2),
            "iopsNote": f"measured random-read IOPS: {measured_iops}",
            "iopsMeasured": measured_iops,
            "iopsVerified": measured_iops >= 100_000,
            "iopsEvidenceSha256": iops_evidence_sha256,
            "storagePathSha256": storage_hash,
            "backingSource": Path(backing_source).name or filesystem or "unknown",
        },
        "gpu": {"model": gpu_name, "vramGb": gpu_vram, "count": gpu_count},
        "network": {
            "bandwidthGbps": bandwidth,
            "latencyMsAssumed": 1,
        },
        "os": {
            "distro": (
                f"{os_release.get('ID', platform.system())}-"
                f"{os_release.get('VERSION_ID', platform.release())}"
            ),
            "arch": platform.machine(),
        },
    }


def compose(
    env_file: Path,
    project: str,
    gpu_enabled: bool,
    nested_enabled: bool,
) -> list[str]:
    command = [
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
    if nested_enabled:
        command.extend(
            ["-f", str(ROOT / "deploy/spike/compose.nested.yml")]
        )
    if gpu_enabled:
        command.extend(["--profile", "gpu"])
    return command


def meets_reference_target(
    actual: dict,
    gpu_enabled: bool,
    nested_enabled: bool,
) -> bool:
    target = json.loads(
        (
            ROOT
            / "bench/markhand_web/environments/on-prem-reference.yaml"
        ).read_text()
    )
    return (
        gpu_enabled
        and not nested_enabled
        and actual["cpu"]["physicalCoresMeasured"]
        and actual["cpu"]["cores"] >= target["cpu"]["cores"]
        and actual["cpu"]["threads"] >= target["cpu"]["threads"]
        and actual["ramGb"] >= target["ramGb"]
        and actual["disk"]["capacityGb"] >= target["disk"]["capacityGb"]
        and actual["disk"]["type"] == target["disk"]["type"]
        and actual["disk"]["iopsVerified"]
        and isinstance(actual["disk"]["iopsEvidenceSha256"], str)
        and actual["gpu"]["count"] >= target["gpu"]["count"]
        and actual["gpu"]["vramGb"] >= target["gpu"]["vramGb"]
        and actual["network"]["bandwidthGbps"]
        >= target["network"]["bandwidthGbps"]
        and actual["os"]["arch"] == target["os"]["arch"]
        and actual["os"]["distro"] == target["os"]["distro"]
    )


def fingerprint(env_file: Path) -> dict:
    environment = load_env(env_file)
    environment.update(os.environ)
    project = environment.get("MARKHAND_COMPOSE_PROJECT", "markhand-spike")
    gpu_enabled = environment.get("SPIKE_GPU", "0") == "1"
    nested_enabled = environment.get("MARKHAND_SPIKE_NESTED", "0") == "1"
    command = compose(env_file, project, gpu_enabled, nested_enabled)
    rendered = subprocess.check_output(
        [*command, "config"],
        cwd=ROOT,
        env=environment,
    )
    runtime_services = ["postgres", "qdrant", "minio", "otel", "mock-embedding"]
    image_services = [*runtime_services, "minio-init"]
    if gpu_enabled:
        runtime_services.append("vllm")
        image_services.append("vllm")
    images: dict[str, str] = {}
    versions: dict[str, str] = {}
    for service in image_services:
        container_id = run(*command, "ps", "--all", "-q", service)
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
    fixture_manifest = ROOT / "bench/markhand_web/manifest.lock.json"
    if not fixture_manifest.is_file():
        fixture_manifest = ROOT / "tests/fixtures/manifest.json"
    fixture_hash = hashlib.sha256(fixture_manifest.read_bytes()).hexdigest()
    git_commit = run("git", "rev-parse", "HEAD")
    git_dirty = bool(run("git", "status", "--porcelain", "--untracked-files=no"))
    docker_root = Path(
        run("docker", "info", "--format", "{{.DockerRootDir}}")
    )
    actual_hardware = hardware(docker_root)
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
        "fixtureManifestPath": fixture_manifest.relative_to(ROOT).as_posix(),
        "result": {
            "metric": "healthy_spike_services",
            "value": len(runtime_services),
            "pass": True,
        },
        "profiles": [
            "gpu" if gpu_enabled else "cpu-smoke",
            *(
                ["nested-network-workaround"]
                if nested_enabled
                else []
            ),
        ],
        "targetMatch": meets_reference_target(
            actual_hardware,
            gpu_enabled,
            nested_enabled,
        ),
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
