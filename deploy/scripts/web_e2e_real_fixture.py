#!/usr/bin/env python3
"""Dev/CI-only run-scoped fixture CLI for Markhand real web E2E (P2-20).

Creates and tears down a unique run namespace with runtime credentials and
resource IDs. Refuses production profile before any subprocess, network, or
write. Subprocess/HTTP/object/vector execution is injectable for hermetic tests.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import string
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Protocol

REPO_ROOT = Path(__file__).resolve().parents[2]
COMPOSE_FILE = REPO_ROOT / "deploy" / "dev" / "compose.yml"
KEY_DOMAIN = b"markhand-object-key-v1"

RUN_ID_RE = re.compile(
    r"^(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|e2e-[a-f0-9]{7,40}-\d+)$",
    re.IGNORECASE,
)
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)

SENSITIVE_MANIFEST_KEYS = {
    "adminpassword",
    "viewerpassword",
    "password",
    "passwordhash",
    "accesstoken",
    "refreshtoken",
    "objectkeys",
    "objectkey",
    "token",
    "secret",
}


class FixtureError(RuntimeError):
    """Fail-closed fixture error (message must never include secrets)."""


class Commands(Protocol):
    def hash_password(self, password: str) -> str: ...

    def psql(self, sql: str, *, redact: list[str] | None = None) -> str: ...

    def http(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes | None = None,
        timeout: float,
    ) -> Any: ...

    def object_exists(self, key: str) -> bool: ...

    def vector_exists(self, point_id: str) -> bool: ...

    def sleep(self, seconds: float) -> None: ...


@dataclass
class HttpResponse:
    status: int
    headers: dict[str, str]
    body: bytes


def monotonic() -> float:
    return time.monotonic()


def fixture_checksum(ids: list[str]) -> str:
    joined = "\n".join(sorted(ids))
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()


def opaque_identity(kind: str, identity: str) -> str:
    value = validate_uuid(identity, field=kind)
    hasher = hashlib.sha256()
    hasher.update(KEY_DOMAIN)
    hasher.update(kind.encode("utf-8"))
    hasher.update(uuid.UUID(value).bytes)
    return hasher.hexdigest()


def quarantine_object_key(org_id: str, object_id: str) -> str:
    org = validate_uuid(org_id, field="orgId")
    obj = validate_uuid(object_id, field="objectId")
    return f"quarantine/{opaque_identity('org', org)}/{uuid.UUID(obj).hex}"


def validate_run_id(run_id: str) -> str:
    value = (run_id or "").strip()
    if not RUN_ID_RE.fullmatch(value):
        raise FixtureError("invalid run-id")
    return value


def validate_uuid(value: str, *, field: str) -> str:
    text = (value or "").strip()
    if not UUID_RE.fullmatch(text):
        raise FixtureError(f"invalid {field}")
    return text.lower()


def is_production_profile(environ: Mapping[str, str]) -> bool:
    profile = (environ.get("MARKHAND_PROFILE") or "").strip().lower()
    return profile == "prod"


def refuse_production(environ: Mapping[str, str]) -> None:
    if is_production_profile(environ):
        raise FixtureError("refusing MARKHAND_PROFILE=prod")


def _sql_quote_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _sql_uuid(value: str, *, field: str) -> str:
    return _sql_quote_literal(validate_uuid(value, field=field))


def _atomic_write_json(path: Path, payload: dict[str, Any], *, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=path.name + ".", dir=str(path.parent))
    tmp_path = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(tmp_path, mode)
        os.replace(tmp_path, path)
        os.chmod(path, mode)
    finally:
        if tmp_path.exists():
            tmp_path.unlink(missing_ok=True)


def _remove_credentials(path: Path) -> None:
    try:
        if path.exists():
            path.unlink()
    except OSError as error:
        raise FixtureError("failed to remove credentials file") from error


def _load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise FixtureError("invalid json file") from error
    if not isinstance(payload, dict):
        raise FixtureError("invalid json object")
    return payload


def _generate_password(length: int = 24) -> str:
    alphabet = string.ascii_letters + string.digits
    return "".join(secrets.choice(alphabet) for _ in range(length))


def _slug_for_run(run_id: str) -> str:
    compact = re.sub(r"[^a-z0-9-]", "-", run_id.lower())
    compact = re.sub(r"-+", "-", compact).strip("-")
    if not compact:
        compact = "run"
    slug = f"e2e-{compact}"
    return slug[:63]


class LiveCommands:
    """Compose psql + local HTTP + optional MinIO/Qdrant probes."""

    def __init__(self, *, environ: Mapping[str, str] | None = None) -> None:
        self.environ = dict(environ or os.environ)
        self.repo_root = REPO_ROOT
        self.compose_file = COMPOSE_FILE

    def hash_password(self, password: str) -> str:
        cmd = [
            "cargo",
            "run",
            "-q",
            "-p",
            "fileconv-server",
            "--bin",
            "dev-hash-password",
            "--",
            password,
        ]
        proc = subprocess.run(
            cmd,
            cwd=str(self.repo_root),
            capture_output=True,
            text=True,
            check=False,
            env=dict(self.environ),
        )
        if proc.returncode != 0:
            raise FixtureError("dev-hash-password failed")
        hash_value = (proc.stdout or "").strip()
        if len(hash_value) < 8:
            raise FixtureError("dev-hash-password returned empty hash")
        return hash_value

    def psql(self, sql: str, *, redact: list[str] | None = None) -> str:
        _ = redact  # secrets stay out of logs; callers must not print sql
        if not self.compose_file.is_file():
            raise FixtureError("compose/db unavailable")
        user = self.environ.get("MARKHAND_POSTGRES_USER", "markhand")
        db = self.environ.get("MARKHAND_POSTGRES_DB", "markhand")
        cmd = [
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
            db,
            "-v",
            "ON_ERROR_STOP=1",
            "-tAc",
            sql,
        ]
        proc = subprocess.run(
            cmd,
            cwd=str(self.repo_root),
            capture_output=True,
            text=True,
            check=False,
            env=dict(self.environ),
        )
        if proc.returncode != 0:
            raise FixtureError("compose/db unavailable")
        return (proc.stdout or "").strip()

    def http(
        self,
        method: str,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes | None = None,
        timeout: float,
    ) -> HttpResponse:
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

    def object_exists(self, key: str) -> bool:
        # Best-effort live probe via compose `mc` against MinIO. Missing mc/stack
        # or missing object both count as absent (idempotent clean).
        if "/" not in key or ".." in key or key.startswith("/"):
            raise FixtureError("invalid object key")
        user = self.environ.get("MARKHAND_MINIO_USER", "markhand")
        password = self.environ.get("MARKHAND_MINIO_PASSWORD", "markhand_dev_only")
        bucket = self.environ.get("MARKHAND_MINIO_DOCUMENTS_BUCKET", "markhand-documents")
        cmd = [
            "docker",
            "compose",
            "-f",
            str(self.compose_file),
            "run",
            "--rm",
            "--no-deps",
            "--entrypoint",
            "mc",
            "-e",
            f"MC_HOST_local=http://{user}:{password}@minio:9000",
            "minio-init",
            "stat",
            f"local/{bucket}/{key}",
        ]
        proc = subprocess.run(
            cmd,
            cwd=str(self.repo_root),
            capture_output=True,
            text=True,
            check=False,
            env=dict(self.environ),
        )
        return proc.returncode == 0

    def vector_exists(self, point_id: str) -> bool:
        # Failed-document fixtures do not upsert vectors; live verify uses Qdrant only
        # when MARKHAND_QDRANT_COLLECTION is set. Default: treat unknown as absent.
        base = self.environ.get("MARKHAND_QDRANT_URL", "http://127.0.0.1:6333").rstrip("/")
        collection = self.environ.get("MARKHAND_QDRANT_COLLECTION", "").strip()
        if not collection:
            return False
        validate_uuid(point_id, field="vectorPointId")
        if not re.fullmatch(r"[a-zA-Z0-9_-]+", collection):
            raise FixtureError("invalid qdrant collection")
        url = f"{base}/collections/{collection}/points/{point_id}"
        try:
            response = self.http("GET", url, timeout=5.0)
        except FixtureError:
            return False
        return response.status == 200

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)


def _assert_manifest_public(payload: dict[str, Any]) -> None:
    for key in payload:
        if key.lower() in SENSITIVE_MANIFEST_KEYS:
            raise FixtureError("manifest contains sensitive key")


def cmd_setup(
    *,
    run_id: str,
    manifest_out: Path,
    credentials_out: Path,
    commands: Commands,
    environ: Mapping[str, str],
) -> None:
    refuse_production(environ)
    run_id = validate_run_id(run_id)
    if manifest_out.exists() or credentials_out.exists():
        # Allow overwrite only of prior outputs; still refuse prod above.
        pass

    admin_password = _generate_password()
    viewer_password = _generate_password()
    admin_hash = commands.hash_password(admin_password)
    viewer_hash = commands.hash_password(viewer_password)

    # Stable per-run identities so setup is idempotent for the same run-id.
    namespace = uuid.uuid5(uuid.NAMESPACE_URL, f"markhand-e2e-fixture:{run_id}")
    org_id = str(uuid.uuid5(namespace, "org"))
    admin_user_id = str(uuid.uuid5(namespace, "admin"))
    viewer_user_id = str(uuid.uuid5(namespace, "viewer"))
    collection_id = str(uuid.uuid5(namespace, "collection"))
    failed_document_id = str(uuid.uuid5(namespace, "failed-doc"))
    failed_version_id = str(uuid.uuid5(namespace, "failed-ver"))
    object_id = str(uuid.uuid5(namespace, "object"))
    vector_point_id = str(uuid.uuid5(namespace, "vector"))

    slug = _slug_for_run(run_id)
    collection_name = f"E2E Library {run_id}"
    collection_slug = _slug_for_run(f"{run_id}-library")
    admin_email = f"admin+{run_id.lower()}@example.test"
    viewer_email = f"viewer+{run_id.lower()}@example.test"
    object_key = quarantine_object_key(org_id, object_id)
    content_sha = hashlib.sha256(f"e2e-failed:{run_id}".encode("utf-8")).hexdigest()

    # Password hashes are bound only via escaped literals for psql (same pattern as
    # seed-dev-password.sh). Callers must pass redact= so recorded SQL never keeps
    # secrets; never print hashes/passwords/object keys.
    sql = f"""
