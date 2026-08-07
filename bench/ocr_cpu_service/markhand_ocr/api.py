"""FastAPI transport for the bounded OCR benchmark service."""

from __future__ import annotations

from dataclasses import asdict, replace
from typing import Annotated

from fastapi import FastAPI, File, Form, HTTPException, UploadFile

from .backend import OcrBackend
from .service import (
    ConversionRejected,
    ConvertRequest,
    InvalidPdf,
    convert_pdf,
)


def create_app(
    backend: OcrBackend, *, limits: ConvertRequest | None = None
) -> FastAPI:
    """Create an app whose OCR implementation is supplied by the caller."""
    app = FastAPI(title="Markhand OCR benchmark", docs_url=None, redoc_url=None)
    base_request = limits or ConvertRequest()

    @app.get("/healthz")
    def health() -> dict[str, str]:
        return {"status": "ready", "backend": backend.name}

    @app.post("/v1/convert")
    async def convert(
        file: Annotated[UploadFile, File()],
        pages: Annotated[list[int] | None, Form()] = None,
        dpi: Annotated[int | None, Form()] = None,
    ) -> dict[str, object]:
        if file.content_type != "application/pdf":
            await file.close()
            raise HTTPException(status_code=415, detail="unsupported media type")

        try:
            data = await file.read(base_request.max_input_bytes + 1)
        finally:
            await file.close()

        if len(data) > base_request.max_input_bytes:
            raise HTTPException(status_code=413, detail="conversion bounds exceeded")
        if not data.startswith(b"%PDF-"):
            raise HTTPException(status_code=400, detail="invalid PDF")

        request = replace(
            base_request,
            pages=pages if pages is not None else base_request.pages,
            dpi=dpi if dpi is not None else base_request.dpi,
        )
        try:
            result = convert_pdf(data, request, backend)
        except InvalidPdf as error:
            raise HTTPException(status_code=400, detail="invalid PDF") from error
        except ConversionRejected as error:
            status_code = (
                413
                if error.kind in {"payload", "page_limit", "render_bound"}
                else 422
            )
            raise HTTPException(
                status_code=status_code, detail="conversion bounds exceeded"
            ) from error
        except Exception as error:
            raise HTTPException(status_code=502, detail="OCR backend failed") from error

        return asdict(result)

    return app
