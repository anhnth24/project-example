#!/usr/bin/env python3
"""Unit contracts for independent O03 green-target attestation."""

from __future__ import annotations

import json
import sys
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest import mock

LIB = Path(__file__).resolve().parent / "lib"
sys.path.insert(0, str(LIB))


class GreenAttestationTests(unittest.TestCase):
    def test_build_requires_every_independent_check(self) -> None:
        from green_attestation import GreenAttestationError, build_attestation

        with self.assertRaises(GreenAttestationError):
            build_attestation(
                manifest_sha256="a" * 64,
                fence_epoch="11111111-1111-1111-1111-111111111111",
                target={
                    "pgDatabase": "green",
                    "minioEndpoint": "http://minio:9000",
                    "minioBucket": "green-bucket",
                    "qdrantUrl": "http://qdrant-green:6333",
                    "qdrantCollection": "markhand_chunks_" + "b" * 64,
                },
                checks={
                    "manifestAuthenticated": True,
                    "postgresConsistent": True,
                    "minioConsistent": True,
                    "qdrantConsistent": False,
                    "crossStoreRefsConsistent": True,
                    "restoreFenceMatches": True,
                },
                verified_at_epoch=100,
            )

    def test_attestation_digest_is_stable_and_secret_free(self) -> None:
        from green_attestation import build_attestation

        kwargs = {
            "manifest_sha256": "a" * 64,
            "fence_epoch": "11111111-1111-1111-1111-111111111111",
            "target": {
                "pgDatabase": "green",
                "minioEndpoint": "http://minio:9000",
                "minioBucket": "green-bucket",
                "qdrantUrl": "http://qdrant-green:6333",
                "qdrantCollection": "markhand_chunks_" + "b" * 64,
            },
            "checks": {
                "manifestAuthenticated": True,
                "postgresConsistent": True,
                "minioConsistent": True,
                "qdrantConsistent": True,
                "crossStoreRefsConsistent": True,
                "restoreFenceMatches": True,
            },
            "verified_at_epoch": 100,
        }
        first, first_digest = build_attestation(**kwargs)
        second, second_digest = build_attestation(**kwargs)

        self.assertEqual(first, second)
        self.assertEqual(first_digest, second_digest)
        self.assertRegex(first_digest, r"^[a-f0-9]{64}$")
        encoded = json.dumps(first, sort_keys=True)
        self.assertNotIn("password", encoded.lower())
        self.assertNotIn("secret", encoded.lower())
        self.assertNotIn("token", encoded.lower())

    def test_clear_fence_sql_binds_epoch_and_digest(self) -> None:
        from green_attestation import clear_fence_sql

        sql = clear_fence_sql()
        self.assertIn("attestation_sha256 = :'digest'", sql)
        self.assertIn("reason LIKE '%fence_epoch=' || :'epoch' || '%'", sql)
        self.assertIn("active = true", sql)
        self.assertIn("RETURNING name", sql)

    def test_psql_sends_sql_on_stdin_so_variables_are_substituted(self) -> None:
        import green_attestation

        @contextmanager
        def private_env(_database_url):
            yield "postgres://user@db/green", {"PGPASSFILE": "/private"}

        completed = mock.Mock(stdout="1\n")
        with mock.patch.object(
            green_attestation, "private_pg_env", private_env
        ), mock.patch.object(
            green_attestation.subprocess,
            "run",
            return_value=completed,
        ) as run:
            result = green_attestation._psql(
                "postgres://user:secret@db/green",
                "SELECT :'signature';",
                {"signature": "abc"},
            )

        argv = run.call_args.args[0]
        self.assertNotIn("-c", argv)
        self.assertEqual(run.call_args.kwargs["input"], "SELECT :'signature';\n")
        self.assertEqual(result, "1")


if __name__ == "__main__":
    unittest.main()
