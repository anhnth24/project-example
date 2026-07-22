"""Evidence redaction helpers — strip secrets and high-cardinality tenant fields.

Preserves harness case IDs (fmt-*, sec-*, fault-*, adv-*, harness-*) while
rejecting fixture tokens, prompt text, tenant/user UUIDs, and plaintext secrets.
"""

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
    re.compile(r"(?i)\b(password|passwd|pwd)\s*[:=]\s*(?!\[REDACTED\])\S+"),
    re.compile(r"(?i)\b(access[_-]?token|refresh[_-]?token)\s*[:=]\s*(?!\[REDACTED\])\S+"),
)

DEFAULT_PASSWORD_RE = re.compile(
    r"(?i)\b(?:markhand-e2e|markhand_root(?:_poc)?(?:_change_me)?|markhand_app_poc_change_me|"
    r"changeme|password123|secret123|admin123)\b"
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
FIXTURE_TOKEN_RE = re.compile(
    r"\b(?:MAHOA_E2E_[A-Z0-9_]+|MARKHAND[-_]E2E[-_][A-Z0-9]+|PROMPT[-_]INJECTION[-_][A-Z0-9]+)\b",
    re.IGNORECASE,
)
PROMPT_TEXT_RE = re.compile(
    r"(?i)(?:ignore previous instructions|exfiltrate secrets|dump credentials|"
    r"PROMPT-INJECTION-CANARY|SYSTEM OVERRIDE|dump secrets)"
)
CASE_ID_RE = re.compile(
    r"\b(?:fmt|sec|fault|adv|harness)-[a-z0-9\-]+\b",
    re.IGNORECASE,
)

# Fixed all-zero / hermetic sentinels allowed in committed evidence.
ALLOWED_UUID_SENTINELS = {
    "00000000-0000-0000-0000-000000000000",
    "00000000-0000-4000-8000-000000000004",
}

# Seeded tenant/user/collection UUIDs — always redact even in prose.
SEEDED_UUID_LITERALS = (
    "11111111-1111-1111-1111-111111111111",
    "22222222-2222-2222-2222-222222222201",
    "22222222-2222-2222-2222-222222222211",
    "22222222-2222-2222-2222-222222222212",
    "12121212-1212-4212-8212-121212121212",
    "23232323-2323-4232-8232-232323232301",
    "55555555-5555-5555-5555-555555555501",
    "56565656-5656-4565-8565-565656565601",
    "45454545-4545-4545-8545-454545454501",
)


def scrub_text(value: str) -> str:
    # Preserve case IDs by temporarily masking them.
    preserved: list[str] = []

    def _hold_case_id(match: re.Match[str]) -> str:
        preserved.append(match.group(0))
        return f"__CASE_ID_{len(preserved) - 1}__"

    out = CASE_ID_RE.sub(_hold_case_id, value)
    for lit in SEEDED_UUID_LITERALS:
        out = out.replace(lit, "[OPAQUE_ID]")
        out = out.replace(lit.upper(), "[OPAQUE_ID]")
    for pattern in SECRET_PATTERNS:
        out = pattern.sub("[REDACTED]", out)
    out = DEFAULT_PASSWORD_RE.sub("[REDACTED]", out)
    out = QUARANTINE_KEY_RE.sub("[REDACTED_OBJECT_KEY]", out)
    out = FIXTURE_TOKEN_RE.sub("[REDACTED_FIXTURE_TOKEN]", out)
    out = PROMPT_TEXT_RE.sub("[REDACTED_PROMPT_TEXT]", out)

    def _uuid_sub(match: re.Match[str]) -> str:
        text = match.group(0)
        if text.lower() in {s.lower() for s in ALLOWED_UUID_SENTINELS}:
            return text
        return "[OPAQUE_ID]"

    out = UUID_RE.sub(_uuid_sub, out)
    for idx, case_id in enumerate(preserved):
        out = out.replace(f"__CASE_ID_{idx}__", case_id)
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
    cleaned = re.sub(r"\[REDACTED_FIXTURE_TOKEN\]", "", cleaned)
    cleaned = re.sub(r"\[REDACTED_PROMPT_TEXT\]", "", cleaned)
    # Preserve harness case IDs while scanning for fixture tokens/secrets.
    cleaned_no_cases = CASE_ID_RE.sub("<case-id>", cleaned)
    errors: list[str] = []
    for pattern in SECRET_PATTERNS:
        if pattern.search(cleaned_no_cases):
            errors.append(f"secret pattern leaked: {pattern.pattern[:48]}")
    if QUARANTINE_KEY_RE.search(cleaned_no_cases):
        errors.append("raw object key leaked")
    if FIXTURE_TOKEN_RE.search(cleaned_no_cases):
        errors.append("fixture token leaked")
    if PROMPT_TEXT_RE.search(cleaned_no_cases):
        errors.append("prompt injection text leaked")
    if DEFAULT_PASSWORD_RE.search(cleaned_no_cases):
        errors.append("default/plain password leaked")
    for lit in SEEDED_UUID_LITERALS:
        if lit in cleaned_no_cases or lit.upper() in cleaned_no_cases:
            errors.append("seeded tenant/user UUID leaked")
            break
    allowed = {s.lower() for s in ALLOWED_UUID_SENTINELS}
    for match in UUID_RE.finditer(cleaned_no_cases):
        if match.group(0).lower() not in allowed:
            errors.append(f"arbitrary UUID leaked: {match.group(0)[:13]}…")
            break
    lowered = cleaned_no_cases.lower()
    for needle in ("password=", "bearer ey", "refresh_token", "access_token"):
        if needle in lowered:
            errors.append(f"forbidden token-like content: {needle}")
    return errors
