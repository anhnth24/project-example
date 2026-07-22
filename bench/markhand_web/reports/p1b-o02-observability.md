# P1B-O02 evidence — dashboards, alerts, runbooks (round-1 fixes)

Status: **In Progress** (branch `cursor/implement-p1b-o02-5007`; not Review/Done).
`claims_real_outage`: **false**

## Commands and exact results

```bash
$ bash scripts/fetch-promtool.sh
# → .tools/promtool (Prometheus 2.55.1, sha256 verified)

$ .tools/promtool check rules \
    deploy/observability/prometheus/recording_rules.yml \
    deploy/observability/prometheus/alert_rules.yml
Checking deploy/observability/prometheus/recording_rules.yml
  SUCCESS: 13 rules found
Checking deploy/observability/prometheus/alert_rules.yml
  SUCCESS: 16 rules found

$ .tools/promtool test rules deploy/observability/prometheus/tests/alerts_test.yml
  SUCCESS

$ python3 scripts/check-observability-o02.py --self-test
P1B-O02 observability validation OK (16 active alerts, 2 blocked, 4 dashboards, promtool check/test OK)
Ran 5 tests … OK

$ cargo test -p fileconv-server metrics::tests --lib
test result: ok. 5 passed; 0 failed …
```

Machine report regenerated at `deploy/observability/evidence/validation-report.json`.

## Round-1 fixes

1. Explicit second histogram boundaries (`0.5`/`1.0`) in O01 + bucket export tests
2. Digest-pinned node-exporter + blackbox; HTTP vs TCP modules; real POC service names
3. Broad Alertmanager inhibition removed
4. Result enums: `http_error`/`invalid_response`/`outage`/`timeout`/`truncated`
5. Auth deny = count policy (O02-OPS-AUTH-DENY-COUNT), not SLA ratio
6. SLO/availability on `route=search` only; P99 filtered-query + GLM probe **blocked**
7. Dead-letter/reconcile use `increase()` without long `for` (promtool proves fire/resolve)
8. Validator invokes pinned promtool + mutation self-tests; evidence auto-regenerated
9. Runbooks cite `compose.poc.yml` / `poc-health.sh` / worker kinds; escalate where no admin CLI

## Non-claims / blockers

- Docker unavailable in this VM — compose not live-booted; pins/endpoints validated statically
- Profile B SLO evidence still null / `targetMatch=false`
- Filtered-query P99 series not emitted — alert blocked
- GLM blackbox probe blocked (no configured endpoint)
- No supported admin requeue/reconcile CLI — contain/escalate gap documented
- Backup alerts remain O03
