# P0-08 ingest capacity report

- Generated: `2026-07-18T19:21:29.862681Z`
- Mode: `local-cpu-converter-smoke`
- Measurement scope: `local-cpu; not Profile B`
- Git commit: `d1a294a718ac8546cd2fa3f40e3fdcd328981fc3`
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
| singleWorker | 1 | 31/0 | 5.31 | 21015.601 | 19659.756 | 218.598 |
| concurrent2 | 2 | 31/0 | 2.765 | 40359.4 | 37755.567 | 218.785 |

## Per-format sizing from concurrent run

| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |
|---|---:|---:|---:|---:|---:|---:|---:|
| audio | 2 | 2 | 0 | 3880.728 | 945.363 | 217.379 | 0 |
| csv | 3 | 3 | 0 | 165200.765 | 22.158 | 7.266 | 3 |
| docx | 7 | 7 | 0 | 171217.752 | 21.707 | 1.914 | 7 |
| html | 3 | 3 | 0 | 168692.012 | 21.628 | 2.82 | 3 |
| image_ocr | 3 | 3 | 0 | 8238.458 | 448.237 | 106.371 | 3 |
| pdf_native | 3 | 3 | 0 | 169592.664 | 21.617 | 2.355 | 3 |
| pdf_scan | 2 | 2 | 0 | 4491.508 | 801.95 | 218.785 | 2 |
| pptx | 2 | 2 | 0 | 163766.633 | 22.66 | 7.922 | 2 |
| text_legacy | 3 | 3 | 0 | 172270.784 | 21.001 | 1.551 | 3 |
| xlsx | 3 | 3 | 0 | 170942.877 | 21.113 | 1.895 | 3 |

## Headroom estimate

- Target: `1200.0` docs/hour.
- Required for 30% headroom: `1714.286` docs/hour.
- Measured successful local-cpu throughput: `40359.4` docs/hour.
- Local observation headroom: `97.027`%.
- Meets 30% headroom as local observation only: `true`.
- Gate-valid effective capacity (requires Profile B targetMatch): `0.0` docs/hour.
- Capacity valid for gate: `false`.
- Meets headroom target for gate: `false`.

## Queue age simulation

These rows are deterministic **local** simulations from the measured
converter rate. Gate-valid capacity stays zero while `targetMatch=false`.

- Measured local service rate: `40359.4` docs/hour.
- Local simulation service rate: `40359.4` docs/hour.
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
| `gitClean` | `true` |
| `honestFlagsSet` | `true` |
| `allDocumentsSucceeded` | `true` |
