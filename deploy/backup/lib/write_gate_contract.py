"""Machine-verifiable O03 app mutation write-gate contract.

The detector is intentionally architecture-specific: a random `ops_fence::`
import is not enough. Required:

1. Central middleware file exporting `mutation_write_gate` and contract id.
2. Shared/exclusive advisory lock key 7303003 (matches backup capture).
3. Router wires `mutation_write_gate` as middleware.
4. Background mutation loops acquire RAII guards around real DB writes.
5. Ask stream producer acquires the same guard around append transactions.
"""

from __future__ import annotations

import re
from pathlib import Path

CONTRACT_ID = "markhand.write_gate.v1"
ADVISORY_LOCK_KEY = "7303003"
MIDDLEWARE_REL = Path("middleware/write_gate.rs")
HTTP_REL = Path("http.rs")
ASK_STREAM_REL = Path("services/qa/ask_stream.rs")


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def _strip_rust_noise(src: str) -> str:
    """Remove line/block comments and string literals so comment-only greps fail."""
    no_block = re.sub(r"/\*.*?\*/", " ", src, flags=re.S)
    no_line = re.sub(r"//.*?$", " ", no_block, flags=re.M)
    return re.sub(r'"(?:\\.|[^"\\])*"', '""', no_line)


def _fn_region(src: str, fn_name: str) -> str:
    """Best-effort body text for `fn <name>` until the next top-level fn."""
    match = re.search(rf"\bfn\s+{re.escape(fn_name)}\b", src)
    if not match:
        return ""
    start = match.start()
    nxt = re.search(r"\n(?:pub\s+)?(?:async\s+)?fn\s+\w+", src[match.end() :])
    end = match.end() + nxt.start() if nxt else len(src)
    return src[start:end]


def _guard_held_across(region: str, mutation_call: str) -> bool:
    """True when acquire...guard appears before mutation and release after it."""
    if not region or mutation_call not in region:
        return False
    acq = region.find("acquire_background_mutation_guard")
    mut = region.find(mutation_call)
    if acq < 0 or mut < 0 or acq > mut:
        return False
    # Release must follow the mutation in the same function region.
    rel = region.find(".release()", mut)
    if rel < 0:
        return False
    # Reject distant/ unrelated acquires: keep acquire near the mutation site.
    if mut - acq > 800:
        return False
    return True


def _ask_producer_append_guarded(ask_src: str) -> bool:
    """Require RAII guard around append_event_authorized inside run_producer."""
    code = _strip_rust_noise(ask_src)
    region = _fn_region(code, "run_producer")
    if not region:
        return False
    # Must not be a comment/string-only hit (already stripped).
    if "acquire_background_mutation_guard" not in region:
        return False
    if "append_event_authorized" not in region:
        return False
    # Locate the append closure / authorized append path: acquire before append,
    # release after. Prefer the window around append_event_authorized.
    idx = region.find("append_event_authorized")
    window_start = max(0, idx - 700)
    window = region[window_start : idx + 400]
    acq = window.find("acquire_background_mutation_guard")
    # acquire may sit just before the window if slightly farther — also accept
    # earlier in region when release follows append in the same async move block.
    if acq < 0:
        acq_region = region.rfind("acquire_background_mutation_guard", 0, idx)
        if acq_region < 0 or idx - acq_region > 900:
            return False
        after = region[idx : idx + 500]
        return ".release()" in after
    after_append = window[window.find("append_event_authorized") :]
    return ".release()" in after_append or ".release()" in region[idx : idx + 500]


def evaluate_write_gate_tree(server_src: Path) -> dict[str, bool]:
    """Evaluate each contract component under a server `src/` tree."""
    middleware = _read(server_src / MIDDLEWARE_REL)
    http = _read(server_src / HTTP_REL)
    ask = _read(server_src / ASK_STREAM_REL)
    # Ignore #[cfg(test)] modules so anti-pattern string checks stay honest.
    prod_middleware = middleware.split("#[cfg(test)]")[0]
    # Strip comments for structural checks; keep raw text for SQL/string anchors.
    http_code = _strip_rust_noise(http)
    middleware_code = _strip_rust_noise(prod_middleware)
    gate_fn = _fn_region(middleware_code, "mutation_write_gate")

    quota_region = _fn_region(http_code, "start_quota_sweep")
    ask_maint_region = _fn_region(http_code, "start_ask_stream_maintenance")

    return {
        "middleware_present": (server_src / MIDDLEWARE_REL).is_file(),
        "contract_id": f'WRITE_GATE_CONTRACT_ID: &str = "{CONTRACT_ID}"' in middleware
        or f'pub const WRITE_GATE_CONTRACT_ID: &str = "{CONTRACT_ID}"' in middleware,
        "lock_key": f"BACKUP_ADVISORY_LOCK_KEY: i64 = {ADVISORY_LOCK_KEY}" in middleware
        or f"pub const BACKUP_ADVISORY_LOCK_KEY: i64 = {ADVISORY_LOCK_KEY}" in middleware,
        "middleware_fn": "pub async fn mutation_write_gate" in middleware,
        "shared_lock": "pg_try_advisory_lock_shared" in prod_middleware,
        "holds_across_next_run": (
            "next.run(request).await" in gate_fn
            and "guard.release().await" in gate_fn
            and gate_fn.find("next.run(request).await")
            < gate_fn.find("guard.release().await")
            and "release_before_handler" not in prod_middleware
            and "is_long_lived_stream_path" not in middleware_code
        ),
        "background_guard_api": "pub async fn acquire_background_mutation_guard"
        in middleware,
        "router_wired": "from_fn_with_state(state.clone(), mutation_write_gate)" in http,
        "background_quota_guard": _guard_held_across(
            quota_region, "sweep_expired_all_orgs"
        ),
        "background_ask_stream_guard": _guard_held_across(
            ask_maint_region, "run_maintenance"
        ),
        "ask_producer_append_guard": _ask_producer_append_guarded(ask),
    }


def app_mutation_write_gate_sufficient_in(server_src: Path) -> bool:
    checks = evaluate_write_gate_tree(server_src)
    return all(checks.values())
