# Runbooks

Phase F-08 and later add local setup, backup/restore, incident and release runbooks.
Each runbook must state prerequisites, commands, expected evidence and rollback.

Operational incident runbooks follow **detection → contain → recover → verify**,
with an explicit **rollback** section.

## Local / contributor

- [Local development](local-development.md)
- [Contributor setup](contributor-setup.md)
- [Knowledge index compatibility](knowledge-index-compatibility.md)

## P1B-O02 operations

| Runbook | Primary alerts |
|---|---|
| [Stuck / dead-letter jobs](stuck-dead-jobs.md) | `MarkhandQueueOldestAgeHigh`, `MarkhandQueueDepthWarning`, `MarkhandDeadLetterJobs` |
| [Converter outbreak](converter-outbreak.md) | `MarkhandConversionErrorOutbreak` |
| [Dependency outage](dependency-outage.md) | `MarkhandDependencyProbeDown`, embedding/retrieval/SLO burns |
| [Vector rebuild / drift](vector-rebuild.md) | `MarkhandDriftDetected`, `MarkhandReconcileErrors` |
| [Disk exhaustion](disk-exhaustion.md) | `MarkhandDiskLow` |
| [GLM fallback](glm-fallback.md) | `MarkhandGlmProviderErrors` (metric-only; GLM probe blocked) |
| [Key rotation](key-rotation.md) | `MarkhandAuthDenySpike` (count policy, not SLA ratio) |

Blocked (not loaded): filtered-query P99 SLO alert; GLM blackbox probe.
Artifacts/validation: `deploy/observability/` + `make check-observability` (pinned promtool).
Backup/restore runbooks are **P1B-O03** (out of O02 scope).
