#!/usr/bin/env python3
"""Hermetic executable tests for web-e2e-real.sh process orchestration."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "web-e2e-real.sh"
REPO_ROOT = SCRIPT.resolve().parents[2]
ARTIFACTS = Path(__file__).resolve().parent / "web_e2e_real_artifacts.py"


def _write_executable(path: Path, content: str) -> None:
    path.write_text(textwrap.dedent(content).lstrip(), encoding="utf-8")
    path.chmod(0o755)


class OrchestrationHarness:
    def __init__(self) -> None:
        self.tempdir = Path(tempfile.mkdtemp(prefix="web-e2e-real-orch-"))
        self.root = self.tempdir / "repo"
        self.state = self.tempdir / "state"
        self.shim_bin = self.tempdir / "shims"
        self.path_bin = self.tempdir / "path"
        self.artifact_dir = self.tempdir / "artifacts"
        self.state.mkdir()
        self.shim_bin.mkdir()
        self.path_bin.mkdir()
        self.artifact_dir.mkdir()
        self._install_shims()
        self._install_env()

    def _install_env(self) -> None:
        env_dir = self.root / "deploy" / "dev"
        env_dir.mkdir(parents=True)
        (env_dir / ".env").write_text(
            "\n".join(
                [
                    "MARKHAND_BIND_ADDR=127.0.0.1:8787",
                    "MARKHAND_WORKER_DB_USER=markhand_worker",
                    "MARKHAND_WORKER_DB_PASSWORD=markhand_worker_dev_only",
                    "MARKHAND_POSTGRES_PORT=54329",
                    "MARKHAND_POSTGRES_DB=markhand",
                ]
            )
            + "\n",
            encoding="utf-8",
        )

    def _install_shims(self) -> None:
        state = self.state
        shim_bin = self.shim_bin
        root = self.root

        _write_executable(
            shim_bin / "cargo",
            f"""\
            #!/usr/bin/env bash
            set -euo pipefail
            bin_dir="{root}/target/debug"
            mkdir -p "$bin_dir"
            install -m 0755 "{shim_bin}/fileconv-server" "$bin_dir/fileconv-server"
            install -m 0755 "{shim_bin}/fileconv-worker" "$bin_dir/fileconv-worker"
            install -m 0755 "{shim_bin}/fileconv" "$bin_dir/fileconv"
            exit 0
            """,
        )

        _write_executable(
            shim_bin / "fileconv-server",
            f"""\
            #!/usr/bin/env bash
            set -euo pipefail
            echo "server:$$" >> "{state}/child.pids"
            echo "MARKHAND_AUTH_SIGNING_KEY=super-secret-key-at-least-32-bytes"
            echo "server-start pid=$$" >> "{state}/server.events"
            echo "MARKHAND_MAX_UPLOAD_BYTES=${{MARKHAND_MAX_UPLOAD_BYTES:-}}" >> "{state}/server.env"
            echo "MARKHAND_RATE_ROUTE_PER_MINUTE=${{MARKHAND_RATE_ROUTE_PER_MINUTE:-}}" >> "{state}/server.env"
            if [[ "${{WEB_E2E_REAL_SERVER_EXIT_EARLY:-}}" == "1" ]]; then
              echo "server-exit-early" >> "{state}/server.events"
              exit 1
            fi
            trap 'echo server-reaped:$$ >> "{state}/cleanup.reaped"; echo server-term >> "{state}/server.events"; exit 0' TERM INT
            while true; do sleep 0.05; done
            """,
        )

        _write_executable(
            shim_bin / "fileconv-worker",
            f"""\
            #!/usr/bin/env bash
            set -euo pipefail
            echo "worker-${{MARKHAND_WORKER_KIND}}:$$" >> "{state}/child.pids"
            echo "kind=${{MARKHAND_WORKER_KIND:-unset}}" >> "{state}/workers.started"
            echo "db=${{MARKHAND_WORKER_DATABASE_URL:-}}" >> "{state}/workers.env"
            if [[ -n "${{MARKHAND_CONVERTER_ARGV_JSON:-}}" ]]; then
              echo "converter=${{MARKHAND_CONVERTER_ARGV_JSON}}" >> "{state}/workers.env"
            fi
            delay="${{WEB_E2E_REAL_WORKER_DIE_DELAY:-}}"
            die_kind="${{WEB_E2E_REAL_WORKER_DIE_KIND:-convert}}"
            if [[ -n "$delay" && "$delay" != "999" && "${{MARKHAND_WORKER_KIND}}" == "$die_kind" ]]; then
              (
                while [[ ! -f "{state}/playwright.started" ]]; do sleep 0.05; done
                sleep "$delay"
                echo "worker-die-${{MARKHAND_WORKER_KIND}}" >> "{state}/workers.events"
                kill -TERM "$$" 2>/dev/null || exit 1
              ) &
            fi
            trap 'echo worker-reaped-${{MARKHAND_WORKER_KIND}}:$$ >> "{state}/cleanup.reaped"; echo worker-term-${{MARKHAND_WORKER_KIND}} >> "{state}/cleanup.signals"; exit 0' TERM INT
            while true; do sleep 0.05; done
            """,
        )

        _write_executable(
            shim_bin / "fileconv",
            "#!/usr/bin/env bash\nexit 0\n",
        )

        _write_executable(
            shim_bin / "curl",
            """\
            #!/usr/bin/env bash
            if [[ "${*: -1}" == */api/v1/health/ready ]]; then
              exit 0
            fi
            exit 0
            """,
        )

        _write_executable(
            shim_bin / "pnpm",
            f"""\
            #!/usr/bin/env bash
            set -euo pipefail
            if [[ "${{1:-}}" == "--version" || "${{1:-}}" == "-v" ]]; then
              echo "10.0.0"
              exit 0
            fi
            if [[ "$1" == "--dir" && "$3" == "build" ]]; then
              mkdir -p "{root}/web/dist"
              touch "{root}/web/dist/index.html"
              exit 0
            fi
            if [[ "$1" == "--dir" && "$3" == "exec" && "$4" == "playwright" && "${{5:-}}" == "test" ]]; then
              exec "{shim_bin}/playwright-shim" "$@"
            fi
            if [[ "$1" == "--dir" && "$3" == "exec" && "$4" == "playwright" ]]; then
              echo "Version 1.55.0"
              exit 0
            fi
            if [[ "$1" == "exec" && "$2" == "playwright" ]]; then
              echo "Version 1.55.0"
              exit 0
            fi
            echo "unexpected pnpm invocation: $*" >&2
            exit 1
            """,
        )

        _write_executable(
            shim_bin / "playwright-shim",
            f"""\
            #!/usr/bin/env bash
            set -euo pipefail
            echo "playwright-start pid=$$" >> "{state}/playwright.events"
            touch "{state}/playwright.started"
            echo "creds=${{MARKHAND_E2E_REAL_CREDENTIALS_FILE:-}}" >> "{state}/playwright.env"
            echo "fixture=${{MARKHAND_E2E_REAL_FIXTURE_FILE:-}}" >> "{state}/playwright.env"
            echo "run_id=${{MARKHAND_E2E_REAL_RUN_ID:-}}" >> "{state}/playwright.env"
            results="${{WEB_E2E_REAL_PLAYWRIGHT_RESULTS:-}}"
            if [[ -n "$results" ]]; then
              mkdir -p "$(dirname "$results")"
              if [[ "${{WEB_E2E_REAL_PLAYWRIGHT_SKIP:-}}" == "1" ]]; then
                cat >"$results" <<'JSON'
            {{"suites":[],"stats":{{"expected":0,"unexpected":0,"flaky":0,"skipped":1}},"tests":[{{"title":"required scenario","outcome":"skipped","ok":false,"duration":1}}]}}
            JSON
              elif [[ "${{WEB_E2E_REAL_CANARY_IN_RESULTS:-}}" == "1" ]]; then
                cat >"$results" <<'JSON'
            {{"suites":[],"stats":{{"expected":1,"unexpected":0,"flaky":0,"skipped":0}},"tests":[{{"title":"login","outcome":"expected","ok":true,"duration":5}}],"leak":"password=super-secret-canary-value"}}
            JSON
              else
                cat >"$results" <<'JSON'
            {{"suites":[],"stats":{{"expected":1,"unexpected":0,"flaky":0,"skipped":0}},"tests":[{{"title":"login","outcome":"expected","ok":true,"duration":5}}]}}
            JSON
              fi
            fi
            if [[ "${{WEB_E2E_REAL_PLAYWRIGHT_FAIL:-}}" == "1" ]]; then
              echo "playwright-fail" >> "{state}/playwright.events"
              exit 9
            fi
            trap 'echo playwright-term >> "{state}/playwright.events"; exit 143' TERM INT
            i=0
            while [[ $i -lt 40 ]]; do
              sleep 0.05
              i=$((i + 1))
            done
            echo "playwright-done" >> "{state}/playwright.events"
            exit 0
            """,
        )

        for name in ("bootstrap-server-role.sh", "migrate.sh", "seed-dev-all.sh"):
            script_path = self.root / "deploy" / "scripts" / name
            script_path.parent.mkdir(parents=True, exist_ok=True)
            _write_executable(
                script_path,
                "#!/usr/bin/env bash\nexit 0\n",
            )
        (self.root / "deploy" / "poc").mkdir(parents=True, exist_ok=True)
        (self.root / "deploy" / "poc" / "qdrant-init.py").write_text(
            "#!/usr/bin/env python3\n", encoding="utf-8"
        )

        deploy_scripts = REPO_ROOT / "deploy" / "scripts"
        script_dest = self.root / "deploy" / "scripts" / "web-e2e-real.sh"
        script_dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SCRIPT, script_dest)
        shutil.copy2(
            deploy_scripts / "redact_secrets.py",
            self.root / "deploy" / "scripts" / "redact_secrets.py",
        )
        shutil.copy2(
            deploy_scripts / "init-dev-env.sh",
            self.root / "deploy" / "scripts" / "init-dev-env.sh",
        )
        if ARTIFACTS.exists():
            shutil.copy2(
                ARTIFACTS,
                self.root / "deploy" / "scripts" / "web_e2e_real_artifacts.py",
            )
        self._install_fixture_shim()

    def _install_fixture_shim(self) -> None:
        state = self.state
        fixture_path = self.root / "deploy" / "scripts" / "web_e2e_real_fixture.py"
        fixture_path.parent.mkdir(parents=True, exist_ok=True)
        fixture_path.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                from __future__ import annotations

                import json
                import os
                import sys
                from pathlib import Path

                STATE = Path({str(state)!r})


                def _arg(flag: str) -> str:
                    argv = sys.argv[1:]
                    for index, value in enumerate(argv):
                        if value == flag and index + 1 < len(argv):
                            return argv[index + 1]
                    raise SystemExit(f"missing {{flag}}")


                def main() -> int:
                    if len(sys.argv) < 2:
                        return 2
                    command = sys.argv[1]
                    if command == "setup":
                        (STATE / "fixture.events").write_text(
                            "setup-start\\n", encoding="utf-8"
                        )
                        if os.environ.get("WEB_E2E_REAL_FIXTURE_SETUP_FAIL") == "1":
                            with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                                handle.write("setup-fail\\n")
                            print("fixture setup failed", file=sys.stderr)
                            return 3
                        run_id = _arg("--run-id")
                        manifest_out = Path(_arg("--manifest-out"))
                        credentials_out = Path(_arg("--credentials-out"))
                        manifest_out.parent.mkdir(parents=True, exist_ok=True)
                        credentials_out.parent.mkdir(parents=True, exist_ok=True)
                        manifest = {{
                            "runId": run_id,
                            "orgId": "11111111-1111-1111-1111-111111111111",
                            "adminUserId": "22222222-2222-2222-2222-222222222201",
                            "viewerUserId": "22222222-2222-2222-2222-222222222202",
                            "collectionId": "33333333-3333-3333-3333-333333333333",
                            "collectionName": f"E2E Library {{run_id}}",
                            "failedDocumentId": "44444444-4444-4444-4444-444444444401",
                            "failedVersionId": "44444444-4444-4444-4444-444444444402",
                            "objectIds": ["55555555-5555-5555-5555-555555555501"],
                            "vectorPointIds": ["66666666-6666-6666-6666-666666666601"],
                            "checksum": "a" * 64,
                        }}
                        credentials = {{
                            "runId": run_id,
                            "adminEmail": f"admin+{{run_id}}@example.test",
                            "adminPassword": "admin-secret-value",
                            "viewerEmail": f"viewer+{{run_id}}@example.test",
                            "viewerPassword": "viewer-secret-value",
                        }}
                        manifest_out.write_text(json.dumps(manifest), encoding="utf-8")
                        credentials_out.write_text(json.dumps(credentials), encoding="utf-8")
                        credentials_out.chmod(0o600)
                        with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                            handle.write("setup-ok\\n")
                        (STATE / "fixture.manifest").write_text(str(manifest_out), encoding="utf-8")
                        (STATE / "fixture.credentials").write_text(
                            str(credentials_out), encoding="utf-8"
                        )
                        return 0
                    if command == "cleanup":
                        with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                            handle.write("cleanup-start\\n")
                        if os.environ.get("WEB_E2E_REAL_FIXTURE_CLEANUP_FAIL") == "1":
                            with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                                handle.write("cleanup-fail\\n")
                            print("fixture cleanup failed", file=sys.stderr)
                            return 4
                        with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                            handle.write("cleanup-ok\\n")
                        return 0
                    if command == "verify-clean":
                        return 0
                    print(f"unknown command: {{command}}", file=sys.stderr)
                    return 2


                if __name__ == "__main__":
                    raise SystemExit(main())
                """
            ),
            encoding="utf-8",
        )
        fixture_path.chmod(0o755)

    def run(
        self,
        *,
        extra_env: dict[str, str] | None = None,
        timeout: float = 30.0,
        failing_cargo: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "WEB_E2E_REAL_ORCHESTRATION_TEST": "1",
                "WEB_E2E_REAL_ENV_FILE": str(self.root / "deploy" / "dev" / ".env"),
                "WEB_E2E_REAL_ROOT": str(self.root),
                "WEB_E2E_REAL_SHIM_BIN_DIR": str(self.shim_bin),
                "WEB_E2E_REAL_PLAYWRIGHT_CMD": f"exec {self.shim_bin}/playwright-shim",
                "WEB_E2E_REAL_ARTIFACT_DIR": str(self.artifact_dir),
                "WEB_E2E_REAL_RUN_ID": "e2e-orch-test-run",
                "PATH": f"{self.path_bin}:{self.shim_bin}:{env.get('PATH', '')}",
                "HOME": str(self.tempdir),
            }
        )
        if extra_env:
            env.update(extra_env)
        for shim_name in (
            "curl",
            "pnpm",
            "fileconv-server",
            "fileconv-worker",
            "fileconv",
            "playwright-shim",
        ):
            shutil.copy2(self.shim_bin / shim_name, self.path_bin / shim_name)
        if failing_cargo:
            _write_executable(
                self.path_bin / "cargo",
                "#!/usr/bin/env bash\nexit 17\n",
            )
        else:
            shutil.copy2(self.shim_bin / "cargo", self.path_bin / "cargo")
        return subprocess.run(
            ["bash", str(self.root / "deploy" / "scripts" / "web-e2e-real.sh")],
            cwd=str(self.root),
            env=env,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )

    def install_failing_redactor(self) -> None:
        redact = self.root / "deploy" / "scripts" / "redact_secrets.py"
        _write_executable(
            redact,
            """\
            #!/usr/bin/env python3
            import sys
            from pathlib import Path
            raw = Path(sys.argv[1]).read_text(encoding="utf-8")
            if "super-secret-key-at-least-32-bytes" in raw:
                sys.stderr.write("INJECTED_REDACT_FAIL\\n")
            sys.exit(1)
            """,
        )

    def install_failing_artifact_validator(self) -> None:
        artifacts = self.root / "deploy" / "scripts" / "web_e2e_real_artifacts.py"
        _write_executable(
            artifacts,
            """\
            #!/usr/bin/env python3
            import sys
            if len(sys.argv) > 1 and sys.argv[1] == "write":
                # Still write a stub so the orchestrator reaches validate.
                out = None
                argv = sys.argv[1:]
                for index, value in enumerate(argv):
                    if value == "--out" and index + 1 < len(argv):
                        out = argv[index + 1]
                if out:
                    from pathlib import Path
                    Path(out).write_text("{\\"schemaVersion\\":1}\\n", encoding="utf-8")
                raise SystemExit(0)
            print("artifact validation injected failure", file=sys.stderr)
            raise SystemExit(11)
            """,
        )

    def child_pids(self) -> list[int]:
        pids_file = self.state / "child.pids"
        if not pids_file.exists():
            return []
        pids: list[int] = []
        for line in pids_file.read_text(encoding="utf-8").splitlines():
            _, pid_text = line.split(":", 1)
            pids.append(int(pid_text))
        return pids

    def assert_no_live_child_pids(self) -> None:
        for pid in self.child_pids():
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                continue
            self.fail(f"pid {pid} still alive")

    def cleanup(self) -> None:
        shutil.rmtree(self.tempdir, ignore_errors=True)


