#!/usr/bin/env python3
"""P1B-O04 vertical-slice / security release suite harness.

Machine-verifiable, redacted evidence under:
  bench/markhand_web/reports/phase-1b-gate/o04-release.{json,md}
  bench/markhand_web/reports/phase-1b-gate/raw/o04-<git>/

Architecture (honest): live pass requires black-box probes against the
deployed Compose API over public HTTP plus cargo integration suites for the
vertical/security checks. Without public HTTP exercise the report is non-pass.

Never writes or overwrites O05 ``summary.json``.
Default status is honest ``not_run``. ``pass`` only when MARKHAND_E2E=1,
every required suite exits 0 with testsRun>0, format matrix matches
``phase1b-mixed.yaml``, F02 boot evidence passed with matching POC images,
provenance/raw/redaction gates hold, and no high/critical findings.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any
from urllib.parse import urljoin, urlsplit, urlunsplit

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "bench/markhand_web/reports/phase-1b-gate"
RAW_ROOT = OUT / "raw"
O05_SUMMARY = OUT / "summary.json"
F02_BOOT = Path(
    os.environ.get(
        "MARKHAND_F02_BOOT_REPORT",
        str(ROOT / ".artifacts/markhand_web/reports/poc-f02-boot.json"),
    )
)
F02_VALIDATOR = ROOT / "deploy/scripts/poc_f02_boot_evidence.py"
COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
WORKLOAD_YAML = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
ISSUE = "P1B-O04"
SCHEMA_VERSION = 2
DEFAULT_COMPOSE_PROJECT = "markhand-poc"
RAW_MANIFEST = "raw-manifest.json"
DEFAULT_COMMAND_TIMEOUT_SECS = 15 * 60
DEFAULT_MAX_CAPTURE_BYTES = 8 * 1024 * 1024

# POC services that must appear under the Compose project label for live pass.
EXPECTED_POC_SERVICES = [
    "api",
    "minio",
    "postgres",
    "qdrant",
    "worker-convert",
    "worker-index",
]

REQUIRED_SUITES = [
    "vertical_slice_formats",
    "unauthorized_cross_tenant",
    "suspend_membership_delete_deny",
    "adversarial_upload",
    "worker_kill_replay",
]

REQUIRED_TOP_LEVEL = [
    "schemaVersion",
    "issue",
    "status",
    "markhandE2e",
    "expectedFormats",
    "formatsObserved",
    "suites",
    "findings",
    "provenance",
    "redactionScan",
    "rawDir",
    "blockers",
    "architecture",
    "f02Boot",
    "blackBoxApiProbes",
    "externalWorkerKill",
]

REQUIRED_PROVENANCE = [
    "gitSha",
    "gitShaFull",
    "dockerVersion",
    "composeVersion",
    "composeProject",
    "migrationManifestSha256",
    "composeFileSha256",
    "effectiveComposeSha256",
    "f02ReportSha256",
    "f02ManifestSha256",
    "indexSignature",
    "containerIds",
    "imageIds",
    "composeServiceMap",
    "testEndpoints",
]

INDEX_SIG_RE = re.compile(r"^[0-9a-f]{64}$")
UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)
FORMAT_COVERAGE_RE = re.compile(r"^O04_FORMAT_COVERAGE\t(.+)$", re.MULTILINE)
TEST_LINE_RE = re.compile(
    r"^test (?P<name>\S+) \.\.\. (?P<status>ok|FAILED|ignored)$", re.MULTILINE
)
TEST_OK_RE = re.compile(r"^test (?P<name>\S+) \.\.\. ok$", re.MULTILINE)
TEST_FAILED_RE = re.compile(r"^test (?P<name>\S+) \.\.\. FAILED$", re.MULTILINE)
TEST_IGNORED_RE = re.compile(r"^test (?P<name>\S+) \.\.\. ignored$", re.MULTILINE)
SKIPPED_RE = re.compile(r"(?im)^skipped:")
COMMAND_EXIT_RE = re.compile(r"^O04_COMMAND_EXIT_CODE\t(-?\d+)$", re.MULTILINE)
COMMAND_TIMEOUT_RE = re.compile(r"^O04_COMMAND_TIMED_OUT\t(true|false)$", re.MULTILINE)
COMMAND_EOF_RE = re.compile(r"^O04_COMMAND_EOF\ttrue$", re.MULTILINE)
COMMAND_TRUNCATED_RE = re.compile(
    r"^O04_COMMAND_OUTPUT_TRUNCATED\t(true|false)$", re.MULTILINE
)
SUMMARY_RE = re.compile(
    r"(?P<passed>\d+) passed;\s*(?P<failed>\d+) failed;"
    r"(?:\s*(?P<ignored>\d+) ignored;)?"
    r"(?:\s*(?P<measured>\d+) measured;)?"
    r"(?:\s*(?P<filtered>\d+) filtered out)?"
)
WORKLOAD_FORMATS_RE = re.compile(
    r"formats:\s*\[([^\]]+)\]",
    re.MULTILINE,
)

REDACT_PATTERNS = [
    (re.compile(r"(?i)(Bearer\s+)[A-Za-z0-9._\-+=/]+"), r"\1[REDACTED]"),
    (re.compile(r"(?i)(Basic\s+)[A-Za-z0-9._\-+=/]+"), r"\1[REDACTED]"),
    (re.compile(r"(?i)\b(Set-Cookie|Cookie)\s*:\s*[^\r\n]+"), r"\1: [REDACTED]"),
    (re.compile(r"([a-z][a-z0-9+.-]*://)[^/@\s:]+:[^/@\s]+@"), r"\1[REDACTED]@"),
    (
        re.compile(
            r"(?i)\b([A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|SECRET_KEY|TOKEN|"
            r"AUTHORIZATION|ACCESS_KEY|API[_-]?KEY|COOKIE|SESSION|CAPABILITY|"
            r"PRIVATE[_-]?KEY|AWS[_-]?SECRET|AWS[_-]?ACCESS|AZURE[_-]?|GCP[_-]?|"
            r"GOOGLE[_-]?|CLOUD[_-]?)[A-Z0-9_]*)\b"
            r"\"?\s*[:=]\s*\"?[^\s\",}]+"
        ),
        r"\1:[REDACTED]",
    ),
    (
        re.compile(
            r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"
        ),
        "[REDACTED_JWT]",
    ),
]

SECRETISH_RE = re.compile(
    r"(?i)("
    r"\b[A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|SECRET_KEY|TOKEN|AUTHORIZATION|"
    r"ACCESS_KEY|API[_-]?KEY|COOKIE|SESSION|CAPABILITY|PRIVATE[_-]?KEY|"
    r"AWS[_-]?SECRET|AWS[_-]?ACCESS|AZURE[_-]?|GCP[_-]?|GOOGLE[_-]?|CLOUD[_-]?)[A-Z0-9_]*"
    r"\b\s*[:=]\s*[^\s\",}]+|"
    r"\b(Set-Cookie|Cookie)\s*:\s*[^\r\n]+|"
    r"Bearer\s+[A-Za-z0-9._\-+=/]{12,}|"
    r"Basic\s+[A-Za-z0-9._\-+=/]{12,}|"
    r"[a-z][a-z0-9+.-]*://[^/@\s:]+:[^/@\s]+@|"
    r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}"
    r")"
)

EXPECTED_SUITE_TESTS = {
    "vertical_slice_formats": ["live_upload_convert_index_citation_vertical_slice"],
    "unauthorized_cross_tenant": [
        "live_http_unauthenticated_and_cross_tenant_are_consistent"
    ],
    "suspend_membership_delete_deny": [
        "live_citation_authz_expiry_replay_idor_and_immediate_deny"
    ],
    "adversarial_upload": [
        "corrupt_and_page_bomb_pdf_reject",
        "declared_entry_count_rejects_before_name_allocation",
        "entry_count_bomb_rejects",
        "forged_central_directory_size_rejects_during_inflation",
        "hidden_nested_polyglot_duplicate_and_symlink_reject",
        "malformed_and_traversal_docx_reject",
        "mime_mismatch_and_malformed_audio_reject",
        "oversize_stream_rejects_early",
        "rejected_upload_is_not_stored",
        "spoof_pdf_and_html_pdf_reject",
        "unparseable_compressed_span_rejects_closed",
        "zip_bomb_rejects_without_unbounded_decompress",
    ],
    "worker_kill_replay": ["live_convert_worker_cancel_loses_lease_and_kills_sandbox"],
}


def configure_output_dir(path: Path | None) -> None:
    global OUT, RAW_ROOT, O05_SUMMARY
    if path is None:
        return
    OUT = path.resolve()
    RAW_ROOT = OUT / "raw"
    O05_SUMMARY = OUT / "summary.json"


def load_expected_formats(path: Path = WORKLOAD_YAML) -> list[str]:
    """Single source of truth: ingest formats from phase1b-mixed.yaml."""
    text = path.read_text(encoding="utf-8")
    match = WORKLOAD_FORMATS_RE.search(text)
    if not match:
        raise RuntimeError(f"formats list missing in {path}")
    formats = sorted(
        {part.strip().lower() for part in match.group(1).split(",") if part.strip()}
    )
    if not formats:
        raise RuntimeError(f"empty formats list in {path}")
    return formats


EXPECTED_FORMATS = load_expected_formats()


def redact(text: str) -> str:
    out = text
    for pattern, repl in REDACT_PATTERNS:
        out = pattern.sub(repl, out)
    return out


def repo_rel(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return resolved.relative_to(OUT.resolve()).as_posix()


def path_inside(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def normalize_raw_dir(
    value: str | Path, *, must_exist: bool
) -> tuple[Path | None, str | None]:
    raw = Path(value)
    if raw.is_absolute():
        return None, "raw_dir_not_repo_relative"
    if any(part == ".." for part in raw.parts):
        return None, "raw_dir_not_repo_relative"
    root_candidate = (ROOT / raw).resolve()
    out_candidate = (OUT / raw).resolve()
    path = root_candidate if path_inside(root_candidate, RAW_ROOT) else out_candidate
    if not path_inside(path, RAW_ROOT):
        return None, "raw_dir_outside_evidence_root"
    if must_exist and not path.is_dir():
        return None, "raw_dir_missing"
    return path, None


def sanitize_url(value: str | None) -> str | None:
    if not value:
        return None
    try:
        parsed = urlsplit(value)
    except ValueError:
        return redact(value)
    if not parsed.scheme or not parsed.netloc:
        return redact(value)
    host = parsed.hostname or ""
    port = f":{parsed.port}" if parsed.port else ""
    netloc = f"{host}{port}"
    return urlunsplit((parsed.scheme, netloc, parsed.path, "", ""))


def live_test_endpoints() -> dict[str, str | None]:
    return {
        "database": sanitize_url(os.environ.get("MARKHAND_TEST_DATABASE_URL")),
        "appDatabase": sanitize_url(os.environ.get("MARKHAND_TEST_APP_DATABASE_URL")),
        "minio": sanitize_url(
            os.environ.get("MARKHAND_TEST_MINIO_ENDPOINT")
            or os.environ.get("MARKHAND_TEST_OBJECT_STORE_ENDPOINT")
        ),
        "qdrant": sanitize_url(os.environ.get("MARKHAND_TEST_QDRANT_URL")),
    }


def live_api_endpoint() -> str | None:
    configured = (
        os.environ.get("MARKHAND_TEST_API_ENDPOINT")
        or os.environ.get("MARKHAND_API_URL")
        or os.environ.get("MARKHAND_O04_API_URL")
    )
    if configured:
        return sanitize_url(configured)
    return sanitize_url(
        f"http://127.0.0.1:{os.environ.get('MARKHAND_API_PORT', '8788')}"
    )


def command_timeout_secs() -> int:
    raw = os.environ.get("MARKHAND_O04_COMMAND_TIMEOUT_SECS", "")
    try:
        value = int(raw) if raw else DEFAULT_COMMAND_TIMEOUT_SECS
    except ValueError:
        return DEFAULT_COMMAND_TIMEOUT_SECS
    return max(1, value)


def max_capture_bytes() -> int:
    raw = os.environ.get("MARKHAND_O04_MAX_CAPTURE_BYTES", "")
    try:
        value = int(raw) if raw else DEFAULT_MAX_CAPTURE_BYTES
    except ValueError:
        return DEFAULT_MAX_CAPTURE_BYTES
    return max(4096, value)


def current_git_state() -> tuple[str, str, bool]:
    full = git_output("rev-parse", "HEAD")
    short = git_output("rev-parse", "--short", "HEAD")
    dirty = bool(git_output("status", "--porcelain"))
    return short, full, dirty


def git_output(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


class BoundedCommandResult:
    def __init__(self, returncode: int, output: str, timed_out: bool, truncated: bool):
        self.returncode = returncode
        self.output = output
        self.timed_out = timed_out
        self.truncated = truncated


def run_bounded_command(
    args: list[str],
    env: dict[str, str],
    *,
    timeout_secs: int,
    max_bytes: int,
) -> BoundedCommandResult:
    proc = subprocess.Popen(
        args,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    captured = bytearray()
    truncated = False

    def drain() -> None:
        nonlocal truncated
        assert proc.stdout is not None
        while True:
            chunk = proc.stdout.read(64 * 1024)
            if not chunk:
                break
            remaining = max_bytes - len(captured)
            if remaining > 0:
                captured.extend(chunk[:remaining])
            if len(chunk) > remaining:
                truncated = True

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    timed_out = False
    try:
        returncode = proc.wait(timeout=timeout_secs)
    except subprocess.TimeoutExpired:
        timed_out = True
        proc.kill()
        returncode = proc.wait()
    reader.join(timeout=5)
    text = captured.decode("utf-8", errors="replace")
    if truncated:
        text += "\nO04_COMMAND_OUTPUT_TRUNCATED\ttrue\n"
    else:
        text += "\nO04_COMMAND_OUTPUT_TRUNCATED\tfalse\n"
    return BoundedCommandResult(returncode, text, timed_out, truncated)


def cmd_text(args: list[str]) -> str | None:
    try:
        proc = run_bounded_command(
            args, os.environ.copy(), timeout_secs=30, max_bytes=64 * 1024
        )
    except FileNotFoundError:
        return None
    if proc.returncode != 0 and not proc.output.strip():
        return None
    return (proc.output or "").strip() or None


def migration_manifest_sha256() -> str:
    path = ROOT / "crates/server/migrations/manifest.json"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def effective_compose_sha256() -> str | None:
    args = ["docker", "compose", "-f", str(COMPOSE_FILE), "config"]
    try:
        proc = run_bounded_command(
            args, os.environ.copy(), timeout_secs=60, max_bytes=4 * 1024 * 1024
        )
    except FileNotFoundError:
        return None
    if proc.returncode != 0 or proc.truncated:
        return None
    return hashlib.sha256(proc.output.encode("utf-8")).hexdigest()


def compose_project() -> str:
    return os.environ.get(
        "MARKHAND_COMPOSE_PROJECT", DEFAULT_COMPOSE_PROJECT
    ).strip() or (DEFAULT_COMPOSE_PROJECT)


def filters_after_double_dash(cmd: list[str]) -> list[str]:
    if "--" not in cmd:
        return []
    rest = cmd[cmd.index("--") + 1 :]
    return [arg for arg in rest if not arg.startswith("-")]


def validate_cargo_command_shape(cmd: list[str]) -> None:
    """libtest accepts a single FILTER; multiple positional filters are invalid."""
    if not cmd or cmd[0] != "cargo":
        raise ValueError(f"command must start with cargo: {cmd!r}")
    filters = filters_after_double_dash(cmd)
    if len(filters) > 1:
        raise ValueError(
            "libtest accepts only one FILTER after '--'; got "
            f"{len(filters)}: {filters!r} in {cmd!r}"
        )


def validate_canonical_suite_command(suite_key: str, commands: list[list[str]]) -> None:
    expected = suite_specs().get(suite_key)
    if commands != expected:
        raise ValueError(f"non-canonical suite command for {suite_key}")


def resolve_index_signature() -> str | None:
    """Machine-verifiable 64-lowercase-hex signature. Never logs secret env values."""
    env_sig = os.environ.get("MARKHAND_INDEX_SIGNATURE", "").strip()
    if INDEX_SIG_RE.fullmatch(env_sig):
        return env_sig
    # Fallback: inspect POC API container Config.Env for MARKHAND_INDEX_SIGNATURE only.
    project = compose_project()
    try:
        proc = run_bounded_command(
            [
                "docker",
                "ps",
                "-a",
                "--filter",
                f"label=com.docker.compose.project={project}",
                "--filter",
                "label=com.docker.compose.service=api",
                "--format",
                "{{.ID}}",
            ],
            os.environ.copy(),
            timeout_secs=30,
            max_bytes=64 * 1024,
        )
    except FileNotFoundError:
        return None
    cid = (proc.output or "").strip().splitlines()
    if not cid:
        return None
    insp = run_bounded_command(
        [
            "docker",
            "inspect",
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
            cid[0],
        ],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=64 * 1024,
    )
    if insp.returncode != 0:
        return None
    for line in (insp.output or "").splitlines():
        if line.startswith("MARKHAND_INDEX_SIGNATURE="):
            value = line.split("=", 1)[1].strip()
            if INDEX_SIG_RE.fullmatch(value):
                return value
            return None
    return None


def collect_poc_image_metadata(
    project: str,
) -> tuple[dict[str, str], dict[str, str], dict[str, str], list[str]]:
    """Return (imageIds, imageDigests, containerIds, missingServices) for Compose services.

    Digests are recorded only when RepoDigests is non-empty. Locally built images
    keep immutable image IDs in imageIds without fabricating digest strings.
    """
    ids: dict[str, str] = {}
    digests: dict[str, str] = {}
    containers: dict[str, str] = {}
    if not shutil.which("docker"):
        return ids, digests, containers, list(EXPECTED_POC_SERVICES)
    proc = run_bounded_command(
        [
            "docker",
            "ps",
            "-a",
            "--filter",
            f"label=com.docker.compose.project={project}",
            "--format",
            '{{.ID}}\t{{.Label "com.docker.compose.service"}}',
        ],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=512 * 1024,
    )
    if proc.returncode != 0 or not proc.output.strip():
        return ids, digests, containers, list(EXPECTED_POC_SERVICES)
    for line in proc.output.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        cid, service = parts[0].strip(), parts[1].strip()
        if not service:
            continue
        containers[service] = cid
        insp = run_bounded_command(
            [
                "docker",
                "inspect",
                "--format",
                "{{.Image}}",
                cid,
            ],
            os.environ.copy(),
            timeout_secs=30,
            max_bytes=64 * 1024,
        )
        if insp.returncode != 0:
            continue
        image_id = (insp.output or "").strip()
        if image_id:
            ids[service] = image_id
            # RepoDigests live on the image object, not the container.
            img = run_bounded_command(
                [
                    "docker",
                    "image",
                    "inspect",
                    "--format",
                    "{{json .RepoDigests}}",
                    image_id,
                ],
                os.environ.copy(),
                timeout_secs=30,
                max_bytes=64 * 1024,
            )
            repo_json = (img.output or "").strip() if img.returncode == 0 else "[]"
            try:
                repo = json.loads(repo_json) if repo_json else []
            except json.JSONDecodeError:
                repo = []
            if isinstance(repo, list):
                real = [d for d in repo if isinstance(d, str) and "@sha256:" in d]
                if real:
                    digests[service] = real[0]
    missing = [
        svc for svc in EXPECTED_POC_SERVICES if svc not in ids or svc not in containers
    ]
    return ids, digests, containers, missing


def collect_compose_service_map(project: str) -> dict[str, Any]:
    service_map: dict[str, Any] = {}
    if not shutil.which("docker"):
        return service_map
    proc = run_bounded_command(
        [
            "docker",
            "ps",
            "-a",
            "--filter",
            f"label=com.docker.compose.project={project}",
            "--format",
            '{{.ID}}\t{{.Label "com.docker.compose.service"}}',
        ],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=512 * 1024,
    )
    if proc.returncode != 0:
        return service_map
    for line in proc.output.splitlines():
        parts = line.split("\t")
        if len(parts) != 2:
            continue
        cid, service = parts[0].strip(), parts[1].strip()
        if not cid or not service:
            continue
        inspect = run_bounded_command(
            ["docker", "inspect", cid],
            os.environ.copy(),
            timeout_secs=30,
            max_bytes=1024 * 1024,
        )
        if inspect.returncode != 0:
            continue
        try:
            payload = json.loads(inspect.output)[0]
        except (json.JSONDecodeError, IndexError, KeyError, TypeError):
            continue
        state = payload.get("State") if isinstance(payload.get("State"), dict) else {}
        config = (
            payload.get("Config") if isinstance(payload.get("Config"), dict) else {}
        )
        network = (
            payload.get("NetworkSettings")
            if isinstance(payload.get("NetworkSettings"), dict)
            else {}
        )
        labels = config.get("Labels") if isinstance(config.get("Labels"), dict) else {}
        service_map[service] = {
            "containerId": cid,
            "imageId": payload.get("Image"),
            "health": state.get("Health", {}).get("Status")
            if isinstance(state.get("Health"), dict)
            else state.get("Status"),
            "running": state.get("Running"),
            "ports": network.get("Ports") or {},
            "labels": {
                key: labels.get(key)
                for key in [
                    "com.docker.compose.project",
                    "com.docker.compose.service",
                    "com.docker.compose.config-hash",
                    "com.docker.compose.container-number",
                ]
                if key in labels
            },
        }
    return service_map


def parse_cargo_result(log: str) -> dict[str, Any]:
    ok_names = [m.group("name") for m in TEST_OK_RE.finditer(log)]
    failed_names = [m.group("name") for m in TEST_FAILED_RE.finditer(log)]
    ignored_names = [m.group("name") for m in TEST_IGNORED_RE.finditer(log)]
    ok = len(ok_names)
    failed = len(failed_names)
    ignored_lines = len(ignored_names)
    skipped = bool(SKIPPED_RE.search(log))
    formats: list[str] = []
    match = FORMAT_COVERAGE_RE.search(log)
    if match:
        try:
            parsed = json.loads(match.group(1))
            if isinstance(parsed, list):
                formats = [str(x) for x in parsed]
        except json.JSONDecodeError:
            formats = []
    summary = SUMMARY_RE.search(log)
    if summary:
        passed = int(summary.group("passed"))
        failed_n = int(summary.group("failed"))
        ignored_n = int(summary.group("ignored") or "0")
    else:
        passed = ok
        failed_n = failed
        ignored_n = ignored_lines
    return {
        "testsRun": passed + failed_n,
        "testsPassed": passed,
        "testsFailed": failed_n,
        "ignoredCount": ignored_n,
        "skipped": skipped,
        "formatsObserved": sorted(set(formats)),
        "hasIgnoredLine": ignored_lines > 0,
        "testResults": {
            "ok": sorted(ok_names),
            "failed": sorted(failed_names),
            "ignored": sorted(ignored_names),
        },
        "exitCode": int(COMMAND_EXIT_RE.search(log).group(1))
        if COMMAND_EXIT_RE.search(log)
        else None,
        "timedOut": (COMMAND_TIMEOUT_RE.search(log).group(1) == "true")
        if COMMAND_TIMEOUT_RE.search(log)
        else None,
        "eof": bool(COMMAND_EOF_RE.search(log)),
        "truncated": (COMMAND_TRUNCATED_RE.search(log).group(1) == "true")
        if COMMAND_TRUNCATED_RE.search(log)
        else None,
    }


def artifact_info(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    return {"sha256": hashlib.sha256(data).hexdigest(), "sizeBytes": len(data)}


def write_raw_manifest(raw_dir: Path, artifact_paths: list[Path]) -> Path:
    artifacts: dict[str, Any] = {}
    for path in sorted(artifact_paths):
        rel = path.resolve().relative_to(raw_dir.resolve()).as_posix()
        artifacts[rel] = artifact_info(path)
    manifest = {
        "schema": 1,
        "rawDir": repo_rel(raw_dir),
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "artifacts": artifacts,
    }
    path = raw_dir / RAW_MANIFEST
    path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return path


def load_raw_manifest(raw_dir: Path) -> tuple[dict[str, Any] | None, list[str]]:
    path = raw_dir / RAW_MANIFEST
    if not path.is_file():
        return None, ["raw_manifest_missing"]
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return None, [f"raw_manifest_invalid:{exc}"]
    if data.get("rawDir") != repo_rel(raw_dir):
        return data, ["raw_manifest_raw_dir_mismatch"]
    artifacts = data.get("artifacts")
    if not isinstance(artifacts, dict) or not artifacts:
        return data, ["raw_manifest_empty"]
    blockers: list[str] = []
    for rel, claimed in artifacts.items():
        if (
            not isinstance(rel, str)
            or Path(rel).is_absolute()
            or ".." in Path(rel).parts
        ):
            blockers.append("raw_manifest_path_invalid")
            continue
        path = (raw_dir / rel).resolve()
        if not path_inside(path, raw_dir):
            blockers.append("raw_manifest_path_invalid")
            continue
        if not path.is_file():
            blockers.append(f"raw_manifest_artifact_missing:{rel}")
            continue
        actual = artifact_info(path)
        if not isinstance(claimed, dict) or claimed.get("sha256") != actual["sha256"]:
            blockers.append(f"raw_manifest_sha_mismatch:{rel}")
        if (
            not isinstance(claimed, dict)
            or claimed.get("sizeBytes") != actual["sizeBytes"]
        ):
            blockers.append(f"raw_manifest_size_mismatch:{rel}")
    return data, blockers


def validate_raw_log_rel(raw_dir: Path, rel: str) -> tuple[Path | None, str | None]:
    path_rel = Path(rel)
    if path_rel.is_absolute() or ".." in path_rel.parts:
        return None, "raw_log_path_invalid"
    path = (raw_dir / path_rel).resolve()
    if not path_inside(path, raw_dir):
        return None, "raw_log_path_invalid"
    if not path.is_file():
        return None, f"raw_log_missing:{rel}"
    return path, None


def recompute_raw_suites(
    report: dict[str, Any], raw_dir: Path, manifest: dict[str, Any]
) -> tuple[dict[str, Any], list[str], list[str]]:
    blockers: list[str] = []
    recomputed: dict[str, Any] = {}
    observed: set[str] = set()
    manifest_artifacts = manifest.get("artifacts") if isinstance(manifest, dict) else {}
    if not isinstance(manifest_artifacts, dict):
        manifest_artifacts = {}
    suites = report.get("suites") if isinstance(report.get("suites"), dict) else {}
    for suite_key in REQUIRED_SUITES:
        suite = suites.get(suite_key) if isinstance(suites, dict) else None
        if not isinstance(suite, dict):
            blockers.append(f"missing_suite:{suite_key}")
            continue
        commands = suite.get("commands") or (
            [suite.get("command")] if suite.get("command") else []
        )
        if not isinstance(commands, list) or not commands:
            blockers.append(f"missing_commands:{suite_key}")
            continue
        normalized_commands: list[list[str]] = []
        for cmd in commands:
            if not isinstance(cmd, list):
                blockers.append(f"command_shape:{suite_key}")
                continue
            normalized = [str(x) for x in cmd]
            try:
                validate_cargo_command_shape(normalized)
            except ValueError:
                blockers.append(f"command_shape:{suite_key}")
            normalized_commands.append(normalized)
        try:
            validate_canonical_suite_command(suite_key, normalized_commands)
        except ValueError:
            blockers.append(f"noncanonical_command:{suite_key}")
        raw_logs = suite.get("rawLogs")
        if not isinstance(raw_logs, list) or not raw_logs:
            blockers.append(f"raw_logs_missing:{suite_key}")
            continue
        if len(raw_logs) != 1:
            blockers.append(f"raw_logs_not_unique:{suite_key}")
        if len(raw_logs) != len(normalized_commands):
            blockers.append(f"raw_logs_count:{suite_key}")
        if len(set(str(item) for item in raw_logs)) != len(raw_logs):
            blockers.append(f"raw_logs_duplicate:{suite_key}")
        logs: list[str] = []
        exit_codes: list[int] = []
        for rel_raw in raw_logs:
            if not isinstance(rel_raw, str):
                blockers.append(f"raw_log_path_invalid:{suite_key}")
                continue
            path, error = validate_raw_log_rel(raw_dir, rel_raw)
            if error:
                blockers.append(f"{error}:{suite_key}")
                continue
            if rel_raw not in manifest_artifacts:
                blockers.append(f"raw_manifest_unlisted:{rel_raw}")
            assert path is not None
            log = path.read_text(encoding="utf-8", errors="replace")
            parsed = parse_cargo_result(log)
            if parsed["exitCode"] is None:
                blockers.append(f"raw_exit_missing:{suite_key}")
                exit_codes.append(1)
            else:
                exit_codes.append(int(parsed["exitCode"]))
            if parsed["eof"] is not True:
                blockers.append(f"raw_eof_missing:{suite_key}")
            if parsed["timedOut"] is True:
                blockers.append(f"timeout:{suite_key}")
            if parsed["truncated"] is True:
                blockers.append(f"truncated:{suite_key}")
            if parsed["truncated"] is None:
                blockers.append(f"truncation_marker_missing:{suite_key}")
            expected_tests = EXPECTED_SUITE_TESTS.get(suite_key, [])
            actual_tests = sorted(
                parsed["testResults"]["ok"]
                + parsed["testResults"]["failed"]
                + parsed["testResults"]["ignored"]
            )
            if actual_tests != expected_tests:
                blockers.append(f"test_names_mismatch:{suite_key}")
            if parsed["testResults"]["failed"]:
                blockers.append(f"test_failed_names:{suite_key}")
            if parsed["testResults"]["ignored"]:
                blockers.append(f"test_ignored_names:{suite_key}")
            if suite_key != "vertical_slice_formats" and parsed["formatsObserved"]:
                blockers.append(f"format_coverage_from_non_vertical:{suite_key}")
            logs.append(log)
        suite_result = aggregate_suite_runs(normalized_commands, logs, exit_codes)
        suite_result["rawLogs"] = list(raw_logs)
        suite_result["rawLog"] = suite.get("rawLog")
        recomputed[suite_key] = suite_result
        if suite_key == "vertical_slice_formats":
            observed.update(suite_result["formatsObserved"])
    return recomputed, sorted(observed), blockers


def load_f02_boot() -> dict[str, Any]:
    if not F02_BOOT.is_file():
        return {"path": str(F02_BOOT), "passed": False, "error": "missing"}
    try:
        data = json.loads(F02_BOOT.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return {"path": str(F02_BOOT), "passed": False, "error": f"invalid_json:{exc}"}
    validation_returncode = 1
    validation_payload: dict[str, Any] = {
        "status": "fail",
        "blockers": ["f02_validator_unavailable"],
    }
    try:
        spec = importlib.util.spec_from_file_location(
            "poc_f02_boot_evidence", F02_VALIDATOR
        )
        if spec and spec.loader:
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            status, blockers = module.evaluate_report(
                data, raw_root=None, allow_fixture=False
            )
            validation_payload = {"status": status, "blockers": blockers}
            validation_returncode = 0 if status == "pass" else 1
    except Exception as exc:  # noqa: BLE001 - report validator availability as evidence.
        validation_payload = {
            "status": "fail",
            "blockers": [f"f02_validator_error:{exc}"],
        }
    manifest = (
        data.get("rawArtifactManifest")
        if isinstance(data.get("rawArtifactManifest"), dict)
        else {}
    )
    manifest_path = manifest.get("path")
    manifest_sha = manifest.get("sha256")
    return {
        "path": str(F02_BOOT),
        "reportSha256": sha256_file(F02_BOOT),
        "manifestSha256": manifest_sha,
        "manifestPath": manifest_path,
        "passed": bool(data.get("passed") is True and validation_returncode == 0),
        "validatorStatus": validation_payload.get("status") or "unknown",
        "validatorBlockers": validation_payload.get("blockers") or [],
        "issue": data.get("issue"),
        "composeProject": data.get("composeProject") or data.get("compose_project"),
        "containerIds": data.get("containerIds") or data.get("container_ids") or {},
        "imageIds": data.get("imageIds") or data.get("image_ids") or {},
        "composeFileSha256": data.get("composeFileSha256"),
        "effectiveComposeSha256": data.get("composeBlobSha256")
        or data.get("effectiveComposeSha256"),
        "stamp_utc": data.get("stamp_utc"),
    }


def architecture_block() -> dict[str, Any]:
    return {
        "kind": "compose_api_public_http",
        "apiHttpExercised": True,
        "description": (
            "O04 pass requires black-box probes against the deployed Compose API "
            "over its host-published HTTP endpoint plus cargo integration suites."
        ),
    }


def evaluate_report(
    report: dict[str, Any],
    *,
    raw_must_exist: bool = True,
    report_path: Path | None = None,
    bind_current_git: bool = False,
    current_git_full: str | None = None,
    current_git_dirty: bool | None = None,
) -> tuple[str, list[str]]:
    """Return (status, blockers). Only ``pass`` when every acceptance gate holds."""
    blockers: list[str] = []
    expected_formats = load_expected_formats()

    for key in REQUIRED_TOP_LEVEL:
        if key not in report:
            blockers.append(f"missing:{key}")

    if report.get("issue") != ISSUE:
        blockers.append("issue_mismatch")

    if (
        not isinstance(report.get("schemaVersion"), int)
        or report.get("schemaVersion") != SCHEMA_VERSION
    ):
        blockers.append("schema_version")
    if report.get("status") not in {"not_run", "fail", "pass"}:
        blockers.append("status_type")
    if report.get("markhandE2e") is not True:
        extra = list(blockers)
        if "MARKHAND_E2E!=1" not in extra:
            extra.append("MARKHAND_E2E!=1")
        return "not_run", extra

    raw_dir: Path | None = None
    raw_dir_value = report.get("rawDir")
    if not raw_dir_value:
        blockers.append("raw_dir_missing")
    else:
        raw_dir, raw_error = normalize_raw_dir(
            str(raw_dir_value), must_exist=raw_must_exist
        )
        if raw_error:
            blockers.append(raw_error)

    manifest: dict[str, Any] | None = None
    recomputed_suites: dict[str, Any] | None = None
    recomputed_observed: list[str] | None = None
    if raw_dir is not None and raw_must_exist:
        manifest, manifest_blockers = load_raw_manifest(raw_dir)
        blockers.extend(manifest_blockers)
        if manifest is not None:
            recomputed_suites, recomputed_observed, raw_blockers = recompute_raw_suites(
                report, raw_dir, manifest
            )
            blockers.extend(raw_blockers)

    expected = sorted(report.get("expectedFormats") or [])
    if expected != expected_formats:
        blockers.append("expected_formats_mismatch")

    observed = sorted(
        recomputed_observed
        if recomputed_observed is not None
        else report.get("formatsObserved") or []
    )
    if observed != expected:
        blockers.append("partial_format")

    suites = (
        recomputed_suites
        if recomputed_suites is not None
        else report.get("suites") or {}
    )
    if not isinstance(suites, dict) or not suites:
        blockers.append("missing:suites")
    for suite_key in REQUIRED_SUITES:
        suite = suites.get(suite_key) if isinstance(suites, dict) else None
        if not isinstance(suite, dict):
            blockers.append(f"missing_suite:{suite_key}")
            continue
        # Reject illegal multi-filter command shapes recorded in evidence.
        commands = suite.get("commands") or (
            [suite["command"]] if suite.get("command") else []
        )
        for cmd in commands:
            if not isinstance(cmd, list):
                blockers.append(f"command_shape:{suite_key}")
                continue
            try:
                validate_cargo_command_shape([str(x) for x in cmd])
            except ValueError:
                blockers.append(f"command_shape:{suite_key}")
        try:
            validate_canonical_suite_command(
                suite_key,
                [[str(x) for x in cmd] for cmd in commands if isinstance(cmd, list)],
            )
        except ValueError:
            blockers.append(f"noncanonical_command:{suite_key}")
        if len(suite.get("rawLogs") or []) != 1:
            blockers.append(f"raw_logs_not_unique:{suite_key}")
        if suite.get("skipped"):
            blockers.append(f"skipped:{suite_key}")
        if suite.get("ignored"):
            blockers.append(f"ignored:{suite_key}")
        if int(suite.get("ignoredCount") or 0) > 0:
            blockers.append(f"ignored:{suite_key}")
        if suite.get("timedOut"):
            blockers.append(f"timeout:{suite_key}")
        tests_run = int(suite.get("testsRun") or 0)
        if tests_run <= 0:
            blockers.append(f"zero_test:{suite_key}")
        if suite.get("exitCode") != 0:
            blockers.append(f"exit:{suite_key}")
        if not suite.get("passed"):
            blockers.append(f"failed:{suite_key}")

    for finding in report.get("findings") or []:
        if not isinstance(finding, dict):
            blockers.append("finding:invalid")
            continue
        sev = str(finding.get("severity") or "").lower()
        if sev in {"high", "critical"}:
            blockers.append(f"finding:{sev}")

    prov = report.get("provenance") or {}
    if not isinstance(prov, dict):
        blockers.append("missing:provenance")
        prov = {}
    for key in REQUIRED_PROVENANCE:
        val = prov.get(key)
        if val is None or val == "" or val == {} or val == []:
            blockers.append(f"provenance_missing:{key}")
    for key in [
        "migrationManifestSha256",
        "composeFileSha256",
        "effectiveComposeSha256",
        "f02ReportSha256",
        "f02ManifestSha256",
    ]:
        val = prov.get(key)
        if not (isinstance(val, str) and re.fullmatch(r"[0-9a-f]{64}", val)):
            blockers.append(f"provenance_hash_invalid:{key}")
    index_sig = prov.get("indexSignature")
    if not (isinstance(index_sig, str) and INDEX_SIG_RE.fullmatch(index_sig)):
        blockers.append("provenance_missing:indexSignature")

    image_ids = prov.get("imageIds") if isinstance(prov.get("imageIds"), dict) else {}
    container_ids = (
        prov.get("containerIds") if isinstance(prov.get("containerIds"), dict) else {}
    )
    missing_services = [svc for svc in EXPECTED_POC_SERVICES if svc not in image_ids]
    if missing_services:
        blockers.append("provenance_missing:expected_poc_services")
    missing_containers = [
        svc for svc in EXPECTED_POC_SERVICES if svc not in container_ids
    ]
    if missing_containers:
        blockers.append("provenance_missing:expected_poc_containers")
    service_map = (
        prov.get("composeServiceMap")
        if isinstance(prov.get("composeServiceMap"), dict)
        else {}
    )
    for svc in EXPECTED_POC_SERVICES:
        entry = service_map.get(svc) if isinstance(service_map, dict) else None
        if not isinstance(entry, dict):
            blockers.append(f"compose_service_map_missing:{svc}")
            continue
        labels = entry.get("labels") if isinstance(entry.get("labels"), dict) else {}
        if labels.get("com.docker.compose.project") != prov.get("composeProject"):
            blockers.append(f"compose_label_project:{svc}")
        if labels.get("com.docker.compose.service") != svc:
            blockers.append(f"compose_label_service:{svc}")
        if entry.get("containerId") != container_ids.get(svc):
            blockers.append(f"compose_container_mismatch:{svc}")
        if entry.get("imageId") != image_ids.get(svc):
            blockers.append(f"compose_image_mismatch:{svc}")

    endpoints = (
        prov.get("testEndpoints") if isinstance(prov.get("testEndpoints"), dict) else {}
    )
    for key in ["database", "appDatabase", "minio", "qdrant"]:
        if not endpoints.get(key):
            blockers.append(f"provenance_missing:test_endpoint:{key}")
    if "apiEndpoint" not in prov:
        blockers.append("provenance_missing:apiEndpoint")

    if bind_current_git:
        if current_git_full is None or current_git_dirty is None:
            try:
                _short, current_git_full, current_git_dirty = current_git_state()
            except subprocess.CalledProcessError:
                blockers.append("git_state_unavailable")
                current_git_full = None
                current_git_dirty = True
        if current_git_dirty:
            blockers.append("git_dirty")
        if current_git_full and prov.get("gitShaFull") != current_git_full:
            blockers.append("git_sha_mismatch")

    # Reject fabricated digest strings like "[] sha256:...".
    digests = (
        prov.get("imageDigests") if isinstance(prov.get("imageDigests"), dict) else {}
    )
    for svc, digest in digests.items():
        if (
            not isinstance(digest, str)
            or digest.startswith("[]")
            or "@sha256:" not in digest
        ):
            blockers.append(f"provenance_fake_digest:{svc}")

    arch = report.get("architecture") or {}
    if not isinstance(arch, dict):
        blockers.append("missing:architecture")
    else:
        if arch.get("kind") != "compose_api_public_http":
            blockers.append("architecture_kind")
        if arch.get("apiHttpExercised") is not True:
            blockers.append("api_http_not_exercised")

    api_probes = (
        report.get("blackBoxApiProbes")
        if isinstance(report.get("blackBoxApiProbes"), dict)
        else {}
    )
    if (
        api_probes.get("apiHttpExercised") is not True
        or api_probes.get("passed") is not True
    ):
        blockers.append("api_http_probes_not_passed")
    required_probe_names = {
        "health_live",
        "health_ready",
        "auth_me",
        "existing_resource_cross_tenant",
        "vertical_upload",
        "vertical_job",
        "vertical_search",
        "vertical_ask",
    }
    probes = (
        api_probes.get("probes") if isinstance(api_probes.get("probes"), dict) else {}
    )
    for name in required_probe_names:
        if (
            not isinstance(probes.get(name), dict)
            or probes[name].get("passed") is not True
        ):
            blockers.append(f"api_probe_failed:{name}")

    f02 = report.get("f02Boot") or {}
    for blocker in f02.get("validatorBlockers") or []:
        blockers.append(f"f02_validator:{blocker}")
    if not isinstance(f02, dict) or f02.get("passed") is not True:
        blockers.append("f02_boot_not_passed")
    else:
        f02_project = f02.get("composeProject")
        o04_project = prov.get("composeProject")
        if not f02_project:
            blockers.append("f02_missing_compose_project")
        elif f02_project != o04_project:
            blockers.append("f02_compose_project_mismatch")
        f02_images = (
            f02.get("imageIds") if isinstance(f02.get("imageIds"), dict) else {}
        )
        if not f02_images:
            blockers.append("f02_missing_image_ids")
        else:
            for svc, image_id in f02_images.items():
                if svc in EXPECTED_POC_SERVICES and image_ids.get(svc) != image_id:
                    blockers.append(f"f02_image_mismatch:{svc}")
        f02_containers = (
            f02.get("containerIds") if isinstance(f02.get("containerIds"), dict) else {}
        )
        if not f02_containers:
            blockers.append("f02_missing_container_ids")
        else:
            for svc, cid in f02_containers.items():
                if svc in EXPECTED_POC_SERVICES and container_ids.get(svc) != cid:
                    blockers.append(f"f02_container_mismatch:{svc}")
        if f02.get("reportSha256") != prov.get("f02ReportSha256"):
            blockers.append("f02_report_hash_mismatch")
        if f02.get("manifestSha256") != prov.get("f02ManifestSha256"):
            blockers.append("f02_manifest_hash_mismatch")
        if f02.get("composeFileSha256") != prov.get("composeFileSha256"):
            blockers.append("f02_compose_file_hash_mismatch")
        if f02.get("effectiveComposeSha256") != prov.get("effectiveComposeSha256"):
            blockers.append("f02_effective_compose_hash_mismatch")

    kill = (
        report.get("externalWorkerKill")
        if isinstance(report.get("externalWorkerKill"), dict)
        else {}
    )
    kill_required = {
        "harnessControlled": True,
        "stdoutProofAccepted": False,
        "deathVerified": True,
        "leaseExpired": True,
        "replacementWorkerVerified": True,
        "replacementReclaimed": True,
        "replayConsistent": True,
        "dbStateVerified": True,
    }
    for key, want in kill_required.items():
        if kill.get(key) is not want:
            blockers.append(f"external_worker_kill:{key}")
    if not kill.get("killedContainerId") or kill.get("killedContainerId") == kill.get(
        "replacementContainerId"
    ):
        blockers.append("external_worker_kill:distinct_replacement")

    redaction_paths = [report_path] if report_path else []
    redaction = scan_redaction(
        raw_dir, extra_paths=redaction_paths, extra_texts=[json.dumps(report)]
    )
    if not redaction.get("passed"):
        blockers.append("redaction_failed")

    seen: set[str] = set()
    uniq: list[str] = []
    for item in blockers:
        if item not in seen:
            seen.add(item)
            uniq.append(item)
    if uniq:
        return "fail", uniq
    return "pass", []


def suite_specs() -> dict[str, list[list[str]]]:
    """Each suite has exactly one canonical command and at most one libtest FILTER."""
    return {
        "vertical_slice_formats": [
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "retrieval_vertical_slice",
                "--",
                "--ignored",
                "--nocapture",
                "live_upload_convert_index_citation_vertical_slice",
            ]
        ],
        "unauthorized_cross_tenant": [
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "api_http_contracts",
                "--",
                "--ignored",
                "--nocapture",
                "live_http_unauthenticated_and_cross_tenant_are_consistent",
            ]
        ],
        "suspend_membership_delete_deny": [
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "citation_authz_matrix",
                "--",
                "--ignored",
                "--nocapture",
                "live_citation_authz_expiry_replay_idor_and_immediate_deny",
            ]
        ],
        # Whole hermetic uploads binary (no multi-filter). Live #[ignore] tests stay off.
        "adversarial_upload": [
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "uploads",
                "--",
                "--nocapture",
                "reject",
            ]
        ],
        "worker_kill_replay": [
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "worker",
                "--",
                "--ignored",
                "--nocapture",
                "live_convert_worker_cancel_loses_lease_and_kills_sandbox",
            ],
        ],
    }


def suite_commands_flat() -> dict[str, list[str]]:
    """Compatibility: first command only (tests use suite_specs)."""
    return {key: cmds[0] for key, cmds in suite_specs().items()}


def base_not_run_report(
    *, git_short: str, git_full: str, raw_dir: Path
) -> dict[str, Any]:
    f02 = load_f02_boot()
    return {
        "schemaVersion": SCHEMA_VERSION,
        "issue": ISSUE,
        "status": "not_run",
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "markhandE2e": False,
        "expectedFormats": list(EXPECTED_FORMATS),
        "formatsObserved": [],
        "workload": str(WORKLOAD_YAML.relative_to(ROOT)),
        "architecture": architecture_block(),
        "f02Boot": f02,
        "suites": {
            key: {
                "commands": [],
                "command": [],
                "exitCode": None,
                "testsRun": 0,
                "testsPassed": 0,
                "testsFailed": 0,
                "skipped": False,
                "ignored": False,
                "passed": False,
                "rawLog": None,
            }
            for key in REQUIRED_SUITES
        },
        "findings": [],
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "gitDirty": None,
            "dockerVersion": None,
            "composeVersion": None,
            "composeProject": compose_project(),
            "migrationManifestSha256": migration_manifest_sha256(),
            "composeFileSha256": sha256_file(COMPOSE_FILE),
            "effectiveComposeSha256": None,
            "f02ReportSha256": f02.get("reportSha256"),
            "f02ManifestSha256": f02.get("manifestSha256"),
            "indexSignature": None,
            "containerIds": {},
            "imageIds": {},
            "imageDigests": {},
            "composeServiceMap": {},
            "apiEndpoint": live_api_endpoint(),
            "testEndpoints": live_test_endpoints(),
        },
        "blackBoxApiProbes": {"apiHttpExercised": False, "passed": False, "probes": {}},
        "externalWorkerKill": {
            "harnessControlled": True,
            "stdoutProofAccepted": False,
            "deathVerified": False,
            "leaseExpired": False,
            "replacementWorkerVerified": False,
            "replacementReclaimed": False,
            "replayConsistent": False,
            "dbStateVerified": False,
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": repo_rel(raw_dir),
        "blockers": ["MARKHAND_E2E!=1"],
        "notes": (
            "Harness complete; live release suite not opted in. "
            "Set MARKHAND_E2E=1 with POC PG/MinIO/Qdrant + built fileconv + "
            "F02 poc-f02-boot.json passed=true (with composeProject/imageIds), then re-run. "
            "O04 pass also requires public HTTP Compose API probes and external worker kill proof."
        ),
    }


def write_reports(report: dict[str, Any]) -> None:
    if O05_SUMMARY.resolve() == (OUT / "o04-release.json").resolve():
        raise RuntimeError("refusing to treat O05 summary.json as O04 evidence")
    OUT.mkdir(parents=True, exist_ok=True)
    if OUT / "summary.json" == Path(report.get("rawDir") or ""):
        raise RuntimeError("refusing to write O04 evidence into O05 summary path")
    (OUT / "o04-release.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    arch = report.get("architecture") or {}
    md = [
        "# P1B-O04 vertical-slice / security release suite",
        "",
        f"- Status: `{report['status']}`",
        f"- Issue: `{report['issue']}`",
        f"- MARKHAND_E2E: `{report['markhandE2e']}`",
        f"- Architecture: `{arch.get('kind')}` (apiHttpExercised={arch.get('apiHttpExercised')})",
        f"- Expected formats (from phase1b-mixed.yaml): "
        f"`{', '.join(report.get('expectedFormats') or [])}`",
        f"- Formats observed: `{', '.join(report.get('formatsObserved') or []) or '(none)'}`",
        f"- Git: `{((report.get('provenance') or {}).get('gitShaFull'))}`",
        f"- F02 boot passed: `{(report.get('f02Boot') or {}).get('passed')}`",
        f"- Raw: `{report.get('rawDir')}`",
        "",
        "## Suites",
        "",
    ]
    for key in REQUIRED_SUITES:
        suite = (report.get("suites") or {}).get(key) or {}
        md.append(
            f"- `{key}`: passed={suite.get('passed')} exit={suite.get('exitCode')} "
            f"testsRun={suite.get('testsRun')} skipped={suite.get('skipped')} "
            f"ignored={suite.get('ignored')}"
        )
    md += ["", "## Blockers", ""]
    blockers = report.get("blockers") or []
    md += [f"- {b}" for b in blockers] or ["- (none)"]
    md += ["", "## Notes", "", str(report.get("notes") or ""), ""]
    (OUT / "o04-release.md").write_text("\n".join(md), encoding="utf-8")


def write_raw(raw_dir: Path, name: str, data: str) -> Path:
    raw_dir.mkdir(parents=True, exist_ok=True)
    path = raw_dir / name
    path.write_text(redact(data), encoding="utf-8")
    return path


def scan_redaction(
    raw_dir: Path | None,
    *,
    extra_paths: list[Path] | None = None,
    extra_texts: list[str] | None = None,
) -> dict[str, Any]:
    findings: list[str] = []
    if raw_dir is not None and raw_dir.is_dir():
        for path in sorted(raw_dir.rglob("*")):
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            if SECRETISH_RE.search(text):
                findings.append(f"residual_secret_pattern:{path.name}")
    for path in extra_paths or []:
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if SECRETISH_RE.search(text):
            findings.append(f"residual_secret_pattern:{path.name}")
    for idx, text in enumerate(extra_texts or []):
        if SECRETISH_RE.search(text):
            findings.append(f"residual_secret_pattern:memory:{idx}")
    return {"passed": not findings, "findings": findings}


def run_cargo(args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    validate_cargo_command_shape(args)
    bounded = run_bounded_command(
        args,
        env,
        timeout_secs=command_timeout_secs(),
        max_bytes=max_capture_bytes(),
    )
    marker = (
        f"\nO04_COMMAND_EXIT_CODE\t{bounded.returncode}\n"
        f"O04_COMMAND_TIMED_OUT\t{str(bounded.timed_out).lower()}\n"
        "O04_COMMAND_EOF\ttrue\n"
    )
    return subprocess.CompletedProcess(
        args, bounded.returncode, bounded.output + marker, ""
    )


def aggregate_suite_runs(
    commands: list[list[str]], logs: list[str], exit_codes: list[int]
) -> dict[str, Any]:
    tests_run = tests_passed = tests_failed = 0
    skipped = False
    ignored = False
    ignored_count = 0
    timed_out = False
    formats: set[str] = set()
    for log, code in zip(logs, exit_codes):
        parsed = parse_cargo_result(log)
        tests_run += parsed["testsRun"]
        tests_passed += parsed["testsPassed"]
        tests_failed += parsed["testsFailed"]
        ignored_count += parsed["ignoredCount"]
        skipped = skipped or parsed["skipped"]
        ignored = ignored or parsed["ignoredCount"] > 0 or parsed["hasIgnoredLine"]
        timed_out = timed_out or parsed["timedOut"] is True
        formats.update(parsed["formatsObserved"])
        if code != 0:
            tests_failed = max(tests_failed, 1)
    exit_code = 0 if all(code == 0 for code in exit_codes) else 1
    passed = (
        exit_code == 0
        and not skipped
        and not ignored
        and not timed_out
        and tests_run > 0
        and tests_failed == 0
    )
    return {
        "commands": commands,
        "command": commands[0] if commands else [],
        "exitCode": exit_code,
        "testsRun": tests_run,
        "testsPassed": tests_passed,
        "testsFailed": tests_failed,
        "ignoredCount": ignored_count,
        "skipped": skipped,
        "ignored": ignored,
        "timedOut": timed_out,
        "passed": passed,
        "formatsObserved": sorted(formats),
    }


def _http_json(
    method: str,
    base_url: str,
    path: str,
    *,
    token: str | None = None,
    body: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
    timeout: int = 20,
) -> tuple[int, dict[str, Any], str]:
    import urllib.error
    import urllib.request

    data = None
    merged_headers = dict(headers or {})
    if token:
        merged_headers["Authorization"] = f"Bearer {token}"
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        merged_headers["Content-Type"] = "application/json"
    req = urllib.request.Request(
        urljoin(base_url.rstrip("/") + "/", path.lstrip("/")),
        data=data,
        method=method,
        headers=merged_headers,
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            try:
                parsed = json.loads(raw) if raw else {}
            except json.JSONDecodeError:
                parsed = {"raw": raw}
            return resp.status, parsed, raw
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed = {"raw": raw}
        return exc.code, parsed, raw
    except Exception as exc:  # noqa: BLE001 - evidence should capture transport failures.
        return 0, {"error": str(exc)}, str(exc)


def _http_multipart_upload(
    base_url: str,
    token: str,
    collection_id: str,
    marker: str,
    *,
    timeout: int = 30,
) -> tuple[int, dict[str, Any], str]:
    import urllib.error
    import urllib.request

    boundary = "----markhandO04HttpProbeBoundary"
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="collectionId"\r\n\r\n{collection_id}\r\n'
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="o04-http-probe.txt"\r\n'
        "Content-Type: text/plain\r\n\r\n"
        f"{marker}\n"
        f"\r\n--{boundary}--\r\n"
    ).encode("utf-8")
    req = urllib.request.Request(
        urljoin(base_url.rstrip("/") + "/", "api/v1/uploads"),
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Idempotency-Key": f"o04-http-{hashlib.sha256(marker.encode()).hexdigest()[:16]}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            return resp.status, json.loads(raw), raw
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed = {"raw": raw}
        return exc.code, parsed, raw
    except Exception as exc:  # noqa: BLE001
        return 0, {"error": str(exc)}, str(exc)


def _psql_json(
    container: str, sql: str
) -> tuple[dict[str, Any] | None, dict[str, Any]]:
    user = os.environ.get("MARKHAND_O04_PSQL_USER", "markhand")
    db = os.environ.get("MARKHAND_O04_PSQL_DB", "markhand")
    result = run_bounded_command(
        [
            "docker",
            "exec",
            container,
            "psql",
            "-U",
            user,
            "-d",
            db,
            "-AtX",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            sql,
        ],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=256 * 1024,
    )
    meta = {
        "exitCode": result.returncode,
        "timedOut": result.timed_out,
        "truncated": result.truncated,
    }
    if result.returncode != 0 or result.timed_out or result.truncated:
        meta["error"] = redact(result.output[-2000:])
        return None, meta
    lines = [line for line in result.output.splitlines() if line.strip()]
    if not lines:
        meta["error"] = "empty_psql_output"
        return None, meta
    try:
        parsed = json.loads(lines[-1])
    except json.JSONDecodeError as exc:
        meta["error"] = f"invalid_json:{exc}"
        return None, meta
    if not isinstance(parsed, dict):
        meta["error"] = "json_not_object"
        return None, meta
    return parsed, meta


def _job_state_sql(job_id: str) -> str:
    return f"""
