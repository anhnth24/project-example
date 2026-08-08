# Phase A CPU OCR benchmark

Gate decision: **STOP**

This report contains metrics and metadata only; no complete document text or OCR output is included.

## Run metadata

- Generated (UTC): `2026-08-07T16:01:26Z`
- Commit: `bc7194a56dcca952633ab145edb18d211d2835a5`
- Host: 8 logical CPUs, 48197.3 MiB RAM
- Corpus manifest SHA-256: `81236508294eb70b3e149a68cc06f80ea329f61fcab3089750630f4e2a4c388a`
- Quantitative pages: 12
- Strata: real-scan=9, synthetic-scan=3

## Sample-size and representativeness limits

- Only 9 real-scan pages and 3 synthetic-scan pages have pinned human-verified text.
- These descriptive CER/WER values apply to this bounded pinned sample; they are not a population estimate and no confidence interval is claimed.
- The synthetic receipt stratum is reported separately and must not be treated as additional real-document evidence.
- The official PDF sample has no human-verified transcription, so it contributes runtime/failure context only and cannot reduce quality uncertainty.

## Cold initialization and resource semantics

Cold initialization is measured once from isolated worker process start through its ready event. Per-page latency is warm worker-request wall time and excludes that cold start. RSS values are 10 ms sampled process-tree RSS sums during each labeled interval; no before/after value is labeled as peak.

| Candidate | Cold wall seconds | Cold candidate seconds | Cold sampled process-tree RSS MiB |
|---|--:|--:|--:|
| Markhand default | 0.045 | 0.000 | 18.3 |
| Markhand tessdata_best | 0.044 | 0.000 | 18.5 |
| PP-OCRv6 | 1.932 | 1.889 | 567.8 |

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
| Markhand default | 0.1108 | 0.1918 | 1.134 | 1.472 | 116.0 | 0 |
| Markhand tessdata_best | 0.1026 | 0.1703 | 1.537 | 1.843 | 138.1 | 0 |
| PP-OCRv6 | 0.3994 | 0.7206 | 4.369 | 5.067 | 3336.3 | 0 |

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
| Markhand default | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1879 | 0.2857 | 1.099 | 111.4 | ok |
| Markhand default | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1059 | 0.1570 | 1.414 | 116.0 | ok |
| Markhand default | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1269 | 0.2418 | 1.169 | 112.5 | ok |
| Markhand default | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1942 | 0.2581 | 0.892 | 108.8 | ok |
| Markhand default | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0951 | 0.1592 | 1.344 | 113.7 | ok |
| Markhand default | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0778 | 0.1690 | 0.965 | 110.5 | ok |
| Markhand default | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1654 | 0.2303 | 1.227 | 112.4 | ok |
| Markhand default | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1210 | 0.2542 | 1.542 | 112.5 | ok |
| Markhand default | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1144 | 0.2596 | 1.265 | 103.4 | ok |
| Markhand default | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0015 | 0.0071 | 0.593 | 104.3 | ok |
| Markhand default | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0081 | 0.0335 | 0.815 | 106.8 | ok |
| Markhand default | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0054 | 0.0129 | 0.613 | 103.9 | ok |
| Markhand tessdata_best | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1717 | 0.2492 | 1.518 | 130.4 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1035 | 0.1495 | 1.790 | 138.1 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1128 | 0.1978 | 1.581 | 137.4 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1879 | 0.2419 | 1.139 | 131.7 | ok |
| Markhand tessdata_best | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0908 | 0.1368 | 1.814 | 136.4 | ok |
| Markhand tessdata_best | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0732 | 0.1444 | 1.387 | 131.4 | ok |
| Markhand tessdata_best | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1505 | 0.1966 | 1.557 | 132.9 | ok |
| Markhand tessdata_best | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1030 | 0.2288 | 1.878 | 131.9 | ok |
| Markhand tessdata_best | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1072 | 0.2356 | 1.718 | 123.6 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0000 | 0.0000 | 0.677 | 120.0 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0054 | 0.0251 | 0.982 | 122.8 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0082 | 0.0387 | 0.776 | 122.0 | ok |
| PP-OCRv6 | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.5125 | 0.8116 | 4.707 | 1874.3 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.3323 | 0.6804 | 5.390 | 2727.6 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.3796 | 0.6703 | 4.414 | 3206.0 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.4227 | 0.7097 | 3.765 | 3174.4 | ok |
| PP-OCRv6 | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.4229 | 0.6990 | 4.703 | 3252.2 | ok |
| PP-OCRv6 | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.4833 | 0.8028 | 3.805 | 3260.9 | ok |
| PP-OCRv6 | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.4496 | 0.7753 | 4.803 | 3248.6 | ok |
| PP-OCRv6 | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.4648 | 0.8305 | 4.636 | 3336.3 | ok |
| PP-OCRv6 | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.4931 | 0.8726 | 4.325 | 2861.9 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.1515 | 0.5214 | 3.143 | 3179.5 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0747 | 0.3431 | 3.860 | 3134.7 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.4041 | 0.6839 | 3.207 | 3155.2 | ok |

