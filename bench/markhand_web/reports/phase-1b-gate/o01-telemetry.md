# P1B-O01 telemetry evidence

- Status: `incomplete`
- Git: `cf1384d` / `cf1384d-dirty`
- Raw: `/workspace/bench/markhand_web/reports/phase-1b-gate/raw/o01-cf1384d` (redacted)
- Blockers: 1

## Commands

- `cargo_telemetry`: `cargo test -p fileconv-server telemetry -- --nocapture`
- `cargo_live_o01`: `cargo test -p fileconv-server --test telemetry_audit -- --ignored --nocapture`
- `evidence`: `python3 bench/markhand_web/scripts/run_o01_telemetry_evidence.py`

- BLOCKER: async API→worker→provider canary not opted in (MARKHAND_O01_ASYNC!=1)
