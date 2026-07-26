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
import tempfile
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

REQUIRED_RUNTIME_SERVICES = [
    "api",
    "worker-convert",
    "worker-index",
    "worker-embedding",
    "worker-delete",
    "worker-reconcile",
]

REQUIRED_BASE_SERVICES = [
    "api",
    "minio",
    "postgres",
    "qdrant",
    "worker-convert",
    "worker-index",
    "worker-embedding",
    "worker-delete",
    "worker-reconcile",
]

# Resource-limit checks for long-lived POC services. `mock-embedding` is required
# because the default POC profile is `mock`; aiteamvn evidence may include
# `embedding-cpu` as an additional validated service.
LIMIT_SERVICES = [
    "api",
    "postgres",
    "qdrant",
    "minio",
    "mock-embedding",
    "worker-convert",
    "worker-index",
    "worker-embedding",
    "worker-delete",
    "worker-reconcile",
]
OPTIONAL_LIMIT_SERVICES = ["embedding-cpu"]
REQUIRED_NATIVE_FORMATS = ["csv", "docx", "html", "pdf", "png", "pptx", "txt", "xlsx"]

NONSTANDARD_STORAGE_DRIVERS = frozenset({"vfs", "fuse-overlayfs"})

# Pinned alpine already used by POC mock-embedding — available after poc-up without
# inventing a new third-party pin. Probe runs on the convert network only.
DEFAULT_EGRESS_PROBE_IMAGE = (
    "python:3.12.12-alpine@sha256:"
    "2d91681153dd4b8cdb52d4fd34a17b9edbafa4dd3086143cfd4b6c3a84c1acb0"
)

RAW_MANIFEST_NAME = "manifest.json"
HEX40_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
IMAGE_ID_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
CONTAINER_ID_RE = re.compile(r"^[0-9a-f]{12,64}$")
IMAGE_DIGEST_RE = re.compile(r"^[^@\s]+@sha256:[0-9a-f]{64}$")

INSPECT_ALLOWLIST_TOP = (
    "Id",
    "Name",
    "Created",
    "Image",
    "Config",
    "HostConfig",
    "Mounts",
    "NetworkSettings",
    "State",
)
CONFIG_ALLOWLIST = ("User", "Image")
HOST_ALLOWLIST = (
    "ReadonlyRootfs",
    "SecurityOpt",
    "CapDrop",
    "CapAdd",
    "Privileged",
    "Binds",
    "Mounts",
    "Devices",
    "Tmpfs",
    "Memory",
    "NanoCpus",
    "CpuPeriod",
    "CpuQuota",
    "PidsLimit",
)
MOUNT_ALLOWLIST = ("Type", "Name", "Destination", "Mode", "RW", "Propagation")
DEVICE_ALLOWLIST = ("PathInContainer", "CgroupPermissions")
STATE_ALLOWLIST = ("Status", "Running", "ExitCode", "Health", "OOMKilled", "Pid")
NETWORK_ENDPOINT_ALLOWLIST = (
    "IPAddress",
    "Gateway",
    "NetworkID",
    "EndpointID",
    "MacAddress",
    "Aliases",
)

TMPFS_REQUIRED_TARGETS = ("/tmp", "/var/lib/markhand")
TMPFS_REQUIRED_OPTIONS = frozenset({"rw", "noexec", "nosuid", "nodev"})


def _safe_host_config(value: dict[str, Any]) -> dict[str, Any]:
    cleaned: dict[str, Any] = {}
    for key in HOST_ALLOWLIST:
        if key not in value:
            continue
        item = value[key]
        if key == "Binds" and isinstance(item, list):
            # Do not preserve host source paths. The target/mode is enough to
            # reject host bind usage without leaking local filesystem layout.
            binds = []
            for raw in item:
                if not isinstance(raw, str):
                    continue
                parts = raw.split(":")
                binds.append(
                    {
                        "target": parts[1] if len(parts) >= 2 else "",
                        "mode": parts[2] if len(parts) >= 3 else "",
                    }
                )
            cleaned[key] = binds
        elif key == "Mounts" and isinstance(item, list):
            cleaned[key] = [
                {k: mount.get(k) for k in ("Type", "Target", "ReadOnly") if k in mount}
                for mount in item
                if isinstance(mount, dict)
            ]
        elif key == "Devices" and isinstance(item, list):
            cleaned[key] = [
                {k: device.get(k) for k in DEVICE_ALLOWLIST if k in device}
                for device in item
                if isinstance(device, dict)
            ]
        else:
            cleaned[key] = item
    return cleaned


