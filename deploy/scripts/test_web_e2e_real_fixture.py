#!/usr/bin/env python3
"""Hermetic tests for deploy/scripts/web_e2e_real_fixture.py (P2-20 Task 1)."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import web_e2e_real_fixture as fixture  # noqa: E402


@dataclass
class FakeHttpResponse:
    status: int
    headers: dict[str, str] = field(default_factory=dict)
    body: bytes = b""


@dataclass
class FakeCommands:
    """Deterministic command/HTTP/object/vector seam for hermetic tests."""

    psql_handler: Any = None
    http_handler: Any = None
    hash_password_result: str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$ZmFrZWhhc2hmYWtl"
    object_keys: set[str] = field(default_factory=set)
    vector_ids: set[str] = field(default_factory=set)
    subprocess_calls: list[list[str]] = field(default_factory=list)
    psql_calls: list[str] = field(default_factory=list)
    http_calls: list[dict[str, Any]] = field(default_factory=list)
    fail_psql: bool = False
    sleep_calls: list[float] = field(default_factory=list)

    def hash_password(self, password: str) -> str:
        self.subprocess_calls.append(["dev-hash-password", "<redacted>"])
        if not password:
            raise RuntimeError("empty password")
        return self.hash_password_result

    def psql(self, sql: str, *, redact: list[str] | None = None) -> str:
        recorded = sql
        for secret in redact or []:
            if secret:
                recorded = recorded.replace(secret, "<redacted>")
        self.psql_calls.append(recorded)
        if self.fail_psql:
            raise fixture.FixtureError("compose/db unavailable")
        if self.psql_handler is not None:
            return self.psql_handler(sql)
        return ""

    def http(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes | None = None,
        timeout: float,
    ) -> FakeHttpResponse:
        safe_headers = dict(headers or {})
        if "authorization" in {k.lower() for k in safe_headers}:
            safe_headers = {
                key: ("<redacted>" if key.lower() == "authorization" else value)
                for key, value in safe_headers.items()
            }
        self.http_calls.append(
            {
                "method": method,
                "url": url,
                "headers": safe_headers,
                "body": body,
                "timeout": timeout,
            }
        )
        if method == "DELETE" and "/api/v1/documents/" in url:
            # Simulate delete-worker object/vector cleanup after API tombstone.
            self.object_keys.clear()
            self.vector_ids.clear()
        if self.http_handler is not None:
            return self.http_handler(method, url, headers=headers, body=body, timeout=timeout)
        return FakeHttpResponse(status=204)

    def object_exists(self, key: str) -> bool:
        return key in self.object_keys

    def vector_exists(self, point_id: str) -> bool:
        return point_id in self.vector_ids

    def sleep(self, seconds: float) -> None:
        self.sleep_calls.append(seconds)


def _write_json(path: Path, payload: dict[str, Any], mode: int = 0o600) -> None:
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(mode)


def _sample_ids(run_id: str) -> dict[str, str]:
    base = uuid.uuid5(uuid.NAMESPACE_URL, f"markhand-e2e:{run_id}")
    org_id = str(uuid.uuid5(base, "org"))
    object_id = str(uuid.uuid5(base, "object"))
    return {
        "runId": run_id,
        "orgId": org_id,
        "adminUserId": str(uuid.uuid5(base, "admin")),
        "viewerUserId": str(uuid.uuid5(base, "viewer")),
        "collectionId": str(uuid.uuid5(base, "collection")),
        "failedDocumentId": str(uuid.uuid5(base, "failed-doc")),
        "failedVersionId": str(uuid.uuid5(base, "failed-ver")),
        "objectId": object_id,
        "vectorPointId": str(uuid.uuid5(base, "vector")),
        "objectKey": fixture.quarantine_object_key(org_id, object_id),
    }


def _manifest_payload(ids: dict[str, str]) -> dict[str, Any]:
    checksum = fixture.fixture_checksum(
        [
            ids["orgId"],
            ids["adminUserId"],
            ids["viewerUserId"],
            ids["collectionId"],
            ids["failedDocumentId"],
            ids["failedVersionId"],
            ids["objectId"],
            ids["vectorPointId"],
        ]
    )
    return {
        "runId": ids["runId"],
        "orgId": ids["orgId"],
        "adminUserId": ids["adminUserId"],
        "viewerUserId": ids["viewerUserId"],
        "collectionId": ids["collectionId"],
        "collectionName": f"E2E Library {ids['runId']}",
        "failedDocumentId": ids["failedDocumentId"],
        "failedVersionId": ids["failedVersionId"],
        "objectIds": [ids["objectId"]],
        "vectorPointIds": [ids["vectorPointId"]],
        "checksum": checksum,
    }


def _credentials_payload(ids: dict[str, str]) -> dict[str, Any]:
    return {
        "runId": ids["runId"],
        "adminEmail": f"admin+{ids['runId']}@example.test",
        "adminPassword": "admin-secret-value",
        "viewerEmail": f"viewer+{ids['runId']}@example.test",
        "viewerPassword": "viewer-secret-value",
        "objectKeys": [ids["objectKey"]],
        "vectorPointIds": [ids["vectorPointId"]],
    }


class ProductionRefuseTests(unittest.TestCase):
    def test_setup_prod_exits_nonzero_and_writes_nothing(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-refuse-") as tmp:
            tmp_path = Path(tmp)
            manifest = tmp_path / "manifest.json"
            credentials = tmp_path / "credentials.json"
            commands = FakeCommands()
            code = fixture.main(
                [
                    "setup",
                    "--run-id",
                    "e2e-deadbeef-1",
                    "--manifest-out",
                    str(manifest),
                    "--credentials-out",
                    str(credentials),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "prod"},
            )
            self.assertNotEqual(code, 0)
            self.assertFalse(manifest.exists())
            self.assertFalse(credentials.exists())
            self.assertEqual(commands.psql_calls, [])
            self.assertEqual(commands.http_calls, [])
            self.assertEqual(commands.subprocess_calls, [])

    def test_cleanup_and_verify_clean_refuse_prod_before_io(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-refuse-clean-") as tmp:
            tmp_path = Path(tmp)
            ids = _sample_ids("e2e-cafe0001-2")
            manifest = tmp_path / "manifest.json"
            credentials = tmp_path / "credentials.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            _write_json(credentials, _credentials_payload(ids))
            commands = FakeCommands()
            cleanup_code = fixture.main(
                [
                    "cleanup",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                    "--credentials",
                    str(credentials),
                    "--api-base",
                    "http://127.0.0.1:9",
                    "--timeout-secs",
                    "1",
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "prod"},
            )
            verify_code = fixture.main(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "prod"},
            )
            self.assertNotEqual(cleanup_code, 0)
            self.assertNotEqual(verify_code, 0)
            self.assertEqual(commands.psql_calls, [])
            self.assertEqual(commands.http_calls, [])
            self.assertTrue(credentials.exists())


class CredentialsModeTests(unittest.TestCase):
    def test_setup_writes_credentials_mode_0600_and_id_only_manifest(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-setup-") as tmp:
            tmp_path = Path(tmp)
            manifest = tmp_path / "manifest.json"
            credentials = tmp_path / "credentials.json"
            run_id = "e2e-abcd1234-3"
            ids = _sample_ids(run_id)

            def psql_handler(sql: str) -> str:
                if "fixture_result" in sql or '"orgId"' in sql or "as fixture_json" in sql.lower():
                    return json.dumps(
                        {
                            "orgId": ids["orgId"],
                            "adminUserId": ids["adminUserId"],
                            "viewerUserId": ids["viewerUserId"],
                            "collectionId": ids["collectionId"],
                            "failedDocumentId": ids["failedDocumentId"],
                            "failedVersionId": ids["failedVersionId"],
                            "objectId": ids["objectId"],
                            "vectorPointId": ids["vectorPointId"],
                        }
                    )
                return "1"

            commands = FakeCommands(psql_handler=psql_handler)
            code = fixture.main(
                [
                    "setup",
                    "--run-id",
                    run_id,
                    "--manifest-out",
                    str(manifest),
                    "--credentials-out",
                    str(credentials),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertEqual(code, 0)
            self.assertTrue(manifest.exists())
            self.assertTrue(credentials.exists())
            mode = credentials.stat().st_mode & 0o777
            self.assertEqual(mode, 0o600, oct(mode))

            manifest_payload = json.loads(manifest.read_text(encoding="utf-8"))
            credentials_payload = json.loads(credentials.read_text(encoding="utf-8"))

            forbidden_manifest_keys = {
                "adminPassword",
                "viewerPassword",
                "password",
                "passwordHash",
                "accessToken",
                "refreshToken",
                "objectKeys",
                "objectKey",
            }
            self.assertTrue(forbidden_manifest_keys.isdisjoint(manifest_payload.keys()))
            self.assertIn("checksum", manifest_payload)
            self.assertEqual(manifest_payload["runId"], run_id)
            self.assertEqual(manifest_payload["collectionName"], f"E2E Library {run_id}")
            self.assertIn("adminPassword", credentials_payload)
            self.assertIn("viewerPassword", credentials_payload)
            self.assertTrue(commands.psql_calls)
            self.assertTrue(commands.subprocess_calls)
            combined = "\n".join(commands.psql_calls)
            self.assertNotIn(credentials_payload["adminPassword"], combined)
            self.assertNotIn(credentials_payload["viewerPassword"], combined)
            self.assertNotIn(commands.hash_password_result, combined)


class CleanupLeakTests(unittest.TestCase):
    def _paths(self, tmp: Path, run_id: str) -> tuple[Path, Path, dict[str, str]]:
        ids = _sample_ids(run_id)
        manifest = tmp / "manifest.json"
        credentials = tmp / "credentials.json"
        _write_json(manifest, _manifest_payload(ids), mode=0o644)
        _write_json(credentials, _credentials_payload(ids))
        return manifest, credentials, ids

    def test_verify_clean_detects_db_object_and_vector_leaks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-leaks-") as tmp:
            tmp_path = Path(tmp)
            run_id = "e2e-a11e0001-4"
            manifest, _credentials, ids = self._paths(tmp_path, run_id)

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "from orgs" in lowered and "count" in lowered:
                    return "1"
                if "from documents" in lowered and "count" in lowered:
                    return "1"
                if "from users" in lowered and "count" in lowered:
                    return "1"
                if "from collections" in lowered and "count" in lowered:
                    return "1"
                return "0"

            commands = FakeCommands(
                psql_handler=psql_handler,
                object_keys={ids["objectKey"]},
                vector_ids={ids["vectorPointId"]},
            )
            code = fixture.main(
                [
                    "verify-clean",
                    "--run-id",
                    run_id,
                    "--manifest",
                    str(manifest),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertNotEqual(code, 0)

    def test_cleanup_timeout_is_nonzero_when_api_delete_never_finishes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-timeout-") as tmp:
            tmp_path = Path(tmp)
            run_id = "e2e-dead0005-5"
            manifest, credentials, ids = self._paths(tmp_path, run_id)

            def http_handler(method: str, url: str, **_kwargs: Any) -> FakeHttpResponse:
                if method == "POST" and url.rstrip("/").endswith("/api/v1/auth/login"):
                    return FakeHttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "accessToken": "access-token-value",
                                "refreshToken": "refresh-token-value",
                            }
                        ).encode("utf-8"),
                    )
                if method == "DELETE" and "/api/v1/documents/" in url:
                    return FakeHttpResponse(status=204)
                return FakeHttpResponse(status=404, body=b"{}")

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "from documents" in lowered and "count" in lowered:
                    return "1"
                if "select state" in lowered:
                    return "tombstoned"
                return "0"

            commands = FakeCommands(
                http_handler=http_handler,
                psql_handler=psql_handler,
                object_keys={ids["objectKey"]},
                vector_ids={ids["vectorPointId"]},
            )
            times = iter([0.0, 0.25, 0.5, 0.75, 1.1, 1.2])
            with mock.patch.object(fixture, "monotonic", side_effect=lambda: next(times)):
                code = fixture.main(
                    [
                        "cleanup",
                        "--run-id",
                        run_id,
                        "--manifest",
                        str(manifest),
                        "--credentials",
                        str(credentials),
                        "--api-base",
                        "http://127.0.0.1:8787",
                        "--timeout-secs",
                        "1",
                    ],
                    commands=commands,
                    environ={"MARKHAND_PROFILE": "dev"},
                )
            self.assertNotEqual(code, 0)
            self.assertTrue(any(call["method"] == "DELETE" for call in commands.http_calls))
            self.assertTrue(credentials.exists(), "credentials remain on failed cleanup")

    def test_cleanup_idempotent_when_already_clean(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-idempotent-") as tmp:
            tmp_path = Path(tmp)
            run_id = "e2e-c1ea0001-6"
            manifest, credentials, _ids = self._paths(tmp_path, run_id)

            def http_handler(method: str, url: str, **_kwargs: Any) -> FakeHttpResponse:
                if method == "POST" and url.rstrip("/").endswith("/api/v1/auth/login"):
                    return FakeHttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "accessToken": "access-token-value",
                                "refreshToken": "refresh-token-value",
                            }
                        ).encode("utf-8"),
                    )
                if method == "DELETE":
                    return FakeHttpResponse(status=404, body=b'{"code":"not_found"}')
                return FakeHttpResponse(status=200, body=b"{}")

            commands = FakeCommands(
                http_handler=http_handler,
                psql_handler=lambda _sql: "0",
            )
            code = fixture.main(
                [
                    "cleanup",
                    "--run-id",
                    run_id,
                    "--manifest",
                    str(manifest),
                    "--credentials",
                    str(credentials),
                    "--api-base",
                    "http://127.0.0.1:8787",
                    "--timeout-secs",
                    "5",
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertEqual(code, 0)
            self.assertFalse(credentials.exists())
            verify = fixture.main(
                [
                    "verify-clean",
                    "--run-id",
                    run_id,
                    "--manifest",
                    str(manifest),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertEqual(verify, 0)
            code2 = fixture.main(
                [
                    "cleanup",
                    "--run-id",
                    run_id,
                    "--manifest",
                    str(manifest),
                    "--credentials",
                    str(credentials),
                    "--api-base",
                    "http://127.0.0.1:8787",
                    "--timeout-secs",
                    "5",
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertEqual(code2, 0)

    def test_partial_cleanup_reports_leak_ids_only(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-partial-") as tmp:
            tmp_path = Path(tmp)
            run_id = "e2e-ab270001-7"
            manifest, credentials, ids = self._paths(tmp_path, run_id)
            report_path = tmp_path / "leak-report.json"

            def http_handler(method: str, url: str, **_kwargs: Any) -> FakeHttpResponse:
                if method == "POST" and url.rstrip("/").endswith("/api/v1/auth/login"):
                    return FakeHttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "accessToken": "access-token-value",
                                "refreshToken": "refresh-token-value",
                            }
                        ).encode("utf-8"),
                    )
                if method == "DELETE":
                    return FakeHttpResponse(status=204)
                return FakeHttpResponse(status=200, body=b"{}")

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "delete from" in lowered or "session_replication_role" in lowered:
                    return ""
                if "from orgs" in lowered and "count" in lowered:
                    return "1"
                if "from documents" in lowered and "count" in lowered:
                    return "0"
                if "from users" in lowered and "count" in lowered:
                    return "0"
                if "from collections" in lowered and "count" in lowered:
                    return "0"
                if "select state" in lowered:
                    return "purged"
                return "0"

            commands = FakeCommands(
                http_handler=http_handler,
                psql_handler=psql_handler,
                object_keys={ids["objectKey"]},
                vector_ids=set(),
            )
            code = fixture.main(
                [
                    "cleanup",
                    "--run-id",
                    run_id,
                    "--manifest",
                    str(manifest),
                    "--credentials",
                    str(credentials),
                    "--api-base",
                    "http://127.0.0.1:8787",
                    "--timeout-secs",
                    "5",
                    "--leak-report-out",
                    str(report_path),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertNotEqual(code, 0)
            self.assertTrue(report_path.exists())
            report = json.loads(report_path.read_text(encoding="utf-8"))
            blob = json.dumps(report)
            self.assertNotIn("admin-secret-value", blob)
            self.assertNotIn("viewer-secret-value", blob)
            self.assertNotIn("access-token-value", blob)
            self.assertIn(ids["orgId"], blob)
            # Object keys must never appear; IDs are allowed when objects leak.
            self.assertNotIn(ids["objectKey"], blob)
            self.assertIn("orgIds", report["leaks"])

    def test_missing_compose_db_fails_closed_on_setup(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-missing-db-") as tmp:
            tmp_path = Path(tmp)
            manifest = tmp_path / "manifest.json"
            credentials = tmp_path / "credentials.json"
            commands = FakeCommands(fail_psql=True)
            code = fixture.main(
                [
                    "setup",
                    "--run-id",
                    "e2e-00db0001-8",
                    "--manifest-out",
                    str(manifest),
                    "--credentials-out",
                    str(credentials),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertNotEqual(code, 0)
            self.assertFalse(manifest.exists())
            self.assertFalse(credentials.exists())


class RunIdValidationTests(unittest.TestCase):
    def test_rejects_unsafe_run_id_before_subprocess(self) -> None:
        commands = FakeCommands()
        code = fixture.main(
            [
                "setup",
                "--run-id",
                "bad;drop table",
                "--manifest-out",
                "/tmp/nope-manifest.json",
                "--credentials-out",
                "/tmp/nope-credentials.json",
            ],
            commands=commands,
            environ={"MARKHAND_PROFILE": "dev"},
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(commands.psql_calls, [])
        self.assertEqual(commands.subprocess_calls, [])


if __name__ == "__main__":
    unittest.main()