BEGIN;
SET LOCAL row_security = off;
SET LOCAL app.org_id = {_sql_uuid(org_id, field='orgId')};

INSERT INTO orgs (id, slug, name)
VALUES (
  {_sql_uuid(org_id, field='orgId')},
  {_sql_quote_literal(slug)},
  {_sql_quote_literal(f'E2E Org {run_id}')}
)
ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, updated_at = now();

SELECT provision_org_role_catalog({_sql_uuid(org_id, field='orgId')});

INSERT INTO org_quotas (
  org_id, max_storage_bytes, max_documents, max_concurrent_jobs, max_monthly_tokens
) VALUES (
  {_sql_uuid(org_id, field='orgId')}, 10737418240, 10000, 8, 5000000
) ON CONFLICT (org_id) DO NOTHING;

INSERT INTO users (id, email, display_name, password_hash)
VALUES (
  {_sql_uuid(admin_user_id, field='adminUserId')},
  {_sql_quote_literal(admin_email)},
  {_sql_quote_literal(f'E2E Admin {run_id}')},
  {_sql_quote_literal(admin_hash)}
)
ON CONFLICT (id) DO UPDATE
SET email = EXCLUDED.email,
    display_name = EXCLUDED.display_name,
    password_hash = EXCLUDED.password_hash,
    updated_at = now();

