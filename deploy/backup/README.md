# Phase 1B backup / restore (P1B-O03)

Blue/green capture and **restore-green only**. Scripts wrap `lib/pipeline.py`.
Promote/cutover is **disabled** until the API consumes durable routing and an
independent target-state reconcile attestation exists (no false traffic switch).

## Required auth policy

- `MARKHAND_BACKUP_SIGNING_KEY` (≥32 bytes) — **required** (env only; never argv)
- `MARKHAND_BACKUP_KEY_ID` — recorded in `trustedBoundary.keyId`
- Manifest HMAC verified on **raw bytes before JSON parse/use**
- JSON Schema Draft-07 enforced (`additionalProperties: false`); artifact paths
  use safe `is_relative_to` open; **symlinks refused**
- Schema version const `3` (downgrade refused)

## Encryption

- Prefer `MARKHAND_BACKUP_ENCRYPTED=1` + `.markhand-backup-encrypted` marker next
  to the destination parent
- Or explicit POC policy:
  `MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY=explicit_poc_tmp_only` (path under `tmp/`)
- Otherwise capture **fails closed**

## Identity / immutable allowlists

Postgres identity: `pg_control_system.system_identifier` + `current_database()`.
Restore requires **mandatory** frozen allowlists:
`MARKHAND_GREEN_ALLOWLIST_JSON`, `MARKHAND_GREEN_MINIO_ALLOWLIST_JSON`,
`MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON`. Green PG/MinIO/Qdrant targets must be
**absent**; restore performs exclusive create (no Qdrant DELETE, no
`--ignore-existing`). Existing allowlisted targets fail **before any mutation**.
Restore issues a creation token; cleanup may delete only token-owned resources.
Endpoint/bucket/collection aliases of blue are refused. Backup dirs use external
`mktemp` (never workspace `tmp/markhand-backup-o03`).

## Consistency / write gate

- Session-scoped advisory lock held for the **entire** capture (`PgSession`)
- Strict job drain: wait then **fail without mutating jobs** (never cancel)
- Consistency requires the central API write-gate contract
  (`middleware/write_gate.rs` `mutation_write_gate`, advisory lock `7303003`,
  RAII `acquire_background_mutation_guard` around quota/ask maintenance and
  ask-stream append); otherwise refuse
  (`MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE=1`, default). Isolation drills may
  set `=0` and record watermark `fence_drain_lock_app_write_gate_absent`.
  Detector: `deploy/backup/lib/write_gate_contract.py`.
- Scripts perform **no DDL**; `ops_fences` must already exist. `ops_routing` is
  **not** retained (no migration) while promote is disabled

## MinIO / Qdrant

Versioning Enabled. Chronological per-key inventory including checked delete
markers; semantic inventory equality + byte-for-byte object verify on green.
Qdrant verify uses full pagination and config/payload reference SHA256.

## Promote

`restore.sh` never cutovers. `pipeline.py promote|cutover` fails with
`PROMOTE_DISABLED_...`.

## Secrets

- PG via `PGPASSFILE` / env — password never on argv
- MinIO via `MC_HOST_*` env only
- `umask 077` on wrappers and artifact writes

## Drill

`deploy/scripts/o03-bluegreen-restore-drill.sh` invokes real `backup.sh` /
`restore.sh`. Evidence is metadata/logs/synthetic only (no dumps, no credential
wrappers). Report regenerable via `deploy/scripts/o03-report-from-raw.py`.
