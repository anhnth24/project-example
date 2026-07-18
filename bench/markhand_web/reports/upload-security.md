# P0-09 upload security report

- Scope: `local-cpu policy/sandbox smoke; not Profile B malware scanner`
- Environment: `local-cpu-quality`
- Git clean at harness start: `false`
- `p0_09_closed`: `false`

## Closure

| field | value |
|---|---|
| `adversarialDispositionComplete` | `true` |
| `policyLinterPassed` | `true` |
| `sandboxProfilePassed` | `true` |
| `denialSimulationsPassed` | `true` |
| `licenseCheckerWouldPass` | `true` |
| `gitClean` | `false` |

## Adversarial fixture dispositions

- Passed: `10/10`
- Ratio: `1.0`

| attack | threat class | expected | actual | pass |
|---|---|---|---|---|
| `adv-corrupt-pdf` | `parser_corruption` | `reject` | `reject` | `true` |
| `adv-formula-csv` | `csv_formula` | `quarantine` | `quarantine` | `true` |
| `adv-html-pdf` | `mime_mismatch` | `reject` | `reject` | `true` |
| `adv-long-audio` | `audio_duration_limit` | `quarantine` | `quarantine` | `true` |
| `adv-malformed-docx` | `malformed_ooxml` | `reject` | `reject` | `true` |
| `adv-page-bomb` | `pdf_page_bomb` | `reject` | `reject` | `true` |
| `adv-prompt-html` | `prompt_injection` | `quarantine` | `quarantine` | `true` |
| `adv-spoof-pdf` | `extension_spoof` | `reject` | `reject` | `true` |
| `adv-traversal-docx` | `archive_path_traversal` | `reject` | `reject` | `true` |
| `adv-zip-bomb` | `archive_bomb` | `reject` | `reject` | `true` |

## Denial simulations

These are in-process policy checks, not container runtime execution.

| check | denied |
|---|---|
| `egressDenied` | `true` |
| `traversalDenied` | `true` |
| `forkBombDenied` | `true` |
| `timeoutDenied` | `true` |

## License checker

- Pass: `true`
- stdout: `{"metric": "approved_runtime_licenses", "value": 1.0}`

## Scope notes

- G0-SEC uses local-cpu-quality for policy/sandbox smoke evidence.
- No adversarial fixture is parsed or executed by this harness.
- Production upload workers must implement the sandbox profile before accepting user uploads.
- Does not claim: malware scanner coverage.
- Does not claim: container runtime enforcement.
- Does not claim: Profile B production hardening.

Dirty paths at harness start:
- `bench/markhand_web/gates.yaml`
- `plans/markhand-web/backlog/phase-0/issues/README.md`
- `bench/markhand_web/scripts/run_upload_security.py`
- `bench/markhand_web/security/`
- `docs/licenses/`
- `docs/markhand-web-model-license-inventory.md`
- `docs/markhand-web-runtime-license-inventory.json`
- `docs/markhand-web-runtime-license-requirements.json`
- `docs/markhand-web-upload-policy.md`
- `docs/markhand-web-upload-threat-model.md`
