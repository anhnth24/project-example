# Phase 1B mixed-load soak (P1B-O05)

Measured fail-closed soak harness. Default is honest `not_run`.
`MARKHAND_SOAK=1` alone without prerequisites/metrics cannot pass.

## Commands

```bash
# Hermetic tests
python3 bench/markhand_web/soak/run_soak.py --self-test

# Template evidence (not_run)
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate

# Smoke only (non-qualifying; never pass)
MARKHAND_SOAK=1 python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate \
  --duration-seconds 30

# Official live (duration must be profile 1800 exactly)
MARKHAND_SOAK=1 bash deploy/scripts/o05-soak.sh --enable-failure-injection
```

## Artifacts

| File | Role |
|---|---|
| `reports/phase-1b-gate/o05-soak.json` | Canonical machine report |
| `reports/phase-1b-gate/o05-soak.md` | Human summary |
| `reports/phase-1b-gate/raw/o05-<stamp>/` | Redacted raw samples |
| `reports/phase-1b-gate/summary.json` | Thin O05 pointer (`issue=P1B-O05`) |

See `docs/runbooks/phase-1b/soak-o05.md` for prerequisites, thresholds, and
failure-injection safety rules.
