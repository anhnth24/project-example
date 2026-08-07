# Phase A CPU OCR benchmark

Gate decision: **STOP**

This report contains metrics and metadata only; no complete document text or OCR output is included.

## Run metadata

- Generated (UTC): `2026-08-07T13:56:35Z`
- Commit: `228151acc185fb3afc426a55a7978ae4801639ff`
- Host: 8 logical CPUs, 48197.3 MiB RAM
- Corpus manifest SHA-256: `ac274ab9af348fddf80af297bf6dcbc25c879698e4f923cf0e5e4a558f5442eb`
- Quantitative pages: 12
- Strata: real-scan=9, synthetic-scan=3

## Candidate summary

| Candidate | CER | WER | Median s/page | p95 s/page | Peak RSS MiB | Failures |
|---|--:|--:|--:|--:|--:|--:|
| Markhand default | 0.1108 | 0.1918 | 1.155 | 1.441 | 97.6 | 0 |
| Markhand tessdata_best | 0.1026 | 0.1703 | 1.541 | 1.864 | 119.5 | 0 |
| PP-OCRv6 | 0.3994 | 0.7206 | 4.149 | 4.914 | 2345.5 | 0 |

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

| Candidate | Source ID | Stratum | Page | CER | WER | Seconds | Peak RSS MiB | Status |
|---|---|---|--:|--:|--:|--:|--:|---|
| Markhand default | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1879 | 0.2857 | 1.132 | 93.0 | ok |
| Markhand default | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1059 | 0.1570 | 1.389 | 97.6 | ok |
| Markhand default | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1269 | 0.2418 | 1.179 | 93.9 | ok |
| Markhand default | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1942 | 0.2581 | 0.902 | 90.7 | ok |
| Markhand default | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0951 | 0.1592 | 1.346 | 95.5 | ok |
| Markhand default | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0778 | 0.1690 | 0.975 | 92.2 | ok |
| Markhand default | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1654 | 0.2303 | 1.241 | 94.0 | ok |
| Markhand default | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1210 | 0.2542 | 1.504 | 94.2 | ok |
| Markhand default | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1144 | 0.2596 | 1.261 | 85.1 | ok |
| Markhand default | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0015 | 0.0071 | 0.573 | 85.7 | ok |
| Markhand default | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0081 | 0.0335 | 0.775 | 88.3 | ok |
| Markhand default | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0054 | 0.0129 | 0.593 | 85.4 | ok |
| Markhand tessdata_best | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.1717 | 0.2492 | 1.492 | 111.9 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.1035 | 0.1495 | 1.789 | 119.5 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.1128 | 0.1978 | 1.590 | 118.9 | ok |
| Markhand tessdata_best | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.1879 | 0.2419 | 1.134 | 112.9 | ok |
| Markhand tessdata_best | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.0908 | 0.1368 | 1.812 | 117.8 | ok |
| Markhand tessdata_best | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.0732 | 0.1444 | 1.388 | 112.8 | ok |
| Markhand tessdata_best | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.1505 | 0.1966 | 1.601 | 114.3 | ok |
| Markhand tessdata_best | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.1030 | 0.2288 | 1.928 | 113.5 | ok |
| Markhand tessdata_best | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.1072 | 0.2356 | 1.769 | 104.9 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.0000 | 0.0000 | 0.741 | 103.0 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0054 | 0.0251 | 0.986 | 105.3 | ok |
| Markhand tessdata_best | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.0082 | 0.0387 | 0.783 | 101.4 | ok |
| PP-OCRv6 | `vnocr-real-cv-3722-metro-p1` | real-scan | 1 | 0.5125 | 0.8116 | 4.549 | 1745.1 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-kh-173-festival-p1` | real-scan | 1 | 0.3323 | 0.6804 | 5.359 | 2084.5 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-qd-2280-water-p1` | real-scan | 1 | 0.3796 | 0.6703 | 4.315 | 2129.3 | ok |
| PP-OCRv6 | `vnocr-real-hanoi-tb-453-flag-p1` | real-scan | 1 | 0.4227 | 0.7097 | 3.822 | 2129.3 | ok |
| PP-OCRv6 | `vnocr-real-nq-115-ba-vi-p1` | real-scan | 1 | 0.4229 | 0.6990 | 4.434 | 2167.9 | ok |
| PP-OCRv6 | `vnocr-real-qd-707-ttg-p1` | real-scan | 1 | 0.4833 | 0.8028 | 3.610 | 2173.0 | ok |
| PP-OCRv6 | `vnocr-real-qd-729-ttg-p1` | real-scan | 1 | 0.4496 | 0.7753 | 4.124 | 2227.8 | ok |
| PP-OCRv6 | `vnocr-real-tt-21-bct-fuel-p1` | real-scan | 1 | 0.4648 | 0.8305 | 4.255 | 2242.4 | ok |
| PP-OCRv6 | `vnocr-real-tt-37-bca-vehicle-p1` | real-scan | 1 | 0.4931 | 0.8726 | 4.173 | 2304.3 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-bien-lai-p1` | synthetic-scan | 1 | 0.1515 | 0.5214 | 2.795 | 2304.3 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-hoa-don-p1` | synthetic-scan | 1 | 0.0747 | 0.3431 | 3.465 | 2295.4 | ok |
| PP-OCRv6 | `vnocr-synthetic-scan-phieu-chi-p1` | synthetic-scan | 1 | 0.4041 | 0.6839 | 2.988 | 2345.5 | ok |

## Official 89/2026/TT-BTC bounded sample

- Classification: **mixed**.
- Inspection: 839 pages; 1 with extractable text; 839 with page images.
- Deterministic sampled pages: 1, 420, 839.
- This source has no pinned human-verified page transcription and is excluded from CER/WER and the quality gate.

## Official sample runtime evidence

| Candidate | Page | Seconds | Peak RSS MiB | Status |
|---|--:|--:|--:|---|
| Markhand default | 1 | 1.293 | 94.7 | ok |
| Markhand default | 420 | 1.418 | 92.0 | ok |
| Markhand default | 839 | 0.794 | 88.9 | ok |
| Markhand tessdata_best | 1 | 1.748 | 120.3 | ok |
| Markhand tessdata_best | 420 | 2.088 | 111.3 | ok |
| Markhand tessdata_best | 839 | 1.060 | 108.2 | ok |
| PP-OCRv6 | 1 | 4.608 | 2346.1 | ok |
| PP-OCRv6 | 420 | 4.793 | 2360.6 | ok |
| PP-OCRv6 | 839 | 3.334 | 2360.6 | ok |

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
