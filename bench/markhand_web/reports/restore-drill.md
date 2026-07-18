# P0-10 restore drill smoke

- Generated: `2026-07-18T19:37:30.656237Z`
- Mode: `offline-synthetic-recorded-spike`
- Seed: `20260718`
- Git commit: `eabb2e18cceb8cac925a6f4b4d7ab8e3c650a5d8`
- Dirty at harness start: `true`
- `targetMatch`: `false`
- `profileBDrGatePassed`: `false`
- `profileBDrBlocked`: `true`
- `p0_10_restore_smoke_closed`: `true`

## Scope

This is an offline synthetic restore drill using recorded spike lifecycle
evidence. It emits recovery-order markers and placeholder checksums only.

Explicit note: does NOT claim G0-DR Profile B pass evidence.

## Authority order

- `postgres`: authority for visibility, auth, chunks, jobs and index generation pointers.
- `minio`: durable originals and artifacts; not reconstructable from vectors.
- `qdrant`: rebuildable from PostgreSQL chunks and active index signature.

## Recovery order

| order | stage | marker | note |
|---:|---|---|---|
| 1 | `fence-writes` | `restore-fence-writes-9d3cded088e2` | Freeze API mutations and workers before recovery-point selection. |
| 2 | `restore-postgres` | `restore-restore-postgres-286596b26b17` | Restore PostgreSQL first; it is authority for visibility and auth. |
| 3 | `restore-minio` | `restore-restore-minio-f0e472e40137` | Restore MinIO originals and derived artifacts to the PG recovery point. |
| 4 | `restore-or-rebuild-qdrant` | `restore-restore-or-rebuild-qdrant-5b8afa1dd9ed` | Restore matching Qdrant snapshot or rebuild from PG chunks. |
| 5 | `reconcile` | `restore-reconcile-a25ff6244107` | Reconcile missing/orphan/stale objects and vectors before readiness. |
| 6 | `open-query-ready` | `restore-open-query-ready-d8f5d72703f1` | Open authorized read/search path without claiming full vector rebuild. |
| 7 | `complete-full-vector` | `restore-complete-full-vector-bad87fbb5482` | Finish active generation restore/rebuild and verification. |

## Store markers and checksum placeholders

| store | backup marker | restore marker | checksum placeholder |
|---|---|---|---|
| `minio` | `minio-backup-ceff1e7fbf27` | `minio-restore-640c4f168de8` | `42ad9ba9e53839df72cee78ca8b33ae11cb393e32b4207f0169c33c10877c845` |
| `postgres` | `postgres-backup-467ec63501d1` | `postgres-restore-963d4c772d5a` | `c3ad98ed2488323e1ca3a770874aa27b30fd20e1ca87008b00c4d684ee20ff05` |
| `qdrant` | `qdrant-backup-4889d9198c70` | `qdrant-restore-c1b3756e1d59` | `1ec2d669e292ac144a565b59b849f507db8975ea31877ddd9a5e0faa25dabad0` |

## Synthetic timings

| metric | synthetic value min | target min | synthetic within target | gate-valid pass |
|---|---:|---:|---|---|
| RPO | 5.381 | 15 | true | false |
| Query-ready RTO | 30.828 | 60 | true | false |
| Full-vector RTO | 153.647 | 240 | true | false |

Synthetic values are not gate-valid because `targetMatch=false`.

## Closure

| field | value |
|---|---|
| `recordedSpikeEvidenceLoaded` | `true` |
| `recoveryOrderMarkersEmitted` | `true` |
| `checksumPlaceholdersEmitted` | `true` |
| `timingsEmitted` | `true` |
| `honestFlagsSet` | `true` |

Dirty paths at harness start:
- `docs/adr/0002-version-aware-citations.md`
- `docs/adr/README.md`
- `plans/markhand-web/backlog/github-issues.json`
- `bench/markhand_web/scripts/run_phase0_gate.py`
- `bench/markhand_web/scripts/run_query_load.py`
- `bench/markhand_web/scripts/run_restore_drill.py`
- `docs/adr/0007-tenant-isolation-rls.md`
- `docs/adr/0010-auth-session-lifecycle.md`
- `docs/adr/0011-model-index-migration.md`
- `docs/adr/0012-backup-recovery-order.md`
- `docs/adr/phase0-decisions.json`
- `docs/markhand-web-risk-register.md`
- `docs/markhand-web-sla-targets.md`
