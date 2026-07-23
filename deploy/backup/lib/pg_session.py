#!/usr/bin/env python3
"""Long-lived psql session + private PGPASSFILE helpers (never touch caller env)."""

from __future__ import annotations

import os
import re
import subprocess
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator
from urllib.parse import unquote, urlparse


class PgSessionError(RuntimeError):
    pass


def _write_private_pgpass(host: str, port: int, db: str, user: str, password: str) -> Path:
    """Always create a unique private pgpass file; never reuse caller PGPASSFILE."""
    old = os.umask(0o077)
    try:
        fd, name = tempfile.mkstemp(
            prefix="o03-pgpass-",
            suffix=".pgpass",
            dir=os.environ.get("TMPDIR") or "/tmp",
        )
        os.close(fd)
        path = Path(name)
        path.write_text(f"{host}:{port}:{db}:{user}:{password}\n", encoding="utf-8")
        os.chmod(path, 0o600)
        return path
    finally:
        os.umask(old)


@contextmanager
def private_pg_env(database_url: str) -> Iterator[tuple[str, dict[str, str]]]:
    """Yield (argv-safe URL, env) with a private PGPASSFILE.

    Never reads, writes, or replaces the caller's ``os.environ['PGPASSFILE']``.
    """
    caller_pgpass = os.environ.get("PGPASSFILE")
    env = os.environ.copy()
    pgpass_path: Path | None = None
    try:
        parsed = urlparse(database_url)
        if parsed.password:
            host = parsed.hostname or "localhost"
            port = parsed.port or 5432
            user = unquote(parsed.username or "")
            password = unquote(parsed.password)
            db = (parsed.path or "/").lstrip("/") or "*"
            pgpass_path = _write_private_pgpass(host, port, db, user, password)
            env["PGPASSFILE"] = str(pgpass_path)
            netloc = f"{user}@{host}:{port}" if parsed.port else f"{user}@{host}"
            safe_url = parsed._replace(netloc=netloc).geturl()
        else:
            safe_url = database_url
        if os.environ.get("PGPASSFILE") != caller_pgpass:
            raise PgSessionError("caller PGPASSFILE was mutated")
        yield safe_url, env
    finally:
        if pgpass_path is not None:
            try:
                pgpass_path.unlink(missing_ok=True)
            except OSError:
                pass
        if os.environ.get("PGPASSFILE") != caller_pgpass:
            # Best-effort restore if something else raced; still signal error.
            if caller_pgpass is None:
                os.environ.pop("PGPASSFILE", None)
            else:
                os.environ["PGPASSFILE"] = caller_pgpass
            raise PgSessionError("caller PGPASSFILE was mutated")


# Back-compat name used by identity/pipeline.
def safe_psql_url(database_url: str) -> tuple[str, dict[str, str], Path | None]:
    """Non-context helper — prefer ``private_pg_env``. Returns pgpass path to unlink."""
    caller_pgpass = os.environ.get("PGPASSFILE")
    env = os.environ.copy()
    parsed = urlparse(database_url)
    if not parsed.password:
        return database_url, env, None
    host = parsed.hostname or "localhost"
    port = parsed.port or 5432
    user = unquote(parsed.username or "")
    password = unquote(parsed.password)
    db = (parsed.path or "/").lstrip("/") or "*"
    pgpass = _write_private_pgpass(host, port, db, user, password)
    env["PGPASSFILE"] = str(pgpass)
    netloc = f"{user}@{host}:{port}" if parsed.port else f"{user}@{host}"
    if os.environ.get("PGPASSFILE") != caller_pgpass:
        raise PgSessionError("caller PGPASSFILE was mutated")
    return parsed._replace(netloc=netloc).geturl(), env, pgpass


