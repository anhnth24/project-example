#!/usr/bin/env python3
"""Unit / hermetic tests for P1B-F02 boot evidence hardening."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "poc_f02_boot_evidence.py"
sys.path.insert(0, str(SCRIPT.parent))

import poc_f02_boot_evidence as f02  # noqa: E402


def _base_good_report() -> dict:
    services = f02.expected_services_for_profiles(["mock"])
    raw_dir = "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc1234"
    runtime = {
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
    }
    return {
        "issue": "P1B-F02",
        "stamp_utc": "20260724T000000Z",
        "generatedAt": "2026-07-24T00:00:00+00:00",
        "passed": True,
        "pass_count": 12,
        "fail_count": 0,
        "passes": ["convert network Internal=true"],
        "fails": [],
        "notes": [],
        "composeProject": "markhand-poc",
        "composeProfiles": ["mock"],
        "containerIds": {svc: f"{i:064x}" for i, svc in enumerate(services, start=1)},
        "imageIds": {svc: f"sha256:{i:064d}" for i, svc in enumerate(services)},
        "imageDigests": {
            "postgres": "postgres@sha256:" + ("a" * 64),
            "minio": "minio@sha256:" + ("b" * 64),
        },
        "composeLabels": {
            svc: {"service": svc, "project": "markhand-poc"} for svc in services
        },
        "gitSha": "a" * 7,
        "gitShaFull": "a" * 40,
        "dockerVersion": "24.0.0",
        "composeVersion": "2.24.0",
        "composeFileSha256": "c" * 64,
        "composeBlobSha256": "c" * 64,
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
            "exitCode": 28,
            "routeProbe": {
                "target": "1.1.1.1:443",
                "blocked": True,
                "classification": "route_blocked",
                "defaultRoutePresent": False,
            },
            "raw": "wget: download timed out",
            "network": "markhand-poc_convert",
            "probeImage": f02.DEFAULT_EGRESS_PROBE_IMAGE,
        },
        "resourceLimits": {
            svc: {
                "memory": 512 * 1024 * 1024,
                "nanoCpus": 1_000_000_000,
                "pidsLimit": 256,
            }
            for svc in f02.LIMIT_SERVICES
        },
        "rawDir": raw_dir,
        "rawArtifactManifest": {"path": f"{raw_dir}/manifest.json", "sha256": "a" * 64},
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
        "runtimeSecurity": runtime,
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
            "contentAssertions": {fmt: True for fmt in f02.REQUIRED_NATIVE_FORMATS},
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


class PocF02EvidenceTests(unittest.TestCase):
    def test_sanitize_inspect_drops_env_secrets(self) -> None:
        raw = [
            {
                "Id": "sha256:abc",
                "Name": "/markhand-poc-api-1",
                "Config": {
                    "User": "10001:10001",
                    "Image": "markhand-api:poc",
                    "Env": [
                        "MARKHAND_AUTH_SIGNING_KEY=super-secret",
                        "POSTGRES_PASSWORD=hunter2",
                        "MINIO_ROOT_PASSWORD=minioadmin",
                    ],
                },
                "HostConfig": {
                    "ReadonlyRootfs": True,
                    "SecurityOpt": ["no-new-privileges:true"],
                    "CapDrop": ["ALL"],
                    "Memory": 536870912,
                    "NanoCpus": 1000000000,
                    "PidsLimit": 256,
                },
                "NetworkSettings": {
                    "Networks": {"markhand-poc_private": {"IPAddress": "10.0.0.2"}}
                },
                "State": {
                    "Status": "running",
                    "Running": True,
                    "ExitCode": 0,
                    "Health": {"Status": "healthy"},
                },
                "Image": "sha256:" + ("d" * 64),
            }
        ]
        cleaned = f02.sanitize_inspect(raw)
        blob = json.dumps(cleaned)
        self.assertNotIn("Env", blob)
        self.assertNotIn("super-secret", blob)
        self.assertNotIn("hunter2", blob)
        self.assertNotIn("minioadmin", blob)
        self.assertEqual(cleaned[0]["Config"]["User"], "10001:10001")
        self.assertEqual(cleaned[0]["HostConfig"]["Memory"], 536870912)
        self.assertEqual(cleaned[0]["Image"], "sha256:" + ("d" * 64))

    def test_secret_bearing_inspect_rejected_by_scan(self) -> None:
        text = json.dumps(
            {
                "Config": {
                    "Env": [
                        "MARKHAND_AUTH_SIGNING_KEY=abcdef0123456789",
                        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaa.bbb",
                    ]
                }
            }
        )
        findings = f02.scan_committed_text(text)
        self.assertTrue(findings)
        report = _base_good_report()
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp)
            (raw / "inspect-api.json").write_text(text, encoding="utf-8")
            report["rawDir"] = str(raw)
            status, blockers = f02.evaluate_report(report, raw_root=raw)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any("secret" in b or "redaction" in b for b in blockers), blockers
        )

    def test_missing_service_image_metadata_rejected(self) -> None:
        report = _base_good_report()
        report["imageIds"] = {"api": report["imageIds"]["api"]}
        report["containerIds"] = {"api": report["containerIds"]["api"]}
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any(
                "missing" in b and ("image" in b or "container" in b or "service" in b)
                for b in blockers
            ),
            blockers,
        )

    def test_missing_egress_execution_rejected(self) -> None:
        report = _base_good_report()
        report["egressProbe"] = {
            "executed": False,
            "toolMissing": True,
            "blocked": None,
            "raw": "curl absent — expected lean image",
        }
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("egress_not_executed", blockers)

    def test_resource_limit_zero_rejected(self) -> None:
        report = _base_good_report()
        report["resourceLimits"]["api"]["memory"] = 0
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("resource_limit" in b for b in blockers), blockers)

    def test_nolimit_compose_cannot_pass(self) -> None:
        report = _base_good_report()
        report["nolimitComposeUsed"] = True
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("nolimit_compose", blockers)

    def test_vfs_storage_without_limits_rejected(self) -> None:
        report = _base_good_report()
        report["storageDriver"] = "vfs"
        report["resourceLimits"]["worker-convert"]["pidsLimit"] = 0
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")

    def test_complete_fixture_accepted(self) -> None:
        report = _base_good_report()
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertEqual(status, "pass", blockers)
        self.assertEqual(blockers, [])

    def test_fabricated_release_report_without_manifest_rejected(self) -> None:
        report = _base_good_report()
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any(
                blocker in blockers
                for blocker in (
                    "raw_manifest_file_missing",
                    "raw_dir_missing",
                    "stale_target_git_sha",
                )
            ),
            blockers,
        )

    def test_runtime_security_rejects_added_cap_and_external_convert_network(
        self,
    ) -> None:
        report = _base_good_report()
        report["runtimeSecurity"]["worker-convert"]["capAdd"] = ["NET_ADMIN"]
        report["runtimeSecurity"]["worker-convert"]["networks"].append(
            "markhand-poc_edge"
        )
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("runtime_security_cap_add:worker-convert", blockers)
        self.assertTrue(
            any(
                blocker.startswith("runtime_security_network_extra:worker-convert")
                for blocker in blockers
            ),
            blockers,
        )

    def test_convert_seccomp_unconfined_is_rejected(self) -> None:
        report = _base_good_report()
        report["runtimeSecurity"]["worker-convert"]["securityOpt"] = [
            "no-new-privileges:true",
            "seccomp=unconfined",
        ]
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("runtime_security_seccomp_unconfined:worker-convert", blockers)

    def test_egress_timeout_with_default_route_is_rejected(self) -> None:
        report = _base_good_report()
        report["egressProbe"]["exitCode"] = 20
        report["egressProbe"]["routeProbe"]["defaultRoutePresent"] = True
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("egress_default_route_present_or_unknown", blockers)

    def test_clean_boot_minio_and_qdrant_proofs_are_required(self) -> None:
        report = _base_good_report()
        report["bootEvidence"] = {"cleanBootMeasured": False}
        report["minioCredentialProbe"] = {
            "positiveListBucket": False,
            "negativeAdminDenied": False,
        }
        report["qdrantInit"] = {"exitCode": 1, "configVerified": False}
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("clean_boot_not_measured", blockers)
        self.assertIn("minio_positive_probe_failed", blockers)
        self.assertIn("minio_negative_probe_failed", blockers)
        self.assertIn("qdrant_init_not_successful", blockers)
        self.assertIn("qdrant_config_not_verified", blockers)

    def test_native_smoke_must_use_worker_sandbox_path(self) -> None:
        report = _base_good_report()
        report["nativeSmoke"] = {"productionWorkerSandboxPath": False}
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("native_smoke_not_worker_sandbox_path", blockers)

    def test_live_harness_uses_worker_sandbox_conversion_probe(self) -> None:
        harness = (SCRIPT.parent / "poc-boot-evidence.sh").read_text(encoding="utf-8")
        self.assertIn("/usr/local/bin/fileconv-worker --sandbox-convert-probe", harness)
        self.assertNotIn(
            '/usr/local/bin/fileconv one "/tmp/format-smoke/sample.$fmt"',
            harness,
        )
        self.assertIn(
            '["productionWorkerSandboxPath"] = all(assertions.values())',
            harness,
        )

    def test_native_smoke_requires_full_format_matrix(self) -> None:
        report = _base_good_report()
        del report["nativeSmoke"]["contentAssertions"]["docx"]
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("native_smoke_format_matrix_incomplete", blockers)

    def test_absolute_raw_path_rejected_for_release(self) -> None:
        report = _base_good_report()
        report["rawDir"] = str(Path("/tmp/f02-absolute").resolve())
        report["rawArtifactManifest"] = {
            "path": f"{report['rawDir']}/manifest.json",
            "sha256": "a" * 64,
        }
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertIn("raw_dir_not_repo_relative", blockers)

    def test_dirty_worktree_release_rejected(self) -> None:
        report = _base_good_report()
        report["gitWorktree"] = {
            "dirty": True,
            "porcelain": [" M deploy/compose.poc.yml"],
        }
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp)
            report["rawDir"] = "bench/markhand_web/reports/phase-1b-gate/raw/f02-test"
            (raw / "summary.txt").write_text("PASS: fixture\n", encoding="utf-8")
            manifest = {
                "schema": "markhand.p1b.f02.raw-manifest.v1",
                "issue": "P1B-F02",
                "rawDir": report["rawDir"],
                "targetGitShaFull": report["gitShaFull"],
                "dirtyWorktree": {
                    "dirty": True,
                    "porcelain": [" M deploy/compose.poc.yml"],
                },
                "composeFileSha256": report["composeFileSha256"],
                "composeBlobSha256": report["composeBlobSha256"],
                "containers": report["containerIds"],
                "imageIds": report["imageIds"],
                "files": {"summary.txt": f02.sha256_file(raw / "summary.txt")},
            }
            (raw / "manifest.json").write_text(
                json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
            )
            report["rawArtifactManifest"] = {
                "path": f"{report['rawDir']}/manifest.json",
                "sha256": f02.sha256_file(raw / "manifest.json"),
            }
            status, blockers = f02.evaluate_report(
                report,
                raw_root=raw,
                current_git_sha=report["gitShaFull"],
            )
        self.assertNotEqual(status, "pass")
        self.assertIn("dirty_worktree", blockers)

    def test_source_git_change_during_artifact_generation_rejected(self) -> None:
        report = _base_good_report()
        report["sourceGit"]["after"]["gitShaFull"] = "b" * 40
        report["sourceGit"]["headUnchanged"] = False
        report["sourceGit"]["after"]["porcelain"] = [
            "?? bench/markhand_web/reports/poc-f02-boot.json"
        ]
        report["sourceGit"]["porcelainUnchanged"] = False
        status, blockers = f02.evaluate_report(report, allow_fixture=True)
        self.assertNotEqual(status, "pass")
        self.assertIn("source_git_head_changed_or_unknown", blockers)
        self.assertIn("source_git_worktree_changed_or_unknown", blockers)

    def test_repo_relative_raw_path(self) -> None:
        root = Path("/workspace")
        inside = root / "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc"
        self.assertEqual(
            f02.repo_relative_raw_dir(inside, root),
            "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc",
        )
        outside = Path("/tmp/markhand-f02-evidence")
        self.assertEqual(
            f02.repo_relative_raw_dir(outside, root),
            str(outside.resolve()).replace("\\", "/"),
        )

    def test_nolimit_compose_temp_is_private_and_cleanup_removes_it(self) -> None:
        bash = shutil.which("bash")
        if os.name == "nt":
            program_files = Path(os.environ.get("ProgramFiles", r"C:\Program Files"))
            git_bash = program_files / "Git" / "bin" / "bash.exe"
            if git_bash.is_file():
                bash = str(git_bash)
        if not bash:
            self.skipTest("bash unavailable")
        probe = subprocess.run(
            [bash, "-lc", "echo ok"], capture_output=True, text=True, check=False
        )
        if probe.returncode != 0:
            self.skipTest("bash unavailable or not configured")
        script = Path(__file__).resolve().parent / "poc-compose.sh"
        with tempfile.TemporaryDirectory() as tmp:
            tmp_posix = Path(tmp).as_posix()
            script_posix = script.as_posix()
            command = f"""
