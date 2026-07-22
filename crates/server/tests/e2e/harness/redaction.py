"""Evidence redaction helpers — strip secrets and high-cardinality tenant fields."""

from __future__ import annotations

import re
from typing import Any

# Patterns that must never appear in committed evidence / logs.
SECRET_PATTERNS = (
    re.compile(r"(?i)bearer\s+[a-z0-9\-._~+/]+=*"),
    re.compile(r"(?i)(\"accessToken\"\s*:\s*\")[^\"]+(\")"),
    re.compile(r"(?i)(\"refreshToken\"\s*:\s*\")[^\"]+(\")"),
    re.compile(r"(?i)(\"password\"\s*:\s*\")[^\"]+(\")"),
    re.compile(r"(?i)postgres(?:ql)?://[^\s\"']+"),
    re.compile(r"(?i)https?://[^\s\"']+\?[^\s\"']*(?:X-Amz-|Signature=|token=)[^\s\"']*"),
    re.compile(r"(?i)-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bghp_[A-Za-z0-9]{20,}\b"),
)

# High-cardinality / tenant fields replaced with opaque labels when present as JSON keys.
FORBIDDEN_JSON_KEYS = {
    "orgId",
    "org_id",
    "userId",
    "user_id",
    "tenantId",
    "tenant_id",
    "accessToken",
    "refreshToken",
    "password",
    "authorization",
    "originalObjectKey",
    "objectKey",
    "object_key",
    "signedUrl",
    "signed_url",
    "downloadUrl",
    "markdown",
    "quote",
    "snippet",
    "answer",
    "question",
    "prompt",
    "email",
}

UUID_RE = re.compile(
    r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}\b"
)
QUARANTINE_KEY_RE = re.compile(r"\b(?:quarantine|trusted)/[0-9a-f]{64}/[0-9a-f]{32,64}\b")


def scrub_text(value: str) -> str:
    out = value
    for pattern in SECRET_PATTERNS:
        out = pattern.sub("[REDACTED]", out)
    out = QUARANTINE_KEY_RE.sub("[REDACTED_OBJECT_KEY]", out)
    # Keep run-scoped opaque refs that are already labels (run-*), otherwise mask UUIDs.
    def _uuid_sub(match: re.Match[str]) -> str:
        text = match.group(0)
        if text.startswith("00000000-"):
            return "[OPAQUE_ID]"
        return "[OPAQUE_ID]"

    out = UUID_RE.sub(_uuid_sub, out)
    return out


def redact_value(value: Any, *, key: str | None = None) -> Any:
    if key is not None and key in FORBIDDEN_JSON_KEYS:
        return "[REDACTED]"
    if isinstance(value, str):
        return scrub_text(value)
    if isinstance(value, list):
        return [redact_value(item) for item in value]
    if isinstance(value, dict):
        return {k: redact_value(v, key=k) for k, v in value.items()}
    return value


def assert_no_forbidden_evidence(text: str) -> list[str]:
    # Placeholders emitted by redact_value/scrub_text are allowed.
    cleaned = re.sub(r"\[REDACTED(?:_[A-Z]+)?\]", "", text)
    cleaned = re.sub(r"\[OPAQUE_ID\]", "", cleaned)
    errors: list[str] = []
    for pattern in SECRET_PATTERNS:
        if pattern.search(cleaned):
            errors.append(f"secret pattern leaked: {pattern.pattern[:48]}")
    if QUARANTINE_KEY_RE.search(cleaned):
        errors.append("raw object key leaked")
    lowered = cleaned.lower()
    for needle in ("password=", "bearer ey", "refresh_token", "access_token"):
        if needle in lowered:
            errors.append(f"forbidden token-like content: {needle}")
    return errors