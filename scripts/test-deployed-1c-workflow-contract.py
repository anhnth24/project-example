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
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
ENV_EXAMPLE = REPO_ROOT / "deploy/.env.example"
COMPOSE_POC = REPO_ROOT / "deploy/compose.poc.yml"
MINIO_POLICY_TEMPLATE = REPO_ROOT / "deploy/poc/minio-app-policy.json.tmpl"
JOB_NAME = "deployed-1c-integration"

NARROW_MINIO_ACCESS_DEFAULT = "${MARKHAND_MINIO_ACCESS_KEY:-markhand_app}"
NARROW_MINIO_SECRET_DEFAULT = "${MARKHAND_MINIO_SECRET_KEY:-markhand_app_poc_change_me}"

BOOTSTRAP_MINIO_SERVICES = frozenset({"minio-init"})

EXPECTED_POC_RUNTIME_MINIO_SERVICES = frozenset(
    {
        "migrate",
        "api",
        "worker-convert",
        "worker-index",
        "worker-embedding",
        "worker-delete",
        "worker-reconcile",
        "worker-reconcile-oneshot",
    }
)

BUCKET_MANAGEMENT_ACTIONS = frozenset(
    {
        "s3:*",
        "s3:CreateBucket",
        "s3:DeleteBucket",
        "s3:ListAllMyBuckets",
        "s3:PutBucketPolicy",
        "s3:DeleteBucketPolicy",
        "s3:GetBucketPolicy",
        "s3:PutBucketAcl",
        "s3:GetBucketAcl",
        "s3:PutBucketVersioning",
        "s3:PutLifecycleConfiguration",
    }
)

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


def parse_compose_service_blocks(compose_text: str) -> dict[str, str]:
    match = re.search(r"^services:\n", compose_text, flags=re.MULTILINE)
    if match is None:
        return {}
    rest = compose_text[match.end() :]
    stop = re.search(r"^(?:networks|volumes|secrets|configs):", rest, flags=re.MULTILINE)
    if stop:
        rest = rest[: stop.start()]
    blocks: dict[str, str] = {}
    parts = re.split(r"(?m)^  ([a-z0-9_-]+):\s*$", rest)
    iterator = iter(parts[1:])
    for name in iterator:
        body = next(iterator, "")
        blocks[name] = body
    return blocks


def parse_service_environment(service_block: str) -> dict[str, str]:
    env: dict[str, str] = {}
    env_match = re.search(
        r"(?m)^    environment:\n((?:      .+\n)*)",
        service_block,
    )
    if env_match is None:
        return env
    for line in env_match.group(1).splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        entry = re.sub(r"\s+#.*$", "", stripped)
        key_match = re.match(r"^([A-Z0-9_]+):\s*(.+)$", entry)
        if key_match is None:
            continue
        env[key_match.group(1)] = key_match.group(2).strip()
    return env


def poc_runtime_minio_service_envs(compose_text: str) -> dict[str, dict[str, str]]:
    services: dict[str, dict[str, str]] = {}
    for name, block in parse_compose_service_blocks(compose_text).items():
        if name in BOOTSTRAP_MINIO_SERVICES:
            continue
        env = parse_service_environment(block)
        if "MARKHAND_MINIO_ACCESS_KEY" in env and "MARKHAND_MINIO_SECRET_KEY" in env:
            services[name] = env
    return services


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

    runtime_services = poc_runtime_minio_service_envs(compose_text)
    if not EXPECTED_POC_RUNTIME_MINIO_SERVICES.issubset(runtime_services):
        missing = sorted(EXPECTED_POC_RUNTIME_MINIO_SERVICES - set(runtime_services))
        errors.append(
            "deploy/compose.poc.yml missing MinIO runtime services: "
            + ", ".join(missing)
        )

    for service, env in sorted(runtime_services.items()):
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
        for label, value in (("access", access), ("secret", secret)):
            lowered = value.lower()
            if root_user.lower() in lowered or "minio_root" in lowered:
                errors.append(
                    f"{service} must not reference MinIO root credentials in {label} env"
                )
            if "markhand_root" in lowered:
                errors.append(
                    f"{service} must not reference markhand_root in {label} env"
                )
    return errors


