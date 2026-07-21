# P1B-F02 POC Docker boot evidence

- Stamp (UTC): `2026-07-21T14:45:00Z`
- Result: `PASS`
- Host notes: nested Cloud VM required `vfs` storage + cgroup memory-limit strip for
  first boot (`threaded` cgroup); subsequent stack reached healthy with fixed
  `minio-init` (tmpfs `MC_CONFIG_DIR`, bash policy expand — no `sed`/`grep`).

## Checks

- PASS: images built (`markhand-api:poc`, `markhand-worker:poc`)
- PASS: `minio-init` exit 0 — bucket + narrow app policy/user
- PASS: API `/api/v1/health/live` → 200 `{"status":"ok"}`
- PASS: API `/api/v1/health/ready` → 200 `{"status":"ok"}`
- PASS: workers running; `worker-convert` health=healthy
- PASS: convert `--sandbox-preflight` → `sandbox preflight ok`
- PASS: isolation api/convert — UID `10001:10001`, `read_only`, `cap_drop ALL`,
  `no-new-privileges`
- PASS: convert network `Internal=true` (no egress)
- PASS: native format smoke via worker image — txt/html/csv
- PASS: offline `deploy/scripts/poc-isolation-smoke.sh`

## Runtime snapshot

| Service | Status |
|---|---|
| postgres | healthy |
| qdrant | up |
| minio | up |
| minio-init | exited 0 |
| mock-embedding | healthy |
| api | healthy |
| worker-convert | healthy |
| worker-index | up |
| worker-embedding | up |

## Commands

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
deploy/scripts/poc-health.sh
deploy/scripts/poc-boot-evidence.sh
# convert sandbox:
docker compose -f deploy/compose.poc.yml exec worker-convert \
  /usr/local/bin/fileconv-worker --sandbox-preflight
```

## Acceptance mapping

| Criterion | Evidence |
|---|---|
| Clean host boot | compose up + api ready + workers up |
| API/worker images separated | distinct tags; api lacks `fileconv`, worker has both |
| Isolation UID/cap/read_only/no-new-privileges | docker inspect |
| Convert no egress | convert network Internal=true |
| Sandbox preflight | `sandbox preflight ok` |
| Native format smoke | txt/html/csv markdown output |
| Narrow MinIO creds | minio-init policy attach to `markhand_app` |
| No PhoWhisper | worker Dockerfile guard + offline smoke |
