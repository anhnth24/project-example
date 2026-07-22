# P1B-O02 evidence — dashboards, alerts, runbooks (verification blockers)

Status: **In Progress** (branch `cursor/implement-p1b-o02-5007`; not Review/Done).
`claims_real_outage`: **false**

## Commands and exact results

```bash
$ bash scripts/fetch-promtool.sh
# → .tools/promtool (Prometheus 2.55.1, sha256 verified)

$ git diff --check
# exit 0

$ .tools/promtool check rules \
    deploy/observability/prometheus/recording_rules.yml \
    deploy/observability/prometheus/alert_rules.yml
Checking deploy/observability/prometheus/recording_rules.yml
  SUCCESS: 12 rules found
Checking deploy/observability/prometheus/alert_rules.yml
  SUCCESS: 15 rules found

$ .tools/promtool test rules deploy/observability/prometheus/tests/alerts_test.yml
  SUCCESS

$ python3 scripts/check-observability-o02.py --self-test
P1B-O02 observability validation OK (15 active alerts, 4 blocked, 4 dashboards, promtool check/test OK)
Ran 15 tests … OK

$ make check-static
# exit 0

$ make check-observability
# exit 0
```

Machine report regenerated at `deploy/observability/evidence/validation-report.json`
(v4; repo-relative paths; no `dockerAvailable`/host notes — runtime printed to stdout only).

## Verification-blocker fixes

1. Exact PromQL metric inventory (any prefix) vs O01+infra raw / derived recording inventories; unknown prefixed+unprefixed mutations fail
2. Evidence report deterministic (no `dockerAvailable`/dynamic notes); tabletop requires unique semantic `promtool_case`↔alert mapping + mutations
3. Blocked reconcile cites `O02-OPS-RECONCILE-ERROR-EVENT-BLOCKED` (>0), not live `O02-OPS-DRIFT-COUNT`
4. Key-rotation keeps `POC_WITH_OBSERVABILITY=1` and verifies OTLP env after API recreate
5. Disk runbook: `docker buildx du`, `docker inspect --size` before `.SizeRootFs`; diagnostics non-destructive by default

## Non-claims / remaining blockers

- Docker unavailable in this VM — compose not live-booted; binds/network/OTEL matrix validated statically only
- Profile B SLO evidence still null / `targetMatch=false`
- Filtered-query P99 series not emitted — alert blocked
- GLM blackbox probe blocked (no configured endpoint)
- Named-volume disk attribution blocked (host node_exporter only)
- Reconcile `result=error` alert blocked (product emits success|drift; policy `O02-OPS-RECONCILE-ERROR-EVENT-BLOCKED`)
- No supported admin requeue/reconcile CLI — contain/escalate gap documented
- Backup alerts remain O03
