#!/usr/bin/env python3
"""Build the standalone Markhand roadmap from Markdown issue catalogs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ROADMAP = ROOT / "plans/markhand-web/roadmap.html"
DATA_PATTERN = re.compile(
    r"(?P<indent>[ \t]*)/\* ROADMAP_DATA_START \*/"
    r".*?"
    r"(?P=indent)/\* ROADMAP_DATA_END \*/",
    re.DOTALL,
)
DEFAULT_STATUS_PATTERN = re.compile(
    r"<!--\s*roadmap-default-status:\s*([a-z_ ]+)\s*-->",
    re.IGNORECASE,
)
ISSUE_PATTERN = re.compile(
    r"^#{2,3}\s+"
    r"(?P<id>(?:F|P0|P1A|P1B|1C|P2|P3|P4)-[A-Z0-9]+)"
    r"\s+—\s+"
    r"(?P<title>.+?)\s*$",
    re.MULTILINE,
)
STATUS_FIELD_PATTERN = re.compile(
    r"^\s*-\s*\*\*Status:\*\*\s*(?P<value>[^\r\n]*)$",
    re.IGNORECASE | re.MULTILINE,
)
STATUS_VALUE_PATTERN = re.compile(
    r"^(Ready|Blocked|Backlog|In progress|Review|Done)\b",
    re.IGNORECASE,
)
VALID_STATUSES = {
    "ready": "ready",
    "blocked": "blocked",
    "backlog": "backlog",
    "in progress": "in_progress",
    "review": "review",
    "done": "done",
}


@dataclass(frozen=True)
class PhaseConfig:
    directory: str
    code: str
    name: str
    description: str
    expected_ids: tuple[str, ...]

    @property
    def catalog(self) -> Path:
        return (
            ROOT
            / "plans/markhand-web/backlog"
            / self.directory
            / "issues/README.md"
        )

    @property
    def html_catalog(self) -> str:
        return f"backlog/{self.directory}/issues/README.md"


PHASES = (
    PhaseConfig(
        "phase-f",
        "F",
        "Engineering Foundation",
        "Rules, skeleton, dev environment và CI foundation",
        tuple(f"F-{number:02d}" for number in range(1, 13)),
    ),
    PhaseConfig(
        "phase-0",
        "0",
        "Discovery & Gates",
        "Benchmark, threat model, SLA và architecture decisions",
        tuple(f"P0-{number:02d}" for number in range(1, 11)),
    ),
    PhaseConfig(
        "phase-1a",
        "1A",
        "Knowledge Extraction",
        "Tách fileconv-knowledge, giữ desktop parity",
        tuple(f"P1A-{number:02d}" for number in range(1, 11)),
    ),
    PhaseConfig(
        "phase-1b",
        "1B",
        "Single-org POC",
        "Upload → convert → index → Q&A citation",
        tuple(
            [f"P1B-F{number:02d}" for number in range(1, 7)]
            + [f"P1B-I{number:02d}" for number in range(1, 8)]
            + [f"P1B-R{number:02d}" for number in range(1, 7)]
            + [f"P1B-O{number:02d}" for number in range(1, 6)]
        ),
    ),
    PhaseConfig(
        "phase-1c",
        "1C",
        "Multi-org Security",
        "RBAC, ACL, quota, fairness và denial suite",
        tuple(f"1C-{number:02d}" for number in range(1, 14)),
    ),
    PhaseConfig(
        "phase-2",
        "2",
        "Web SPA MVP",
        "Login, library, upload, Q&A và admin tối thiểu",
        tuple(f"P2-{number:02d}" for number in range(1, 17)),
    ),
    PhaseConfig(
        "phase-3",
        "3",
        "Document Intelligence",
        "BRD/PRD, quality, PII, tables, versions và export",
        tuple(f"P3-{number:02d}" for number in range(1, 15)),
    ),
    PhaseConfig(
        "phase-4",
        "4",
        "Production Hardening",
        "OIDC, HA, DR, deployment và go-live",
        tuple(f"P4-{number:02d}" for number in range(1, 15)),
    ),
)


def normalize_status(raw: str, *, source: Path) -> str:
    normalized = re.sub(r"\s+", " ", raw.strip().lower())
    try:
        return VALID_STATUSES[normalized]
    except KeyError as error:
        valid = ", ".join(sorted(VALID_STATUSES))
        raise ValueError(
            f"{source}: trạng thái không hợp lệ {raw!r}; cần một trong {valid}"
        ) from error


def mask_non_content(markdown: str) -> str:
    """Mask fenced code and HTML comments while preserving character offsets."""

    def masked_text(value: str) -> str:
        return "".join("\n" if char == "\n" else " " for char in value)

    def mask(match: re.Match[str]) -> str:
        return masked_text(match.group(0))

    without_comments = re.sub(r"<!--.*?-->", mask, markdown, flags=re.DOTALL)
    lines = without_comments.splitlines(keepends=True)
    result: list[str] = []
    fence: str | None = None
    for line in lines:
        fence_match = re.match(r"^\s*(```+|~~~+)", line)
        if fence_match:
            marker = fence_match.group(1)[0]
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            result.append(masked_text(line))
        elif fence is not None:
            result.append(masked_text(line))
        else:
            result.append(line)
    if fence is not None:
        raise ValueError("Markdown có code fence chưa đóng")
    return "".join(result)


def parse_phase(config: PhaseConfig) -> dict[str, object]:
    source = config.catalog
    markdown = source.read_text(encoding="utf-8")
    default_matches = DEFAULT_STATUS_PATTERN.findall(markdown)
    if len(default_matches) != 1:
        raise ValueError(
            f"{source}: cần đúng một roadmap-default-status, có {len(default_matches)}"
        )
    default_status = normalize_status(default_matches[0], source=source)
    content = mask_non_content(markdown)
    matches = list(ISSUE_PATTERN.finditer(content))
    issues: list[list[str]] = []

    for index, match in enumerate(matches):
        section_end = matches[index + 1].start() if index + 1 < len(matches) else len(content)
        section = content[match.end() : section_end]
        status_fields = list(STATUS_FIELD_PATTERN.finditer(section))
        if len(status_fields) > 1:
            raise ValueError(
                f"{source}: {match.group('id')} có {len(status_fields)} Status fields"
            )
        status = default_status
        if status_fields:
            raw_status = status_fields[0].group("value").strip()
            status_match = STATUS_VALUE_PATTERN.match(raw_status)
            if not status_match:
                raise ValueError(
                    f"{source}: {match.group('id')} có Status không hợp lệ: "
                    f"{raw_status!r}"
                )
            status = normalize_status(status_match.group(1), source=source)
        issues.append(
            [
                match.group("id").strip(),
                match.group("title").strip(),
                status,
            ]
        )

    issue_ids = tuple(issue[0] for issue in issues)
    if issue_ids != config.expected_ids:
        raise ValueError(
            f"{source}: issue IDs/order không đúng; "
            f"found={issue_ids}, expected={config.expected_ids}"
        )

    return {
        "id": config.directory,
        "code": config.code,
        "name": config.name,
        "description": config.description,
        "catalog": config.html_catalog,
        "issues": issues,
    }


def load_phases() -> list[dict[str, object]]:
    phases = [parse_phase(config) for config in PHASES]
    ids = [
        issue[0]
        for phase in phases
        for issue in phase["issues"]  # type: ignore[index]
    ]
    expected = sum(len(config.expected_ids) for config in PHASES)
    if len(ids) != expected:
        raise ValueError(f"Tổng issues {len(ids)}, cần {expected}")
    duplicate_ids = sorted({issue_id for issue_id in ids if ids.count(issue_id) > 1})
    if duplicate_ids:
        raise ValueError(f"Trùng issue IDs: {', '.join(duplicate_ids)}")
    return phases


def source_hash(phases: list[dict[str, object]]) -> str:
    canonical = json.dumps(
        phases,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()[:16]


def data_block(phases: list[dict[str, object]], indent: str) -> str:
    payload = json.dumps(phases, ensure_ascii=False, indent=2)
    # Keep the generated JavaScript safe even if a future Markdown title contains HTML.
    payload = payload.replace("<", r"\u003c")
    payload = payload.replace("\n", "\n" + indent)
    return (
        f"{indent}/* ROADMAP_DATA_START */\n"
        f'{indent}const ROADMAP_SOURCE_HASH = "{source_hash(phases)}";\n'
        f"{indent}const phases = {payload};\n"
        f"{indent}/* ROADMAP_DATA_END */"
    )


def render(current: str, phases: list[dict[str, object]]) -> str:
    matches = list(DATA_PATTERN.finditer(current))
    if len(matches) != 1:
        raise ValueError(
            f"{ROADMAP}: cần đúng một roadmap data block, có {len(matches)}"
        )
    match = matches[0]
    block = data_block(phases, match.group("indent"))
    return current[: match.start()] + block + current[match.end() :]


def atomic_write(path: Path, content: str) -> None:
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="\n",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def status_summary(phases: list[dict[str, object]]) -> dict[str, int]:
    summary = {status: 0 for status in VALID_STATUSES.values()}
    for phase in phases:
        for _, _, status in phase["issues"]:  # type: ignore[misc]
            summary[status] += 1
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build plans/markhand-web/roadmap.html from Markdown catalogs."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if roadmap.html is stale; do not write.",
    )
    args = parser.parse_args()

    try:
        phases = load_phases()
        current = ROADMAP.read_text(encoding="utf-8")
        generated = render(current, phases)
    except (OSError, ValueError) as error:
        print(f"roadmap build error: {error}", file=sys.stderr)
        return 1

    issue_count = sum(len(phase["issues"]) for phase in phases)  # type: ignore[arg-type]
    summary = status_summary(phases)
    if args.check:
        if generated != current:
            print(
                "roadmap build error: roadmap.html đã cũ; "
                "chạy python3 scripts/build-roadmap.py",
                file=sys.stderr,
            )
            return 1
        print(
            f"roadmap up to date: {issue_count} issues, "
            f"source={source_hash(phases)}, status={summary}"
        )
        return 0

    if generated != current:
        atomic_write(ROADMAP, generated)
    print(
        f"built {ROADMAP.relative_to(ROOT)}: {issue_count} issues, "
        f"source={source_hash(phases)}, status={summary}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
