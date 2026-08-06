#!/usr/bin/env python3
"""Hermetic executable tests for web-e2e-real.sh process orchestration."""

from __future__ import annotations

import json
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

HERMETIC_FIXTURE_CHECKSUM = "b" * 64
HERMETIC_ORG_ID = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1"
HERMETIC_ADMIN_USER_ID = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbb1"
HERMETIC_RUN_ID = "e2e-abcdef012345-1"
HERMETIC_ADMIN_PASSWORD = "hermetic-admin-password-not-for-logs"


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
            echo "max_upload=${{MARKHAND_MAX_UPLOAD_BYTES:-}}" >> "{state}/server.knobs"
            echo "rate_route=${{MARKHAND_RATE_ROUTE_PER_MINUTE:-}}" >> "{state}/server.knobs"
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
            echo "org=${{MARKHAND_WORKER_ORG_ID:-}}" >> "{state}/workers.env"
            echo "user=${{MARKHAND_WORKER_USER_ID:-}}" >> "{state}/workers.env"
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
            results="${{WEB_E2E_REAL_PLAYWRIGHT_RESULTS:-}}"
            if [[ -n "$results" ]]; then
              cat > "$results" <<'JSON'
            {{
              "suites": [
                {{
                  "title": "auth.spec.ts",
                  "file": "e2e-real/auth.spec.ts",
                  "specs": [
                    {{
                      "title": "login succeeds",
                      "ok": true,
                      "tests": [
                        {{
                          "projectName": "real",
                          "results": [
                            {{
                              "status": "passed",
                              "duration": 42,
                              "errors": [],
                              "stdout": [],
                              "stderr": []
                            }}
                          ],
                          "status": "expected"
                        }}
                      ]
                    }}
                  ],
                  "suites": []
                }}
              ],
              "errors": [],
              "stats": {{
                "duration": 42,
                "expected": 1,
                "skipped": 0,
                "unexpected": 0,
                "flaky": 0
              }}
            }}
            JSON
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
        self._install_fixture_shim()
        self._install_artifacts_shim()

    def _install_fixture_shim(self) -> None:
        state = self.state
        dest = self.root / "deploy" / "scripts" / "web_e2e_real_fixture.py"
        dest.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                from __future__ import annotations

                import argparse
                import json
                import os
                import sys
                from pathlib import Path

                STATE = Path({str(state)!r})


                def _record(event: str) -> None:
                    with (STATE / "fixture.events").open("a", encoding="utf-8") as handle:
                        handle.write(event + "\\n")


                def cmd_setup(args: argparse.Namespace) -> int:
                    _record("setup:" + args.run_id)
                    if os.environ.get("WEB_E2E_REAL_FIXTURE_SETUP_FAIL") == "1":
                        _record("setup-fail")
                        print("web_e2e_real_fixture: setup failed", file=sys.stderr)
                        return 3
                    manifest = {{
                        "runId": args.run_id,
                        "orgId": {HERMETIC_ORG_ID!r},
                        "adminUserId": {HERMETIC_ADMIN_USER_ID!r},
                        "viewerUserId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbb2",
                        "collectionId": "cccccccc-cccc-cccc-cccc-ccccccccccc1",
                        "collectionName": "E2E Library " + args.run_id,
                        "failedDocumentId": "dddddddd-dddd-dddd-dddd-ddddddddddd1",
                        "failedVersionId": "dddddddd-dddd-dddd-dddd-ddddddddddd2",
                        "objectIds": ["eeeeeeee-eeee-eeee-eeee-eeeeeeeeeee1"],
                        "vectorPointIds": ["ffffffff-ffff-ffff-ffff-fffffffffff1"],
                        "checksum": {HERMETIC_FIXTURE_CHECKSUM!r},
                    }}
                    credentials = {{
                        "runId": args.run_id,
                        "adminEmail": "admin+" + args.run_id + "@example.test",
                        "adminPassword": {HERMETIC_ADMIN_PASSWORD!r},
                        "viewerEmail": "viewer+" + args.run_id + "@example.test",
                        "viewerPassword": "hermetic-viewer-password-not-for-logs",
                    }}
                    manifest_out = Path(args.manifest_out)
                    credentials_out = Path(args.credentials_out)
                    manifest_out.parent.mkdir(parents=True, exist_ok=True)
                    credentials_out.parent.mkdir(parents=True, exist_ok=True)
                    manifest_out.write_text(json.dumps(manifest), encoding="utf-8")
                    credentials_out.write_text(json.dumps(credentials), encoding="utf-8")
                    os.chmod(credentials_out, 0o600)
                    _record("setup-ok")
                    _record("credentials-mode:%03o" % (credentials_out.stat().st_mode & 0o777))
                    return 0


                def cmd_cleanup(args: argparse.Namespace) -> int:
                    _record("cleanup:" + args.run_id)
                    if os.environ.get("WEB_E2E_REAL_FIXTURE_CLEANUP_FAIL") == "1":
                        _record("cleanup-fail")
                        print("web_e2e_real_fixture: cleanup failed", file=sys.stderr)
                        return 4
                    Path(args.credentials).unlink(missing_ok=True)
                    _record("cleanup-ok")
                    return 0


                def cmd_verify_clean(args: argparse.Namespace) -> int:
                    _record("verify-clean:" + args.run_id)
                    if os.environ.get("WEB_E2E_REAL_FIXTURE_VERIFY_FAIL") == "1":
                        _record("verify-fail")
                        print("web_e2e_real_fixture: verify-clean found leaks", file=sys.stderr)
                        return 5
                    _record("verify-ok")
                    return 0


                def main(argv: list[str] | None = None) -> int:
                    parser = argparse.ArgumentParser(prog="web_e2e_real_fixture.py")
                    sub = parser.add_subparsers(dest="command", required=True)
                    setup = sub.add_parser("setup")
                    setup.add_argument("--run-id", required=True)
                    setup.add_argument("--manifest-out", required=True)
                    setup.add_argument("--credentials-out", required=True)
                    cleanup = sub.add_parser("cleanup")
                    cleanup.add_argument("--run-id", required=True)
                    cleanup.add_argument("--manifest", required=True)
                    cleanup.add_argument("--credentials", required=True)
                    cleanup.add_argument("--api-base", required=True)
                    cleanup.add_argument("--timeout-secs", required=True, type=float)
                    verify = sub.add_parser("verify-clean")
                    verify.add_argument("--run-id", required=True)
                    verify.add_argument("--manifest", required=True)
                    args = parser.parse_args(argv)
                    if args.command == "setup":
                        return cmd_setup(args)
                    if args.command == "cleanup":
                        return cmd_cleanup(args)
                    return cmd_verify_clean(args)


                if __name__ == "__main__":
                    raise SystemExit(main())
                """
            ),
            encoding="utf-8",
        )
        dest.chmod(0o755)

    def _install_artifacts_shim(self) -> None:
        state = self.state
        deploy_scripts = REPO_ROOT / "deploy" / "scripts"
        source = deploy_scripts / "web_e2e_real_artifacts.py"
        dest = self.root / "deploy" / "scripts" / "web_e2e_real_artifacts.py"
        if source.exists():
            shutil.copy2(source, dest)
            return
        dest.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                from __future__ import annotations
                import argparse
                import json
                import sys
                from pathlib import Path

                STATE = Path({str(state)!r})


                def main() -> int:
                    with (STATE / "artifacts.events").open("a", encoding="utf-8") as handle:
                        handle.write("missing-impl:" + " ".join(sys.argv[1:]) + "\\n")
                    print("web_e2e_real_artifacts: not implemented", file=sys.stderr)
                    return 99


                if __name__ == "__main__":
                    raise SystemExit(main())
                """
            ),
            encoding="utf-8",
        )
        dest.chmod(0o755)

    def install_failing_artifacts_validate(self) -> None:
        dest = self.root / "deploy" / "scripts" / "web_e2e_real_artifacts.py"
        state = self.state
        dest.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                from __future__ import annotations

                import argparse
                import hashlib
                import json
                import sys
                from pathlib import Path

                STATE = Path({str(state)!r})


                def _record(event: str) -> None:
                    with (STATE / "artifacts.events").open("a", encoding="utf-8") as handle:
                        handle.write(event + "\\n")


                def cmd_write(args: argparse.Namespace) -> int:
                    _record("write")
                    fixture = json.loads(Path(args.fixture).read_text(encoding="utf-8"))
                    out = Path(args.out)
                    out.parent.mkdir(parents=True, exist_ok=True)
                    manifest = {{
                        "schemaVersion": 1,
                        "runId": fixture.get("runId", ""),
                        "git": {{"sha": "deadbeef", "ref": "HEAD"}},
                        "toolVersions": {{"node": "0", "pnpm": "0", "playwright": "0"}},
                        "fixtureChecksum": fixture.get("checksum", ""),
                        "scenarios": [
                            {{"title": "login succeeds", "outcome": "passed", "durationMs": 42}}
                        ],
                        "skippedCount": 0,
                        "teardown": {{"result": args.teardown}},
                        "artifactChecksums": {{}},
                    }}
                    payload = json.dumps(manifest, indent=2, sort_keys=True) + "\\n"
                    out.write_text(payload, encoding="utf-8")
                    digest = hashlib.sha256(out.read_bytes()).hexdigest()
                    manifest["artifactChecksums"][out.name] = digest
                    out.write_text(
                        json.dumps(manifest, indent=2, sort_keys=True) + "\\n",
                        encoding="utf-8",
                    )
                    return 0


                def cmd_validate(args: argparse.Namespace) -> int:
                    _record("validate-fail")
                    print("web_e2e_real_artifacts: validation failed", file=sys.stderr)
                    return 7


                def main() -> int:
                    parser = argparse.ArgumentParser()
                    sub = parser.add_subparsers(dest="command", required=True)
                    write = sub.add_parser("write")
                    write.add_argument("--results", required=True)
                    write.add_argument("--fixture", required=True)
                    write.add_argument("--out", required=True)
                    write.add_argument("--teardown", required=True)
                    validate = sub.add_parser("validate")
                    validate.add_argument("--manifest", required=True)
                    validate.add_argument("--artifact-dir", required=True)
                    args = parser.parse_args()
                    if args.command == "write":
                        return cmd_write(args)
                    return cmd_validate(args)


                if __name__ == "__main__":
                    raise SystemExit(main())
                """
            ),
            encoding="utf-8",
        )
        dest.chmod(0o755)

    def install_canary_planting_artifacts(self) -> None:
        dest = self.root / "deploy" / "scripts" / "web_e2e_real_artifacts.py"
        state = self.state
        dest.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                from __future__ import annotations

                import argparse
                import hashlib
                import json
                import os
                import sys
                from pathlib import Path

                STATE = Path({str(state)!r})


                def _record(event: str) -> None:
                    with (STATE / "artifacts.events").open("a", encoding="utf-8") as handle:
                        handle.write(event + "\\n")


                def _canaries() -> list[str]:
                    values: list[str] = []
                    for key in ("WEB_E2E_REAL_SECRET_CANARIES", "WEB_E2E_REAL_CONTENT_CANARIES"):
                        raw = os.environ.get(key, "")
                        for part in raw.replace(",", "\\n").splitlines():
                            item = part.strip()
                            if item:
                                values.append(item)
                    return values


                def cmd_write(args: argparse.Namespace) -> int:
                    _record("write-canary")
                    fixture = json.loads(Path(args.fixture).read_text(encoding="utf-8"))
                    out = Path(args.out)
                    out.parent.mkdir(parents=True, exist_ok=True)
                    planted = out.parent / "planted.txt"
                    planted.write_text("CANARY_CONTENT_LEAK\\n", encoding="utf-8")
                    manifest = {{
                        "schemaVersion": 1,
                        "runId": fixture.get("runId", ""),
                        "git": {{"sha": "deadbeef", "ref": "HEAD"}},
                        "toolVersions": {{"node": "0", "pnpm": "0", "playwright": "0"}},
                        "fixtureChecksum": fixture.get("checksum", ""),
                        "scenarios": [
                            {{"title": "login succeeds", "outcome": "passed", "durationMs": 42}}
                        ],
                        "skippedCount": 0,
                        "teardown": {{"result": args.teardown}},
                        "artifactChecksums": {{
                            "planted.txt": hashlib.sha256(planted.read_bytes()).hexdigest(),
                        }},
                    }}
                    out.write_text(
                        json.dumps(manifest, indent=2, sort_keys=True) + "\\n",
                        encoding="utf-8",
                    )
                    manifest["artifactChecksums"][out.name] = hashlib.sha256(
                        out.read_bytes()
                    ).hexdigest()
                    out.write_text(
                        json.dumps(manifest, indent=2, sort_keys=True) + "\\n",
                        encoding="utf-8",
                    )
                    return 0


                def cmd_validate(args: argparse.Namespace) -> int:
                    _record("validate")
                    artifact_dir = Path(args.artifact_dir)
                    for path in sorted(artifact_dir.rglob("*")):
                        if not path.is_file():
                            continue
                        text = path.read_text(encoding="utf-8", errors="replace")
                        for canary in _canaries():
                            if canary in text:
                                print(
                                    "web_e2e_real_artifacts: canary matched",
                                    file=sys.stderr,
                                )
                                return 8
                    return 0


                def main() -> int:
                    parser = argparse.ArgumentParser()
                    sub = parser.add_subparsers(dest="command", required=True)
                    write = sub.add_parser("write")
                    write.add_argument("--results", required=True)
                    write.add_argument("--fixture", required=True)
                    write.add_argument("--out", required=True)
                    write.add_argument("--teardown", required=True)
                    validate = sub.add_parser("validate")
                    validate.add_argument("--manifest", required=True)
                    validate.add_argument("--artifact-dir", required=True)
                    args = parser.parse_args()
                    if args.command == "write":
                        return cmd_write(args)
                    return cmd_validate(args)


                if __name__ == "__main__":
                    raise SystemExit(main())
                """
            ),
            encoding="utf-8",
        )
        dest.chmod(0o755)

    def run(
        self,
        *,
        extra_env: dict[str, str] | None = None,
        timeout: float = 30.0,
        failing_cargo: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        artifact_dir = self.tempdir / "artifacts"
        runtime_dir = self.tempdir / "runtime"
        artifact_dir.mkdir(exist_ok=True)
        runtime_dir.mkdir(exist_ok=True)
        env = os.environ.copy()
        env.update(
            {
                "WEB_E2E_REAL_ORCHESTRATION_TEST": "1",
                "WEB_E2E_REAL_ENV_FILE": str(self.root / "deploy" / "dev" / ".env"),
                "WEB_E2E_REAL_ROOT": str(self.root),
                "WEB_E2E_REAL_SHIM_BIN_DIR": str(self.shim_bin),
                "WEB_E2E_REAL_PLAYWRIGHT_CMD": f"exec {self.shim_bin}/playwright-shim",
                "WEB_E2E_REAL_RUN_ID": HERMETIC_RUN_ID,
                "WEB_E2E_REAL_ARTIFACT_DIR": str(artifact_dir),
                "WEB_E2E_REAL_RUNTIME_DIR": str(runtime_dir),
                "WEB_E2E_REAL_CLEANUP_TIMEOUT_SECS": "5",
                "WEB_E2E_REAL_CONTENT_CANARIES": "CANARY_CONTENT_LEAK",
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
            self.assertIn("kind=delete", started)
            env_dump = (harness.state / "workers.env").read_text(encoding="utf-8")
            self.assertRegex(env_dump, r"markhand_worker")
            self.assertRegex(env_dump, r"target/debug/fileconv")
            self.assertIn(f"org={HERMETIC_ORG_ID}", env_dump)
            self.assertIn(f"user={HERMETIC_ADMIN_USER_ID}", env_dump)
            self.assertNotIn("11111111-1111-1111-1111-111111111111", env_dump)
            knobs = (harness.state / "server.knobs").read_text(encoding="utf-8")
            self.assertIn("max_upload=4096", knobs)
            self.assertIn("rate_route=1", knobs)
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-ok", fixture_events)
            self.assertIn("cleanup-ok", fixture_events)
            self.assertIn("verify-ok", fixture_events)
            manifest_path = harness.tempdir / "artifacts" / "manifest.json"
            self.assertTrue(manifest_path.is_file(), msg="happy path must stage sanitized manifest")
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            for key in (
                "schemaVersion",
                "runId",
                "git",
                "toolVersions",
                "fixtureChecksum",
                "scenarios",
                "skippedCount",
                "teardown",
                "artifactChecksums",
            ):
                self.assertIn(key, manifest)
            self.assertEqual(manifest["skippedCount"], 0)
            self.assertEqual(manifest["teardown"], {"result": "ok"})
            self.assertEqual(manifest["fixtureChecksum"], HERMETIC_FIXTURE_CHECKSUM)
            self.assertIsInstance(manifest["scenarios"], list)
            self.assertGreaterEqual(len(manifest["scenarios"]), 1)
            self.assertFalse(
                (harness.tempdir / "runtime" / "playwright-results.json").exists(),
                msg="raw Playwright JSON must not remain after EXIT cleanup",
            )
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
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
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
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
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
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
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup:", fixture_events)
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
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-fail", fixture_events)
            self.assertFalse((harness.state / "playwright.started").exists())
            self.assertFalse((harness.state / "workers.started").exists())
        finally:
            harness.cleanup()

    def test_fixture_cleanup_failure_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_FIXTURE_CLEANUP_FAIL": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(result.returncode, 0)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("setup-ok", fixture_events)
            self.assertIn("cleanup-fail", fixture_events)
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
        finally:
            harness.cleanup()

    def test_fixture_verify_clean_failure_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_FIXTURE_VERIFY_FAIL": "1"},
                timeout=20.0,
            )
            self.assertNotEqual(result.returncode, 0)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup-ok", fixture_events)
            self.assertIn("verify-fail", fixture_events)
            self.assertIn("playwright-done", (harness.state / "playwright.events").read_text())
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
        finally:
            harness.cleanup()

    def test_playwright_failure_still_runs_fixture_cleanup(self) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1"},
                timeout=15.0,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.returncode, 9)
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup-ok", fixture_events)
            self.assertIn("verify-ok", fixture_events)
        finally:
            harness.cleanup()

    def test_playwright_failure_preserves_status_when_fixture_cleanup_also_fails(
        self,
    ) -> None:
        harness = OrchestrationHarness()
        try:
            result = harness.run(
                extra_env={
                    "WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1",
                    "WEB_E2E_REAL_FIXTURE_CLEANUP_FAIL": "1",
                },
                timeout=20.0,
            )
            self.assertEqual(
                result.returncode,
                9,
                msg=(
                    "primary Playwright status must be preserved when teardown also fails; "
                    f"stdout={result.stdout}\nstderr={result.stderr}"
                ),
            )
            fixture_events = (harness.state / "fixture.events").read_text(encoding="utf-8")
            self.assertIn("cleanup-fail", fixture_events)
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            for kind in ("convert", "index", "embedding", "delete"):
                self.assertIn(f"worker-reaped-{kind}:", reaped)
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
        finally:
            harness.cleanup()

    def test_playwright_failure_preserves_status_when_artifact_validation_also_fails(
        self,
    ) -> None:
        harness = OrchestrationHarness()
        try:
            harness.install_failing_artifacts_validate()
            result = harness.run(
                extra_env={"WEB_E2E_REAL_PLAYWRIGHT_FAIL": "1"},
                timeout=20.0,
            )
            self.assertEqual(
                result.returncode,
                9,
                msg=(
                    "primary Playwright status must be preserved when artifact validation "
                    f"also fails; stdout={result.stdout}\nstderr={result.stderr}"
                ),
            )
            events = (harness.state / "artifacts.events").read_text(encoding="utf-8")
            self.assertIn("write", events)
            self.assertIn("validate-fail", events)
            reaped = (harness.state / "cleanup.reaped").read_text(encoding="utf-8")
            self.assertIn("server-reaped:", reaped)
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
        finally:
            harness.cleanup()

    def test_artifact_validation_failure_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            harness.install_failing_artifacts_validate()
            result = harness.run(timeout=20.0)
            self.assertNotEqual(result.returncode, 0)
            events = (harness.state / "artifacts.events").read_text(encoding="utf-8")
            self.assertIn("write", events)
            self.assertIn("validate-fail", events)
            self.assertIn("cleanup-ok", (harness.state / "fixture.events").read_text())
        finally:
            harness.cleanup()

    def test_canary_match_fails_the_job(self) -> None:
        harness = OrchestrationHarness()
        try:
            harness.install_canary_planting_artifacts()
            result = harness.run(timeout=20.0)
            self.assertNotEqual(result.returncode, 0)
            combined = result.stdout + result.stderr
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
            events = (harness.state / "artifacts.events").read_text(encoding="utf-8")
            self.assertIn("write-canary", events)
        finally:
            harness.cleanup()

    def test_redactor_failure_fails_closed_without_raw_secrets(self) -> None:
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
            self.assertNotIn(HERMETIC_ADMIN_PASSWORD, combined)
            self.assertIn("raw log withheld", combined.lower())
        finally:
            harness.cleanup()


def main() -> int:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
