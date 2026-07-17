#!/usr/bin/env python3
"""Sync Markhand Web backlog catalogs to GitHub issues."""

from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import sys
import tempfile
import textwrap
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import importlib.util

_spec = importlib.util.spec_from_file_location(
    "build_roadmap", ROOT / "scripts/build-roadmap.py"
)
roadmap = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["build_roadmap"] = roadmap
_spec.loader.exec_module(roadmap)


REPO = "anhnth24/project-example"
PHASE_LABELS = {
    "F": ("web-foundation", "Phase F — Engineering foundation"),
    "0": ("web-p0", "Phase 0 — Discovery & Gates"),
    "1A": ("web-p1a", "Phase 1A — Knowledge Extraction"),
    "1B": ("web-p1b", "Phase 1B — Single-org POC"),
    "1C": ("web-p1c", "Phase 1C — Multi-org Security"),
    "2": ("web-p2", "Phase 2 — Web SPA MVP"),
    "3": ("web-p3", "Phase 3 — Document Intelligence"),
    "4": ("web-p4", "Phase 4 — Production Hardening"),
}
SHARED_LABELS = {
    "markhand-web": "Markhand Web delivery backlog",
    "docs": "Planning/spec issue",
}
STATUS_LABELS = {
    "ready": ("ready", "Issue ready to start"),
    "blocked": ("blocked", "Blocked by dependency or gate"),
    "backlog": ("backlog", "Milestone not yet activated"),
}
INLINE_FIELD_PATTERN = re.compile(r"\*\*(?P<key>[^*]+?):\*\*\s*")
SECTION_ALIASES = {
    "objective": "Objective",
    "implementation plan": "Implementation plan",
    "plan": "Implementation plan",
    "files/modules": "Files/modules",
    "files": "Files/modules",
    "dependencies/blocks": "Dependencies / blocks",
    "depends": "Dependencies / blocks",
    "dependencies": "Dependencies / blocks",
    "acceptance criteria": "Acceptance criteria",
    "acceptance": "Acceptance criteria",
    "required tests/evidence": "Required tests / evidence",
    "tests/evidence": "Required tests / evidence",
    "acceptance/tests": "Required tests / evidence",
    "security/migration": "Security and migration notes",
    "security": "Security and migration notes",
    "out of scope": "Out of scope",
    "out": "Out of scope",
}


@dataclass(frozen=True)
class CatalogIssue:
    phase_code: str
    issue_id: str
    title: str
    status: str
    catalog_path: str
    plan_path: str
    fields: dict[str, str]

    @property
    def github_title(self) -> str:
        return f"{self.issue_id} — {self.title}"


def gh_json(args: list[str]) -> object:
    command = ["gh", *args, "--repo", REPO]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    if not result.stdout.strip():
        return None
    return json.loads(result.stdout)


def gh_run(args: list[str]) -> None:
    command = ["gh", *args, "--repo", REPO]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def normalize_key(raw: str) -> str:
    return re.sub(r"\s+", " ", raw.strip().lower())


