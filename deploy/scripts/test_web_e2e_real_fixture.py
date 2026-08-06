#!/usr/bin/env python3
"""Hermetic tests for deploy/scripts/web_e2e_real_fixture.py (P2-20 Task 1)."""

from __future__ import annotations

import json
import io
import os
import subprocess
import sys
import tempfile
import unittest
import uuid
from contextlib import redirect_stderr
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import web_e2e_real_fixture as fixture  # noqa: E402
import web_e2e_fixture_database as fixture_database  # noqa: E402


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
    timeout_calls: list[float] = field(default_factory=list)
    object_probe_keys: list[str] = field(default_factory=list)
    qdrant_points_by_collection: dict[str, set[str]] = field(default_factory=dict)
    qdrant_probe_collections: list[str] = field(default_factory=list)
    enforce_immutable_cleanup: bool = False

    def hash_password(self, password: str, *, timeout: float = 30.0) -> str:
        self.timeout_calls.append(timeout)
        self.subprocess_calls.append(["native-argon2", "<redacted>"])
        if not password:
            raise RuntimeError("empty password")
        return self.hash_password_result

    def psql(
        self,
        sql: str,
        *,
        timeout: float = 30.0,
        redact: list[str] | None = None,
    ) -> str:
        self.timeout_calls.append(timeout)
        recorded = sql
        for secret in redact or []:
            if secret:
                recorded = recorded.replace(secret, "<redacted>")
        self.psql_calls.append(recorded)
        if self.fail_psql:
            raise fixture.FixtureError("compose/db unavailable")
        if self.enforce_immutable_cleanup and "fixture_hard_delete_reviewed_order" in sql:
            lowered = sql.lower()
            for table in (
                "conflict_evidence",
                "conflicts",
                "derived_artifacts",
                "index_metadata",
                "document_versions",
                "audit_log",
            ):
                disable = f"alter table {table} disable trigger user"
                delete = f"delete from {table}"
                enable = f"alter table {table} enable trigger user"
                if not (
                    disable in lowered
                    and delete in lowered
                    and enable in lowered
                    and lowered.index(disable) < lowered.index(delete) < lowered.index(enable)
                ):
                    raise fixture.FixtureError("immutable cleanup trigger violation")
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
                "body": (
                    b"<redacted>"
                    if method == "POST" and url.rstrip("/").endswith("/api/v1/auth/login")
                    else body
                ),
                "timeout": timeout,
            }
        )
        if self.http_handler is not None:
            return self.http_handler(method, url, headers=headers, body=body, timeout=timeout)
        return FakeHttpResponse(status=204)

    def object_exists(self, key: str, *, timeout: float = 30.0) -> bool:
        self.timeout_calls.append(timeout)
        self.object_probe_keys.append(key)
        return key in self.object_keys

    def qdrant_point_ids(
        self,
        collections: list[str],
        org_id: str,
        *,
        timeout: float = 30.0,
    ) -> list[str]:
        _ = org_id
        self.timeout_calls.append(timeout)
        discovered = {
            collection
            for collection in self.qdrant_points_by_collection
            if collection.startswith("markhand_chunks_")
            and len(collection) == len("markhand_chunks_") + 64
            and all(character in "0123456789abcdef" for character in collection[16:])
        }
        scanned = sorted(set(collections) | discovered)
        self.qdrant_probe_collections.extend(scanned)
        points = set(self.vector_ids)
        for collection in scanned:
            points.update(self.qdrant_points_by_collection.get(collection, set()))
        return sorted(points)

    def sleep(self, seconds: float) -> None:
        self.sleep_calls.append(seconds)


def _write_json(path: Path, payload: dict[str, Any], mode: int = 0o600) -> None:
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(mode)


