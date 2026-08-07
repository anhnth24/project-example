# Phase A CPU OCR benchmark

Gate decision: **STOP**

This report contains metrics and metadata only; no complete document text or OCR output is included.

## Run metadata

- Generated (UTC): `2026-08-07T14:29:00Z`
- Commit: `01312df988f69319b3a8b1f3ccb09b9d2ef95ca9`
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
| Markhand default | 0.045 | 0.000 | 18.1 |
| Markhand tessdata_best | 0.043 | 0.000 | 15.2 |
| PP-OCRv6 | 1.770 | 1.728 | 595.7 |

## Candidate environment and build provenance

- Markhand default sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- Markhand tessdata_best sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- PP-OCRv6 sanitized environment: `LANG=C.UTF-8, LC_ALL=C.UTF-8, OMP_NUM_THREADS=8, OPENBLAS_NUM_THREADS=8, PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True, PATH=/usr/local/bin:/usr/bin:/bin, PYTHONNOUSERSITE=1, PYTHONPATH=bench/ocr_cpu_service`.
- fileconv binary SHA-256: `d85b911c08b8c26f37995f5609f92f50ac0ec49fdacc5014ef49a6e8f8500d50`.
- fileconv build command: `CC=gcc CXX=g++ cargo build --release -p fileconv-cli --no-default-features`.
- fileconv build features: `no-default-features`; profile: `release`.

## Candidate summary

| Candidate | CER | WER | Warm median s/page | Warm p95 s/page | Warm sampled RSS MiB | Failures |
|---|--:|--:|--:|--:|--:|--:|
| Markhand default | 0.1108 | 0.1918 | 1.145 | 1.433 | 115.7 | 0 |
| Markhand tessdata_best | 0.1026 | 0.1703 | 1.520 | 1.844 | 137.7 | 0 |
| PP-OCRv6 | 0.3994 | 0.7206 | 4.198 | 4.872 | 3553.2 | 0 |

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
| Markhand default | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1879 | 0.2857 | 1.128 | 111.1 | ok |
| Markhand default | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1059 | 0.1570 | 1.371 | 115.7 | ok |
| Markhand default | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1269 | 0.2418 | 1.163 | 112.1 | ok |
| Markhand default | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1942 | 0.2581 | 0.891 | 108.7 | ok |
| Markhand default | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0951 | 0.1592 | 1.338 | 114.3 | ok |
| Markhand default | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0778 | 0.1690 | 0.963 | 110.4 | ok |
| Markhand default | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1654 | 0.2303 | 1.223 | 112.5 | ok |
| Markhand default | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1210 | 0.2542 | 1.508 | 112.3 | ok |
| Markhand default | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1144 | 0.2596 | 1.234 | 103.4 | ok |
| Markhand default | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0015 | 0.0071 | 0.566 | 104.2 | ok |
| Markhand default | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0081 | 0.0335 | 0.785 | 106.7 | ok |
| Markhand default | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0054 | 0.0129 | 0.584 | 103.6 | ok |
| Markhand tessdata_best | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1717 | 0.2492 | 1.473 | 130.0 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1035 | 0.1495 | 1.767 | 137.7 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1128 | 0.1978 | 1.717 | 137.0 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1879 | 0.2419 | 1.182 | 131.1 | ok |
| Markhand tessdata_best | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0908 | 0.1368 | 1.802 | 136.3 | ok |
| Markhand tessdata_best | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0732 | 0.1444 | 1.371 | 131.0 | ok |
| Markhand tessdata_best | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1505 | 0.1966 | 1.567 | 132.6 | ok |
| Markhand tessdata_best | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1030 | 0.2288 | 1.895 | 131.6 | ok |
| Markhand tessdata_best | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1072 | 0.2356 | 1.744 | 123.3 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0000 | 0.0000 | 0.681 | 119.8 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0054 | 0.0251 | 0.977 | 124.8 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0082 | 0.0387 | 0.786 | 119.6 | ok |
| PP-OCRv6 | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.5125 | 0.8116 | 4.350 | 2030.9 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.3323 | 0.6804 | 5.408 | 2844.5 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.3796 | 0.6703 | 4.296 | 3161.0 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.4227 | 0.7097 | 3.544 | 3090.9 | ok |
| PP-OCRv6 | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.4229 | 0.6990 | 4.285 | 3188.1 | ok |
| PP-OCRv6 | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.4833 | 0.8028 | 3.541 | 3537.9 | ok |
| PP-OCRv6 | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.4496 | 0.7753 | 4.111 | 3480.9 | ok |
| PP-OCRv6 | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.4648 | 0.8305 | 4.433 | 3553.2 | ok |
| PP-OCRv6 | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.4931 | 0.8726 | 4.317 | 3167.3 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.1515 | 0.5214 | 2.898 | 3478.2 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0747 | 0.3431 | 3.432 | 3446.7 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.4041 | 0.6839 | 2.979 | 3396.3 | ok |

## Official 89/2026/TT-BTC bounded sample

- Classification: **mixed**.
- Benchmark stratum: **mixed**; gate-included: **false**.
- Manifest/inspection mismatch: **false**.
- Inspection: 839 physical PDF pages; 839 image-bearing and 1 text-bearing page observations (categories may overlap).
- Deterministic sampled pages: 1, 420, 839.
- This source has no pinned human-verified page transcription and is excluded from CER/WER and the quality gate.

## Official sample runtime evidence

| Candidate | Page | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|--:|--:|--:|---|
| Markhand default | 1 | 1.238 | 112.9 | ok |
| Markhand default | 420 | 1.374 | 110.3 | ok |
| Markhand default | 839 | 0.796 | 107.3 | ok |
| Markhand tessdata_best | 1 | 1.780 | 138.6 | ok |
| Markhand tessdata_best | 420 | 2.044 | 129.6 | ok |
| Markhand tessdata_best | 839 | 1.069 | 126.6 | ok |
| PP-OCRv6 | 1 | 4.397 | 3472.2 | ok |
| PP-OCRv6 | 420 | 4.801 | 3482.2 | ok |
| PP-OCRv6 | 839 | 3.435 | 3467.5 | ok |

## Gate

- Better Tesseract real-scan CER: 0.1187
- PP-OCRv6 real-scan CER: 0.4335
- Relative improvement: -265.22% (required: 20%)
- Decision reasons:
  - relative real-scan CER improvement below 20%
  - real-scan: CER regression exceeds 0.05
  - synthetic-scan: CER regression exceeds 0.05

## Tool versions

- cargo: `cargo 1.88.0 (873a06493 2025-05-10)`
- paddleocr: `3.7.0`
- paddlepaddle: `3.2.2`
- paddlex: `3.7.2`
- python: `3.12.3`
- tesseract: `tesseract 5.3.4`
