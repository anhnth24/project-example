# P1B-O02 evidence — dashboards, alerts, runbooks

Status: **In Progress** (implementation on `cursor/implement-p1b-o02-5007`; not Review/Done).  
Kind: static rule validation + synthetic fault fixtures + tabletop walkthrough.  
`claims_real_outage`: **false**

## Commands and exact results

```bash
$ python3 scripts/check-observability-o02.py
P1B-O02 observability validation OK (15 alerts, 4 dashboards, 15 fixtures, 7 runbooks); synthetic only — no live outage claimed

$ python3 scripts/check-observability-o02.py --self-test \
    --json-report deploy/observability/evidence/validation-report.json
test_alert_inventory_size ... ok
test_each_fixture_has_firing_and_resolved ... ok
test_gates_registry_approved ... ok
test_no_validation_errors ... ok
Ran 4 tests in ~0.03s
OK
```

Machine report: `deploy/observability/evidence/validation-report.json`:

```json
{
  "version": 1,
  "issue": "P1B-O02",
  "ok": true,
  "alertCount": 15,
  "dashboardCount": 4,
  "fixtureCount": 15,
  "runbookCount": 7,
  "errors": [],
  "claims_real_outage": false
}
```

## Threshold citations

| Threshold | Value | Source |
|---|---:|---|
| Query P95 | 500 ms | `bench/markhand_web/gates.yaml` `#G0-SLO-QUERY-P95` |
| Query P99 | 1000 ms | `bench/markhand_web/gates.yaml` `#G0-SLO-QUERY-P99` |
| Queue age | 120 min | `docs/markhand-web-sla-targets.md` |
| Queue depth warning | 600 | Derived from `#G0-CAP-INGEST-THROUGHPUT` 1200 docs/h × 0.5 h |
| Disk free ratio | ≥ 0.30 | `bench/markhand_web/workload-profile.yaml` `hardware.headroomPercent.disk` |
| Availability | ≥ 99.5% | `docs/markhand-web-sla-targets.md` |

## Deliverables

- Prometheus recording + alert rules against O01 metric/label schema
- Grafana provisioning + dashboards with bounded custom variables only
- OTel collector Prometheus export config
- Alertmanager example routing (placeholder secrets only)
- 15 synthetic alert fixtures (each firing + resolved)
- 7 runbooks with detection → contain → recover → verify (+ rollback)
- Tabletop evidence JSON (not a live game day)

## Explicit non-claims

- No live dependency outage, disk fill, or credential leak was exercised.
- No Profile B `targetMatch=true` SLO measurement claimed.
- Backup-failure alerts deferred to P1B-O03 (no backup metrics in O01).
- No Rust product telemetry changes in this issue.
