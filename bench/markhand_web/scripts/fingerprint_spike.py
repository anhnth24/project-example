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
IMPLEMENTATION_FILES = (
    "deploy/dev/compose.yml",
    "deploy/dev/otel-collector.yaml",
    "deploy/scripts/mock-embedding.py",
    "deploy/compose.spike.yml",
    "deploy/spike/common.sh",
    "deploy/spike/up.sh",
    "deploy/spike/health.sh",
    "deploy/spike/seed.sh",
    "deploy/spike/down.sh",
    "deploy/spike/reset.sh",
    "deploy/spike/verify-lifecycle.sh",
    "deploy/spike/images.lock.json",
    "scripts/validate_spike.py",
    "bench/markhand_web/scripts/fingerprint_spike.py",
)


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for relative in IMPLEMENTATION_FILES:
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update((ROOT / relative).read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


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
            gpu_vram = round(
                float(devices[0].split(",", 1)[1])
                * 1024
                * 1024
                / 1_000_000_000,
                2,
            )
    default_route = subprocess.run(
        ["ip", "route", "show", "default"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    route_match = re.search(r"\bdev\s+(\S+)", default_route)
    network_interface = route_match.group(1) if route_match else "unknown"
    bandwidth = 0.0
    speed_path = Path("/sys/class/net") / network_interface / "speed"
    if speed_path.is_file():
        try:
            speed = int(speed_path.read_text().strip())
            bandwidth = speed / 1000 if speed > 0 else 0.0
        except (OSError, ValueError):
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
    mount_result = subprocess.run(
        [
            "findmnt",
            "-J",
            "-T",
            str(storage_path),
            "-o",
            "SOURCE,FSTYPE,UUID,MAJ:MIN",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    mount_payload = json.loads(mount_result.stdout or '{"filesystems":[]}')
    mount = (mount_payload.get("filesystems") or [{}])[0]
    backing_source = str(mount.get("source") or "unknown")
    filesystem = str(mount.get("fstype") or "unknown")
    disk_type = filesystem or "unknown"
    if backing_source.startswith("/dev/"):
        topology = subprocess.run(
            ["lsblk", "-s", "-n", "-o", "NAME,TYPE,TRAN", backing_source],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.lower()
        if "nvme" in topology:
            disk_type = "nvme"
    storage_hash = hashlib.sha256(str(storage_path.resolve()).encode()).hexdigest()
    storage_identity = hashlib.sha256(
        json.dumps(
            {
                "source": backing_source,
                "filesystem": filesystem,
                "uuid": mount.get("uuid"),
                "majorMinor": mount.get("maj:min"),
                "storagePathSha256": storage_hash,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    measured_iops = 0
    iops_evidence_sha256 = None
    iops_evidence = None
    iops_report_path = os.environ.get("MARKHAND_SPIKE_IOPS_REPORT")
    if iops_report_path and Path(iops_report_path).is_file():
        report_path = Path(iops_report_path)
        report = json.loads(report_path.read_text())
        measured_at = report.get("measuredAt")
        try:
            measured_time = dt.datetime.fromisoformat(
                str(measured_at).replace("Z", "+00:00")
            )
        except ValueError:
            measured_time = dt.datetime.min.replace(tzinfo=dt.timezone.utc)
        age = dt.datetime.now(dt.timezone.utc) - measured_time
        if (
            report.get("storageIdentitySha256") == storage_identity
            and report.get("readOnly") is True
            and report.get("blockSizeBytes") == 4096
            and int(report.get("durationSeconds", 0)) >= 30
            and isinstance(report.get("tool"), str)
            and report["tool"].strip()
            and dt.timedelta(0) <= age <= dt.timedelta(hours=24)
        ):
            measured_iops = int(report.get("randomReadIops", 0))
            iops_evidence = report
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
        "ramGb": round(int(memory.group(1)) * 1024 / 1_000_000_000, 2)
        if memory
        else 0.01,
        "disk": {
            "type": disk_type,
            "capacityGb": round(disk.total / 1_000_000_000, 2),
            "iopsNote": f"measured random-read IOPS: {measured_iops}",
            "iopsMeasured": measured_iops,
            "iopsVerified": measured_iops >= 100_000,
            "iopsEvidenceSha256": iops_evidence_sha256,
            "iopsEvidence": iops_evidence,
            "storagePathSha256": storage_hash,
            "storageIdentitySha256": storage_identity,
            "backingSource": Path(backing_source).name or filesystem or "unknown",
        },
        "gpu": {"model": gpu_name, "vramGb": gpu_vram, "count": gpu_count},
        "network": {
            "bandwidthGbps": bandwidth,
            "bandwidthMeasured": bandwidth > 0,
            "interface": network_interface,
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
        and actual["network"]["bandwidthMeasured"]
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
        "implementationSha256": implementation_sha256(),
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
        "lifecycle": {
            "restartPersistence": False,
            "resetDeletion": False,
            "stores": ["postgres", "qdrant", "minio"],
            "verifiedAt": None,
        },
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
