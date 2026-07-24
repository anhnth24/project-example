# P1B-O04 vertical-slice / security release suite

- Status: `fail`
- Issue: `P1B-O04`
- MARKHAND_E2E: `True`
- Architecture: `in_process_workers_against_poc_services` (apiHttpExercised=False)
- Expected formats (from phase1b-mixed.yaml): `csv, docx, html, pdf, png, pptx, txt, xlsx`
- Formats observed: `csv, docx, html, pdf, png, pptx, txt, xlsx`
- Git: `e3350d26dd258ff44b0cb78e1a507a2ee4c07bea`
- F02 boot passed: `False`
- Raw: `/workspace/bench/markhand_web/reports/phase-1b-gate/raw/o04-e3350d2`

## Suites

- `vertical_slice_formats`: passed=True exit=0 testsRun=1 skipped=False ignored=False
- `unauthorized_cross_tenant`: passed=True exit=0 testsRun=1 skipped=False ignored=False
- `suspend_membership_delete_deny`: passed=True exit=0 testsRun=1 skipped=False ignored=False
- `adversarial_upload`: passed=True exit=0 testsRun=19 skipped=False ignored=False
- `worker_kill_replay`: passed=True exit=0 testsRun=2 skipped=False ignored=False

## Blockers

- f02_boot_not_passed

## Notes

Live run did not meet O04 pass gates; see blockers. Architecture remains in-process workers against service endpoints.
