# Phase 1B alert rules

Prometheus rules live in `../prometheus/markhand-rules.yml`.

Label policy: only low-cardinality labels (`route`, `job_type`, `store`, `leg`,
`outcome`, `kind`). Never attach `org_id`, `user_id`, `document_id`, `request_id`,
or filenames to metrics/alerts.

Validation:

```bash
promtool check rules deploy/observability/prometheus/markhand-rules.yml
```

Tabletop evidence for each alert is recorded under
`bench/markhand_web/reports/phase-1b-gate/` when a live drill is run.
