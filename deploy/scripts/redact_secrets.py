#!/usr/bin/env python3
"""Structured secret redaction for O02 evidence / runbook log capture.

Features:
- Text patterns: Bearer, URL userinfo, password/token assignments, PEM, JWT, AWS keys
- Sensitive MARKHAND_* env assignments (secret-bearing names)
- Recursive JSON object/array redaction by sensitive key names
- Fallback quoted-key redaction for malformed/truncated/prefixed/multi-record JSON
- Fail-closed: residual secrets after redaction → exit 1 without emitting secrets
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

BEARER_RE = re.compile(r"(?i)\b(Bearer)\s+[A-Za-z0-9._\-+=/]{8,}")
URL_CREDS_RE = re.compile(
    r"(?i)\b([a-z][a-z0-9+.-]*://)([^:/?#\s]+):([^@/?#\s]+)@"
)
ASSIGN_RE = re.compile(
    r"(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\s*[=:]\s*"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)
PEM_RE = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.DOTALL,
)
JWTISH_RE = re.compile(
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"
)
AWS_KEY_RE = re.compile(r"\b(AKIA[0-9A-Z]{16})\b")

# Secret-bearing MARKHAND env names (explicit + name heuristics).
MARKHAND_ENV_RE = re.compile(
    r"(?i)\b(MARKHAND_(?:"
    r"AUTH_SIGNING_KEY|MINIO_(?:SECRET_KEY|ROOT_PASSWORD)|"
    r"POSTGRES_PASSWORD|APP_DB_PASSWORD|MIGRATOR_DB_PASSWORD|"
    r"EMBEDDING_API_KEY|QDRANT_API_KEY|GLM_[A-Z0-9_]*KEY|"
    r"CHAT_API_KEY|PROVIDER_API_KEY|LLM_API_KEY|"
    r"DATABASE_URL|JWT_SECRET|S3_SECRET_ACCESS_KEY|OBJECT_STORE_SECRET|"
    r"[A-Z0-9_]*(?:SECRET|PASSWORD|TOKEN|API_KEY|ACCESS_KEY|PRIVATE_KEY|SIGNING_KEY)"
    r"))\s*[=:]\s*([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)

SENSITIVE_KEY_NAME = (
    r"(?:password|passwd|secret|token|authorization|api[_-]?key|access[_-]?key|"
    r"private[_-]?key|signing[_-]?key|client[_-]?secret|refresh[_-]?token|"
    r"database_url|jwt_secret|"
    r"markhand_[a-z0-9_]*(?:secret|password|token|api_key|access_key|private_key|signing_key))"
)

SENSITIVE_JSON_KEYS = re.compile(rf"(?i)^({SENSITIVE_KEY_NAME})$")

# Fallback for malformed / truncated / prefixed JSON: "password": "value"
QUOTED_SENSITIVE_JSON_RE = re.compile(
    rf'(?i)("(?:{SENSITIVE_KEY_NAME})"\s*:\s*)'
    r'("(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|[^\s,}}\]]+)'
)


class ResidualSecretError(RuntimeError):
    """Raised when residual secret-like material remains after redaction.

    Message contains finding *labels* only — never secret values.
    """


def redact_text(text: str) -> str:
    out = PEM_RE.sub("<REDACTED_PEM>", text)
    out = BEARER_RE.sub(r"\1 <REDACTED_BEARER>", out)
    out = URL_CREDS_RE.sub(r"\1***@", out)
    out = ASSIGN_RE.sub(r"\1=<REDACTED_SECRET>", out)
    out = MARKHAND_ENV_RE.sub(r"\1=<REDACTED_ENV>", out)
    out = AWS_KEY_RE.sub("<REDACTED_AWS_KEY>", out)
    out = JWTISH_RE.sub("<REDACTED_JWT>", out)
    out = redact_quoted_sensitive_json(out)
    return out


def redact_quoted_sensitive_json(text: str) -> str:
    """Redact quoted sensitive JSON key/value pairs without requiring parse."""
    return QUOTED_SENSITIVE_JSON_RE.sub(r'\1"<REDACTED>"', text)


def _redact_json(value: Any) -> Any:
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for key, item in value.items():
            if isinstance(key, str) and SENSITIVE_JSON_KEYS.match(key):
                out[key] = "<REDACTED>"
            else:
                out[key] = _redact_json(item)
        return out
    if isinstance(value, list):
        return [_redact_json(item) for item in value]
    if isinstance(value, str):
        return redact_text(value)
    return value


def _looks_like_json_fragment(text: str) -> bool:
    return "{" in text or "[" in text


def _try_load_json(text: str) -> Any | None:
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def redact_json_blobs(text: str) -> str:
    """Redact whole-JSON, prefixed JSON, multi-record, or fall back for broken JSON."""
    stripped = text.lstrip()
    # Whole-document JSON
    if stripped.startswith("{") or stripped.startswith("["):
        data = _try_load_json(text)
        if data is not None:
            return json.dumps(_redact_json(data), ensure_ascii=False, indent=2) + (
                "\n" if text.endswith("\n") else ""
            )
        # Malformed / truncated whole JSON — quoted-key fallback (never leave values).
        return redact_quoted_sensitive_json(text)

    # Prefixed / multi-record / mixed logs: walk lines and brace-ish spans.
    lines = text.splitlines(keepends=True)
    out_lines: list[str] = []
    for line in lines:
        candidate = line.strip()
        if candidate.startswith("{") or candidate.startswith("["):
            data = _try_load_json(candidate)
            if data is not None:
                redacted_line = json.dumps(_redact_json(data), ensure_ascii=False)
                nl = "\n" if line.endswith("\n") else ""
                out_lines.append(redacted_line + nl)
                continue
            # Truncated / malformed JSON line — fallback redaction.
            out_lines.append(redact_quoted_sensitive_json(line))
            continue
        # Prefixed JSON on a line: prefix {...}
        if _looks_like_json_fragment(line):
            data = None
            # Try extract first JSON object/array substring by brace matching (best-effort).
            for start_char, end_char in (("{", "}"), ("[", "]")):
                start = line.find(start_char)
                if start < 0:
                    continue
                depth = 0
                for i, ch in enumerate(line[start:], start=start):
                    if ch == start_char:
                        depth += 1
                    elif ch == end_char:
                        depth -= 1
                        if depth == 0:
                            fragment = line[start : i + 1]
                            data = _try_load_json(fragment)
                            if data is not None:
                                redacted = json.dumps(
                                    _redact_json(data), ensure_ascii=False
                                )
                                out_lines.append(
                                    redact_text(line[:start])
                                    + redacted
                                    + redact_quoted_sensitive_json(line[i + 1 :])
                                )
                                break
                if data is not None:
                    break
            else:
                out_lines.append(redact_quoted_sensitive_json(redact_text(line)))
            continue
        out_lines.append(line)
    return "".join(out_lines)


def redact_structured(text: str, *, fail_closed: bool = True) -> str:
    """Redact JSON recursively then apply text patterns; optionally fail closed.

    On residual findings with fail_closed=True, raises ResidualSecretError with
    labels only — callers must not emit the original or partial text.
    """
    out = redact_json_blobs(text)
    # Apply text patterns to non-JSON remainder.
    if not (out.lstrip().startswith("{") or out.lstrip().startswith("[")):
        lines = out.splitlines(keepends=True)
        rebuilt: list[str] = []
        for line in lines:
            candidate = line.strip()
            if candidate.startswith("{") or candidate.startswith("["):
                # Already handled (parse or quoted fallback).
                if _try_load_json(candidate) is not None:
                    rebuilt.append(line)
                else:
                    rebuilt.append(redact_text(line))
            else:
                rebuilt.append(redact_text(line))
        out = "".join(rebuilt)

    if "{" not in text and "[" not in text:
        out = redact_text(text)

    findings = broad_secret_scan(out)
    if findings and fail_closed:
        raise ResidualSecretError(
            "residual secret patterns after redaction: " + ",".join(findings)
        )
    return out


def broad_secret_scan(text: str) -> list[str]:
    """Return finding labels for residual secret-like material (labels only)."""
    findings: list[str] = []
    if re.search(
        r"(?i)\bBearer\s+(?!<REDACTED_BEARER>)[A-Za-z0-9._\-+=/]{8,}", text
    ):
        findings.append("bearer")
    if URL_CREDS_RE.search(text):
        findings.append("url_userinfo")
    if re.search(
        r"(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\s*[=:]\s*"
        r"(?!<REDACTED_SECRET>)([^\s'\"\\]+|'[^']*'|\"[^\"]*\")",
        text,
    ):
        findings.append("password_assign")
    if re.search(
        r"(?i)\bMARKHAND_[A-Z0-9_]*(?:SECRET|PASSWORD|TOKEN|API_KEY|ACCESS_KEY|"
        r"PRIVATE_KEY|SIGNING_KEY|DATABASE_URL|JWT_SECRET)\s*[=:]\s*"
        r"(?!<REDACTED_ENV>)(\S+)",
        text,
    ):
        findings.append("markhand_env_secret")
    if PEM_RE.search(text):
        findings.append("pem")
    if JWTISH_RE.search(text):
        findings.append("jwt")
    if AWS_KEY_RE.search(text):
        findings.append("aws_key")
    # Quoted JSON sensitive keys still carrying non-redacted values
    if QUOTED_SENSITIVE_JSON_RE.search(text):
        # Allow already-redacted "<REDACTED>" values
        for match in QUOTED_SENSITIVE_JSON_RE.finditer(text):
            value = match.group(2)
            if value not in {'"<REDACTED>"', "'<REDACTED>'", "<REDACTED>"}:
                findings.append("json_quoted_sensitive")
                break
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        data = None
    if data is not None and _json_has_sensitive_plaintext(data):
        findings.append("json_sensitive")
    else:
        for line in text.splitlines():
            candidate = line.strip()
            if not (candidate.startswith("{") or candidate.startswith("[")):
                continue
            try:
                line_data = json.loads(candidate)
            except json.JSONDecodeError:
                continue
            if _json_has_sensitive_plaintext(line_data):
                findings.append("json_sensitive")
                break
    return findings


def _json_has_sensitive_plaintext(value: Any) -> bool:
    if isinstance(value, dict):
        for key, item in value.items():
            if isinstance(key, str) and SENSITIVE_JSON_KEYS.match(key):
                if item != "<REDACTED>":
                    return True
            elif _json_has_sensitive_plaintext(item):
                return True
        return False
    if isinstance(value, list):
        return any(_json_has_sensitive_plaintext(item) for item in value)
    if isinstance(value, str):
        return bool(broad_secret_scan_text_only(value))
    return False


def broad_secret_scan_text_only(text: str) -> list[str]:
    findings: list[str] = []
    if re.search(
        r"(?i)\bBearer\s+(?!<REDACTED_BEARER>)[A-Za-z0-9._\-+=/]{8,}", text
    ):
        findings.append("bearer")
    if URL_CREDS_RE.search(text):
        findings.append("url_userinfo")
    if re.search(
        r"(?i)\b(password|passwd|secret|token|api[_-]?key)\s*[=:]\s*"
        r"(?!<REDACTED_SECRET>)(\S+)",
        text,
    ):
        findings.append("password_assign")
    if PEM_RE.search(text) or JWTISH_RE.search(text) or AWS_KEY_RE.search(text):
        findings.append("token_block")
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", help="file to redact (stdin if omitted)")
    parser.add_argument("-o", "--output", "--out", dest="output", help="write redacted output")
    parser.add_argument("--in", dest="infile", help="alias for positional path")
    parser.add_argument("--scan-only", action="store_true", help="scan without rewriting")
    parser.add_argument("--json", action="store_true", help="emit JSON result metadata only")
    parser.add_argument(
        "--allow-residual",
        action="store_true",
        help="do not fail-closed when residuals remain (default: fail closed)",
    )
    args = parser.parse_args()
    path = args.path or args.infile
    if path:
        raw = Path(path).read_text(encoding="utf-8", errors="replace")
    else:
        raw = sys.stdin.read()

    if args.scan_only:
        findings = broad_secret_scan(raw)
        if args.json:
            print(json.dumps({"findings": findings, "clean": not findings}, indent=2))
        elif findings:
            print("SECRET_SCAN_FAIL: " + ",".join(findings), file=sys.stderr)
        return 0 if not findings else 1

    try:
        redacted = redact_structured(raw, fail_closed=not args.allow_residual)
    except ResidualSecretError as exc:
        # Fail before any secret-bearing output.
        print("SECRET_REDACT_FAIL_CLOSED: " + str(exc), file=sys.stderr)
        return 1

    findings = broad_secret_scan(redacted)
    if findings and not args.allow_residual:
        print(
            "SECRET_REDACT_FAIL_CLOSED: residual=" + ",".join(findings),
            file=sys.stderr,
        )
        return 1

    if args.json:
        print(
            json.dumps(
                {
                    "findings": findings,
                    "clean": not findings,
                    "charsIn": len(raw),
                    "charsOut": len(redacted),
                    "failClosed": not args.allow_residual,
                },
                indent=2,
            )
        )
    if args.output:
        Path(args.output).write_text(redacted, encoding="utf-8")
    elif not args.json:
        sys.stdout.write(redacted)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
