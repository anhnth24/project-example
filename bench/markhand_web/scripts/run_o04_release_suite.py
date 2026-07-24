#!/usr/bin/env python3
"""P1B-O04 vertical-slice / security release suite harness.

Machine-verifiable, redacted evidence under:
  bench/markhand_web/reports/phase-1b-gate/o04-release.{json,md}
  bench/markhand_web/reports/phase-1b-gate/raw/o04-<git>/

Architecture (honest): cargo integration tests boot in-process axum +
ConvertWorker/IndexWorker against live PG/MinIO/Qdrant service endpoints.
They do **not** exercise the Compose API container HTTP surface.

Never writes or overwrites O05 ``summary.json``.
Default status is honest ``not_run``. ``pass`` only when MARKHAND_E2E=1,
every required suite exits 0 with testsRun>0, format matrix matches
``phase1b-mixed.yaml``, F02 boot evidence passed with matching POC images,
provenance/raw/redaction gates hold, and no high/critical findings.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "bench/markhand_web/reports/phase-1b-gate"
O05_SUMMARY = OUT / "summary.json"
F02_BOOT = ROOT / "bench/markhand_web/reports/poc-f02-boot.json"
WORKLOAD_YAML = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
ISSUE = "P1B-O04"
DEFAULT_COMPOSE_PROJECT = "markhand-poc"

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
]

REQUIRED_PROVENANCE = [
    "gitSha",
    "gitShaFull",
    "dockerVersion",
    "composeVersion",
    "composeProject",
    "migrationManifestSha256",
    "indexSignature",
    "imageIds",
]

INDEX_SIG_RE = re.compile(r"^[0-9a-f]{64}$")
FORMAT_COVERAGE_RE = re.compile(r"^O04_FORMAT_COVERAGE\t(.+)$", re.MULTILINE)
TEST_OK_RE = re.compile(r"^test .+ \.\.\. ok$", re.MULTILINE)
TEST_FAILED_RE = re.compile(r"^test .+ \.\.\. FAILED$", re.MULTILINE)
TEST_IGNORED_RE = re.compile(r"^test .+ \.\.\. ignored$", re.MULTILINE)
SKIPPED_RE = re.compile(r"(?im)^skipped:")
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
    (re.compile(r"(Bearer\s+)[A-Za-z0-9._\-+=/]+"), r"\1[REDACTED]"),
    (re.compile(r"(postgres(?:ql)?://)[^@\s]+@"), r"\1[REDACTED]@"),
    (
        re.compile(
            r"(?i)(password|passwd|secret|token|authorization|api[_-]?key)"
            r"\"?\s*[:=]\s*\"?[^\s\",}]+"
        ),
        r"\1:[REDACTED]",
    ),
    (
        re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"),
        "[REDACTED_JWT]",
    ),
]


def load_expected_formats(path: Path = WORKLOAD_YAML) -> list[str]:
    """Single source of truth: ingest formats from phase1b-mixed.yaml."""
    text = path.read_text(encoding="utf-8")
    match = WORKLOAD_FORMATS_RE.search(text)
    if not match:
        raise RuntimeError(f"formats list missing in {path}")
    formats = sorted({part.strip().lower() for part in match.group(1).split(",") if part.strip()})
    if not formats:
        raise RuntimeError(f"empty formats list in {path}")
    return formats


EXPECTED_FORMATS = load_expected_formats()


def redact(text: str) -> str:
    out = text
    for pattern, repl in REDACT_PATTERNS:
        out = pattern.sub(repl, out)
    return out


def git_output(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def cmd_text(args: list[str]) -> str | None:
    try:
        proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return None
    if proc.returncode != 0 and not proc.stdout.strip():
        return None
    return (proc.stdout or proc.stderr or "").strip() or None


def migration_manifest_sha256() -> str:
    path = ROOT / "crates/server/migrations/manifest.json"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compose_project() -> str:
    return os.environ.get("MARKHAND_COMPOSE_PROJECT", DEFAULT_COMPOSE_PROJECT).strip() or (
        DEFAULT_COMPOSE_PROJECT
    )


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


def resolve_index_signature() -> str | None:
    """Machine-verifiable 64-lowercase-hex signature. Never logs secret env values."""
    env_sig = os.environ.get("MARKHAND_INDEX_SIGNATURE", "").strip()
    if INDEX_SIG_RE.fullmatch(env_sig):
        return env_sig
    # Fallback: inspect POC API container Config.Env for MARKHAND_INDEX_SIGNATURE only.
    project = compose_project()
    try:
        proc = subprocess.run(
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
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        return None
    cid = (proc.stdout or "").strip().splitlines()
    if not cid:
        return None
    insp = subprocess.run(
        ["docker", "inspect", "--format", "{{range .Config.Env}}{{println .}}{{end}}", cid[0]],
        capture_output=True,
        text=True,
        check=False,
    )
    if insp.returncode != 0:
        return None
    for line in (insp.stdout or "").splitlines():
        if line.startswith("MARKHAND_INDEX_SIGNATURE="):
            value = line.split("=", 1)[1].strip()
            if INDEX_SIG_RE.fullmatch(value):
                return value
            return None
    return None


def collect_poc_image_metadata(project: str) -> tuple[dict[str, str], dict[str, str], list[str]]:
    """Return (imageIds, imageDigests, missingServices) for Compose project services.

    Digests are recorded only when RepoDigests is non-empty. Locally built images
    keep immutable image IDs in imageIds without fabricating digest strings.
    """
    ids: dict[str, str] = {}
    digests: dict[str, str] = {}
    if not shutil.which("docker"):
        return ids, digests, list(EXPECTED_POC_SERVICES)
    proc = subprocess.run(
        [
            "docker",
            "ps",
            "-a",
            "--filter",
            f"label=com.docker.compose.project={project}",
            "--format",
            "{{.ID}}\t{{.Label \"com.docker.compose.service\"}}",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0 or not proc.stdout.strip():
        return ids, digests, list(EXPECTED_POC_SERVICES)
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        cid, service = parts[0].strip(), parts[1].strip()
        if not service:
            continue
        insp = subprocess.run(
            [
                "docker",
                "inspect",
                "--format",
                "{{.Image}}",
                cid,
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        if insp.returncode != 0:
            continue
        image_id = (insp.stdout or "").strip()
        if image_id:
            ids[service] = image_id
            # RepoDigests live on the image object, not the container.
            img = subprocess.run(
                [
                    "docker",
                    "image",
                    "inspect",
                    "--format",
                    "{{json .RepoDigests}}",
                    image_id,
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            repo_json = (img.stdout or "").strip() if img.returncode == 0 else "[]"
            try:
                repo = json.loads(repo_json) if repo_json else []
            except json.JSONDecodeError:
                repo = []
            if isinstance(repo, list):
                real = [d for d in repo if isinstance(d, str) and "@sha256:" in d]
                if real:
                    digests[service] = real[0]
    missing = [svc for svc in EXPECTED_POC_SERVICES if svc not in ids]
    return ids, digests, missing


def parse_cargo_result(log: str) -> dict[str, Any]:
    ok = len(TEST_OK_RE.findall(log))
    failed = len(TEST_FAILED_RE.findall(log))
    ignored_lines = len(TEST_IGNORED_RE.findall(log))
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
    }


def load_f02_boot() -> dict[str, Any]:
    if not F02_BOOT.is_file():
        return {"path": str(F02_BOOT), "passed": False, "error": "missing"}
    try:
        data = json.loads(F02_BOOT.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return {"path": str(F02_BOOT), "passed": False, "error": f"invalid_json:{exc}"}
    return {
        "path": str(F02_BOOT),
        "passed": bool(data.get("passed") is True),
        "issue": data.get("issue"),
        "composeProject": data.get("composeProject") or data.get("compose_project"),
        "imageIds": data.get("imageIds") or data.get("image_ids") or {},
        "stamp_utc": data.get("stamp_utc"),
    }


def architecture_block() -> dict[str, Any]:
    return {
        "kind": "in_process_workers_against_poc_services",
        "apiHttpExercised": False,
        "description": (
            "Cargo #[ignore] integration tests boot in-process axum + ConvertWorker/"
            "IndexWorker against live PG/MinIO/Qdrant endpoints from MARKHAND_TEST_*; "
            "they do not send HTTP to the Compose API container."
        ),
    }


def evaluate_report(report: dict[str, Any], *, raw_must_exist: bool = True) -> tuple[str, list[str]]:
    """Return (status, blockers). Only ``pass`` when every acceptance gate holds."""
    blockers: list[str] = []
    expected_formats = load_expected_formats()

    for key in REQUIRED_TOP_LEVEL:
        if key not in report:
            blockers.append(f"missing:{key}")

    if report.get("issue") != ISSUE:
        blockers.append("issue_mismatch")

    if not report.get("markhandE2e"):
        extra = list(blockers)
        if "MARKHAND_E2E!=1" not in extra:
            extra.append("MARKHAND_E2E!=1")
        return "not_run", extra

    expected = sorted(report.get("expectedFormats") or [])
    if expected != expected_formats:
        blockers.append("expected_formats_mismatch")

    observed = sorted(report.get("formatsObserved") or [])
    if observed != expected:
        blockers.append("partial_format")

    suites = report.get("suites") or {}
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
        if suite.get("skipped"):
            blockers.append(f"skipped:{suite_key}")
        if suite.get("ignored"):
            blockers.append(f"ignored:{suite_key}")
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
    index_sig = prov.get("indexSignature")
    if not (isinstance(index_sig, str) and INDEX_SIG_RE.fullmatch(index_sig)):
        blockers.append("provenance_missing:indexSignature")

    image_ids = prov.get("imageIds") if isinstance(prov.get("imageIds"), dict) else {}
    missing_services = [svc for svc in EXPECTED_POC_SERVICES if svc not in image_ids]
    if missing_services:
        blockers.append("provenance_missing:expected_poc_services")

    # Reject fabricated digest strings like "[] sha256:...".
    digests = prov.get("imageDigests") if isinstance(prov.get("imageDigests"), dict) else {}
    for svc, digest in digests.items():
        if not isinstance(digest, str) or digest.startswith("[]") or "@sha256:" not in digest:
            blockers.append(f"provenance_fake_digest:{svc}")

    arch = report.get("architecture") or {}
    if not isinstance(arch, dict):
        blockers.append("missing:architecture")
    else:
        if arch.get("kind") != "in_process_workers_against_poc_services":
            blockers.append("architecture_kind")
        if arch.get("apiHttpExercised") is True:
            blockers.append("architecture_false_api_http_claim")

    f02 = report.get("f02Boot") or {}
    if not isinstance(f02, dict) or f02.get("passed") is not True:
        blockers.append("f02_boot_not_passed")
    else:
        f02_project = f02.get("composeProject")
        o04_project = prov.get("composeProject")
        if not f02_project:
            blockers.append("f02_missing_compose_project")
        elif f02_project != o04_project:
            blockers.append("f02_compose_project_mismatch")
        f02_images = f02.get("imageIds") if isinstance(f02.get("imageIds"), dict) else {}
        if not f02_images:
            blockers.append("f02_missing_image_ids")
        else:
            for svc, image_id in f02_images.items():
                if image_ids.get(svc) != image_id:
                    blockers.append(f"f02_image_mismatch:{svc}")

    raw_dir = report.get("rawDir")
    if not raw_dir:
        blockers.append("raw_dir_missing")
    elif raw_must_exist and not Path(str(raw_dir)).is_dir():
        blockers.append("raw_dir_missing")

    redaction = report.get("redactionScan") or {}
    if not isinstance(redaction, dict) or not redaction.get("passed"):
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
    """Each suite is one or more cargo commands; each command has ≤1 libtest FILTER."""
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
            ]
        ],
        # One FILTER per invocation; aggregate below.
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
                "live_convert_worker_fault_injection_rolls_back_and_retries_promotion",
            ],
        ],
    }


def suite_commands_flat() -> dict[str, list[str]]:
    """Compatibility: first command only (tests use suite_specs)."""
    return {key: cmds[0] for key, cmds in suite_specs().items()}


def base_not_run_report(*, git_short: str, git_full: str, raw_dir: Path) -> dict[str, Any]:
    f02 = load_f02_boot()
    return {
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
            "dockerVersion": None,
            "composeVersion": None,
            "composeProject": compose_project(),
            "migrationManifestSha256": migration_manifest_sha256(),
            "indexSignature": None,
            "imageIds": {},
            "imageDigests": {},
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": str(raw_dir),
        "blockers": ["MARKHAND_E2E!=1"],
        "notes": (
            "Harness complete; live release suite not opted in. "
            "Set MARKHAND_E2E=1 with POC PG/MinIO/Qdrant + built fileconv + "
            "F02 poc-f02-boot.json passed=true (with composeProject/imageIds), then re-run. "
            "Suites are in-process workers against service endpoints — not Compose API HTTP."
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


def scan_redaction(raw_dir: Path) -> dict[str, Any]:
    findings: list[str] = []
    secretish = re.compile(
        r"(?i)(password\s*[:=]\s*\S+|Bearer\s+[A-Za-z0-9._\-]{12,}|"
        r"postgres(?:ql)?://[^:\s]+:[^@\s]+@|"
        r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,})"
    )
    if raw_dir.is_dir():
        for path in sorted(raw_dir.rglob("*")):
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            if secretish.search(text):
                findings.append(f"residual_secret_pattern:{path.name}")
    return {"passed": not findings, "findings": findings}


def run_cargo(args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    validate_cargo_command_shape(args)
    return subprocess.run(args, cwd=ROOT, capture_output=True, text=True, env=env)


def aggregate_suite_runs(
    commands: list[list[str]], logs: list[str], exit_codes: list[int]
) -> dict[str, Any]:
    tests_run = tests_passed = tests_failed = 0
    skipped = False
    ignored = False
    formats: set[str] = set()
    for log, code in zip(logs, exit_codes):
        parsed = parse_cargo_result(log)
        tests_run += parsed["testsRun"]
        tests_passed += parsed["testsPassed"]
        tests_failed += parsed["testsFailed"]
        skipped = skipped or parsed["skipped"]
        ignored = ignored or (parsed["hasIgnoredLine"] and parsed["testsRun"] == 0)
        formats.update(parsed["formatsObserved"])
        if code != 0:
            tests_failed = max(tests_failed, 1)
    exit_code = 0 if all(code == 0 for code in exit_codes) else 1
    passed = (
        exit_code == 0
        and not skipped
        and not ignored
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
        "skipped": skipped,
        "ignored": ignored,
        "passed": passed,
        "formatsObserved": sorted(formats),
    }


def self_test() -> None:
    raw = Path("/tmp/o04-self-test-raw")
    raw.mkdir(parents=True, exist_ok=True)
    (raw / "ok.txt").write_text("ok\n", encoding="utf-8")

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
    good = {
        "issue": ISSUE,
        "status": "pass",
        "markhandE2e": True,
        "expectedFormats": list(formats),
        "formatsObserved": list(formats),
        "architecture": architecture_block(),
        "f02Boot": {
            "path": str(F02_BOOT),
            "passed": True,
            "composeProject": DEFAULT_COMPOSE_PROJECT,
            "imageIds": dict(image_ids),
        },
        "suites": {k: dict(good_suite) for k in REQUIRED_SUITES},
        "findings": [],
        "provenance": {
            "gitSha": "abc1234",
            "gitShaFull": "abc1234deadbeef",
            "dockerVersion": "Docker version 29",
            "composeVersion": "Docker Compose version 2",
            "composeProject": DEFAULT_COMPOSE_PROJECT,
            "migrationManifestSha256": "a" * 64,
            "indexSignature": "b" * 64,
            "imageIds": dict(image_ids),
            "imageDigests": {"postgres": "postgres@sha256:" + ("c" * 64)},
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": str(raw),
        "blockers": [],
    }
    # Patch suite commands to valid shapes from suite_specs.
    for key, cmds in suite_specs().items():
        good["suites"][key]["commands"] = cmds
        good["suites"][key]["command"] = cmds[0]

    status, blockers = evaluate_report(good)
    assert status == "pass" and not blockers, (status, blockers)

    missing = dict(good)
    del missing["suites"]
    status, blockers = evaluate_report(missing)
    assert status != "pass" and any(b.startswith("missing:") for b in blockers), blockers

    skipped = json.loads(json.dumps(good))
    skipped["suites"]["vertical_slice_formats"]["skipped"] = True
    skipped["suites"]["vertical_slice_formats"]["passed"] = False
    status, blockers = evaluate_report(skipped)
    assert status != "pass" and "skipped:vertical_slice_formats" in blockers, blockers

    ignored = json.loads(json.dumps(good))
    ignored["suites"]["worker_kill_replay"]["ignored"] = True
    ignored["suites"]["worker_kill_replay"]["passed"] = False
    status, blockers = evaluate_report(ignored)
    assert status != "pass" and "ignored:worker_kill_replay" in blockers, blockers

    zero = json.loads(json.dumps(good))
    zero["suites"]["adversarial_upload"]["testsRun"] = 0
    zero["suites"]["adversarial_upload"]["passed"] = False
    status, blockers = evaluate_report(zero)
    assert status != "pass" and "zero_test:adversarial_upload" in blockers, blockers

    partial = json.loads(json.dumps(good))
    partial["formatsObserved"] = ["pdf", "txt"]
    status, blockers = evaluate_report(partial)
    assert status != "pass" and "partial_format" in blockers, blockers

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
        "imageIds": {},
    }
    status, blockers = evaluate_report(no_f02_images)
    assert status != "pass" and "f02_missing_image_ids" in blockers, blockers

    # Invalid index signature => non-pass.
    bad_sig = json.loads(json.dumps(good))
    bad_sig["provenance"]["indexSignature"] = "not-a-hex"
    status, blockers = evaluate_report(bad_sig)
    assert status != "pass" and "provenance_missing:indexSignature" in blockers, blockers

    # False API HTTP claim => non-pass.
    false_http = json.loads(json.dumps(good))
    false_http["architecture"]["apiHttpExercised"] = True
    status, blockers = evaluate_report(false_http)
    assert status != "pass" and "architecture_false_api_http_claim" in blockers, blockers

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

    print("self-test ok")


def run_live(raw_dir: Path) -> dict[str, Any]:
    git_short = git_output("rev-parse", "--short", "HEAD")
    git_full = git_output("rev-parse", "HEAD")
    raw_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MARKHAND_E2E"] = "1"
    project = compose_project()
    image_ids, image_digests, missing = collect_poc_image_metadata(project)
    index_sig = resolve_index_signature()
    f02 = load_f02_boot()

    report: dict[str, Any] = {
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
            "dockerVersion": cmd_text(["docker", "--version"]),
            "composeVersion": cmd_text(["docker", "compose", "version"])
            or cmd_text(["docker-compose", "version"]),
            "composeProject": project,
            "migrationManifestSha256": migration_manifest_sha256(),
            "indexSignature": index_sig,
            "imageIds": image_ids,
            "imageDigests": image_digests,
            "missingPocServices": missing,
        },
        "redactionScan": {"passed": False, "findings": []},
        "rawDir": str(raw_dir),
        "blockers": [],
        "notes": "Live O04 release suite (in-process workers against POC service endpoints).",
        "commands": {k: v for k, v in suite_specs().items()},
    }

    observed: set[str] = set()
    for key, commands in suite_specs().items():
        logs: list[str] = []
        codes: list[int] = []
        for idx, command in enumerate(commands):
            proc = run_cargo(command, env)
            log = (proc.stdout or "") + "\n" + (proc.stderr or "")
            write_raw(raw_dir, f"{key}.{idx}.txt", log)
            logs.append(log)
            codes.append(proc.returncode)
        suite = aggregate_suite_runs(commands, logs, codes)
        suite["rawLog"] = f"{key}.*.txt"
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
    report["redactionScan"] = scan_redaction(raw_dir)
    if not report["redactionScan"]["passed"]:
        report["findings"].append(
            {
                "severity": "critical",
                "id": "redaction_residual",
                "details": report["redactionScan"]["findings"],
            }
        )

    status, blockers = evaluate_report(report)
    report["status"] = status
    report["blockers"] = blockers
    if status != "pass":
        report["notes"] = (
            "Live run did not meet O04 pass gates; see blockers. "
            "Architecture remains in-process workers against service endpoints."
        )
    else:
        report["notes"] = "All required O04 suites passed with complete format matrix."
    return report


def validate_report_cli(path: Path) -> int:
    report = json.loads(path.read_text(encoding="utf-8"))
    status, blockers = evaluate_report(report, raw_must_exist=True)
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
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.validate_report is not None:
        return validate_report_cli(args.validate_report.resolve())

    git_short = git_output("rev-parse", "--short", "HEAD")
    git_full = git_output("rev-parse", "HEAD")
    raw_dir = args.raw_dir or (OUT / "raw" / f"o04-{git_short}")
    raw_dir = raw_dir.resolve()
    raw_dir.mkdir(parents=True, exist_ok=True)

    if os.environ.get("MARKHAND_E2E") != "1":
        report = base_not_run_report(git_short=git_short, git_full=git_full, raw_dir=raw_dir)
        write_raw(raw_dir, "harness-not-run.txt", "MARKHAND_E2E!=1; evidence template only\n")
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
