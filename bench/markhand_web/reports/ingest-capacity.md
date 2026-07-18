# P0-08 ingest capacity report

- Generated: `2026-07-18T19:17:28.182762Z`
- Mode: `local-cpu-converter-smoke`
- Measurement scope: `local-cpu; not Profile B`
- Git commit: `eb0fab5633c64c2dfe61ad4f1349e26d223a1fbd`
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
| singleWorker | 1 | 24/7 | 0.832 | 103866.23 | 103866.23 | 59.199 |
| concurrent2 | 2 | 24/7 | 0.425 | 203366.067 | 203366.067 | 52.664 |

## Per-format sizing from concurrent run

| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |
|---|---:|---:|---:|---:|---:|---:|---:|
| audio | 2 | 0 | 2 | 0.0 | 20.99 | 1.504 | 0 |
| csv | 3 | 3 | 0 | 170290.598 | 21.39 | 2.98 | 3 |
| docx | 7 | 7 | 0 | 170587.24 | 21.57 | 1.715 | 7 |
| html | 3 | 3 | 0 | 170057.316 | 21.209 | 1.945 | 3 |
| image_ocr | 3 | 0 | 3 | 0.0 | 82.254 | 52.664 | 3 |
| pdf_native | 3 | 3 | 0 | 168329.177 | 21.609 | 3.551 | 3 |
| pdf_scan | 2 | 0 | 2 | 0.0 | 21.488 | 1.836 | 2 |
| pptx | 2 | 2 | 0 | 161601.652 | 22.684 | 6.262 | 2 |
| text_legacy | 3 | 3 | 0 | 171570.185 | 21.142 | 1.414 | 3 |
| xlsx | 3 | 3 | 0 | 170446.475 | 21.234 | 1.895 | 3 |

## Headroom estimate

- Target: `1200.0` docs/hour.
- Required for 30% headroom: `1714.286` docs/hour.
- Measured successful local-cpu throughput: `203366.067` docs/hour.
- Gate-valid effective capacity: `0.0` docs/hour.
- Estimated headroom: `-100.0`%.
- Meets 30% headroom on this runner: `false`.

## Queue age simulation

These rows are deterministic simulations. If any workload format failed,
the gate-valid service rate is set to zero instead of extrapolating from
partial successes.

- Measured service rate: `203366.067` docs/hour.
- Effective simulated service rate: `0.0` docs/hour.
- Capacity valid for gate: `false`.
- Note: set to zero for queue simulation because one or more workload documents failed.

| scenario | arrival docs/hour | final queue docs | oldest age min | stable |
|---|---:|---:|---:|---|
| normal1x | 300.0 | 600.0 | 120.0 | false |
| recovery2xNormal | 600.0 | 1200.0 | 120.0 | false |
| peakGateLoad | 1200.0 | 2400.0 | 120.0 | false |

## Closure

| field | value |
|---|---|
| `harnessCompleted` | `true` |
| `reportWritten` | `true` |
| `gitClean` | `true` |
| `honestFlagsSet` | `true` |

## Failures/timeouts

- `singleWorker` `gold-004` (pdf_scan): error exit=1
- `singleWorker` `gold-005` (pdf_scan): error exit=1
- `singleWorker` `gold-020` (image_ocr): error exit=1
- `singleWorker` `gold-021` (image_ocr): error exit=1
- `singleWorker` `gold-022` (image_ocr): error exit=1
- `singleWorker` `gold-023` (audio): error exit=1
- `singleWorker` `gold-024` (audio): error exit=1
- `concurrent2` `gold-004` (pdf_scan): error exit=1
- `concurrent2` `gold-005` (pdf_scan): error exit=1
- `concurrent2` `gold-020` (image_ocr): error exit=1
- `concurrent2` `gold-021` (image_ocr): error exit=1
- `concurrent2` `gold-022` (image_ocr): error exit=1
- `concurrent2` `gold-023` (audio): error exit=1
- `concurrent2` `gold-024` (audio): error exit=1
