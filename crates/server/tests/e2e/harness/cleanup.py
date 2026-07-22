"""Mutation helpers with mandatory finally-restore (live isolation).

SQL uses typed/validated psql variables — never string interpolation of
UUID/email/token values into SQL text.
"""

from __future__ import annotations

import re
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Callable, Iterator, Sequence

from .compose_util import ComposeCommandFailed, run_compose

UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
)
EMAIL_RE = re.compile(r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$")
ROLE_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}$")
VISIBILITY_RE = re.compile(r"^(org|private|public)$")
SERVICE_RE = re.compile(r"^[a-z][a-z0-9\-]{0,63}$")
TESTISH_RE = re.compile(r"(?:^|[-_])(?:e2e|test)(?:[-_]|$)", re.I)


class CleanupFailed(RuntimeError):
    """Cleanup/restore failed — high/critical for the release suite."""

    code = "cleanup_failed"


class IsolationError(RuntimeError):
    """Cleanup refused because the stack is not a tagged e2e/test environment."""

    code = "cleanup_isolation"


UndoFn = Callable[[], None]


@dataclass
class CleanupStack:
    _undos: list[UndoFn] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)

    def push(self, undo: UndoFn) -> None:
        self._undos.append(undo)

    def run_all(self) -> None:
        while self._undos:
            undo = self._undos.pop()
            try:
                undo()
            except Exception as exc:  # noqa: BLE001 — aggregate cleanup errors
                self.failures.append(str(exc))
        if self.failures:
            raise CleanupFailed(
                "cleanup restore failed: " + "; ".join(self.failures[:8])
            )


@contextmanager
def mutation_scope(stack: CleanupStack | None = None) -> Iterator[CleanupStack]:
    owned = stack is None
    active = stack or CleanupStack()
    try:
        yield active
    finally:
        if owned:
            active.run_all()


def require_uuid(label: str, value: str) -> str:
    if not UUID_RE.fullmatch(value):
        raise ValueError(f"invalid UUID for {label}: {value!r}")
    return value


def require_email(label: str, value: str) -> str:
    if not EMAIL_RE.fullmatch(value) or len(value) > 254:
        raise ValueError(f"invalid email for {label}: {value!r}")
    return value


def require_role(label: str, value: str) -> str:
    if not ROLE_RE.fullmatch(value):
        raise ValueError(f"invalid role for {label}: {value!r}")
    return value


def require_visibility(label: str, value: str) -> str:
    if not VISIBILITY_RE.fullmatch(value):
        raise ValueError(f"invalid visibility for {label}: {value!r}")
    return value


def require_service(label: str, value: str) -> str:
    if not SERVICE_RE.fullmatch(value):
        raise ValueError(f"invalid service for {label}: {value!r}")
    return value


def verify_cleanup_isolation(
    *,
    compose_project: str,
    postgres_db: str,
    minio_bucket: str,
    stack_tag: str,
) -> None:
    """Refuse cleanup against untagged/human stacks."""
    errors: list[str] = []
    for label, value in (
        ("MARKHAND_COMPOSE_PROJECT", compose_project),
        ("MARKHAND_POSTGRES_DB", postgres_db),
        ("MARKHAND_MINIO_BUCKET", minio_bucket),
    ):
        if not value or not TESTISH_RE.search(value):
            errors.append(f"{label} must contain an e2e/test name segment (got {value!r})")
    if compose_project in {"markhand", "markhand-poc"}:
        errors.append(f"refusing human compose project {compose_project!r}")
    if postgres_db == "markhand":
        errors.append("refusing human postgres db 'markhand'")
    if minio_bucket == "markhand-documents":
        errors.append("refusing human minio bucket")
    if stack_tag != "test":
        errors.append(f"MARKHAND_E2E_STACK_TAG must be 'test' (got {stack_tag!r})")
    if errors:
        raise IsolationError("; ".join(errors))


def sql_exec(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    sql: str,
    variables: dict[str, str] | None = None,
) -> str:
    """Execute SQL with psql -v variables (values never interpolated into SQL)."""
    args: list[str] = [
        "exec",
        "-T",
        "postgres",
        "psql",
        "-U",
        postgres_user,
        "-d",
        postgres_db,
        "--set",
        "ON_ERROR_STOP=1",
    ]
    for key, value in (variables or {}).items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            raise ValueError(f"invalid psql variable name: {key!r}")
        # Disallow shell/metacharacters in values even though -v passes them safely.
        if any(ch in value for ch in ("\n", "\r", "\x00")):
            raise ValueError(f"invalid psql variable value for {key}")
        args.extend(["-v", f"{key}={value}"])
    args.extend(["-c", sql])
    try:
        return run_compose(compose, args, check=True)
    except ComposeCommandFailed as exc:
        raise CleanupFailed(str(exc)) from exc


def sql_query_scalar(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    sql: str,
    variables: dict[str, str] | None = None,
) -> str:
    """Execute a query that must return exactly one non-empty scalar row."""
    args: list[str] = [
        "exec",
        "-T",
        "postgres",
        "psql",
        "-U",
        postgres_user,
        "-d",
        postgres_db,
        "--set",
        "ON_ERROR_STOP=1",
        "-tA",
    ]
    for key, value in (variables or {}).items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            raise ValueError(f"invalid psql variable name: {key!r}")
        if any(ch in value for ch in ("\n", "\r", "\x00")):
            raise ValueError(f"invalid psql variable value for {key}")
        args.extend(["-v", f"{key}={value}"])
    args.extend(["-c", sql])
    try:
        output = run_compose(compose, args, check=True)
    except ComposeCommandFailed as exc:
        raise CleanupFailed(str(exc)) from exc
    rows = [line for line in output.splitlines() if line.strip()]
    if len(rows) != 1:
        raise CleanupFailed(f"expected one scalar row, got {len(rows)}")
    return rows[0]


