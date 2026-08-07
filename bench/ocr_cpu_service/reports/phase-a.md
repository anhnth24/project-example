# Phase A CPU OCR benchmark

Gate decision: **STOP**

This report contains metrics and metadata only; no complete document text or OCR output is included.

## Run metadata

- Generated (UTC): `2026-08-07T15:12:36Z`
- Commit: `42cc64bfbf05da97ed4ca78494711b63c2774898`
- Host: 8 logical CPUs, 48197.3 MiB RAM
- Corpus manifest SHA-256: `81236508294eb70b3e149a68cc06f80ea329f61fcab3089750630f4e2a4c388a`
- Quantitative pages: 12
- Strata: real-scan=9, synthetic-scan=3

## Sample-size and representativeness limits

- Only 9 real-scan pages and 3 synthetic-scan pages have pinned human-verified text.
- These descriptive CER/WER values apply to this bounded pinned sample; they are not a population estimate and no confidence interval is claimed.
- The synthetic receipt stratum is reported separately and must not be treated as additional real-document evidence.
- The official mixed PDF sample has no human-verified transcription, so it contributes runtime/failure context only and cannot reduce quality uncertainty.

## Cold initialization and resource semantics

Cold initialization is measured once from isolated worker process start through its ready event. Per-page latency is warm worker-request wall time and excludes that cold start. RSS values are 10 ms sampled process-tree RSS sums during each labeled interval; no before/after value is labeled as peak.

| Candidate | Cold wall seconds | Cold candidate seconds | Cold sampled process-tree RSS MiB |
|---|--:|--:|--:|
| Markhand default | 0.045 | 0.000 | 18.0 |
| Markhand tessdata_best | 0.042 | 0.000 | 15.4 |
| PP-OCRv6 | 1.751 | 1.708 | 566.0 |

## Candidate environment and build provenance

- Markhand default sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- Markhand default timing: warm timing includes a fresh fileconv/Tesseract subprocess spawn, execution, and output collection for every page.
- Markhand tessdata_best sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- Markhand tessdata_best timing: warm timing includes a fresh fileconv/Tesseract subprocess spawn, execution, and output collection for every page.
- PP-OCRv6 sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- fileconv binary SHA-256: `d85b911c08b8c26f37995f5609f92f50ac0ec49fdacc5014ef49a6e8f8500d50`.
- fileconv build command: `CC=gcc CXX=g++ cargo build --release -p fileconv-cli --no-default-features`.
- fileconv build features: `no-default-features`; profile: `release`.

## Candidate summary

| Candidate | CER | WER | Warm median s/page | Warm p95 s/page | Warm sampled RSS MiB | Failures |
|---|--:|--:|--:|--:|--:|--:|
| Markhand default | 0.1108 | 0.1918 | 1.114 | 1.402 | 115.6 | 0 |
| Markhand tessdata_best | 0.1026 | 0.1703 | 1.521 | 1.831 | 138.1 | 0 |
| PP-OCRv6 | 0.3994 | 0.7206 | 4.062 | 4.701 | 3269.4 | 0 |

## CER/WER by stratum

| Candidate | Stratum | Pages | CER | WER |
|---|---|--:|--:|--:|
| Markhand default | real-scan | 9 | 0.1282 | 0.2196 |
| Markhand default | synthetic-scan | 3 | 0.0056 | 0.0206 |
| Markhand tessdata_best | real-scan | 9 | 0.1187 | 0.1943 |
| Markhand tessdata_best | synthetic-scan | 3 | 0.0048 | 0.0225 |
| PP-OCRv6 | real-scan | 9 | 0.4335 | 0.7582 |
| PP-OCRv6 | synthetic-scan | 3 | 0.1917 | 0.4888 |

## Per-page quantitative metrics

| Candidate | Source ID | Stratum | Page | CER | WER | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|---|---|--:|--:|--:|--:|--:|---|
| Markhand default | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1879 | 0.2857 | 1.087 | 111.0 | ok |
| Markhand default | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1059 | 0.1570 | 1.355 | 115.6 | ok |
| Markhand default | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1269 | 0.2418 | 1.140 | 112.1 | ok |
| Markhand default | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1942 | 0.2581 | 0.868 | 108.8 | ok |
| Markhand default | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0951 | 0.1592 | 1.311 | 113.5 | ok |
| Markhand default | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0778 | 0.1690 | 0.963 | 110.5 | ok |
| Markhand default | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1654 | 0.2303 | 1.189 | 112.3 | ok |
| Markhand default | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1210 | 0.2542 | 1.459 | 112.5 | ok |
| Markhand default | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1144 | 0.2596 | 1.234 | 103.3 | ok |
| Markhand default | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0015 | 0.0071 | 0.564 | 104.1 | ok |
| Markhand default | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0081 | 0.0335 | 0.772 | 106.7 | ok |
| Markhand default | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0054 | 0.0129 | 0.584 | 103.4 | ok |
| Markhand tessdata_best | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1717 | 0.2492 | 1.476 | 130.0 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1035 | 0.1495 | 1.801 | 138.1 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1128 | 0.1978 | 1.609 | 137.2 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1879 | 0.2419 | 1.129 | 131.5 | ok |
| Markhand tessdata_best | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0908 | 0.1368 | 1.788 | 136.3 | ok |
| Markhand tessdata_best | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0732 | 0.1444 | 1.358 | 131.2 | ok |
| Markhand tessdata_best | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1505 | 0.1966 | 1.567 | 132.8 | ok |
| Markhand tessdata_best | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1030 | 0.2288 | 1.868 | 131.7 | ok |
| Markhand tessdata_best | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1072 | 0.2356 | 1.723 | 124.2 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0000 | 0.0000 | 0.683 | 119.6 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0054 | 0.0251 | 0.990 | 122.9 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0082 | 0.0387 | 0.778 | 119.7 | ok |
| PP-OCRv6 | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.5125 | 0.8116 | 4.296 | 2129.8 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.3323 | 0.6804 | 5.121 | 2969.8 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.3796 | 0.6703 | 4.342 | 3142.8 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.4227 | 0.7097 | 3.512 | 3070.3 | ok |
| PP-OCRv6 | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.4229 | 0.6990 | 4.358 | 3165.1 | ok |
| PP-OCRv6 | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.4833 | 0.8028 | 3.708 | 3167.1 | ok |
| PP-OCRv6 | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.4496 | 0.7753 | 4.292 | 3158.9 | ok |
| PP-OCRv6 | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.4648 | 0.8305 | 4.128 | 3198.4 | ok |
| PP-OCRv6 | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.4931 | 0.8726 | 3.995 | 2806.7 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.1515 | 0.5214 | 2.801 | 3269.4 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0747 | 0.3431 | 3.418 | 3172.4 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.4041 | 0.6839 | 2.904 | 3143.7 | ok |

