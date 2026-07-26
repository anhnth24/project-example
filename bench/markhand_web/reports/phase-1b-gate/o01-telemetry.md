# P1B-O01 telemetry evidence

- Status: `pass`
- Git: `f4f33cd1b` / `f4f33cd1b`
- Raw: `/home/administrator/markhand/.artifacts/markhand_web/reports/phase-1b-gate/raw/o01-f4f33cd1b` (redacted)
- Blockers: 0

## Commands

- `cargo_telemetry`: `cargo test -p fileconv-server telemetry -- --nocapture`
- `cargo_live_o01`: `cargo test -p fileconv-server --test telemetry_audit -- --ignored --nocapture`
- `capture_unit`: `python3 deploy/scripts/test_otel_capture.py`
- `evidence`: `python3 bench/markhand_web/scripts/run_o01_telemetry_evidence.py`
