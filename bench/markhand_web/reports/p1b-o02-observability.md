# P1B-O02 evidence — dashboards, alerts, runbooks (final-round fixes)

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
  SUCCESS: 12 rules found
Checking deploy/observability/prometheus/alert_rules.yml
  SUCCESS: 15 rules found

$ .tools/promtool test rules deploy/observability/prometheus/tests/alerts_test.yml
  SUCCESS

$ python3 scripts/check-observability-o02.py --self-test
P1B-O02 observability validation OK (15 active alerts, 4 blocked, 4 dashboards, promtool check/test OK)
Ran 7 tests … OK

$ cargo fmt --all -- --check
# exit 0

$ cargo metadata --locked --format-version 1 --no-deps
# exit 0

$ python3 scripts/check-dependency-policy.py
dependency policy passed

$ cargo test -p fileconv-server metrics::tests --lib
test result: ok. 5 passed; 0 failed …

$ cargo clippy -p fileconv-server --lib --no-deps -- -D warnings
# exit 0

$ make check-static
# exit 0

$ make check-observability
# exit 0
```

Machine report regenerated at `deploy/observability/evidence/validation-report.json`
(repo-relative paths, deterministic fields, `sort_keys=True`).

## Final-round fixes

1. Merged Compose: `REPO_ROOT` + `--project-directory`, root-relative binds, `up.sh`; validator resolves bind paths without Docker
2. OTEL wired on api/worker-index/worker-embedding/worker-convert; collector on `private`+`convert`; convert stays internal-only
3. Search availability: `or vector(0)` numerator + `and on()` traffic gate; 5xx-only fires; empty traffic does not; temporal fixture
4. `MarkhandReconcileErrors` blocked (unemitted `result=error`); live `MarkhandDriftDetected` retained; fake fixture/dashboard removed
5. Validator: O01+recording inventory, nonexistent-metric mutation, per-alert promtool fire+non-fire, tabletop stages/links, deterministic report
6. Stable `embedding` alias for mock + aiteamvn profiles; static profile/alias checks
7. Policy IDs: dead-letter event, probe failure/absence, drift/auth citations (no error-ratio misuse)
8. Disk: host root `mountpoint="/"` only; named-volume attribution explicitly unavailable/blocked
9. Runbooks: compose argv array/`poc_compose_init`, no `logs --tail=0`; Docker disk diagnostics without data deletion

## Non-claims / remaining blockers

- Docker unavailable in this VM — compose not live-booted; binds/network/OTEL matrix validated statically only
- Profile B SLO evidence still null / `targetMatch=false`
- Filtered-query P99 series not emitted — alert blocked
- GLM blackbox probe blocked (no configured endpoint)
- Named-volume disk attribution blocked (host node_exporter only)
- Reconcile `result=error` alert blocked (product emits success|drift)
- No supported admin requeue/reconcile CLI — contain/escalate gap documented
- Backup alerts remain O03
