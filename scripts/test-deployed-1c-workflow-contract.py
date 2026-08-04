#!/usr/bin/env python3
"""Hermetic structural contract tests for deployed-1c-integration CI wiring."""

from __future__ import annotations

import argparse
import json
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
ENV_EXAMPLE = REPO_ROOT / "deploy/.env.example"
COMPOSE_POC = REPO_ROOT / "deploy/compose.poc.yml"
MINIO_POLICY_TEMPLATE = REPO_ROOT / "deploy/poc/minio-app-policy.json.tmpl"
JOB_NAME = "deployed-1c-integration"

NARROW_MINIO_ACCESS_DEFAULT = "${MARKHAND_MINIO_ACCESS_KEY:-markhand_app}"
NARROW_MINIO_SECRET_DEFAULT = "${MARKHAND_MINIO_SECRET_KEY:-markhand_app_poc_change_me}"

BOOTSTRAP_MINIO_SERVICES = frozenset({"minio-init", "minio", "minio-restore-green"})

MINIO_CREDENTIAL_REFERENCE_KEYS = frozenset(
    {
        "MARKHAND_MINIO_ACCESS_KEY",
        "MARKHAND_MINIO_SECRET_KEY",
        "MARKHAND_MINIO_ROOT_USER",
        "MARKHAND_MINIO_ROOT_PASSWORD",
        "MINIO_ROOT_USER",
        "MINIO_ROOT_PASSWORD",
    }
)

MINIO_NARROW_CREDENTIAL_ENV_KEYS = frozenset(
    {"MARKHAND_MINIO_ACCESS_KEY", "MARKHAND_MINIO_SECRET_KEY"}
)

RESTORE_GREEN_MINIO_ROOT_USER = "${MARKHAND_MINIO_ACCESS_KEY:-markhand_app}"
RESTORE_GREEN_MINIO_ROOT_PASSWORD = "${MARKHAND_MINIO_SECRET_KEY:-markhand_app_poc_change_me}"

EXTENSION_BLOCK_RE = re.compile(r"^(x-[a-z0-9_-]+):\s*(?:&([a-z0-9_-]+))?\s*$", re.IGNORECASE)
ANCHOR_DEF_RE = re.compile(r"(?<![A-Za-z0-9_-])&([a-z0-9_-]+)(?![A-Za-z0-9_-])")
ANCHOR_ALIAS_RE = re.compile(r"(?<![A-Za-z0-9_-])\*([a-z0-9_-]+)(?![A-Za-z0-9_-])")
EXPLICIT_RUNTIME_SERVICE_NAMES = frozenset({"migrate", "api"})
EXPLICIT_RUNTIME_SERVICE_PREFIXES = ("api", "worker-")

CANONICAL_POLICY_VERSION = "2012-10-17"
POLICY_TOP_LEVEL_KEYS = frozenset({"Version", "Statement"})
STATEMENT_KEYS = frozenset({"Sid", "Effect", "Action", "Resource"})

CANONICAL_STATEMENTS_BY_SID: dict[str, dict[str, object]] = {
    "MarkhandDocumentsBucket": {
        "Sid": "MarkhandDocumentsBucket",
        "Effect": "Allow",
        "Action": ["s3:GetBucketLocation", "s3:ListBucket"],
        "Resource": ["arn:aws:s3:::__BUCKET__"],
    },
    "MarkhandDocumentsObjects": {
        "Sid": "MarkhandDocumentsObjects",
        "Effect": "Allow",
        "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
        "Resource": [
            "arn:aws:s3:::__BUCKET__/quarantine/*",
            "arn:aws:s3:::__BUCKET__/trusted/*",
        ],
    },
}

REQUIRED_STEP_ORDER = (
    "Boot POC Compose stack",
    "Run Phase 1C denial suite (deployed POC stack)",
    "Tear down POC stack",
    "Render Phase 1C denial report",
    "Upload 1C integration report",
    "Enforce 1C-12 deployed gate",
)

RENDER_ARG_ORDER = (
    "--input",
    "--output",
    "--expected-git-sha",
    "--expected-manifest-sha256",
    "--expected-git-ref",
    "--ci-run-url",
    "--runner-exit-code",
    "--teardown-exit-code",
    "--input-failure-category",
)

PHASE1C_SOURCE_REF_ENV = "${{ github.head_ref || github.ref_name }}"

ARTIFACT_PATHS = (
    "${{ runner.temp }}/markhand-1c-integration/manifest-run.json",
    "${{ runner.temp }}/markhand-1c-integration/phase1c-denial-report.md",
)


@dataclass(frozen=True)
class WorkflowStep:
    name: str
    run: str | None
    if_condition: str | None
    step_id: str | None
    uses: str | None
    with_block: str | None = None
    env: dict[str, str] | None = None


def extract_job_block(workflow_text: str, job_name: str) -> str:
    pattern = rf"^  {re.escape(job_name)}:\n"
    match = re.search(pattern, workflow_text, flags=re.MULTILINE)
    if match is None:
        raise ValueError(f"job {job_name!r} not found")
    start = match.start()
    tail = workflow_text[start + 1 :]
    next_job = re.search(r"^  [A-Za-z0-9_-]+:\n", tail, flags=re.MULTILINE)
    end = start + 1 + (next_job.start() if next_job else len(tail))
    return workflow_text[start:end]


def strip_shell_comments(script: str) -> str:
    cleaned: list[str] = []
    for line in script.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("#"):
            continue
        without_trailing = re.sub(r"(?<!\\)#.*$", "", line).rstrip()
        if without_trailing:
            cleaned.append(without_trailing)
    return "\n".join(cleaned)


def parse_step_env(chunk: str) -> dict[str, str]:
    match = re.search(r"^        env:\n((?:          .+\n)*)", chunk, flags=re.MULTILINE)
    if match is None:
        return {}
    env: dict[str, str] = {}
    for line in match.group(1).splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        entry = re.sub(r"\s+#.*$", "", stripped)
        if not entry or entry.startswith("#"):
            continue
        key_match = re.match(r"^([A-Z0-9_]+):\s*(.+)$", entry)
        if key_match is None:
            continue
        key = key_match.group(1)
        value = key_match.group(2).strip().strip('"').strip("'")
        env[key] = value
    return env


def parse_job_env(job_block: str) -> dict[str, str]:
    match = re.search(r"^    env:\n((?:      .+\n)*)", job_block, flags=re.MULTILINE)
    if match is None:
        return {}
    env: dict[str, str] = {}
    for line in match.group(1).splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        entry = re.sub(r"\s+#.*$", "", stripped)
        if not entry or entry.startswith("#"):
            continue
        key_match = re.match(r"^([A-Z0-9_]+):\s*(.+)$", entry)
        if key_match is None:
            continue
        key = key_match.group(1)
        value = key_match.group(2).strip().strip('"').strip("'")
        env[key] = value
    return env


