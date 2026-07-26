import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("run_o04_release_suite.py")
SPEC = importlib.util.spec_from_file_location("run_o04_release_suite", SCRIPT)
o04 = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(o04)


def cargo_log(names, *, formats=None, ignored=0):
    names = [names] if isinstance(names, str) else list(names)
    coverage = f"O04_FORMAT_COVERAGE\t{json.dumps(formats)}\n" if formats else ""
    test_lines = "".join(f"test {name} ... {'ignored' if ignored else 'ok'}\n" for name in names)
    passed = 0 if ignored else len(names)
    return (
        f"{test_lines}"
        f"{coverage}"
        f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; "
        "0 measured; 0 filtered out\n"
        "O04_COMMAND_EXIT_CODE\t0\n"
        "O04_COMMAND_TIMED_OUT\tfalse\n"
        "O04_COMMAND_OUTPUT_TRUNCATED\tfalse\n"
        "O04_COMMAND_EOF\ttrue\n"
    )


class O04ReleaseSuiteHardeningTest(unittest.TestCase):
    def setUp(self):
        o04.RAW_ROOT.mkdir(parents=True, exist_ok=True)

    def make_report(self, raw):
        formats = o04.load_expected_formats()
        image_ids = {svc: f"sha256:{i:064d}" for i, svc in enumerate(o04.EXPECTED_POC_SERVICES)}
        container_ids = {svc: f"cid-{svc}" for svc in o04.EXPECTED_POC_SERVICES}
        artifacts = []
        suites = {}
        for suite_key, commands in o04.suite_specs().items():
            raw_logs = []
            for idx, command in enumerate(commands):
                suite_formats = formats if suite_key == "vertical_slice_formats" and idx == 0 else None
                path = o04.write_raw(
                    raw,
                    f"{suite_key}.{idx}.txt",
                    cargo_log(
                        o04.EXPECTED_SUITE_TESTS[suite_key],
                        formats=suite_formats,
                    ),
                )
                artifacts.append(path)
                raw_logs.append(path.relative_to(raw).as_posix())
            suites[suite_key] = {
                "commands": commands,
                "command": commands[0],
                "exitCode": 0,
                "testsRun": len(commands),
                "testsPassed": len(commands),
                "testsFailed": 0,
                "skipped": False,
                "ignored": False,
                "passed": True,
                "rawLogs": raw_logs,
                "rawLog": raw_logs[0],
            }
        o04.write_raw_manifest(raw, artifacts)
        return {
            "issue": o04.ISSUE,
            "schemaVersion": o04.SCHEMA_VERSION,
            "status": "pass",
            "markhandE2e": True,
            "expectedFormats": formats,
            "formatsObserved": formats,
            "architecture": o04.architecture_block(),
            "f02Boot": {
                "passed": True,
                "composeProject": o04.DEFAULT_COMPOSE_PROJECT,
                "reportSha256": "f" * 64,
                "manifestSha256": "1" * 64,
                "composeFileSha256": "d" * 64,
                "effectiveComposeSha256": "e" * 64,
                "containerIds": container_ids,
                "imageIds": image_ids,
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
            "suites": suites,
            "findings": [],
            "provenance": {
                "gitSha": "abc1234",
                "gitShaFull": "abc1234deadbeef",
                "gitDirty": False,
                "dockerVersion": "Docker",
                "composeVersion": "Compose",
                "composeProject": o04.DEFAULT_COMPOSE_PROJECT,
                "migrationManifestSha256": "a" * 64,
                "composeFileSha256": "d" * 64,
                "effectiveComposeSha256": "e" * 64,
                "f02ReportSha256": "f" * 64,
                "f02ManifestSha256": "1" * 64,
                "indexSignature": "b" * 64,
                "containerIds": container_ids,
                "imageIds": image_ids,
                "imageDigests": {},
                "composeServiceMap": {
                    svc: {
                        "containerId": container_ids[svc],
                        "imageId": image_ids[svc],
                        "health": "healthy",
                        "running": True,
                        "ports": {},
                        "labels": {
                            "com.docker.compose.project": o04.DEFAULT_COMPOSE_PROJECT,
                            "com.docker.compose.service": svc,
                        },
                    }
                    for svc in o04.EXPECTED_POC_SERVICES
                },
                "apiEndpoint": None,
                "testEndpoints": {
                    "database": "postgres://127.0.0.1:5432/db",
                    "appDatabase": "postgres://127.0.0.1:5432/db",
                    "minio": "http://127.0.0.1:9000",
                    "qdrant": "http://127.0.0.1:6333",
                },
            },
            "redactionScan": {"passed": True, "findings": []},
            "rawDir": o04.repo_rel(raw),
            "blockers": [],
        }

    def test_ignored_raw_cargo_output_blocks_claimed_pass(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            rel = report["suites"]["worker_kill_replay"]["rawLogs"][0]
            (raw / rel).write_text(cargo_log("worker_kill_replay", ignored=1), encoding="utf-8")
            o04.write_raw_manifest(raw, list(raw.glob("*.txt")))

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("ignored:worker_kill_replay", blockers)

    def test_modified_raw_log_hash_blocks_claimed_pass(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            rel = report["suites"]["adversarial_upload"]["rawLogs"][0]
            (raw / rel).write_text((raw / rel).read_text(encoding="utf-8") + "tamper\n", encoding="utf-8")

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertTrue(any(item.startswith("raw_manifest_sha_mismatch") for item in blockers))

    def test_stdout_hard_kill_marker_does_not_satisfy_external_kill_proof(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            rel = report["suites"]["worker_kill_replay"]["rawLogs"][0]
            report["externalWorkerKill"]["deathVerified"] = False
            (raw / rel).write_text(
                cargo_log(o04.EXPECTED_SUITE_TESTS["worker_kill_replay"])
                + "O04_WORKER_HARD_KILL_EVIDENCE\tpid=123 lease_expired=true replay_consistent=true\n",
                encoding="utf-8",
            )
            o04.write_raw_manifest(raw, list(raw.glob("*.txt")))

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("external_worker_kill:deathVerified", blockers)

    def test_git_binding_rejects_stale_or_dirty_live_report(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            report = self.make_report(Path(tmp))

            status, blockers = o04.evaluate_report(
                report,
                bind_current_git=True,
                current_git_full="different",
                current_git_dirty=True,
            )

            self.assertNotEqual(status, "pass")
            self.assertIn("git_sha_mismatch", blockers)
            self.assertIn("git_dirty", blockers)

    def test_canonical_report_redaction_scan_blocks_url_userinfo(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            report["notes"] = "leak http://user:password@example.invalid/path"
            report_path = raw / "report.json"
            report_path.write_text(json.dumps(report), encoding="utf-8")

            status, blockers = o04.evaluate_report(report, report_path=report_path)

            self.assertNotEqual(status, "pass")
            self.assertIn("redaction_failed", blockers)

    def test_noncanonical_filter_and_forged_test_name_block_pass(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            report["suites"]["adversarial_upload"]["commands"] = [
                [
                    "cargo",
                    "test",
                    "-p",
                    "fileconv-server",
                    "--test",
                    "uploads",
                    "--",
                    "--nocapture",
                    "happy",
                ]
            ]
            rel = report["suites"]["adversarial_upload"]["rawLogs"][0]
            (raw / rel).write_text(cargo_log(["forged_everything_passed"]), encoding="utf-8")
            o04.write_raw_manifest(raw, list(raw.glob("*.txt")))

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("noncanonical_command:adversarial_upload", blockers)
            self.assertIn("test_names_mismatch:adversarial_upload", blockers)

    def test_missing_eof_and_truncated_output_block_pass(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            rel = report["suites"]["vertical_slice_formats"]["rawLogs"][0]
            log = cargo_log(o04.EXPECTED_SUITE_TESTS["vertical_slice_formats"], formats=o04.load_expected_formats())
            log = log.replace("O04_COMMAND_OUTPUT_TRUNCATED\tfalse\n", "O04_COMMAND_OUTPUT_TRUNCATED\ttrue\n")
            log = log.replace("O04_COMMAND_EOF\ttrue\n", "")
            (raw / rel).write_text(log, encoding="utf-8")
            o04.write_raw_manifest(raw, list(raw.glob("*.txt")))

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("raw_eof_missing:vertical_slice_formats", blockers)
            self.assertIn("truncated:vertical_slice_formats", blockers)

    def test_api_http_false_or_failed_probe_blocks_pass(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            report = self.make_report(Path(tmp))
            report["architecture"]["apiHttpExercised"] = False
            report["blackBoxApiProbes"]["probes"]["vertical_ask"]["passed"] = False

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("api_http_not_exercised", blockers)
            self.assertIn("api_probe_failed:vertical_ask", blockers)

    def test_release_gate_validates_evidence_generated_outside_source_tree(self):
        with tempfile.TemporaryDirectory(prefix="o04-out-") as out:
            out_dir = Path(out)
            raw = out_dir / "raw" / "o04-abc"
            raw.mkdir(parents=True)
            previous_out = o04.OUT
            o04.configure_output_dir(out_dir)
            try:
                report = self.make_report(raw)
                # A separate validation process only receives the report path.
                o04.configure_output_dir(
                    o04.validation_output_dir(out_dir / "o04-release.json")
                )
                status, blockers = o04.evaluate_report(report)

                escaped = dict(report, rawDir="../escape")
                escaped_status, escaped_blockers = o04.evaluate_report(escaped)
            finally:
                o04.configure_output_dir(previous_out)

            self.assertNotIn("raw_dir_outside_evidence_root", blockers)
            self.assertNotIn("raw_dir_missing", blockers)
            self.assertNotEqual(escaped_status, "pass")
            self.assertIn("raw_dir_not_repo_relative", escaped_blockers)

    def test_expanded_redaction_blocks_cookie_basic_and_cloud_keys(self):
        with tempfile.TemporaryDirectory(prefix="o04-test-", dir=o04.RAW_ROOT) as tmp:
            raw = Path(tmp)
            report = self.make_report(raw)
            leak = raw / "headers.txt"
            leak.write_text(
                "Set-Cookie: sid=secret-session\n"
                "Authorization: Basic dXNlcjpwYXNzd29yZA==\n"
                "AWS_SECRET_ACCESS_KEY=cloud-secret\n"
                "PRIVATE_KEY=-----BEGIN\n",
                encoding="utf-8",
            )
            o04.write_raw_manifest(raw, list(raw.glob("*.txt")))

            status, blockers = o04.evaluate_report(report)

            self.assertNotEqual(status, "pass")
            self.assertIn("redaction_failed", blockers)


if __name__ == "__main__":
    unittest.main()