class PgSession:
    """Keep one psql backend open so session-level advisory locks persist."""

    def __init__(self, database_url: str):
        self._url = database_url
        self._proc: subprocess.Popen[str] | None = None
        self._pgpass: Path | None = None
        self._caller_pgpass = os.environ.get("PGPASSFILE")

    def __enter__(self) -> "PgSession":
        self._start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def _start(self) -> None:
        env = os.environ.copy()
        parsed = urlparse(self._url)
        if parsed.password:
            host = parsed.hostname or "localhost"
            port = parsed.port or 5432
            user = unquote(parsed.username or "")
            password = unquote(parsed.password)
            db = (parsed.path or "/").lstrip("/") or "*"
            self._pgpass = _write_private_pgpass(host, port, db, user, password)
            env["PGPASSFILE"] = str(self._pgpass)
            netloc = f"{user}@{host}:{port}" if parsed.port else f"{user}@{host}"
            safe_url = parsed._replace(netloc=netloc).geturl()
        else:
            safe_url = self._url
        if os.environ.get("PGPASSFILE") != self._caller_pgpass:
            raise PgSessionError("caller PGPASSFILE was mutated")

        self._proc = subprocess.Popen(
            ["psql", safe_url, "-v", "ON_ERROR_STOP=1", "-Atq"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            bufsize=1,
        )

    def query(self, sql: str, timeout_s: float = 120.0) -> str:
        if not self._proc or not self._proc.stdin or not self._proc.stdout:
            raise PgSessionError("session not started")
        marker = f"__O03_DONE_{time.time_ns()}__"
        script = sql.strip().rstrip(";") + f";\n\\echo {marker}\n"
        self._proc.stdin.write(script)
        self._proc.stdin.flush()
        lines: list[str] = []
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            if self._proc.poll() is not None:
                err = self._proc.stderr.read() if self._proc.stderr else ""
                raise PgSessionError(f"psql exited early: {err[:200]}")
            line = self._proc.stdout.readline()
            if line == "":
                time.sleep(0.01)
                continue
            text = line.rstrip("\n")
            if text == marker:
                break
            lines.append(text)
        else:
            raise PgSessionError("psql query timeout")
        return "\n".join(lines).strip()

    def try_advisory_lock(self, key: int = 7303003) -> bool:
        got = self.query(f"SELECT pg_try_advisory_lock({int(key)});")
        return got in {"t", "true"}

    def unlock(self, key: int = 7303003) -> None:
        try:
            self.query(f"SELECT pg_advisory_unlock({int(key)});")
        except PgSessionError:
            pass

    def close(self) -> None:
        try:
            if self._proc:
                try:
                    if self._proc.stdin:
                        self._proc.stdin.write("\\q\n")
                        self._proc.stdin.flush()
                except Exception:
                    pass
                try:
                    self._proc.kill()
                except Exception:
                    pass
                self._proc = None
        finally:
            if self._pgpass and self._pgpass.is_file():
                try:
                    self._pgpass.unlink()
                except OSError:
                    pass
                self._pgpass = None
            if os.environ.get("PGPASSFILE") != self._caller_pgpass:
                if self._caller_pgpass is None:
                    os.environ.pop("PGPASSFILE", None)
                else:
                    os.environ["PGPASSFILE"] = self._caller_pgpass


def assert_no_password_argv(cmd: list[str]) -> None:
    joined = " ".join(cmd)
    if re.search(r"postgres(?:ql)?://[^:]+:[^@]+@", joined):
        raise PgSessionError("password must not appear on argv (use PGPASSFILE)")


def assert_no_mc_credentials_argv(cmd: list[str]) -> None:
    """Refuse mc alias/config forms that place access keys on argv."""
    if not cmd:
        return
    joined = " ".join(cmd)
    if cmd[0] == "mc" or cmd[0].endswith("/mc"):
        if "alias" in cmd and "set" in cmd:
            raise PgSessionError("mc alias credentials must not appear on argv (use MC_HOST_*)")
        # Common mistaken form: scheme://access:secret@host on argv
        if re.search(r"https?://[^:]+:[^@]+@", joined):
            raise PgSessionError("MinIO credentials must not appear on argv (use MC_HOST_*)")
