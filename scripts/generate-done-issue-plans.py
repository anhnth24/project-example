#!/usr/bin/env python3
"""Backfill and validate one provenance-safe plan per Done web issue.

The historical catalogs are the source of truth. Generated files preserve their
wording and mark missing historical facts as unknown instead of reconstructing
details from hindsight.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPORTS_DIR = ROOT / "plans/reports"
PLAN_MARKER = "generated-done-issue-plan"
PLAN_LINK_PATTERN = re.compile(
    r"^- \*\*Plan file:\*\* \[[^\]]+\]\((?P<path>[^)]+)\)\s*$",
    re.MULTILINE,
)
FIELD_PATTERN = re.compile(r"\*\*(?P<key>[^*]+?):\*\*\s*")
SHA_PATTERN = re.compile(r"(?<![0-9a-f])(?P<sha>[0-9a-f]{7,40})(?![0-9a-f])")
REF_PATTERN = re.compile(r"(?:PR\s*)?#(?P<number>\d+)", re.IGNORECASE)
STAMP_PATTERN = re.compile(r"^\d{6}-\d{4}$")


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


roadmap = load_module("build_roadmap_for_done_plans", ROOT / "scripts/build-roadmap.py")
sync = load_module(
    "sync_github_issues_for_done_plans", ROOT / "scripts/sync-github-issues.py"
)


@dataclass(frozen=True)
class IssueRecord:
    phase_code: str
    issue_id: str
    title: str
    status: str
    catalog: Path
    catalog_html_path: str
    phase_plan_html_path: str
    section_start: int
    section_end: int
    section: str
    fields: dict[str, str]

    @property
    def github_title(self) -> str:
        return f"{self.issue_id} — {self.title}"


@dataclass(frozen=True)
class GitHubIssue:
    number: int
    url: str
    state: str
    closed_at: str | None


@dataclass(frozen=True)
class GitHubPr:
    number: int
    url: str
    title: str
    merged_at: str | None
    merge_commit: str | None


def normalize_key(raw: str) -> str:
    return re.sub(r"\s+", " ", raw.strip().lower())


def canonical_key(raw: str) -> str | None:
    key = normalize_key(raw)
    if key == "status":
        return "Status"
    if key == "objective":
        return "Objective"
    if key in {"implementation plan", "plan"}:
        return "Implementation plan"
    if key in {"plan/files", "plan / files"}:
        return "Plan/files"
    if key in {"files/modules", "files", "files / scope", "files/scope"}:
        return "Files/modules"
    if key in {"dependencies/blocks", "dependencies", "depends"}:
        return "Dependencies / blocks"
    if key.startswith("acceptance") and "/tests" not in key:
        return "Acceptance criteria"
    if key in {
        "required tests/evidence",
        "tests/evidence",
        "tests",
    }:
        return "Required tests / evidence"
    if key in {"acceptance/tests", "acceptance / tests"}:
        return "Acceptance/tests"
    if key in {"security/migration", "security"}:
        return "Security and migration notes"
    if key in {"out of scope", "out"}:
        return "Out of scope"
    if key == "plan file":
        return "Plan file"
    return None


def append_field(fields: dict[str, str], key: str, value: str) -> None:
    value = value.strip()
    if not value:
        return
    if key in fields:
        fields[key] = f"{fields[key]}\n{value}".strip()
    else:
        fields[key] = value


def parse_fields(section: str) -> dict[str, str]:
    """Parse verbose, compact, and multi-field catalog cards."""
    fields: dict[str, str] = {}
    current_key: str | None = None
    current_lines: list[str] = []

    def flush() -> None:
        nonlocal current_key, current_lines
        if current_key is not None:
            append_field(fields, current_key, "\n".join(current_lines))
        current_key = None
        current_lines = []

    for raw_line in section.splitlines():
        stripped = raw_line.strip()
        matches = list(FIELD_PATTERN.finditer(stripped))
        if matches:
            flush()
            for index, match in enumerate(matches):
                key = canonical_key(match.group("key"))
                start = match.end()
                end = (
                    matches[index + 1].start()
                    if index + 1 < len(matches)
                    else len(stripped)
                )
                value = stripped[start:end].strip()
                if key is None:
                    continue
                if index + 1 < len(matches):
                    append_field(fields, key, value)
                else:
                    current_key = key
                    current_lines = [value] if value else []
            continue
        if current_key is not None:
            if stripped:
                current_lines.append(stripped)
            elif current_lines and current_lines[-1] != "":
                current_lines.append("")
    flush()
    return fields


def load_records() -> list[IssueRecord]:
    configs, expected = roadmap.parse_registry()
    records: list[IssueRecord] = []
    for config in configs:
        markdown = config.catalog.read_text(encoding="utf-8")
        masked = roadmap.mask_non_content(markdown)
        default_matches = roadmap.DEFAULT_STATUS_PATTERN.findall(markdown)
        default_status = roadmap.normalize_status(
            default_matches[0], source=config.catalog
        )
        matches = list(roadmap.ISSUE_PATTERN.finditer(masked))
        for index, match in enumerate(matches):
            heading_level = len(match.group("heading"))
            section_end = len(masked)
            for next_heading in roadmap.HEADING_PATTERN.finditer(masked, match.end()):
                if len(next_heading.group("heading")) <= heading_level:
                    section_end = next_heading.start()
                    break
            section = markdown[match.end() : section_end]
            fields = parse_fields(section)
            status = default_status
            raw_status = fields.get("Status", "")
            status_match = roadmap.STATUS_VALUE_PATTERN.match(raw_status)
            if status_match:
                status = roadmap.normalize_status(
                    status_match.group(1), source=config.catalog
                )
            records.append(
                IssueRecord(
                    phase_code=config.code,
                    issue_id=match.group("id").strip(),
                    title=match.group("title").strip(),
                    status=status,
                    catalog=config.catalog,
                    catalog_html_path=config.html_catalog,
                    phase_plan_html_path=config.html_plan,
                    section_start=match.end(),
                    section_end=section_end,
                    section=section,
                    fields=fields,
                )
            )
    if len(records) != expected:
        raise ValueError(f"Expected {expected} issues, loaded {len(records)}")
    return records


def slugify(value: str, max_length: int = 52) -> str:
    value = value.replace("đ", "d").replace("Đ", "D")
    value = unicodedata.normalize("NFKD", value)
    value = "".join(character for character in value if not unicodedata.combining(character))
    value = re.sub(r"[^A-Za-z0-9]+", "-", value).strip("-").lower()
    value = value[:max_length].rstrip("-")
    return value or "issue"


def filename_for(issue: IssueRecord, stamp: str) -> str:
    issue_slug = slugify(issue.issue_id, max_length=24)
    title_slug = slugify(issue.title)
    return f"plan-{stamp}-{issue_slug}-{title_slug}.md"


def gh_json(args: list[str]) -> object:
    result = subprocess.run(
        ["gh", *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return json.loads(result.stdout or "null")


def fetch_github_issues(records: list[IssueRecord]) -> dict[str, GitHubIssue]:
    payload = gh_json(
        [
            "issue",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "number,title,url,state,closedAt,labels,milestone",
        ]
    )
    expected_phase = {
        code: milestone for code, (_, milestone) in sync.PHASE_LABELS.items()
    }
    candidates: dict[str, list[tuple[int, dict[str, object]]]] = {}
    titles = {record.github_title: record for record in records}
    for item in payload:
        title = str(item.get("title") or "")
        record = titles.get(title)
        if record is None:
            continue
        labels = {
            str(label.get("name") or "")
            for label in (item.get("labels") or [])
            if isinstance(label, dict)
        }
        milestone = item.get("milestone") or {}
        milestone_title = (
            str(milestone.get("title") or "") if isinstance(milestone, dict) else ""
        )
        score = 0
        if "markhand-web" in labels:
            score += 4
        if milestone_title == expected_phase[record.phase_code]:
            score += 4
        if record.status == "done" and item.get("state") == "CLOSED":
            score += 1
        candidates.setdefault(title, []).append((score, item))

    resolved: dict[str, GitHubIssue] = {}
    for record in records:
        choices = candidates.get(record.github_title, [])
        if not choices:
            continue
        _, item = max(choices, key=lambda pair: (pair[0], -int(pair[1]["number"])))
        resolved[record.issue_id] = GitHubIssue(
            number=int(item["number"]),
            url=str(item["url"]),
            state=str(item["state"]),
            closed_at=str(item["closedAt"]) if item.get("closedAt") else None,
        )
    return resolved


def fetch_github_prs() -> dict[int, GitHubPr]:
    payload = gh_json(
        [
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--json",
            "number,url,title,mergedAt,mergeCommit",
        ]
    )
    result: dict[int, GitHubPr] = {}
    for item in payload:
        merge_commit = item.get("mergeCommit") or {}
        result[int(item["number"])] = GitHubPr(
            number=int(item["number"]),
            url=str(item["url"]),
            title=str(item["title"]),
            merged_at=str(item["mergedAt"]) if item.get("mergedAt") else None,
            merge_commit=(
                str(merge_commit.get("oid"))
                if isinstance(merge_commit, dict) and merge_commit.get("oid")
                else None
            ),
        )
    return result


def status_without_done(raw_status: str) -> str:
    match = roadmap.STATUS_VALUE_PATTERN.match(raw_status.strip())
    if not match:
        return raw_status.strip()
    remainder = raw_status.strip()[match.end() :].strip()
    return remainder.lstrip("—-: ").strip() or "Catalog records status as Done."


def markdown_quote(value: str) -> str:
    lines = value.strip().splitlines() or [""]
    return "\n".join(f"> {line}" if line else ">" for line in lines)


def field_or_unknown(issue: IssueRecord, key: str, message: str) -> str:
    value = issue.fields.get(key, "").strip()
    return value if value else f"UNKNOWN — {message}"


def issue_prs(issue: IssueRecord, prs: dict[int, GitHubPr]) -> list[GitHubPr]:
    status = issue.fields.get("Status", "")
    numbers = {int(match.group("number")) for match in REF_PATTERN.finditer(status)}
    return [prs[number] for number in sorted(numbers) if number in prs]


def evidence_shas(issue: IssueRecord, linked_prs: list[GitHubPr]) -> list[str]:
    shas = {
        match.group("sha")
        for match in SHA_PATTERN.finditer(issue.fields.get("Status", ""))
    }
    shas.update(pr.merge_commit for pr in linked_prs if pr.merge_commit)
    return sorted(sha for sha in shas if sha)


def render_plan(
    issue: IssueRecord,
    github_issue: GitHubIssue | None,
    prs: dict[int, GitHubPr],
) -> str:
    linked_prs = issue_prs(issue, prs)
    shas = evidence_shas(issue, linked_prs)
    catalog_link = f"../markhand-web/{issue.catalog_html_path}"
    phase_link = f"../markhand-web/{issue.phase_plan_html_path}"
    source_issue = (
        f"[#{github_issue.number}]({github_issue.url})"
        if github_issue
        else "UNKNOWN — matching GitHub issue was not resolved during backfill"
    )
    objective = issue.fields.get("Objective", "").strip()
    if not objective:
        objective = (
            "Not separately recorded in the compact catalog card. "
            f"The recorded outcome is the issue title: **{issue.title}**."
        )

    combined_plan = issue.fields.get("Plan/files", "").strip()
    implementation = (
        issue.fields.get("Implementation plan", "").strip()
        or combined_plan
        or "UNKNOWN — no separate implementation plan was recorded in the catalog card."
    )
    files = issue.fields.get("Files/modules", "").strip()
    if not files:
        files = (
            "The source catalog records implementation and file scope together; "
            "see **Implementation plan** above."
            if combined_plan
            else "UNKNOWN — no separate file/module inventory was recorded."
        )

    combined_acceptance = issue.fields.get("Acceptance/tests", "").strip()
    acceptance = (
        issue.fields.get("Acceptance criteria", "").strip()
        or combined_acceptance
        or "UNKNOWN — no separate acceptance criteria were recorded."
    )
    tests = (
        issue.fields.get("Required tests / evidence", "").strip()
        or combined_acceptance
        or "UNKNOWN — no separate test/evidence command was recorded."
    )
    status_evidence = status_without_done(issue.fields.get("Status", "Done"))

    pr_lines = (
        "\n".join(
            f"- [PR #{pr.number}]({pr.url}) — {pr.title}"
            + (f"; merged `{pr.merged_at}`" if pr.merged_at else "")
            for pr in linked_prs
        )
        if linked_prs
        else "- UNKNOWN — no implementation PR is cited in the catalog status."
    )
    sha_lines = (
        "\n".join(f"- `{sha}`" for sha in shas)
        if shas
        else "- UNKNOWN — no completion/evidence commit is cited in the catalog status."
    )
    close_line = (
        f"- GitHub sync-closed timestamp: `{github_issue.closed_at}` "
        "(recorded for traceability; not treated as the delivery date)."
        if github_issue and github_issue.closed_at
        else "- GitHub sync-closed timestamp: UNKNOWN."
    )

    return f"""<!-- {PLAN_MARKER}: {issue.issue_id} -->