def disable_user(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    email: str,
    stack: CleanupStack,
) -> None:
    email = require_email("email", email)
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql="UPDATE users SET disabled_at = now() WHERE email = :'email';",
        variables={"email": email},
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql="UPDATE users SET disabled_at = NULL WHERE email = :'email';",
            variables={"email": email},
        )

    stack.push(restore)


def remove_membership(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    org_id: str,
    email: str,
    role: str,
    stack: CleanupStack,
) -> None:
    org_id = require_uuid("org_id", org_id)
    email = require_email("email", email)
    role = require_role("role", role)
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=(
            "DELETE FROM org_memberships WHERE org_id = :'org_id'::uuid AND user_id = ("
            "SELECT id FROM users WHERE email = :'email');"
        ),
        variables={"org_id": org_id, "email": email},
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql=(
                "INSERT INTO org_memberships (org_id, user_id, role) "
                "SELECT :'org_id'::uuid, id, :'role' FROM users WHERE email = :'email' "
                "ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;"
            ),
            variables={"org_id": org_id, "email": email, "role": role},
        )

    stack.push(restore)


def set_collection_visibility(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    org_id: str,
    collection_id: str,
    visibility: str,
    previous: str,
    stack: CleanupStack,
) -> None:
    org_id = require_uuid("org_id", org_id)
    collection_id = require_uuid("collection_id", collection_id)
    visibility = require_visibility("visibility", visibility)
    previous = require_visibility("previous", previous)
    acl_snapshot = sql_query_scalar(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=(
            "WITH configured AS MATERIALIZED ("
            "SELECT set_config('app.org_id', :'org_id', true)"
            ") "
            "SELECT COALESCE("
            "jsonb_agg(jsonb_build_object("
            "'org_id', acl.org_id, "
            "'collection_id', acl.collection_id, "
            "'user_id', acl.user_id, "
            "'access_level', acl.access_level, "
            "'created_at', acl.created_at"
            ")) FILTER (WHERE acl.id IS NOT NULL), "
            "'[]'::jsonb"
            ")::text "
            "FROM configured "
            "LEFT JOIN collection_user_access acl "
            "ON acl.collection_id = :'collection_id'::uuid;"
        ),
        variables={
            "org_id": org_id,
            "collection_id": collection_id,
        },
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql=(
                "SELECT set_config('app.org_id', :'org_id', true); "
                "UPDATE collections SET visibility = :'previous' "
                "WHERE id = :'collection_id'::uuid; "
                "INSERT INTO collection_user_access "
                "(org_id, collection_id, user_id, access_level, created_at) "
                "SELECT saved.org_id, saved.collection_id, saved.user_id, "
                "saved.access_level, saved.created_at "
                "FROM jsonb_to_recordset(:'acl_snapshot'::jsonb) AS saved("
                "org_id uuid, collection_id uuid, user_id uuid, "
                "access_level text, created_at timestamptz"
                ") "
                "ON CONFLICT (collection_id, user_id) DO UPDATE "
                "SET access_level = EXCLUDED.access_level, "
                "created_at = EXCLUDED.created_at;"
            ),
            variables={
                "org_id": org_id,
                "collection_id": collection_id,
                "previous": previous,
                "acl_snapshot": acl_snapshot,
            },
        )

    stack.push(restore)
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=(
            "SELECT set_config('app.org_id', :'org_id', true); "
            "UPDATE collections SET visibility = :'visibility' "
            "WHERE id = :'collection_id'::uuid; "
            "DELETE FROM collection_user_access WHERE collection_id = :'collection_id'::uuid;"
        ),
        variables={
            "org_id": org_id,
            "collection_id": collection_id,
            "visibility": visibility,
        },
    )


def stop_service(
    compose: Sequence[str],
    service: str,
    stack: CleanupStack,
) -> None:
    service = require_service("service", service)
    try:
        run_compose(compose, ["stop", service], check=True)
    except ComposeCommandFailed as exc:
        raise CleanupFailed(f"stop {service} failed: {exc}") from exc

    def restore() -> None:
        try:
            run_compose(compose, ["start", service], check=True)
        except ComposeCommandFailed as exc:
            raise CleanupFailed(f"start {service} failed: {exc}") from exc

    stack.push(restore)


def kill_and_restart_service(
    compose: Sequence[str],
    service: str,
    stack: CleanupStack,
) -> None:
    service = require_service("service", service)
    try:
        run_compose(compose, ["kill", service], check=True)
    except ComposeCommandFailed as exc:
        raise CleanupFailed(f"kill {service} failed: {exc}") from exc

    def restore() -> None:
        try:
            run_compose(compose, ["up", "-d", service], check=True)
        except ComposeCommandFailed as exc:
            raise CleanupFailed(f"up {service} failed: {exc}") from exc

    stack.push(restore)
    try:
        run_compose(compose, ["up", "-d", service], check=True)
    except ComposeCommandFailed as exc:
        raise CleanupFailed(f"restart {service} failed: {exc}") from exc


def schedule_document_delete(
    delete_fn: Callable[[], None],
    stack: CleanupStack,
) -> None:
    """Register synthetic document deletion through a supported API callback."""

    def undo() -> None:
        try:
            delete_fn()
        except Exception as exc:  # noqa: BLE001
            raise CleanupFailed(f"synthetic document delete failed: {exc}") from exc

    stack.push(undo)