## Official 89/2026/TT-BTC bounded sample

- Classification: **scan**.
- Benchmark stratum: **mixed**; gate-included: **false**.
- Manifest/inspection mismatch: **true**.
- Inspection: 839 physical PDF pages; 839 image-bearing and 0 text-bearing page observations (categories may overlap).
- Deterministic sampled pages: 1, 420, 839.
- This source has no pinned human-verified page transcription and is excluded from CER/WER and the quality gate.

## Official sample runtime evidence

| Candidate | Page | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|--:|--:|--:|---|
| Markhand default | 1 | 1.187 | 112.8 | ok |
| Markhand default | 420 | 1.412 | 110.1 | ok |
| Markhand default | 839 | 0.820 | 107.2 | ok |
| Markhand tessdata_best | 1 | 1.447 | 136.4 | ok |
| Markhand tessdata_best | 420 | 2.074 | 129.9 | ok |
| Markhand tessdata_best | 839 | 1.074 | 126.1 | ok |
| PP-OCRv6 | 1 | 4.242 | 3261.3 | ok |
| PP-OCRv6 | 420 | 4.829 | 3222.7 | ok |
| PP-OCRv6 | 839 | 3.377 | 3298.0 | ok |

## Historical qualitative evidence

These public historical scans were already checksum-pinned in the manifest and were run as bounded qualitative samples. There is no trustworthy transcription for the sampled pages, so no CER/WER or quality-gate claim is made.

- `wikimedia-cuu-quoc-1945-09-05`: classification **scan**; sampled pages 1, 2; evidence mode **qualitative only**.
- `wikimedia-dai-nam-1907-804`: classification **scan**; sampled pages 1, 4, 8; evidence mode **qualitative only**.

| Candidate | Source ID | Page | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|---|--:|--:|--:|---|
| Markhand default | `wikimedia-cuu-quoc-1945-09-05` | 1 | 6.472 | 198.8 | ok |
| Markhand default | `wikimedia-cuu-quoc-1945-09-05` | 2 | 9.631 | 198.7 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 1 | 2.082 | 104.9 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 4 | 2.447 | 106.5 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 8 | 2.514 | 106.6 | ok |
| Markhand tessdata_best | `wikimedia-cuu-quoc-1945-09-05` | 1 | 9.240 | 198.9 | ok |
| Markhand tessdata_best | `wikimedia-cuu-quoc-1945-09-05` | 2 | 12.944 | 198.8 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 1 | 2.845 | 125.9 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 4 | 3.555 | 123.5 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 8 | 3.601 | 123.5 | ok |
| PP-OCRv6 | `wikimedia-cuu-quoc-1945-09-05` | 1 | 23.188 | 5229.3 | ok |
| PP-OCRv6 | `wikimedia-cuu-quoc-1945-09-05` | 2 | 24.185 | 5296.6 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 1 | 3.515 | 3127.4 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 4 | 5.158 | 3014.2 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 8 | 5.745 | 3060.4 | ok |

## Reviewed multi-column reading order

The deterministic two-column fixture uses source-ground-truth column-major anchor order. Violations are pairwise inversions among observed anchors; missing anchors are reported separately and are not silently counted as correctly ordered.

| Candidate | Expected anchors | Observed anchors | Comparable pairs | Violations | Missing anchors |
|---|--:|--:|--:|--:|--:|
| Markhand default | 6 | 6 | 15 | 0 | 0 |
| Markhand tessdata_best | 6 | 6 | 15 | 0 | 0 |
| PP-OCRv6 | 6 | 6 | 15 | 3 | 0 |

## Gate

- Better Tesseract real-scan CER: 0.1187
- PP-OCRv6 real-scan CER: 0.4335
- Relative improvement: -265.22% (required: 20%)
- Decision reasons:
  - relative real-scan CER improvement below 20%
  - real-scan: CER regression exceeds 0.05
  - synthetic-scan: CER regression exceeds 0.05
  - 2 resource-limit violation(s)

## Tool versions

- cargo: `cargo 1.88.0 (873a06493 2025-05-10)`
- paddleocr: `3.7.0`
- paddlepaddle: `3.2.2`
- paddlex: `3.7.2`
- pypdfium2: `5.12.1`
- python: `3.12.3`
- tesseract: `tesseract 5.3.4`