# {issue.issue_id} — {issue.title}

Date: 2026-08-04 (backfill authoring date; not the historical completion date)
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: {source_issue}
Catalog: [`{issue.catalog_html_path}`]({catalog_link})
Phase plan: [`{issue.phase_plan_html_path}`]({phase_link})
Status: Done

> Provenance: this plan was backfilled after completion from the catalog card and
> its cited evidence. It preserves recorded intent; it is not evidence that this
> standalone file existed before implementation, and it does not invent missing history.

## Objective

{objective}

## Context

- Phase: `{issue.phase_code}`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

{markdown_quote(status_evidence)}

## Implementation plan

{implementation}

## Files/modules

{files}

## Dependencies / blocks

{field_or_unknown(issue, "Dependencies / blocks", "not recorded in the catalog card.")}

## Acceptance criteria

{acceptance}

## Required tests / evidence

{tests}

## Security and migration notes

{field_or_unknown(issue, "Security and migration notes", "not recorded in the catalog card.")}

## Out of scope

{field_or_unknown(issue, "Out of scope", "not recorded in the catalog card.")}

## Delivery evidence

### Implementation PRs

{pr_lines}

### Completion/evidence commits

{sha_lines}

{close_line}

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- This backfill indexes existing evidence; it does not independently re-certify the
  historical implementation.
