#!/usr/bin/env python3
"""PostgreSQL source/target identity via pg_control_system + current_database."""

from __future__ import annotations

import json
import os
import subprocess
import sys

from pg_session import assert_no_password_argv, private_pg_env


class PgIdentityError(ValueError):
    pass


def _psql(database_url: str, sql: str) -> str:
    with private_pg_env(database_url) as (safe_url, env):
        cmd = ["psql", safe_url, "-v", "ON_ERROR_STOP=1", "-Atc", sql]
        assert_no_password_argv(cmd)
        proc = subprocess.run(
            cmd,
            check=False,
            capture_output=True,
            text=True,
            env=env,
        )
    if proc.returncode != 0:
        raise PgIdentityError("psql identity query failed")
    return proc.stdout.strip()


def read_identity(database_url: str) -> dict[str, str]:
    sys_id = _psql(database_url, "SELECT system_identifier::text FROM pg_control_system();")
    db = _psql(database_url, "SELECT current_database();")
    if not sys_id.isdigit() or not db:
        raise PgIdentityError("invalid pg identity")
    return {"pgSystemIdentifier": sys_id, "pgDatabase": db}


def assert_green_allowlisted(
    green_url: str,
    blue_url: str,
    allowlist_json: str,
) -> dict[str, str]:
    if not allowlist_json.strip():
        raise PgIdentityError("MARKHAND_GREEN_ALLOWLIST_JSON required")
    try:
        allow = json.loads(allowlist_json)
    except json.JSONDecodeError as exc:
        raise PgIdentityError("green allowlist JSON malformed") from exc
    if not isinstance(allow, list) or not allow:
        raise PgIdentityError("green allowlist must be non-empty list")
    green = read_identity(green_url)
    blue = read_identity(blue_url)
    if green["pgSystemIdentifier"] == blue["pgSystemIdentifier"] and green[
        "pgDatabase"
    ] == blue["pgDatabase"]:
        raise PgIdentityError("green identity equals blue; refuse destructive restore")
    ok = False
    for entry in allow:
        if not isinstance(entry, dict):
            continue
        if (
            str(entry.get("pgSystemIdentifier")) == green["pgSystemIdentifier"]
            and str(entry.get("pgDatabase")) == green["pgDatabase"]
        ):
            ok = True
            break
    if not ok:
        raise PgIdentityError("green identity not in MARKHAND_GREEN_ALLOWLIST_JSON")
    return green


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: pg_identity.py read|assert-green", file=sys.stderr)
        return 2
    cmd = argv[1]
    try:
        if cmd == "read":
            url = os.environ["DATABASE_URL"]
            print(json.dumps(read_identity(url)))
            return 0
        if cmd == "assert-green":
            green = assert_green_allowlisted(
                os.environ["MARKHAND_GREEN_DATABASE_URL"],
                os.environ["DATABASE_URL"],
                os.environ.get("MARKHAND_GREEN_ALLOWLIST_JSON", ""),
            )
            print(json.dumps(green))
            return 0
    except (PgIdentityError, KeyError) as exc:
        print(f"pg_identity_error: {exc}", file=sys.stderr)
        return 1
    print("unknown command", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