SELECT COALESCE((
    SELECT jsonb_build_object(
        'found', true,
        'id', j.id::text,
        'orgId', j.org_id::text,
        'jobType', j.job_type,
        'status', j.status,
        'attempts', j.attempts,
        'maxAttempts', j.max_attempts,
        'leaseOwnerPresent', j.lease_owner IS NOT NULL,
        'leaseOwnerPrefix', split_part(COALESCE(j.lease_owner, ''), ':', 1),
        'leaseOwnerFingerprint', CASE
            WHEN j.lease_owner IS NULL THEN NULL
            ELSE md5(j.lease_owner)
        END,
        'leaseExpired', j.lease_expires_at IS NOT NULL
            AND j.lease_expires_at < clock_timestamp(),
        'eventCounts', COALESCE((
            SELECT jsonb_object_agg(event_type, n)
            FROM (
                SELECT event_type, count(*)::int AS n
                FROM event_log
                WHERE job_id = j.id
                GROUP BY event_type
            ) e
        ), '{{}}'::jsonb),
        'outboxCounts', COALESCE((
            SELECT jsonb_object_agg(event_type, n)
            FROM (
                SELECT event_type, count(*)::int AS n
                FROM outbox_events
                WHERE job_id = j.id
                GROUP BY event_type
            ) o
        ), '{{}}'::jsonb)
    )
    FROM jobs j
    WHERE j.id = '{job_id}'::uuid
), '{{"found": false}}'::jsonb)::text;
"""


def _event_count(state: dict[str, Any] | None, event_type: str) -> int:
    counts = state.get("eventCounts") if isinstance(state, dict) else {}
    if not isinstance(counts, dict):
        return 0
    raw = counts.get(event_type, 0)
    try:
        return int(raw)
    except (TypeError, ValueError):
        return 0


def _poll_job_state(
    container: str,
    job_id: str,
    predicate,
    *,
    attempts: int,
    sleep_secs: float,
) -> tuple[dict[str, Any] | None, list[dict[str, Any]]]:
    observations: list[dict[str, Any]] = []
    for _ in range(max(1, attempts)):
        state, meta = _psql_json(container, _job_state_sql(job_id))
        observation = {"query": meta}
        if state is not None:
            observation["state"] = state
            if predicate(state):
                observations.append(observation)
                return state, observations
        observations.append(observation)
        time.sleep(max(0.1, sleep_secs))
    return None, observations


def _bearer_token(base_url: str, evidence: dict[str, Any]) -> str | None:
    token = os.environ.get("MARKHAND_O04_API_BEARER_TOKEN", "").strip()
    if token:
        evidence["authSource"] = "MARKHAND_O04_API_BEARER_TOKEN"
        return token
    email = os.environ.get("MARKHAND_O04_API_EMAIL", "admin@poc.example")
    password = os.environ.get("MARKHAND_O04_API_PASSWORD") or os.environ.get(
        "MARKHAND_DEV_PASSWORD"
    )
    if not password:
        evidence["authSource"] = "missing"
        return None
    status, body, _raw = _http_json(
        "POST",
        base_url,
        "/api/v1/auth/login",
        body={"email": email, "password": password},
    )
    evidence["authSource"] = "password_login"
    evidence["loginStatus"] = status
    access = body.get("accessToken") or body.get("access_token")
    return access if isinstance(access, str) and access else None


def run_api_http_probes(raw_dir: Path) -> dict[str, Any]:
    base = live_api_endpoint()
    evidence: dict[str, Any] = {
        "apiHttpExercised": False,
        "passed": False,
        "endpoint": base,
        "probes": {},
    }
    if not base:
        evidence["blocker"] = "api_endpoint_missing"
        return evidence

    def record(
        name: str, passed: bool, status: int, details: dict[str, Any] | None = None
    ) -> None:
        evidence["probes"][name] = {
            "passed": bool(passed),
            "status": status,
            "details": details or {},
        }

    status, body, _ = _http_json("GET", base, "/api/v1/health/live")
    record("health_live", status == 200, status, {"status": body.get("status")})
    status, body, _ = _http_json("GET", base, "/api/v1/health/ready")
    record("health_ready", status == 200, status, {"status": body.get("status")})

    token = _bearer_token(base, evidence)
    if not token:
        record("auth_me", False, 0, {"error": "missing_token_or_login"})
    else:
        status, body, _ = _http_json("GET", base, "/api/v1/auth/me", token=token)
        record(
            "auth_me",
            status == 200 and bool(body.get("orgId") or body.get("org_id")),
            status,
        )

    collection_id = os.environ.get(
        "MARKHAND_O04_COLLECTION_ID", "55555555-5555-5555-5555-555555555501"
    )
    foreign_collection = os.environ.get("MARKHAND_O04_FOREIGN_COLLECTION_ID")
    foreign_document = os.environ.get("MARKHAND_O04_FOREIGN_DOCUMENT_ID")
    if token and foreign_collection and foreign_document:
        c_status, c_body, _ = _http_json(
            "GET", base, f"/api/v1/collections/{foreign_collection}", token=token
        )
        d_status, d_body, _ = _http_json(
            "GET", base, f"/api/v1/documents/{foreign_document}", token=token
        )
        leaked = str(c_body) + str(d_body)
        record(
            "existing_resource_cross_tenant",
            c_status == 404
            and d_status == 404
            and foreign_collection not in leaked
            and foreign_document not in leaked,
            max(c_status, d_status),
        )
    else:
        record(
            "existing_resource_cross_tenant",
            False,
            0,
            {"error": "MARKHAND_O04_FOREIGN_COLLECTION_ID/DOCUMENT_ID required"},
        )

    marker = f"O04HTTP{int(time.time())}"
    job_id = None
    if token:
        status, body, _ = _http_multipart_upload(
            base, token, collection_id, f"Kinh phi {marker} la 15 trieu dong"
        )
        job_id = body.get("jobId") or body.get("job_id")
        evidence["documentId"] = body.get("documentId") or body.get("document_id")
        evidence["jobId"] = job_id
        record("vertical_upload", status == 201 and isinstance(job_id, str), status)
    else:
        record("vertical_upload", False, 0, {"error": "missing_token"})

    job_terminal = False
    if token and isinstance(job_id, str):
        last_status = 0
        last_body: dict[str, Any] = {}
        for _ in range(int(os.environ.get("MARKHAND_O04_JOB_POLL_ATTEMPTS", "30"))):
            last_status, last_body, _ = _http_json(
                "GET", base, f"/api/v1/jobs/{job_id}", token=token
            )
            state = str(last_body.get("status") or last_body.get("state") or "").lower()
            if state in {"succeeded", "failed", "cancelled"}:
                job_terminal = state == "succeeded"
                break
            time.sleep(2)
        record(
            "vertical_job",
            last_status == 200 and job_terminal,
            last_status,
            {"terminalSucceeded": job_terminal},
        )
    else:
        record("vertical_job", False, 0, {"error": "missing_job"})

    if token:
        status, body, _ = _http_json(
            "POST",
            base,
            "/api/v1/search",
            token=token,
            body={"query": marker, "collectionIds": [collection_id], "limit": 5},
        )
        record(
            "vertical_search",
            status == 200,
            status,
            {"items": len(body.get("items") or body.get("hits") or [])},
        )
        status, body, _ = _http_json(
            "POST",
            base,
            "/api/v1/ask",
            token=token,
            body={
                "question": f"Kinh phi {marker}?",
                "collectionIds": [collection_id],
                "limit": 5,
            },
        )
        record(
            "vertical_ask",
            status == 200,
            status,
            {"hasAnswer": bool(body.get("answer"))},
        )
    else:
        record("vertical_search", False, 0, {"error": "missing_token"})
        record("vertical_ask", False, 0, {"error": "missing_token"})

    evidence["apiHttpExercised"] = True
    evidence["passed"] = all(
        probe.get("passed") is True for probe in evidence["probes"].values()
    )
    path = write_raw(
        raw_dir, "api-http-probes.json", json.dumps(evidence, indent=2, sort_keys=True)
    )
    evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
    return evidence


def run_external_worker_kill_probe(
    raw_dir: Path, project: str, api_job_id: str | None = None
) -> dict[str, Any]:
    evidence: dict[str, Any] = {
        "harnessControlled": True,
        "stdoutProofAccepted": False,
        "deathVerified": False,
        "leaseExpired": False,
        "replacementWorkerVerified": False,
        "replacementReclaimed": False,
        "replayConsistent": False,
        "dbStateVerified": False,
    }
    if os.environ.get("MARKHAND_O04_EXTERNAL_WORKER_KILL") != "1":
        evidence["blocker"] = "MARKHAND_O04_EXTERNAL_WORKER_KILL!=1"
        path = write_raw(
            raw_dir,
            "external-worker-kill.json",
            json.dumps(evidence, indent=2, sort_keys=True),
        )
        evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
        return evidence
    ids, _digests, containers, missing = collect_poc_image_metadata(project)
    killed = containers.get("worker-convert")
    postgres = containers.get("postgres")
    job_id = (
        os.environ.get("MARKHAND_O04_WORKER_KILL_JOB_ID") or api_job_id or ""
    ).strip()
    expected_worker_id = os.environ.get("MARKHAND_O04_WORKER_ID", "poc-convert-1")
    evidence["killedContainerId"] = killed
    evidence["postgresContainerId"] = postgres
    evidence["observedJobId"] = job_id or None
    evidence["missingServices"] = missing
    if not killed or not postgres:
        evidence["blocker"] = "worker_or_postgres_container_missing"
        path = write_raw(
            raw_dir,
            "external-worker-kill.json",
            json.dumps(evidence, indent=2, sort_keys=True),
        )
        evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
        return evidence
    if not job_id or not UUID_RE.match(job_id):
        evidence["blocker"] = (
            "MARKHAND_O04_WORKER_KILL_JOB_ID required for DB-bound kill proof"
        )
        path = write_raw(
            raw_dir,
            "external-worker-kill.json",
            json.dumps(evidence, indent=2, sort_keys=True),
        )
        evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
        return evidence
    before, before_observations = _poll_job_state(
        postgres,
        job_id,
        lambda state: state.get("status") == "leased"
        and state.get("leaseOwnerPrefix") == expected_worker_id,
        attempts=int(
            os.environ.get("MARKHAND_O04_WORKER_KILL_LEASE_POLL_ATTEMPTS", "30")
        ),
        sleep_secs=1,
    )
    evidence["dbBeforeKill"] = before
    evidence["dbBeforeKillObservations"] = before_observations[-3:]
    if not before:
        evidence["blocker"] = "job_not_leased_by_target_worker_before_kill"
        path = write_raw(
            raw_dir,
            "external-worker-kill.json",
            json.dumps(evidence, indent=2, sort_keys=True),
        )
        evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
        return evidence
    kill = run_bounded_command(
        ["docker", "kill", killed],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=64 * 1024,
    )
    evidence["killExitCode"] = kill.returncode
    dead = run_bounded_command(
        ["docker", "inspect", "--format", "{{.State.Running}}", killed],
        os.environ.copy(),
        timeout_secs=30,
        max_bytes=64 * 1024,
    )
    dead_lines = dead.output.strip().splitlines()
    evidence["deathVerified"] = (
        dead.returncode == 0 and bool(dead_lines) and dead_lines[0] == "false"
    )
    expired, expired_observations = _poll_job_state(
        postgres,
        job_id,
        lambda state: state.get("status") == "leased"
        and state.get("leaseExpired") is True
        and state.get("leaseOwnerFingerprint") == before.get("leaseOwnerFingerprint"),
        attempts=int(
            os.environ.get("MARKHAND_O04_WORKER_KILL_EXPIRY_POLL_ATTEMPTS", "90")
        ),
        sleep_secs=1,
    )
    evidence["dbExpiredLease"] = expired
    evidence["dbExpiredLeaseObservations"] = expired_observations[-3:]
    evidence["leaseExpired"] = expired is not None
    recreate = run_bounded_command(
        [
            "docker",
            "compose",
            "-f",
            str(COMPOSE_FILE),
            "up",
            "-d",
            "--no-deps",
            "--force-recreate",
            "worker-convert",
        ],
        os.environ.copy(),
        timeout_secs=180,
        max_bytes=512 * 1024,
    )
    evidence["replacementCommandExitCode"] = recreate.returncode
    for _ in range(30):
        _ids, _digests, next_containers, _missing = collect_poc_image_metadata(project)
        replacement = next_containers.get("worker-convert")
        if replacement and replacement != killed:
            evidence["replacementContainerId"] = replacement
            evidence["replacementWorkerVerified"] = True
            break
        time.sleep(2)
    final, final_observations = _poll_job_state(
        postgres,
        job_id,
        lambda state: state.get("status")
        in {"succeeded", "failed", "dead_letter", "cancelled"},
        attempts=int(
            os.environ.get("MARKHAND_O04_WORKER_KILL_REPLAY_POLL_ATTEMPTS", "120")
        ),
        sleep_secs=2,
    )
    evidence["dbFinal"] = final
    evidence["dbFinalObservations"] = final_observations[-3:]
    before_attempts = int(before.get("attempts") or 0)
    final_attempts = int(final.get("attempts") or 0) if final else 0
    reclaimed_events = _event_count(final, "job.reclaimed") - _event_count(
        before, "job.reclaimed"
    )
    succeeded_events = _event_count(final, "job.succeeded") - _event_count(
        before, "job.succeeded"
    )
    evidence["replacementReclaimed"] = (
        evidence["replacementWorkerVerified"] is True
        and reclaimed_events > 0
        and final_attempts > before_attempts
    )
    evidence["replayConsistent"] = (
        final is not None
        and final.get("status") == "succeeded"
        and succeeded_events > 0
        and final.get("leaseOwnerPresent") is False
    )
    evidence["dbStateVerified"] = (
        evidence["deathVerified"] is True
        and evidence["leaseExpired"] is True
        and evidence["replacementReclaimed"] is True
        and evidence["replayConsistent"] is True
    )
    path = write_raw(
        raw_dir,
        "external-worker-kill.json",
        json.dumps(evidence, indent=2, sort_keys=True),
    )
    evidence["rawLog"] = path.relative_to(raw_dir).as_posix()
    return evidence


def self_test() -> None:
    RAW_ROOT.mkdir(parents=True, exist_ok=True)
    temp = tempfile.TemporaryDirectory(prefix="o04-self-test-", dir=RAW_ROOT)
    raw = Path(temp.name)
    raw_artifacts: list[Path] = []

    def cargo_log(
        test_names: str | list[str],
        *,
        formats: list[str] | None = None,
        ignored: int = 0,
    ) -> str:
        status = "ignored" if ignored else "ok"
        names = [test_names] if isinstance(test_names, str) else list(test_names)
        coverage = (
            f"O04_FORMAT_COVERAGE\t{json.dumps(formats)}\n"
            if formats is not None
            else ""
        )
        lines = "".join(f"test {name} ... {status}\n" for name in names)
        passed = 0 if ignored else len(names)
        return (
            f"{lines}"
            f"{coverage}"
            f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; "
            "0 measured; 0 filtered out\n"
            "O04_COMMAND_EXIT_CODE\t0\n"
            "O04_COMMAND_TIMED_OUT\tfalse\n"
            "O04_COMMAND_OUTPUT_TRUNCATED\tfalse\n"
            "O04_COMMAND_EOF\ttrue\n"
        )

    # Command-shape: production suite specs must be valid.
    for key, commands in suite_specs().items():
        assert commands, key
        for cmd in commands:
            validate_cargo_command_shape(cmd)
            assert len(filters_after_double_dash(cmd)) <= 1, (key, cmd)

    # Negative: multiple filters must raise.
    bad_cmd = [
        "cargo",
        "test",
        "-p",
        "fileconv-server",
        "--test",
        "uploads",
        "--",
        "--nocapture",
        "spoof_pdf_and_html_pdf_reject",
        "malformed_and_traversal_docx_reject",
    ]
    try:
        validate_cargo_command_shape(bad_cmd)
        raise AssertionError("expected multi-filter command to raise")
    except ValueError as exc:
        assert "only one FILTER" in str(exc)

    # Formats come from workload YAML (includes png).
    formats = load_expected_formats()
    assert formats == sorted(
        ["pdf", "docx", "pptx", "xlsx", "csv", "html", "txt", "png"]
    ), formats
    assert "png" in formats

    good_suite = {
        "commands": suite_specs()["adversarial_upload"],
        "command": suite_specs()["adversarial_upload"][0],
        "exitCode": 0,
        "testsRun": 2,
        "testsPassed": 2,
        "testsFailed": 0,
        "skipped": False,
        "ignored": False,
        "passed": True,
        "rawLog": "suite.txt",
    }
    image_ids = {svc: f"sha256:{i:064d}" for i, svc in enumerate(EXPECTED_POC_SERVICES)}
    container_ids = {svc: f"container-{svc}" for svc in EXPECTED_POC_SERVICES}
    good = {
        "schemaVersion": SCHEMA_VERSION,
        "issue": ISSUE,
        "status": "pass",
        "markhandE2e": True,
        "expectedFormats": list(formats),
        "formatsObserved": list(formats),
        "architecture": architecture_block(),
        "f02Boot": {
            "path": str(F02_BOOT),
            "reportSha256": "f" * 64,
            "manifestSha256": "1" * 64,
            "passed": True,
            "composeProject": DEFAULT_COMPOSE_PROJECT,
            "containerIds": dict(container_ids),
            "imageIds": dict(image_ids),
            "composeFileSha256": "d" * 64,
            "effectiveComposeSha256": "e" * 64,
        },
        "blackBoxApiProbes": {
            "apiHttpExercised": True,
            "passed": True,
            "probes": {
                "health_live": {"passed": True, "status": 200},
                "health_ready": {"passed": True, "status": 200},
                "auth_me": {"passed": True, "status": 200},
                "existing_resource_cross_tenant": {"passed": True, "status": 404},
                "vertical_upload": {"passed": True, "status": 201},
                "vertical_job": {"passed": True, "status": 200},
                "vertical_search": {"passed": True, "status": 200},
                "vertical_ask": {"passed": True, "status": 200},
            },
        },
        "externalWorkerKill": {
            "harnessControlled": True,
            "stdoutProofAccepted": False,
            "deathVerified": True,
            "leaseExpired": True,
            "replacementWorkerVerified": True,
            "replacementReclaimed": True,
            "replayConsistent": True,
            "dbStateVerified": True,
            "killedContainerId": "old-worker",
            "replacementContainerId": "new-worker",
        },
        "suites": {k: dict(good_suite) for k in REQUIRED_SUITES},
        "findings": [],
        "provenance": {
            "gitSha": "abc1234",
            "gitShaFull": "abc1234deadbeef",
            "gitDirty": False,
            "dockerVersion": "Docker version 29",
            "composeVersion": "Docker Compose version 2",
            "composeProject": DEFAULT_COMPOSE_PROJECT,
            "migrationManifestSha256": "a" * 64,
            "composeFileSha256": "d" * 64,
            "effectiveComposeSha256": "e" * 64,
            "f02ReportSha256": "f" * 64,
            "f02ManifestSha256": "1" * 64,
            "indexSignature": "b" * 64,
            "containerIds": dict(container_ids),
            "imageIds": dict(image_ids),
            "imageDigests": {"postgres": "postgres@sha256:" + ("c" * 64)},
            "composeServiceMap": {
                svc: {
                    "containerId": container_ids[svc],
                    "imageId": image_ids[svc],
                    "health": "healthy",
                    "running": True,
                    "ports": {},
                    "labels": {
                        "com.docker.compose.project": DEFAULT_COMPOSE_PROJECT,
                        "com.docker.compose.service": svc,
                    },
                }
                for svc in EXPECTED_POC_SERVICES
            },
            "apiEndpoint": None,
            "testEndpoints": {
                "database": "postgres://127.0.0.1:5432/markhand",
                "appDatabase": "postgres://127.0.0.1:5432/markhand",
                "minio": "http://127.0.0.1:9000",
                "qdrant": "http://127.0.0.1:6333",
            },
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": repo_rel(raw),
        "blockers": [],
    }
    # Patch suite commands to valid shapes from suite_specs.
    for key, cmds in suite_specs().items():
        good["suites"][key]["commands"] = cmds
        good["suites"][key]["command"] = cmds[0]
        raw_logs = []
        for idx, _cmd in enumerate(cmds):
            formats_for_log = (
                formats if key == "vertical_slice_formats" and idx == 0 else None
            )
            path = write_raw(
                raw,
                f"{key}.{idx}.txt",
                cargo_log(
                    EXPECTED_SUITE_TESTS[key],
                    formats=formats_for_log,
                ),
            )
            raw_artifacts.append(path)
            raw_logs.append(path.relative_to(raw).as_posix())
        good["suites"][key]["rawLogs"] = raw_logs
        good["suites"][key]["rawLog"] = raw_logs[0]
    write_raw_manifest(raw, raw_artifacts)

    status, blockers = evaluate_report(
        good, current_git_full="abc1234deadbeef", current_git_dirty=False
    )
    assert status == "pass" and not blockers, (status, blockers)

    missing = dict(good)
    del missing["suites"]
    status, blockers = evaluate_report(missing)
    assert status != "pass" and any(b.startswith("missing:") for b in blockers), (
        blockers
    )

    skipped = json.loads(json.dumps(good))
    skipped_path = raw / skipped["suites"]["vertical_slice_formats"]["rawLogs"][0]
    skipped_path.write_text(
        "skipped: MARKHAND_TEST_QDRANT_URL unset\n"
        "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
        "O04_COMMAND_EXIT_CODE\t0\n"
        "O04_COMMAND_TIMED_OUT\tfalse\n"
        "O04_COMMAND_OUTPUT_TRUNCATED\tfalse\n"
        "O04_COMMAND_EOF\ttrue\n",
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(skipped)
    assert status != "pass" and "skipped:vertical_slice_formats" in blockers, blockers
    skipped_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["vertical_slice_formats"], formats=formats),
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)

    zero = json.loads(json.dumps(good))
    zero_path = raw / zero["suites"]["adversarial_upload"]["rawLogs"][0]
    zero_path.write_text(
        "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
        "O04_COMMAND_EXIT_CODE\t0\n"
        "O04_COMMAND_TIMED_OUT\tfalse\n"
        "O04_COMMAND_OUTPUT_TRUNCATED\tfalse\n"
        "O04_COMMAND_EOF\ttrue\n",
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(zero)
    assert status != "pass" and "zero_test:adversarial_upload" in blockers, blockers
    zero_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["adversarial_upload"]), encoding="utf-8"
    )
    write_raw_manifest(raw, raw_artifacts)

    partial = json.loads(json.dumps(good))
    partial_path = raw / partial["suites"]["vertical_slice_formats"]["rawLogs"][0]
    partial_path.write_text(
        cargo_log(
            EXPECTED_SUITE_TESTS["vertical_slice_formats"], formats=["pdf", "txt"]
        ),
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(partial)
    assert status != "pass" and "partial_format" in blockers, blockers
    partial_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["vertical_slice_formats"], formats=formats),
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)

    high = json.loads(json.dumps(good))
    high["findings"] = [{"severity": "critical", "id": "x"}]
    status, blockers = evaluate_report(high)
    assert status != "pass" and "finding:critical" in blockers, blockers

    # Multi-filter command shape in report => non-pass.
    shaped = json.loads(json.dumps(good))
    shaped["suites"]["adversarial_upload"]["commands"] = [bad_cmd]
    shaped["suites"]["adversarial_upload"]["command"] = bad_cmd
    status, blockers = evaluate_report(shaped)
    assert status != "pass" and "command_shape:adversarial_upload" in blockers, blockers

    # Fake digest string rejected.
    fake = json.loads(json.dumps(good))
    fake["provenance"]["imageDigests"] = {"api": "[] sha256:" + ("d" * 64)}
    status, blockers = evaluate_report(fake)
    assert status != "pass" and "provenance_fake_digest:api" in blockers, blockers

    # F02 not passed / missing image provenance => non-pass.
    no_f02 = json.loads(json.dumps(good))
    no_f02["f02Boot"] = {"passed": False, "path": str(F02_BOOT)}
    status, blockers = evaluate_report(no_f02)
    assert status != "pass" and "f02_boot_not_passed" in blockers, blockers

    no_f02_images = json.loads(json.dumps(good))
    no_f02_images["f02Boot"] = {
        "passed": True,
        "composeProject": DEFAULT_COMPOSE_PROJECT,
        "containerIds": dict(container_ids),
        "imageIds": {},
    }
    status, blockers = evaluate_report(no_f02_images)
    assert status != "pass" and "f02_missing_image_ids" in blockers, blockers

    # Invalid index signature => non-pass.
    bad_sig = json.loads(json.dumps(good))
    bad_sig["provenance"]["indexSignature"] = "not-a-hex"
    status, blockers = evaluate_report(bad_sig)
    assert status != "pass" and "provenance_missing:indexSignature" in blockers, (
        blockers
    )

    # API HTTP must be exercised by black-box probes.
    false_http = json.loads(json.dumps(good))
    false_http["architecture"]["apiHttpExercised"] = False
    false_http["blackBoxApiProbes"]["apiHttpExercised"] = False
    status, blockers = evaluate_report(false_http)
    assert status != "pass" and "api_http_not_exercised" in blockers, blockers

    # Stale project/container/image provenance must not validate.
    wrong_project = json.loads(json.dumps(good))
    wrong_project["provenance"]["composeProject"] = "other-project"
    status, blockers = evaluate_report(wrong_project)
    assert status != "pass" and "f02_compose_project_mismatch" in blockers, blockers

    wrong_container = json.loads(json.dumps(good))
    wrong_container["provenance"]["containerIds"]["api"] = "other-api-container"
    status, blockers = evaluate_report(wrong_container)
    assert status != "pass" and "f02_container_mismatch:api" in blockers, blockers

    stale_sha = json.loads(json.dumps(good))
    status, blockers = evaluate_report(
        stale_sha,
        bind_current_git=True,
        current_git_full="different-full-sha",
        current_git_dirty=False,
    )
    assert status != "pass" and "git_sha_mismatch" in blockers, blockers

    dirty = json.loads(json.dumps(good))
    status, blockers = evaluate_report(
        dirty,
        bind_current_git=True,
        current_git_full=dirty["provenance"]["gitShaFull"],
        current_git_dirty=True,
    )
    assert status != "pass" and "git_dirty" in blockers, blockers

    # Raw logs are authoritative: an ignored libtest result blocks even if the report claims pass.
    ignored_raw = json.loads(json.dumps(good))
    raw_ignored_path = raw / ignored_raw["suites"]["worker_kill_replay"]["rawLogs"][0]
    raw_ignored_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["worker_kill_replay"], ignored=1),
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(ignored_raw)
    assert status != "pass" and "ignored:worker_kill_replay" in blockers, blockers
    raw_ignored_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["worker_kill_replay"]), encoding="utf-8"
    )
    write_raw_manifest(raw, raw_artifacts)

    stdout_only = json.loads(json.dumps(good))
    stdout_only["externalWorkerKill"]["deathVerified"] = False
    stdout_only["externalWorkerKill"]["leaseExpired"] = False
    cooperative_path = raw / stdout_only["suites"]["worker_kill_replay"]["rawLogs"][0]
    cooperative_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["worker_kill_replay"])
        + "O04_WORKER_HARD_KILL_EVIDENCE\tpid=123 lease_expired=true replay_consistent=true\n",
        encoding="utf-8",
    )
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(stdout_only)
    assert status != "pass" and "external_worker_kill:deathVerified" in blockers, (
        blockers
    )
    cooperative_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["worker_kill_replay"]), encoding="utf-8"
    )
    write_raw_manifest(raw, raw_artifacts)

    modified_raw = json.loads(json.dumps(good))
    raw_modified_path = raw / modified_raw["suites"]["adversarial_upload"]["rawLogs"][0]
    raw_modified_path.write_text(
        raw_modified_path.read_text(encoding="utf-8") + "tamper\n",
        encoding="utf-8",
    )
    status, blockers = evaluate_report(modified_raw)
    assert status != "pass" and any(
        b.startswith("raw_manifest_sha_mismatch") for b in blockers
    ), blockers
    raw_modified_path.write_text(
        cargo_log(EXPECTED_SUITE_TESTS["adversarial_upload"]), encoding="utf-8"
    )
    write_raw_manifest(raw, raw_artifacts)

    missing_raw = json.loads(json.dumps(good))
    (raw / missing_raw["suites"]["vertical_slice_formats"]["rawLogs"][0]).unlink()
    status, blockers = evaluate_report(missing_raw)
    assert status != "pass" and any(
        "raw_manifest_artifact_missing" in b or "raw_log_missing" in b for b in blockers
    ), blockers
    restored = write_raw(
        raw,
        "vertical_slice_formats.0.txt",
        cargo_log(EXPECTED_SUITE_TESTS["vertical_slice_formats"], formats=formats),
    )
    if restored not in raw_artifacts:
        raw_artifacts.append(restored)
    write_raw_manifest(raw, raw_artifacts)

    secret_raw = json.loads(json.dumps(good))
    secret_path = write_raw(
        raw, "redaction-safe.txt", "MARKHAND_MINIO_SECRET_KEY=notshown\n"
    )
    assert "notshown" not in secret_path.read_text(encoding="utf-8")
    leak_path = raw / "redaction-leak.txt"
    leak_path.write_text("FILECONV_LLM_SECRET_KEY=leaked-value\n", encoding="utf-8")
    raw_artifacts.append(leak_path)
    write_raw_manifest(raw, raw_artifacts)
    status, blockers = evaluate_report(secret_raw)
    assert status != "pass" and "redaction_failed" in blockers, blockers
    leak_path.unlink()
    raw_artifacts = [path for path in raw_artifacts if path.exists()]
    write_raw_manifest(raw, raw_artifacts)

    not_run = base_not_run_report(git_short="deadbee", git_full="deadbeef", raw_dir=raw)
    status, blockers = evaluate_report(not_run, raw_must_exist=True)
    assert status == "not_run", status
    assert "MARKHAND_E2E!=1" in blockers

    # Aggregate two worker subcommands correctly.
    agg = aggregate_suite_runs(
        suite_specs()["worker_kill_replay"],
        [
            "test live_convert_worker_cancel_loses_lease_and_kills_sandbox ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "test live_convert_worker_fault_injection_rolls_back_and_retries_promotion ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        ],
        [0, 0],
    )
    assert agg["testsRun"] == 2 and agg["passed"] is True, agg

    sample = (
        "skipped: MARKHAND_TEST_QDRANT_URL unset\n"
        "test live_upload_convert_index_citation_vertical_slice ... ok\n"
        'O04_FORMAT_COVERAGE\t["pdf","png","txt"]\n'
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
    )
    parsed = parse_cargo_result(sample)
    assert parsed["skipped"] is True
    assert parsed["formatsObserved"] == ["pdf", "png", "txt"]

    ignored_sample = (
        "test live_required_gate ... ignored\n"
        "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n"
        "O04_COMMAND_EXIT_CODE\t0\n"
        "O04_COMMAND_TIMED_OUT\tfalse\n"
    )
    agg_ignored = aggregate_suite_runs(
        [suite_specs()["vertical_slice_formats"][0]],
        [ignored_sample],
        [0],
    )
    assert agg_ignored["ignored"] is True and agg_ignored["passed"] is False, (
        agg_ignored
    )

    print("self-test ok")


def run_live(raw_dir: Path) -> dict[str, Any]:
    git_short, git_full, git_dirty = current_git_state()
    raw_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MARKHAND_E2E"] = "1"
    project = compose_project()
    image_ids, image_digests, container_ids, missing = collect_poc_image_metadata(
        project
    )
    compose_service_map = collect_compose_service_map(project)
    index_sig = resolve_index_signature()
    f02 = load_f02_boot()
    api_probes = run_api_http_probes(raw_dir)
    external_kill = run_external_worker_kill_probe(
        raw_dir,
        project,
        api_job_id=api_probes.get("jobId")
        if isinstance(api_probes.get("jobId"), str)
        else None,
    )
    raw_artifacts: list[Path] = [
        raw_dir / api_probes["rawLog"],
        raw_dir / external_kill["rawLog"],
    ]

    report: dict[str, Any] = {
        "schemaVersion": SCHEMA_VERSION,
        "issue": ISSUE,
        "status": "fail",
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "markhandE2e": True,
        "expectedFormats": list(EXPECTED_FORMATS),
        "formatsObserved": [],
        "workload": str(WORKLOAD_YAML.relative_to(ROOT)),
        "architecture": architecture_block(),
        "f02Boot": f02,
        "suites": {},
        "findings": [],
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "gitDirty": git_dirty,
            "dockerVersion": cmd_text(["docker", "--version"]),
            "composeVersion": cmd_text(["docker", "compose", "version"])
            or cmd_text(["docker-compose", "version"]),
            "composeProject": project,
            "migrationManifestSha256": migration_manifest_sha256(),
            "composeFileSha256": sha256_file(COMPOSE_FILE),
            "effectiveComposeSha256": effective_compose_sha256(),
            "f02ReportSha256": f02.get("reportSha256"),
            "f02ManifestSha256": f02.get("manifestSha256"),
            "indexSignature": index_sig,
            "containerIds": container_ids,
            "imageIds": image_ids,
            "imageDigests": image_digests,
            "composeServiceMap": compose_service_map,
            "missingPocServices": missing,
            "apiEndpoint": live_api_endpoint(),
            "testEndpoints": live_test_endpoints(),
        },
        "blackBoxApiProbes": api_probes,
        "externalWorkerKill": external_kill,
        "redactionScan": {"passed": False, "findings": []},
        "rawDir": repo_rel(raw_dir),
        "blockers": [],
        "notes": "Live O04 release suite against deployed Compose API plus cargo vertical/security suites.",
        "commands": {k: v for k, v in suite_specs().items()},
    }
    if git_dirty:
        raw_artifacts.append(
            write_raw(raw_dir, "git-dirty.txt", git_output("status", "--porcelain"))
        )
        manifest_path = write_raw_manifest(raw_dir, raw_artifacts)
        report["rawArtifactManifest"] = {
            "path": repo_rel(manifest_path),
            "sha256": sha256_file(manifest_path),
        }
        status, blockers = evaluate_report(
            report,
            bind_current_git=True,
            current_git_full=git_full,
            current_git_dirty=git_dirty,
        )
        report["status"] = status
        report["blockers"] = blockers
        report["redactionScan"] = scan_redaction(
            raw_dir, extra_texts=[json.dumps(report)]
        )
        report["notes"] = "Live O04 release suite refused to run with a dirty git tree."
        return report

    observed: set[str] = set()
    for key, commands in suite_specs().items():
        logs: list[str] = []
        codes: list[int] = []
        raw_logs: list[str] = []
        for idx, command in enumerate(commands):
            proc = run_cargo(command, env)
            log = (proc.stdout or "") + "\n" + (proc.stderr or "")
            path = write_raw(raw_dir, f"{key}.{idx}.txt", log)
            raw_artifacts.append(path)
            raw_logs.append(path.relative_to(raw_dir).as_posix())
            logs.append(log)
            codes.append(proc.returncode)
        suite = aggregate_suite_runs(commands, logs, codes)
        suite["rawLogs"] = raw_logs
        suite["rawLog"] = raw_logs[0] if raw_logs else None
        report["suites"][key] = suite
        observed.update(suite["formatsObserved"])
        if not suite["passed"]:
            report["findings"].append(
                {
                    "severity": "high" if key == "adversarial_upload" else "medium",
                    "suite": key,
                    "id": f"suite_failed:{key}",
                }
            )

    report["formatsObserved"] = sorted(observed)
    manifest_path = write_raw_manifest(raw_dir, raw_artifacts)
    report["rawArtifactManifest"] = {
        "path": repo_rel(manifest_path),
        "sha256": sha256_file(manifest_path),
    }
    report["redactionScan"] = scan_redaction(raw_dir, extra_texts=[json.dumps(report)])
    if not report["redactionScan"]["passed"]:
        report["findings"].append(
            {
                "severity": "critical",
                "id": "redaction_residual",
                "details": report["redactionScan"]["findings"],
            }
        )

    status, blockers = evaluate_report(
        report,
        bind_current_git=True,
        current_git_full=git_full,
        current_git_dirty=git_dirty,
    )
    report["status"] = status
    report["blockers"] = blockers
    if status != "pass":
        report["notes"] = (
            "Live run did not meet O04 pass gates; see blockers. "
            "Public HTTP API probes and external worker-kill proof are required."
        )
    else:
        report["notes"] = "All required O04 suites passed with complete format matrix."
    return report


def validate_report_cli(path: Path) -> int:
    report = json.loads(path.read_text(encoding="utf-8"))
    status, blockers = evaluate_report(
        report, raw_must_exist=True, report_path=path, bind_current_git=True
    )
    payload = {"status": status, "blockers": blockers}
    print(json.dumps(payload, indent=2, sort_keys=True))
    return 0 if status == "pass" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="P1B-O04 release suite harness")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--validate-report",
        type=Path,
        default=None,
        help="Validate an o04-release.json and print {status,blockers}",
    )
    parser.add_argument(
        "--raw-dir",
        type=Path,
        default=None,
        help="Override raw evidence directory (default raw/o04-<gitsha>)",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Write o04-release.json/.md and raw/ under this directory",
    )
    args = parser.parse_args()
    configure_output_dir(args.output_dir)
    if args.self_test:
        self_test()
        return 0
    if args.validate_report is not None:
        return validate_report_cli(args.validate_report.resolve())

    git_short, git_full, _git_dirty = current_git_state()
    raw_dir = (args.raw_dir or (RAW_ROOT / f"o04-{git_full}")).resolve()
    if not path_inside(raw_dir, RAW_ROOT):
        raise RuntimeError(f"raw evidence directory must stay under {RAW_ROOT}")
    raw_dir.mkdir(parents=True, exist_ok=True)

    if os.environ.get("MARKHAND_E2E") != "1":
        report = base_not_run_report(
            git_short=git_short, git_full=git_full, raw_dir=raw_dir
        )
        artifact = write_raw(
            raw_dir, "harness-not-run.txt", "MARKHAND_E2E!=1; evidence template only\n"
        )
        write_raw_manifest(raw_dir, [artifact])
        status, blockers = evaluate_report(report)
        report["status"] = status
        report["blockers"] = blockers
        write_reports(report)
        print(OUT / "o04-release.json")
        return 0

    report = run_live(raw_dir)
    write_reports(report)
    print(OUT / "o04-release.json")
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
