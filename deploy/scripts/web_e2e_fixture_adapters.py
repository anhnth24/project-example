#!/usr/bin/env python3
"""Bounded subprocess, HTTP, MinIO, and Qdrant adapters for fixture tooling."""

from __future__ import annotations

import ctypes
import datetime as dt
import hashlib
import hmac
import json
import os
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol

from web_e2e_fixture_identity import (
    EffectiveConfig,
    FixtureError,
    FixtureProbeError,
    load_effective_config,
    validate_signature,
    validate_uuid,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
COMPOSE_FILE = REPO_ROOT / "deploy" / "dev" / "compose.yml"
QDRANT_COLLECTION_PREFIX = "markhand_chunks_"


@dataclass
class HttpResponse:
    status: int
    headers: dict[str, str]
    body: bytes


class Deadline:
    """One monotonic deadline shared by every operation in a command."""

    def __init__(self, timeout_secs: float, *, clock: Callable[[], float] = time.monotonic) -> None:
        if timeout_secs <= 0:
            raise FixtureError("timeout-secs must be > 0")
        self._clock = clock
        self._expires_at = clock() + timeout_secs

    def remaining(self) -> float:
        remaining = self._expires_at - self._clock()
        if remaining <= 0:
            raise FixtureError("overall fixture deadline exceeded")
        return remaining


class Commands(Protocol):
    def hash_password(self, password: str, *, timeout: float) -> str: ...

    def psql(
        self,
        sql: str,
        *,
        timeout: float,
        redact: list[str] | None = None,
    ) -> str: ...

    def http(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes | None = None,
        timeout: float,
    ) -> Any: ...

    def object_exists(self, key: str, *, timeout: float) -> bool: ...

    def qdrant_point_ids(
        self,
        collections: list[str],
        org_id: str,
        *,
        timeout: float,
    ) -> list[str]: ...

    def sleep(self, seconds: float) -> None: ...


def collection_name_for_signature(signature: str) -> str:
    return QDRANT_COLLECTION_PREFIX + validate_signature(signature)


def _hash_password_native(password: str) -> str:
    """Argon2id PHC hash without putting the password in argv or environment."""

    if not password:
        raise FixtureError("password hashing failed")
    try:
        library = ctypes.CDLL("libargon2.so.1")
        function = library.argon2id_hash_encoded
    except (OSError, AttributeError) as error:
        raise FixtureError("native Argon2 password hashing unavailable") from error
    function.argtypes = [
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    function.restype = ctypes.c_int
    password_bytes = password.encode("utf-8")
    salt = os.urandom(16)
    output = ctypes.create_string_buffer(256)
    result = function(
        2,
        19_456,
        1,
        password_bytes,
        len(password_bytes),
        salt,
        len(salt),
        32,
        output,
        len(output),
    )
    if result != 0:
        raise FixtureError("native Argon2 password hashing failed")
    encoded = output.value.decode("ascii", errors="strict")
    if not encoded.startswith("$argon2id$v=19$m=19456,t=2,p=1$"):
        raise FixtureError("native Argon2 password hashing returned invalid output")
    return encoded


class LiveCommands:
    """Compose PostgreSQL plus in-process authenticated storage probes."""

    def __init__(self, *, environ: Mapping[str, str] | None = None) -> None:
        self.environ = dict(environ or os.environ)
        self.repo_root = REPO_ROOT
        self.compose_file = COMPOSE_FILE
        self._config: EffectiveConfig | None = None

    def _effective_config(self) -> EffectiveConfig:
        if self._config is None:
            self._config = load_effective_config(self.environ)
        return self._config

    def hash_password(self, password: str, *, timeout: float = 30.0) -> str:
        if timeout <= 0:
            raise FixtureError("overall fixture deadline exceeded")
        value = _hash_password_native(password)
        if timeout <= 0:
            raise FixtureError("overall fixture deadline exceeded")
        return value

    def psql(
        self,
        sql: str,
        *,
        timeout: float = 30.0,
        redact: list[str] | None = None,
    ) -> str:
        _ = redact
        if not self.compose_file.is_file():
            raise FixtureError("compose/db unavailable")
        if timeout <= 0:
            raise FixtureError("overall fixture deadline exceeded")
        user = self.environ.get("MARKHAND_POSTGRES_USER", "markhand")
        database = self.environ.get("MARKHAND_POSTGRES_DB", "markhand")
        command = [
            "docker",
            "compose",
            "-f",
            str(self.compose_file),
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            user,
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-Atq",
            "-f",
            "-",
        ]
        timeout_ms = max(1, int(timeout * 1000))
        child_env = dict(self.environ)
        pg_options = child_env.get("PGOPTIONS", "")
        child_env["PGOPTIONS"] = (
            f"{pg_options} -c statement_timeout={timeout_ms} -c lock_timeout={timeout_ms}"
        ).strip()
        try:
            process = subprocess.run(
                command,
                cwd=str(self.repo_root),
                input=sql,
                capture_output=True,
                text=True,
                check=False,
                env=child_env,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as error:
            raise FixtureError("compose/db operation timed out") from error
        if process.returncode != 0:
            raise FixtureError("compose/db unavailable")
        return (process.stdout or "").strip()

    def http(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes | None = None,
        timeout: float,
    ) -> HttpResponse:
        if timeout <= 0:
            raise FixtureError("overall fixture deadline exceeded")
        request = urllib.request.Request(
            url,
            data=body,
            headers=headers or {},
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return HttpResponse(
                    status=int(response.status),
                    headers=dict(response.headers.items()),
                    body=response.read(),
                )
        except urllib.error.HTTPError as error:
            return HttpResponse(
                status=int(error.code),
                headers=dict(error.headers.items()) if error.headers else {},
                body=error.read(),
            )
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            raise FixtureError("api request failed") from error

    def _minio_settings(self) -> tuple[str, str, str, str, str, bool]:
        config = self._effective_config()
        endpoint = str(config.value("MARKHAND_MINIO_URL", "")).strip().rstrip("/")
        access_key = str(config.value("MARKHAND_MINIO_ACCESS_KEY", "")).strip()
        secret_key = str(config.value("MARKHAND_MINIO_SECRET_KEY", "")).strip()
        bucket = str(config.value("MARKHAND_MINIO_BUCKET", "markhand-documents")).strip()
        region = str(config.value("MARKHAND_MINIO_REGION", "us-east-1")).strip()
        path_style_raw = config.value("MARKHAND_MINIO_PATH_STYLE", True)
        if isinstance(path_style_raw, str):
            normalized = path_style_raw.strip().lower()
            if normalized not in {"1", "true", "yes", "on", "0", "false", "no", "off"}:
                raise FixtureError("invalid MinIO path-style setting")
            path_style = normalized in {"1", "true", "yes", "on"}
        else:
            path_style = bool(path_style_raw)
        if not endpoint or not access_key or not secret_key or not bucket or not region:
            raise FixtureProbeError("minio probe unavailable")
        parsed = urllib.parse.urlsplit(endpoint)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise FixtureProbeError("minio probe unavailable")
        return endpoint, access_key, secret_key, bucket, region, path_style

    def object_exists(self, key: str, *, timeout: float = 30.0) -> bool:
        if not key or key.startswith("/") or ".." in key.split("/"):
            raise FixtureError("invalid object key")
        endpoint, access_key, secret_key, bucket, region, path_style = self._minio_settings()
        parsed = urllib.parse.urlsplit(endpoint)
        encoded_key = urllib.parse.quote(key, safe="/-_.~")
        base_path = parsed.path.rstrip("/")
        if path_style:
            host = parsed.netloc
            canonical_uri = f"{base_path}/{urllib.parse.quote(bucket, safe='-_.~')}/{encoded_key}"
        else:
            host = f"{bucket}.{parsed.netloc}"
            canonical_uri = f"{base_path}/{encoded_key}"
        canonical_uri = canonical_uri or "/"
        now = dt.datetime.now(dt.timezone.utc)
        amz_date = now.strftime("%Y%m%dT%H%M%SZ")
        date_stamp = now.strftime("%Y%m%d")
        payload_hash = hashlib.sha256(b"").hexdigest()
        canonical_headers = (
            f"host:{host}\n"
            f"x-amz-content-sha256:{payload_hash}\n"
            f"x-amz-date:{amz_date}\n"
        )
        signed_headers = "host;x-amz-content-sha256;x-amz-date"
        canonical_request = "\n".join(
            ["HEAD", canonical_uri, "", canonical_headers, signed_headers, payload_hash]
        )
        credential_scope = f"{date_stamp}/{region}/s3/aws4_request"
        string_to_sign = "\n".join(
            [
                "AWS4-HMAC-SHA256",
                amz_date,
                credential_scope,
                hashlib.sha256(canonical_request.encode("utf-8")).hexdigest(),
            ]
        )

        def sign(key_bytes: bytes, value: str) -> bytes:
            return hmac.new(key_bytes, value.encode("utf-8"), hashlib.sha256).digest()

        signing_key = sign(
            sign(sign(sign(("AWS4" + secret_key).encode("utf-8"), date_stamp), region), "s3"),
            "aws4_request",
        )
        signature = hmac.new(
            signing_key, string_to_sign.encode("utf-8"), hashlib.sha256
        ).hexdigest()
        authorization = (
            f"AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, "
            f"SignedHeaders={signed_headers}, Signature={signature}"
        )
        url = urllib.parse.urlunsplit((parsed.scheme, host, canonical_uri, "", ""))
        response = self.http(
            "HEAD",
            url,
            headers={
                "authorization": authorization,
                "host": host,
                "x-amz-content-sha256": payload_hash,
                "x-amz-date": amz_date,
            },
            timeout=timeout,
        )
        if response.status in {200, 204}:
            return True
        if response.status == 404:
            return False
        raise FixtureProbeError("minio probe unavailable")

    def _qdrant_request(
        self,
        method: str,
        path: str,
        *,
        body: bytes | None,
        timeout: float,
    ) -> HttpResponse:
        config = self._effective_config()
        base = str(config.value("MARKHAND_QDRANT_URL", "")).strip().rstrip("/")
        if not base:
            raise FixtureProbeError("qdrant probe unavailable")
        headers = {"content-type": "application/json"}
        api_key = str(config.value("MARKHAND_QDRANT_API_KEY", "")).strip()
        if api_key:
            headers["api-key"] = api_key
        try:
            response = self.http(method, base + path, headers=headers, body=body, timeout=timeout)
        except FixtureError as error:
            raise FixtureProbeError("qdrant probe unavailable") from error
        return response

    def qdrant_point_ids(
        self,
        collections: list[str],
        org_id: str,
        *,
        timeout: float = 30.0,
    ) -> list[str]:
        org_id = validate_uuid(org_id, field="orgId")
        local_deadline = Deadline(timeout)
        response = self._qdrant_request(
            "GET", "/collections", body=None, timeout=local_deadline.remaining()
        )
        if response.status != 200:
            raise FixtureProbeError("qdrant probe unavailable")
        try:
            payload = json.loads(response.body.decode("utf-8"))
            listed = {
                item["name"]
                for item in payload["result"]["collections"]
                if isinstance(item, dict) and isinstance(item.get("name"), str)
            }
        except (KeyError, TypeError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise FixtureProbeError("qdrant probe returned invalid data") from error

        # PostgreSQL/config hints can disappear before a retry or verify-clean run.
        # Scan every server-owned collection whose full signature validates; the
        # mandatory org_id filter below keeps the probe run-scoped.
        _ = collections
        validated_collections: set[str] = set()
        for name in listed:
            if not name.startswith(QDRANT_COLLECTION_PREFIX):
                continue
            digest = name[len(QDRANT_COLLECTION_PREFIX) :]
            try:
                expected = collection_name_for_signature(digest)
            except FixtureError:
                continue
            if name == expected:
                validated_collections.add(name)
        point_ids: list[str] = []
        for collection in sorted(validated_collections):
            offset: str | None = None
            while True:
                request_payload: dict[str, Any] = {
                    "filter": {
                        "must": [{"key": "org_id", "match": {"value": org_id}}]
                    },
                    "limit": 256,
                    "with_payload": False,
                    "with_vector": False,
                }
                if offset is not None:
                    request_payload["offset"] = offset
                response = self._qdrant_request(
                    "POST",
                    f"/collections/{urllib.parse.quote(collection, safe='')}/points/scroll",
                    body=json.dumps(request_payload, separators=(",", ":")).encode("utf-8"),
                    timeout=local_deadline.remaining(),
                )
                if response.status != 200:
                    raise FixtureProbeError("qdrant probe unavailable")
                try:
                    payload = json.loads(response.body.decode("utf-8"))
                    result = payload["result"]
                    points = result["points"]
                    next_offset = result.get("next_page_offset")
                    if not isinstance(points, list):
                        raise TypeError
                    for point in points:
                        point_id = validate_uuid(str(point["id"]), field="vectorPointId")
                        point_ids.append(point_id)
                except (
                    KeyError,
                    TypeError,
                    UnicodeDecodeError,
                    json.JSONDecodeError,
                ) as error:
                    raise FixtureProbeError("qdrant probe returned invalid data") from error
                if next_offset is None:
                    break
                offset = validate_uuid(str(next_offset), field="vectorPointId")
        return sorted(set(point_ids))

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)