def _safe_mounts(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [
        {k: mount.get(k) for k in MOUNT_ALLOWLIST if k in mount}
        for mount in value
        if isinstance(mount, dict)
    ]


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
                cleaned[key] = _safe_host_config(item[key])
            elif key == "Mounts":
                cleaned[key] = _safe_mounts(item[key])
            elif key == "State" and isinstance(item[key], dict):
                state = {k: item[key][k] for k in STATE_ALLOWLIST if k in item[key]}
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
        return raw.relative_to(base).as_posix()
    except ValueError:
        return str(raw).replace("\\", "/")


def is_safe_repo_relative_path(value: Any) -> bool:
    if not isinstance(value, str) or not value.strip():
        return False
    text = value.replace("\\", "/")
    if text.startswith("/") or re.match(r"^[A-Za-z]:/", text):
        return False
    parts = [part for part in text.split("/") if part]
    return bool(parts) and all(part not in (".", "..") for part in parts)


def resolve_repo_relative_dir(value: Any, root: Path = ROOT) -> Path | None:
    if not is_safe_repo_relative_path(value):
        return None
    path = (root / str(value).replace("\\", "/")).resolve()
    try:
        path.relative_to(root.resolve())
    except ValueError:
        return None
    return path


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git_state(root: Path = ROOT) -> dict[str, Any]:
    try:
        sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip()
        short = subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"], cwd=root, text=True
        ).strip()
        porcelain = subprocess.check_output(
            ["git", "status", "--porcelain"],
            cwd=root,
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return {
            "gitSha": "unknown",
            "gitShaFull": "unknown",
            "dirty": True,
            "porcelain": ["git unavailable"],
        }
    lines = [line for line in porcelain.splitlines() if line.strip()]
    return {
        "gitSha": short,
        "gitShaFull": sha,
        "dirty": bool(lines),
        "porcelain": lines,
    }


def _iter_manifest_files(raw_dir: Path) -> list[Path]:
    files = []
    for path in sorted(raw_dir.rglob("*")):
        if not path.is_file() or path.name == RAW_MANIFEST_NAME:
            continue
        files.append(path)
    return files


def build_raw_manifest(
    *,
    raw_dir: Path,
    raw_rel: str,
    compose_file_sha256: str,
    compose_blob_sha256: str,
    container_ids: dict[str, str],
    image_ids: dict[str, str],
    git: dict[str, Any],
    meta: dict[str, Any],
) -> dict[str, Any]:
    files = {
        path.relative_to(raw_dir).as_posix(): sha256_file(path)
        for path in _iter_manifest_files(raw_dir)
    }
    return {
        "schema": "markhand.p1b.f02.raw-manifest.v1",
        "issue": ISSUE,
        "rawDir": raw_rel,
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat(),
        "targetGitShaFull": git.get("gitShaFull"),
        "dirtyWorktree": {
            "dirty": bool(git.get("dirty")),
            "porcelain": list(git.get("porcelain") or []),
        },
        "composeFile": "deploy/compose.poc.yml",
        "composeFileSha256": compose_file_sha256,
        "composeBlobSha256": compose_blob_sha256,
        "containers": container_ids,
        "imageIds": image_ids,
        "imageDigests": dict(meta.get("imageDigests") or {}),
        "files": files,
    }


def write_raw_manifest(raw_dir: Path, manifest: dict[str, Any]) -> tuple[str, str]:
    path = raw_dir / RAW_MANIFEST_NAME
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return path.name, sha256_file(path)


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


def _suffix_in(name: str, suffixes: set[str]) -> bool:
    return any(name == suffix or name.endswith(f"_{suffix}") for suffix in suffixes)


def normalize_compose_profiles(value: Any) -> list[str]:
    if isinstance(value, list):
        parts = [str(item).strip() for item in value]
    elif isinstance(value, str):
        parts = re.split(r"[,\s]+", value.strip())
    else:
        parts = []
    profiles = sorted({part for part in parts if part})
    return profiles or ["mock"]


def expected_services_for_profiles(profiles_value: Any) -> list[str]:
    profiles = set(normalize_compose_profiles(profiles_value))
    services = list(REQUIRED_BASE_SERVICES)
    if "aiteamvn" in profiles:
        services.append("embedding-cpu")
    else:
        services.append("mock-embedding")
    return services


def _tmpfs_options(value: Any) -> set[str]:
    if isinstance(value, list):
        raw = ",".join(str(item) for item in value)
    else:
        raw = str(value or "")
    return {part.strip().lower() for part in raw.split(",") if part.strip()}


def _validate_runtime_security(report: dict[str, Any]) -> list[str]:
    blockers: list[str] = []
    runtime = (
        report.get("runtimeSecurity")
        if isinstance(report.get("runtimeSecurity"), dict)
        else {}
    )
    required_networks = {
        "api": {"edge", "private"},
        "worker-convert": {"convert"},
        "worker-index": {"private"},
        "worker-embedding": {"private"},
        "worker-delete": {"private"},
        "worker-reconcile": {"private"},
    }
    for svc in REQUIRED_RUNTIME_SERVICES:
        sec = runtime.get(svc) if isinstance(runtime.get(svc), dict) else {}
        if not sec:
            blockers.append(f"runtime_security_missing:{svc}")
            continue
        if sec.get("user") not in ("10001", "10001:10001"):
            blockers.append(f"runtime_security_user:{svc}")
        if sec.get("privileged") is not False:
            blockers.append(f"runtime_security_privileged:{svc}")
        cap_add = sec.get("capAdd")
        if cap_add not in (None, []):
            blockers.append(f"runtime_security_cap_add:{svc}")
        cap_drop = sec.get("capDrop") if isinstance(sec.get("capDrop"), list) else []
        if not any(str(cap).upper() == "ALL" for cap in cap_drop):
            blockers.append(f"runtime_security_cap_drop:{svc}")
        if sec.get("readOnlyRootfs") is not True:
            blockers.append(f"runtime_security_readonly:{svc}")
        security_opt = [str(x) for x in sec.get("securityOpt") or []]
        if not any("no-new-privileges:true" in item for item in security_opt):
            blockers.append(f"runtime_security_no_new_privileges:{svc}")
        if any("seccomp=unconfined" in item for item in security_opt):
            blockers.append(f"runtime_security_seccomp_unconfined:{svc}")
        if sec.get("devices") not in (None, [], {}):
            blockers.append(f"runtime_security_devices:{svc}")
        if sec.get("bindMounts") not in (None, [], {}):
            blockers.append(f"runtime_security_host_binds:{svc}")
        tmpfs = sec.get("tmpfs") if isinstance(sec.get("tmpfs"), dict) else {}
        for target in TMPFS_REQUIRED_TARGETS:
            if target not in tmpfs:
                blockers.append(f"runtime_security_tmpfs_missing:{svc}:{target}")
                continue
            options = _tmpfs_options(tmpfs.get(target))
            missing = sorted(TMPFS_REQUIRED_OPTIONS.difference(options))
            if missing:
                blockers.append(
                    f"runtime_security_tmpfs_options:{svc}:{target}:{','.join(missing)}"
                )
            if not any(option.startswith("size=") for option in options):
                blockers.append(f"runtime_security_tmpfs_size:{svc}:{target}")
        nets = [str(n) for n in sec.get("networks") or []]
        suffixes = required_networks[svc]
        if not all(
            any(_suffix_in(net, {suffix}) for net in nets) for suffix in suffixes
        ):
            blockers.append(f"runtime_security_network_missing:{svc}")
        extra = [net for net in nets if not _suffix_in(net, suffixes)]
        if extra:
            blockers.append(f"runtime_security_network_extra:{svc}:{','.join(extra)}")
        internal = (
            sec.get("networkInternal")
            if isinstance(sec.get("networkInternal"), dict)
            else {}
        )
        if svc == "worker-convert":
            for net in nets:
                if _suffix_in(net, {"convert"}) and internal.get(net) is not True:
                    blockers.append("runtime_security_convert_network_not_internal")
    return blockers


def _validate_raw_manifest(
    report: dict[str, Any],
    *,
    allow_fixture: bool,
    current_git_sha: str | None,
    raw_root: Path | None,
) -> list[str]:
    blockers: list[str] = []
    raw_dir_value = report.get("rawDir")
    if not is_safe_repo_relative_path(raw_dir_value):
        blockers.append("raw_dir_not_repo_relative")
        if allow_fixture:
            return blockers
    raw_dir = raw_root or resolve_repo_relative_dir(raw_dir_value)
    if raw_dir is None or not raw_dir.is_dir():
        blockers.append("raw_dir_missing")
        return blockers
    manifest_ref = report.get("rawArtifactManifest")
    if not isinstance(manifest_ref, dict):
        blockers.append("raw_manifest_missing")
        return blockers
    manifest_rel = manifest_ref.get("path")
    manifest_sha = manifest_ref.get("sha256")
    if not isinstance(manifest_sha, str) or not SHA256_RE.fullmatch(manifest_sha):
        blockers.append("raw_manifest_hash_invalid")
    if manifest_rel != f"{raw_dir_value}/{RAW_MANIFEST_NAME}":
        blockers.append("raw_manifest_path_mismatch")
    manifest_path = raw_dir / RAW_MANIFEST_NAME
    if not manifest_path.is_file():
        blockers.append("raw_manifest_file_missing")
        return blockers
    actual_manifest_sha = sha256_file(manifest_path)
    if actual_manifest_sha != manifest_sha:
        blockers.append("raw_manifest_hash_mismatch")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        blockers.append("raw_manifest_unreadable")
        return blockers
    if manifest.get("rawDir") != raw_dir_value:
        blockers.append("raw_manifest_raw_dir_mismatch")
    if manifest.get("targetGitShaFull") != report.get("gitShaFull"):
        blockers.append("raw_manifest_git_mismatch")
    if current_git_sha and report.get("gitShaFull") != current_git_sha:
        blockers.append("stale_target_git_sha")
    if manifest.get("composeFileSha256") != report.get("composeFileSha256"):
        blockers.append("raw_manifest_compose_hash_mismatch")
    if manifest.get("composeBlobSha256") != report.get("composeBlobSha256"):
        blockers.append("raw_manifest_compose_blob_mismatch")
    if manifest.get("containers") != report.get("containerIds"):
        blockers.append("raw_manifest_container_mismatch")
    if manifest.get("imageIds") != report.get("imageIds"):
        blockers.append("raw_manifest_image_mismatch")
    dirty = (manifest.get("dirtyWorktree") or {}).get("dirty")
    report_dirty = (
        (report.get("gitWorktree") or {}).get("dirty")
        if isinstance(report.get("gitWorktree"), dict)
        else None
    )
    if dirty is not report_dirty:
        blockers.append("raw_manifest_dirty_mismatch")
    if dirty is True and not allow_fixture:
        blockers.append("dirty_worktree")
    files = manifest.get("files") if isinstance(manifest.get("files"), dict) else {}
    if not files:
        blockers.append("raw_manifest_files_missing")
    for rel, expected_hash in files.items():
        if not is_safe_repo_relative_path(str(rel)):
            blockers.append(f"raw_manifest_bad_file_path:{rel}")
            continue
        if not isinstance(expected_hash, str) or not SHA256_RE.fullmatch(expected_hash):
            blockers.append(f"raw_manifest_bad_file_hash:{rel}")
            continue
        path = (raw_dir / str(rel).replace("\\", "/")).resolve()
        try:
            path.relative_to(raw_dir.resolve())
        except ValueError:
            blockers.append(f"raw_manifest_file_outside:{rel}")
            continue
        if not path.is_file():
            blockers.append(f"raw_manifest_file_missing:{rel}")
        elif sha256_file(path) != expected_hash:
            blockers.append(f"raw_manifest_file_hash_mismatch:{rel}")
    return blockers


def evaluate_report(
    report: dict[str, Any],
    *,
    raw_root: Path | None = None,
    allow_fixture: bool = False,
    current_git_sha: str | None = None,
) -> tuple[str, list[str]]:
    """Return (status, blockers). ``pass`` only when structural + security gates hold."""
    blockers: list[str] = []

    if report.get("issue") != ISSUE:
        blockers.append("issue_mismatch")

    for key in (
        "composeProject",
        "composeProfiles",
        "containerIds",
        "imageIds",
        "composeLabels",
        "gitShaFull",
        "composeFileSha256",
        "composeBlobSha256",
        "dockerVersion",
        "composeVersion",
        "generatedAt",
        "pass_count",
        "fail_count",
        "storageDriver",
        "egressProbe",
        "resourceLimits",
        "rawDir",
        "rawArtifactManifest",
        "gitWorktree",
        "sourceGit",
        "redactionScan",
        "runtimeSecurity",
        "bootEvidence",
        "nativeSmoke",
        "minioCredentialProbe",
        "qdrantInit",
    ):
        if key not in report or report.get(key) in (None, ""):
            blockers.append(f"missing:{key}")

    project = report.get("composeProject")
    if not isinstance(project, str) or not project.strip():
        blockers.append("missing:composeProject")
    profiles = normalize_compose_profiles(report.get("composeProfiles"))
    if not profiles:
        blockers.append("missing:composeProfiles")

    container_ids = (
        report.get("containerIds")
        if isinstance(report.get("containerIds"), dict)
        else {}
    )
    image_ids = (
        report.get("imageIds") if isinstance(report.get("imageIds"), dict) else {}
    )
    compose_labels = (
        report.get("composeLabels")
        if isinstance(report.get("composeLabels"), dict)
        else {}
    )
    for svc in expected_services_for_profiles(profiles):
        if svc not in container_ids or not container_ids.get(svc):
            blockers.append(f"missing_container:{svc}")
        elif not CONTAINER_ID_RE.fullmatch(str(container_ids.get(svc))):
            blockers.append(f"invalid_container_id:{svc}")
        if svc not in image_ids or not image_ids.get(svc):
            blockers.append(f"missing_image:{svc}")
        elif not IMAGE_ID_RE.fullmatch(str(image_ids.get(svc))):
            blockers.append(f"invalid_image_id:{svc}")
        labels = (
            compose_labels.get(svc) if isinstance(compose_labels.get(svc), dict) else {}
        )
        if labels.get("service") != svc:
            blockers.append(f"compose_service_label_mismatch:{svc}")
        if labels.get("project") != project:
            blockers.append(f"compose_project_label_mismatch:{svc}")

    if not isinstance(report.get("gitShaFull"), str) or not HEX40_RE.fullmatch(
        str(report.get("gitShaFull"))
    ):
        blockers.append("invalid_git_sha")
    if not isinstance(report.get("composeFileSha256"), str) or not SHA256_RE.fullmatch(
        str(report.get("composeFileSha256"))
    ):
        blockers.append("invalid_compose_file_hash")
    if not isinstance(report.get("composeBlobSha256"), str) or not SHA256_RE.fullmatch(
        str(report.get("composeBlobSha256"))
    ):
        blockers.append("invalid_compose_blob_hash")

    digests = (
        report.get("imageDigests")
        if isinstance(report.get("imageDigests"), dict)
        else {}
    )
    for svc, digest in digests.items():
        if not isinstance(digest, str) or not IMAGE_DIGEST_RE.fullmatch(digest):
            blockers.append(f"fake_digest:{svc}")

    egress = (
        report.get("egressProbe") if isinstance(report.get("egressProbe"), dict) else {}
    )
    if egress.get("executed") is not True:
        blockers.append("egress_not_executed")
    elif egress.get("toolMissing") is True:
        blockers.append("egress_tool_missing")
    elif egress.get("blocked") is not True:
        blockers.append("egress_not_blocked")
    probe_image = str(egress.get("probeImage") or "")
    if not IMAGE_DIGEST_RE.fullmatch(probe_image):
        blockers.append("egress_probe_image_not_pinned")
    route_probe_for_default = (
        egress.get("routeProbe") if isinstance(egress.get("routeProbe"), dict) else {}
    )
    if route_probe_for_default.get("defaultRoutePresent") is not False:
        blockers.append("egress_default_route_present_or_unknown")

    if report.get("nolimitComposeUsed") is True:
        blockers.append("nolimit_compose")

    limits = (
        report.get("resourceLimits")
        if isinstance(report.get("resourceLimits"), dict)
        else {}
    )
    limit_scope = [
        svc
        for svc in LIMIT_SERVICES
        if svc != "mock-embedding" or "mock" in set(profiles)
    ]
    for svc in OPTIONAL_LIMIT_SERVICES:
        if svc in limits:
            limit_scope.append(svc)
    for svc in limit_scope:
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

    source_git = (
        report.get("sourceGit") if isinstance(report.get("sourceGit"), dict) else {}
    )
    before_git = (
        source_git.get("before") if isinstance(source_git.get("before"), dict) else {}
    )
    after_git = (
        source_git.get("after") if isinstance(source_git.get("after"), dict) else {}
    )
    if not before_git:
        blockers.append("source_git_before_missing")
    if not after_git:
        blockers.append("source_git_after_missing")
    if source_git.get("headUnchanged") is not True:
        blockers.append("source_git_head_changed_or_unknown")
    if source_git.get("porcelainUnchanged") is not True:
        blockers.append("source_git_worktree_changed_or_unknown")
    if before_git.get("dirty") is not False:
        blockers.append("source_git_dirty_before")

    redaction = (
        report.get("redactionScan")
        if isinstance(report.get("redactionScan"), dict)
        else {}
    )
    if redaction.get("passed") is not True:
        blockers.append("redaction_failed")

    boot = (
        report.get("bootEvidence")
        if isinstance(report.get("bootEvidence"), dict)
        else {}
    )
    if boot.get("cleanBootMeasured") is not True:
        blockers.append("clean_boot_not_measured")
    elif (
        not isinstance(boot.get("durationSeconds"), (int, float))
        or boot.get("durationSeconds") <= 0
    ):
        blockers.append("clean_boot_duration_invalid")
    elif not boot.get("transcript"):
        blockers.append("clean_boot_transcript_missing")
    if boot.get("freshVolumes") is not True:
        blockers.append("clean_boot_fresh_volumes_not_proven")
    if boot.get("readinessChecked") is not True:
        blockers.append("clean_boot_readiness_not_measured")
    if boot.get("uniqueComposeProject") is not True:
        blockers.append("clean_boot_project_not_unique")

    minio_probe = (
        report.get("minioCredentialProbe")
        if isinstance(report.get("minioCredentialProbe"), dict)
        else {}
    )
    if minio_probe.get("positiveListBucket") is not True:
        blockers.append("minio_positive_probe_failed")
    if minio_probe.get("negativeAdminDenied") is not True:
        blockers.append("minio_negative_probe_failed")
    if minio_probe.get("negativeCrossBucketDenied") is not True:
        blockers.append("minio_cross_bucket_probe_failed")
    for label in ("adminDenialKind", "crossBucketDenialKind"):
        if minio_probe.get(label) != "authorization_denied":
            blockers.append(f"minio_denial_not_authorization:{label}")

    qdrant_init = (
        report.get("qdrantInit") if isinstance(report.get("qdrantInit"), dict) else {}
    )
    if qdrant_init.get("exitCode") != 0:
        blockers.append("qdrant_init_not_successful")
    if qdrant_init.get("configVerified") is not True:
        blockers.append("qdrant_config_not_verified")
    index_signature = report.get("indexSignature") or qdrant_init.get("indexSignature")
    if not isinstance(index_signature, str) or not SHA256_RE.fullmatch(index_signature):
        blockers.append("index_signature_missing_or_invalid")
    migration_hash = report.get("migrationManifestSha256")
    if not isinstance(migration_hash, str) or not SHA256_RE.fullmatch(migration_hash):
        blockers.append("migration_manifest_hash_missing_or_invalid")

    native_smoke = (
        report.get("nativeSmoke") if isinstance(report.get("nativeSmoke"), dict) else {}
    )
    if native_smoke.get("productionWorkerSandboxPath") is not True:
        blockers.append("native_smoke_not_worker_sandbox_path")
    assertions = (
        native_smoke.get("contentAssertions")
        if isinstance(native_smoke.get("contentAssertions"), dict)
        else {}
    )
    observed_formats = sorted(
        str(fmt) for fmt, passed in assertions.items() if passed is True
    )
    if observed_formats != REQUIRED_NATIVE_FORMATS:
        blockers.append("native_smoke_format_matrix_incomplete")

    egress_route = (
        egress.get("routeProbe") if isinstance(egress.get("routeProbe"), dict) else {}
    )
    if egress.get("blocked") is True:
        if egress_route.get("blocked") is not True:
            blockers.append("egress_route_not_blocked")
        if egress_route.get("classification") in {
            "dns_error",
            "http_error",
            "tls_error",
        }:
            blockers.append("egress_probe_not_route_block")

    blockers.extend(_validate_runtime_security(report))
    if not allow_fixture:
        if current_git_sha is None:
            state = git_state()
            current_git_sha = str(state.get("gitShaFull") or "")
        try:
            if sha256_file(COMPOSE_FILE) != report.get("composeFileSha256"):
                blockers.append("current_compose_hash_mismatch")
        except OSError:
            blockers.append("current_compose_missing")
    if not allow_fixture or raw_root is not None:
        blockers.extend(
            _validate_raw_manifest(
                report,
                allow_fixture=allow_fixture,
                current_git_sha=None if allow_fixture else current_git_sha,
                raw_root=raw_root,
            )
        )

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
            if path.suffix.lower() not in {
                ".json",
                ".txt",
                ".md",
                ".err",
                ".out",
                ".log",
                "",
            }:
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
    compose_profiles: list[str],
    container_ids: dict[str, str],
    image_ids: dict[str, str],
    image_digests: dict[str, str],
    compose_labels: dict[str, Any],
    git_sha: str,
    git_sha_full: str,
    docker_version: str | None,
    compose_version: str | None,
    compose_file_sha256: str,
    compose_blob_sha256: str,
    storage_driver: str,
    nolimit_compose_used: bool,
    egress_probe: dict[str, Any],
    resource_limits: dict[str, dict[str, Any]],
    raw_dir: str,
    raw_artifact_manifest: dict[str, Any],
    git_worktree: dict[str, Any],
    source_git: dict[str, Any],
    runtime_security: dict[str, Any],
    boot_evidence: dict[str, Any],
    native_smoke: dict[str, Any],
    minio_credential_probe: dict[str, Any],
    qdrant_init: dict[str, Any],
    redaction_scan: dict[str, Any],
) -> dict[str, Any]:
    limit_scope = [
        svc
        for svc in LIMIT_SERVICES
        if svc != "mock-embedding" or "mock" in set(compose_profiles)
    ]
    for svc in OPTIONAL_LIMIT_SERVICES:
        if svc in resource_limits:
            limit_scope.append(svc)
    cgroup_ok = (not nolimit_compose_used) and all(
        isinstance(resource_limits.get(svc, {}).get("memory"), (int, float))
        and resource_limits[svc]["memory"] > 0
        and _nonzero_cpu(resource_limits.get(svc, {}))
        and isinstance(resource_limits.get(svc, {}).get("pidsLimit"), (int, float))
        and resource_limits[svc]["pidsLimit"] > 0
        for svc in limit_scope
    )
    native_assertions = (
        native_smoke.get("contentAssertions")
        if isinstance(native_smoke.get("contentAssertions"), dict)
        else {}
    )
    migration_manifest_sha256 = sha256_file(
        ROOT / "crates/server/migrations/manifest.json"
    )
    index_signature = qdrant_init.get("indexSignature")
    standard = (
        fail == 0
        and cgroup_ok
        and not nolimit_compose_used
        and storage_driver.strip().lower() not in NONSTANDARD_STORAGE_DRIVERS
        and egress_probe.get("executed") is True
        and egress_probe.get("blocked") is True
        and redaction_scan.get("passed") is True
        and boot_evidence.get("cleanBootMeasured") is True
        and minio_credential_probe.get("positiveListBucket") is True
        and minio_credential_probe.get("negativeAdminDenied") is True
        and minio_credential_probe.get("negativeCrossBucketDenied") is True
        and qdrant_init.get("exitCode") == 0
        and qdrant_init.get("configVerified") is True
        and native_smoke.get("productionWorkerSandboxPath") is True
        and sorted(fmt for fmt, ok in native_assertions.items() if ok is True)
        == REQUIRED_NATIVE_FORMATS
        and git_worktree.get("dirty") is False
        and source_git.get("headUnchanged") is True
        and source_git.get("porcelainUnchanged") is True
        and all(
            svc in container_ids and svc in image_ids
            for svc in expected_services_for_profiles(compose_profiles)
        )
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
        "composeProfiles": compose_profiles,
        "containerIds": container_ids,
        "imageIds": image_ids,
        "imageDigests": image_digests,
        "composeLabels": compose_labels,
        "gitSha": git_sha,
        "gitShaFull": git_sha_full,
        "dockerVersion": docker_version,
        "composeVersion": compose_version,
        "composeFileSha256": compose_file_sha256,
        "composeBlobSha256": compose_blob_sha256,
        "migrationManifestSha256": migration_manifest_sha256,
        "indexSignature": index_signature,
        "storageDriver": storage_driver,
        "nolimitComposeUsed": nolimit_compose_used,
        "cgroupLimitsEnforced": bool(cgroup_ok),
        "standardHostQualification": bool(standard),
        "egressProbe": egress_probe,
        "resourceLimits": resource_limits,
        "rawDir": raw_dir,
        "rawArtifactManifest": raw_artifact_manifest,
        "gitWorktree": git_worktree,
        "sourceGit": source_git,
        "runtimeSecurity": runtime_security,
        "bootEvidence": boot_evidence,
        "nativeSmoke": native_smoke,
        "minioCredentialProbe": minio_credential_probe,
        "qdrantInit": qdrant_init,
        "redactionScan": redaction_scan,
        "provenance": {
            "gitShaFull": git_sha_full,
            "composeProject": compose_project,
            "imageIds": image_ids,
            "migrationManifestSha256": migration_manifest_sha256,
            "composeFileSha256": compose_file_sha256,
            "indexSignature": index_signature,
        },
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
        f"- Dirty worktree: `{(payload.get('gitWorktree') or {}).get('dirty')}`",
        f"- Compose file SHA256: `{payload.get('composeFileSha256')}`",
        f"- Compose blob SHA256: `{payload.get('composeBlobSha256')}`",
        f"- Docker: `{payload.get('dockerVersion')}` / Compose: `{payload.get('composeVersion')}`",
        f"- Storage driver: `{payload.get('storageDriver')}`",
        f"- Standard-host qualification: `{payload.get('standardHostQualification')}`",
        f"- Raw artifacts: `{payload.get('rawDir')}`",
        f"- Raw manifest: `{(payload.get('rawArtifactManifest') or {}).get('path')}`",
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
        "deploy/scripts/poc-boot-evidence.sh",
        "MARKHAND_F02_BOOT_REPORT=.artifacts/markhand_web/reports/poc-f02-boot.json \\",
        "  deploy/scripts/o04-release-suite.sh",
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
    for path, text in (
        (json_path, json.dumps(payload, indent=2) + "\n"),
        (md_path, render_markdown(payload)),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=str(path.parent),
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as tmp:
            tmp.write(text)
            tmp_path = Path(tmp.name)
        os.replace(tmp_path, path)


def write_initial_report(
    *,
    json_path: Path,
    md_path: Path,
    raw_dir: Path,
    stamp: str,
    compose_project: str,
) -> None:
    git = git_state()
    raw_rel = repo_relative_raw_dir(raw_dir, ROOT)
    try:
        compose_sha = sha256_file(COMPOSE_FILE)
    except OSError:
        compose_sha = ""
    payload = {
        "issue": ISSUE,
        "stamp_utc": stamp,
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat(),
        "passed": False,
        "pass_count": 0,
        "fail_count": 1,
        "passes": [],
        "fails": ["evidence run started but final report has not completed"],
        "notes": [],
        "composeProject": compose_project,
        "composeProfiles": normalize_compose_profiles(
            os.environ.get("COMPOSE_PROFILES", "mock")
        ),
        "containerIds": {},
        "imageIds": {},
        "imageDigests": {},
        "composeLabels": {},
        "gitSha": git.get("gitSha"),
        "gitShaFull": git.get("gitShaFull"),
        "gitWorktree": {
            "dirty": bool(git.get("dirty")),
            "porcelain": list(git.get("porcelain") or []),
        },
        "sourceGit": {
            "before": git,
            "after": {},
            "headUnchanged": False,
            "porcelainUnchanged": False,
        },
        "dockerVersion": None,
        "composeVersion": None,
        "composeFileSha256": compose_sha,
        "composeBlobSha256": compose_sha,
        "storageDriver": None,
        "nolimitComposeUsed": False,
        "cgroupLimitsEnforced": False,
        "standardHostQualification": False,
        "egressProbe": {"executed": False},
        "resourceLimits": {},
        "rawDir": raw_rel,
        "rawArtifactManifest": {},
        "runtimeSecurity": {},
        "bootEvidence": {"cleanBootMeasured": False},
        "nativeSmoke": {"productionWorkerSandboxPath": False},
        "minioCredentialProbe": {},
        "qdrantInit": {},
        "redactionScan": {"passed": False, "findings": ["run_incomplete"]},
        "evaluationBlockers": ["run_incomplete"],
    }
    write_report_files(json_path, md_path, payload)


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
    check(
        "sanitize_drops_env",
        "Env" not in blob and "supersecretvalue" not in blob,
        blob[:200],
    )

    secret_text = (
        "POSTGRES_PASSWORD=hunter2\n"
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaaa.bbbb\n"
        "postgres://user:s3cret@db:5432/app\n"
        "MINIO_ROOT_PASSWORD=minio-secret\n"
    )
    findings = scan_committed_text(secret_text)
    check("secret_scan_finds", bool(findings), str(findings))

    services = expected_services_for_profiles(["mock"])

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
            "composeProfiles": ["mock"],
            "containerIds": {
                svc: f"{i:064x}" for i, svc in enumerate(services, start=1)
            },
            "imageIds": {svc: f"sha256:{i:064d}" for i, svc in enumerate(services)},
            "imageDigests": {"postgres": "postgres@sha256:" + ("e" * 64)},
            "composeLabels": {
                svc: {"service": svc, "project": DEFAULT_COMPOSE_PROJECT}
                for svc in services
            },
            "gitSha": "a" * 7,
            "gitShaFull": "a" * 40,
            "dockerVersion": "24.0.0",
            "composeVersion": "2.24.0",
            "composeFileSha256": "f" * 64,
            "composeBlobSha256": "f" * 64,
            "migrationManifestSha256": "d" * 64,
            "indexSignature": "b" * 64,
            "storageDriver": "overlay2",
            "nolimitComposeUsed": False,
            "cgroupLimitsEnforced": True,
            "standardHostQualification": True,
            "egressProbe": {
                "executed": True,
                "toolMissing": False,
                "blocked": True,
                "exitCode": 1,
                "probeImage": DEFAULT_EGRESS_PROBE_IMAGE,
                "routeProbe": {
                    "target": "1.1.1.1:443",
                    "blocked": True,
                    "classification": "route_blocked",
                    "defaultRoutePresent": False,
                },
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
            "rawArtifactManifest": {
                "path": "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc1234/manifest.json",
                "sha256": "a" * 64,
            },
            "gitWorktree": {"dirty": False, "porcelain": []},
            "sourceGit": {
                "before": {
                    "gitSha": "a" * 7,
                    "gitShaFull": "a" * 40,
                    "dirty": False,
                    "porcelain": [],
                },
                "after": {
                    "gitSha": "a" * 7,
                    "gitShaFull": "a" * 40,
                    "dirty": False,
                    "porcelain": [],
                },
                "headUnchanged": True,
                "porcelainUnchanged": True,
            },
            "runtimeSecurity": {
                "api": {
                    "user": "10001:10001",
                    "privileged": False,
                    "capAdd": [],
                    "capDrop": ["ALL"],
                    "readOnlyRootfs": True,
                    "securityOpt": ["no-new-privileges:true"],
                    "devices": [],
                    "bindMounts": [],
                    "tmpfs": {
                        "/tmp": "rw,noexec,nosuid,nodev,size=256m",
                        "/var/lib/markhand": "rw,noexec,nosuid,nodev,size=64m",
                    },
                    "networks": ["markhand-poc_edge", "markhand-poc_private"],
                    "networkInternal": {
                        "markhand-poc_edge": False,
                        "markhand-poc_private": False,
                    },
                },
                "worker-convert": {
                    "user": "10001:10001",
                    "privileged": False,
                    "capAdd": [],
                    "capDrop": ["ALL"],
                    "readOnlyRootfs": True,
                    "securityOpt": ["no-new-privileges:true"],
                    "devices": [],
                    "bindMounts": [],
                    "tmpfs": {
                        "/tmp": "rw,noexec,nosuid,nodev,size=512m",
                        "/var/lib/markhand": "rw,noexec,nosuid,nodev,size=64m",
                    },
                    "networks": ["markhand-poc_convert"],
                    "networkInternal": {"markhand-poc_convert": True},
                },
                "worker-index": {
                    "user": "10001:10001",
                    "privileged": False,
                    "capAdd": [],
                    "capDrop": ["ALL"],
                    "readOnlyRootfs": True,
                    "securityOpt": ["no-new-privileges:true"],
                    "devices": [],
                    "bindMounts": [],
                    "tmpfs": {
                        "/tmp": "rw,noexec,nosuid,nodev,size=256m",
                        "/var/lib/markhand": "rw,noexec,nosuid,nodev,size=64m",
                    },
                    "networks": ["markhand-poc_private"],
                    "networkInternal": {"markhand-poc_private": False},
                },
                "worker-embedding": {
                    "user": "10001:10001",
                    "privileged": False,
                    "capAdd": [],
                    "capDrop": ["ALL"],
                    "readOnlyRootfs": True,
                    "securityOpt": ["no-new-privileges:true"],
                    "devices": [],
                    "bindMounts": [],
                    "tmpfs": {
                        "/tmp": "rw,noexec,nosuid,nodev,size=256m",
                        "/var/lib/markhand": "rw,noexec,nosuid,nodev,size=64m",
                    },
                    "networks": ["markhand-poc_private"],
                    "networkInternal": {"markhand-poc_private": False},
                },
                **{
                    service: {
                        "user": "10001:10001",
                        "privileged": False,
                        "capAdd": [],
                        "capDrop": ["ALL"],
                        "readOnlyRootfs": True,
                        "securityOpt": ["no-new-privileges:true"],
                        "devices": [],
                        "bindMounts": [],
                        "tmpfs": {
                            "/tmp": "rw,noexec,nosuid,nodev,size=256m",
                            "/var/lib/markhand": "rw,noexec,nosuid,nodev,size=64m",
                        },
                        "networks": ["markhand-poc_private"],
                        "networkInternal": {"markhand-poc_private": False},
                    }
                    for service in ("worker-delete", "worker-reconcile")
                },
            },
            "bootEvidence": {
                "cleanBootMeasured": True,
                "durationSeconds": 1.0,
                "transcript": "clean-boot.txt",
                "freshVolumes": True,
                "readinessChecked": True,
                "uniqueComposeProject": True,
            },
            "nativeSmoke": {
                "productionWorkerSandboxPath": True,
                "contentAssertions": {fmt: True for fmt in REQUIRED_NATIVE_FORMATS},
            },
            "minioCredentialProbe": {
                "positiveListBucket": True,
                "negativeAdminDenied": True,
                "negativeCrossBucketDenied": True,
                "adminDenialKind": "authorization_denied",
                "crossBucketDenialKind": "authorization_denied",
            },
            "qdrantInit": {
                "exitCode": 0,
                "configVerified": True,
                "indexSignature": "b" * 64,
            },
            "redactionScan": {"passed": True, "findings": []},
        }

    # 2) missing service/image metadata rejected
    missing = good()
    missing["imageIds"] = {}
    missing["containerIds"] = {}
    status, blockers = evaluate_report(missing, allow_fixture=True)
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
    status, blockers = evaluate_report(no_egress, allow_fixture=True)
    check(
        "egress_missing_rejected",
        status != "pass" and "egress_not_executed" in blockers,
        str(blockers),
    )

    # 4) resource limit zero rejected
    zero = good()
    zero["resourceLimits"]["api"]["memory"] = 0
    status, blockers = evaluate_report(zero, allow_fixture=True)
    check(
        "zero_limit_rejected",
        status != "pass"
        and any("resource_limit_zero:api:memory" == b for b in blockers),
        str(blockers),
    )

    # 5) complete fixture accepted
    status, blockers = evaluate_report(good(), allow_fixture=True)
    check("complete_accepted", status == "pass" and blockers == [], str(blockers))

    # bonus: nolimit + vfs honesty
    nested = good()
    nested["nolimitComposeUsed"] = True
    nested["storageDriver"] = "vfs"
    status, blockers = evaluate_report(nested, allow_fixture=True)
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
        proc = subprocess.run(
            args, cwd=ROOT, capture_output=True, text=True, check=False
        )
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
    compose_labels = dict(meta.get("composeLabels") or {})
    compose_profiles = normalize_compose_profiles(
        meta.get("composeProfiles") or os.environ.get("COMPOSE_PROFILES", "mock")
    )
    resource_limits = dict(meta.get("resourceLimits") or {})
    runtime_security = dict(meta.get("runtimeSecurity") or {})
    boot_evidence = dict(meta.get("bootEvidence") or {"cleanBootMeasured": False})
    native_smoke = dict(
        meta.get("nativeSmoke") or {"productionWorkerSandboxPath": False}
    )
    minio_credential_probe = dict(meta.get("minioCredentialProbe") or {})
    qdrant_init = dict(meta.get("qdrantInit") or {})
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

    git_after = git_state()
    source_git = dict(meta.get("sourceGit") or {})
    before_git = (
        source_git.get("before") if isinstance(source_git.get("before"), dict) else {}
    )
    if not before_git:
        before_git = git_after
    before_porcelain = list(before_git.get("porcelain") or [])
    after_porcelain = list(git_after.get("porcelain") or [])
    source_git = {
        "before": before_git,
        "after": git_after,
        "headUnchanged": before_git.get("gitShaFull") == git_after.get("gitShaFull"),
        "porcelainUnchanged": before_porcelain == after_porcelain,
    }
    git_sha_full = str(before_git.get("gitShaFull") or "unknown")
    git_sha = str(before_git.get("gitSha") or "unknown")

    docker_version = _cmd_text(["docker", "version", "--format", "{{.Server.Version}}"])
    compose_version = _cmd_text(["docker", "compose", "version", "--short"])
    if not compose_version:
        compose_version = _cmd_text(["docker", "compose", "version"])

    compose_sha = sha256_file(COMPOSE_FILE)
    compose_blob_sha = str(meta.get("composeBlobSha256") or compose_sha)
    raw_rel = repo_relative_raw_dir(raw_dir, ROOT)
    manifest = build_raw_manifest(
        raw_dir=raw_dir,
        raw_rel=raw_rel,
        compose_file_sha256=compose_sha,
        compose_blob_sha256=compose_blob_sha,
        container_ids=container_ids,
        image_ids=image_ids,
        git=before_git,
        meta=meta,
    )
    manifest_name, manifest_sha = write_raw_manifest(raw_dir, manifest)
    raw_artifact_manifest = {
        "path": f"{raw_rel}/{manifest_name}",
        "sha256": manifest_sha,
    }

    payload = build_report_payload(
        stamp=stamp,
        fail=fail,
        passes=passes,
        fails=fails,
        notes=notes,
        compose_project=compose_project,
        compose_profiles=compose_profiles,
        container_ids=container_ids,
        image_ids=image_ids,
        image_digests=image_digests,
        compose_labels=compose_labels,
        git_sha=git_sha,
        git_sha_full=git_sha_full,
        docker_version=docker_version,
        compose_version=compose_version,
        compose_file_sha256=compose_sha,
        compose_blob_sha256=compose_blob_sha,
        storage_driver=storage_driver,
        nolimit_compose_used=nolimit_compose_used,
        egress_probe=egress_probe,
        resource_limits=resource_limits,
        raw_dir=raw_rel,
        raw_artifact_manifest=raw_artifact_manifest,
        git_worktree={
            "dirty": bool(before_git.get("dirty")),
            "porcelain": list(before_git.get("porcelain") or []),
        },
        source_git=source_git,
        runtime_security=runtime_security,
        boot_evidence=boot_evidence,
        native_smoke=native_smoke,
        minio_credential_probe=minio_credential_probe,
        qdrant_init=qdrant_init,
        redaction_scan=redaction_scan,
    )
    # Keep shell FAIL bits visible even when evaluation adds blockers.
    if fail != 0:
        payload["passed"] = False
        payload["standardHostQualification"] = False
        if "shell_fail" not in (payload.get("evaluationBlockers") or []):
            payload.setdefault("evaluationBlockers", []).append("shell_fail")
    write_report_files(json_path, md_path, payload)
    return payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test", action="store_true", help="run hermetic validator"
    )
    parser.add_argument(
        "--validate-report",
        type=Path,
        help="validate a generated poc-f02-boot.json and print {status,blockers}",
    )
    parser.add_argument(
        "--init-report", action="store_true", help="write initial non-pass report"
    )
    parser.add_argument(
        "--finalize", action="store_true", help="build report from raw dir"
    )
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

    if args.validate_report is not None:
        report = json.loads(args.validate_report.read_text(encoding="utf-8"))
        status, blockers = evaluate_report(report)
        print(
            json.dumps(
                {"status": status, "blockers": blockers}, indent=2, sort_keys=True
            )
        )
        return 0 if status == "pass" else 1

    if args.init_report:
        if not args.json or not args.md or not args.raw_dir:
            parser.error("--init-report requires --json --md --raw-dir")
        write_initial_report(
            json_path=args.json,
            md_path=args.md,
            raw_dir=args.raw_dir,
            stamp=args.stamp
            or dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ"),
            compose_project=args.compose_project,
        )
        print(f"initialized non-pass report {args.json}")
        print(f"initialized non-pass report {args.md}")
        return 0

    if args.finalize:
        if not args.json or not args.md or not args.raw_dir:
            parser.error("--finalize requires --json --md --raw-dir")
        payload = finalize_from_raw(
            json_path=args.json,
            md_path=args.md,
            raw_dir=args.raw_dir,
            stamp=args.stamp
            or dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ"),
            fail=args.fail,
            compose_project=args.compose_project,
            nolimit_compose_used=bool(args.nolimit_compose),
        )
        print(f"wrote {args.json}")
        print(f"wrote {args.md}")
        return 0 if payload.get("passed") else 1

    parser.error("specify --self-test, --validate-report, --init-report, or --finalize")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
