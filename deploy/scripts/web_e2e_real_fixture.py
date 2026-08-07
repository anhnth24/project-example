#!/usr/bin/env python3
"""Stable setup/cleanup/verify-clean entrypoint for real web E2E fixtures."""

from __future__ import annotations

import subprocess  # Re-exported for hermetic argv tests.
import sys

import web_e2e_fixture_cli as fixture_cli
from web_e2e_fixture_adapters import Commands, HttpResponse, LiveCommands
from web_e2e_fixture_cli import (
    build_parser,
    cmd_cleanup,
    cmd_setup,
    cmd_verify_clean,
    collect_leaks,
    main,
)
from web_e2e_fixture_database import hard_delete_run_rows
from web_e2e_fixture_identity import (
    FixtureError,
    FixtureLeakError,
    FixtureProbeError,
    _assert_manifest_public,
    _atomic_write_json,
    _email_for_run,
    _load_json,
    _manifest_ids,
    _remove_credentials,
    _slug_for_run,
    fixture_checksum,
    is_production_profile,
    opaque_identity,
    quarantine_object_key,
    refuse_production,
    validate_run_id,
    validate_uuid,
)

monotonic = fixture_cli.monotonic


def _hard_delete_run_rows(commands: Commands, ids: dict[str, object]) -> None:
    """Compatibility seam for focused tests; production shares one bounded deadline."""

    from web_e2e_fixture_adapters import Deadline

    hard_delete_run_rows(
        commands=commands,
        ids=ids,
        deadline=Deadline(30.0, clock=fixture_cli.monotonic),
    )


if __name__ == "__main__":
    sys.exit(main())
