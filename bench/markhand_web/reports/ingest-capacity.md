# P0-08 ingest capacity report

- Generated: `2026-07-18T19:21:13.957260Z`
- Mode: `local-cpu-converter-smoke`
- Measurement scope: `local-cpu; not Profile B`
- Git commit: `04a37b1270b4aab91e38e336f66c38b789713b25`
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
| singleWorker | 1 | 31/0 | 5.35 | 20859.956 | 19514.153 | 219.023 |
| concurrent2 | 2 | 31/0 | 2.932 | 38061.558 | 35605.973 | 218.891 |

## Per-format sizing from concurrent run

| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |
|---|---:|---:|---:|---:|---:|---:|---:|
| audio | 2 | 2 | 0 | 3735.06 | 982.092 | 217.527 | 0 |
| csv | 3 | 3 | 0 | 169728.591 | 21.398 | 2.062 | 3 |
| docx | 7 | 7 | 0 | 170286.378 | 22.087 | 3.777 | 7 |
| html | 3 | 3 | 0 | 168014.935 | 21.762 | 3.73 | 3 |
| image_ocr | 3 | 3 | 0 | 7478.339 | 499.059 | 108.707 | 3 |
| pdf_native | 3 | 3 | 0 | 167167.136 | 22.192 | 1.406 | 3 |
| pdf_scan | 2 | 2 | 0 | 4198.478 | 867.042 | 218.891 | 2 |
| pptx | 2 | 2 | 0 | 168255.749 | 21.59 | 1.754 | 2 |
| text_legacy | 3 | 3 | 0 | 173335.259 | 20.818 | 0.703 | 3 |
| xlsx | 3 | 3 | 0 | 171660.176 | 21.008 | 1.613 | 3 |

## Headroom estimate

- Target: `1200.0` docs/hour.
- Required for 30% headroom: `1714.286` docs/hour.
- Measured successful local-cpu throughput: `38061.558` docs/hour.
- Local observation headroom: `96.847`%.
- Meets 30% headroom as local observation only: `true`.
- Gate-valid effective capacity (requires Profile B targetMatch): `0.0` docs/hour.
- Capacity valid for gate: `false`.
- Meets headroom target for gate: `false`.

## Queue age simulation

These rows are deterministic **local** simulations from the measured
converter rate. Gate-valid capacity stays zero while `targetMatch=false`.

- Measured local service rate: `38061.558` docs/hour.
- Local simulation service rate: `38061.558` docs/hour.
- Gate-valid service rate: `0.0` docs/hour.
- Capacity valid for gate: `false`.
- Note: local queue simulation uses measured converter rate; gate-valid rate requires targetMatch=true on Profile B.

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
- `bench/markhand_web/scripts/run_ingest_capacity.py`
