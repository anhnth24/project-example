"""FastAPI transport for the bounded OCR benchmark service."""

from __future__ import annotations

import asyncio
import math
import threading
import time
from concurrent.futures import (
    Future,
    ThreadPoolExecutor,
    TimeoutError as FutureTimeout,
)
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


class AdmissionUnavailable(RuntimeError):
    """No conversion slot became available within the bounded wait."""


class ConversionDeadline(RuntimeError):
    """The response deadline elapsed while conversion remains admitted."""


class ConversionLease:
    """Hold one request slot through body handling, conversion, and response."""

    def __init__(
        self, admission: ConversionAdmission, *, request_deadline_seconds: float
    ) -> None:
        self._admission = admission
        self._deadline = time.monotonic() + request_deadline_seconds
        self._lock = threading.Lock()
        self._future: Future[object] | None = None
        self._request_finished = False
        self._released = False

    @property
    def remaining_seconds(self) -> float:
        return max(0.0, self._deadline - time.monotonic())

    def run(self, operation: object) -> object:
        if not callable(operation):
            raise TypeError("operation must be callable")
        with self._lock:
            if self._future is not None:
                raise RuntimeError("conversion lease already used")
            if self.remaining_seconds <= 0:
                raise ConversionDeadline("conversion deadline exceeded")
            try:
                future = self._admission._executor.submit(operation)
            except BaseException:
                self._request_finished = True
                self._release_locked()
                raise
            self._future = future
        future.add_done_callback(self._operation_finished)
        try:
            return future.result(timeout=self.remaining_seconds)
        except FutureTimeout as error:
            raise ConversionDeadline("conversion deadline exceeded") from error

    def finish_request(self) -> None:
        """Release now if idle/done, otherwise defer until conversion exits."""
        with self._lock:
            self._request_finished = True
            if self._future is None or self._future.done():
                self._release_locked()

    def _operation_finished(self, future: Future[object]) -> None:
        del future
        with self._lock:
            if self._request_finished:
                self._release_locked()

    def _release_locked(self) -> None:
        if not self._released:
            self._released = True
            self._admission._capacity.release()


class ConversionAdmission:
    """Admit one process-local request with a strictly bounded waiting queue."""

    def __init__(self, *, max_waiters: int = 1) -> None:
        if max_waiters < 0:
            raise ValueError("max_waiters must not be negative")
        self._capacity = threading.BoundedSemaphore(value=1)
        self._executor = ThreadPoolExecutor(
            max_workers=1,
            thread_name_prefix="markhand-ocr-conversion",
        )
        self._max_waiters = max_waiters
        self._waiting = 0
        self._waiting_lock = threading.Lock()

    @property
    def waiting_count(self) -> int:
        with self._waiting_lock:
            return self._waiting

    def acquire(
        self,
        *,
        acquisition_timeout_seconds: float,
        request_deadline_seconds: float,
    ) -> ConversionLease:
        if self._capacity.acquire(blocking=False):
            return ConversionLease(
                self, request_deadline_seconds=request_deadline_seconds
            )

        with self._waiting_lock:
            if self._waiting >= self._max_waiters:
                raise AdmissionUnavailable("conversion queue is full")
            self._waiting += 1
        try:
            acquired = self._capacity.acquire(
                timeout=acquisition_timeout_seconds
            )
        finally:
            with self._waiting_lock:
                self._waiting -= 1
        if not acquired:
            raise AdmissionUnavailable("conversion capacity unavailable")
        return ConversionLease(
            self, request_deadline_seconds=request_deadline_seconds
        )


_PROCESS_CONVERSION_ADMISSION = ConversionAdmission()


class _ConversionAdmissionMiddleware:
    """Acquire before ASGI body reads and enforce one whole-request deadline."""

    def __init__(
        self,
        app: ASGIApp,
        *,
        admission: ConversionAdmission,
        acquisition_timeout_seconds: float,
        request_deadline_seconds: float,
    ) -> None:
        self.app = app
        self.admission = admission
        self.acquisition_timeout_seconds = acquisition_timeout_seconds
        self.request_deadline_seconds = request_deadline_seconds

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if (
            scope["type"] != "http"
            or scope.get("method") != "POST"
            or scope.get("path") != "/v1/convert"
        ):
            await self.app(scope, receive, send)
            return

        try:
            lease = await asyncio.to_thread(
                self.admission.acquire,
                acquisition_timeout_seconds=self.acquisition_timeout_seconds,
                request_deadline_seconds=self.request_deadline_seconds,
            )
        except AdmissionUnavailable:
            await _json_error(503, "conversion capacity unavailable")(
                scope, receive, send
            )
            return

        state = scope.setdefault("state", {})
        state["conversion_lease"] = lease
        response_started = False

        async def receive_before_deadline() -> Message:
            remaining = lease.remaining_seconds
            if remaining <= 0:
                raise ConversionDeadline("conversion deadline exceeded")
            try:
                return await asyncio.wait_for(receive(), timeout=remaining)
            except TimeoutError as error:
                raise ConversionDeadline(
                    "conversion deadline exceeded"
                ) from error

        async def track_send(message: Message) -> None:
            nonlocal response_started
            if message["type"] == "http.response.start":
                response_started = True
            await send(message)

        try:
            await self.app(scope, receive_before_deadline, track_send)
        except ConversionDeadline:
            if response_started:
                raise
            await _json_error(504, "conversion deadline exceeded")(
                scope, receive, send
            )
        finally:
            lease.finish_request()


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
    admission: ConversionAdmission | None = None,
    acquisition_timeout_seconds: float = 0.1,
    conversion_deadline_seconds: float = 120.0,
) -> FastAPI:
    """Create an app whose OCR implementation is supplied by the caller."""
    app = FastAPI(title="Markhand OCR benchmark", docs_url=None, redoc_url=None)
    base_request = limits or ConvertRequest()
    conversion_admission = admission or _PROCESS_CONVERSION_ADMISSION
    for name, value in (
        ("acquisition_timeout_seconds", acquisition_timeout_seconds),
        ("conversion_deadline_seconds", conversion_deadline_seconds),
    ):
        if not math.isfinite(value) or value <= 0:
            raise ValueError(f"{name} must be finite and positive")
    body_limit = (
        max_body_bytes
        if max_body_bytes is not None
        else base_request.max_input_bytes + 1_000_000
    )
    if body_limit <= 0:
        raise ValueError("max_body_bytes must be positive")
    app.add_middleware(_BodyLimitMiddleware, max_body_bytes=body_limit)
    app.add_middleware(
        _ConversionAdmissionMiddleware,
        admission=conversion_admission,
        acquisition_timeout_seconds=acquisition_timeout_seconds,
        request_deadline_seconds=conversion_deadline_seconds,
    )

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
        http_request: Request,
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
            lease = http_request.scope["state"]["conversion_lease"]
            result = lease.run(
                lambda: convert_pdf(data, request, backend),
            )
        except ConversionDeadline as error:
            raise HTTPException(
                status_code=504, detail="conversion deadline exceeded"
            ) from error
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