def _sample_ids(run_id: str) -> dict[str, str]:
    base = uuid.uuid5(uuid.NAMESPACE_URL, f"markhand-e2e-fixture:{run_id}")
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
                    "e2e-deadbeef0001-1",
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
            ids = _sample_ids("e2e-cafe00010002-2")
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
            run_id = "e2e-abcd12340003-3"
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
            run_id = "e2e-a11e00010004-4"
            manifest, _credentials, ids = self._paths(tmp_path, run_id)

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "fixture_resource_inventory" in lowered:
                    return json.dumps(
                        {
                            "documents": [
                                {"id": ids["failedDocumentId"], "state": "failed"}
                            ],
                            "objects": [
                                {
                                    "resourceId": ids["objectId"],
                                    "key": ids["objectKey"],
                                }
                            ],
                            "signatures": [],
                        }
                    )
                if "fixture_org_table_leaks" in lowered:
                    return json.dumps(
                        {
                            "orgs": [ids["orgId"]],
                            "documents": [ids["failedDocumentId"]],
                            "users": [ids["adminUserId"], ids["viewerUserId"]],
                            "collections": [ids["collectionId"]],
                        }
                    )
                return ""

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
            run_id = "e2e-dead00050005-5"
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
                if "fixture_resource_inventory" in lowered:
                    return json.dumps(
                        {
                            "documents": [
                                {"id": ids["failedDocumentId"], "state": "tombstoned"}
                            ],
                            "objects": [
                                {
                                    "resourceId": ids["objectId"],
                                    "key": ids["objectKey"],
                                }
                            ],
                            "signatures": [],
                        }
                    )
                if "fixture_org_table_leaks" in lowered:
                    return json.dumps(
                        {"documents": [ids["failedDocumentId"]], "orgs": [ids["orgId"]]}
                    )
                return ""

            commands = FakeCommands(
                http_handler=http_handler,
                psql_handler=psql_handler,
                object_keys={ids["objectKey"]},
                vector_ids={ids["vectorPointId"]},
            )
            tick = {"value": -0.1}

            def advancing_clock() -> float:
                tick["value"] += 0.1
                return tick["value"]

            with mock.patch.object(
                fixture.fixture_cli, "monotonic", side_effect=advancing_clock
            ):
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
            run_id = "e2e-c1ea00010006-6"
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

            def clean_psql(sql: str) -> str:
                if "fixture_resource_inventory" in sql.lower():
                    return '{"documents":[],"objects":[],"signatures":[]}'
                if "fixture_org_table_leaks" in sql.lower():
                    return "{}"
                return ""

            commands = FakeCommands(http_handler=http_handler, psql_handler=clean_psql)
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
            run_id = "e2e-ab2700010007-7"
            manifest, credentials, ids = self._paths(tmp_path, run_id)

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "fixture_resource_inventory" in lowered:
                    return json.dumps(
                        {
                            "documents": [],
                            "objects": [
                                {
                                    "resourceId": ids["objectId"],
                                    "key": ids["objectKey"],
                                }
                            ],
                            "signatures": [],
                        }
                    )
                if "fixture_org_table_leaks" in lowered:
                    return json.dumps({"orgs": [ids["orgId"]]})
                return ""

            commands = FakeCommands(
                psql_handler=psql_handler,
                object_keys={ids["objectKey"]},
                vector_ids=set(),
            )
            stderr = io.StringIO()
            with redirect_stderr(stderr):
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
            self.assertNotEqual(code, 0)
            blob = stderr.getvalue()
            self.assertNotIn("admin-secret-value", blob)
            self.assertNotIn("viewer-secret-value", blob)
            self.assertNotIn("access-token-value", blob)
            self.assertIn(ids["orgId"], blob)
            # Object keys must never appear; IDs are allowed when objects leak.
            self.assertNotIn(ids["objectKey"], blob)
            self.assertIn("database_rows", blob)

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
                    "e2e-00db00010008-8",
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


