# Phase 1B operations runbooks

Each runbook follows **detect → contain → recover → verify** with exact alert
names, PromQL, and safe Compose commands. Never paste secrets, JWTs, document
bodies, prompts, or embeddings into tickets.

| Scenario | Alert(s) | Runbook |
|---|---|---|
| API latency burn | `MarkhandApiLatencyBurn` | [api-latency.md](api-latency.md) |
| Stuck / dead-letter / quota | `MarkhandQueueGrowth`, `MarkhandQueueAgeHigh`, `MarkhandQuotaExceeded` | [stuck-jobs.md](stuck-jobs.md) |
| Converter outbreak | `MarkhandConversionFailures` | [converter-outbreak.md](converter-outbreak.md) |
| Dependency / embed outage | `MarkhandDependencyDown`, `MarkhandEmbeddingErrors` | [dependency-outage.md](dependency-outage.md) |
| GLM / chat fallback | `MarkhandProviderErrors` | [glm-fallback.md](glm-fallback.md) |
| Disk exhaustion | `MarkhandDiskLow` | [disk-exhaustion.md](disk-exhaustion.md) |
| Reconcile drift | `MarkhandReconcileDrift` | [reconcile-drift.md](reconcile-drift.md) |
| Backup / restore | `MarkhandBackupStale` | [backup-restore.md](backup-restore.md) |
| Vector rebuild | (follows Qdrant/embed loss) | [vector-rebuild.md](vector-rebuild.md) |
| Key rotation | (auth/leak procedures) | [key-rotation.md](key-rotation.md) |

Rule + tabletop evidence:

```bash
bash deploy/scripts/o02-alert-tabletop.sh
```

Log capture must pipe through `python3 deploy/scripts/redact_secrets.py` (or use allowlisted runbooks without raw docker logs).
