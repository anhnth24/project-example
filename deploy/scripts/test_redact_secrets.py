#!/usr/bin/env python3
"""Unit tests for deploy/scripts/redact_secrets.py."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parent / "redact_secrets.py"
sys.path.insert(0, str(SCRIPT.parent))

import redact_secrets as rs  # noqa: E402


class RedactSecretsTests(unittest.TestCase):
    def test_bearer_token(self) -> None:
        out = rs.redact_structured(
            "Authorization: Bearer sk-live-abc123XYZ", fail_closed=False
        )
        self.assertIn("<REDACTED_BEARER>", out)
        self.assertNotIn("sk-live", out)

    def test_url_userinfo(self) -> None:
        out = rs.redact_structured(
            "postgres://markhand:s3cret@db:5432/app", fail_closed=False
        )
        self.assertIn("***@", out)
        self.assertNotIn("s3cret", out)

    def test_password_assignment(self) -> None:
        out = rs.redact_structured(
            'MARKHAND_DATABASE_URL="postgres://x:y@z/db"', fail_closed=False
        )
        self.assertNotIn("postgres://x:y@z/db", out)
        self.assertIn("<REDACTED_ENV>", out)

    def test_pem_block(self) -> None:
        pem = (
            "-----BEGIN PRIVATE KEY-----\n"
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7\n"
            "-----END PRIVATE KEY-----\n"
        )
        out = rs.redact_structured(pem, fail_closed=False)
        self.assertIn("<REDACTED_PEM>", out)
        self.assertNotIn("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7", out)

    def test_aws_access_key(self) -> None:
        out = rs.redact_structured(
            "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE", fail_closed=False
        )
        self.assertIn("<REDACTED_AWS_KEY>", out)
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", out)

    def test_jwt_like(self) -> None:
        token = (
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
            "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        )
        # Use a non-assignment wrapper so JWT pattern is visible (token= is
        # caught by ASSIGN_RE first).
        out = rs.redact_structured(f"Authorization: {token}", fail_closed=False)
        self.assertIn("<REDACTED_JWT>", out)
        self.assertNotIn("SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c", out)

    def test_json_nested_sensitive_keys(self) -> None:
        payload = {
            "ok": True,
            "nested": {
                "password": "hunter2",
                "token": "abc",
                "safe": "keep",
                "deeper": {"api_key": "k-123", "count": 1},
            },
            "list": [{"authorization": "Bearer xxxxxxxx"}, {"name": "ok"}],
        }
        text = json.dumps(payload)
        out = rs.redact_structured(text, fail_closed=False)
        parsed = json.loads(out)
        self.assertEqual(parsed["nested"]["password"], "<REDACTED>")
        self.assertEqual(parsed["nested"]["token"], "<REDACTED>")
        self.assertEqual(parsed["nested"]["safe"], "keep")
        self.assertEqual(parsed["nested"]["deeper"]["api_key"], "<REDACTED>")
        self.assertEqual(parsed["nested"]["deeper"]["count"], 1)
        self.assertEqual(parsed["list"][0]["authorization"], "<REDACTED>")
        self.assertEqual(parsed["list"][1]["name"], "ok")
        self.assertNotIn("hunter2", out)
        self.assertNotIn("k-123", out)

    def test_markhand_sensitive_env_names(self) -> None:
        samples = [
            "MARKHAND_DATABASE_URL=postgres://u:p@h/db",
            "export MARKHAND_JWT_SECRET=supersecretjwt",
            "MARKHAND_S3_SECRET_ACCESS_KEY: 'minio-secret'",
            "MARKHAND_EMBEDDING_API_KEY=emb-key-xyz",
            'MARKHAND_LLM_API_KEY="llm-secret"',
            "MARKHAND_OBJECT_STORE_SECRET=obj-secret",
        ]
        for line in samples:
            with self.subTest(line=line.split("=", 1)[0].split(":", 1)[0]):
                out = rs.redact_structured(line, fail_closed=False)
                self.assertIn("<REDACTED_ENV>", out)
                self.assertNotIn("supersecretjwt", out)
                self.assertNotIn("minio-secret", out)
                self.assertNotIn("emb-key-xyz", out)
                self.assertNotIn("llm-secret", out)
                self.assertNotIn("obj-secret", out)
                self.assertNotIn("postgres://u:p@h/db", out)

    def test_fail_closed_residual_without_emitting_secret(self) -> None:
        secret = "hunter2-residual-secret"
        text = f"password={secret}"
        with mock.patch.object(rs, "redact_json_blobs", side_effect=lambda t: t):
            with mock.patch.object(rs, "redact_text", side_effect=lambda t: t):
                with self.assertRaises(rs.ResidualSecretError) as ctx:
                    rs.redact_structured(text, fail_closed=True)
        self.assertNotIn(secret, str(ctx.exception))
        self.assertIn("password_assign", str(ctx.exception).lower())

    def test_cli_fail_closed_exit_code_and_no_secret_on_stdout(self) -> None:
        secret = "cli-residual-hunter2"
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / "raw.txt"
            out = Path(tmp) / "out.txt"
            raw.write_text(f"no_secret_here password={secret}\n", encoding="utf-8")
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), "--in", str(raw), "--out", str(out)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertNotIn(secret, out.read_text(encoding="utf-8"))
            self.assertNotIn(secret, proc.stdout)
            self.assertNotIn(secret, proc.stderr)

        wrapper = (
            "import sys\n"
            f"sys.path.insert(0, {str(SCRIPT.parent)!r})\n"
            "import redact_secrets as rs\n"
            "rs.redact_json_blobs = lambda t: t\n"
            "rs.redact_text = lambda t: t\n"
            "sys.argv = ['redact_secrets.py']\n"
            "raise SystemExit(rs.main())\n"
        )
        proc2 = subprocess.run(
            [sys.executable, "-c", wrapper],
            input=f"password={secret}\n",
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc2.returncode, 1)
        self.assertNotIn(secret, proc2.stdout)
        self.assertNotIn(secret, proc2.stderr)
        self.assertIn("residual", proc2.stderr.lower())

    def test_main_roundtrip_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / "raw.txt"
            out = Path(tmp) / "out.txt"
            raw.write_text(
                "Authorization: Bearer sk-test-12345678\n"
                '{"nested":{"api_key":"k-secret","ok":true}}\n',
                encoding="utf-8",
            )
            code = subprocess.run(
                [sys.executable, str(SCRIPT), "--in", str(raw), "--out", str(out)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(code.returncode, 0, code.stderr)
            text = out.read_text(encoding="utf-8")
            self.assertNotIn("sk-test-12345678", text)
            self.assertNotIn("k-secret", text)
            self.assertIn("<REDACTED_BEARER>", text)

    def test_truncated_json_quoted_key_fallback(self) -> None:
        secret = "trunc-secret-value"
        text = '{"password":"' + secret + '","ok":true'
        out = rs.redact_structured(text, fail_closed=False)
        self.assertNotIn(secret, out)
        self.assertIn('"<REDACTED>"', out)

    def test_malformed_json_quoted_key_fallback(self) -> None:
        secret = "malformed-secret-xyz"
        text = '{password:"' + secret + '", broken'
        # Without quotes around key, quoted fallback may not match — still fail-closed
        # if residual assign-like patterns remain. Prefer quoted form:
        text = '{"api_key": "' + secret + '",,,,}'
        out = rs.redact_structured(text, fail_closed=False)
        self.assertNotIn(secret, out)
        self.assertIn("<REDACTED>", out)

    def test_prefixed_json_redacts_sensitive(self) -> None:
        secret = "prefixed-hunter2"
        text = f'log-line prefix={{"token":"{secret}","n":1}} trailing\n'
        out = rs.redact_structured(text, fail_closed=False)
        self.assertNotIn(secret, out)
        self.assertIn("<REDACTED>", out)

    def test_multi_record_json_lines(self) -> None:
        a = "multi-a-secret"
        b = "multi-b-secret"
        text = (
            json.dumps({"password": a, "ok": True})
            + "\n"
            + json.dumps({"api_key": b, "x": 1})
            + "\n"
        )
        out = rs.redact_structured(text, fail_closed=False)
        self.assertNotIn(a, out)
        self.assertNotIn(b, out)
        self.assertEqual(out.count("<REDACTED>"), 2)

    def test_cli_malformed_json_never_emits_secret(self) -> None:
        secret = "cli-trunc-json-secret"
        proc = subprocess.run(
            [sys.executable, str(SCRIPT)],
            input='{"password":"' + secret,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotIn(secret, proc.stdout)
        self.assertNotIn(secret, proc.stderr)
        if proc.returncode == 0:
            self.assertIn("<REDACTED>", proc.stdout)
        else:
            self.assertIn("FAIL_CLOSED", proc.stderr)


if __name__ == "__main__":
    unittest.main()
