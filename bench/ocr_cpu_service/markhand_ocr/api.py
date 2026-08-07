"""FastAPI transport for the bounded OCR benchmark service."""

from __future__ import annotations

from dataclasses import asdict, replace
from typing import Annotated

from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.exceptions import RequestValidationError
from starlette.datastructures import Headers
from starlette.responses import JSONResponse
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from .backend import OcrBackend
from .service import (
    BackendFailure,
    ConversionRejected,
    ConvertRequest,
    InvalidPdf,
    convert_pdf,
)


class _BodyLimitMiddleware:
    """Buffer a bounded body before multipart parsing and reject excess bytes."""

    def __init__(self, app: ASGIApp, *, max_body_bytes: int) -> None:
        self.app = app
        self.max_body_bytes = max_body_bytes

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        content_length = Headers(scope=scope).get("content-length")
        if content_length is not None:
            try:
                declared_length = int(content_length)
            except ValueError:
                await _json_error(400, "invalid request")(scope, receive, send)
                return
            if declared_length < 0:
                await _json_error(400, "invalid request")(scope, receive, send)
                return
            if declared_length > self.max_body_bytes:
                await _json_error(413, "request body too large")(scope, receive, send)
                return

        messages: list[Message] = []
        total_bytes = 0
        while True:
            message = await receive()
            messages.append(message)
            if message["type"] != "http.request":
                break
            total_bytes += len(message.get("body", b""))
            if total_bytes > self.max_body_bytes:
                await _json_error(413, "request body too large")(scope, receive, send)
                return
            if not message.get("more_body", False):
                break

        message_index = 0

        async def replay() -> Message:
            nonlocal message_index
            if message_index >= len(messages):
                return {"type": "http.disconnect"}
            message = messages[message_index]
            message_index += 1
            return message

        await self.app(scope, replay, send)


def _json_error(status_code: int, detail: str) -> JSONResponse:
    return JSONResponse(status_code=status_code, content={"detail": detail})


def create_app(
    backend: OcrBackend,
    *,
    limits: ConvertRequest | None = None,
    max_body_bytes: int | None = None,
) -> FastAPI:
    """Create an app whose OCR implementation is supplied by the caller."""
    app = FastAPI(title="Markhand OCR benchmark", docs_url=None, redoc_url=None)
    base_request = limits or ConvertRequest()
    body_limit = (
        max_body_bytes
        if max_body_bytes is not None
        else base_request.max_input_bytes + 1_000_000
    )
    if body_limit <= 0:
        raise ValueError("max_body_bytes must be positive")
    app.add_middleware(_BodyLimitMiddleware, max_body_bytes=body_limit)

    @app.exception_handler(RequestValidationError)
    async def invalid_request(
        request: Request, error: RequestValidationError
    ) -> JSONResponse:
        del request, error
        return _json_error(400, "invalid request")

    @app.get("/healthz")
    def health() -> dict[str, str]:
        return {"status": "ready", "backend": backend.name}

    @app.post("/v1/convert")
    def convert(
        file: Annotated[UploadFile, File()],
        pages: Annotated[list[int] | None, Form()] = None,
        dpi: Annotated[int | None, Form()] = None,
    ) -> dict[str, object]:
        if file.content_type != "application/pdf":
            file.file.close()
            raise HTTPException(status_code=415, detail="unsupported media type")

        try:
            data = file.file.read(base_request.max_input_bytes + 1)
        finally:
            file.file.close()

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
        except BackendFailure as error:
            raise HTTPException(status_code=502, detail="OCR backend failed") from error
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
