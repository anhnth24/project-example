"""Compose argv helpers for fault injection / authz probes (no storage mutation)."""

from __future__ import annotations

import subprocess
from typing import Sequence


class ComposeCommandFailed(RuntimeError):
    """A docker compose mutation/control command failed."""

    def __init__(self, args: Sequence[str], returncode: int, stderr: str) -> None:
        self.args = list(args)
        self.returncode = returncode
        self.stderr = stderr
        super().__init__(
            f"compose command failed ({returncode}): {' '.join(args)}\n{stderr[-2000:]}"
        )


def run_compose(compose: Sequence[str], args: Sequence[str], *, check: bool = True) -> str:
    cmd = list(compose) + list(args)
    proc = subprocess.run(cmd, check=False, text=True, capture_output=True)
    if check and proc.returncode != 0:
        raise ComposeCommandFailed(args, proc.returncode, proc.stderr or proc.stdout or "")
    return proc.stdout
