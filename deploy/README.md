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
| [`poc/images.lock.json`](poc/images.lock.json) | Digest/hash + index-signature pins |

### Clean-host boot

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh      # build images, compose up, health
deploy/scripts/poc-health.sh  # readiness + worker state
```

Defaults:

- `COMPOSE_PROFILES=mock` — deterministic 8-d L2-normalized embedding, no GPU/HF download
- API on `http://127.0.0.1:8788` (`/api/v1/health/ready`)
- Host ports are loopback-only and offset from `deploy/dev` to avoid clashes

AITeamVN CPU embedding (not GLM):

```bash
# edit deploy/.env: COMPOSE_PROFILES=aiteamvn + AITeamVN signature/URL block
# signature must be dc6f6af4… (see images.lock.json / print-index-signature.py)
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
| seccomp | default | **`unconfined` (sandbox preflight)** | default |

Convert path has no external egress: the `convert` network is `internal: true` and
only shares Postgres + MinIO. Default Docker seccomp blocks landlock/unshare
sequences used by the in-process convert sandbox, so convert alone sets
`seccomp=unconfined` while keeping `cap_drop: ALL` and no-egress networking.
Landlock allowlists PDFium (`/opt/pdfium`) and Tesseract tessdata paths.

### Index signatures

```bash
python deploy/scripts/print-index-signature.py \
  --base-url http://mock-embedding:8080/v1 \
  --model markhand-mock --revision poc-local --dimensions 8
# → 72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874

python deploy/scripts/print-index-signature.py \
  --base-url http://embedding-cpu:8080/v1 \
  --model AITeamVN/Vietnamese_Embedding \
  --revision dea33aa1ab339f38d66ae0a40e6c40e0a9249568 --dimensions 1024
# → dc6f6af4922063ae815fa3c84e17491b059d7c323fb8320d827f34386a038f86
```

### Secrets and models

- MinIO root credentials are init-only; app/workers use `MARKHAND_MINIO_ACCESS_KEY`
  scoped by the bucket-aware [`poc/minio-app-policy.json.tmpl`](poc/minio-app-policy.json.tmpl)
  (init fails closed if policy install/attach fails).
- PhoWhisper and other unresolved-license models are **not** bundled.
- PDFium is pinned to `chromium/7906` with sha256 verification (not `releases/latest`).
- Do not commit `deploy/.env`.

### Validation

```bash
deploy/scripts/poc-isolation-smoke.sh   # offline; no GPU required
# With Docker:
deploy/scripts/poc-up.sh
deploy/scripts/poc-boot-evidence.sh
docker compose -f deploy/compose.poc.yml exec worker-convert \
  /usr/local/bin/fileconv-worker --sandbox-preflight
```

Boot evidence: [`bench/markhand_web/reports/poc-f02-boot.md`](../bench/markhand_web/reports/poc-f02-boot.md).

On nested hosts where cgroup v2 is stuck in `threaded` mode, `poc-up.sh`
auto-strips `mem_limit`/`cpus`/`pids_limit` for boot only; the canonical
`compose.poc.yml` still declares those limits for normal Docker hosts.

### Out of scope (F02)

Kubernetes/HA, production TLS termination, Profile B GPU capacity claims.

<!-- ci: nudge after digest pins -->