class WebE2eRealExecutableOrchestrationTests(unittest.TestCase):
    def test_starts_workers_with_dedicated_role_and_repo_fileconv(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(timeout=20.0)
            self.assertEqual(
                result.returncode,
                0,
                msg=f"stdout={result.stdout}\nstderr={result.stderr}",
            )
            started = (harness.state / "workers.started").read_text(encoding="utf-8")
            self.assertIn("kind=convert", started)
            self.assertIn("kind=index", started)
            self.assertIn("kind=embedding", started)
            self.assertIn("kind=delete", started)
            env_dump = (harness.state / "workers.env").read_text(encoding="utf-8")
            self.assertRegex(env_dump, r"markhand_worker")
            self.assertRegex(env_dump, r"target/debug/fileconv")
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
            server_env = (harness.state / "server.env").read_text(encoding="utf-8")
            self.assertIn("MARKHAND_MAX_UPLOAD_BYTES=4096", server_env)
            self.assertIn("MARKHAND_RATE_ROUTE_PER_MINUTE=1", server_env)
            playwright_env = (harness.state / "playwright.env").read_text(encoding="utf-8")
            self.assertIn("creds=", playwright_env)
            self.assertIn("fixture=", playwright_env)
            self.assertIn("run_id=e2e-orch-test-run", playwright_env)
            self.assertTrue(
                (harness.artifact_dir / "manifest.json").is_file(),
                msg="sanitized artifact manifest must be written",
            )
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-ok", fixture_events)
            self.assertIn("cleanup-ok", fixture_events)
        finally:
            harness.cleanup()

    def test_fail_closed_log_dump_withholds_raw_secrets(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_SERVER_EXIT_EARLY": "1"},
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            combined = result.stdout + result.stderr
            self.assertNotIn("super-secret-key-at-least-32-bytes", combined)
            self.assertIn("=== web-e2e-real:", result.stderr)
            self.assertTrue(
                "raw log withheld" in combined.lower()
                or "redaction failed" in combined.lower()
                or "<REDACTED" in combined
                or "SECRET_REDACT_FAIL_CLOSED" in combined,
                msg=combined,
            )
        finally:
            harness.cleanup()

    def test_supervises_and_aborts_playwright_when_worker_dies(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_WORKER_DIE_DELAY": "0.15"},
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("during Playwright", result.stderr)
            events = (harness.state / "playwright.events").read_text(encoding="utf-8")
            self.assertIn("playwright-start", events)
            self.assertIn("playwright-term", events)
            self.assertNotIn("playwright-done", events)
            self.assertIn("worker-die-convert", (harness.state / "workers.events").read_text())
        finally:
            harness.cleanup()

    def test_cleanup_waits_children_after_build_failure(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(timeout=10.0, failing_cargo=True)
            self.assertEqual(result.returncode, 17)
            self.assertNotIn("unbound variable", result.stderr.lower())
        finally:
            harness.cleanup()

    def test_failing_redactor_emits_only_safe_withholding_text(self) -> None:
        harness = OrchestrationHarness()
        try:
            harness.install_failing_redactor()
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1"},
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            combined = result.stdout + result.stderr
            self.assertNotIn("super-secret-key-at-least-32-bytes", combined)
            self.assertIn("raw log withheld", combined.lower())
            self.assertNotRegex(combined, r"MARKHAND_AUTH_SIGNING_KEY=super-secret")
        finally:
            harness.cleanup()

    def test_cleanup_terminates_and_waits_all_children_after_failure(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1"},
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertGreaterEqual(len(harness.child_pids()), 5)
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            for kind in ("convert", "index", "embedding", "delete"):
                self.assertIn(f"worker-reaped-{kind}:", reaped)
            harness.assert_no_live_child_pids()
        finally:
            harness.cleanup()

    def test_unified_liveness_check_after_seed(self) -> None:
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertRegex(
            text,
            r'required_processes_alive[^\n]*after seed, before Playwright',
            msg="must use unified server+worker liveness after seed",
        )
        seed_to_playwright = text.split('seed-dev-all.sh" --skip-init', 1)[1]
        self.assertNotIn("workers_alive", seed_to_playwright.split("run_playwright_supervised", 1)[0])

    def test_forbids_fail_open_redaction_flags(self) -> None:
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn("--allow-residual", text)
        self.assertNotRegex(
            text,
            r'redact_secrets\.py[^\n]*\|\|\s*cat',
            msg="must not fall back to raw cat after redaction failure",
        )
        self.assertNotRegex(
            text,
            r"set -e\s*\n\s*run_playwright_supervised\s*\n\s*playwright_status=\$\?",
            msg="must not re-enable errexit before capturing Playwright status",
        )
        self.assertIn("run_playwright_supervised || playwright_status=", text)

    def test_fixture_setup_failure_aborts_before_playwright(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_FIXTURE_SETUP_FAIL": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(result.returncode, 0)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-fail", fixture_events)
            self.assertFalse(
                (harness.state / "playwright.started").exists(),
                msg="Playwright must not start after fixture setup failure",
            )
            combined = result.stdout + result.stderr
            self.assertNotIn("admin-secret-value", combined)
        finally:
            harness.cleanup()

    def test_fixture_cleanup_failure_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_FIXTURE_CLEANUP_FAIL": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(
                result.returncode,
                0,
                msg=f"stdout={result.stdout}\nstderr={result.stderr}",
            )
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-ok", fixture_events)
            self.assertIn("cleanup-fail", fixture_events)
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
        finally:
            harness.cleanup()

    def test_playwright_failure_still_runs_fixture_cleanup(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(result.returncode, 0)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-ok", fixture_events)
            self.assertIn("cleanup-ok", fixture_events)
            self.assertIn("playwright-fail", (harness.state / "playwright.events").read_text())
        finally:
            harness.cleanup()

    def test_canary_match_in_artifacts_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_CANARY_IN_RESULTS": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(
                result.returncode,
                0,
                msg=f"stdout={result.stdout}\nstderr={result.stderr}",
            )
            combined = result.stdout + result.stderr
            self.assertNotIn("super-secret-canary-value", combined)
            self.assertTrue(
                "canary" in combined.lower() or "secret" in combined.lower(),
                msg=combined,
            )
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup-ok", fixture_events)
        finally:
            harness.cleanup()

    def test_artifact_validation_failure_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            harness.install_failing_artifact_validator()
            result = harness.run(timeout=20.0)
            self.assertNotEqual(
                result.returncode,
                0,
                msg=f"stdout={result.stdout}\nstderr={result.stderr}",
            )
            self.assertIn("artifact", (result.stdout + result.stderr).lower())
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup-ok", fixture_events)
        finally:
            harness.cleanup()

    def test_skipped_required_scenario_fails_artifact_validation(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_SKIP": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(
                result.returncode,
                0,
                msg=f"stdout={result.stdout}\nstderr={result.stderr}",
            )
            combined = (result.stdout + result.stderr).lower()
            self.assertTrue(
                "skip" in combined or "artifact" in combined,
                msg=combined,
            )
        finally:
            harness.cleanup()

    def test_script_wires_fixture_artifact_and_delete_worker_hooks(self) -> None:
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("web_e2e_real_fixture.py", text)
        self.assertIn("web_e2e_real_artifacts.py", text)
        self.assertIn("MARKHAND_MAX_UPLOAD_BYTES=4096", text)
        self.assertIn("MARKHAND_RATE_ROUTE_PER_MINUTE=1", text)
        self.assertIn("MARKHAND_E2E_REAL_CREDENTIALS_FILE", text)
        self.assertIn("MARKHAND_E2E_REAL_FIXTURE_FILE", text)
        self.assertRegex(text, r"start_worker\s+delete\b")
        self.assertIn("WEB_E2E_REAL_RUN_ID", text)
        self.assertIn("WEB_E2E_REAL_ARTIFACT_DIR", text)


def main() -> int:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
