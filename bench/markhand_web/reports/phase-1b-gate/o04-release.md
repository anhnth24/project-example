# P1B-O04 vertical-slice / security release suite

- Status: `not_run`
- Issue: `P1B-O04`
- MARKHAND_E2E: `False`
- Architecture: `in_process_workers_against_poc_services` (apiHttpExercised=False)
- Expected formats (from phase1b-mixed.yaml): `csv, docx, html, pdf, png, pptx, txt, xlsx`
- Formats observed: `(none)`
- Git: `57a31ea0709690467478ea655be518aee42cbe03`
- F02 boot passed: `True`
- Raw: `/workspace/bench/markhand_web/reports/phase-1b-gate/raw/o04-57a31ea`

## Suites

- `vertical_slice_formats`: passed=False exit=None testsRun=0 skipped=False ignored=False
- `unauthorized_cross_tenant`: passed=False exit=None testsRun=0 skipped=False ignored=False
- `suspend_membership_delete_deny`: passed=False exit=None testsRun=0 skipped=False ignored=False
- `adversarial_upload`: passed=False exit=None testsRun=0 skipped=False ignored=False
- `worker_kill_replay`: passed=False exit=None testsRun=0 skipped=False ignored=False

## Blockers

- MARKHAND_E2E!=1

## Notes

Harness complete; live release suite not opted in. Set MARKHAND_E2E=1 with POC PG/MinIO/Qdrant + built fileconv + F02 poc-f02-boot.json passed=true (with composeProject/imageIds), then re-run. Suites are in-process workers against service endpoints — not Compose API HTTP.