"""


def link_for(filename: str, issue_id: str) -> str:
    return (
        f"- **Plan file:** [{issue_id} detailed implementation plan]"
        f"(../../../../reports/{filename})"
    )


def insertion_offset(issue: IssueRecord) -> int:
    status_match = re.search(
        r"(?m)^- \*\*Status:\*\*.*$", issue.section,
    )
    if status_match is None:
        raise ValueError(f"{issue.issue_id}: Done issue has no explicit Status field")
    for match in re.finditer(r"(?m)^- \*\*(?P<key>[^*]+?):\*\*.*$", issue.section):
        if match.start() <= status_match.start():
            continue
        if canonical_key(match.group("key")) != "Status":
            return issue.section_start + match.start()
    return issue.section_end


def write_plans(records: list[IssueRecord], stamp: str, use_github: bool) -> None:
    if not STAMP_PATTERN.fullmatch(stamp):
        raise ValueError("--stamp must use YYMMDD-HHMM")
    done = [issue for issue in records if issue.status == "done"]
    github_issues: dict[str, GitHubIssue] = {}
    github_prs: dict[int, GitHubPr] = {}
    if use_github:
        github_issues = fetch_github_issues(records)
        github_prs = fetch_github_prs()

    REPORTS_DIR.mkdir(parents=True, exist_ok=True)
    catalog_insertions: dict[Path, list[tuple[int, str]]] = {}
    for issue in done:
        existing = PLAN_LINK_PATTERN.search(issue.section)
        if existing:
            continue
        filename = filename_for(issue, stamp)
        destination = REPORTS_DIR / filename
        if destination.exists():
            raise FileExistsError(f"refusing to overwrite {destination}")
        destination.write_text(
            render_plan(issue, github_issues.get(issue.issue_id), github_prs),
            encoding="utf-8",
        )
        catalog_insertions.setdefault(issue.catalog, []).append(
            (insertion_offset(issue), link_for(filename, issue.issue_id))
        )

    for catalog, insertions in catalog_insertions.items():
        markdown = catalog.read_text(encoding="utf-8")
        for offset, link in sorted(insertions, reverse=True):
            markdown = f"{markdown[:offset]}{link}\n{markdown[offset:]}"
        catalog.write_text(markdown, encoding="utf-8")
    print(
        f"wrote {sum(len(items) for items in catalog_insertions.values())} "
        f"Done issue plans across {len(catalog_insertions)} catalogs"
    )


def resolve_plan_path(issue: IssueRecord, relative: str) -> Path:
    return (issue.catalog.parent / relative).resolve()


def check_plans(records: list[IssueRecord]) -> list[str]:
    errors: list[str] = []
    done = [issue for issue in records if issue.status == "done"]
    seen_targets: dict[Path, str] = {}
    for issue in done:
        links = list(PLAN_LINK_PATTERN.finditer(issue.section))
        if len(links) != 1:
            errors.append(
                f"{issue.issue_id}: expected exactly one Plan file link, got {len(links)}"
            )
            continue
        target = resolve_plan_path(issue, links[0].group("path"))
        previous = seen_targets.get(target)
        if previous:
            errors.append(
                f"{issue.issue_id}: plan target also used by {previous}: {target}"
            )
        seen_targets[target] = issue.issue_id
        if not target.is_file():
            errors.append(f"{issue.issue_id}: missing plan file {target}")
            continue
        content = target.read_text(encoding="utf-8")
        marker = f"<!-- {PLAN_MARKER}: {issue.issue_id} -->"
        if marker not in content:
            errors.append(f"{issue.issue_id}: missing marker in {target}")
        for heading in (
            "## Objective",
            "## Context",
            "## Implementation plan",
            "## Files/modules",
            "## Dependencies / blocks",
            "## Acceptance criteria",
            "## Required tests / evidence",
            "## Security and migration notes",
            "## Out of scope",
            "## Delivery evidence",
            "## Definition of done",
        ):
            if heading not in content:
                errors.append(f"{issue.issue_id}: {target} missing {heading}")

    for issue in records:
        if issue.status != "done" and PLAN_LINK_PATTERN.search(issue.section):
            errors.append(f"{issue.issue_id}: non-Done issue must not have a backfill plan")

    generated = {
        path.resolve()
        for path in REPORTS_DIR.glob("plan-*.md")
        if f"<!-- {PLAN_MARKER}:" in path.read_text(encoding="utf-8")
    }
    unreferenced = generated - set(seen_targets)
    for path in sorted(unreferenced):
        errors.append(f"unreferenced generated plan: {path}")
    if not errors:
        print(f"Done issue plans OK ({len(done)} plans)")
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    parser.add_argument("--stamp", help="Backfill filename stamp (YYMMDD-HHMM)")
    parser.add_argument(
        "--no-github",
        action="store_true",
        help="Do not resolve direct GitHub issue/PR evidence links",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    records = load_records()
    if args.write:
        if not args.stamp:
            print("--write requires --stamp YYMMDD-HHMM", file=sys.stderr)
            return 2
        write_plans(records, args.stamp, not args.no_github)
        records = load_records()
    errors = check_plans(records)
    if errors:
        print("Done issue plan validation FAILED:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
