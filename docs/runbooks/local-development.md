# Local development environment

F-08 provides a CPU-only stack for development, not benchmark or production evidence.
It starts PostgreSQL, Qdrant, MinIO, OpenTelemetry Collector and a deterministic mock
embedding endpoint. All ports bind to `127.0.0.1`; named volumes retain data until reset.

## Prerequisites

- Docker Engine with Compose v2.
- Bash and curl.
- Optional NVIDIA runtime only when explicitly enabling the `gpu` profile.

## Commands

```bash
cp deploy/dev/.env.example deploy/dev/.env
deploy/scripts/up.sh
deploy/scripts/health.sh
deploy/scripts/down.sh
deploy/scripts/reset.sh
```

`up.sh` starts services, waits for health and inserts only `markhand_dev_seed`. MinIO
initializes `markhand-quarantine`, `markhand-documents` and `markhand-artifacts`.
`reset.sh` destroys only volumes in the selected Compose project, then starts a clean
stack. Do not point `MARKHAND_COMPOSE_PROJECT` at shared/prod resources.

## Endpoints

| Service | Local endpoint |
|---|---|
| PostgreSQL | `127.0.0.1:54329` |
| Qdrant | `http://127.0.0.1:6333` |
| MinIO API / console | `http://127.0.0.1:9000` / `http://127.0.0.1:9001` |
| OTLP gRPC / health | `127.0.0.1:4317` / `http://127.0.0.1:13133` |
| Mock embeddings | `http://127.0.0.1:8088/v1/embeddings` |

## GPU profile

The default stack never starts vLLM. To opt in, set a permitted local model reference
and run `docker compose --profile gpu up -d vllm`. GPU model choice and performance
evidence remain Phase 0 decisions.

## Failure and reset

- `docker compose -f deploy/dev/compose.yml logs <service>` for startup diagnostics.
- Run `deploy/scripts/health.sh` after restart; it reports the failed endpoint.
- Use `reset.sh` only for local dev data. Never use this stack for customer data,
  production credentials or benchmark corpus.
