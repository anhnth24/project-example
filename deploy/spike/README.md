# Phase 0 benchmark spike

The spike composes the Phase F local services with isolated project names, ports and
volumes. CPU smoke uses the deterministic embedding stub; the `gpu` profile adds vLLM
only when `SPIKE_GPU=1` and runtime model configuration is supplied outside Git.

```bash
cp deploy/spike/.env.example deploy/spike/.env
deploy/spike/up.sh
deploy/spike/health.sh
deploy/spike/down.sh
deploy/spike/reset.sh
```

`up.sh` performs health, PostgreSQL/Qdrant/MinIO seed, then writes a non-secret
fingerprint to `bench/markhand_web/reports/spike-environment.json`.

Validation:

```bash
python3 scripts/validate_spike.py --config-only
python3 scripts/validate_spike.py
```

The current-runner CPU smoke cannot satisfy Profile B. Target evidence requires the
approved 32-core/256GB/4TB/24GB-GPU/10Gbps hardware and independently verified NVMe
IOPS.
