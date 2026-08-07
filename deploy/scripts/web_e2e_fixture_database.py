#!/usr/bin/env python3
"""PostgreSQL setup, inventory, leak inspection, and reviewed cleanup order."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from web_e2e_fixture_adapters import Commands, Deadline
from web_e2e_fixture_identity import (
    FixtureError,
    _sql_quote_literal,
    _sql_uuid,
    validate_signature,
    validate_uuid,
)


@dataclass(frozen=True)
class DocumentResource:
    document_id: str
    state: str


@dataclass(frozen=True)
class ObjectResource:
    resource_id: str
    key: str


@dataclass(frozen=True)
class RunInventory:
    documents: tuple[DocumentResource, ...]
    objects: tuple[ObjectResource, ...]
    signatures: tuple[str, ...]


def setup_fixture_rows(
    *,
    commands: Commands,
    deadline: Deadline,
    values: dict[str, str],
    admin_hash: str,
    viewer_hash: str,
) -> dict[str, str]:
    org_id = values["orgId"]
    admin_user_id = values["adminUserId"]
    viewer_user_id = values["viewerUserId"]
    collection_id = values["collectionId"]
    failed_document_id = values["failedDocumentId"]
    failed_version_id = values["failedVersionId"]
    object_id = values["objectId"]
    vector_point_id = values["vectorPointId"]
    sql = f"""
-- fixture_setup_rows
BEGIN;
SET LOCAL row_security = off;
SET LOCAL app.org_id = {_sql_uuid(org_id, field='orgId')};

