"""Secret redaction and residual scan for O05 raw evidence."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any


REDACT_PATTERNS = [
    (re.compile(r"(Bearer\s+)[A-Za-z0-9._\-+=/]+"), r"\1[REDACTED]"),
    (re.compile(r"(postgres(?:ql)?://)[^@\s]+@"), r"\1[REDACTED]@"),
    (
        re.compile(
            r"(?i)(password|passwd|secret|token|authorization|api[_-]?key)"
            r"\"?\s*[:=]\s*\"?[^\s\",}]+"
        ),
        r"\1:[REDACTED]",
    ),
    (
        re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"),
        "[REDACTED_JWT]",
    ),
]

RESIDUAL_PATTERNS = [
    # Ignore already-redacted placeholders.
    re.compile(r"(?i)password\s*[:=]\s*\"?(?!\[REDACTED\])[^\s\",}]{4,}"),
    re.compile(
        r"(?i)(api[_-]?key|secret|token)\s*[:=]\s*\"?(?!\[REDACTED\])[^\s\",}]{6,}"
    ),
    re.compile(r"postgres(?:ql)?://[^:\s\[\]]+:[^@\s\[\]]+@"),
    re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"),
    re.compile(r"(?i)bearer\s+(?!\[REDACTED\])[a-z0-9\-_\.=]{20,}"),
]


def redact_text(text: str) -> str:
    out = text
    for pattern, repl in REDACT_PATTERNS:
        out = pattern.sub(repl, out)
    return out


def scan_text(text: str) -> list[str]:
    findings: list[str] = []
    for pattern in RESIDUAL_PATTERNS:
        if pattern.search(text):
            findings.append(pattern.pattern)
    return findings


def scan_raw_dir(raw_dir: Path) -> dict[str, Any]:
    findings: list[dict[str, str]] = []
    if not raw_dir.is_dir():
        return {"passed": False, "findings": [{"path": str(raw_dir), "reason": "missing"}]}
    for path in sorted(raw_dir.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix.lower() in {".png", ".jpg", ".jpeg", ".pdf", ".docx", ".xlsx", ".pptx"}:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            findings.append({"path": str(path), "reason": f"read_error:{exc}"})
            continue
        for hit in scan_text(text):
            findings.append({"path": str(path.relative_to(raw_dir)), "reason": hit})
    return {"passed": not findings, "findings": findings}
