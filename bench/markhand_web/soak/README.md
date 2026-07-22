# Phase 1B mixed-load soak (P1B-O05)

```bash
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate
```

Gates (numeric) are defined in `bench/markhand_web/gates.yaml` and summarized into
`reports/phase-1b-gate/summary.json`. A soak is only `pass` when:

- no unbounded memory/temp/connection/queue growth;
- recovery/worker kill/dependency blip scenarios are exercised;
- post-restore retrieval still denies deleted/unauthorized content;
- exact app/migration/index versions are recorded.
