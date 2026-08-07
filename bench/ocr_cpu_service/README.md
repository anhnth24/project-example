# Markhand CPU OCR benchmark service

This isolated benchmark service renders bounded local PDFs and recognizes page
images through PaddleOCR's public Python API. It does not import or run MinerU.
The Paddle backend uses the current PP-OCRv6 defaults, explicitly selects CPU,
and disables the optional orientation and unwarping models.

## Reproduce the benchmark

Run from the repository root with Python 3.12:

```bash
python3 -m venv bench/ocr_cpu_service/.venv
bench/ocr_cpu_service/.venv/bin/python -m pip install --upgrade pip
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --extra-index-url https://www.paddlepaddle.org.cn/packages/stable/cpu/ \
  -e 'bench/ocr_cpu_service[test,model]'
```

`requirements.lock` records the complete environment resolved by that command.
For an exact reinstall, install the lock with the same CPU wheel index, then
install this package without resolving dependencies:

```bash
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --extra-index-url https://www.paddlepaddle.org.cn/packages/stable/cpu/ \
  -r bench/ocr_cpu_service/requirements.lock
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --no-deps -e bench/ocr_cpu_service
```

PaddleOCR does not install or pin the PaddlePaddle framework. At investigation
time, 3.3.1 was the newest wheel on the official CPU index, but that runtime has
a confirmed CPU/oneDNN inference regression. The model extra therefore pins
PaddlePaddle 3.2.2, the newest upstream-recommended compatible CPU release.
PaddleX also requires NumPy below 2.4, so the latest compatible NumPy line is
constrained explicitly.

Download and checksum-verify the pinned corpus into the ignored data directory:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -c \
'from pathlib import Path; from corpus.download import download_sources; items = download_sources(Path("bench/ocr_cpu_service/corpus/sources.json"), Path("bench/ocr_cpu_service/.data/corpus")); print(f"verified {len(items)} sources, {sum(item.bytes_downloaded for item in items)} bytes")'
```

Place complete official PaddleOCR detection and recognition directories at
`bench/ocr_cpu_service/.data/models/detection` and
`bench/ocr_cpu_service/.data/models/recognition`. Each directory must contain
`inference.json`, `inference.yml`, and `inference.pdiparams`; the benchmark is
cache-only and records asset hashes rather than model identifiers. Then build
the measured baseline and run all candidates serially:

```bash
CC=gcc CXX=g++ cargo build --release \
  -p fileconv-cli --no-default-features

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m benchmark.run \
  --output bench/ocr_cpu_service/reports/phase-a.json \
  --manifest bench/ocr_cpu_service/corpus/sources.json \
  --corpus-dir bench/ocr_cpu_service/.data/corpus \
  --work-dir bench/ocr_cpu_service/.data/benchmark \
  --fileconv target/release/fileconv \
  --system-tessdata /usr/share/tesseract-ocr/5/tessdata \
  --best-tessdata tessdata_best \
  --paddle-detection-dir bench/ocr_cpu_service/.data/models/detection \
  --paddle-recognition-dir bench/ocr_cpu_service/.data/models/recognition \
  --cpu-threads 8 \
  --max-rss-bytes 4294967296

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m benchmark.report \
  bench/ocr_cpu_service/reports/phase-a.json
