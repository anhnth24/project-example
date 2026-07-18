# P0-08 ingest capacity report

- Generated: `2026-07-18T19:18:54.468310Z`
- Mode: `local-cpu-converter-smoke`
- Measurement scope: `local-cpu; not Profile B`
- Git commit: `093c0a357b4537193b128b67cf76506449f3905b`
- Dirty at harness start: `false`
- `targetMatch`: `false`
- `profileBGatePassed`: `false`
- `productionCapacityBlocked`: `true`
- `p0_08_closed`: `true`

## Scope

This is local-cpu converter smoke evidence. It does **not** claim Profile B
headroom or a `G0-CAP-INGEST-THROUGHPUT` pass.

Explicit note: does NOT claim Profile B G0-CAP-INGEST-THROUGHPUT pass evidence.

## Workload coverage

- Manifest documents: `31`
- Selected documents: `31`
- Formats covered: `audio, csv, docx, html, image_ocr, pdf_native, pdf_scan, pptx, text_legacy, xlsx`
- Conversion-only policy: skip only when a non-conversionOnly fixture covers the same format.

## Runs

| run | workers | docs ok/error | wall s | docs/hour | pages/hour | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| singleWorker | 1 | 31/0 | 5.396 | 20680.226 | 19346.018 | 223.672 |
| concurrent2 | 2 | 31/0 | 2.809 | 39729.296 | 37166.115 | 219.059 |

## Per-format sizing from concurrent run

| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |
|---|---:|---:|---:|---:|---:|---:|---:|
| audio | 2 | 2 | 0 | 3915.81 | 934.606 | 217.355 | 0 |
| csv | 3 | 3 | 0 | 173007.609 | 20.949 | 1.574 | 3 |
| docx | 7 | 7 | 0 | 172962.882 | 20.993 | 1.402 | 7 |
| html | 3 | 3 | 0 | 172089.614 | 21.005 | 1.332 | 3 |
| image_ocr | 3 | 3 | 0 | 8105.747 | 451.287 | 111.648 | 3 |
| pdf_native | 3 | 3 | 0 | 169733.926 | 21.398 | 1.676 | 3 |
| pdf_scan | 2 | 2 | 0 | 4507.307 | 800.091 | 219.059 | 2 |
| pptx | 2 | 2 | 0 | 172943.889 | 20.917 | 1.16 | 2 |
| text_legacy | 3 | 3 | 0 | 172422.051 | 21.025 | 0.848 | 3 |
| xlsx | 3 | 3 | 0 | 172243.31 | 20.959 | 1.395 | 3 |

## Headroom estimate

- Target: `1200.0` docs/hour.
- Required for 30% headroom: `1714.286` docs/hour.
- Measured successful local-cpu throughput: `39729.296` docs/hour.
- Gate-valid effective capacity: `39729.296` docs/hour.
- Estimated headroom: `96.98`%.
- Meets 30% headroom on this runner: `true`.

## Queue age simulation

These rows are deterministic simulations. If any workload format failed,
the gate-valid service rate is set to zero instead of extrapolating from
partial successes.

- Measured service rate: `39729.296` docs/hour.
- Effective simulated service rate: `39729.296` docs/hour.
- Capacity valid for gate: `true`.
- Note: all workload documents succeeded.

| scenario | arrival docs/hour | final queue docs | oldest age min | stable |
|---|---:|---:|---:|---|
| normal1x | 300.0 | 0.0 | 0.0 | true |
| recovery2xNormal | 600.0 | 0.0 | 0.0 | true |
| peakGateLoad | 1200.0 | 0.0 | 0.0 | true |

## Closure

| field | value |
|---|---|
| `harnessCompleted` | `true` |
| `reportWritten` | `true` |
| `gitClean` | `true` |
| `honestFlagsSet` | `true` |
| `allDocumentsSucceeded` | `true` |
