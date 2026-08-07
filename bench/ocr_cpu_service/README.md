# Markhand CPU OCR benchmark service

This isolated benchmark service renders bounded local PDFs and recognizes page
images through PaddleOCR's public Python API. It does not import or run MinerU.
The Paddle backend uses the current PP-OCRv6 defaults, explicitly selects CPU,
and disables the optional orientation and unwarping models.

## Install

From `bench/ocr_cpu_service`:

```bash
python3 -m venv .venv
.venv/bin/python -m pip install --upgrade pip
.venv/bin/python -m pip install \
  --extra-index-url https://www.paddlepaddle.org.cn/packages/stable/cpu/ \
  -e '.[test,model]'
```

`requirements.lock` records the complete environment resolved by that command.
For an exact reinstall, install the lock with the same CPU wheel index, then
install this package without resolving dependencies:

```bash
.venv/bin/python -m pip install \
  --extra-index-url https://www.paddlepaddle.org.cn/packages/stable/cpu/ \
  -r requirements.lock
.venv/bin/python -m pip install --no-deps -e .
```

PaddleOCR 3.7.0 installs with PaddlePaddle 3.3.1, but that runtime has a
confirmed CPU/oneDNN inference regression. The model extra therefore pins
PaddlePaddle 3.2.2, the latest upstream-recommended compatible CPU release.
PaddleX also requires NumPy below 2.4, so the latest compatible NumPy line is
constrained explicitly.

## Test and run

The fast suite does not initialize or download models:

```bash
.venv/bin/python -m pytest tests -q
```

The opt-in live test generates its own Vietnamese image. Model files are cached
outside the repository by PaddleOCR and are never committed:

```bash
MARKHAND_OCR_LIVE=1 .venv/bin/python -m pytest \
  tests/test_paddle_backend.py -m live -q
```

Start one service process, and therefore one initialized model pipeline:

```bash
MARKHAND_OCR_BACKEND=paddle scripts/run_service.sh
```

The default bind address is `127.0.0.1:8765`. Override it with
`MARKHAND_OCR_HOST` and `MARKHAND_OCR_PORT`. The health response reports only
the stable backend name (`paddle`), not model identifiers.

## Dependency and license notes

- PaddleOCR, PaddlePaddle, PaddleX, and the official pretrained OCR weights are
  Apache-2.0.
- The resolved environment contains no package whose name indicates a CUDA,
  ROCm, GPU, TensorRT, or cuDNN-only runtime.
- Transitive licenses are predominantly Apache, BSD, MIT, MPL, and PSF.
  `python-bidi` is LGPL-3.0-or-later. PyMuPDF retains its existing
  AGPL-3.0/commercial dual license and must be reviewed before distribution.
- Downloaded model assets and the `.venv` are runtime-only, ignored artifacts.
