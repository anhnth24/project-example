# Markhand CPU OCR benchmark service

This isolated benchmark service renders bounded local PDFs and recognizes page
images through PaddleOCR's public Python API. It does not import or run MinerU.
The Paddle backend uses the current PP-OCRv6 defaults, explicitly selects CPU,
and disables the optional orientation and unwarping models.

## Reproduce the benchmark

Run from the repository root with Python 3.12:

```bash
python3 -m venv bench/ocr_cpu_service/.venv
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --upgrade pip==26.2.1
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  -r bench/ocr_cpu_service/pylock.toml
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --no-deps -e bench/ocr_cpu_service
```

`pylock.toml` is the native pip 26 platform lock for CPython 3.12 on Linux
x86_64. It records the exact selected wheel URL and SHA-256 for every runtime,
test, and model dependency; installation does not re-resolve versions. Regenerate
it only on the declared target with:

```bash
bench/ocr_cpu_service/.venv/bin/python -m pip lock --only-deps \
  'bench/ocr_cpu_service[test,model]' \
  --extra-index-url https://www.paddlepaddle.org.cn/packages/stable/cpu/ \
  -o bench/ocr_cpu_service/pylock.toml
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

The downloader keeps the strict hostname allowlist, rejects any DNS answer
containing a non-public address, and binds each TLS connection to the exact
validated IP set while retaining the allowlisted hostname for SNI/certificate
verification. Redirects are revalidated and re-resolved per hop. A monotonic
120-second whole-download deadline and shorter connection deadline are enforced
in addition to byte/checksum limits; `read1()` keeps slow-drip progress visible
to the total-deadline check.

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

Conversion admission is process-wide and occurs in ASGI middleware before the
application reads or parses the multipart body. Exactly one request owns the
slot and at most one additional request may wait; further requests receive
`503` immediately, while the bounded waiter receives `503` if its acquisition
timeout expires. The admitted request's deadline covers body receipt,
multipart parsing, rendering, inference, and response construction.

Python cannot safely terminate an inference thread. A conversion that exceeds
the deadline therefore returns `504` but truthfully retains the sole capacity
slot until its underlying work exits and closes PDF/page/image resources.
Later requests remain bounded or rejected during that interval. The slot is
released only after both request handling and underlying work finish. Both
timeouts must be finite positive seconds, and one Uvicorn worker prevents
multiplying the process-local bound. ASGI servers may buffer at the transport
layer before invoking the application; application-level body buffering starts
only after admission.

## Reviewed Phase A result and decision

Phase A is **STOP**. The quantitative quality set is limited to 12 pinned pages
with human-verified text: 9 real scans and 3 synthetic receipts. The values are
descriptive for this bounded sample only; they are not a population estimate,
and no confidence interval is claimed.

| Candidate | Aggregate CER | Aggregate WER | Real-scan CER | Synthetic CER | Warm median s/page | Warm p95 s/page | Warm sampled RSS MiB |
|---|--:|--:|--:|--:|--:|--:|--:|
| Markhand default | 0.110828 | 0.191784 | 0.128154 | 0.005616 | 1.134 | 1.472 | 116.0 |
| Markhand `tessdata_best` | 0.102603 | 0.170330 | 0.118708 | 0.004813 | 1.537 | 1.843 | 138.1 |
| PP-OCRv6 | 0.399353 | 0.720565 | 0.433545 | 0.191737 | 4.369 | 5.067 | 3336.3 |

The better Tesseract real-scan CER is `0.118708`; PP-OCRv6 is `0.433545`.
Relative improvement is `-265.22%`, below the required `20%`, and both strata
regress by more than 0.05 absolute CER. There were no candidate failures or
timeouts. PP-OCRv6 exceeded the 4 GiB sampled process-tree RSS bound on both
bounded `Cứu Quốc` historical pages (5.05–5.09 GiB), adding two
resource-limit violations and independently reinforcing STOP.

Warm latency is parent wall time from request flush through the result event
with an initialized worker. For both Markhand candidates that interval includes
a fresh `fileconv`/Tesseract subprocess spawn, execution, and output collection
for every page; it is not in-process Tesseract-only time. RSS is not an OS
high-water mark: it is the maximum of 10 ms samples of the worker plus
descendant-process RSS during each labeled interval. Cold worker initialization
was measured separately.

PDFium classifies the official `89/2026/TT-BTC` PDF as **scan**: all 839
physical pages are image-bearing and PDFium exposes no text-layer characters.
This differs from the manifest's original `mixed` classification, and the
generated report records that mismatch. Deterministic pages 1, 420, and 839
were sampled. They have no pinned human-verified
transcription, so they provide runtime/failure context only and are excluded
from CER/WER and the quality calculation.

The complete rerun also processed deterministic bounded pages 1–2 from
`Cứu Quốc` (5 September 1945) and pages 1, 4, and 8 from `Đại Nam Đăng Cổ
Tùng Báo` (13 June 1907) for every candidate. No trustworthy transcription was
available, so these historical pages are qualitative/runtime evidence only;
the report makes no CER/WER claim for them.

The deterministic source-ground-truth two-column case uses six reviewed anchor
positions in column-major order. Both Tesseract configurations observed all six
with `0/15` pairwise reading-order violations; PP-OCRv6 observed all six with
`3/15` violations.

The existing pinned 1907 Wikimedia scan adds a limited qualitative check on
page 4 using five human-reviewed short heading anchors in expected
column-major sequence. Both Tesseract configurations observed all five with
`0/10` violations. PP-OCRv6 observed three with `0/3` violations and two
missing anchors. Matching folds accents/punctuation and permits at most 25%
character edits to tolerate OCR noise. This is not a transcription, CER
sample, or general layout score. Missing anchors are reported separately and
recognized OCR text is not stored in the report.

Phase B was correctly not run after the STOP decision. This spike does not
adopt PP-OCRv6 or this Python service in production; any future production
integration requires separate evidence and review.

## Dependency and license notes

- PaddleOCR, PaddlePaddle, PaddleX, and the official pretrained OCR weights are
  Apache-2.0.
- pypdfium2 5.12.1 reports `BSD-3-Clause, Apache-2.0, dependency licenses`
  in installed package metadata. Upstream documents pypdfium2 as
  Apache-2.0/BSD-3-Clause and PDFium as BSD-style:
  <https://github.com/pypdfium2-team/pypdfium2/#licensing>.
  Binary redistribution must also ship the PDFium and bundled third-party
  notices from the wheel's `BUILD_LICENSES` material; exact build composition
  still requires release review.
- The resolved environment contains no package whose name indicates a CUDA,
  ROCm, GPU, TensorRT, or cuDNN-only runtime.
- Transitive licenses are predominantly Apache, BSD, MIT, MPL, and PSF.
  `python-bidi` is LGPL-3.0-or-later. PyMuPDF is no longer a direct or
  transitive locked dependency.
- Downloaded model assets and the `.venv` are runtime-only, ignored artifacts.