INSERT INTO users (id, email, display_name, password_hash)
VALUES (
  {_sql_uuid(viewer_user_id, field='viewerUserId')},
  {_sql_quote_literal(viewer_email)},
  {_sql_quote_literal(f'E2E Viewer {run_id}')},
  {_sql_quote_literal(viewer_hash)}
)
ON CONFLICT (id) DO UPDATE
SET email = EXCLUDED.email,
    display_name = EXCLUDED.display_name,
    password_hash = EXCLUDED.password_hash,
    updated_at = now();

INSERT INTO org_memberships (org_id, user_id, role, state)
VALUES
  (
    {_sql_uuid(org_id, field='orgId')},
    {_sql_uuid(admin_user_id, field='adminUserId')},
    'admin',
    'active'
  ),
  (
    {_sql_uuid(org_id, field='orgId')},
    {_sql_uuid(viewer_user_id, field='viewerUserId')},
    'viewer',
    'active'
  )
ON CONFLICT (org_id, user_id) DO UPDATE
SET role = EXCLUDED.role, state = 'active';

INSERT INTO collections (
  id, org_id, name, slug, description, owner_user_id, visibility
) VALUES (
  {_sql_uuid(collection_id, field='collectionId')},
  {_sql_uuid(org_id, field='orgId')},
  {_sql_quote_literal(collection_name)},
  {_sql_quote_literal(collection_slug)},
  {_sql_quote_literal('Run-scoped E2E collection')},
  {_sql_uuid(admin_user_id, field='adminUserId')},
  'org'
)
ON CONFLICT (id) DO UPDATE
SET name = EXCLUDED.name, updated_at = now(), deleted_at = NULL;