set -euo pipefail
TMPDIR={tmp_posix!r}
ENV_FILE=/dev/null
COMPOSE_FILE=/dev/null
function docker() {{
  cat <<'YAML'
services:
  api:
    image: test
    mem_limit: 512m
    cpus: 1.0
    pids_limit: 128
YAML
}}
source {script_posix!r}
poc_write_nolimit_compose >/dev/null
out="$POC_COMPOSE_EFFECTIVE"
mode="$("$POC_PYTHON_BIN" - "$out" <<'PY'
import os, stat, sys
print(oct(stat.S_IMODE(os.stat(sys.argv[1]).st_mode)))
PY
)"
grep -q 'image: test' "$out"
! grep -q 'mem_limit' "$out"
! grep -q 'cpus:' "$out"
! grep -q 'pids_limit' "$out"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) ;;
  *) test "$mode" = "0o600" ;;
esac
POC_COMPOSE_EFFECTIVE="$out"
_poc_cleanup_nolimit_compose
test ! -e "$out"
"""
            proc = subprocess.run(
                [bash, "-lc", command], capture_output=True, text=True, check=False
            )
        self.assertEqual(proc.returncode, 0, proc.stderr + proc.stdout)

    def test_cli_self_test_passes(self) -> None:
        # Invoked via module main --self-test after implementation lands.
        rc = f02.run_self_test()
        self.assertEqual(rc, 0)

    def test_finalize_from_raw_publishes_non_pass_after_shell_failure(self) -> None:
        artifacts_root = f02.ROOT / ".artifacts" / "tests"
        artifacts_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix="f02-finalize-", dir=artifacts_root
        ) as tmp:
            raw = Path(tmp) / "raw"
            raw.mkdir()
            report = _base_good_report()
            (raw / "summary.txt").write_text(
                "PASS: command docker\nFAIL: injected shell failure\n", encoding="utf-8"
            )
            meta = {
                "storageDriver": report["storageDriver"],
                "composeProject": report["composeProject"],
                "composeProfiles": report["composeProfiles"],
                "containerIds": report["containerIds"],
                "imageIds": report["imageIds"],
                "imageDigests": report["imageDigests"],
                "composeLabels": report["composeLabels"],
                "resourceLimits": report["resourceLimits"],
                "runtimeSecurity": report["runtimeSecurity"],
                "bootEvidence": report["bootEvidence"],
                "nativeSmoke": report["nativeSmoke"],
                "minioCredentialProbe": report["minioCredentialProbe"],
                "qdrantInit": report["qdrantInit"],
                "egressProbe": report["egressProbe"],
                "sourceGit": report["sourceGit"],
                "composeBlobSha256": f02.sha256_file(f02.COMPOSE_FILE),
            }
            (raw / "meta.json").write_text(
                json.dumps(meta, indent=2) + "\n", encoding="utf-8"
            )
            json_path = Path(tmp) / "poc-f02-boot.json"
            md_path = Path(tmp) / "poc-f02-boot.md"

            payload = f02.finalize_from_raw(
                json_path=json_path,
                md_path=md_path,
                raw_dir=raw,
                stamp="20260724T000000Z",
                fail=1,
                compose_project=report["composeProject"],
                nolimit_compose_used=False,
            )

            self.assertFalse(payload["passed"])
            self.assertIn("injected shell failure", "\n".join(payload["fails"]))
            self.assertTrue(json_path.is_file())
            self.assertTrue(md_path.is_file())
            self.assertIn("shell_fail", payload["evaluationBlockers"])


if __name__ == "__main__":
    raise SystemExit(unittest.main())
