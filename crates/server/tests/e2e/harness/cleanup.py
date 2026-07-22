"""Mutation helpers with mandatory finally-restore (live isolation)."""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import Callable, Iterator, Sequence

from .compose_util import run_compose


class CleanupFailed(RuntimeError):
    """Cleanup/restore failed — high/critical for the release suite."""

    code = "cleanup_failed"


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


def sql_exec(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    sql: str,
) -> str:
    return run_compose(
        compose,
        [
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
            "-c",
            sql,
        ],
    )


def disable_user(
    compose: Sequence[str],
    *,
    postgres_user: str,
    postgres_db: str,
    email: str,
    stack: CleanupStack,
) -> None:
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=f"UPDATE users SET disabled_at = now() WHERE email = '{email}';",
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql=f"UPDATE users SET disabled_at = NULL WHERE email = '{email}';",
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
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=(
            f"DELETE FROM org_memberships WHERE org_id = '{org_id}' AND user_id = ("
            f"SELECT id FROM users WHERE email = '{email}');"
        ),
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql=(
                "INSERT INTO org_memberships (org_id, user_id, role) "
                f"SELECT '{org_id}', id, '{role}' FROM users WHERE email = '{email}' "
                "ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;"
            ),
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
    sql_exec(
        compose,
        postgres_user=postgres_user,
        postgres_db=postgres_db,
        sql=(
            f"SELECT set_config('app.org_id', '{org_id}', true); "
            f"UPDATE collections SET visibility = '{visibility}' WHERE id = '{collection_id}'; "
            f"DELETE FROM collection_user_access WHERE collection_id = '{collection_id}';"
        ),
    )

    def restore() -> None:
        sql_exec(
            compose,
            postgres_user=postgres_user,
            postgres_db=postgres_db,
            sql=(
                f"SELECT set_config('app.org_id', '{org_id}', true); "
                f"UPDATE collections SET visibility = '{previous}' WHERE id = '{collection_id}';"
            ),
        )

    stack.push(restore)


def stop_service(
    compose: Sequence[str],
    service: str,
    stack: CleanupStack,
) -> None:
    run_compose(compose, ["stop", service], check=False)

    def restore() -> None:
        run_compose(compose, ["start", service], check=False)

    stack.push(restore)


def kill_and_restart_service(
    compose: Sequence[str],
    service: str,
    stack: CleanupStack,
) -> None:
    run_compose(compose, ["kill", service], check=False)

    def restore() -> None:
        run_compose(compose, ["up", "-d", service], check=False)

    stack.push(restore)
    # Immediately schedule restart so suite continues; restore is idempotent.
    run_compose(compose, ["up", "-d", service], check=False)