```

The measured run used an x86_64 Linux host provisioned with 8 vCPUs and
48 GiB RAM (50,538,512,384 bytes visible to the process).

## Test and service run

The fast suite does not initialize or download models:

```bash
python3 -m pytest bench/ocr_cpu_service/tests -q
```

The opt-in live test generates its own Vietnamese image and is strictly
cache-only. Point both variables at complete local PaddleOCR model directories.
Each directory must contain `inference.json`, `inference.yml`, and
`inference.pdiparams`. The test skips before PaddleOCR initialization if either
directory or any required asset is unavailable, and passes the directories
through PaddleOCR's documented local-model arguments so it cannot download:

```bash
MARKHAND_OCR_LIVE=1 \
MARKHAND_OCR_LIVE_DETECTION_MODEL_DIR=/path/to/cached/detection \
MARKHAND_OCR_LIVE_RECOGNITION_MODEL_DIR=/path/to/cached/recognition \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_paddle_backend.py -m live -q
```

Cached models remain outside the repository and are never committed.

Start one service process, and therefore one initialized model pipeline:

```bash
cd bench/ocr_cpu_service
MARKHAND_OCR_BACKEND=paddle \
MARKHAND_OCR_DETECTION_MODEL_DIR="$PWD/.data/models/detection" \
MARKHAND_OCR_RECOGNITION_MODEL_DIR="$PWD/.data/models/recognition" \
MARKHAND_OCR_ACQUISITION_TIMEOUT_SECONDS=0.1 \
MARKHAND_OCR_CONVERSION_DEADLINE_SECONDS=120 \
scripts/run_service.sh
```

The default bind address is `127.0.0.1:8765`. Override it with
`MARKHAND_OCR_HOST` and `MARKHAND_OCR_PORT`. The health response reports only
the stable backend name (`paddle`), not model identifiers. Startup fails before
PaddleOCR construction unless both configured local model directories exist
and contain non-empty `inference.json`, `inference.yml`, and
`inference.pdiparams` assets. The real service therefore has no model-download
fallback.

Conversion admission is process-wide, allows exactly one active conversion,
and has no unbounded waiting queue. A request waits at most the configured
acquisition timeout, then receives `503`. Once admitted, a request receives
`504` when its response deadline expires. Python cannot safely terminate an
inference thread, so a timed-out conversion deliberately retains the sole
capacity slot until its underlying work actually exits; later requests remain
bounded/rejected during that interval. The completion callback eventually
releases the slot. Both timeout settings must be finite positive seconds, and
the process uses one Uvicorn worker so these bounds cannot be multiplied by
worker count.

## Reviewed Phase A result and decision

Phase A is **STOP**. The quantitative quality set is limited to 12 pinned pages
with human-verified text: 9 real scans and 3 synthetic receipts. The values are
descriptive for this bounded sample only; they are not a population estimate,
and no confidence interval is claimed.

| Candidate | Aggregate CER | Aggregate WER | Real-scan CER | Synthetic CER | Warm median s/page | Warm p95 s/page | Warm sampled RSS MiB |
|---|--:|--:|--:|--:|--:|--:|--:|
| Markhand default | 0.110828 | 0.191784 | 0.128154 | 0.005616 | 1.145 | 1.433 | 115.7 |
| Markhand `tessdata_best` | 0.102603 | 0.170330 | 0.118708 | 0.004813 | 1.520 | 1.844 | 137.7 |
| PP-OCRv6 | 0.399353 | 0.720565 | 0.433545 | 0.191737 | 4.198 | 4.872 | 3553.2 |

The better Tesseract real-scan CER is `0.118708`; PP-OCRv6 is `0.433545`.
Relative improvement is `-265.22%`, below the required `20%`, and both strata
regress by more than 0.05 absolute CER. There were no failures, timeouts, or
resource-limit violations.

Warm latency is parent wall time from request flush through the result event
with an initialized worker. RSS is not an OS high-water mark: it is the maximum
of 10 ms samples of the worker plus descendant-process RSS during each labeled
interval. Cold worker initialization was measured separately.

The official `89/2026/TT-BTC` PDF is **mixed**. Inspection produced 840
overlapping page-type observations across 839 physical PDF pages: 839
image-bearing observations and 1 text-bearing observation, because page 1 has
both. Deterministic pages 1, 420, and 839 were sampled. They have no pinned
human-verified transcription, so they provide runtime/failure context only and
are excluded from CER/WER and the gate.

Phase B was correctly not run after the STOP decision. This spike does not
adopt PP-OCRv6 or this Python service in production; any future production
integration requires separate evidence and review.

## Dependency and license notes

- PaddleOCR, PaddlePaddle, PaddleX, and the official pretrained OCR weights are
  Apache-2.0.
- The resolved environment contains no package whose name indicates a CUDA,
  ROCm, GPU, TensorRT, or cuDNN-only runtime.
- Transitive licenses are predominantly Apache, BSD, MIT, MPL, and PSF.
  `python-bidi` is LGPL-3.0-or-later. PyMuPDF retains its existing
  AGPL-3.0/commercial dual license and must be reviewed before distribution.
- Downloaded model assets and the `.venv` are runtime-only, ignored artifacts.
