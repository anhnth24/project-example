"""Machine-verifiable O03 app mutation write-gate contract.

The detector is intentionally architecture-specific: a random `ops_fence::`
import is not enough. Required:

1. Central middleware file exporting `mutation_write_gate` and contract id.
2. Shared/exclusive advisory lock key 7303003 (matches backup capture).
3. Router wires `mutation_write_gate` as middleware.
4. Background mutation loops call `ensure_background_mutations_allowed`.
"""

from __future__ import annotations

from pathlib import Path

CONTRACT_ID = "markhand.write_gate.v1"
ADVISORY_LOCK_KEY = "7303003"
MIDDLEWARE_REL = Path("middleware/write_gate.rs")
HTTP_REL = Path("http.rs")


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def evaluate_write_gate_tree(server_src: Path) -> dict[str, bool]:
    """Evaluate each contract component under a server `src/` tree."""
    middleware = _read(server_src / MIDDLEWARE_REL)
    http = _read(server_src / HTTP_REL)
    return {
        "middleware_present": (server_src / MIDDLEWARE_REL).is_file(),
        "contract_id": f'WRITE_GATE_CONTRACT_ID: &str = "{CONTRACT_ID}"' in middleware
        or f'pub const WRITE_GATE_CONTRACT_ID: &str = "{CONTRACT_ID}"' in middleware,
        "lock_key": f"BACKUP_ADVISORY_LOCK_KEY: i64 = {ADVISORY_LOCK_KEY}" in middleware
        or f"pub const BACKUP_ADVISORY_LOCK_KEY: i64 = {ADVISORY_LOCK_KEY}" in middleware,
        "middleware_fn": "pub async fn mutation_write_gate" in middleware,
        "shared_lock": "pg_try_advisory_lock_shared" in middleware,
        "router_wired": "from_fn_with_state(state.clone(), mutation_write_gate)" in http,
        "background_quota_skip": (
            "fn start_quota_sweep" in http
            and "ensure_background_mutations_allowed(&pool)" in http
            and http.find("fn start_quota_sweep")
            < http.find("ensure_background_mutations_allowed(&pool)")
        ),
        "background_ask_stream_skip": (
            "fn start_ask_stream_maintenance" in http
            and "ensure_background_mutations_allowed(&pool)" in http
            and "ask stream maintenance skipped" in http
        ),
    }


def app_mutation_write_gate_sufficient_in(server_src: Path) -> bool:
    checks = evaluate_write_gate_tree(server_src)
    return all(checks.values())