def parse_job_steps(job_block: str) -> list[WorkflowStep]:
    steps: list[WorkflowStep] = []
    chunks = re.split(r"\n      - ", job_block)
    for chunk in chunks[1:]:
        name_match = re.match(r"name: (.+)\n", chunk)
        if name_match is None:
            continue
        name = name_match.group(1).strip()
        step_id = None
        id_match = re.search(r"^        id: (.+)\n", chunk, flags=re.MULTILINE)
        if id_match:
            step_id = id_match.group(1).strip()
        if_condition = None
        if_match = re.search(r"^        if: (.+)\n", chunk, flags=re.MULTILINE)
        if if_match:
            if_condition = if_match.group(1).strip()
        uses = None
        uses_match = re.search(r"^        uses: (.+)\n", chunk, flags=re.MULTILINE)
        if uses_match:
            uses = uses_match.group(1).strip()
        run = None
        run_match = re.search(r"^        run: \|\n((?:          .*\n?)*)", chunk, flags=re.MULTILINE)
        if run_match:
            run = "\n".join(line[10:] for line in run_match.group(1).splitlines())
        with_block = None
        with_match = re.search(r"^        with:\n((?:          .*\n?)*)", chunk, flags=re.MULTILINE)
        if with_match:
            with_block = with_match.group(1)
        steps.append(
            WorkflowStep(
                name=name,
                run=run,
                if_condition=if_condition,
                step_id=step_id,
                uses=uses,
                with_block=with_block,
                env=parse_step_env(chunk),
            )
        )
    return steps


def step_index(steps: Sequence[WorkflowStep], name: str) -> int:
    for index, step in enumerate(steps):
        if step.name == name:
            return index
    raise ValueError(f"step {name!r} not found")


def args_in_order(text: str, args: Sequence[str]) -> bool:
    position = 0
    for arg in args:
        index = text.find(arg, position)
        if index < 0:
            return False
        position = index + len(arg)
    return True


def parse_upload_paths(with_block: str | None) -> list[str]:
    if with_block is None:
        return []
    path_match = re.search(r"^          path: \|\n((?:            .+\n)*)", with_block, flags=re.MULTILINE)
    if path_match is None:
        single = re.search(r"^          path: (.+)\n", with_block, flags=re.MULTILINE)
        return [single.group(1).strip()] if single else []
    return [
        line.strip()
        for line in path_match.group(1).splitlines()
        if line.strip()
    ]


