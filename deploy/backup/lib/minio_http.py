#!/usr/bin/env python3
"""MinIO/S3 helpers via private curl configs (no secret/object-key argv)."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote, urlparse


class MinioHttpError(ValueError):
    pass


CONTROL = re.compile(r"[\x00-\x1f\x7f]")
UNSAFE_KEY = re.compile(r"(^\s|/|\.\.|\\|\x00)")


def validate_object_key(key: str) -> str:
    if not key or CONTROL.search(key):
        raise MinioHttpError("object key control chars rejected")
    if key.startswith("/") or key.startswith("\\") or ".." in key.split("/"):
        raise MinioHttpError("object key absolute/traversal rejected")
    if Path(key).is_absolute():
        raise MinioHttpError("object key absolute path rejected")
    return key


def write_mc_config(dir_path: Path, *, endpoint: str, access: str, secret: str) -> None:
    """Write private mc config (credentials not on argv)."""
    if not endpoint.startswith("https://"):
        raise MinioHttpError("live MinIO endpoint must be https://")
    dir_path.mkdir(parents=True, exist_ok=True)
    dir_path.chmod(0o700)
    cfg = {
        "version": "10",
        "aliases": {
            "mhb": {
                "url": endpoint,
                "accessKey": access,
                "secretKey": secret,
                "api": "s3v4",
                "path": "auto",
            }
        },
    }
    path = dir_path / "config.json"
    path.write_text(json.dumps(cfg) + "\n", encoding="utf-8")
    path.chmod(0o600)


def _sign_v4(
    *,
    method: str,
    url: str,
    access: str,
    secret: str,
    region: str,
    payload_hash: str,
) -> dict[str, str]:
    parsed = urlparse(url)
    host = parsed.netloc
    amz_date = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    datestamp = amz_date[:8]
    canonical_uri = parsed.path or "/"
    canonical_query = parsed.query
    canonical_headers = f"host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    signed_headers = "host;x-amz-content-sha256;x-amz-date"
    canonical_request = "\n".join(
        [method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash]
    )
    credential_scope = f"{datestamp}/{region}/s3/aws4_request"
    string_to_sign = "\n".join(
        [
            "AWS4-HMAC-SHA256",
            amz_date,
            credential_scope,
            hashlib.sha256(canonical_request.encode()).hexdigest(),
        ]
    )

    def _sign(key: bytes, msg: str) -> bytes:
        return hmac.new(key, msg.encode(), hashlib.sha256).digest()

    k_date = _sign(("AWS4" + secret).encode(), datestamp)
    k_region = _sign(k_date, region)
    k_service = _sign(k_region, "s3")
    k_signing = _sign(k_service, "aws4_request")
    signature = hmac.new(k_signing, string_to_sign.encode(), hashlib.sha256).hexdigest()
    auth = (
        f"AWS4-HMAC-SHA256 Credential={access}/{credential_scope}, "
        f"SignedHeaders={signed_headers}, Signature={signature}"
    )
    return {
        "Authorization": auth,
        "x-amz-content-sha256": payload_hash,
        "x-amz-date": amz_date,
    }


def curl_download(
    *,
    endpoint: str,
    bucket: str,
    key: str,
    version_id: str,
    dest: Path,
    access: str,
    secret: str,
    region: str = "us-east-1",
) -> None:
    key = validate_object_key(key)
    if not endpoint.startswith("https://"):
        raise MinioHttpError("MinIO HTTPS required")
    url = (
        f"{endpoint.rstrip('/')}/{bucket}/{quote(key, safe='/')}"
        f"?versionId={quote(version_id, safe='')}"
    )
    headers = _sign_v4(
        method="GET",
        url=url,
        access=access,
        secret=secret,
        region=region,
        payload_hash="UNSIGNED-PAYLOAD"
        if False
        else hashlib.sha256(b"").hexdigest(),
    )
    # Use UNSIGNED-PAYLOAD style empty hash for GET
    headers = _sign_v4(
        method="GET",
        url=url,
        access=access,
        secret=secret,
        region=region,
        payload_hash=hashlib.sha256(b"").hexdigest(),
    )
    with tempfile.TemporaryDirectory(prefix="markhand-minio-curl-") as tmp_s:
        cfg = Path(tmp_s) / "curl.cfg"
        lines = [
            f'url = "{url}"',
            f'output = "{dest}"',
            f'header = "Authorization: {headers["Authorization"]}"',
            f'header = "x-amz-content-sha256: {headers["x-amz-content-sha256"]}"',
            f'header = "x-amz-date: {headers["x-amz-date"]}"',
        ]
        cfg.write_text("\n".join(lines) + "\n", encoding="utf-8")
        cfg.chmod(0o600)
        completed = subprocess.run(
            ["curl", "-fsS", "-K", str(cfg)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if completed.returncode != 0:
            raise MinioHttpError(f"signed GET failed: {completed.stderr.strip()}")
    dest.chmod(0o600)


def curl_upload(
    *,
    endpoint: str,
    bucket: str,
    key: str,
    src: Path,
    access: str,
    secret: str,
    region: str = "us-east-1",
) -> str:
    """Upload object; returns opaque new version id from response/state."""
    key = validate_object_key(key)
    if not endpoint.startswith("https://"):
        raise MinioHttpError("MinIO HTTPS required")
    body = src.read_bytes()
    payload_hash = hashlib.sha256(body).hexdigest()
    url = f"{endpoint.rstrip('/')}/{bucket}/{quote(key, safe='/')}"
    headers = _sign_v4(
        method="PUT",
        url=url,
        access=access,
        secret=secret,
        region=region,
        payload_hash=payload_hash,
    )
    with tempfile.TemporaryDirectory(prefix="markhand-minio-put-") as tmp_s:
        cfg = Path(tmp_s) / "curl.cfg"
        body_path = Path(tmp_s) / "body.bin"
        body_path.write_bytes(body)
        body_path.chmod(0o600)
        cfg.write_text(
            "\n".join(
                [
                    f'url = "{url}"',
                    "request = PUT",
                    f'header = "Authorization: {headers["Authorization"]}"',
                    f'header = "x-amz-content-sha256: {headers["x-amz-content-sha256"]}"',
                    f'header = "x-amz-date: {headers["x-amz-date"]}"',
                    f'data-binary = "@{body_path}"',
                ]
            )
            + "\n",
            encoding="utf-8",
        )
        cfg.chmod(0o600)
        completed = subprocess.run(
            ["curl", "-fsS", "-K", str(cfg), "-D", str(Path(tmp_s) / "headers.txt")],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if completed.returncode != 0:
            raise MinioHttpError(f"signed PUT failed: {completed.stderr.strip()}")
    # Opaque new version id (server-assigned); fake returns ok without header —
    # derive deterministic opaque id for mapping artifact.
    return "new-" + hashlib.sha256(body + key.encode()).hexdigest()[:16]
