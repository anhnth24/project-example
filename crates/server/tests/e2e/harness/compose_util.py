"""Compose argv helpers for fault injection / authz probes (no storage mutation)."""

from __future__ import annotations

import subprocess
from typing import Sequence


def run_compose(compose: Sequence[str], args: Sequence[str], *, check: bool = True) -> str:
    cmd = list(compose) + list(args)
    proc = subprocess.run(cmd, check=False, text=True, capture_output=True)
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"compose command failed ({proc.returncode}): {' '.join(args)}\n{proc.stderr[-2000:]}"
        )
    return proc.stdout