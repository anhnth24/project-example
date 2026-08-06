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
# AppArmor hosts only, once per machine (see "Convert sandbox confinement"):
sudo apparmor_parser -r -W deploy/poc/apparmor-markhand-convert
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

### Sizing prerequisites (ingest throughput gate)

`G0-CAP-INGEST-THROUGHPUT-POC` (environment `poc-compose`) requires >= 300
documents/hour. Two measured data points:

| Host | CPU given to Docker | Ingest throughput | Gate (>= 300/h) |
|---|---:|---:|---|
| 24-core Ubuntu 22.04 | 10 CPU | 356 documents/hour | pass |
| 8 vCPU | 8 vCPU | 168 documents/hour | fail (expected) |

`compose.poc.yml`'s per-service CPU limits reserve roughly **8.5 CPU** in
total, so Docker needs meaningfully more than that available to have a chance
at the gate — the only passing run gave Docker 10 CPU.

Both measurements ran `COMPOSE_PROFILES=mock` (8-dimension deterministic
embedding): these are ingest-throughput numbers only, not a retrieval-quality
claim and not a Profile B capacity claim (the 1200 documents/hour peak gate
belongs to `on-prem-reference`, not `poc-compose`).

Evidence: [`bench/markhand_web/reports/phase-1b-gate/o05-soak.json`](../bench/markhand_web/reports/phase-1b-gate/o05-soak.json).

### Convert sandbox confinement

The convert worker builds a nested sandbox for every job: it unshares user,
network, mount and PID namespaces, remounts `/` as `MS_REC|MS_PRIVATE`, applies a
Landlock ruleset and per-job rlimits, then execs the converter. Two host policies
have to permit that:

| Layer | Profile | Why |
|---|---|---|
| seccomp | `poc/worker-sandbox-seccomp.json` | Docker's default allows `mount`/`unshare` only with `CAP_SYS_ADMIN`, which the container drops |
| AppArmor | `poc/apparmor-markhand-convert` | `docker-default` carries `deny mount,`, so the private remount fails |

On an AppArmor host the profile must be loaded before the stack starts:

```bash
sudo apparmor_parser -r -W deploy/poc/apparmor-markhand-convert
sudo aa-status | grep markhand-convert
```

`poc-compose.sh` resolves `MARKHAND_CONVERTER_APPARMOR_PROFILE` automatically:
`unconfined` when the host has no AppArmor (Docker Desktop, most nested VMs) and
`markhand-convert` otherwise, failing with the load command when AppArmor is
enabled but the profile is missing. Without it the worker crash-loops on
`converter worker initialization failed: sandbox error`.

### Isolation matrix

| Control | api | worker-convert | worker-index / embedding / delete / reconcile |
|---|---|---|---|
| non-root UID 10001 | yes | yes | yes |
| `read_only` rootfs | yes | yes | yes |
| `tmpfs` scratch | yes | yes (512m `/tmp`) | yes |
| `cap_drop: ALL` | yes | yes | yes |
| `no-new-privileges` | yes | yes | yes |
| mem/cpu/pids limits | yes | yes | yes |
| network | `edge`+`private` | **`convert` only (`internal: true`)** | `private` |
| seccomp | default | **custom default-deny sandbox profile** | default |

Convert path has no external egress: the `convert` network is `internal: true` and
only shares Postgres + MinIO. Docker's default seccomp profile blocks the
`mount`/`unshare` sequence used by the in-process convert sandbox. Convert uses
`deploy/poc/worker-sandbox-seccomp.json`, which retains a default-deny allowlist
and adds only those sandbox syscalls; `seccomp=unconfined` remains forbidden.
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
deploy/scripts/poc-boot-evidence.sh --self-test   # hermetic F02 evidence validator
# With Docker (standard host for Done):
deploy/scripts/poc-up.sh
POC_EVIDENCE_RAW_DIR=bench/markhand_web/reports/phase-1b-gate/raw/f02-$(git rev-parse --short HEAD) \
  deploy/scripts/poc-boot-evidence.sh
docker compose -f deploy/compose.poc.yml exec worker-convert \
  /usr/local/bin/fileconv-worker --sandbox-preflight
