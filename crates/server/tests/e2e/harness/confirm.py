"""Fail-closed safety gates for live E2E (never mutate human environments)."""

from __future__ import annotations

import os
import re
from dataclasses import dataclass

CONFIRM_ENV = "MARKHAND_E2E_CONFIRM"
STACK_TAG_ENV = "MARKHAND_E2E_STACK_TAG"
DEFAULT_CONFIRM = "i-understand-this-mutates-only-tagged-test-stacks"
TESTISH = re.compile(r"(?:^|[-_])(?:e2e|test)(?:[-_]|$)", re.IGNORECASE)


@dataclass(frozen=True)
class LiveGateResult:
    ok: bool
    errors: tuple[str, ...]


def _looks_testish(value: str | None) -> bool:
    if not value:
        return False
    return bool(TESTISH.search(value))


def validate_live_gates(
    *,
    confirm_phrase: str = DEFAULT_CONFIRM,
    compose_project: str | None = None,
    postgres_db: str | None = None,
    minio_bucket: str | None = None,
    stack_tag: str | None = None,
    environ: dict[str, str] | None = None,
) -> LiveGateResult:
    env = environ if environ is not None else dict(os.environ)
    errors: list[str] = []

    confirm = env.get(CONFIRM_ENV, "")
    if confirm != confirm_phrase:
        errors.append(
            f"{CONFIRM_ENV} must equal the exact phrase "
            f"(got {'set' if confirm else 'unset'}; refusing live mutation)"
        )

    project = compose_project if compose_project is not None else env.get("MARKHAND_COMPOSE_PROJECT")
    if not _looks_testish(project):
        errors.append(
            "MARKHAND_COMPOSE_PROJECT must contain an 'e2e' or 'test' name segment "
            f"(got {project!r})"
        )
    if project in {"markhand", "markhand-poc"}:
        errors.append(
            f"MARKHAND_COMPOSE_PROJECT refuses untagged/human stack {project!r}"
        )

    db = postgres_db if postgres_db is not None else env.get("MARKHAND_POSTGRES_DB")
    if not _looks_testish(db):
        errors.append(
            "MARKHAND_POSTGRES_DB must contain an 'e2e' or 'test' name segment "
            f"(got {db!r})"
        )
    if db == "markhand":
        errors.append("MARKHAND_POSTGRES_DB refuses human db 'markhand'")

    bucket = minio_bucket if minio_bucket is not None else env.get("MARKHAND_MINIO_BUCKET")
    if not _looks_testish(bucket):
        errors.append(
            "MARKHAND_MINIO_BUCKET must contain an 'e2e' or 'test' name segment "
            f"(got {bucket!r})"
        )
    if bucket == "markhand-documents":
        errors.append("MARKHAND_MINIO_BUCKET refuses human bucket 'markhand-documents'")

    tag = stack_tag if stack_tag is not None else env.get(STACK_TAG_ENV)
    if tag != "test":
        errors.append(f"{STACK_TAG_ENV} must equal 'test' (got {tag!r})")

    return LiveGateResult(ok=not errors, errors=tuple(errors))


def require_live_gates(**kwargs: object) -> None:
    result = validate_live_gates(**kwargs)  # type: ignore[arg-type]
    if not result.ok:
        raise RuntimeError("live E2E gate failed:\n- " + "\n- ".join(result.errors))