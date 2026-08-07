#!/usr/bin/env bash
set -euo pipefail

service_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
venv_dir="${MARKHAND_OCR_VENV:-${service_dir}/.venv}"

if [[ "${MARKHAND_OCR_BACKEND:-}" != "paddle" ]]; then
  echo "MARKHAND_OCR_BACKEND must be set to paddle" >&2
  exit 2
fi
if [[ ! -x "${venv_dir}/bin/python" ]]; then
  echo "OCR virtual environment is missing: ${venv_dir}" >&2
  exit 2
fi

export PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True
exec "${venv_dir}/bin/python" -m uvicorn \
  --app-dir "${service_dir}" \
  --factory markhand_ocr.paddle_backend:create_runtime_app \
  --host "${MARKHAND_OCR_HOST:-127.0.0.1}" \
  --port "${MARKHAND_OCR_PORT:-8765}" \
  --workers 1