def parse_section_fields(section: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    current_key: str | None = None
    current_lines: list[str] = []

    def flush() -> None:
        nonlocal current_key, current_lines
        if current_key and current_lines:
            value = "\n".join(line.rstrip() for line in current_lines).strip()
            if value:
                if current_key in fields:
                    fields[current_key] = f"{fields[current_key]}\n{value}".strip()
                else:
                    fields[current_key] = value
        current_key = None
        current_lines = []

    for raw_line in section.splitlines():
        stripped = raw_line.strip()
        if not stripped:
            if current_key and current_lines:
                current_lines.append("")
            continue
        if stripped.startswith("- **"):
            matches = list(INLINE_FIELD_PATTERN.finditer(stripped))
            if matches:
                flush()
                for index, match in enumerate(matches):
                    start = match.end()
                    end = (
                        matches[index + 1].start()
                        if index + 1 < len(matches)
                        else len(stripped)
                    )
                    value = stripped[start:end].strip()
                    canonical = SECTION_ALIASES.get(normalize_key(match.group("key")))
                    if not canonical:
                        continue
                    if index == 0:
                        current_key = canonical
                        current_lines = [value] if value else []
                    elif value:
                        if canonical in fields:
                            fields[canonical] = f"{fields[canonical]}\n{value}".strip()
                        else:
                            fields[canonical] = value
                continue
        if current_key is not None:
            current_lines.append(stripped)
    flush()
    return fields


def load_catalog_issues() -> list[CatalogIssue]:
    configs, expected = roadmap.parse_registry()
    issues: list[CatalogIssue] = []
    for config in configs:
        markdown = config.catalog.read_text(encoding="utf-8")
        default_matches = roadmap.DEFAULT_STATUS_PATTERN.findall(markdown)
        default_status = roadmap.normalize_status(
            default_matches[0], source=config.catalog
        )
        content = roadmap.mask_non_content(markdown)
        phase_issues: list[CatalogIssue] = []
        for match in roadmap.ISSUE_PATTERN.finditer(content):
            heading_level = len(match.group("heading"))
            section_end = len(content)
            for next_heading in roadmap.HEADING_PATTERN.finditer(content, match.end()):
                if len(next_heading.group("heading")) <= heading_level:
                    section_end = next_heading.start()
                    break
            section = content[match.end() : section_end]
            status_fields = list(roadmap.STATUS_FIELD_PATTERN.finditer(section))
            status = default_status
            if status_fields:
                raw_status = status_fields[0].group("value").strip()
                status_match = roadmap.STATUS_VALUE_PATTERN.match(raw_status)
                if status_match:
                    status = roadmap.normalize_status(
                        status_match.group(1), source=config.catalog
                    )
            phase_issues.append(
                CatalogIssue(
                    phase_code=config.code,
                    issue_id=match.group("id").strip(),
                    title=match.group("title").strip(),
                    status=status,
                    catalog_path=config.html_catalog,
                    plan_path=config.html_plan,
                    fields=parse_section_fields(section),
                )
            )
        roadmap.validate_phase_issue_ids(
            config, tuple(item.issue_id for item in phase_issues)
        )
        issues.extend(phase_issues)
    if len(issues) != expected:
        raise ValueError(f"Expected {expected} issues, loaded {len(issues)}")
    return issues


def render_body(issue: CatalogIssue) -> str:
    phase_label, milestone_title = PHASE_LABELS[issue.phase_code]
    sections = [
        ("Metadata", textwrap.dedent(
            f"""
            - Milestone: {milestone_title}
            - Phase code: {issue.phase_code}
            - Issue ID: {issue.issue_id}
            - Status: `{issue.status}`
            - Catalog: `{issue.catalog_path}`
            - Phase plan: `{issue.plan_path}`
            """
        ).strip()),
    ]
    ordered = [
        "Objective",
        "Implementation plan",
        "Files/modules",
        "Dependencies / blocks",
        "Acceptance criteria",
        "Required tests / evidence",
        "Security and migration notes",
        "Out of scope",
    ]
    for key in ordered:
        value = issue.fields.get(key)
        if value:
            sections.append((key, value))
    body_parts = []
    for heading, value in sections:
        body_parts.append(f"## {heading}\n\n{value.strip()}\n")
    body_parts.append(
        "## Source\n\n"
        "Generated from Markhand Web backlog catalog. "
        "Update the catalog first, then re-run "
        "`python3 scripts/sync-github-issues.py --update` if specs change.\n"
    )
    return "\n".join(body_parts)


def ensure_labels() -> None:
    try:
        existing = {
            item["name"]
            for item in gh_json(["label", "list", "--limit", "200", "--json", "name"])  # type: ignore[index]
        }
    except RuntimeError as error:
        if "403" in str(error):
            print("warning: không có quyền tạo label; dùng label sẵn có nếu tạo issue")
            return
        raise
    desired = {
        **SHARED_LABELS,
        **{name: f"Phase {code} milestone" for code, (name, _) in PHASE_LABELS.items()},
        **{name: description for name, description in STATUS_LABELS.values()},
    }
    for name, description in desired.items():
        if name in existing:
            continue
        try:
            gh_run(["label", "create", name, "--description", description, "--color", "5319e7"])
        except RuntimeError as error:
            if "403" in str(error):
                print(f"warning: bỏ qua label {name} (403)")
                continue
            raise


def ensure_milestones() -> dict[str, int]:
    existing = gh_json(["api", "repos/anhnth24/project-example/milestones?state=all&per_page=100"])
    by_title = {item["title"]: item["number"] for item in existing or []}  # type: ignore[union-attr]
    milestone_ids: dict[str, int] = {}
    for code, (_, title) in PHASE_LABELS.items():
        if title in by_title:
            milestone_ids[code] = by_title[title]
            continue
        created = gh_json(
            [
                "api",
                "-X",
                "POST",
                "repos/anhnth24/project-example/milestones",
                "-f",
                f"title={title}",
                "-f",
                "state=open",
            ]
        )
        milestone_ids[code] = created["number"]  # type: ignore[index]
    return milestone_ids


def export_shell(issues: list[CatalogIssue], path: Path) -> None:
    lines = [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        f"REPO={REPO!r}",
        "",
        "ensure_milestone() {",
        "  local title=\"$1\"",
        "  gh api \"repos/${REPO}/milestones?state=all&per_page=100\" --jq \".[] | select(.title==\\\"$title\\\") | .number\" | head -n1 || true",
        "}",
        "",
        "create_if_missing() {",
        "  local title=\"$1\"",
        "  if gh issue list --repo \"$REPO\" --state all --search \"in:title \\\"$title\\\"\" --json number --jq 'length' | grep -qv '^0$'; then",
        "    echo \"skip existing: $title\"",
        "    return 0",
        "  fi",
        "  shift",
        "  gh issue create --repo \"$REPO\" \"$@\"",
        "}",
        "",
    ]
    seen_milestones: set[str] = set()
    for issue in issues:
        milestone = PHASE_LABELS[issue.phase_code][1]
        if milestone not in seen_milestones:
            seen_milestones.add(milestone)
            lines.extend(
                [
                    f"gh api --method POST repos/${{REPO}}/milestones -f title={shlex.quote(milestone)} -f state=open >/dev/null 2>&1 || true",
                    "",
                ]
            )
    for issue in issues:
        milestone = PHASE_LABELS[issue.phase_code][1]
        labels = ",".join(issue_labels(issue))
        lines.extend(
            [
                f"create_if_missing {json.dumps(issue.github_title)} \\",
                f"  --title {json.dumps(issue.github_title)} \\",
                f"  --body {json.dumps(render_body(issue))} \\",
                f"  --milestone {json.dumps(milestone)} \\",
                f"  --label {json.dumps(labels)}",
                "",
            ]
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    path.chmod(0o755)


def export_json(issues: list[CatalogIssue], path: Path) -> None:
    payload = [
        {
            "phase": issue.phase_code,
            "id": issue.issue_id,
            "title": issue.github_title,
            "status": issue.status,
            "milestone": PHASE_LABELS[issue.phase_code][1],
            "labels": issue_labels(issue),
            "body": render_body(issue),
            "catalog": issue.catalog_path,
        }
        for issue in issues
    ]
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def existing_issues_by_title() -> dict[str, int]:
    items: dict[str, int] = {}
    for page in range(1, 11):
        batch = gh_json(
            [
                "api",
                f"repos/anhnth24/project-example/issues?state=all&per_page=100&page={page}",
            ]
        )
        if not batch:
            break
        for item in batch:  # type: ignore[union-attr]
            if "pull_request" in item:
                continue
            items[item["title"]] = item["number"]
        if len(batch) < 100:  # type: ignore[arg-type]
            break
    return items


def issue_labels(issue: CatalogIssue) -> list[str]:
    phase_label, _ = PHASE_LABELS[issue.phase_code]
    labels = ["markhand-web", "docs", phase_label]
    status_label = STATUS_LABELS.get(issue.status)
    if status_label:
        labels.append(status_label[0])
    return labels


def create_issue(issue: CatalogIssue, milestone_id: int) -> int:
    labels = issue_labels(issue)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".md", delete=False) as handle:
        handle.write(render_body(issue))
        body_path = handle.name
    try:
        command = [
            "issue",
            "create",
            "--title",
            issue.github_title,
            "--body-file",
            body_path,
            "--milestone",
            PHASE_LABELS[issue.phase_code][1],
        ]
        for label in labels:
            command.extend(["--label", label])
        result = subprocess.run(
            ["gh", *command, "--repo", REPO],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())
        match = re.search(r"/issues/(\d+)\s*$", result.stdout.strip())
        if not match:
            raise RuntimeError(f"Không parse được issue number: {result.stdout.strip()}")
        return int(match.group(1))
    finally:
        Path(body_path).unlink(missing_ok=True)


def update_issue(number: int, issue: CatalogIssue) -> None:
    labels = issue_labels(issue)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".md", delete=False) as handle:
        handle.write(render_body(issue))
        body_path = handle.name
    try:
        gh_run(
            [
                "issue",
                "edit",
                str(number),
                "--title",
                issue.github_title,
                "--body-file",
                body_path,
                "--milestone",
                PHASE_LABELS[issue.phase_code][1],
            ]
        )
        gh_run(["issue", "edit", str(number), "--add-label", *labels])
    finally:
        Path(body_path).unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Sync Markhand Web backlog to GitHub issues.")
    parser.add_argument("--dry-run", action="store_true", help="Print actions only.")
    parser.add_argument("--create", action="store_true", help="Create missing issues.")
    parser.add_argument("--update", action="store_true", help="Update existing issue bodies.")
    parser.add_argument(
        "--export-shell",
        type=Path,
        help="Write bash script with gh issue create commands.",
    )
    parser.add_argument(
        "--export-json",
        type=Path,
        help="Write JSON payload for all issues.",
    )
    args = parser.parse_args()
    if not any(
        [args.dry_run, args.create, args.update, args.export_shell, args.export_json]
    ):
        parser.error("Specify an action flag")

    issues = load_catalog_issues()
    print(f"Loaded {len(issues)} catalog issues")

    if args.dry_run:
        for issue in issues:
            print(f"[{issue.phase_code}] {issue.github_title} ({issue.status})")
        return 0

    if args.export_shell:
        export_shell(issues, args.export_shell)
        print(f"exported shell script: {args.export_shell}")
    if args.export_json:
        export_json(issues, args.export_json)
        print(f"exported json: {args.export_json}")
    if args.export_shell or args.export_json:
        if not (args.create or args.update):
            return 0

    try:
        ensure_labels()
        milestone_ids = ensure_milestones()
    except RuntimeError as error:
        if "403" in str(error):
            print(
                "error: token hiện tại không có quyền tạo issue/milestone trên GitHub.\n"
                "Chạy lại trên máy local với PAT có scope `repo`/`issues`, hoặc dùng:\n"
                "  python3 scripts/sync-github-issues.py --export-shell plans/markhand-web/backlog/create-github-issues.sh\n"
                "  bash plans/markhand-web/backlog/create-github-issues.sh",
                file=sys.stderr,
            )
            return 1
        raise
    existing = existing_issues_by_title()
    created = updated = skipped = 0

    for issue in issues:
        if issue.github_title in existing:
            if args.update:
                update_issue(existing[issue.github_title], issue)
                updated += 1
                print(f"updated #{existing[issue.github_title]} {issue.github_title}")
            else:
                skipped += 1
            continue
        if not args.create:
            skipped += 1
            continue
        number = create_issue(issue, milestone_ids[issue.phase_code])
        created += 1
        print(f"created #{number} {issue.github_title}")

    print(
        f"done: created={created}, updated={updated}, skipped={skipped}, "
        f"milestones={len(milestone_ids)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
