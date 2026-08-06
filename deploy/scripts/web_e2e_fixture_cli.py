#!/usr/bin/env python3
"""CLI parsing and fail-closed setup/cleanup orchestration."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Mapping

from web_e2e_fixture_adapters import (
    Commands,
    Deadline,
    LiveCommands,
    collection_name_for_signature,
)
from web_e2e_fixture_database import (
    RunInventory,
    hard_delete_run_rows,
    inspect_database_leaks,
    load_inventory,
    setup_fixture_rows,
)
from web_e2e_fixture_identity import (
    DEFAULT_OPERATION_TIMEOUT_SECS,
    EffectiveConfig,
    FixtureError,
    FixtureLeakError,
    FixtureProbeError,
    _assert_manifest_public,
    _atomic_write_json,
    _email_for_run,
    _generate_password,
    _load_json,
    _manifest_ids,
    _paths_alias,
    _remove_credentials,
    _slug_for_run,
    fixture_checksum,
    quarantine_object_key,
    refuse_production,
    validate_run_id,
    validate_signature,
    validate_uuid,
)

monotonic = time.monotonic


def _deadline(timeout_secs: float) -> Deadline:
    return Deadline(timeout_secs, clock=monotonic)


def _login(
    commands: Commands,
    api_base: str,
    email: str,
    password: str,
    deadline: Deadline,
) -> str:
    response = commands.http(
        "POST",
        f"{api_base.rstrip('/')}/api/v1/auth/login",
        headers={"content-type": "application/json"},
        body=json.dumps({"email": email, "password": password}).encode("utf-8"),
        timeout=deadline.remaining(),
    )
    if response.status != 200:
        raise FixtureError("login failed")
    try:
        payload = json.loads(response.body.decode("utf-8"))
        token = payload["accessToken"]
    except (KeyError, TypeError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FixtureError("login returned invalid json") from error
    if not isinstance(token, str) or not token:
        raise FixtureError("login missing access token")
    return token


def _request_document_delete(
    commands: Commands,
    *,
    api_base: str,
    token: str,
    document_id: str,
    deadline: Deadline,
) -> None:
    document_id = validate_uuid(document_id, field="documentId")
    response = commands.http(
        "DELETE",
        f"{api_base.rstrip('/')}/api/v1/documents/{document_id}",
        headers={"authorization": f"Bearer {token}"},
        body=None,
        timeout=deadline.remaining(),
    )
    if response.status not in {204, 404}:
        raise FixtureError("document delete request failed")


def _derived_collections(
    inventory: RunInventory,
    config: EffectiveConfig,
    remembered: set[str] | None = None,
) -> set[str]:
    signatures = set(inventory.signatures)
    configured = config.value("MARKHAND_INDEX_SIGNATURE")
    if configured is not None and str(configured).strip():
        signatures.add(validate_signature(str(configured).strip()))
    collections = {collection_name_for_signature(signature) for signature in signatures}
    if remembered:
        collections.update(remembered)
    return collections


def _storage_leaks(
    *,
    commands: Commands,
    ids: dict[str, Any],
    inventory: RunInventory,
    config: EffectiveConfig,
    deadline: Deadline,
    remembered_objects: dict[str, set[str]] | None = None,
    remembered_collections: set[str] | None = None,
) -> tuple[dict[str, list[str]], dict[str, set[str]], set[str]]:
    objects = {
        key: set(resource_ids)
        for key, resource_ids in (remembered_objects or {}).items()
    }
    for object_id in ids["objectIds"]:
        key = quarantine_object_key(ids["orgId"], object_id)
        objects.setdefault(key, set()).add(object_id)
    for resource in inventory.objects:
        objects.setdefault(resource.key, set()).add(resource.resource_id)
    collections = _derived_collections(inventory, config, remembered_collections)

    object_leaks: list[str] = []
    for key, resource_ids in sorted(objects.items()):
        if commands.object_exists(key, timeout=deadline.remaining()):
            object_leaks.extend(resource_ids)
    vector_ids = commands.qdrant_point_ids(
        sorted(collections),
        ids["orgId"],
        timeout=deadline.remaining(),
    )
    leaks: dict[str, list[str]] = {}
    if object_leaks:
        leaks["object_resource_ids"] = sorted(set(object_leaks))
    if vector_ids:
        leaks["vector_point_ids"] = sorted(set(vector_ids))
    return leaks, objects, collections


def collect_leaks(
    *,
    ids: dict[str, Any],
    commands: Commands,
    config: EffectiveConfig,
    deadline: Deadline,
    inventory: RunInventory | None = None,
    remembered_objects: dict[str, set[str]] | None = None,
    remembered_collections: set[str] | None = None,
) -> tuple[dict[str, Any], RunInventory, dict[str, set[str]], set[str]]:
    current = inventory or load_inventory(
        commands=commands,
        org_id=ids["orgId"],
        deadline=deadline,
    )
    database_rows = inspect_database_leaks(commands=commands, ids=ids, deadline=deadline)
    storage, objects, collections = _storage_leaks(
        commands=commands,
        ids=ids,
        inventory=current,
        config=config,
        deadline=deadline,
        remembered_objects=remembered_objects,
        remembered_collections=remembered_collections,
    )
    leaks: dict[str, Any] = {}
    if database_rows:
        leaks["database_rows"] = database_rows
    leaks.update(storage)
    return leaks, current, objects, collections


def _raise_leaks(message: str, leaks: dict[str, Any]) -> None:
    if leaks:
        raise FixtureLeakError(message, leaks)
    raise FixtureError(message)


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
    if _paths_alias(manifest_out, credentials_out):
        raise FixtureError("manifest and credentials paths must differ")
    if manifest_out.exists() or credentials_out.exists():
        raise FixtureError("fixture output already exists")
    deadline = _deadline(DEFAULT_OPERATION_TIMEOUT_SECS)

    admin_password = _generate_password()
    viewer_password = _generate_password()
    try:
        admin_hash = commands.hash_password(admin_password, timeout=deadline.remaining())
        viewer_hash = commands.hash_password(viewer_password, timeout=deadline.remaining())
    except FixtureError:
        raise
    except Exception as error:
        raise FixtureError("password hashing failed") from error

    namespace = uuid.uuid5(uuid.NAMESPACE_URL, f"markhand-e2e-fixture:{run_id}")
    ids = {
        "orgId": str(uuid.uuid5(namespace, "org")),
        "adminUserId": str(uuid.uuid5(namespace, "admin")),
        "viewerUserId": str(uuid.uuid5(namespace, "viewer")),
        "collectionId": str(uuid.uuid5(namespace, "collection")),
        "failedDocumentId": str(uuid.uuid5(namespace, "failed-doc")),
        "failedVersionId": str(uuid.uuid5(namespace, "failed-ver")),
        "objectId": str(uuid.uuid5(namespace, "object")),
        "vectorPointId": str(uuid.uuid5(namespace, "vector")),
    }
    admin_email = _email_for_run("admin", run_id)
    viewer_email = _email_for_run("viewer", run_id)
    object_key = quarantine_object_key(ids["orgId"], ids["objectId"])
    collection_name = f"E2E Library {run_id}"
    values = {
        **ids,
        "orgSlug": _slug_for_run(run_id, "org"),
        "orgName": f"E2E Org {run_id}",
        "adminEmail": admin_email,
        "adminName": f"E2E Admin {run_id}",
        "viewerEmail": viewer_email,
        "viewerName": f"E2E Viewer {run_id}",
        "collectionName": collection_name,
        "collectionSlug": _slug_for_run(run_id, "library"),
        "failedDocumentTitle": f"E2E Failed {run_id}",
        "contentSha": hashlib.sha256(f"e2e-failed:{run_id}".encode("utf-8")).hexdigest(),
        "objectKey": object_key,
    }
    compensation_ids = {
        "runId": run_id,
        "orgId": ids["orgId"],
        "adminUserId": ids["adminUserId"],
        "viewerUserId": ids["viewerUserId"],
        "collectionId": ids["collectionId"],
        "collectionName": collection_name,
        "failedDocumentId": ids["failedDocumentId"],
        "failedVersionId": ids["failedVersionId"],
        "objectIds": [ids["objectId"]],
        "vectorPointIds": [ids["vectorPointId"]],
    }
    try:
        created = setup_fixture_rows(
            commands=commands,
            deadline=deadline,
            values=values,
            admin_hash=admin_hash,
            viewer_hash=viewer_hash,
        )
        if any(created[field] != ids[field] for field in ids):
            raise FixtureError("fixture setup returned mismatched identifiers")
        checksum = fixture_checksum(
            [
                created["orgId"],
                created["adminUserId"],
                created["viewerUserId"],
                created["collectionId"],
                created["failedDocumentId"],
                created["failedVersionId"],
                created["objectId"],
                created["vectorPointId"],
            ]
        )
        manifest = {
            **compensation_ids,
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
            "vectorPointIds": [created["vectorPointId"]],
        }
        deadline.remaining()
        _atomic_write_json(credentials_out, credentials, mode=0o600)
        deadline.remaining()
        _atomic_write_json(manifest_out, manifest, mode=0o644)
        if credentials_out.stat().st_mode & 0o777 != 0o600:
            raise FixtureError("credentials file mode must be 0600")
    except Exception as setup_error:
        output_cleanup_error: OSError | FixtureError | None = None
        try:
            _remove_credentials(credentials_out)
            manifest_out.unlink(missing_ok=True)
        except (OSError, FixtureError) as error:
            output_cleanup_error = error
        compensation_deadline = _deadline(DEFAULT_OPERATION_TIMEOUT_SECS)
        try:
            hard_delete_run_rows(
                commands=commands,
                ids=compensation_ids,
                deadline=compensation_deadline,
            )
            remaining = inspect_database_leaks(
                commands=commands,
                ids=compensation_ids,
                deadline=compensation_deadline,
            )
        except FixtureError as compensation_error:
            raise FixtureError("fixture setup failed and database compensation failed") from compensation_error
        if remaining:
            raise FixtureLeakError(
                "fixture setup failed and database compensation left rows",
                {"database_rows": remaining},
            ) from setup_error
        if output_cleanup_error is not None:
            raise FixtureError("fixture setup failed and output cleanup failed") from output_cleanup_error
        if isinstance(setup_error, FixtureError):
            raise setup_error
        raise FixtureError("fixture setup post-commit step failed") from setup_error


def _load_credentials(path: Path, run_id: str) -> dict[str, Any] | None:
    if not path.exists():
        return None
    credentials = _load_json(path)
    if str(credentials.get("runId", "")) != run_id:
        raise FixtureError("run-id does not match credentials")
    return credentials


def _wait_for_worker_cleanup(
    *,
    ids: dict[str, Any],
    commands: Commands,
    config: EffectiveConfig,
    api_base: str,
    token: str,
    deadline: Deadline,
    initial_inventory: RunInventory,
    remembered_objects: dict[str, set[str]],
    remembered_collections: set[str],
) -> tuple[dict[str, set[str]], set[str]]:
    requested: set[str] = set()
    inventory = initial_inventory
    while True:
        active_documents = [
            document.document_id
            for document in inventory.documents
            if document.state != "purged"
        ]
        for document_id in active_documents:
            if document_id not in requested:
                _request_document_delete(
                    commands,
                    api_base=api_base,
                    token=token,
                    document_id=document_id,
                    deadline=deadline,
                )
                requested.add(document_id)
        inventory = load_inventory(
            commands=commands,
            org_id=ids["orgId"],
            deadline=deadline,
        )
        storage, remembered_objects, remembered_collections = _storage_leaks(
            commands=commands,
            ids=ids,
            inventory=inventory,
            config=config,
            deadline=deadline,
            remembered_objects=remembered_objects,
            remembered_collections=remembered_collections,
        )
        active_documents = [
            document.document_id
            for document in inventory.documents
            if document.state != "purged"
        ]
        if not active_documents and not storage:
            return remembered_objects, remembered_collections
        remaining = deadline.remaining()
        commands.sleep(min(0.05, remaining))


def cmd_cleanup(
    *,
    run_id: str,
    manifest_path: Path,
    credentials_path: Path,
    api_base: str,
    timeout_secs: float,
    commands: Commands,
    environ: Mapping[str, str],
) -> None:
    config = refuse_production(environ)
    run_id = validate_run_id(run_id)
    if timeout_secs <= 0:
        raise FixtureError("timeout-secs must be > 0")
    if not api_base.strip():
        raise FixtureError("api-base required")
    if _paths_alias(manifest_path, credentials_path):
        raise FixtureError("manifest and credentials paths must differ")
    deadline = _deadline(timeout_secs)
    manifest = _load_json(manifest_path)
    ids = _manifest_ids(manifest)
    if ids["runId"] != run_id:
        raise FixtureError("run-id does not match manifest")
    credentials = _load_credentials(credentials_path, run_id)

    leaks, inventory, remembered_objects, remembered_collections = collect_leaks(
        ids=ids,
        commands=commands,
        config=config,
        deadline=deadline,
    )
    if not leaks:
        _remove_credentials(credentials_path)
        return

    active_documents = [
        document.document_id for document in inventory.documents if document.state != "purged"
    ]
    if active_documents:
        if credentials is None:
            raise FixtureLeakError(
                "cleanup incomplete: credentials missing while api-deletable documents remain",
                leaks,
            )
        admin_email = str(credentials.get("adminEmail", ""))
        admin_password = str(credentials.get("adminPassword", ""))
        if not admin_email or not admin_password:
            raise FixtureLeakError("credentials missing admin login fields", leaks)
        try:
            token = _login(commands, api_base, admin_email, admin_password, deadline)
            remembered_objects, remembered_collections = _wait_for_worker_cleanup(
                ids=ids,
                commands=commands,
                config=config,
                api_base=api_base,
                token=token,
                deadline=deadline,
                initial_inventory=inventory,
                remembered_objects=remembered_objects,
                remembered_collections=remembered_collections,
            )
        except FixtureProbeError:
            raise
        except FixtureError as error:
            raise FixtureLeakError("cleanup interrupted while leaks remain", leaks) from error
    else:
        storage, remembered_objects, remembered_collections = _storage_leaks(
            commands=commands,
            ids=ids,
            inventory=inventory,
            config=config,
            deadline=deadline,
            remembered_objects=remembered_objects,
            remembered_collections=remembered_collections,
        )
        if storage:
            raise FixtureLeakError("cleanup incomplete: storage leaks remain", {**leaks, **storage})

    try:
        hard_delete_run_rows(commands=commands, ids=ids, deadline=deadline)
    except FixtureError as error:
        raise FixtureLeakError(
            f"cleanup database deletion failed: {error}", leaks
        ) from error
    final_leaks, _inventory, _objects, _collections = collect_leaks(
        ids=ids,
        commands=commands,
        config=config,
        deadline=deadline,
        remembered_objects=remembered_objects,
        remembered_collections=remembered_collections,
    )
    if final_leaks:
        raise FixtureLeakError("cleanup incomplete: leaks remain", final_leaks)
    _remove_credentials(credentials_path)


def cmd_verify_clean(
    *,
    run_id: str,
    manifest_path: Path,
    commands: Commands,
    environ: Mapping[str, str],
) -> None:
    config = refuse_production(environ)
    run_id = validate_run_id(run_id)
    deadline = _deadline(DEFAULT_OPERATION_TIMEOUT_SECS)
    manifest = _load_json(manifest_path)
    ids = _manifest_ids(manifest)
    if ids["runId"] != run_id:
        raise FixtureError("run-id does not match manifest")
    leaks, _inventory, _objects, _collections = collect_leaks(
        ids=ids,
        commands=commands,
        config=config,
        deadline=deadline,
    )
    if leaks:
        raise FixtureLeakError("verify-clean found leaks", leaks)


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
        return int(error.code) if isinstance(error.code, int) else 1
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
    except FixtureLeakError as error:
        print(f"web_e2e_real_fixture: {error}", file=sys.stderr)
        print(json.dumps({"leaks": error.leaks}, sort_keys=True), file=sys.stderr)
        return 1
    except FixtureError as error:
        print(f"web_e2e_real_fixture: {error}", file=sys.stderr)
        return 1
    except OSError:
        print("web_e2e_real_fixture: filesystem error", file=sys.stderr)
        return 1
    return 0
