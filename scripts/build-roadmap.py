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
MASTER_PLAN = ROOT / "plans/markhand-web/README.md"
ROADMAP = ROOT / "plans/markhand-web/roadmap.html"
TOTAL_PATTERN = re.compile(r"Issue-level backlog \((\d+) issues\)")
PHASE_ROW_PATTERN = re.compile(
    r"^\|\s*(?P<code>F|[0-9]+[A-Z]?)\s*"
    r"\|\s*(?P<description>.*?)\s*"
    r"\|\s*\[[^\]]+\]\((?P<plan>[^)]+)\)\s*"
    r"\|\s*\[(?P<count>\d+)\s+issues\]\((?P<catalog>[^)]+)\)\s*\|\s*$",
    re.MULTILINE,
)
PLAN_TITLE_PATTERN = re.compile(
    r"^#\s+Phase\s+(?P<code>[A-Z0-9]+)\s+—\s+(?P<title>.+?)\s*$",
    re.MULTILINE,
)
PARENT_PLAN_PATTERN = re.compile(
    r"^Parent plan:\s*\[[^\]]+\]\((?P<plan>[^)]+)\)\s*$",
    re.MULTILINE,
)
GROUPS_PATTERN = re.compile(
    r"<!--\s*roadmap-groups:\s*([A-Z](?:\s*,\s*[A-Z])*)\s*-->",
    re.IGNORECASE,
)
TECH_STACK_BLOCK_PATTERN = re.compile(
    r"<!--\s*roadmap-tech-stack-start\s*-->\s*"
    r"(?P<table>.*?)"
    r"\s*<!--\s*roadmap-tech-stack-end\s*-->",
    re.DOTALL | re.IGNORECASE,
)
DATA_PATTERN = re.compile(
    r"^(?P<indent>[ \t]*)/\* ROADMAP_DATA_START \*/[ \t]*\r?\n"
    r".*?"
    r"^(?P=indent)/\* ROADMAP_DATA_END \*/[ \t]*$",
    re.DOTALL | re.MULTILINE,
)
DEFAULT_STATUS_PATTERN = re.compile(
    r"<!--\s*roadmap-default-status:\s*([a-z_ ]+)\s*-->",
    re.IGNORECASE,
)
ISSUE_PATTERN = re.compile(
    r"^(?P<heading>#{2,3})\s+"
    r"(?P<id>[A-Z0-9]+(?:-[A-Z0-9]+)+)"
    r"\s+—\s+"
    r"(?P<title>.+?)\s*$",
    re.MULTILINE,
)
HEADING_PATTERN = re.compile(r"^(?P<heading>#{1,6})\s+", re.MULTILINE)
STATUS_FIELD_PATTERN = re.compile(
    r"^\s*-\s*\*\*Status:\*\*\s*(?P<value>[^\r\n]*)$",
    re.IGNORECASE | re.MULTILINE,
)
STATUS_VALUE_PATTERN = re.compile(
    r"^(Ready|Blocked|Backlog|In progress|Review|Done)"
    r"(?:\s*(?:[.;,:—-]\s*.*|bởi\b.*))?$",
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
    code: str
    name: str
    description: str
    plan: Path
    catalog: Path
    expected_issues: int
    expected_groups: tuple[str, ...] = ()

    @property
    def directory(self) -> str:
        return self.catalog.parent.parent.name

    @property
    def html_catalog(self) -> str:
        return self.catalog.relative_to(MASTER_PLAN.parent).as_posix()

    @property
    def html_plan(self) -> str:
        return self.plan.relative_to(MASTER_PLAN.parent).as_posix()


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


def resolve_registry_path(raw: str, *, kind: str) -> Path:
    base = MASTER_PLAN.parent.resolve()
    resolved = (MASTER_PLAN.parent / raw).resolve()
    if not resolved.is_relative_to(base):
        raise ValueError(f"{MASTER_PLAN}: {kind} vượt ngoài Markhand Web plan: {raw}")
    if not resolved.is_file():
        raise ValueError(f"{MASTER_PLAN}: không tìm thấy {kind}: {raw}")
    return resolved


def clean_inline_markdown(value: str) -> str:
    return re.sub(r"(`+)(.*?)\1", r"\2", value).strip()


def split_markdown_table_row(line: str, *, line_number: int) -> list[str]:
    content = line[1:-1]
    cells: list[str] = []
    current: list[str] = []
    index = 0
    while index < len(content):
        char = content[index]
        if (
            char == "\\"
            and index + 1 < len(content)
            and content[index + 1] in {"|", "`"}
        ):
            current.append(content[index + 1])
            index += 2
            continue
        if char == "`":
            end = index + 1
            while end < len(content) and content[end] == "`":
                end += 1
            marker = content[index:end]
            closing = re.search(
                rf"(?<!`){re.escape(marker)}(?!`)",
                content[end:],
            )
            if closing is None:
                current.append(marker)
                index = end
                continue
            closing_end = end + closing.end()
            current.append(content[index:closing_end])
            index = closing_end
            continue
        if char == "|":
            cells.append("".join(current))
            current = []
        else:
            current.append(char)
        index += 1
    cells.append("".join(current))
    return cells


def parse_tech_stack(markdown: str) -> list[dict[str, str]]:
    blocks = TECH_STACK_BLOCK_PATTERN.findall(markdown)
    if len(blocks) != 1:
        raise ValueError(
            f"{MASTER_PLAN}: cần đúng một roadmap tech-stack block, có {len(blocks)}"
        )
    lines = [line.strip() for line in blocks[0].splitlines() if line.strip()]
    if len(lines) < 3:
        raise ValueError(f"{MASTER_PLAN}: tech-stack table không có data rows")
    if lines[0] != "| Lớp | Công nghệ | Trách nhiệm | Delivery |":
        raise ValueError(f"{MASTER_PLAN}: tech-stack table header không hợp lệ")
    if not re.fullmatch(r"\|\s*---\s*\|\s*---\s*\|\s*---\s*\|\s*---\s*\|", lines[1]):
        raise ValueError(f"{MASTER_PLAN}: tech-stack table separator không hợp lệ")

    stack: list[dict[str, str]] = []
    for line_number, line in enumerate(lines[2:], start=3):
        if not line.startswith("|") or not line.endswith("|"):
            raise ValueError(
                f"{MASTER_PLAN}: tech-stack row {line_number} không phải Markdown table row"
            )
        cells = [
            clean_inline_markdown(cell)
            for cell in split_markdown_table_row(line, line_number=line_number)
        ]
        if len(cells) != 4 or any(not cell for cell in cells):
            raise ValueError(
                f"{MASTER_PLAN}: tech-stack row {line_number} cần đúng 4 cells có nội dung"
            )
        stack.append(
            {
                "layer": cells[0],
                "technology": cells[1],
                "responsibility": cells[2],
                "delivery": cells[3],
            }
        )

    layers = [item["layer"] for item in stack]
    if len(layers) != len(set(layers)):
        raise ValueError(f"{MASTER_PLAN}: trùng tech-stack layers")
    return stack


def parse_registry() -> tuple[list[PhaseConfig], int]:
    markdown = MASTER_PLAN.read_text(encoding="utf-8")
    total_matches = TOTAL_PATTERN.findall(markdown)
    if len(total_matches) != 1:
        raise ValueError(
            f"{MASTER_PLAN}: cần đúng một Issue-level backlog total, "
            f"có {len(total_matches)}"
        )
    expected_total = int(total_matches[0])
    rows = list(PHASE_ROW_PATTERN.finditer(mask_non_content(markdown)))
    if not rows:
        raise ValueError(f"{MASTER_PLAN}: không tìm thấy phase registry rows")

    configs: list[PhaseConfig] = []
    for row in rows:
        plan = resolve_registry_path(row.group("plan"), kind="phase plan")
        catalog = resolve_registry_path(row.group("catalog"), kind="issue catalog")
        plan_titles = list(
            PLAN_TITLE_PATTERN.finditer(
                mask_non_content(plan.read_text(encoding="utf-8"))
            )
        )
        if len(plan_titles) != 1:
            raise ValueError(
                f"{plan}: cần đúng một '# Phase ... — ...' title, có {len(plan_titles)}"
            )
        plan_title = plan_titles[0]
        if plan_title.group("code") != row.group("code"):
            raise ValueError(
                f"{plan}: title Phase {plan_title.group('code')} không khớp "
                f"registry Phase {row.group('code')}"
            )
        expected_directory = f"phase-{row.group('code').lower()}"
        expected_catalog = (
            MASTER_PLAN.parent
            / "backlog"
            / expected_directory
            / "issues"
            / "README.md"
        ).resolve()
        if catalog != expected_catalog:
            raise ValueError(
                f"{catalog}: catalog phải nằm tại "
                f"backlog/{expected_directory}/issues/README.md"
            )
        catalog_markdown = catalog.read_text(encoding="utf-8")
        parent_links = PARENT_PLAN_PATTERN.findall(
            mask_non_content(catalog_markdown)
        )
        if len(parent_links) != 1:
            raise ValueError(
                f"{catalog}: cần đúng một Parent plan backlink, có {len(parent_links)}"
            )
        catalog_parent = (catalog.parent / parent_links[0]).resolve()
        if catalog_parent != plan:
            raise ValueError(
                f"{catalog}: Parent plan {catalog_parent} không khớp registry {plan}"
            )
        group_matches = GROUPS_PATTERN.findall(catalog_markdown)
        if row.group("code") == "1B":
            if len(group_matches) != 1:
                raise ValueError(
                    f"{catalog}: Phase 1B cần đúng một roadmap-groups"
                )
            expected_groups = tuple(
                part.strip().upper() for part in group_matches[0].split(",")
            )
        else:
            if group_matches:
                raise ValueError(
                    f"{catalog}: roadmap-groups chỉ được dùng cho grouped issue IDs"
                )
            expected_groups = ()
        configs.append(
            PhaseConfig(
                code=row.group("code"),
                name=plan_title.group("title").strip(),
                description=clean_inline_markdown(row.group("description")),
                plan=plan,
                catalog=catalog,
                expected_issues=int(row.group("count")),
                expected_groups=expected_groups,
            )
        )

    codes = [config.code for config in configs]
    plans = [config.plan for config in configs]
    catalogs = [config.catalog for config in configs]
    if len(codes) != len(set(codes)):
        raise ValueError(f"{MASTER_PLAN}: trùng phase codes")
    if len(catalogs) != len(set(catalogs)):
        raise ValueError(f"{MASTER_PLAN}: trùng issue catalogs")
    if len(plans) != len(set(plans)):
        raise ValueError(f"{MASTER_PLAN}: trùng phase plans")
    if sum(config.expected_issues for config in configs) != expected_total:
        raise ValueError(
            f"{MASTER_PLAN}: tổng phase issue counts không bằng {expected_total}"
        )
    return configs, expected_total


def validate_phase_issue_ids(config: PhaseConfig, issue_ids: tuple[str, ...]) -> None:
    if len(issue_ids) != config.expected_issues:
        raise ValueError(
            f"{config.catalog}: tìm thấy {len(issue_ids)} issues, "
            f"registry cần {config.expected_issues}"
        )

    if config.code == "F":
        pattern = re.compile(r"^F-(?P<number>\d{2})$")
    elif config.code == "1C":
        pattern = re.compile(r"^1C-(?P<number>\d{2})$")
    elif config.code == "1B":
        pattern = re.compile(r"^P1B-(?P<group>[A-Z])(?P<number>\d{2})$")
    else:
        pattern = re.compile(
            rf"^P{re.escape(config.code)}-(?P<number>\d{{2}})$"
        )

    parsed: list[tuple[str, int]] = []
    for issue_id in issue_ids:
        match = pattern.fullmatch(issue_id)
        if not match:
            raise ValueError(
                f"{config.catalog}: {issue_id} không thuộc Phase {config.code}"
            )
        parsed.append((match.groupdict().get("group") or "", int(match.group("number"))))

    groups: list[str] = []
    for group, _ in parsed:
        if group not in groups:
            groups.append(group)
    if tuple(groups) != config.expected_groups and config.expected_groups:
        raise ValueError(
            f"{config.catalog}: groups {tuple(groups)} không khớp "
            f"roadmap-groups {config.expected_groups}"
        )
    if [group for group, _ in parsed] != [
        group for group in groups for _ in range(sum(1 for item in parsed if item[0] == group))
    ]:
        raise ValueError(f"{config.catalog}: issue groups không liên tục")
    for group in groups:
        numbers = [number for item_group, number in parsed if item_group == group]
        if numbers != list(range(1, len(numbers) + 1)):
            raise ValueError(
                f"{config.catalog}: numbering group {group or 'default'} không liên tục"
            )


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

    for match in matches:
        heading_level = len(match.group("heading"))
        section_end = len(content)
        for next_heading in HEADING_PATTERN.finditer(content, match.end()):
            if len(next_heading.group("heading")) <= heading_level:
                section_end = next_heading.start()
                break
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
    validate_phase_issue_ids(config, issue_ids)

    return {
        "id": config.directory,
        "code": config.code,
        "name": config.name,
        "description": config.description,
        "plan": config.html_plan,
        "catalog": config.html_catalog,
        "issues": issues,
    }


def load_phases() -> list[dict[str, object]]:
    configs, expected = parse_registry()
    phases = [parse_phase(config) for config in configs]
    ids = [
        issue[0]
        for phase in phases
        for issue in phase["issues"]  # type: ignore[index]
    ]
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


def javascript_payload(value: object, indent: str) -> str:
    payload = json.dumps(value, ensure_ascii=False, indent=2)
    # Keep the generated JavaScript safe even if a future Markdown title contains HTML.
    payload = payload.replace("<", r"\u003c")
    return payload.replace("\n", "\n" + indent)


def data_block(
    phases: list[dict[str, object]],
    tech_stack: list[dict[str, str]],
    indent: str,
) -> str:
    phase_payload = javascript_payload(phases, indent)
    stack_payload = javascript_payload(tech_stack, indent)
    return (
        f"{indent}/* ROADMAP_DATA_START */\n"
        f'{indent}const ROADMAP_SOURCE_HASH = "{source_hash(phases)}";\n'
        f"{indent}const phases = {phase_payload};\n"
        f"{indent}const techStack = {stack_payload};\n"
        f"{indent}/* ROADMAP_DATA_END */"
    )


def render(
    current: str,
    phases: list[dict[str, object]],
    tech_stack: list[dict[str, str]],
) -> str:
    matches = list(DATA_PATTERN.finditer(current))
    if len(matches) != 1:
        raise ValueError(
            f"{ROADMAP}: cần đúng một roadmap data block, có {len(matches)}"
        )
    match = matches[0]
    block = data_block(phases, tech_stack, match.group("indent"))
    return current[: match.start()] + block + current[match.end() :]


def atomic_write(path: Path, content: str) -> None:
    temporary: Path | None = None
    mode = path.stat().st_mode & 0o777
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
        os.chmod(temporary, mode)
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
        master_markdown = MASTER_PLAN.read_text(encoding="utf-8")
        phases = load_phases()
        tech_stack = parse_tech_stack(master_markdown)
        current = ROADMAP.read_text(encoding="utf-8")
        generated = render(current, phases, tech_stack)
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