```

Boot evidence: [`bench/markhand_web/reports/poc-f02-boot.md`](../bench/markhand_web/reports/poc-f02-boot.md)
(+ machine JSON `poc-f02-boot.json` with `composeProject` / `imageIds` for O04).

Harness notes:

- Inspect artifacts are **allowlisted** (identity/image/user/readOnly/securityOpt/
  capDrop/limits/networks/health/status) — never raw `Config.Env`.
- Convert egress requires `Internal=true` **and** an executable probe on the convert
  network (pinned alpine from `images.lock.json`); missing probe tooling fails.
- Nonzero memory/CPU/pids limits are required for `passed=true`. Nested hosts that
  strip limits (`poc-compose.sh` nolimit fallback) or use `vfs` may boot for
  debugging but **cannot** qualify F02 Done.

On nested hosts where cgroup v2 is stuck in `threaded` mode, `poc-up.sh`
auto-strips `mem_limit`/`cpus`/`pids_limit` for boot only; the canonical
`compose.poc.yml` still declares those limits for normal Docker hosts.

### Out of scope (F02)

Kubernetes/HA, production TLS termination, Profile B GPU capacity claims.

## Web SPA static serving (P2-16)

`fileconv-server` can serve the built web SPA (`web/dist`) directly — hashed,
long-cached assets under `/assets/*`, history-fallback for UI routes, and
strict security headers on both. See `crates/server/src/spa.rs` for the
implementation and its own module docs for the exact CSP/cache-control
contract.

- Build the SPA first: `pnpm --dir web build` → `web/dist`.
- Point the server at it with `MARKHAND_WEB_DIST_DIR=/path/to/web/dist`
  (absolute path recommended for containers). Unset, the server falls back to
  `./web/dist` relative to its CWD, and if neither resolves it simply serves
  the API alone — **serving the SPA is optional, never required to boot**.
- `deploy/Dockerfile.server` and `compose.poc.yml` do **not** currently build
  or copy `web/dist` into the API image/container — that would add a
  Node/pnpm build stage to a pipeline whose base images and evidence are
  digest-pinned (`poc/images.lock.json`) for the F02 gate, which is a
  separate decision this change deliberately did not make unasked. Until
  that ADR lands, ship the SPA either behind a separate static host/CDN
  pointed at the same API origin, or bind-mount a locally built `web/dist`
  into the API container and set `MARKHAND_WEB_DIST_DIR` accordingly.
- HSTS is intentionally **not** set by the application — it is a
  reverse-proxy/TLS-terminator concern in production (the app has no
  certificate and cannot know if the edge actually terminates HTTPS).

<!-- ci: nudge after digest pins -->

## Real web E2E fixture + artifacts (P2-20)

Dev/CI-only harness for the Playwright `real` project. It creates a run-scoped
fixture against the local Compose stack, runs `web-e2e-real.sh`, then stages a
**sanitized** result manifest. Do not use these tools against production.

### Local command

Prerequisites: dev Compose stack up (`make dev-up` / `DEV_STACK_MODE=full bash
deploy/scripts/dev-stack-ci.sh`), Node/pnpm installed, Playwright Chromium
installed, and `deploy/dev/.env` present (or let the orchestrator call
`init-dev-env.sh`).

```bash
# Full CI path (Compose + real Playwright + fixture teardown):
DEV_STACK_MODE=full bash deploy/scripts/dev-stack-ci.sh

# Or invoke the orchestrator when the stack is already up:
bash deploy/scripts/web-e2e-real.sh
```

Hermetic unit coverage (no Docker):

```bash
python3 deploy/scripts/test_web_e2e_real_fixture.py
python3 deploy/scripts/test_web_e2e_real_orchestration.py
python3 deploy/scripts/test_web_e2e_real_artifacts.py
```

### Fixture CLI contract

`deploy/scripts/web_e2e_real_fixture.py`:

| Command | Required flags | Notes |
|---|---|---|
| `setup` | `--run-id`, `--manifest-out`, `--credentials-out` | Creates run-namespaced org/users/collection/docs; writes sanitized manifest + mode-0600 credentials |
| `cleanup` | `--run-id`, `--manifest`, `--credentials`, `--api-base`, `--timeout-secs` | Bounded teardown of run-attributable rows/objects/vectors |
| `verify-clean` | `--run-id`, `--manifest` | Fail-closed leak check after cleanup |

Runtime credentials are never the fixed seed account. Fixture tooling refuses
`MARKHAND_PROFILE=prod` (and other production profiles) before any DB write.

### Artifact CLI contract

`deploy/scripts/web_e2e_real_artifacts.py`:

| Command | Required flags | Notes |
|---|---|---|
| `write` | `--results`, `--fixture`, `--out`, `--teardown` | Extracts only `{title, outcome, durationMs}` per scenario; records git/tool versions, fixture checksum, skipped count, teardown, companion checksums |
| `validate` | `--manifest`, `--artifact-dir` | Fail-closed: missing scenarios, `skippedCount != 0`, teardown ≠ `ok`, checksum drift, inventory mismatch, secret/content canaries |

Optional canary env (comma- or newline-separated):

- `WEB_E2E_REAL_SECRET_CANARIES` — tokens/passwords/capability strings that must not appear in staged artifacts
- `WEB_E2E_REAL_CONTENT_CANARIES` — document/preview body markers that must not be retained

### Output location

| Path | Contents |
|---|---|
| `$WEB_E2E_REAL_ARTIFACT_DIR` (default: temp under `/tmp/markhand-web-e2e-real-artifacts.*`) | Staged sanitized `manifest.json` + allowlisted companions |
| `$WEB_E2E_REAL_RUNTIME_DIR` (default: temp under `/tmp/markhand-web-e2e-real-runtime.*`) | Fixture manifest, credentials (0600), raw Playwright JSON — **outside** the uploaded artifact dir |

Override either directory via env when CI needs a stable collection path.

### Production refusal

Fixture setup/cleanup/verify-clean exit non-zero when `MARKHAND_PROFILE` is
production. There is no production test route, auth bypass, or fixed seed
authority path in this harness.

### Sanitization warning

Retained artifacts must never include document bodies, prompts, PII, tokens,
keys, signed URLs, cookies, or passwords. Playwright traces/screenshots are
restricted; log dumps go through `redact_secrets.py` fail-closed. Secret or
content canary matches fail the job. Reviewers should treat any raw credential
or preview body in CI uploads as a security finding.

