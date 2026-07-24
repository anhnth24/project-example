# Deployment scripts

Deterministic setup for the CPU-only dev stack in `deploy/dev/`, plus POC
boot/isolation helpers for `deploy/compose.poc.yml` (P1B-F02). Production
orchestration belongs to Phase 4.

POC:

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
deploy/scripts/poc-isolation-smoke.sh
deploy/scripts/poc-boot-evidence.sh --self-test
POC_EVIDENCE_RAW_DIR=bench/markhand_web/reports/phase-1b-gate/raw/f02-$(git rev-parse --short HEAD) \
  deploy/scripts/poc-boot-evidence.sh
```

See [`deploy/README.md`](../README.md).

Phase 1B gate harnesses:

```bash
bash deploy/scripts/o04-release-suite.sh --self-test
bash deploy/scripts/o05-soak.sh --self-test
# Official O05 live (MARKHAND_SOAK=1, ~1800s): see docs/runbooks/phase-1b/soak-o05.md
```

Full runbook: [`docs/runbooks/local-development.md`](../../docs/runbooks/local-development.md).

## Recommended first-time flow

```bash
deploy/scripts/init-dev-env.sh     # .env + worker.env (AITeamVN defaults)
make dev-up && make dev-health     # first health may take ~15 min (model download)
set -a && source deploy/dev/.env && set +a
deploy/scripts/bootstrap-server-role.sh
cargo run -p fileconv-server       # migrations, then Ctrl+C
deploy/scripts/seed-dev-all.sh --skip-init
make dev-print-defaults
cargo run -p fileconv-server
```

Optional prefetch (before `dev-up`):

```bash
deploy/scripts/download-aiteamvn-embedding.sh
```

## Embedding profiles

| Profile | Service | Use |
|---|---|---|
| **`aiteamvn`** (default) | `embedding-cpu` | AITeamVN @ `:8088` — **dev + on-prem CPU prod path** |
| `mock` | `mock-embedding` | 8-dim stub — **CI** (`COMPOSE_PROFILES=mock`) |
| `gpu` | `vllm` | Optional GPU inference |

Set in `deploy/dev/.env`: `COMPOSE_PROFILES=aiteamvn`

## Scripts

| Script | Purpose |
|---|---|
| [`init-dev-env.sh`](init-dev-env.sh) | Create `.env` / `worker.env` from examples |
| [`download-aiteamvn-embedding.sh`](download-aiteamvn-embedding.sh) | Prefetch HF weights into Docker volume |
| [`seed-dev-all.sh`](seed-dev-all.sh) | Full DB seed after migrations |
| [`print-dev-defaults.sh`](print-dev-defaults.sh) | Defaults reference card |
| [`aiteamvn-embedding-server.py`](aiteamvn-embedding-server.py) | OpenAI-compatible CPU server (also in Docker image) |

## Dev defaults (AITeamVN CPU)

| Item | Value |
|---|---|
| Model | `AITeamVN/Vietnamese_Embedding` @ `dea33aa1…` |
| Dimensions | 1024 |
| API | `http://127.0.0.1:8088/v1/embeddings` |
| API key | `dev-embedding-key` |
| Index signature | `ca03085c08f4c01d391ac973192815c944892f6e74b52e7bf4e1f135f65ae97c` |
| Login password | `markhand-dev` |

Recompute signature after env change:

```bash
python3 deploy/scripts/print-index-signature.py
```
