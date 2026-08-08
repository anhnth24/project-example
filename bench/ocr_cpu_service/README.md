# Markhand CPU OCR benchmark archive

This directory now contains benchmark-only tooling: the pinned corpus
downloader, bounded renderer, model-neutral command candidate runner, metrics,
report generator, retained tests, and archived Phase A reports. The rejected
FastAPI service, Paddle runtime, model integration, launcher, and their tests
were intentionally removed after review.

The archive is evidence, not a supported service or live-model integration.
`reports/phase-a.json` and `reports/phase-a.md` preserve the reviewed PP-OCRv6
experiment and its **STOP** decision unchanged.

## Setup

Run from the repository root on CPython 3.12/Linux x86_64:

```bash
python3.12 -m venv bench/ocr_cpu_service/.venv
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --require-hashes -r bench/ocr_cpu_service/bootstrap-requirements.txt
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  -r bench/ocr_cpu_service/pylock.toml
bench/ocr_cpu_service/.venv/bin/python -m pip install \
  --no-build-isolation --no-deps -e bench/ocr_cpu_service
```

`pylock.toml` is a platform-specific pip lock with exact wheel URLs and
SHA-256 hashes. It contains Pillow, psutil, pypdfium2, pytest, packages
required by pytest, and the separately declared setuptools build backend.
Setuptools is installed only through the `build` extra so it is not a runtime
dependency. `bootstrap-requirements.txt` independently pins and hashes the pip
version needed to consume the native lock. Regenerate the dependency lock on
the declared target with:

```bash
bench/ocr_cpu_service/.venv/bin/python -m pip lock --only-deps \
  'bench/ocr_cpu_service[build,test]' \
  -o bench/ocr_cpu_service/pylock.toml
```

## Restore the retained corpus

The downloaded corpus remains at the ignored path
`bench/ocr_cpu_service/.data/corpus/`. Cleanup does not delete it. Restore or
checksum-verify the pinned inputs with:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -c \
'from pathlib import Path; from corpus.download import download_sources; items = download_sources(Path("bench/ocr_cpu_service/corpus/sources.json"), Path("bench/ocr_cpu_service/.data/corpus")); print(f"verified {len(items)} sources, {sum(item.bytes_downloaded for item in items)} bytes")'
```

The downloader enforces the checked-in hostname allowlist, public DNS
resolution, TLS hostname verification, redirect revalidation, byte limits,
whole-download deadlines, and SHA-256 checks.

## Build and run the retained baseline

Build the project-owned baseline:

```bash
CC=gcc CXX=g++ cargo build --release \
  -p fileconv-cli --no-default-features
```

Run the bounded benchmark and render a report:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m benchmark.run \
  --output bench/ocr_cpu_service/.data/benchmark/latest.json \
  --manifest bench/ocr_cpu_service/corpus/sources.json \
  --corpus-dir bench/ocr_cpu_service/.data/corpus \
  --work-dir bench/ocr_cpu_service/.data/benchmark \
  --fileconv target/release/fileconv \
  --system-tessdata /usr/share/tesseract-ocr/5/tessdata \
  --best-tessdata tessdata_best \
  --cpu-threads 8 \
  --max-rss-bytes 4294967296 \
  --max-output-bytes 1048576

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m benchmark.report \
  bench/ocr_cpu_service/.data/benchmark/latest.json
```

The retained candidate engine executes argv arrays without a shell, uses an
allowlisted environment, serializes candidates, separates cold initialization
from warm page timing, samples process-tree RSS, and kills timed-out process
groups. Candidate stdout and stderr each have a hard per-stream byte bound.
`--max-rss-bytes` is a measured gate applied to sampled process-tree RSS, not
an OS-enforced memory limit. It does not initialize or download models.

## Tests

The retained suite is local and requires neither a model cache nor network:

```bash
python3 -m pytest bench/ocr_cpu_service/tests -q
```

## Archived Phase A STOP evidence

The archived result is deliberately preserved:

- Sample: 12 pinned pages with human-verified text (9 real scans and 3
  synthetic receipts), plus bounded qualitative/runtime pages.
- Markhand default aggregate CER: `0.110828`.
- Markhand `tessdata_best` aggregate CER: `0.102603`.
- PP-OCRv6 aggregate CER: `0.399353`; real-scan CER: `0.433545`.
- The better Tesseract real-scan CER was `0.118708`, so PP-OCRv6's relative
  improvement was `-265.22%`, below the required `20%`.
- PP-OCRv6 also exceeded the 4 GiB sampled process-tree RSS bound on both
  bounded `Cứu Quốc` pages.
- Decision: **STOP**. Phase B was not run, and neither PP-OCRv6 nor the removed
  Python service was adopted.

These values describe the bounded archived sample only. They are not a
population estimate and claim no confidence interval. See
`reports/phase-a.md` and `reports/phase-a.json` for the complete immutable
evidence and measurement semantics.
