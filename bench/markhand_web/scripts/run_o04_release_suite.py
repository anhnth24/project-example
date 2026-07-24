#!/usr/bin/env python3
"""P1B-O04 vertical-slice / security release suite harness.

Machine-verifiable, redacted evidence under:
  bench/markhand_web/reports/phase-1b-gate/o04-release.{json,md}
  bench/markhand_web/reports/phase-1b-gate/raw/o04-<git>/

Never writes or overwrites O05 ``summary.json``.
Default status is honest ``not_run``. ``pass`` only when MARKHAND_E2E=1,
every required suite exits 0 with testsRun>0, format matrix is complete,
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
ISSUE = "P1B-O04"

# Explicit POC document formats with in-process fixtures (no OCR/audio models).
EXPECTED_FORMATS = ["csv", "docx", "html", "pdf", "pptx", "txt", "xlsx"]

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
]

REQUIRED_PROVENANCE = [
    "gitSha",
    "gitShaFull",
    "dockerVersion",
    "composeVersion",
    "migrationManifestSha256",
    "indexSignature",
    "imageIds",
    "imageDigests",
]

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
    (re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"), "[REDACTED_JWT]"),
]


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


def index_signature_or_none() -> str | None:
    script = ROOT / "deploy/scripts/print-index-signature.py"
    if not script.is_file():
        return None
    proc = subprocess.run(
        [sys.executable, str(script)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        return None
    sig = proc.stdout.strip()
    return sig or None


def collect_image_metadata() -> tuple[dict[str, str], dict[str, str]]:
    ids: dict[str, str] = {}
    digests: dict[str, str] = {}
    compose = shutil.which("docker")
    if not compose:
        return ids, digests
    # Prefer compose poc project labels when present; never fail closed on missing stack.
    proc = subprocess.run(
        [
            "docker",
            "ps",
            "-a",
            "--filter",
            "name=markhand",
            "--format",
            "{{.Names}}\t{{.Image}}\t{{.ID}}",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0 or not proc.stdout.strip():
        return ids, digests
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        name, image, cid = parts[0], parts[1], parts[2]
        ids[name] = cid
        insp = subprocess.run(
            ["docker", "inspect", "--format", "{{json .RepoDigests}} {{.Id}}", cid],
            capture_output=True,
            text=True,
            check=False,
        )
        if insp.returncode == 0:
            digests[name] = insp.stdout.strip()
        else:
            digests[name] = image
    return ids, digests


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
    tests_run = passed + failed_n
    return {
        "testsRun": tests_run,
        "testsPassed": passed,
        "testsFailed": failed_n,
        "ignoredCount": ignored_n,
        "skipped": skipped,
        "formatsObserved": sorted(set(formats)),
        "hasIgnoredLine": ignored_lines > 0,
    }


def evaluate_report(report: dict[str, Any], *, raw_must_exist: bool = True) -> tuple[str, list[str]]:
    """Return (status, blockers). Only ``pass`` when every acceptance gate holds."""
    blockers: list[str] = []

    for key in REQUIRED_TOP_LEVEL:
        if key not in report:
            blockers.append(f"missing:{key}")

    if report.get("issue") != ISSUE:
        blockers.append("issue_mismatch")

    if not report.get("markhandE2e"):
        # Honest default path — never promote to pass.
        extra = list(blockers)
        if "MARKHAND_E2E!=1" not in extra:
            extra.append("MARKHAND_E2E!=1")
        return "not_run", extra

    expected = sorted(report.get("expectedFormats") or [])
    if expected != EXPECTED_FORMATS:
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

    raw_dir = report.get("rawDir")
    if not raw_dir:
        blockers.append("raw_dir_missing")
    elif raw_must_exist and not Path(str(raw_dir)).is_dir():
        blockers.append("raw_dir_missing")

    redaction = report.get("redactionScan") or {}
    if not isinstance(redaction, dict) or not redaction.get("passed"):
        blockers.append("redaction_failed")

    # Deduplicate while preserving order.
    seen: set[str] = set()
    uniq: list[str] = []
    for item in blockers:
        if item not in seen:
            seen.add(item)
            uniq.append(item)

    if uniq:
        return "fail", uniq
    return "pass", []


def base_not_run_report(*, git_short: str, git_full: str, raw_dir: Path) -> dict[str, Any]:
    return {
        "issue": ISSUE,
        "status": "not_run",
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "markhandE2e": False,
        "expectedFormats": list(EXPECTED_FORMATS),
        "formatsObserved": [],
        "suites": {
            key: {
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
            "Set MARKHAND_E2E=1 with POC DB/MinIO/Qdrant + built fileconv, then re-run."
        ),
    }


def write_reports(report: dict[str, Any]) -> None:
    if O05_SUMMARY.resolve() == (OUT / "o04-release.json").resolve():
        raise RuntimeError("refusing to treat O05 summary.json as O04 evidence")
    OUT.mkdir(parents=True, exist_ok=True)
    # Hard guard: never overwrite O05 soak summary.
    if OUT / "summary.json" == Path(report.get("rawDir") or ""):
        raise RuntimeError("refusing to write O04 evidence into O05 summary path")
    (OUT / "o04-release.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    md = [
        "# P1B-O04 vertical-slice / security release suite",
        "",
        f"- Status: `{report['status']}`",
        f"- Issue: `{report['issue']}`",
        f"- MARKHAND_E2E: `{report['markhandE2e']}`",
        f"- Expected formats: `{', '.join(report.get('expectedFormats') or [])}`",
        f"- Formats observed: `{', '.join(report.get('formatsObserved') or []) or '(none)'}`",
        f"- Git: `{((report.get('provenance') or {}).get('gitShaFull'))}`",
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
    return subprocess.run(args, cwd=ROOT, capture_output=True, text=True, env=env)


def suite_commands() -> dict[str, list[str]]:
    return {
        "vertical_slice_formats": [
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
        ],
        "unauthorized_cross_tenant": [
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
        ],
        "suspend_membership_delete_deny": [
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
        ],
        "adversarial_upload": [
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
            "zip_bomb_rejects_without_unbounded_decompress",
            "mime_mismatch_and_malformed_audio_reject",
            "formula_csv_and_prompt_html_quarantine",
            "corrupt_and_page_bomb_pdf_reject",
        ],
        "worker_kill_replay": [
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
            "live_convert_worker_fault_injection_rolls_back_and_retries_promotion",
        ],
    }


def self_test() -> None:
    raw = Path("/tmp/o04-self-test-raw")
    raw.mkdir(parents=True, exist_ok=True)
    (raw / "ok.txt").write_text("ok\n", encoding="utf-8")

    good_suite = {
        "command": ["cargo", "test"],
        "exitCode": 0,
        "testsRun": 2,
        "testsPassed": 2,
        "testsFailed": 0,
        "skipped": False,
        "ignored": False,
        "passed": True,
        "rawLog": "suite.txt",
    }
    good = {
        "issue": ISSUE,
        "status": "pass",
        "markhandE2e": True,
        "expectedFormats": list(EXPECTED_FORMATS),
        "formatsObserved": list(EXPECTED_FORMATS),
        "suites": {k: dict(good_suite) for k in REQUIRED_SUITES},
        "findings": [],
        "provenance": {
            "gitSha": "abc1234",
            "gitShaFull": "abc1234deadbeef",
            "dockerVersion": "Docker version 29",
            "composeVersion": "Docker Compose version 2",
            "migrationManifestSha256": "a" * 64,
            "indexSignature": "idx-sig",
            "imageIds": {"api": "sha256:1"},
            "imageDigests": {"api": "repo@sha256:2"},
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": str(raw),
        "blockers": [],
    }
    status, blockers = evaluate_report(good)
    assert status == "pass" and not blockers, (status, blockers)

    # missing top-level => non-pass
    missing = dict(good)
    del missing["suites"]
    status, blockers = evaluate_report(missing)
    assert status != "pass" and any(b.startswith("missing:") for b in blockers), blockers

    # skipped suite => non-pass
    skipped = json.loads(json.dumps(good))
    skipped["suites"]["vertical_slice_formats"]["skipped"] = True
    skipped["suites"]["vertical_slice_formats"]["passed"] = False
    status, blockers = evaluate_report(skipped)
    assert status != "pass" and "skipped:vertical_slice_formats" in blockers, blockers

    # ignored suite => non-pass
    ignored = json.loads(json.dumps(good))
    ignored["suites"]["worker_kill_replay"]["ignored"] = True
    ignored["suites"]["worker_kill_replay"]["passed"] = False
    status, blockers = evaluate_report(ignored)
    assert status != "pass" and "ignored:worker_kill_replay" in blockers, blockers

    # zero-test => non-pass
    zero = json.loads(json.dumps(good))
    zero["suites"]["adversarial_upload"]["testsRun"] = 0
    zero["suites"]["adversarial_upload"]["passed"] = False
    status, blockers = evaluate_report(zero)
    assert status != "pass" and "zero_test:adversarial_upload" in blockers, blockers

    # partial format => non-pass
    partial = json.loads(json.dumps(good))
    partial["formatsObserved"] = ["pdf", "txt"]
    status, blockers = evaluate_report(partial)
    assert status != "pass" and "partial_format" in blockers, blockers

    # high/critical finding => non-pass
    high = json.loads(json.dumps(good))
    high["findings"] = [{"severity": "critical", "id": "x"}]
    status, blockers = evaluate_report(high)
    assert status != "pass" and "finding:critical" in blockers, blockers

    # default not_run when e2e unset
    not_run = base_not_run_report(git_short="deadbee", git_full="deadbeef", raw_dir=raw)
    status, blockers = evaluate_report(not_run, raw_must_exist=True)
    assert status == "not_run", status
    assert "MARKHAND_E2E!=1" in blockers

    # cargo parse: soft skip + coverage line
    sample = (
        "skipped: MARKHAND_TEST_QDRANT_URL unset\n"
        "test live_upload_convert_index_citation_vertical_slice ... ok\n"
        "O04_FORMAT_COVERAGE\t[\"pdf\",\"txt\"]\n"
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
    )
    parsed = parse_cargo_result(sample)
    assert parsed["skipped"] is True
    assert parsed["formatsObserved"] == ["pdf", "txt"]
    assert parsed["testsRun"] == 1

    print("self-test ok")


def run_live(raw_dir: Path) -> dict[str, Any]:
    git_short = git_output("rev-parse", "--short", "HEAD")
    git_full = git_output("rev-parse", "HEAD")
    raw_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MARKHAND_E2E"] = "1"

    image_ids, image_digests = collect_image_metadata()
    report: dict[str, Any] = {
        "issue": ISSUE,
        "status": "fail",
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "markhandE2e": True,
        "expectedFormats": list(EXPECTED_FORMATS),
        "formatsObserved": [],
        "suites": {},
        "findings": [],
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "dockerVersion": cmd_text(["docker", "--version"]),
            "composeVersion": cmd_text(["docker", "compose", "version"])
            or cmd_text(["docker-compose", "version"]),
            "migrationManifestSha256": migration_manifest_sha256(),
            "indexSignature": index_signature_or_none(),
            "imageIds": image_ids,
            "imageDigests": image_digests,
        },
        "redactionScan": {"passed": False, "findings": []},
        "rawDir": str(raw_dir),
        "blockers": [],
        "notes": "Live O04 release suite execution.",
        "commands": suite_commands(),
    }

    observed: set[str] = set()
    for key, command in suite_commands().items():
        proc = run_cargo(command, env)
        log = (proc.stdout or "") + "\n" + (proc.stderr or "")
        log_name = f"{key}.txt"
        write_raw(raw_dir, log_name, log)
        parsed = parse_cargo_result(log)
        # Soft-skip under MARKHAND_E2E is a hard failure for release evidence.
        skipped = parsed["skipped"]
        ignored = parsed["hasIgnoredLine"] and parsed["testsRun"] == 0
        passed = (
            proc.returncode == 0
            and not skipped
            and not ignored
            and parsed["testsRun"] > 0
            and parsed["testsFailed"] == 0
        )
        report["suites"][key] = {
            "command": command,
            "exitCode": proc.returncode,
            "testsRun": parsed["testsRun"],
            "testsPassed": parsed["testsPassed"],
            "testsFailed": parsed["testsFailed"],
            "skipped": skipped,
            "ignored": ignored,
            "passed": passed,
            "rawLog": log_name,
            "formatsObserved": parsed["formatsObserved"],
        }
        observed.update(parsed["formatsObserved"])
        if not passed:
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
        report["notes"] = "Live run did not meet O04 pass gates; see blockers."
    else:
        report["notes"] = "All required O04 suites passed with complete format matrix."
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description="P1B-O04 release suite harness")
    parser.add_argument("--self-test", action="store_true")
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
