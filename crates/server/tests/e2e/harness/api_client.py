"""Minimal `/api/v1` HTTP client (stdlib urllib). Never logs secrets."""

from __future__ import annotations

import json
import mimetypes
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib import error, request


@dataclass
class HttpResult:
    status: int
    headers: dict[str, str]
    body: bytes

    def json(self) -> Any:
        if not self.body:
            return None
        return json.loads(self.body.decode("utf-8"))


class ApiClient:
    def __init__(self, base_url: str, *, timeout_secs: float = 30.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout_secs = timeout_secs
        self.access_token: str | None = None

    def _headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        headers = {"Accept": "application/json", "X-Request-Id": str(uuid.uuid4())}
        if self.access_token:
            headers["Authorization"] = f"Bearer {self.access_token}"
        if extra:
            headers.update(extra)
        return headers

    def request(
        self,
        method: str,
        path: str,
        *,
        body: bytes | None = None,
        headers: dict[str, str] | None = None,
        content_type: str | None = None,
    ) -> HttpResult:
        url = f"{self.base_url}{path}"
        hdrs = self._headers(headers)
        if content_type:
            hdrs["Content-Type"] = content_type
        req = request.Request(url, data=body, headers=hdrs, method=method.upper())
        try:
            with request.urlopen(req, timeout=self.timeout_secs) as resp:
                return HttpResult(
                    status=resp.getcode(),
                    headers={k.lower(): v for k, v in resp.headers.items()},
                    body=resp.read(),
                )
        except error.HTTPError as exc:
            return HttpResult(
                status=exc.code,
                headers={k.lower(): v for k, v in exc.headers.items()} if exc.headers else {},
                body=exc.read() if exc.fp else b"",
            )

    def login(self, email: str, password: str) -> HttpResult:
        payload = json.dumps({"email": email, "password": password}).encode("utf-8")
        result = self.request(
            "POST",
            "/api/v1/auth/login",
            body=payload,
            content_type="application/json",
        )
        if result.status == 200:
            data = result.json()
            token = data.get("accessToken")
            if isinstance(token, str) and token:
                self.access_token = token
        return result

    def get(self, path: str) -> HttpResult:
        return self.request("GET", path)

    def post_json(self, path: str, payload: dict[str, Any]) -> HttpResult:
        body = json.dumps(payload).encode("utf-8")
        return self.request(
            "POST",
            path,
            body=body,
            content_type="application/json",
        )

    def delete(self, path: str) -> HttpResult:
        return self.request("DELETE", path)

    def upload(
        self,
        file_path: Path,
        *,
        collection_id: str | None = None,
        filename: str | None = None,
        idempotency_key: str | None = None,
    ) -> HttpResult:
        boundary = f"----markhand{uuid.uuid4().hex}"
        name = filename or file_path.name
        content = file_path.read_bytes()
        content_type = mimetypes.guess_type(name)[0] or "application/octet-stream"
        parts: list[bytes] = []
        if collection_id:
            parts.append(
                (
                    f"--{boundary}\r\n"
                    f'Content-Disposition: form-data; name="collectionId"\r\n\r\n'
                    f"{collection_id}\r\n"
                ).encode("utf-8")
            )
        parts.append(
            (
                f"--{boundary}\r\n"
                f'Content-Disposition: form-data; name="file"; filename="{name}"\r\n'
                f"Content-Type: {content_type}\r\n\r\n"
            ).encode("utf-8")
            + content
            + b"\r\n"
        )
        parts.append(f"--{boundary}--\r\n".encode("utf-8"))
        body = b"".join(parts)
        headers = {}
        if idempotency_key:
            headers["Idempotency-Key"] = idempotency_key
        return self.request(
            "POST",
            "/api/v1/uploads",
            body=body,
            headers=headers,
            content_type=f"multipart/form-data; boundary={boundary}",
        )

    def health_ready(self) -> HttpResult:
        return self.get("/api/v1/health/ready")