def parse_env_example(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, _, value = stripped.partition("=")
        env[key.strip()] = value.strip()
    return env


@dataclass(frozen=True)
class ComposeServicesParse:
    blocks: dict[str, str]
    errors: list[str]


CANONICAL_DIRECT_SERVICE_KEY = re.compile(r"^  ([a-z0-9_-]+):\s*$")
CANONICAL_SERVICE_KEY_ERROR = "canonical unquoted service key grammar"


def is_direct_child_services_line(line: str) -> bool:
    return line.startswith("  ") and not line.startswith("    ")


def is_skipped_direct_child_line(line: str) -> bool:
    stripped = line.strip()
    return not stripped or stripped.startswith("#")


def is_canonical_direct_service_key_line(line: str) -> bool:
    return CANONICAL_DIRECT_SERVICE_KEY.match(line) is not None


def parse_compose_services(compose_text: str) -> ComposeServicesParse:
    lines, errors = _services_section_lines(compose_text)
    if not lines and errors:
        return ComposeServicesParse({}, errors)

    key_records: list[tuple[int, str]] = []
    for index, line in enumerate(lines):
        if not is_direct_child_services_line(line):
            continue
        if is_skipped_direct_child_line(line):
            continue
        match = CANONICAL_DIRECT_SERVICE_KEY.match(line)
        if match:
            key_records.append((index, match.group(1)))
            continue
        errors.append(
            "deploy/compose.poc.yml services direct-child line must use "
            f"{CANONICAL_SERVICE_KEY_ERROR} (`  [a-z0-9_-]+:`): {line.strip()!r}"
        )

    blocks: dict[str, str] = {}
    for record_index, (line_index, name) in enumerate(key_records):
        start = line_index + 1
        end = (
            key_records[record_index + 1][0]
            if record_index + 1 < len(key_records)
            else len(lines)
        )
        body = "\n".join(lines[start:end])
        if name in blocks:
            errors.append(f"deploy/compose.poc.yml duplicate service key {name!r}")
        blocks[name] = body

    return ComposeServicesParse(blocks, errors)


def compose_services_contract_errors(compose_text: str) -> list[str]:
    return parse_compose_services(compose_text).errors


def parse_compose_service_blocks(compose_text: str) -> dict[str, str]:
    return parse_compose_services(compose_text).blocks


def reassemble_services_section(compose_text: str, new_service_lines: list[str]) -> str:
    match = re.search(r"^services:\n", compose_text, flags=re.MULTILINE)
    if match is None:
        return compose_text
    before = compose_text[: match.end()]
    after_rest = compose_text[match.end() :]
    stop = re.search(r"^(?:networks|volumes|secrets|configs):", after_rest, flags=re.MULTILINE)
    after = after_rest[stop.start() :] if stop else ""
    body = "\n".join(new_service_lines)
    if body:
        body += "\n"
    return before + body + after


def _services_section_lines(compose_text: str) -> tuple[list[str], list[str]]:
    match = re.search(r"^services:\n", compose_text, flags=re.MULTILINE)
    if match is None:
        return [], ["deploy/compose.poc.yml missing services: mapping"]
    lines: list[str] = []
    for line in compose_text[match.end() :].splitlines():
        if re.match(r"^(?:networks|volumes|secrets|configs):", line):
            break
        lines.append(line)
    return lines, []


def compose_minio_contract_errors(
    compose_text: str,
    example: dict[str, str],
) -> list[str]:
    errors = compose_services_contract_errors(compose_text)
    errors.extend(compose_extension_and_anchor_minio_errors(compose_text))
    errors.extend(minio_restore_green_invariant_errors(compose_text))
    errors.extend(poc_runtime_minio_credential_errors(compose_text, example))
    return errors


def _compose_prefix_before_services(compose_text: str) -> str:
    match = re.search(r"^services:\n", compose_text, flags=re.MULTILINE)
    if match is None:
        return compose_text
    return compose_text[: match.start()]


def _compose_extension_blocks(prefix: str) -> list[tuple[str, str]]:
    blocks: list[tuple[str, str]] = []
    current_name: str | None = None
    current_lines: list[str] = []
    for line in prefix.splitlines():
        extension_match = EXTENSION_BLOCK_RE.match(line)
        if extension_match:
            if current_name is not None:
                blocks.append((current_name, "\n".join(current_lines)))
            current_name = extension_match.group(1)
            current_lines = [line]
            continue
        if current_name is not None:
            if not line.strip():
                current_lines.append(line)
                continue
            if line.startswith(" "):
                current_lines.append(line)
                continue
            blocks.append((current_name, "\n".join(current_lines)))
            current_name = None
            current_lines = []
    if current_name is not None:
        blocks.append((current_name, "\n".join(current_lines)))
    return blocks


def _yaml_anchor_definition_blocks(text: str) -> dict[str, str]:
    blocks: dict[str, str] = {}
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        anchor_match = ANCHOR_DEF_RE.search(lines[index])
        if anchor_match is None:
            index += 1
            continue
        name = anchor_match.group(1)
        base_indent = len(lines[index]) - len(lines[index].lstrip())
        body = [lines[index]]
        index += 1
        while index < len(lines):
            if not lines[index].strip():
                body.append(lines[index])
                index += 1
                continue
            indent = len(lines[index]) - len(lines[index].lstrip())
            if indent <= base_indent:
                break
            body.append(lines[index])
            index += 1
        blocks[name] = "\n".join(body)
    return blocks


def compose_extension_and_anchor_minio_errors(compose_text: str) -> list[str]:
    errors: list[str] = []
    prefix = _compose_prefix_before_services(compose_text)

    for extension_name, block in _compose_extension_blocks(prefix):
        if service_has_minio_credential_references(block):
            errors.append(
                "deploy/compose.poc.yml extension "
                f"{extension_name!r} must not define MinIO credential references"
            )

    forbidden_anchors: set[str] = set()
    for anchor_name, block in _yaml_anchor_definition_blocks(compose_text).items():
        if service_has_minio_credential_references(block):
            forbidden_anchors.add(anchor_name)
            errors.append(
                "deploy/compose.poc.yml YAML anchor "
                f"&{anchor_name} must not define MinIO credential references"
            )

    for match in ANCHOR_ALIAS_RE.finditer(compose_text):
        anchor_name = match.group(1)
        if anchor_name in forbidden_anchors:
            errors.append(
                "deploy/compose.poc.yml must not alias MinIO credential anchor "
                f"*{anchor_name}"
            )
    return errors


def minio_restore_green_invariant_errors(compose_text: str) -> list[str]:
    errors: list[str] = []
    blocks = parse_compose_service_blocks(compose_text)
    block = blocks.get("minio-restore-green")
    if block is None:
        return ["deploy/compose.poc.yml missing minio-restore-green service"]
    env = parse_service_environment(block)
    root_user = env.get("MINIO_ROOT_USER")
    root_password = env.get("MINIO_ROOT_PASSWORD")
    if root_user != RESTORE_GREEN_MINIO_ROOT_USER:
        errors.append(
            "minio-restore-green MINIO_ROOT_USER must map narrow app identity "
            f"({RESTORE_GREEN_MINIO_ROOT_USER!r}, got {root_user!r})"
        )
    if root_password != RESTORE_GREEN_MINIO_ROOT_PASSWORD:
        errors.append(
            "minio-restore-green MINIO_ROOT_PASSWORD must map narrow app secret "
            f"({RESTORE_GREEN_MINIO_ROOT_PASSWORD!r}, got {root_password!r})"
        )
    if service_block_references_minio_credential_key(block, "MARKHAND_MINIO_ROOT_USER"):
        errors.append("minio-restore-green must not use MARKHAND_MINIO_ROOT_USER")
    if service_block_references_minio_credential_key(block, "MARKHAND_MINIO_ROOT_PASSWORD"):
        errors.append("minio-restore-green must not use MARKHAND_MINIO_ROOT_PASSWORD")
    return errors


def insert_compose_extension_root_credential_anchor(compose_text: str) -> str:
    extension = (
        "x-archive-root-env: &archive-root-env\n"
        "  MARKHAND_MINIO_ROOT_USER: ${MARKHAND_MINIO_ROOT_USER:-markhand_root}\n"
        "  MARKHAND_MINIO_ROOT_PASSWORD: ${MARKHAND_MINIO_ROOT_PASSWORD:-markhand_root_poc_change_me}\n"
        "\n"
    )
    match = re.search(r"^services:\n", compose_text, flags=re.MULTILINE)
    if match is None:
        return compose_text
    return compose_text[: match.start()] + extension + compose_text[match.start() :]


def insert_service_with_environment_anchor(
    compose_text: str,
    *,
    service: str = "archive-replay",
    anchor: str = "archive-root-env",
) -> str:
    insertion = [
        f"  {service}:",
        "    image: alpine:latest",
        "    networks: [private]",
        f"    environment: *{anchor}",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def insert_service_scoped_root_credential_anchor(compose_text: str) -> str:
    insertion = [
        "  credential-donor:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment: &service-root-env",
        "      MARKHAND_MINIO_ROOT_USER: ${MARKHAND_MINIO_ROOT_USER:-markhand_root}",
        "      MARKHAND_MINIO_ROOT_PASSWORD: ${MARKHAND_MINIO_ROOT_PASSWORD:-markhand_root_poc_change_me}",
        "  credential-consumer:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment: *service-root-env",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def insert_service_scoped_root_credential_anchor_with_inline_comment(
    compose_text: str,
) -> str:
    insertion = [
        "  credential-donor:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment: &service-root-env # inline anchor hides root credential block",
        "      MARKHAND_MINIO_ROOT_USER: ${MARKHAND_MINIO_ROOT_USER:-markhand_root}",
        "      MARKHAND_MINIO_ROOT_PASSWORD: ${MARKHAND_MINIO_ROOT_PASSWORD:-markhand_root_poc_change_me}",
        "  credential-consumer:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment: *service-root-env",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def mutate_minio_restore_green_root_user(compose_text: str, *, replacement: str) -> str:
    blocks = parse_compose_service_blocks(compose_text)
    block = blocks.get("minio-restore-green")
    if block is None:
        return compose_text
    env = parse_service_environment(block)
    old = env.get("MINIO_ROOT_USER")
    if old is None:
        return compose_text
    return replace_in_service_block(compose_text, "minio-restore-green", old, replacement)


def service_block_line_span(lines: list[str], service: str) -> tuple[int, int] | None:
    for index, line in enumerate(lines):
        match = CANONICAL_DIRECT_SERVICE_KEY.match(line)
        if match is None or match.group(1) != service:
            continue
        start = index + 1
        end = len(lines)
        for next_index in range(index + 1, len(lines)):
            if is_canonical_direct_service_key_line(lines[next_index]):
                end = next_index
                break
        return start, end
    return None


def mutate_service_block_lines(
    compose_text: str,
    service: str,
    mutator: Callable[[list[str]], list[str]],
) -> str:
    lines, _ = _services_section_lines(compose_text)
    span = service_block_line_span(lines, service)
    if span is None:
        return compose_text
    start, end = span
    new_lines = lines[:start] + mutator(lines[start:end]) + lines[end:]
    return reassemble_services_section(compose_text, new_lines)


def parse_service_environment(service_block: str) -> dict[str, str]:
    env: dict[str, str] = {}
    env_match = re.search(
        r"(?m)^    environment:\n((?:      .+(?:\n|$))+)",
        service_block,
    )
    if env_match is None:
        return env
    for line in env_match.group(1).splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        entry = re.sub(r"\s+#.*$", "", stripped)

        list_match = re.match(r"^-\s*(.+)$", entry)
        if list_match:
            item = list_match.group(1).strip().strip('"').strip("'")
            if "=" in item:
                key, _, value = item.partition("=")
                env[key.strip()] = value.strip()
                continue
            key_match = re.match(r"^([A-Z0-9_]+):\s*(.+)$", item)
            if key_match:
                env[key_match.group(1)] = key_match.group(2).strip()
            continue

        key_match = re.match(r"^([A-Z0-9_]+):\s*(.+)$", entry)
        if key_match is None:
            continue
        env[key_match.group(1)] = key_match.group(2).strip()
    return env


def _minio_credential_key_token_pattern(key: str) -> re.Pattern[str]:
    return re.compile(rf"(?<![A-Z0-9_]){re.escape(key)}(?![A-Z0-9_])")


def service_block_references_minio_credential_key(service_block: str, key: str) -> bool:
    return _minio_credential_key_token_pattern(key).search(service_block) is not None


def service_has_minio_credential_references(service_block: str) -> bool:
    return any(
        service_block_references_minio_credential_key(service_block, key)
        for key in MINIO_CREDENTIAL_REFERENCE_KEYS
    )


def service_block_references_minio_root_credentials(
    service_block: str,
    *,
    root_user: str,
    root_pass: str,
) -> bool:
    lowered = service_block.lower()
    if root_user.lower() in lowered or root_pass.lower() in lowered:
        return True
    if "minio_root" in lowered or "markhand_root" in lowered:
        return True
    return False


def poc_runtime_minio_credential_services(compose_text: str) -> frozenset[str]:
    services: set[str] = set()
    for name, block in parse_compose_service_blocks(compose_text).items():
        if name in BOOTSTRAP_MINIO_SERVICES:
            continue
        if service_has_minio_credential_references(block):
            services.add(name)
    return frozenset(services)


def poc_explicit_required_runtime_service_names(compose_text: str) -> frozenset[str]:
    required: set[str] = set()
    for name in parse_compose_service_blocks(compose_text):
        if name in EXPLICIT_RUNTIME_SERVICE_NAMES or any(
            name.startswith(prefix) for prefix in EXPLICIT_RUNTIME_SERVICE_PREFIXES
        ):
            required.add(name)
    return frozenset(required)


def poc_required_runtime_service_names(compose_text: str) -> frozenset[str]:
    return poc_explicit_required_runtime_service_names(compose_text)


def poc_runtime_minio_service_envs(compose_text: str) -> dict[str, dict[str, str]]:
    services: dict[str, dict[str, str]] = {}
    for name in poc_runtime_minio_credential_services(compose_text):
        block = parse_compose_service_blocks(compose_text)[name]
        services[name] = parse_service_environment(block)
    return services


def insert_nonstandard_runtime_minio_service(
    compose_text: str,
    *,
    access_default: str,
    secret_default: str,
) -> str:
    insertion = [
        "  archive-replay:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment:",
        f"      MARKHAND_MINIO_ACCESS_KEY: ${{MARKHAND_MINIO_ACCESS_KEY:-{access_default}}}",
        f"      MARKHAND_MINIO_SECRET_KEY: ${{MARKHAND_MINIO_SECRET_KEY:-{secret_default}}}",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def insert_list_form_runtime_minio_service(
    compose_text: str,
    *,
    access_default: str,
    secret_default: str,
    service: str = "archive-replay",
) -> str:
    insertion = [
        f"  {service}:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment:",
        f"      - MARKHAND_MINIO_ACCESS_KEY=${{MARKHAND_MINIO_ACCESS_KEY:-{access_default}}}",
        f"      - MARKHAND_MINIO_SECRET_KEY=${{MARKHAND_MINIO_SECRET_KEY:-{secret_default}}}",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def insert_root_variable_only_runtime_minio_service(
    compose_text: str,
    *,
    service: str = "archive-replay",
) -> str:
    insertion = [
        f"  {service}:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment:",
        "      MARKHAND_MINIO_ROOT_USER: ${MARKHAND_MINIO_ROOT_USER:-markhand_root}",
        "      MARKHAND_MINIO_ROOT_PASSWORD: ${MARKHAND_MINIO_ROOT_PASSWORD:-markhand_root_poc_change_me}",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def insert_quoted_root_variable_runtime_minio_service(
    compose_text: str,
    *,
    service: str = "archive-replay",
) -> str:
    insertion = [
        f"  {service}:",
        "    image: alpine:latest",
        "    networks: [private]",
        "    environment:",
        '      "MARKHAND_MINIO_ROOT_USER": ${MARKHAND_MINIO_ROOT_USER:-markhand_root}',
        '      "MARKHAND_MINIO_ROOT_PASSWORD": ${MARKHAND_MINIO_ROOT_PASSWORD:-markhand_root_poc_change_me}',
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def poc_runtime_minio_credential_errors(
    compose_text: str,
    example: dict[str, str],
) -> list[str]:
    errors: list[str] = []
    root_user = example.get("MARKHAND_MINIO_ROOT_USER")
    root_pass = example.get("MARKHAND_MINIO_ROOT_PASSWORD")
    app_key = example.get("MARKHAND_MINIO_ACCESS_KEY")
    app_secret = example.get("MARKHAND_MINIO_SECRET_KEY")
    if not root_user or not root_pass or not app_key or not app_secret:
        errors.append(
            "deploy/.env.example must define MARKHAND_MINIO_ROOT_* and MARKHAND_MINIO_ACCESS_KEY/SECRET_KEY"
        )
        return errors

    parsed = parse_compose_services(compose_text)
    errors.extend(parsed.errors)
    blocks = parsed.blocks
    runtime_services = poc_runtime_minio_credential_services(compose_text)
    explicit_required = poc_explicit_required_runtime_service_names(compose_text)

    missing_required = sorted(explicit_required - runtime_services)
    if missing_required:
        errors.append(
            "deploy/compose.poc.yml runtime services missing MinIO credential env: "
            + ", ".join(missing_required)
        )

    for service in sorted(runtime_services):
        if service not in blocks:
            errors.append(f"deploy/compose.poc.yml missing runtime service {service!r}")
            continue
        block = blocks[service]
        env = parse_service_environment(block)
        if (
            service_block_references_minio_credential_key(block, "MARKHAND_MINIO_ROOT_USER")
            or service_block_references_minio_credential_key(block, "MARKHAND_MINIO_ROOT_PASSWORD")
        ):
            errors.append(f"{service} must not define MARKHAND_MINIO_ROOT_* outside bootstrap services")
        if service_block_references_minio_root_credentials(
            block,
            root_user=root_user,
            root_pass=root_pass,
        ):
            errors.append(f"{service} must not reference MinIO root credentials")
        if "MARKHAND_MINIO_ACCESS_KEY" not in env:
            errors.append(f"{service} must define MARKHAND_MINIO_ACCESS_KEY")
            if service_block_references_minio_credential_key(block, "MARKHAND_MINIO_ACCESS_KEY"):
                errors.append(
                    f"{service} MARKHAND_MINIO_ACCESS_KEY must use canonical mapping syntax"
                )
        if "MARKHAND_MINIO_SECRET_KEY" not in env:
            errors.append(f"{service} must define MARKHAND_MINIO_SECRET_KEY")
            if service_block_references_minio_credential_key(block, "MARKHAND_MINIO_SECRET_KEY"):
                errors.append(
                    f"{service} MARKHAND_MINIO_SECRET_KEY must use canonical mapping syntax"
                )
        if "MARKHAND_MINIO_ACCESS_KEY" not in env or "MARKHAND_MINIO_SECRET_KEY" not in env:
            continue

        access = env["MARKHAND_MINIO_ACCESS_KEY"]
        secret = env["MARKHAND_MINIO_SECRET_KEY"]
        if access != NARROW_MINIO_ACCESS_DEFAULT:
            errors.append(
                f"{service} MARKHAND_MINIO_ACCESS_KEY must narrow-default to markhand_app "
                f"({NARROW_MINIO_ACCESS_DEFAULT!r}, got {access!r})"
            )
        if secret != NARROW_MINIO_SECRET_DEFAULT:
            errors.append(
                f"{service} MARKHAND_MINIO_SECRET_KEY must narrow-default to markhand_app secret "
                f"({NARROW_MINIO_SECRET_DEFAULT!r}, got {secret!r})"
            )
    return errors


def _normalize_string_list(
    value: object,
    field_name: str,
    errors: list[str],
    *,
    statement_index: int,
) -> list[str] | None:
    prefix = f"minio app policy Statement[{statement_index}] "
    if isinstance(value, str):
        errors.append(f"{prefix}{field_name} must be a list, not a string")
        return None
    if not isinstance(value, list):
        errors.append(f"{prefix}{field_name} must be a list")
        return None
    return [str(entry) for entry in value]


def minio_app_policy_errors(policy_text: str) -> list[str]:
    errors: list[str] = []
    if "__BUCKET__" not in policy_text:
        errors.append("minio app policy template must parameterize fixed bucket via __BUCKET__")
    try:
        policy = json.loads(policy_text)
    except json.JSONDecodeError as exc:
        return errors + [f"minio app policy template must be valid JSON: {exc}"]

    if not isinstance(policy, dict):
        return errors + ["minio app policy root must be a JSON object"]

    top_level_keys = set(policy.keys())
    if top_level_keys != POLICY_TOP_LEVEL_KEYS:
        extra = sorted(top_level_keys - POLICY_TOP_LEVEL_KEYS)
        missing = sorted(POLICY_TOP_LEVEL_KEYS - top_level_keys)
        if extra:
            errors.append(f"minio app policy has unknown top-level keys: {extra}")
        if missing:
            errors.append(f"minio app policy missing top-level keys: {missing}")

    if policy.get("Version") != CANONICAL_POLICY_VERSION:
        errors.append(
            f"minio app policy Version must be exactly {CANONICAL_POLICY_VERSION!r}"
        )

    statements = policy.get("Statement")
    if not isinstance(statements, list):
        return errors + ["minio app policy Statement must be a list"]
    if len(statements) != 2:
        errors.append("minio app policy must contain exactly two Statement entries")

    seen_sids: set[str] = set()
    for index, statement in enumerate(statements):
        if not isinstance(statement, dict):
            errors.append(f"minio app policy Statement[{index}] must be an object")
            continue

        statement_keys = set(statement.keys())
        if statement_keys != STATEMENT_KEYS:
            forbidden = sorted(statement_keys - STATEMENT_KEYS)
            missing = sorted(STATEMENT_KEYS - statement_keys)
            if forbidden:
                errors.append(
                    f"minio app policy Statement[{index}] has forbidden keys: {forbidden}"
                )
            if missing:
                errors.append(
                    f"minio app policy Statement[{index}] missing keys: {missing}"
                )

        sid = statement.get("Sid")
        if not isinstance(sid, str):
            errors.append(f"minio app policy Statement[{index}] Sid must be a string")
            continue
        if sid in seen_sids:
            errors.append(f"minio app policy duplicate Sid {sid!r}")
        seen_sids.add(sid)

        canonical = CANONICAL_STATEMENTS_BY_SID.get(sid)
        if canonical is None:
            errors.append(f"minio app policy Statement[{index}] has unknown Sid {sid!r}")
            continue
        if statement.get("Effect") != canonical["Effect"]:
            errors.append(f"minio app policy Statement[{index}] Effect must be Allow")

        actions = _normalize_string_list(
            statement.get("Action"),
            "Action",
            errors,
            statement_index=index,
        )
        resources = _normalize_string_list(
            statement.get("Resource"),
            "Resource",
            errors,
            statement_index=index,
        )
        if actions is not None:
            if actions != canonical["Action"]:
                errors.append(
                    f"minio app policy Statement[{index}] Action must be exactly "
                    f"{canonical['Action']!r}"
                )
            for action in actions:
                if "*" in action:
                    errors.append(
                        f"minio app policy Statement[{index}] Action must not contain wildcards"
                    )
        if resources is not None:
            if resources != canonical["Resource"]:
                errors.append(
                    f"minio app policy Statement[{index}] Resource must be exactly "
                    f"{canonical['Resource']!r}"
                )
            for resource in resources:
                if resource in {"*", "arn:aws:s3:::*"}:
                    errors.append("minio app policy must not use wildcard Resource *")

    missing_sids = set(CANONICAL_STATEMENTS_BY_SID) - seen_sids
    if missing_sids:
        errors.append(
            "minio app policy missing required Sid entries: "
            + ", ".join(sorted(missing_sids))
        )
    return errors


def remove_service_env_line(compose_text: str, service: str, env_key: str) -> str:
    prefix = f"      {env_key}: "

    def mutator(body_lines: list[str]) -> list[str]:
        return [line for line in body_lines if not line.startswith(prefix)]

    return mutate_service_block_lines(compose_text, service, mutator)


def mutate_canonical_service_key_line(
    compose_text: str,
    service: str,
    new_line: str,
) -> str:
    lines, _ = _services_section_lines(compose_text)
    for index, line in enumerate(lines):
        match = CANONICAL_DIRECT_SERVICE_KEY.match(line)
        if match and match.group(1) == service:
            lines[index] = new_line
            return reassemble_services_section(compose_text, lines)
    return compose_text


def quote_service_key_line(compose_text: str, service: str) -> str:
    return mutate_canonical_service_key_line(compose_text, service, f"  '{service}':")


def insert_unrelated_quoted_service(compose_text: str) -> str:
    insertion = [
        "  'telemetry-sidecar': # sidecar",
        "    image: alpine:latest",
        "    networks: [private]",
    ]
    lines, _ = _services_section_lines(compose_text)
    return reassemble_services_section(compose_text, lines + insertion)


def replace_in_service_block(
    compose_text: str,
    service: str,
    old: str,
    new: str,
) -> str:
    def mutator(body_lines: list[str]) -> list[str]:
        updated: list[str] = []
        replaced = False
        for line in body_lines:
            if not replaced and old in line:
                updated.append(line.replace(old, new, 1))
                replaced = True
            else:
                updated.append(line)
        return updated

    return mutate_service_block_lines(compose_text, service, mutator)


def swap_runtime_minio_defaults_in_compose(
    compose_text: str,
    *,
    access_default: str,
    secret_default: str,
    only_service: str | None = None,
) -> str:
    result = compose_text
    for service in sorted(poc_runtime_minio_credential_services(compose_text)):
        if only_service is not None and service != only_service:
            continue
        block = parse_compose_service_blocks(compose_text)[service]
        env = parse_service_environment(block)
        if "MARKHAND_MINIO_ACCESS_KEY" not in env or "MARKHAND_MINIO_SECRET_KEY" not in env:
            continue
        old_access = env["MARKHAND_MINIO_ACCESS_KEY"]
        old_secret = env["MARKHAND_MINIO_SECRET_KEY"]
        new_access = f"${{MARKHAND_MINIO_ACCESS_KEY:-{access_default}}}"
        new_secret = f"${{MARKHAND_MINIO_SECRET_KEY:-{secret_default}}}"
        result = replace_in_service_block(result, service, old_access, new_access)
        result = replace_in_service_block(result, service, old_secret, new_secret)
    return result


def minio_fixture_boundary_errors(
    job_block: str,
    compose_text: str | None = None,
    policy_text: str | None = None,
) -> list[str]:
    """Deployed test harness uses root for ephemeral buckets; app stack stays narrow."""
    errors: list[str] = []
    job_env = parse_job_env(job_block)
    example = parse_env_example(ENV_EXAMPLE)
    root_user = example.get("MARKHAND_MINIO_ROOT_USER")
    root_pass = example.get("MARKHAND_MINIO_ROOT_PASSWORD")
    app_key = example.get("MARKHAND_MINIO_ACCESS_KEY")
    if not root_user or not root_pass or not app_key:
        errors.append(
            "deploy/.env.example must define MARKHAND_MINIO_ROOT_* and MARKHAND_MINIO_ACCESS_KEY"
        )
        return errors

    test_key = job_env.get("MARKHAND_TEST_MINIO_ACCESS_KEY")
    test_secret = job_env.get("MARKHAND_TEST_MINIO_SECRET_KEY")
    if test_key != root_user:
        errors.append(
            "MARKHAND_TEST_MINIO_ACCESS_KEY must use POC root fixture identity "
            f"({root_user!r}) for ephemeral markhand-it-* bucket lifecycle"
        )
    if test_secret != root_pass:
        errors.append(
            "MARKHAND_TEST_MINIO_SECRET_KEY must use POC root fixture password "
            "for ephemeral markhand-it-* bucket lifecycle"
        )
    if test_key == app_key:
        errors.append(
            "MARKHAND_TEST_MINIO_ACCESS_KEY must differ from narrow application "
            f"identity ({app_key!r})"
        )

    compose_body = compose_text if compose_text is not None else COMPOSE_POC.read_text(encoding="utf-8")
    policy_body = (
        policy_text if policy_text is not None else MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
    )
    errors.extend(poc_runtime_minio_credential_errors(compose_body, example))
    errors.extend(minio_app_policy_errors(policy_body))
    return errors


def deployed_job_contract_errors(job_block: str) -> list[str]:
    errors: list[str] = []
    steps = parse_job_steps(job_block)
    names = [step.name for step in steps]
    env = parse_job_env(job_block)

    if env.get("MARKHAND_TEST_REQUIRED") != "1":
        errors.append('job env must set MARKHAND_TEST_REQUIRED: "1"')

    if re.search(r"cargo test -p fileconv-server --test '\*'", job_block):
        errors.append("must not run wildcard cargo test --test '*'")

    if "test-output.log" in job_block:
        errors.append("must not upload raw cargo test-output.log")

    if "PHASE1C_MANIFEST_SHA256" not in job_block:
        errors.append("must resolve PHASE1C_MANIFEST_SHA256 before render")

    try:
        ordered = [step_index(steps, name) for name in REQUIRED_STEP_ORDER]
    except ValueError as exc:
        errors.append(str(exc))
        return errors

    if ordered != sorted(ordered):
        errors.append(
            "required steps must appear in order: "
            + " -> ".join(REQUIRED_STEP_ORDER)
        )

    if names[-1] != "Enforce 1C-12 deployed gate":
        errors.append("Enforce 1C-12 deployed gate must be the final step")

    runner = steps[step_index(steps, "Run Phase 1C denial suite (deployed POC stack)")]
    runner_script = strip_shell_comments(runner.run or "")
    if runner.step_id != "phase1c_denial_suite":
        errors.append("runner step must have id phase1c_denial_suite")
    if not runner_script:
        errors.append("runner step must have a run block")
    else:
        if not re.search(
            r"(?m)^[ \t]*python3 scripts/run-phase1c-denial-suite\.py \\",
            runner_script,
        ):
            errors.append("runner step must invoke canonical denial suite command")
        for token in (
            "--manifest crates/server/tests/fixtures/multi-org-denial.manifest.json",
            '--output "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json"',
        ):
            if token not in runner_script:
                errors.append(f"runner step must include {token!r}")
        if "runner_exit_code=" not in runner_script:
            errors.append("runner step must capture runner_exit_code output")

    teardown = steps[step_index(steps, "Tear down POC stack")]
    teardown_script = strip_shell_comments(teardown.run or "")
    if teardown.if_condition != "always()":
        errors.append("teardown step must use if: always()")
    if teardown.step_id != "phase1c_teardown":
        errors.append("teardown step must have id phase1c_teardown")
    if "docker compose -f deploy/compose.poc.yml down -v" not in teardown_script:
        errors.append("teardown step must run docker compose down -v")
    if "teardown_exit_code=" not in teardown_script:
        errors.append("teardown step must capture teardown_exit_code output")

    render = steps[step_index(steps, "Render Phase 1C denial report")]
    render_script = strip_shell_comments(render.run or "")
    if render.if_condition != "always()":
        errors.append("render step must use if: always()")
    if render.step_id != "phase1c_render":
        errors.append("render step must have id phase1c_render")
    if "python3 scripts/render-phase1c-denial-report.py" not in render_script:
        errors.append("render step must invoke render-phase1c-denial-report.py")
    if not args_in_order(render_script, RENDER_ARG_ORDER):
        errors.append(
            "render step must pass renderer args in order: "
            + ", ".join(RENDER_ARG_ORDER)
        )
    render_env = render.env or {}
    if render_env.get("PHASE1C_SOURCE_REF") != PHASE1C_SOURCE_REF_ENV:
        errors.append(
            "render step env must set PHASE1C_SOURCE_REF from github.head_ref || github.ref_name"
        )
    if "--expected-git-ref" not in render_script:
        errors.append("render step must pass trusted expected git ref")
    if '--expected-git-ref "$PHASE1C_SOURCE_REF"' not in render_script:
        errors.append(
            'render step must pass --expected-git-ref "$PHASE1C_SOURCE_REF" (quoted env transport)'
        )
    for forbidden in (
        "${{ github.ref_name }}",
        "${{ github.head_ref",
        "github.ref_name",
        "github.head_ref",
    ):
        if forbidden in render_script:
            errors.append(
                f"render step run block must not interpolate {forbidden!r} directly in shell"
            )
    if "--input-failure-category" not in render_script:
        errors.append("render step must pass --input-failure-category")
    if "|| true" in render_script:
        errors.append("render step must not hide failures with || true")
    if "render_exit_code=" not in render_script:
        errors.append("render step must capture render_exit_code output")

    upload = steps[step_index(steps, "Upload 1C integration report")]
    if upload.if_condition != "always()":
        errors.append("upload step must use if: always()")
    if upload.uses is None or "actions/upload-artifact" not in upload.uses:
        errors.append("upload step must use actions/upload-artifact")
    upload_with = upload.with_block or ""
    if "if-no-files-found: error" not in upload_with:
        errors.append("upload step must set if-no-files-found: error")
    upload_paths = parse_upload_paths(upload_with)
    if upload_paths != list(ARTIFACT_PATHS):
        errors.append(
            "upload step must upload exactly the two canonical artifact paths"
        )

    enforce = steps[step_index(steps, "Enforce 1C-12 deployed gate")]
    enforce_script = strip_shell_comments(enforce.run or "")
    if enforce.if_condition != "always()":
        errors.append("enforce step must use if: always()")
    if not enforce_script:
        errors.append("enforce step must have a run block")
    else:
        for token in (
            "runner_exit_code",
            "teardown_exit_code",
            "render_exit_code",
            "manifest-run.json",
            "phase1c-denial-report.md",
        ):
            if token not in enforce_script:
                errors.append(f"enforce step must check {token}")

    render_index = step_index(steps, "Render Phase 1C denial report")
    teardown_index = step_index(steps, "Tear down POC stack")
    if render_index <= teardown_index:
        errors.append("render must occur after teardown")

    errors.extend(minio_fixture_boundary_errors(job_block))

    return errors


class Deployed1cWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow_text = CI_WORKFLOW.read_text(encoding="utf-8")
        cls.job_block = extract_job_block(cls.workflow_text, JOB_NAME)

    def test_deployed_job_structural_contract(self) -> None:
        errors = deployed_job_contract_errors(self.job_block)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_commenting_required_env_fails_contract(self) -> None:
        mutated = self.job_block.replace(
            'MARKHAND_TEST_REQUIRED: "1"',
            '# MARKHAND_TEST_REQUIRED: "1"',
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any('job env must set MARKHAND_TEST_REQUIRED: "1"' in error for error in errors),
            errors,
        )

    def test_removing_runner_exit_code_arg_fails_contract(self) -> None:
        mutated = self.job_block.replace(
            '            --runner-exit-code "$runner_exit" \\\n',
            "",
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any("render step must pass renderer args in order" in error for error in errors),
            errors,
        )

    def test_third_artifact_path_fails_contract(self) -> None:
        mutated = self.job_block.replace(
            "            ${{ runner.temp }}/markhand-1c-integration/phase1c-denial-report.md",
            "            ${{ runner.temp }}/markhand-1c-integration/phase1c-denial-report.md\n"
            "            ${{ runner.temp }}/markhand-1c-integration/extra.log",
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any("exactly the two canonical artifact paths" in error for error in errors),
            errors,
        )

    def test_comment_mention_cannot_satisfy_runner_contract(self) -> None:
        mutated = self.job_block.replace(
            "python3 scripts/run-phase1c-denial-suite.py",
            "echo runner skipped # python3 scripts/run-phase1c-denial-suite.py",
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any("canonical denial suite command" in error for error in errors),
            errors,
        )

    def test_direct_github_ref_interpolation_fails_contract(self) -> None:
        mutated = self.job_block.replace(
            '--expected-git-ref "$PHASE1C_SOURCE_REF" \\\n',
            '--expected-git-ref "${{ github.ref_name }}" \\\n',
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any("must not interpolate" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any('quoted env transport' in error for error in errors),
            errors,
        )

    def test_narrow_app_minio_for_test_harness_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        root_user = example["MARKHAND_MINIO_ROOT_USER"]
        app_key = example["MARKHAND_MINIO_ACCESS_KEY"]
        mutated = self.job_block.replace(
            f"MARKHAND_TEST_MINIO_ACCESS_KEY: {root_user}",
            f"MARKHAND_TEST_MINIO_ACCESS_KEY: {app_key}",
            1,
        )
        errors = minio_fixture_boundary_errors(mutated)
        self.assertTrue(
            any("must use POC root fixture identity" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("must differ from narrow application" in error for error in errors),
            errors,
        )

    def test_deployed_test_minio_uses_root_fixture_while_compose_app_stays_narrow(self) -> None:
        errors = minio_fixture_boundary_errors(self.job_block)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_migrate_service_must_use_narrow_minio_defaults(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        self.assertIn("migrate", poc_runtime_minio_credential_services(compose_text))
        errors = poc_runtime_minio_credential_errors(compose_text, example)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_swap_migrate_minio_defaults_to_root_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = swap_runtime_minio_defaults_in_compose(
            compose_text,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="migrate",
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("migrate MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )

    def test_nonstandard_runtime_service_with_root_minio_credentials_fails_contract(
        self,
    ) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_nonstandard_runtime_minio_service(
            compose_text,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("archive-replay MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )

    def test_list_form_runtime_minio_service_with_root_credentials_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_list_form_runtime_minio_service(
            compose_text,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("archive-replay MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("archive-replay must not reference MinIO root credentials" in error for error in errors),
            errors,
        )

    def test_root_variable_only_runtime_minio_service_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_root_variable_only_runtime_minio_service(compose_text)
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("archive-replay must define MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("archive-replay must not define MARKHAND_MINIO_ROOT_*" in error for error in errors),
            errors,
        )

    def test_quoted_root_environment_key_on_nonstandard_service_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_quoted_root_variable_runtime_minio_service(compose_text)
        runtime = poc_runtime_minio_credential_services(mutated)
        self.assertIn("archive-replay", runtime)
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("archive-replay must not define MARKHAND_MINIO_ROOT_*" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("archive-replay must define MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("archive-replay must define MARKHAND_MINIO_SECRET_KEY" in error for error in errors),
            errors,
        )

    def test_compose_extension_blocks_must_not_carry_minio_credentials(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        errors = compose_extension_and_anchor_minio_errors(compose_text)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_adversarial_root_credential_anchor_and_environment_alias_fails_contract(
        self,
    ) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_compose_extension_root_credential_anchor(compose_text)
        mutated = insert_service_with_environment_anchor(mutated)
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any("extension 'x-archive-root-env' must not define MinIO credential references" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("YAML anchor &archive-root-env must not define MinIO credential references" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("must not alias MinIO credential anchor *archive-root-env" in error for error in errors),
            errors,
        )

    def test_service_scoped_root_credential_anchor_and_alias_fails_contract(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_service_scoped_root_credential_anchor(compose_text)
        errors = compose_extension_and_anchor_minio_errors(mutated)
        self.assertTrue(
            any("YAML anchor &service-root-env must not define MinIO credential references" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("must not alias MinIO credential anchor *service-root-env" in error for error in errors),
            errors,
        )

    def test_service_scoped_inline_comment_anchor_and_alias_fails_contract(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_service_scoped_root_credential_anchor_with_inline_comment(compose_text)
        errors = compose_extension_and_anchor_minio_errors(mutated)
        self.assertTrue(
            any("YAML anchor &service-root-env must not define MinIO credential references" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("must not alias MinIO credential anchor *service-root-env" in error for error in errors),
            errors,
        )

    def test_minio_restore_green_requires_narrow_app_root_mapping(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        errors = minio_restore_green_invariant_errors(compose_text)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_minio_restore_green_primary_root_user_mutation_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = mutate_minio_restore_green_root_user(
            compose_text,
            replacement="${MARKHAND_MINIO_ROOT_USER:-markhand_root}",
        )
        errors = minio_restore_green_invariant_errors(mutated)
        self.assertTrue(
            any("minio-restore-green MINIO_ROOT_USER must map narrow app identity" in error for error in errors),
            errors,
        )
        self.assertTrue(
            any("minio-restore-green must not use MARKHAND_MINIO_ROOT_USER" in error for error in errors),
            errors,
        )
        boundary_errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any("minio-restore-green MINIO_ROOT_USER must map narrow app identity" in error for error in boundary_errors),
            boundary_errors,
        )

    def test_bootstrap_minio_services_remain_outside_runtime_contract(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        runtime = poc_runtime_minio_credential_services(compose_text)
        self.assertNotIn("minio", runtime)
        self.assertNotIn("minio-init", runtime)
        self.assertNotIn("minio-restore-green", runtime)

    def test_all_runtime_services_use_narrow_minio_defaults(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        required = poc_runtime_minio_credential_services(compose_text)
        self.assertIn("migrate", required)
        self.assertIn("api", required)
        self.assertIn("api-restore-green", required)
        self.assertIn("worker-convert", required)
        errors = poc_runtime_minio_credential_errors(compose_text, example)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_minio_app_policy_matches_exact_canonical_allowlist(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        errors = minio_app_policy_errors(policy_text)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_swap_all_runtime_minio_defaults_to_root_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        required_count = len(poc_runtime_minio_credential_services(compose_text))
        mutated = swap_runtime_minio_defaults_in_compose(
            compose_text,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("must narrow-default to markhand_app" in error for error in errors),
            errors,
        )
        self.assertGreaterEqual(len(errors), required_count)

    def test_swap_one_worker_minio_defaults_to_root_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = swap_runtime_minio_defaults_in_compose(
            compose_text,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="worker-convert",
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("worker-convert MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )

    def test_remove_api_restore_green_access_credential_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = remove_service_env_line(
            compose_text,
            "api-restore-green",
            "MARKHAND_MINIO_ACCESS_KEY",
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("api-restore-green must define MARKHAND_MINIO_ACCESS_KEY" in error for error in errors),
            errors,
        )

    def test_remove_worker_secret_credential_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = remove_service_env_line(
            compose_text,
            "worker-index",
            "MARKHAND_MINIO_SECRET_KEY",
        )
        errors = poc_runtime_minio_credential_errors(mutated, example)
        self.assertTrue(
            any("worker-index must define MARKHAND_MINIO_SECRET_KEY" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_extra_bucket_resource_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"arn:aws:s3:::__BUCKET__"',
            '"arn:aws:s3:::__BUCKET__",\n        "arn:aws:s3:::other-bucket"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("Resource must be exactly" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_create_action_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"s3:ListBucket"',
            '"s3:ListBucket",\n        "s3:CreateBucket"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("Action must be exactly" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_put_bucket_cors_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"s3:DeleteObject"',
            '"s3:DeleteObject",\n        "s3:PutBucketCors"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("Action must be exactly" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_extra_statement_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        policy = json.loads(policy_text)
        policy["Statement"].append(
            {
                "Sid": "ExtraStatement",
                "Effect": "Allow",
                "Action": ["s3:GetObject"],
                "Resource": ["arn:aws:s3:::__BUCKET__/trusted/*"],
            }
        )
        mutated = json.dumps(policy, indent=2) + "\n"
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("exactly two Statement entries" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_not_action_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"Action": [',
            '"NotAction": [',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("forbidden keys" in error and "NotAction" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_not_resource_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"Resource": [',
            '"NotResource": [',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("forbidden keys" in error and "NotResource" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_wildcard_action_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"s3:GetObject"',
            '"s3:*"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("Action must not contain wildcards" in error for error in errors)
            or any("Action must be exactly" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_wildcard_resource_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"arn:aws:s3:::__BUCKET__/trusted/*"',
            '"*"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("wildcard Resource *" in error for error in errors)
            or any("Resource must be exactly" in error for error in errors),
            errors,
        )

    def test_quoted_worker_delete_with_root_credentials_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = quote_service_key_line(compose_text, "worker-delete")
        mutated = swap_runtime_minio_defaults_in_compose(
            mutated,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="worker-delete",
        )
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )

    def test_quoted_api_restore_green_with_root_credentials_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = quote_service_key_line(compose_text, "api-restore-green")
        mutated = swap_runtime_minio_defaults_in_compose(
            mutated,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="api-restore-green",
        )
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )

    def test_unrelated_quoted_service_key_fails_parser_contract(self) -> None:
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_unrelated_quoted_service(compose_text)
        errors = compose_services_contract_errors(mutated)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )
        self.assertIn("telemetry-sidecar", mutated)

    def test_worker_embedding_inline_comment_with_root_credentials_fails_contract(
        self,
    ) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = mutate_canonical_service_key_line(
            compose_text,
            "worker-embedding",
            "  worker-embedding: # comment",
        )
        mutated = swap_runtime_minio_defaults_in_compose(
            mutated,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="worker-embedding",
        )
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )

    def test_api_restore_green_anchor_with_root_credentials_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = mutate_canonical_service_key_line(
            compose_text,
            "api-restore-green",
            "  api-restore-green: &restore_api",
        )
        mutated = swap_runtime_minio_defaults_in_compose(
            mutated,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="api-restore-green",
        )
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )

    def test_quoted_sidecar_with_comment_and_root_credentials_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        mutated = insert_unrelated_quoted_service(compose_text)
        mutated = swap_runtime_minio_defaults_in_compose(
            mutated,
            access_default=example["MARKHAND_MINIO_ROOT_USER"],
            secret_default=example["MARKHAND_MINIO_ROOT_PASSWORD"],
            only_service="worker-convert",
        )
        errors = compose_minio_contract_errors(mutated, example)
        self.assertTrue(
            any(CANONICAL_SERVICE_KEY_ERROR in error for error in errors),
            errors,
        )


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Deployed1cWorkflowContractTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if not args.self_test:
        parser.error("--self-test is required")
    return run_self_tests()


if __name__ == "__main__":
    raise SystemExit(main())
