#!/usr/bin/env python3
"""Hermetic executable tests for web-e2e-real.sh process orchestration."""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "web-e2e-real.sh"
REPO_ROOT = SCRIPT.resolve().parents[2]


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
        self.state.mkdir()
        self.shim_bin.mkdir()
        self.path_bin.mkdir()
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
            if [[ "$1" == "--dir" && "$3" == "build" ]]; then
              mkdir -p "{root}/web/dist"
              touch "{root}/web/dist/index.html"
              exit 0
            fi
            if [[ "$1" == "--dir" && "$3" == "exec" && "$4" == "playwright" ]]; then
              exec "{shim_bin}/playwright-shim" "$@"
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
            env_dump = (harness.state / "workers.env").read_text(encoding="utf-8")
            self.assertRegex(env_dump, r"markhand_worker")
            self.assertRegex(env_dump, r"target/debug/fileconv")
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
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
            self.assertGreaterEqual(len(harness.child_pids()), 4)
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            for kind in ("convert", "index", "embedding"):
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


def main() -> int:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
