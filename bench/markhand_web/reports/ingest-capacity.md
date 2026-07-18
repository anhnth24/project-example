# P0-08 ingest capacity report

- Generated: `2026-07-18T19:18:41.620677Z`
- Mode: `local-cpu-converter-smoke`
- Measurement scope: `local-cpu; not Profile B`
- Git commit: `310f128e510795e04c04d0ee20ab40aae06d9882`
- Dirty at harness start: `true`
- `targetMatch`: `false`
- `profileBGatePassed`: `false`
- `productionCapacityBlocked`: `true`
- `p0_08_closed`: `false`

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
| singleWorker | 1 | 31/0 | 5.204 | 21443.277 | 20059.84 | 220.523 |
| concurrent2 | 2 | 31/0 | 2.77 | 40290.153 | 37690.789 | 220.387 |

## Per-format sizing from concurrent run

| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |
|---|---:|---:|---:|---:|---:|---:|---:|
| audio | 2 | 2 | 0 | 3984.69 | 921.486 | 217.742 | 0 |
| csv | 3 | 3 | 0 | 169117.302 | 21.447 | 2.137 | 3 |
| docx | 7 | 7 | 0 | 170441.864 | 21.621 | 2.168 | 7 |
| html | 3 | 3 | 0 | 169382.538 | 21.666 | 4.254 | 3 |
| image_ocr | 3 | 3 | 0 | 8281.415 | 436.927 | 106.398 | 3 |
| pdf_native | 3 | 3 | 0 | 170204.718 | 21.364 | 1.641 | 3 |
| pdf_scan | 2 | 2 | 0 | 4533.417 | 803.656 | 220.387 | 2 |
| pptx | 2 | 2 | 0 | 171001.069 | 21.146 | 1.707 | 2 |
| text_legacy | 3 | 3 | 0 | 172251.551 | 21.005 | 1.34 | 3 |
| xlsx | 3 | 3 | 0 | 170301.339 | 21.611 | 3.664 | 3 |

## Headroom estimate

- Target: `1200.0` docs/hour.
- Required for 30% headroom: `1714.286` docs/hour.
- Measured successful local-cpu throughput: `40290.153` docs/hour.
- Gate-valid effective capacity: `40290.153` docs/hour.
- Estimated headroom: `97.022`%.
- Meets 30% headroom on this runner: `true`.

## Queue age simulation

These rows are deterministic simulations. If any workload format failed,
the gate-valid service rate is set to zero instead of extrapolating from
partial successes.

- Measured service rate: `40290.153` docs/hour.
- Effective simulated service rate: `40290.153` docs/hour.
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
| `gitClean` | `false` |
| `honestFlagsSet` | `true` |
| `allDocumentsSucceeded` | `true` |

Dirty paths at harness start:
- `bench/markhand_web/scripts/run_ingest_capacity.sh`