def minio_app_policy_errors(policy_text: str) -> list[str]:
    errors: list[str] = []
    if "__BUCKET__" not in policy_text:
        errors.append("minio app policy template must parameterize fixed bucket via __BUCKET__")
    try:
        rendered = policy_text.replace("__BUCKET__", "markhand-documents")
        policy = json.loads(rendered)
    except json.JSONDecodeError as exc:
        return errors + [f"minio app policy template must be valid JSON: {exc}"]

    statements = policy.get("Statement")
    if not isinstance(statements, list) or not statements:
        return errors + ["minio app policy must contain non-empty Statement list"]

    all_resources: list[str] = []
    all_actions: list[str] = []
    for index, statement in enumerate(statements):
        if not isinstance(statement, dict):
            errors.append(f"minio app policy Statement[{index}] must be an object")
            continue
        resources = statement.get("Resource", [])
        if isinstance(resources, str):
            resources = [resources]
        actions = statement.get("Action", [])
        if isinstance(actions, str):
            actions = [actions]
        all_resources.extend(str(resource) for resource in resources)
        all_actions.extend(str(action) for action in actions)
        for resource in resources:
            resource_text = str(resource)
            if resource_text in {"*", "arn:aws:s3:::*"}:
                errors.append("minio app policy must not use wildcard Resource *")
            if resource_text.endswith("/*") and (
                "/quarantine/" not in resource_text and "/trusted/" not in resource_text
            ):
                errors.append(
                    "minio app policy object resources must stay under quarantine/* or trusted/*"
                )
        for action in actions:
            if action in BUCKET_MANAGEMENT_ACTIONS:
                errors.append(
                    f"minio app policy must not include bucket-management action {action!r}"
                )

    resource_blob = " ".join(all_resources)
    if "quarantine" not in resource_blob or "trusted" not in resource_blob:
        errors.append(
            "minio app policy must scope objects to quarantine/* and trusted/* prefixes"
        )
    if "arn:aws:s3:::markhand-documents" not in resource_blob:
        errors.append("minio app policy must include fixed-bucket ARN boundary")
    if "arn:aws:s3:::markhand-documents/quarantine/*" not in resource_blob:
        errors.append("minio app policy must include quarantine object prefix")
    if "arn:aws:s3:::markhand-documents/trusted/*" not in resource_blob:
        errors.append("minio app policy must include trusted object prefix")
    return errors


def replace_in_service_block(
    compose_text: str,
    service: str,
    old: str,
    new: str,
) -> str:
    pattern = rf"(?ms)(^  {re.escape(service)}:\n)(.*?)(?=^  [a-z0-9_-]+:\s*$|^networks:|^volumes:|\Z)"
    match = re.search(pattern, compose_text)
    if match is None or old not in match.group(2):
        return compose_text
    header = match.group(1)
    body = match.group(2).replace(old, new, 1)
    return compose_text[: match.start()] + header + body + compose_text[match.end() :]


def swap_runtime_minio_defaults_in_compose(
    compose_text: str,
    *,
    access_default: str,
    secret_default: str,
    only_service: str | None = None,
) -> str:
    result = compose_text
    for service, block in parse_compose_service_blocks(compose_text).items():
        if service in BOOTSTRAP_MINIO_SERVICES:
            continue
        env = parse_service_environment(block)
        if "MARKHAND_MINIO_ACCESS_KEY" not in env:
            continue
        if only_service is not None and service != only_service:
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

    def test_all_runtime_services_use_narrow_minio_defaults(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
        runtime = poc_runtime_minio_service_envs(compose_text)
        self.assertTrue(EXPECTED_POC_RUNTIME_MINIO_SERVICES.issubset(runtime))
        errors = poc_runtime_minio_credential_errors(compose_text, example)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_minio_app_policy_stays_bucket_object_scoped(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        errors = minio_app_policy_errors(policy_text)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_swap_all_runtime_minio_defaults_to_root_fails_contract(self) -> None:
        example = parse_env_example(ENV_EXAMPLE)
        compose_text = COMPOSE_POC.read_text(encoding="utf-8")
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
        self.assertGreaterEqual(len(errors), len(EXPECTED_POC_RUNTIME_MINIO_SERVICES))

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

    def test_widened_minio_policy_wildcard_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"arn:aws:s3:::__BUCKET__/trusted/*"',
            '"arn:aws:s3:::__BUCKET__/trusted/*",\n        "*"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("wildcard Resource *" in error for error in errors),
            errors,
        )

    def test_widened_minio_policy_bucket_management_fails_contract(self) -> None:
        policy_text = MINIO_POLICY_TEMPLATE.read_text(encoding="utf-8")
        mutated = policy_text.replace(
            '"s3:ListBucket"',
            '"s3:ListBucket",\n        "s3:CreateBucket"',
            1,
        )
        errors = minio_app_policy_errors(mutated)
        self.assertTrue(
            any("bucket-management action 's3:CreateBucket'" in error for error in errors),
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
