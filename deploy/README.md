# Markhand deployment

## Local development

See [`dev/README.md`](dev/README.md) and
[`docs/runbooks/local-development.md`](../docs/runbooks/local-development.md).
The `deploy/dev` workflow is unchanged by the POC stack.

## POC stack (P1B-F02)

Pinned compose stack for a secure single-org POC: API + convert/index/embedding
workers, Postgres, Qdrant, MinIO (narrow app credentials), and embedding
(mock by default or AITeamVN CPU).

| Artifact | Purpose |
|---|---|
| [`Dockerfile.server`](Dockerfile.server) | `fileconv-server` API image (UID 10001) |
| [`Dockerfile.worker`](Dockerfile.worker) | `fileconv-worker` + lean `fileconv` (no PhoWhisper) |
| [`compose.poc.yml`](compose.poc.yml) | Hardened services + networks |
| [`.env.example`](.env.example) | POC env template (copy to `deploy/.env`) |

### Clean-host boot

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh      # build images, compose up, health
deploy/scripts/poc-health.sh  # re-check anytime
```

Defaults:

- `COMPOSE_PROFILES=mock` — deterministic 8-d embedding, no GPU/HF download
- API on `http://127.0.0.1:8788`
- Host ports are loopback-only and offset from `deploy/dev` to avoid clashes

AITeamVN CPU embedding (not GLM):

```bash
# edit deploy/.env: COMPOSE_PROFILES=aiteamvn + AITeamVN signature/URL block
deploy/scripts/poc-up.sh
```

### Isolation matrix

| Control | api | worker-convert | worker-index / embedding |
|---|---|---|---|
| non-root UID 10001 | yes | yes | yes |
| `read_only` rootfs | yes | yes | yes |
| `tmpfs` scratch | yes | yes (512m `/tmp`) | yes |
| `cap_drop: ALL` | yes | yes | yes |
| `no-new-privileges` | yes | yes | yes |
| mem/cpu/pids limits | yes | yes | yes |
| network | `edge`+`private` | **`convert` only (`internal: true`)** | `private` |

Convert path has no external egress: the `convert` network is `internal: true` and
only shares Postgres + MinIO. Index/embedding workers stay on `private` so they
can reach Qdrant and the embedding service. In-process convert sandbox still
applies rlimits / network unshare (see `crates/server` workers).

### Secrets and models

- MinIO root credentials are init-only; app/workers use `MARKHAND_MINIO_ACCESS_KEY`
  scoped by [`poc/minio-app-policy.json`](poc/minio-app-policy.json).
- PhoWhisper and other unresolved-license models are **not** bundled.
- Do not commit `deploy/.env`.

### Validation (no GPU required)

```bash
deploy/scripts/poc-isolation-smoke.sh
```

Checks compose isolation flags, separate images, digest pins, MinIO policy, and
PhoWhisper exclusion. When Docker is available it also runs
`docker compose config`.

### Out of scope (F02)

Kubernetes/HA, production TLS termination, Profile B GPU capacity claims.