## Official 89/2026/TT-BTC bounded sample

- Classification: **scan**.
- Benchmark schema stratum: **mixed** (non-gate context); gate-included: **false**.
- Manifest/inspection mismatch: **true**.
- Inspection: 839 physical PDF pages; 839 image-bearing and 0 text-bearing page observations (categories may overlap).
- Deterministic sampled pages: 1, 420, 839.
- This source has no pinned human-verified page transcription and is excluded from CER/WER and the quality gate.

## Official sample runtime evidence

| Candidate | Page | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|--:|--:|--:|---|
| Markhand default | 1 | 1.253 | 112.8 | ok |
| Markhand default | 420 | 1.430 | 110.4 | ok |
| Markhand default | 839 | 0.848 | 107.5 | ok |
| Markhand tessdata_best | 1 | 1.441 | 136.7 | ok |
| Markhand tessdata_best | 420 | 2.120 | 130.2 | ok |
| Markhand tessdata_best | 839 | 1.064 | 126.1 | ok |
| PP-OCRv6 | 1 | 4.734 | 3260.8 | ok |
| PP-OCRv6 | 420 | 5.198 | 3228.6 | ok |
| PP-OCRv6 | 839 | 3.599 | 3243.7 | ok |

## Historical qualitative evidence

These public historical scans were already checksum-pinned in the manifest and were run as bounded qualitative samples. There is no trustworthy transcription for the sampled pages, so no CER/WER or quality-gate claim is made.

- `wikimedia-cuu-quoc-1945-09-05`: classification **scan**; sampled pages 1, 2; evidence mode **qualitative only**.
- `wikimedia-dai-nam-1907-804`: classification **scan**; sampled pages 1, 4, 8; evidence mode **qualitative only**.

| Candidate | Source ID | Page | Warm seconds | Sampled process-tree RSS MiB | Status |
|---|---|--:|--:|--:|---|
| Markhand default | `wikimedia-cuu-quoc-1945-09-05` | 1 | 6.645 | 199.0 | ok |
| Markhand default | `wikimedia-cuu-quoc-1945-09-05` | 2 | 9.887 | 198.9 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 1 | 2.092 | 105.0 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 4 | 2.497 | 106.9 | ok |
| Markhand default | `wikimedia-dai-nam-1907-804` | 8 | 2.593 | 106.6 | ok |
| Markhand tessdata_best | `wikimedia-cuu-quoc-1945-09-05` | 1 | 9.111 | 199.1 | ok |
| Markhand tessdata_best | `wikimedia-cuu-quoc-1945-09-05` | 2 | 13.190 | 199.0 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 1 | 2.896 | 126.2 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 4 | 3.541 | 123.6 | ok |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 8 | 3.519 | 123.8 | ok |
| PP-OCRv6 | `wikimedia-cuu-quoc-1945-09-05` | 1 | 23.885 | 5173.5 | ok |
| PP-OCRv6 | `wikimedia-cuu-quoc-1945-09-05` | 2 | 26.305 | 5216.5 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 1 | 4.008 | 2985.3 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 4 | 5.727 | 2844.7 | ok |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 8 | 6.262 | 2872.3 | ok |

## Reviewed multi-column reading order

The deterministic two-column fixture uses source-ground-truth column-major anchor order. Violations are pairwise inversions among observed anchors; missing anchors are reported separately and are not silently counted as correctly ordered.
The historical scan case uses only a small human-reviewed sequence of short headings. It is qualitative and limited: it is not a transcription, CER sample, or general layout score. Matching folds accents/punctuation and permits at most 25% character edits for OCR noise.

- `reviewed-multicolumn-v1` page 1 (deterministic-source): L1 → L2 → L3 → R1 → R2 → R3.
- `wikimedia-dai-nam-1907-804` page 4 (human-reviewed-short-anchors): NHỜI ĐÀN BÀ → RAO HẸN → TẬP THƠ, PHÚ, CA, RAO → CÁO BẠCH → HIỆN BÁO HOÀN CẦU.

| Candidate | Source ID | Page | Expected anchors | Observed anchors | Comparable pairs | Violations | Missing anchors |
|---|---|--:|--:|--:|--:|--:|--:|
| Markhand default | `wikimedia-dai-nam-1907-804` | 4 | 5 | 5 | 10 | 0 | 0 |
| Markhand default | `reviewed-multicolumn-v1` | 1 | 6 | 6 | 15 | 0 | 0 |
| Markhand tessdata_best | `wikimedia-dai-nam-1907-804` | 4 | 5 | 5 | 10 | 0 | 0 |
| Markhand tessdata_best | `reviewed-multicolumn-v1` | 1 | 6 | 6 | 15 | 0 | 0 |
| PP-OCRv6 | `wikimedia-dai-nam-1907-804` | 4 | 5 | 3 | 3 | 0 | 2 |
| PP-OCRv6 | `reviewed-multicolumn-v1` | 1 | 6 | 6 | 15 | 3 | 0 |

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
