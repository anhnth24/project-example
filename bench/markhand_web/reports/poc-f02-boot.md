# P1B-F02 POC Docker boot evidence

- Stamp (UTC): `20260724T031535Z`
- Generated: `2026-07-24T03:15:42.185115+00:00`
- Result: `FAIL`
- Passes: `34` / Fails: `13`
- Compose project: `markhand-poc`
- Git: `e3350d26dd258ff44b0cb78e1a507a2ee4c07bea`
- Compose file SHA256: `43ce324989252184f2df7c3ae572e71a3d84b07032ed5c262dc019147c7b3f9f`
- Docker: `29.1.3` / Compose: `2.40.3+ds1-0ubuntu1~24.04.1`
- Storage driver: `vfs`
- Standard-host qualification: `False`
- Raw artifacts: `bench/markhand_web/reports/phase-1b-gate/raw/f02-20260724T031535Z`

## Checks

- PASS: command docker
- PASS: command curl
- PASS: poc-health
- PASS: api user=10001:10001
- PASS: api read_only
- PASS: api cap_drop ALL
- PASS: api no-new-privileges
- PASS: worker-convert user=10001:10001
- PASS: worker-convert read_only
- PASS: worker-convert cap_drop ALL
- PASS: worker-convert no-new-privileges
- PASS: worker-index user=10001:10001
- PASS: worker-index read_only
- PASS: worker-index cap_drop ALL
- PASS: worker-index no-new-privileges
- PASS: worker-embedding user=10001:10001
- PASS: worker-embedding read_only
- PASS: worker-embedding cap_drop ALL
- PASS: worker-embedding no-new-privileges
- PASS: worker-convert on convert network (markhand-poc_convert)
- PASS: worker-convert not on edge/private
- PASS: convert --sandbox-preflight
- PASS: convert network Internal=true
- PASS: convert network external egress blocked (probe exit=1)
- PASS: api /health/ready
- PASS: api/worker images distinct (markhand-api:poc vs markhand-worker:poc)
- PASS: api image lacks fileconv converter
- PASS: worker image has fileconv + fileconv-worker
- PASS: worker excludes PhoWhisper model path
- PASS: native format smoke txt
- PASS: native format smoke html
- PASS: native format smoke csv
- PASS: native format smoke png
- PASS: native format smoke pdf (gold-004)
- FAIL: api memory limit missing/zero (HostConfig.Memory=0) — nested no-limit cannot Done
- FAIL: api cpu limit missing/zero (HostConfig.NanoCpus=0)
- FAIL: api pids limit missing/zero (HostConfig.PidsLimit=<nil>)
- FAIL: worker-convert memory limit missing/zero (HostConfig.Memory=0) — nested no-limit cannot Done
- FAIL: worker-convert cpu limit missing/zero (HostConfig.NanoCpus=0)
- FAIL: worker-convert pids limit missing/zero (HostConfig.PidsLimit=<nil>)
- FAIL: worker-index memory limit missing/zero (HostConfig.Memory=0) — nested no-limit cannot Done
- FAIL: worker-index cpu limit missing/zero (HostConfig.NanoCpus=0)
- FAIL: worker-index pids limit missing/zero (HostConfig.PidsLimit=<nil>)
- FAIL: worker-embedding memory limit missing/zero (HostConfig.Memory=0) — nested no-limit cannot Done
- FAIL: worker-embedding cpu limit missing/zero (HostConfig.NanoCpus=0)
- FAIL: worker-embedding pids limit missing/zero (HostConfig.PidsLimit=<nil>)
- FAIL: storage driver vfs is nested/nonstandard — F02 Done requires standard host (e.g. overlay2)
- NOTE: storageDriver=vfs
- NOTE: nolimit compose fallback active — cannot qualify F02 Done
- NOTE: worker-convert image lacks curl — using external probe image on convert network

## Evaluation blockers

- `nolimit_compose`
- `resource_limit_zero:api:memory`
- `resource_limit_zero:api:cpu`
- `resource_limit_zero:api:pids`
- `resource_limit_zero:worker-convert:memory`
- `resource_limit_zero:worker-convert:cpu`
- `resource_limit_zero:worker-convert:pids`
- `resource_limit_zero:worker-index:memory`
- `resource_limit_zero:worker-index:cpu`
- `resource_limit_zero:worker-index:pids`
- `resource_limit_zero:worker-embedding:memory`
- `resource_limit_zero:worker-embedding:cpu`
- `resource_limit_zero:worker-embedding:pids`
- `nonstandard_storage:vfs`
- `fail_count_nonzero`
- `passed_false`

## Commands

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
POC_EVIDENCE_RAW_DIR=bench/markhand_web/reports/phase-1b-gate/raw/f02-$(git rev-parse --short HEAD) \
  deploy/scripts/poc-boot-evidence.sh
# Hermetic validator:
deploy/scripts/poc-boot-evidence.sh --self-test
```

## Acceptance mapping

| Criterion | Evidence |
|---|---|
| Clean host boot | `poc-up.sh` + `poc-health` |
| API/worker images separated | distinct image refs + binary presence checks |
| Isolation UID/cap/read_only/no-new-privileges | sanitized `inspect-*.json` / `isolation-*.txt` |
| Convert no egress | convert `Internal=true` + executable network probe |
| Resource limits nonzero | `resourceLimits` memory/cpu/pids |
| Sandbox preflight | `sandbox-preflight.txt` |
| Native format smoke | `format-*.md` |
| O04 consumable metadata | `composeProject` + `imageIds` (+ digests when present) |

