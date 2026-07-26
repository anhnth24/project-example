# P1B-F02 POC Docker boot evidence

- Stamp (UTC): `20260726T121843Z`
- Generated: `2026-07-26T12:19:59.514660+00:00`
- Result: `PASS`
- Passes: `81` / Fails: `0`
- Compose project: `markhand-poc-f02-20260726t121843z-1815269-17292`
- Git: `f4f33cd1b476e07d69594dec269002f1159f1b70`
- Dirty worktree: `False`
- Compose file SHA256: `79b1f57a3d5428adae832458d0e55fb01ee9568cd6a634e52640b3d26850f608`
- Compose blob SHA256: `79b1f57a3d5428adae832458d0e55fb01ee9568cd6a634e52640b3d26850f608`
- Docker: `29.6.2` / Compose: `5.3.1`
- Storage driver: `overlayfs`
- Standard-host qualification: `True`
- Raw artifacts: `.artifacts/markhand_web/reports/phase-1b-gate/raw/f02-20260726T121843Z-1815269-17292`
- Raw manifest: `.artifacts/markhand_web/reports/phase-1b-gate/raw/f02-20260726T121843Z-1815269-17292/manifest.json`

## Checks

- PASS: command docker
- PASS: command curl
- PASS: clean project boot measured (27.861307283863425s)
- PASS: poc-health
- PASS: postgres memory limit=1073741824
- PASS: postgres cpu limit nanoCpus=1000000000
- PASS: postgres pids limit=256
- PASS: qdrant memory limit=1073741824
- PASS: qdrant cpu limit nanoCpus=1000000000
- PASS: qdrant pids limit=256
- PASS: minio memory limit=536870912
- PASS: minio cpu limit nanoCpus=1000000000
- PASS: minio pids limit=256
- PASS: mock-embedding memory limit=268435456
- PASS: mock-embedding cpu limit nanoCpus=500000000
- PASS: mock-embedding pids limit=64
- PASS: api user=10001:10001
- PASS: api read_only
- PASS: api cap_drop ALL
- PASS: api no-new-privileges
- PASS: api memory limit=536870912
- PASS: api cpu limit nanoCpus=1000000000
- PASS: api pids limit=256
- PASS: worker-convert user=10001:10001
- PASS: worker-convert read_only
- PASS: worker-convert cap_drop ALL
- PASS: worker-convert no-new-privileges
- PASS: worker-convert memory limit=805306368
- PASS: worker-convert cpu limit nanoCpus=1000000000
- PASS: worker-convert pids limit=512
- PASS: worker-index user=10001:10001
- PASS: worker-index read_only
- PASS: worker-index cap_drop ALL
- PASS: worker-index no-new-privileges
- PASS: worker-index memory limit=805306368
- PASS: worker-index cpu limit nanoCpus=1000000000
- PASS: worker-index pids limit=512
- PASS: worker-embedding user=10001:10001
- PASS: worker-embedding read_only
- PASS: worker-embedding cap_drop ALL
- PASS: worker-embedding no-new-privileges
- PASS: worker-embedding memory limit=805306368
- PASS: worker-embedding cpu limit nanoCpus=1000000000
- PASS: worker-embedding pids limit=512
- PASS: worker-delete user=10001:10001
- PASS: worker-delete read_only
- PASS: worker-delete cap_drop ALL
- PASS: worker-delete no-new-privileges
- PASS: worker-delete memory limit=536870912
- PASS: worker-delete cpu limit nanoCpus=500000000
- PASS: worker-delete pids limit=512
- PASS: worker-reconcile user=10001:10001
- PASS: worker-reconcile read_only
- PASS: worker-reconcile cap_drop ALL
- PASS: worker-reconcile no-new-privileges
- PASS: worker-reconcile memory limit=536870912
- PASS: worker-reconcile cpu limit nanoCpus=500000000
- PASS: worker-reconcile pids limit=512
- PASS: storage driver overlayfs
- PASS: worker-convert exactly on convert network (markhand-poc-f02-20260726t121843z-1815269-17292_convert)
- PASS: convert --sandbox-preflight
- PASS: convert network Internal=true
- PASS: convert worker namespace external route blocked (probe exit=20)
- PASS: api /health/ready
- PASS: MinIO app credential can list configured bucket
- PASS: MinIO app credential admin user list authorization denied
- PASS: MinIO app credential cross-bucket access authorization denied
- PASS: qdrant-init exited 0
- PASS: Qdrant collection config verified
- PASS: api/worker images distinct (markhand-api:poc vs markhand-worker:poc)
- PASS: api image lacks fileconv converter
- PASS: worker image has fileconv + fileconv-worker
- PASS: worker excludes PhoWhisper model path
- PASS: native format smoke csv content
- PASS: native format smoke docx content
- PASS: native format smoke html content
- PASS: native format smoke pdf content
- PASS: native format smoke png OCR content
- PASS: native format smoke pptx content
- PASS: native format smoke txt content
- PASS: native format smoke xlsx content
- NOTE: storageDriver=overlayfs
- NOTE: worker-convert image lacks curl — using external probe image on convert network

## Commands

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
deploy/scripts/poc-boot-evidence.sh
MARKHAND_F02_BOOT_REPORT=.artifacts/markhand_web/reports/poc-f02-boot.json \
  deploy/scripts/o04-release-suite.sh
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