class ReviewFixRedTests(unittest.TestCase):
    """Blocking review regressions; each test was added before its fix."""

    def _invoke(
        self,
        argv: list[str],
        *,
        commands: FakeCommands | None = None,
        environ: dict[str, str] | None = None,
    ) -> tuple[int, str, FakeCommands]:
        active = commands or FakeCommands()
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            code = fixture.main(argv, commands=active, environ=environ or {"MARKHAND_PROFILE": "dev"})
        return code, stderr.getvalue(), active

    def test_config_file_prod_and_invalid_config_refuse_before_commands_or_writes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-config-refuse-") as tmp:
            root = Path(tmp)
            manifest = root / "manifest.json"
            credentials = root / "credentials.json"
            prod_config = root / "prod.json"
            prod_config.write_text('{"profile":"prod"}', encoding="utf-8")
            argv = [
                "setup",
                "--run-id",
                "e2e-0123456789ab-1",
                "--manifest-out",
                str(manifest),
                "--credentials-out",
                str(credentials),
            ]
            for config_text in ('{"profile":"prod"}', "{invalid", '{"profile":7}'):
                prod_config.write_text(config_text, encoding="utf-8")
                commands = FakeCommands()
                code, _stderr, _ = self._invoke(
                    argv,
                    commands=commands,
                    environ={"MARKHAND_CONFIG_FILE": str(prod_config)},
                )
                self.assertNotEqual(code, 0)
                self.assertEqual(commands.subprocess_calls, [])
                self.assertEqual(commands.psql_calls, [])
                self.assertFalse(manifest.exists())
                self.assertFalse(credentials.exists())

    def test_environment_profile_overrides_valid_config_profile(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-config-precedence-") as tmp:
            root = Path(tmp)
            config = root / "config.json"
            config.write_text('{"profile":"prod"}', encoding="utf-8")
            commands = FakeCommands(fail_psql=True)
            code, stderr, _ = self._invoke(
                [
                    "setup",
                    "--run-id",
                    "e2e-0123456789ab-2",
                    "--manifest-out",
                    str(root / "manifest.json"),
                    "--credentials-out",
                    str(root / "credentials.json"),
                ],
                commands=commands,
                environ={
                    "MARKHAND_CONFIG_FILE": str(config),
                    "MARKHAND_PROFILE": "dev",
                },
            )
            self.assertNotEqual(code, 0)
            self.assertNotIn("refusing effective production profile", stderr)
            self.assertTrue(commands.psql_calls, "dev env override must reach database setup")

    def test_cleanup_enumerates_and_api_deletes_every_org_document(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-all-docs-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-123456789abc-3")
            second_document = str(uuid.uuid4())
            manifest = root / "manifest.json"
            credentials = root / "credentials.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            _write_json(credentials, _credentials_payload(ids))

            state = {"documents": [ids["failedDocumentId"], second_document]}

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "fixture_resource_inventory" in lowered:
                    return json.dumps(
                        {
                            "documents": [
                                {"id": document_id, "state": "failed"}
                                for document_id in state["documents"]
                            ],
                            "objects": [],
                            "signatures": [],
                        }
                    )
                if "fixture_org_table_leaks" in lowered:
                    return "{}" if not state["documents"] else json.dumps(
                        {"documents": state["documents"]}
                    )
                if "delete from documents" in lowered:
                    state["documents"].clear()
                    return ""
                return "0"

            def http_handler(method: str, url: str, **_kwargs: Any) -> FakeHttpResponse:
                if method == "POST" and url.endswith("/api/v1/auth/login"):
                    return FakeHttpResponse(
                        200, body=b'{"accessToken":"access-token-value"}'
                    )
                if method == "DELETE":
                    document_id = url.rsplit("/", 1)[-1]
                    if document_id in state["documents"]:
                        state["documents"].remove(document_id)
                    return FakeHttpResponse(204)
                return FakeHttpResponse(200, body=b"{}")

            commands = FakeCommands(psql_handler=psql_handler, http_handler=http_handler)
            code, _stderr, commands = self._invoke(
                [
                    "cleanup",
                    "--run-id",
                    ids["runId"],
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
            )
            self.assertEqual(code, 0)
            deleted = {
                call["url"].rsplit("/", 1)[-1]
                for call in commands.http_calls
                if call["method"] == "DELETE"
            }
            self.assertEqual(deleted, {ids["failedDocumentId"], second_document})

    def test_probe_errors_are_not_treated_as_confirmed_absence(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-probe-errors-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-23456789abcd-4")
            manifest = root / "manifest.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)

            class BrokenProbeCommands(FakeCommands):
                def object_exists(self, key: str, *, timeout: float = 30.0) -> bool:
                    raise fixture.FixtureProbeError("minio probe unavailable")

                def qdrant_point_ids(
                    self,
                    collections: list[str],
                    org_id: str,
                    *,
                    timeout: float = 30.0,
                ) -> list[str]:
                    raise fixture.FixtureProbeError("qdrant probe unavailable")

            code, stderr, _ = self._invoke(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=BrokenProbeCommands(
                    psql_handler=lambda sql: (
                        '{"documents":[],"objects":[],"signatures":[]}'
                        if "fixture_resource_inventory" in sql.lower()
                        else "{}"
                    )
                ),
            )
            self.assertNotEqual(code, 0)
            self.assertIn("probe unavailable", stderr)

    def test_cleanup_uses_one_decreasing_overall_deadline(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-overall-deadline-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-3456789abcde-5")
            manifest = root / "manifest.json"
            credentials = root / "credentials.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            _write_json(credentials, _credentials_payload(ids))

            class TimeoutRecordingCommands(FakeCommands):
                timeouts: list[float]

                def __init__(self) -> None:
                    super().__init__()
                    self.timeouts = []

                def psql(
                    self,
                    sql: str,
                    *,
                    timeout: float,
                    redact: list[str] | None = None,
                ) -> str:
                    self.timeouts.append(timeout)
                    if "fixture_resource_inventory" in sql.lower():
                        return '{"documents":[],"objects":[],"signatures":[]}'
                    if "fixture_org_table_leaks" in sql.lower():
                        return "{}"
                    return "0"

                def object_exists(self, key: str, *, timeout: float) -> bool:
                    self.timeouts.append(timeout)
                    return False

                def qdrant_point_ids(
                    self,
                    collections: list[str],
                    org_id: str,
                    *,
                    timeout: float,
                ) -> list[str]:
                    self.timeouts.append(timeout)
                    return []

            commands = TimeoutRecordingCommands()
            ticks = iter([0.0, 0.1, 0.4, 0.7, 1.0, 1.2, 1.5, 1.7, 1.9])
            with mock.patch.object(
                fixture.fixture_cli, "monotonic", side_effect=lambda: next(ticks)
            ):
                code, _stderr, _ = self._invoke(
                    [
                        "cleanup",
                        "--run-id",
                        ids["runId"],
                        "--manifest",
                        str(manifest),
                        "--credentials",
                        str(credentials),
                        "--api-base",
                        "http://127.0.0.1:8787",
                        "--timeout-secs",
                        "2",
                    ],
                    commands=commands,
                )
            self.assertEqual(code, 0)
            self.assertGreaterEqual(len(commands.timeouts), 3)
            self.assertTrue(
                all(left > right for left, right in zip(commands.timeouts, commands.timeouts[1:])),
                commands.timeouts,
            )

    def test_live_subprocess_argv_contains_no_password_sql_credentials_or_object_key(self) -> None:
        commands = fixture.LiveCommands(
            environ={
                "MARKHAND_MINIO_URL": "http://127.0.0.1:9000",
                "MARKHAND_MINIO_ACCESS_KEY": "minio-access-secret",
                "MARKHAND_MINIO_SECRET_KEY": "minio-password-secret",
            }
        )
        commands.http = mock.Mock(return_value=fixture.HttpResponse(404, {}, b""))
        recorded: list[tuple[list[str], dict[str, Any]]] = []
        saw_password_stdin = False
        saw_sql_stdin = False

        def fake_run(argv: list[str], **kwargs: Any) -> Any:
            nonlocal saw_password_stdin, saw_sql_stdin
            if argv[0] == "cargo":
                saw_password_stdin = kwargs.get("input") == secret_password + "\n"
                stdout = PasswordHelperAdapterRedTests.VALID_HASH
            else:
                saw_sql_stdin = kwargs.get("input") == secret_sql
                stdout = ""
            safe_kwargs = dict(kwargs)
            safe_kwargs["input"] = "<redacted>"
            recorded.append((list(argv), safe_kwargs))
            return mock.Mock(returncode=0, stdout=stdout, stderr="")

        secret_password = "runtime-password-secret"
        secret_sql = "SELECT 'password-hash-secret', 'private/object-key';"
        object_key = "private/object-key"
        with mock.patch.object(fixture.subprocess, "run", side_effect=fake_run):
            commands.hash_password(secret_password)
            commands.psql(secret_sql)
            commands.object_exists(object_key)

        argv_blob = "\n".join(" ".join(argv) for argv, _kwargs in recorded)
        self.assertEqual(len(recorded), 2)
        self.assertTrue(saw_password_stdin)
        self.assertTrue(saw_sql_stdin)
        self.assertTrue(all(kwargs["input"] == "<redacted>" for _argv, kwargs in recorded))
        for forbidden in (
            secret_password,
            secret_sql,
            "password-hash-secret",
            "minio-access-secret",
            "minio-password-secret",
            object_key,
        ):
            self.assertNotIn(forbidden, argv_blob)

    def test_setup_compensates_database_when_output_publication_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-setup-compensation-") as tmp:
            root = Path(tmp)
            run_id = "e2e-456789abcdef-6"
            ids = _sample_ids(run_id)
            state = {"org": True}

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "as fixture_json" in lowered:
                    state["org"] = True
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
                if "delete from orgs" in lowered:
                    state["org"] = False
                    return ""
                if "fixture_org_table_leaks" in lowered:
                    return '{"orgs":["%s"]}' % ids["orgId"] if state["org"] else "{}"
                return ""

            original_write = fixture.fixture_cli._atomic_write_json

            def fail_manifest(path: Path, payload: dict[str, Any], *, mode: int) -> None:
                if path.name == "manifest.json":
                    raise OSError("staging failure")
                original_write(path, payload, mode=mode)

            with mock.patch.object(
                fixture.fixture_cli, "_atomic_write_json", side_effect=fail_manifest
            ):
                code, _stderr, commands = self._invoke(
                    [
                        "setup",
                        "--run-id",
                        run_id,
                        "--manifest-out",
                        str(root / "manifest.json"),
                        "--credentials-out",
                        str(root / "credentials.json"),
                    ],
                    commands=FakeCommands(psql_handler=psql_handler),
                )
            self.assertNotEqual(code, 0)
            self.assertFalse(state["org"], "database fixture must be compensated")
            self.assertTrue(
                any("delete from orgs" in sql.lower() for sql in commands.psql_calls)
            )

    def test_already_clean_removes_stale_credentials_without_auth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-stale-credentials-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-56789abcdef0-7")
            manifest = root / "manifest.json"
            credentials = root / "credentials.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            _write_json(credentials, _credentials_payload(ids))
            def clean_psql(sql: str) -> str:
                if "fixture_resource_inventory" in sql.lower():
                    return '{"documents":[],"objects":[],"signatures":[]}'
                if "fixture_org_table_leaks" in sql.lower():
                    return "{}"
                return ""

            commands = FakeCommands(psql_handler=clean_psql)
            code, _stderr, commands = self._invoke(
                [
                    "cleanup",
                    "--run-id",
                    ids["runId"],
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
            )
            self.assertEqual(code, 0)
            self.assertFalse(credentials.exists())
            self.assertEqual(commands.http_calls, [])

    def test_db_only_residual_without_documents_cleans_without_credentials(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-db-only-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-6789abcdef01-8")
            manifest = root / "manifest.json"
            credentials = root / "missing-credentials.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            state = {"org": True}

            def psql_handler(sql: str) -> str:
                lowered = sql.lower()
                if "fixture_resource_inventory" in lowered:
                    return '{"documents":[],"objects":[],"signatures":[]}'
                if "fixture_org_table_leaks" in lowered:
                    return '{"orgs":["%s"]}' % ids["orgId"] if state["org"] else "{}"
                if "delete from orgs" in lowered:
                    state["org"] = False
                    return ""
                return ""

            commands = FakeCommands(psql_handler=psql_handler)
            code, _stderr, commands = self._invoke(
                [
                    "cleanup",
                    "--run-id",
                    ids["runId"],
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
            )
            self.assertEqual(code, 0)
            self.assertFalse(state["org"])
            self.assertEqual(commands.http_calls, [])

    def test_cli_rejects_removed_leak_option_and_output_aliases(self) -> None:
        parser = fixture.build_parser()
        with self.assertRaises(SystemExit):
            parser.parse_args(
                [
                    "cleanup",
                    "--run-id",
                    "e2e-789abcdef012-9",
                    "--manifest",
                    "/tmp/manifest.json",
                    "--credentials",
                    "/tmp/credentials.json",
                    "--api-base",
                    "http://127.0.0.1:8787",
                    "--timeout-secs",
                    "5",
                    "--leak-report-out",
                    "/tmp/leaks.json",
                ]
            )

        with tempfile.TemporaryDirectory(prefix="fixture-alias-") as tmp:
            shared = Path(tmp) / "shared.json"
            code, _stderr, commands = self._invoke(
                [
                    "setup",
                    "--run-id",
                    "e2e-789abcdef012-10",
                    "--manifest-out",
                    str(shared),
                    "--credentials-out",
                    str(shared),
                ]
            )
            self.assertNotEqual(code, 0)
            self.assertEqual(commands.subprocess_calls, [])
            self.assertEqual(commands.psql_calls, [])

    def test_default_leak_evidence_is_identifier_only(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-default-leaks-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-89abcdef0123-11")
            manifest = root / "manifest.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)

            def psql_handler(sql: str) -> str:
                if "fixture_org_table_leaks" in sql.lower():
                    return json.dumps({"documents": [ids["failedDocumentId"]]})
                if "fixture_resource_inventory" in sql.lower():
                    return '{"documents":[],"objects":[],"signatures":[]}'
                return "0"

            code, stderr, _ = self._invoke(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=FakeCommands(psql_handler=psql_handler),
            )
            self.assertNotEqual(code, 0)
            self.assertIn(ids["failedDocumentId"], stderr)
            for forbidden in (
                "admin-secret-value",
                "viewer-secret-value",
                ids["objectKey"],
                "SELECT ",
            ):
                self.assertNotIn(forbidden, stderr)

    def test_qdrant_collections_come_from_database_signatures(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-qdrant-signatures-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-90abcdef0123-12")
            manifest = root / "manifest.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            signature = "a" * 64

            class SignatureCommands(FakeCommands):
                collections: list[str]

                def __init__(self) -> None:
                    super().__init__()
                    self.collections = []

                def psql(self, sql: str, **_kwargs: Any) -> str:
                    if "fixture_resource_inventory" in sql.lower():
                        return json.dumps(
                            {
                                "documents": [],
                                "objects": [],
                                "signatures": [signature],
                            }
                        )
                    if "fixture_org_table_leaks" in sql.lower():
                        return "{}"
                    return "0"

                def qdrant_point_ids(
                    self,
                    collections: list[str],
                    org_id: str,
                    *,
                    timeout: float,
                ) -> list[str]:
                    self.collections = collections
                    return []

            commands = SignatureCommands()
            code, _stderr, _ = self._invoke(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=commands,
            )
            self.assertEqual(code, 0)
            self.assertEqual(commands.collections, ["markhand_chunks_" + signature])

    def test_verification_reports_rows_from_any_org_scoped_table(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-all-org-tables-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-a0bcdef01234-13")
            manifest = root / "manifest.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)

            def psql_handler(sql: str) -> str:
                if "fixture_resource_inventory" in sql.lower():
                    return '{"documents":[],"objects":[],"signatures":[]}'
                if "fixture_org_table_leaks" in sql.lower():
                    return json.dumps({"future_org_table": [ids["orgId"]]})
                return "0"

            code, stderr, _ = self._invoke(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=FakeCommands(psql_handler=psql_handler),
            )
            self.assertNotEqual(code, 0)
            self.assertIn("future_org_table", stderr)
            self.assertIn(ids["orgId"], stderr)

    def test_hard_delete_preserves_fk_triggers_in_reviewed_order(self) -> None:
        ids = _sample_ids("e2e-b0cdef012345-14")
        commands = FakeCommands()
        fixture._hard_delete_run_rows(commands, fixture._manifest_ids(_manifest_payload(ids)))
        sql = "\n".join(commands.psql_calls).lower()
        expected_order = [
            "delete from conflict_evidence",
            "delete from conflicts",
            "delete from claims",
            "delete from chunks",
            "delete from derived_artifacts",
            "delete from document_versions",
            "delete from documents",
            "delete from collections",
            "delete from projects",
            "delete from orgs",
            "delete from users",
        ]
        positions = [sql.index(marker) for marker in expected_order]
        self.assertEqual(positions, sorted(positions))
        self.assertNotIn("session_replication_role", sql)
        for table in (
            "conflict_evidence",
            "conflicts",
            "derived_artifacts",
            "index_metadata",
            "document_versions",
            "audit_log",
        ):
            disable = sql.index(f"alter table {table} disable trigger user")
            delete = sql.index(f"delete from {table}")
            enable = sql.index(f"alter table {table} enable trigger user")
            self.assertLess(disable, delete)
            self.assertLess(delete, enable)
            # ENABLE must be after COMMIT so pending trigger events are cleared
            # (document_versions deferred invariants + DML while USER triggers off).
            self.assertLess(delete, sql.rfind("commit;", 0, enable))

    def test_run_identity_is_bounded_and_suffix_uses_full_run_id(self) -> None:
        left = "e2e-" + "a" * 39 + "0-1234567890"
        right = "e2e-" + "a" * 39 + "1-1234567890"
        fixture.validate_run_id(left)
        fixture.validate_run_id(right)
        self.assertNotEqual(fixture._slug_for_run(left), fixture._slug_for_run(right))
        self.assertLessEqual(len(fixture._slug_for_run(left)), 63)
        self.assertNotEqual(fixture._email_for_run("admin", left), fixture._email_for_run("admin", right))
        self.assertLessEqual(len(fixture._email_for_run("admin", left)), 254)
        with self.assertRaises(fixture.FixtureError):
            fixture.validate_run_id("e2e-" + "b" * 40 + "-" + "9" * 11)


class SecondReviewRedTests(unittest.TestCase):
    def test_audit_log_cleanup_models_append_only_trigger(self) -> None:
        ids = _sample_ids("e2e-c0def0123456-15")
        commands = FakeCommands(enforce_immutable_cleanup=True)
        fixture._hard_delete_run_rows(commands, fixture._manifest_ids(_manifest_payload(ids)))
        sql = "\n".join(commands.psql_calls).lower()
        self.assertLess(
            sql.index("alter table audit_log disable trigger user"),
            sql.index("delete from audit_log"),
        )
        self.assertLess(
            sql.index("delete from audit_log"),
            sql.index("alter table audit_log enable trigger user"),
        )

    def test_all_object_keys_are_probed_when_resource_id_is_shared(self) -> None:
        with tempfile.TemporaryDirectory(prefix="fixture-shared-object-id-") as tmp:
            root = Path(tmp)
            ids = _sample_ids("e2e-d0ef01234567-16")
            manifest = root / "manifest.json"
            _write_json(manifest, _manifest_payload(ids), mode=0o644)
            first_key = "documents/first-key"
            second_key = "documents/second-key"

            def psql_handler(sql: str) -> str:
                if "fixture_resource_inventory" in sql.lower():
                    return json.dumps(
                        {
                            "documents": [],
                            "objects": [
                                {"resourceId": ids["failedVersionId"], "key": first_key},
                                {"resourceId": ids["failedVersionId"], "key": second_key},
                            ],
                            "signatures": [],
                        }
                    )
                if "fixture_org_table_leaks" in sql.lower():
                    return "{}"
                return ""

            commands = FakeCommands(
                psql_handler=psql_handler,
                object_keys={first_key, second_key},
            )
            code = fixture.main(
                [
                    "verify-clean",
                    "--run-id",
                    ids["runId"],
                    "--manifest",
                    str(manifest),
                ],
                commands=commands,
                environ={"MARKHAND_PROFILE": "dev"},
            )
            self.assertNotEqual(code, 0)
            self.assertEqual(commands.object_probe_keys.count(first_key), 1)
            self.assertEqual(commands.object_probe_keys.count(second_key), 1)

    def test_live_qdrant_scans_every_validated_listed_collection(self) -> None:
        org_id = str(uuid.uuid4())
        first_collection = "markhand_chunks_" + "1" * 64
        second_collection = "markhand_chunks_" + "2" * 64
        invalid_collection = "markhand_chunks_" + "A" * 64
        first_point = str(uuid.uuid4())
        second_point = str(uuid.uuid4())
        commands = fixture.LiveCommands(
            environ={"MARKHAND_QDRANT_URL": "http://qdrant.test"}
        )
        requested_urls: list[str] = []

        def http_handler(
            method: str,
            url: str,
            *,
            headers: dict[str, str] | None = None,
            body: bytes | None = None,
            timeout: float,
        ) -> fixture.HttpResponse:
            _ = headers, body, timeout
            requested_urls.append(url)
            if method == "GET" and url.endswith("/collections"):
                return fixture.HttpResponse(
                    200,
                    {},
                    json.dumps(
                        {
                            "result": {
                                "collections": [
                                    {"name": first_collection},
                                    {"name": second_collection},
                                    {"name": invalid_collection},
                                    {"name": "other_collection"},
                                ]
                            }
                        }
                    ).encode("utf-8"),
                )
            point_id = first_point if first_collection in url else second_point
            return fixture.HttpResponse(
                200,
                {},
                json.dumps(
                    {
                        "result": {
                            "points": [{"id": point_id}],
                            "next_page_offset": None,
                        }
                    }
                ).encode("utf-8"),
            )

        commands.http = http_handler
        point_ids = commands.qdrant_point_ids(
            [first_collection],
            org_id,
            timeout=5,
        )
        self.assertEqual(point_ids, sorted([first_point, second_point]))
        self.assertTrue(any(second_collection in url for url in requested_urls))
        self.assertFalse(any(invalid_collection in url for url in requested_urls))

    def test_qdrant_fake_is_collection_sensitive(self) -> None:
        first_collection = "markhand_chunks_" + "3" * 64
        second_collection = "markhand_chunks_" + "4" * 64
        invalid_collection = "markhand_chunks_" + "G" * 64
        first_point = str(uuid.uuid4())
        second_point = str(uuid.uuid4())
        ignored_point = str(uuid.uuid4())
        commands = FakeCommands(
            qdrant_points_by_collection={
                first_collection: {first_point},
                second_collection: {second_point},
                invalid_collection: {ignored_point},
            }
        )
        result = commands.qdrant_point_ids(
            [first_collection],
            str(uuid.uuid4()),
            timeout=5,
        )
        self.assertEqual(result, sorted([first_point, second_point]))
        self.assertEqual(
            commands.qdrant_probe_collections,
            [first_collection, second_collection],
        )

    def test_setup_parse_or_ambiguous_failure_compensates_expected_ids(self) -> None:
        scenarios: list[Any] = ["{bad-json", "", fixture.FixtureError("psql timed out")]
        for index, setup_result in enumerate(scenarios, start=17):
            with self.subTest(setup_result=repr(setup_result)):
                with tempfile.TemporaryDirectory(prefix="fixture-parse-compensation-") as tmp:
                    root = Path(tmp)
                    run_id = f"e2e-e0f012345678-{index}"
                    state = {"org": False, "setup_calls": 0}

                    def psql_handler(sql: str) -> str:
                        lowered = sql.lower()
                        if "fixture_setup_rows" in lowered:
                            state["org"] = True
                            state["setup_calls"] += 1
                            if isinstance(setup_result, Exception):
                                raise setup_result
                            return setup_result
                        if "fixture_hard_delete_reviewed_order" in lowered:
                            state["org"] = False
                            return ""
                        if "fixture_org_table_leaks" in lowered:
                            return (
                                json.dumps({"orgs": [str(uuid.uuid4())]})
                                if state["org"]
                                else "{}"
                            )
                        return ""

                    commands = FakeCommands(psql_handler=psql_handler)
                    code = fixture.main(
                        [
                            "setup",
                            "--run-id",
                            run_id,
                            "--manifest-out",
                            str(root / "manifest.json"),
                            "--credentials-out",
                            str(root / "credentials.json"),
                        ],
                        commands=commands,
                        environ={"MARKHAND_PROFILE": "dev"},
                    )
                    self.assertNotEqual(code, 0)
                    self.assertEqual(state["setup_calls"], 1)
                    self.assertFalse(state["org"], "ambiguous setup must be compensated")
                    self.assertTrue(
                        any(
                            "fixture_hard_delete_reviewed_order" in sql
                            for sql in commands.psql_calls
                        )
                    )


class PasswordHelperAdapterRedTests(unittest.TestCase):
    VALID_HASH = (
        "$argon2id$v=19$m=19456,t=2,p=1$"
        "DJuP5fQiiu8+OsqZhAX0Sw$"
        "YQIhOhW0a/xYGU8DZf94y4YIaKmD4TpMsjWnb8yuv7g"
    )

    def test_password_uses_exact_stdin_helper_command_without_secret_retention(self) -> None:
        commands = fixture.LiveCommands(environ={"SAFE_SETTING": "fixture"})
        secret = "runtime-password-must-not-leak"
        recorded: list[tuple[list[str], dict[str, Any]]] = []
        saw_expected_stdin = False

        def fake_run(argv: list[str], **kwargs: Any) -> Any:
            nonlocal saw_expected_stdin
            saw_expected_stdin = kwargs.get("input") == secret + "\n"
            safe_kwargs = dict(kwargs)
            if "input" in safe_kwargs:
                safe_kwargs["input"] = "<redacted>"
            recorded.append((list(argv), safe_kwargs))
            return mock.Mock(returncode=0, stdout=self.VALID_HASH, stderr="")

        with mock.patch.object(fixture.subprocess, "run", side_effect=fake_run):
            result = commands.hash_password(secret, timeout=7.5)

        self.assertEqual(result, self.VALID_HASH)
        self.assertTrue(saw_expected_stdin)
        self.assertEqual(
            recorded[0][0],
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "fileconv-server",
                "--bin",
                "dev-hash-password",
                "--",
                "--stdin",
            ],
        )
        self.assertEqual(recorded[0][1]["input"], "<redacted>")
        self.assertEqual(recorded[0][1]["timeout"], 7.5)
        recorded_blob = repr(recorded)
        self.assertNotIn(secret, recorded_blob)
        self.assertNotIn(self.VALID_HASH, recorded_blob)
        self.assertNotIn(secret, repr(recorded[0][1]["env"]))

    def test_password_helper_timeout_nonzero_and_invalid_output_fail_closed(self) -> None:
        commands = fixture.LiveCommands(environ={})
        secret = "never-echo-this-password"

        failures = [
            subprocess.TimeoutExpired(cmd=["cargo"], timeout=0.01),
            mock.Mock(returncode=1, stdout="", stderr=secret),
            mock.Mock(returncode=0, stdout="", stderr=""),
            mock.Mock(returncode=0, stdout="not-a-phc", stderr=""),
            mock.Mock(
                returncode=0,
                stdout="$argon2id$v=19$m=1,t=1,p=1$short$short",
                stderr="",
            ),
        ]
        for outcome in failures:
            with self.subTest(outcome=type(outcome).__name__):
                side_effect = outcome if isinstance(outcome, Exception) else None
                return_value = None if side_effect else outcome
                with mock.patch.object(
                    fixture.subprocess,
                    "run",
                    side_effect=side_effect,
                    return_value=return_value,
                ):
                    with self.assertRaises(fixture.FixtureError) as raised:
                        commands.hash_password(secret, timeout=0.5)
                self.assertNotIn(secret, str(raised.exception))


if __name__ == "__main__":
    unittest.main()