INSERT INTO orgs (id, slug, name)
VALUES (
  {_sql_uuid(org_id, field='orgId')},
  {_sql_quote_literal(values['orgSlug'])},
  {_sql_quote_literal(values['orgName'])}
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
  {_sql_quote_literal(values['adminEmail'])},
  {_sql_quote_literal(values['adminName'])},
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
  {_sql_quote_literal(values['viewerEmail'])},
  {_sql_quote_literal(values['viewerName'])},
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
  {_sql_quote_literal(values['collectionName'])},
  {_sql_quote_literal(values['collectionSlug'])},
  'Run-scoped E2E collection',
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
  {_sql_quote_literal(values['failedDocumentTitle'])},
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
  {_sql_quote_literal(values['contentSha'])},
  {_sql_quote_literal(values['objectKey'])},
  'e2e-failed.txt',
  'text/plain',
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
    raw = commands.psql(
        sql,
        timeout=deadline.remaining(),
        redact=[admin_hash, viewer_hash, values["objectKey"]],
    )
    if not raw:
        raise FixtureError("fixture setup returned empty result")
    try:
        payload = json.loads(raw.splitlines()[-1])
    except (TypeError, json.JSONDecodeError) as error:
        raise FixtureError("fixture setup returned invalid result") from error
    if not isinstance(payload, dict):
        raise FixtureError("fixture setup returned invalid result")
    fields = (
        "orgId",
        "adminUserId",
        "viewerUserId",
        "collectionId",
        "failedDocumentId",
        "failedVersionId",
        "objectId",
        "vectorPointId",
    )
    created: dict[str, str] = {}
    for field in fields:
        try:
            created[field] = validate_uuid(str(payload[field]), field=field)
        except KeyError as error:
            raise FixtureError("fixture setup returned invalid result") from error
    return created


def load_inventory(
    *,
    commands: Commands,
    org_id: str,
    deadline: Deadline,
) -> RunInventory:
    org = _sql_uuid(org_id, field="orgId")
    sql = f"""
-- fixture_resource_inventory
WITH document_rows AS (
  SELECT id::text, state
  FROM documents
  WHERE org_id = {org}
),
object_rows AS (
  SELECT id::text AS resource_id, original_object_key AS object_key
  FROM document_versions
  WHERE org_id = {org}
  UNION
  SELECT id::text, markdown_object_key
  FROM document_versions
  WHERE org_id = {org} AND markdown_object_key IS NOT NULL
  UNION
  SELECT id::text, object_key
  FROM derived_artifacts
  WHERE org_id = {org}
  UNION
  SELECT object_id::text, expected_object_key
  FROM upload_operations
  WHERE org_id = {org} AND expected_object_key IS NOT NULL
  UNION
  SELECT object_id::text, object_key
  FROM upload_operations
  WHERE org_id = {org} AND object_key IS NOT NULL
),
signature_rows AS (
  SELECT index_signature_sha256 AS signature
  FROM index_metadata
  WHERE org_id = {org}
  UNION
  SELECT index_signature
  FROM chunks
  WHERE org_id = {org}
  UNION
  SELECT index_signature_sha256
  FROM vector_cleanup_intents
  WHERE org_id = {org}
)
SELECT json_build_object(
  'documents', COALESCE((
    SELECT json_agg(json_build_object('id', id, 'state', state) ORDER BY id)
    FROM document_rows
  ), '[]'::json),
  'objects', COALESCE((
    SELECT json_agg(
      json_build_object('resourceId', resource_id, 'key', object_key)
      ORDER BY resource_id, object_key
    )
    FROM object_rows
  ), '[]'::json),
  'signatures', COALESCE((
    SELECT json_agg(signature ORDER BY signature)
    FROM signature_rows
  ), '[]'::json)
)::text;
"""
    raw = commands.psql(sql, timeout=deadline.remaining())
    try:
        payload = json.loads(raw.splitlines()[-1])
        document_values = payload["documents"]
        object_values = payload["objects"]
        signature_values = payload["signatures"]
        if not all(isinstance(value, list) for value in (document_values, object_values, signature_values)):
            raise TypeError
        documents = tuple(
            DocumentResource(
                validate_uuid(str(item["id"]), field="documentId"),
                _validate_document_state(str(item["state"])),
            )
            for item in document_values
        )
        objects = tuple(
            ObjectResource(
                validate_uuid(str(item["resourceId"]), field="objectResourceId"),
                _validate_object_key(str(item["key"])),
            )
            for item in object_values
        )
        signatures = tuple(sorted({validate_signature(str(item)) for item in signature_values}))
    except (
        KeyError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
    ) as error:
        raise FixtureError("fixture inventory returned invalid data") from error
    return RunInventory(documents, objects, signatures)


def _validate_document_state(state: str) -> str:
    if state not in {
        "uploaded",
        "converting",
        "converted",
        "indexing",
        "indexed",
        "failed",
        "tombstoned",
        "purged",
    }:
        raise FixtureError("fixture inventory returned invalid document state")
    return state


def _validate_object_key(key: str) -> str:
    if not key or key.startswith("/") or ".." in key.split("/") or "\x00" in key:
        raise FixtureError("fixture inventory returned invalid object key")
    return key


def inspect_database_leaks(
    *,
    commands: Commands,
    ids: dict[str, Any],
    deadline: Deadline,
) -> dict[str, list[str]]:
    org_id = _sql_uuid(ids["orgId"], field="orgId")
    admin_id = _sql_uuid(ids["adminUserId"], field="adminUserId")
    viewer_id = _sql_uuid(ids["viewerUserId"], field="viewerUserId")
    sql = f"""
-- fixture_org_table_leaks
BEGIN;
SET LOCAL row_security = off;
SET LOCAL app.org_id = {org_id};
CREATE TEMP TABLE fixture_leak_rows (
  table_name text NOT NULL,
  row_id uuid NOT NULL
) ON COMMIT DROP;

DO $fixture$
DECLARE
  scoped record;
  has_id boolean;
BEGIN
  FOR scoped IN
    SELECT table_name
    FROM information_schema.columns
    WHERE table_schema = 'public' AND column_name = 'org_id'
    ORDER BY table_name
  LOOP
    SELECT EXISTS (
      SELECT 1
      FROM information_schema.columns
      WHERE table_schema = 'public'
        AND table_name = scoped.table_name
        AND column_name = 'id'
        AND udt_name = 'uuid'
    ) INTO has_id;
    IF has_id THEN
      EXECUTE format(
        'INSERT INTO fixture_leak_rows(table_name, row_id) '
        'SELECT %L, id FROM public.%I WHERE org_id = $1',
        scoped.table_name,
        scoped.table_name
      ) USING {org_id}::uuid;
    ELSE
      EXECUTE format(
        'INSERT INTO fixture_leak_rows(table_name, row_id) '
        'SELECT %L, org_id FROM public.%I WHERE org_id = $1',
        scoped.table_name,
        scoped.table_name
      ) USING {org_id}::uuid;
    END IF;
  END LOOP;
END
$fixture$;

INSERT INTO fixture_leak_rows(table_name, row_id)
SELECT 'orgs', id FROM orgs WHERE id = {org_id};
INSERT INTO fixture_leak_rows(table_name, row_id)
SELECT 'users', id FROM users WHERE id IN ({admin_id}, {viewer_id});

SELECT COALESCE(
  jsonb_object_agg(table_name, row_ids ORDER BY table_name),
  '{{}}'::jsonb
)::text
FROM (
  SELECT table_name, jsonb_agg(row_id::text ORDER BY row_id::text) AS row_ids
  FROM fixture_leak_rows
  GROUP BY table_name
) grouped;
COMMIT;
"""
    raw = commands.psql(sql, timeout=deadline.remaining())
    try:
        payload = json.loads(raw.splitlines()[-1])
        if not isinstance(payload, dict):
            raise TypeError
        leaks: dict[str, list[str]] = {}
        for table, row_ids in payload.items():
            if (
                not isinstance(table, str)
                or not table
                or not all(char.islower() or char.isdigit() or char == "_" for char in table)
                or not isinstance(row_ids, list)
            ):
                raise TypeError
            leaks[table] = [
                validate_uuid(str(row_id), field="databaseRowId") for row_id in row_ids
            ]
        return {table: sorted(set(row_ids)) for table, row_ids in leaks.items() if row_ids}
    except (
        TypeError,
        ValueError,
        json.JSONDecodeError,
    ) as error:
        raise FixtureError("fixture database leak scan returned invalid data") from error


def hard_delete_run_rows(
    *,
    commands: Commands,
    ids: dict[str, Any],
    deadline: Deadline,
) -> None:
    org_id = _sql_uuid(ids["orgId"], field="orgId")
    admin_id = _sql_uuid(ids["adminUserId"], field="adminUserId")
    viewer_id = _sql_uuid(ids["viewerUserId"], field="viewerUserId")
    sql = f"""
-- fixture_hard_delete_reviewed_order
BEGIN;
SET LOCAL row_security = off;
SET LOCAL app.org_id = {org_id};

-- Disable only project-owned USER triggers (immutability + deferred invariant
-- constraint triggers). PostgreSQL FK/internal triggers stay enabled.
-- AccessExclusive locks held until COMMIT keep concurrent writers out.
ALTER TABLE conflict_evidence DISABLE TRIGGER USER;
ALTER TABLE conflicts DISABLE TRIGGER USER;
ALTER TABLE derived_artifacts DISABLE TRIGGER USER;
ALTER TABLE index_metadata DISABLE TRIGGER USER;
ALTER TABLE audit_log DISABLE TRIGGER USER;
ALTER TABLE document_versions DISABLE TRIGGER USER;
-- documents carries DEFERRABLE invariant triggers on current_version_id; disable
-- so nulling the pointer does not queue deferred events that block later ALTER.
ALTER TABLE documents DISABLE TRIGGER USER;

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

-- Null pointer then delete versions while USER triggers are off. Do not ENABLE
-- in this transaction: DML with disabled triggers (and any remaining deferred
-- events) makes PostgreSQL reject ALTER TABLE ... ENABLE TRIGGER with
-- "cannot ALTER TABLE because it has pending trigger events".
UPDATE documents SET current_version_id = NULL WHERE org_id = {org_id};
DELETE FROM document_versions WHERE org_id = {org_id};
DELETE FROM documents WHERE org_id = {org_id};

DELETE FROM collection_user_access WHERE org_id = {org_id};
DELETE FROM collection_group_access WHERE org_id = {org_id};
DELETE FROM collection_role_access WHERE org_id = {org_id};
DELETE FROM collections WHERE org_id = {org_id};
DELETE FROM projects WHERE org_id = {org_id};

DELETE FROM refresh_tokens WHERE org_id = {org_id};
DELETE FROM group_memberships WHERE org_id = {org_id};
DELETE FROM groups WHERE org_id = {org_id};
DELETE FROM role_permissions WHERE org_id = {org_id};
DELETE FROM roles WHERE org_id = {org_id};
DELETE FROM org_memberships WHERE org_id = {org_id};
DELETE FROM org_invites WHERE org_id = {org_id};
DELETE FROM org_quotas WHERE org_id = {org_id};
DELETE FROM orgs WHERE id = {org_id};

DELETE FROM users
WHERE id IN ({admin_id}, {viewer_id})
  AND NOT EXISTS (
    SELECT 1 FROM org_memberships membership WHERE membership.user_id = users.id
  );

COMMIT;

-- Re-enable in a fresh transaction after COMMIT cleared pending trigger events.
-- DISABLE was committed above, so triggers stay off until these ENABLE statements.
BEGIN;
ALTER TABLE conflict_evidence ENABLE TRIGGER USER;
ALTER TABLE conflicts ENABLE TRIGGER USER;
ALTER TABLE derived_artifacts ENABLE TRIGGER USER;
ALTER TABLE index_metadata ENABLE TRIGGER USER;
ALTER TABLE audit_log ENABLE TRIGGER USER;
ALTER TABLE document_versions ENABLE TRIGGER USER;
ALTER TABLE documents ENABLE TRIGGER USER;
COMMIT;
"""
    commands.psql(sql, timeout=deadline.remaining())