INSERT INTO documents (
  id, org_id, collection_id, title, state, created_by_user_id
) VALUES (
  {_sql_uuid(failed_document_id, field='failedDocumentId')},
  {_sql_uuid(org_id, field='orgId')},
  {_sql_uuid(collection_id, field='collectionId')},
  {_sql_quote_literal(f'E2E Failed {run_id}')},
  'uploaded',
  {_sql_uuid(admin_user_id, field='adminUserId')}
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO document_versions (
  id, org_id, document_id, version_number, parent_version_id,
  publication_state, is_current, content_sha256, original_object_key,
  source_filename, source_content_type, byte_size,
  effective_from, created_by_user_id
) VALUES (
  {_sql_uuid(failed_version_id, field='failedVersionId')},
  {_sql_uuid(org_id, field='orgId')},
  {_sql_uuid(failed_document_id, field='failedDocumentId')},
  1,
  NULL,
  'draft',
  false,
  {_sql_quote_literal(content_sha)},
  {_sql_quote_literal(object_key)},
  {_sql_quote_literal('e2e-failed.txt')},
  {_sql_quote_literal('text/plain')},
  12,
  clock_timestamp() - interval '1 hour',
  {_sql_uuid(admin_user_id, field='adminUserId')}
)
ON CONFLICT (id) DO NOTHING;

SELECT CASE
  WHEN EXISTS (
    SELECT 1 FROM document_versions
    WHERE id = {_sql_uuid(failed_version_id, field='failedVersionId')}
      AND publication_state = 'draft'
  ) THEN markhand_publish_document_version(
    {_sql_uuid(org_id, field='orgId')},
    {_sql_uuid(failed_document_id, field='failedDocumentId')},
    {_sql_uuid(failed_version_id, field='failedVersionId')}
  )::text
  ELSE 'already-published'
END;

UPDATE documents
SET state = 'failed',
    current_version_id = COALESCE(
      current_version_id,
      {_sql_uuid(failed_version_id, field='failedVersionId')}
    ),
    updated_at = now()
WHERE id = {_sql_uuid(failed_document_id, field='failedDocumentId')}
  AND org_id = {_sql_uuid(org_id, field='orgId')};

COMMIT;

SELECT json_build_object(
  'orgId', {_sql_uuid(org_id, field='orgId')}::text,
  'adminUserId', {_sql_uuid(admin_user_id, field='adminUserId')}::text,
  'viewerUserId', {_sql_uuid(viewer_user_id, field='viewerUserId')}::text,
  'collectionId', {_sql_uuid(collection_id, field='collectionId')}::text,
  'failedDocumentId', {_sql_uuid(failed_document_id, field='failedDocumentId')}::text,
  'failedVersionId', {_sql_uuid(failed_version_id, field='failedVersionId')}::text,
  'objectId', {_sql_uuid(object_id, field='objectId')}::text,
  'vectorPointId', {_sql_uuid(vector_point_id, field='vectorPointId')}::text
)::text AS fixture_json;
"""

    try:
        raw = commands.psql(
            sql,
            redact=[admin_password, viewer_password, admin_hash, viewer_hash, object_key],
        )
    except FixtureError:
        raise
    except Exception as error:  # pragma: no cover - defensive
        raise FixtureError("compose/db unavailable") from error

    if not raw:
        raise FixtureError("fixture setup returned empty result")
    try:
        created = json.loads(raw.splitlines()[-1])
    except json.JSONDecodeError as error:
        raise FixtureError("fixture setup returned invalid result") from error

    for field_name in (
        "orgId",
        "adminUserId",
        "viewerUserId",
        "collectionId",
        "failedDocumentId",
        "failedVersionId",
        "objectId",
        "vectorPointId",
    ):
        validate_uuid(str(created[field_name]), field=field_name)

    checksum = fixture_checksum(
        [
            str(created["orgId"]),
            str(created["adminUserId"]),
            str(created["viewerUserId"]),
            str(created["collectionId"]),
            str(created["failedDocumentId"]),
            str(created["failedVersionId"]),
            str(created["objectId"]),
            str(created["vectorPointId"]),
        ]
    )
    manifest = {
        "runId": run_id,
        "orgId": str(created["orgId"]),
        "adminUserId": str(created["adminUserId"]),
        "viewerUserId": str(created["viewerUserId"]),
        "collectionId": str(created["collectionId"]),
        "collectionName": collection_name,
        "failedDocumentId": str(created["failedDocumentId"]),
        "failedVersionId": str(created["failedVersionId"]),
        "objectIds": [str(created["objectId"])],
        "vectorPointIds": [str(created["vectorPointId"])],
        "checksum": checksum,
    }
    _assert_manifest_public(manifest)

    credentials = {
        "runId": run_id,
        "adminEmail": admin_email,
        "adminPassword": admin_password,
        "viewerEmail": viewer_email,
        "viewerPassword": viewer_password,
        "objectKeys": [object_key],
        "vectorPointIds": [str(created["vectorPointId"])],
    }

    try:
        _atomic_write_json(credentials_out, credentials, mode=0o600)
        _atomic_write_json(manifest_out, manifest, mode=0o644)
        mode = credentials_out.stat().st_mode & 0o777
        if mode != 0o600:
            raise FixtureError("credentials file mode must be 0600")
    except Exception:
        _remove_credentials(credentials_out)
        if manifest_out.exists():
            manifest_out.unlink(missing_ok=True)
        raise


def _manifest_ids(manifest: dict[str, Any]) -> dict[str, Any]:
    run_id = validate_run_id(str(manifest.get("runId", "")))
    org_id = validate_uuid(str(manifest.get("orgId", "")), field="orgId")
    admin_user_id = validate_uuid(str(manifest.get("adminUserId", "")), field="adminUserId")
    viewer_user_id = validate_uuid(str(manifest.get("viewerUserId", "")), field="viewerUserId")
    collection_id = validate_uuid(str(manifest.get("collectionId", "")), field="collectionId")
    failed_document_id = validate_uuid(
        str(manifest.get("failedDocumentId", "")), field="failedDocumentId"
    )
    failed_version_id = validate_uuid(
        str(manifest.get("failedVersionId", "")), field="failedVersionId"
    )
    object_ids = [
        validate_uuid(str(item), field="objectId") for item in (manifest.get("objectIds") or [])
    ]
    vector_point_ids = [
        validate_uuid(str(item), field="vectorPointId")
        for item in (manifest.get("vectorPointIds") or [])
    ]
    expected = fixture_checksum(
        [
            org_id,
            admin_user_id,
            viewer_user_id,
            collection_id,
            failed_document_id,
            failed_version_id,
            *object_ids,
            *vector_point_ids,
        ]
    )
    checksum = str(manifest.get("checksum", ""))
    if checksum != expected:
        raise FixtureError("manifest checksum mismatch")
    return {
        "runId": run_id,
        "orgId": org_id,
        "adminUserId": admin_user_id,
        "viewerUserId": viewer_user_id,
        "collectionId": collection_id,
        "failedDocumentId": failed_document_id,
        "failedVersionId": failed_version_id,
        "objectIds": object_ids,
        "vectorPointIds": vector_point_ids,
        "collectionName": str(manifest.get("collectionName", "")),
        "checksum": checksum,
    }


def _count_sql(commands: Commands, sql: str) -> int:
    raw = (commands.psql(sql) or "0").strip() or "0"
    try:
        return int(raw.splitlines()[-1])
    except ValueError as error:
        raise FixtureError("invalid psql count") from error


def collect_leaks(
    *,
    ids: dict[str, Any],
    commands: Commands,
) -> dict[str, list[str]]:
    org_id = ids["orgId"]
    leaks: dict[str, list[str]] = {
        "orgIds": [],
        "userIds": [],
        "collectionIds": [],
        "documentIds": [],
        "objectIds": [],
        "vectorPointIds": [],
    }

    if _count_sql(
        commands,
        f"SELECT count(*) FROM orgs WHERE id = {_sql_uuid(org_id, field='orgId')}",
    ):
        leaks["orgIds"].append(org_id)

    for user_id, field_name in (
        (ids["adminUserId"], "adminUserId"),
        (ids["viewerUserId"], "viewerUserId"),
    ):
        if _count_sql(
            commands,
            f"SELECT count(*) FROM users WHERE id = {_sql_uuid(user_id, field=field_name)}",
        ):
            leaks["userIds"].append(user_id)

    if _count_sql(
        commands,
        "SELECT count(*) FROM collections WHERE id = "
        f"{_sql_uuid(ids['collectionId'], field='collectionId')}",
    ):
        leaks["collectionIds"].append(ids["collectionId"])

    if _count_sql(
        commands,
        "SELECT count(*) FROM documents WHERE id = "
        f"{_sql_uuid(ids['failedDocumentId'], field='failedDocumentId')}",
    ):
        leaks["documentIds"].append(ids["failedDocumentId"])

    for object_id in ids["objectIds"]:
        key = quarantine_object_key(org_id, object_id)
        if commands.object_exists(key):
            leaks["objectIds"].append(object_id)

    for point_id in ids["vectorPointIds"]:
        if commands.vector_exists(point_id):
            leaks["vectorPointIds"].append(point_id)

    # Drop empty lists for stable reports.
    return {key: values for key, values in leaks.items() if values}


def _login(commands: Commands, api_base: str, email: str, password: str, timeout: float) -> str:
    url = f"{api_base.rstrip('/')}/api/v1/auth/login"
    body = json.dumps({"email": email, "password": password}).encode("utf-8")
    response = commands.http(
        "POST",
        url,
        headers={"content-type": "application/json"},
        body=body,
        timeout=min(timeout, 30.0),
    )
    if response.status != 200:
        raise FixtureError("login failed")
    try:
        payload = json.loads(response.body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FixtureError("login returned invalid json") from error
    token = payload.get("accessToken")
    if not isinstance(token, str) or not token:
        raise FixtureError("login missing access token")
    return token


def _request_document_delete(
    commands: Commands,
    *,
    api_base: str,
    token: str,
    document_id: str,
    timeout: float,
) -> None:
    url = f"{api_base.rstrip('/')}/api/v1/documents/{validate_uuid(document_id, field='documentId')}"
    response = commands.http(
        "DELETE",
        url,
        headers={"authorization": f"Bearer {token}"},
        body=None,
        timeout=min(timeout, 30.0),
    )
    if response.status not in {204, 404}:
        raise FixtureError("document delete request failed")


def _wait_for_document_cleanup(
    *,
    ids: dict[str, Any],
    commands: Commands,
    timeout_secs: float,
) -> None:
    deadline = monotonic() + timeout_secs
    document_id = ids["failedDocumentId"]
    org_id = ids["orgId"]
    while True:
        remaining_docs = _count_sql(
            commands,
            "SELECT count(*) FROM documents WHERE id = "
            f"{_sql_uuid(document_id, field='failedDocumentId')} "
            f"AND org_id = {_sql_uuid(org_id, field='orgId')} "
            "AND state NOT IN ('purged')",
        )
        object_leaks = [
            object_id
            for object_id in ids["objectIds"]
            if commands.object_exists(quarantine_object_key(org_id, object_id))
        ]
        vector_leaks = [
            point_id for point_id in ids["vectorPointIds"] if commands.vector_exists(point_id)
        ]
        if remaining_docs == 0 and not object_leaks and not vector_leaks:
            return
        if monotonic() >= deadline:
            raise FixtureError("cleanup timed out waiting for api/object/vector deletion")
        commands.sleep(0.05)


def _hard_delete_run_rows(commands: Commands, ids: dict[str, Any]) -> None:
    org_id = _sql_uuid(ids["orgId"], field="orgId")
    admin_id = _sql_uuid(ids["adminUserId"], field="adminUserId")
    viewer_id = _sql_uuid(ids["viewerUserId"], field="viewerUserId")
    collection_id = _sql_uuid(ids["collectionId"], field="collectionId")
    document_id = _sql_uuid(ids["failedDocumentId"], field="failedDocumentId")
    version_id = _sql_uuid(ids["failedVersionId"], field="failedVersionId")

    # Explicit reviewed FK order. session_replication_role bypasses immutability
    # triggers for document_versions/derived_artifacts/audit_log (same pattern as
    # deploy/scripts/o03-bluegreen-restore-drill.sh).
    sql = f"""
BEGIN;
SET LOCAL row_security = off;
SET LOCAL session_replication_role = replica;
SET LOCAL app.org_id = {org_id};

DELETE FROM conflict_evidence WHERE org_id = {org_id};
DELETE FROM conflicts WHERE org_id = {org_id};
DELETE FROM claims WHERE org_id = {org_id};
DELETE FROM chunks WHERE org_id = {org_id};
DELETE FROM vector_cleanup_intents WHERE org_id = {org_id};
DELETE FROM derived_artifacts WHERE org_id = {org_id};
DELETE FROM embedding_batches WHERE org_id = {org_id};
DELETE FROM index_generation_backfills WHERE org_id = {org_id};
DELETE FROM index_metadata WHERE org_id = {org_id};
DELETE FROM download_capability_redemptions WHERE org_id = {org_id};
DELETE FROM upload_operations WHERE org_id = {org_id};
DELETE FROM ask_stream_events WHERE org_id = {org_id};
DELETE FROM ask_stream_sessions WHERE org_id = {org_id};
DELETE FROM qa_chat_turns WHERE org_id = {org_id};
DELETE FROM qa_chat_sessions WHERE org_id = {org_id};
DELETE FROM outbox_events WHERE org_id = {org_id};
DELETE FROM event_log WHERE org_id = {org_id};
DELETE FROM jobs WHERE org_id = {org_id};
DELETE FROM quota_reservations WHERE org_id = {org_id};
DELETE FROM usage_counters WHERE org_id = {org_id};
DELETE FROM audit_log WHERE org_id = {org_id};

UPDATE documents
SET current_version_id = NULL
WHERE org_id = {org_id} AND id = {document_id};

DELETE FROM document_versions
WHERE org_id = {org_id}
  AND (id = {version_id} OR document_id = {document_id});

DELETE FROM documents WHERE org_id = {org_id} AND id = {document_id};

DELETE FROM collection_user_access WHERE org_id = {org_id};
DELETE FROM collection_group_access WHERE org_id = {org_id};
DELETE FROM collection_role_access WHERE org_id = {org_id};
DELETE FROM collections WHERE org_id = {org_id} AND id = {collection_id};

DELETE FROM refresh_tokens WHERE org_id = {org_id};
DELETE FROM group_memberships WHERE org_id = {org_id};
DELETE FROM groups WHERE org_id = {org_id};
DELETE FROM role_permissions WHERE org_id = {org_id};
DELETE FROM roles WHERE org_id = {org_id};
DELETE FROM org_memberships WHERE org_id = {org_id};
DELETE FROM org_invites WHERE org_id = {org_id};
DELETE FROM org_quotas WHERE org_id = {org_id};
DELETE FROM projects WHERE org_id = {org_id};
DELETE FROM orgs WHERE id = {org_id};

DELETE FROM users
WHERE id IN ({admin_id}, {viewer_id})
  AND NOT EXISTS (
    SELECT 1 FROM org_memberships m WHERE m.user_id = users.id
  );

COMMIT;
"""
    commands.psql(sql)


def _write_leak_report(path: Path | None, leaks: dict[str, list[str]]) -> None:
    if path is None:
        return
    _atomic_write_json(path, {"leaks": leaks}, mode=0o644)


def cmd_cleanup(
    *,
    run_id: str,
    manifest_path: Path,
    credentials_path: Path,
    api_base: str,
    timeout_secs: float,
    commands: Commands,
    environ: Mapping[str, str],
    leak_report_out: Path | None = None,
) -> None:
    refuse_production(environ)
    run_id = validate_run_id(run_id)
    if timeout_secs <= 0:
        raise FixtureError("timeout-secs must be > 0")
    if not api_base.strip():
        raise FixtureError("api-base required")

    manifest = _load_json(manifest_path)
    ids = _manifest_ids(manifest)
    if ids["runId"] != run_id:
        raise FixtureError("run-id does not match manifest")

    credentials: dict[str, Any] | None = None
    if credentials_path.exists():
        credentials = _load_json(credentials_path)
        if str(credentials.get("runId", "")) != run_id:
            raise FixtureError("run-id does not match credentials")

    # Idempotent fast-path: already clean and credentials already removed.
    leaks = collect_leaks(ids=ids, commands=commands)
    if not leaks and not credentials_path.exists():
        return

    if credentials is None and leaks:
        # Cannot authenticate to request API deletion; fail closed with IDs only.
        _write_leak_report(leak_report_out, leaks)
        raise FixtureError("cleanup incomplete: credentials missing while leaks remain")

    if credentials is not None:
        admin_email = str(credentials.get("adminEmail", ""))
        admin_password = str(credentials.get("adminPassword", ""))
        if not admin_email or not admin_password:
            raise FixtureError("credentials missing admin login fields")
        try:
            token = _login(
                commands,
                api_base,
                admin_email,
                admin_password,
                timeout_secs,
            )
            _request_document_delete(
                commands,
                api_base=api_base,
                token=token,
                document_id=ids["failedDocumentId"],
                timeout=timeout_secs,
            )
            _wait_for_document_cleanup(
                ids=ids,
                commands=commands,
                timeout_secs=timeout_secs,
            )
        except FixtureError as error:
            leaks = collect_leaks(ids=ids, commands=commands)
            _write_leak_report(leak_report_out, leaks)
            raise error

    try:
        _hard_delete_run_rows(commands, ids)
    except FixtureError as error:
        leaks = collect_leaks(ids=ids, commands=commands)
        _write_leak_report(leak_report_out, leaks)
        raise error

    leaks = collect_leaks(ids=ids, commands=commands)
    if leaks:
        _write_leak_report(leak_report_out, leaks)
        raise FixtureError("cleanup incomplete: leaks remain")

    _remove_credentials(credentials_path)


def cmd_verify_clean(
    *,
    run_id: str,
    manifest_path: Path,
    commands: Commands,
    environ: Mapping[str, str],
) -> None:
    refuse_production(environ)
    run_id = validate_run_id(run_id)
    manifest = _load_json(manifest_path)
    ids = _manifest_ids(manifest)
    if ids["runId"] != run_id:
        raise FixtureError("run-id does not match manifest")
    leaks = collect_leaks(ids=ids, commands=commands)
    if leaks:
        raise FixtureError("verify-clean found leaks")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="web_e2e_real_fixture.py")
    sub = parser.add_subparsers(dest="command", required=True)

    setup = sub.add_parser("setup")
    setup.add_argument("--run-id", required=True)
    setup.add_argument("--manifest-out", required=True)
    setup.add_argument("--credentials-out", required=True)

    cleanup = sub.add_parser("cleanup")
    cleanup.add_argument("--run-id", required=True)
    cleanup.add_argument("--manifest", required=True)
    cleanup.add_argument("--credentials", required=True)
    cleanup.add_argument("--api-base", required=True)
    cleanup.add_argument("--timeout-secs", required=True, type=float)
    cleanup.add_argument("--leak-report-out", required=False)

    verify = sub.add_parser("verify-clean")
    verify.add_argument("--run-id", required=True)
    verify.add_argument("--manifest", required=True)

    return parser


def main(
    argv: list[str] | None = None,
    *,
    commands: Commands | None = None,
    environ: Mapping[str, str] | None = None,
) -> int:
    env = dict(environ if environ is not None else os.environ)
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as error:
        code = error.code
        return int(code) if isinstance(code, int) else 1

    active_commands = commands or LiveCommands(environ=env)
    try:
        if args.command == "setup":
            cmd_setup(
                run_id=args.run_id,
                manifest_out=Path(args.manifest_out),
                credentials_out=Path(args.credentials_out),
                commands=active_commands,
                environ=env,
            )
        elif args.command == "cleanup":
            cmd_cleanup(
                run_id=args.run_id,
                manifest_path=Path(args.manifest),
                credentials_path=Path(args.credentials),
                api_base=args.api_base,
                timeout_secs=float(args.timeout_secs),
                commands=active_commands,
                environ=env,
                leak_report_out=Path(args.leak_report_out) if args.leak_report_out else None,
            )
        elif args.command == "verify-clean":
            cmd_verify_clean(
                run_id=args.run_id,
                manifest_path=Path(args.manifest),
                commands=active_commands,
                environ=env,
            )
        else:
            raise FixtureError("unknown command")
    except FixtureError as error:
        print(f"web_e2e_real_fixture: {error}", file=sys.stderr)
        return 1
    except OSError:
        print("web_e2e_real_fixture: filesystem error", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
