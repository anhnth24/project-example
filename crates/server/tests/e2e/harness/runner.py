"""Manifest-driven O04 runner (hermetic contracts + live orchestration).

Live path uses public `/api/v1` only. Missing production intake identities
(`documentId`/`versionId`/`jobId`) emit high/critical blocker evidence and abort
immediately before security mutations, worker kills, or downstream calls.
"""

from __future__ import annotations

import json
import os
import time
import uuid
from pathlib import Path
from typing import Any, Callable

from .api_client import ApiClient
from .cleanup import (
    CleanupFailed,
    CleanupStack,
    disable_user,
    kill_and_restart_service,
    remove_membership,
    set_collection_visibility,
    stop_service,
)
from .compose_util import run_compose
from .confirm import require_live_gates
from .coverage import evaluate_claims_live_vertical_slice
from .evidence import (
    CaseResult,
    build_report,
    write_live_runtime,
    write_tracked_hermetic,
)
from .intake import ProductionIntakeNotWired, extract_production_intake

E2E_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = E2E_ROOT.parents[3]

AUDIO_DISABLE_MARKERS = (
    "audioconversiondisabled",
    "audio_conversion_disabled",
    "convert audio disabled",
    "audio conversion is disabled",
)

HALLUCINATION_MARKERS = (
    "the ",
    "this ",
    "hello",
    "welcome",
    "subscribe",
    "youtube",
    "transcript",
    "xin chào",
    "cảm ơn",
)


def load_suite_manifest(path: Path | None = None) -> dict[str, Any]:
    manifest_path = path or (E2E_ROOT / "manifest.json")
    return json.loads(manifest_path.read_text(encoding="utf-8"))


def load_fixture_manifest() -> dict[str, Any]:
    return json.loads((E2E_ROOT / "fixtures" / "manifest.json").read_text(encoding="utf-8"))


def fixture_path(fixture_id: str) -> Path | None:
    fixtures = load_fixture_manifest()["fixtures"]
    for item in fixtures:
        if item["id"] == fixture_id:
            path = E2E_ROOT / "fixtures" / item["path"]
            if item.get("approved") is False:
                return None
            if not path.is_file():
                return None
            return path
    return None


def wait_until(
    predicate: Callable[[], bool],
    *,
    timeout_secs: float,
    interval_secs: float = 1.0,
    label: str,
) -> None:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(interval_secs)
    raise TimeoutError(label)


def _body_text(result: Any) -> str:
    raw = getattr(result, "body", b"") or b""
    if isinstance(raw, bytes):
        return raw.decode("utf-8", errors="replace")
    return str(raw)


def _audio_explicitly_disabled(result: Any, body: dict[str, Any]) -> bool:
    blob = " ".join(
        [
            str(body.get("reasonCode") or ""),
            str(body.get("disposition") or ""),
            str(body.get("message") or ""),
            _body_text(result),
        ]
    ).lower()
    return any(marker in blob for marker in AUDIO_DISABLE_MARKERS)


def _blocked_intake(case_id: str, matrix: str, detail: str) -> CaseResult:
    return CaseResult(
        id=case_id,
        matrix=matrix,
        status="blocked",
        postconditions={"production_intake_ids": False},
        severity="critical",
        blocker_code="production_intake_not_wired",
        notes=detail,
    )


def _abort_matrix_for_intake(
    suite: dict[str, Any],
    *,
    detail: str,
    http_statuses: list[int] | None = None,
) -> list[CaseResult]:
    """Emit deterministic high/critical blockers for the full required matrix."""
    statuses = list(http_statuses or [])
    cases: list[CaseResult] = []
    for fmt in suite["formats"]:
        if fmt.get("requirement") == "required":
            cases.append(
                CaseResult(
                    id=fmt["id"],
                    matrix="format",
                    status="blocked",
                    http_statuses=statuses,
                    postconditions={"production_intake_ids": False},
                    severity="critical",
                    blocker_code="production_intake_not_wired",
                    notes=detail,
                )
            )
        else:
            cases.append(
                CaseResult(
                    id=fmt["id"],
                    matrix="format",
                    status="optional_unavailable",
                    http_statuses=statuses,
                    postconditions={"production_intake_ids": False},
                    severity="none",
                    blocker_code="production_intake_not_wired",
                    notes="optional format blocked by missing production intake",
                )
            )
    for sec in suite["security"]:
        cases.append(_blocked_intake(sec["id"], "security", detail))
    for adv in suite.get("adversarial") or []:
        cases.append(_blocked_intake(adv["id"], "adversarial", detail))
    for fault in suite["fault"]:
        cases.append(_blocked_intake(fault["id"], "fault", detail))
    return cases


def probe_production_intake(
    *,
    client: ApiClient,
    collection_id: str,
    suite: dict[str, Any],
) -> tuple[bool, str, list[int]]:
    """Upload one required fixture to prove public intake wiring. No mutations beyond upload."""
    required = next(f for f in suite["formats"] if f.get("requirement") == "required")
    path = fixture_path(required["fixtureId"])
    if path is None:
        return False, f"missing required fixture {required['fixtureId']}", []
    upload = client.upload(
        path,
        collection_id=collection_id,
        idempotency_key=f"e2e-intake-probe-{uuid.uuid4().hex[:8]}",
    )
    http_statuses = [upload.status]
    body = upload.json() if upload.body else {}
    if not isinstance(body, dict):
        body = {}
    if upload.status not in (200, 201):
        return (
            False,
            f"intake probe upload rejected status={upload.status}",
            http_statuses,
        )
    try:
        extract_production_intake(body)
    except ProductionIntakeNotWired as exc:
        return False, str(exc), http_statuses
    return True, "", http_statuses


def run_format_case_live(
    case: dict[str, Any],
    *,
    client: ApiClient,
    collection_id: str,
    env: dict[str, str],
) -> CaseResult:
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}
    opaque: dict[str, str] = {}
    token = case.get("uniqueToken") or ""
    path = fixture_path(case["fixtureId"])

    if case.get("requirement") != "required" and path is None:
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="optional_unavailable",
            postconditions={"approved_spoken_fixture_present": False},
            severity="none",
            notes=(
                "optional spoken-audio coverage requires an explicit approved "
                "spoken-token fixture/model; silence cannot satisfy this case"
            ),
        )

    if path is None:
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            severity="high",
            notes=f"missing fixture {case['fixtureId']}",
        )

    # Spoken audio must never use silence fixtures.
    if case.get("canonicalFormat") == "wav" and "silence" in path.name.lower():
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            severity="critical",
            notes="spoken audio coverage cannot pass from silence fixture",
        )

    upload = client.upload(
        path,
        collection_id=collection_id,
        idempotency_key=f"e2e-{case['id']}-{uuid.uuid4().hex[:8]}",
    )
    http_statuses.append(upload.status)
    body = upload.json() if upload.body else {}
    if not isinstance(body, dict):
        body = {}

    if case.get("requirement") == "optional_model" and _audio_explicitly_disabled(upload, body):
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="optional_unavailable",
            http_statuses=http_statuses,
            postconditions={"server_explicitly_disabled_audio": True},
            severity="none",
            notes="server explicitly disabled audio conversion; no pass claim",
        )

    if upload.status not in (200, 201):
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            http_statuses=http_statuses,
            severity="high",
            notes=f"upload rejected status={upload.status}",
        )

    disposition = body.get("disposition")
    posts["upload_accepted"] = disposition in ("accepted", "quarantined")
    if not posts["upload_accepted"]:
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high",
            notes=f"unexpected disposition={disposition!r}",
        )

    try:
        document_id, version_id, job_id = extract_production_intake(body)
    except ProductionIntakeNotWired as exc:
        raise ProductionIntakeNotWired(str(exc)) from exc

    opaque.update(
        {
            "documentFp": document_id.replace("-", "")[:12],
            "versionFp": version_id.replace("-", "")[:12],
            "jobFp": job_id.replace("-", "")[:12],
        }
    )
    posts["production_intake_ids"] = True

    def jobs_terminal() -> bool:
        job = client.get(f"/api/v1/jobs/{job_id}")
        http_statuses.append(job.status)
        if job.status != 200:
            return False
        payload = job.json() or {}
        status = payload.get("status")
        posts["convert_job_seen"] = payload.get("jobType") == "convert" or payload.get(
            "documentId"
        ) == document_id
        if status == "succeeded":
            posts["convert_succeeded"] = True
            return True
        if status in {"failed", "dead_letter", "cancelled"}:
            err = str(payload.get("lastError") or "")
            if case.get("requirement") == "optional_model" and any(
                m in err.lower() for m in AUDIO_DISABLE_MARKERS
            ):
                posts["audio_disabled"] = True
                posts["convert_succeeded"] = False
                return True
            posts["convert_succeeded"] = False
            return True
        return False

    try:
        wait_until(jobs_terminal, timeout_secs=180, label=f"{case['id']} convert job")
    except TimeoutError:
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            http_statuses=http_statuses,
            postconditions=posts,
            opaque_refs=opaque,
            severity="high",
            notes="timeout waiting for convert job",
        )

    if posts.get("audio_disabled"):
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="optional_unavailable",
            http_statuses=http_statuses,
            postconditions={"server_explicitly_disabled_audio": True},
            opaque_refs=opaque,
            severity="none",
            notes="server explicitly disabled audio conversion; no pass claim",
        )

    if not posts.get("convert_succeeded"):
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            http_statuses=http_statuses,
            postconditions=posts,
            opaque_refs=opaque,
            severity="high",
            notes="convert job did not succeed",
        )

    def indexed() -> bool:
        jobs = client.get("/api/v1/jobs?limit=50")
        http_statuses.append(jobs.status)
        if jobs.status != 200:
            return False
        items = (jobs.json() or {}).get("items") or []
        index_ok = any(
            item.get("jobType") == "index"
            and item.get("status") == "succeeded"
            and item.get("documentId") == document_id
            and item.get("versionId") == version_id
            for item in items
        )
        posts["index_succeeded"] = index_ok
        return index_ok

    try:
        wait_until(indexed, timeout_secs=180, label=f"{case['id']} index job")
    except TimeoutError:
        return CaseResult(
            id=case["id"],
            matrix="format",
            status="fail",
            http_statuses=http_statuses,
            postconditions=posts,
            opaque_refs=opaque,
            severity="high",
            notes="timeout waiting for index success on exact document/version",
        )

    doc = client.get(f"/api/v1/documents/{document_id}")
    http_statuses.append(doc.status)
    doc_body = doc.json() or {}
    current = doc_body.get("currentVersionId")
    if current != version_id:
        publish = client.post_json(
            f"/api/v1/documents/{document_id}/versions/{version_id}/publish",
            {},
        )
        http_statuses.append(publish.status)
        posts["published"] = publish.status in (200, 201)
        doc = client.get(f"/api/v1/documents/{document_id}")
        http_statuses.append(doc.status)
        doc_body = doc.json() or {}
    else:
        posts["published"] = True
    posts["has_current_version"] = doc_body.get("currentVersionId") == version_id

    search = client.post_json(
        "/api/v1/search",
        {"query": token, "collectionIds": [collection_id], "limit": 10},
    )
    http_statuses.append(search.status)
    hits = (search.json() or {}).get("hits") or []
    posts["search_hit"] = search.status == 200 and any(
        h.get("documentId") == document_id
        and h.get("versionId") == version_id
        and token in (h.get("snippet") or "")
        for h in hits
    )

    ask = client.post_json(
        "/api/v1/ask",
        {
            "question": f"Ma truy vet trong tai lieu la gi? {token}",
            "collectionIds": [collection_id],
            "limit": 8,
            "useProvider": False,
        },
    )
    http_statuses.append(ask.status)
    ask_body = ask.json() or {}
    citations = ask_body.get("citations") or []
    matching_cites = [
        c
        for c in citations
        if c.get("documentId") == document_id
        and c.get("versionId") == version_id
        and token in (c.get("quote") or "")
    ]
    posts["ask_ok"] = ask.status == 200 and bool(matching_cites)

    if matching_cites:
        cite = matching_cites[0]
        resolve = client.post_json(
            "/api/v1/citations/resolve",
            {
                "citations": [
                    {
                        "chunkId": cite["chunkId"],
                        "expectedVersionId": version_id,
                        "expectedDocumentId": document_id,
                        "expectedContentSha256": cite.get("contentSha256"),
                        "expectedQuote": cite.get("quote"),
                    }
                ]
            },
        )
        http_statuses.append(resolve.status)
        resolved = (resolve.json() or {}).get("citations") or []
        posts["citation_resolves"] = False
        if resolve.status == 200 and resolved:
            item = resolved[0]
            logical_ok = item.get("logicalDocumentId") == document_id
            posts["citation_resolves"] = (
                logical_ok
                and item.get("versionId") == version_id
                and token in (item.get("quote") or "")
                and isinstance(item.get("spanStart"), int)
                and isinstance(item.get("spanEnd"), int)
                and item["spanEnd"] >= item["spanStart"]
                and isinstance(item.get("contentSha256"), str)
                and len(item.get("contentSha256") or "") == 64
            )
    else:
        posts["citation_resolves"] = False

    preview = client.get(f"/api/v1/documents/{document_id}/versions/{version_id}/preview")
    http_statuses.append(preview.status)
    preview_body = preview.json() or {}
    markdown = preview_body.get("markdown") if isinstance(preview_body, dict) else None
    posts["preview_authorized"] = preview.status == 200 and isinstance(markdown, str)
    posts["preview_token"] = isinstance(markdown, str) and token in markdown
    markdown = None
    preview_body = None

    cap = client.post_json(
        f"/api/v1/documents/{document_id}/versions/{version_id}/download-capabilities",
        {"purpose": "original"},
    )
    http_statuses.append(cap.status)
    posts["download_capability"] = cap.status in (200, 201)

    required_posts = [
        "upload_accepted",
        "production_intake_ids",
        "convert_succeeded",
        "index_succeeded",
        "published",
        "has_current_version",
        "search_hit",
        "ask_ok",
        "citation_resolves",
        "preview_authorized",
        "preview_token",
        "download_capability",
    ]
    ok = all(posts.get(name) for name in required_posts)
    return CaseResult(
        id=case["id"],
        matrix="format",
        status="pass" if ok else "fail",
        http_statuses=http_statuses,
        postconditions=posts,
        opaque_refs=opaque,
        severity="none" if ok else "high",
        notes="" if ok else "one or more exact postconditions failed",
    )


def run_security_case_live(
    case: dict[str, Any],
    *,
    admin: ApiClient,
    victim: ApiClient,
    foreign: ApiClient,
    compose: list[str],
    env: dict[str, str],
    collection_id: str,
    seeded_token: str,
    seeded_document_id: str | None,
    cleanup: CleanupStack,
) -> CaseResult:
    cid = case["id"]
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}
    pg_user = env.get("MARKHAND_POSTGRES_USER", "markhand_e2e")
    pg_db = env["MARKHAND_POSTGRES_DB"]

    if cid in {"sec-user-disabled", "sec-user-suspended"}:
        email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
        try:
            disable_user(
                compose,
                postgres_user=pg_user,
                postgres_db=pg_db,
                email=email,
                stack=cleanup,
            )
            probe = victim.post_json(
                "/api/v1/search",
                {"query": seeded_token, "collectionIds": [collection_id], "limit": 5},
            )
            http_statuses.append(probe.status)
            body = probe.json() if probe.body else {}
            hits = (body or {}).get("hits") if isinstance(body, dict) else None
            posts["denied"] = probe.status in (401, 403)
            posts["no_text"] = not hits and seeded_token not in _body_text(probe)
        finally:
            cleanup.run_all()
        ok = posts.get("denied") and posts.get("no_text")
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not ok else "none",
            notes="disabled/suspended user must not receive document text",
        )

    if cid == "sec-membership-removed":
        email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
        org = env.get("MARKHAND_E2E_ORG_ID", "11111111-1111-1111-1111-111111111111")
        try:
            remove_membership(
                compose,
                postgres_user=pg_user,
                postgres_db=pg_db,
                org_id=org,
                email=email,
                role="viewer",
                stack=cleanup,
            )
            probe = victim.get("/api/v1/auth/me")
            http_statuses.append(probe.status)
            posts["denied"] = probe.status in (401, 403)
        finally:
            cleanup.run_all()
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if posts.get("denied") else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not posts.get("denied") else "none",
        )

    if cid == "sec-collection-acl-revoke":
        if not seeded_document_id or not seeded_token:
            return CaseResult(
                id=cid,
                matrix="security",
                status="blocked",
                severity="high",
                notes="ACL revoke requires seeded token/document from production intake path",
                blocker_code="production_intake_not_wired",
            )
        org = env.get("MARKHAND_E2E_ORG_ID", "11111111-1111-1111-1111-111111111111")
        try:
            set_collection_visibility(
                compose,
                postgres_user=pg_user,
                postgres_db=pg_db,
                org_id=org,
                collection_id=collection_id,
                visibility="private",
                previous="org",
                stack=cleanup,
            )
            password = env.get("MARKHAND_E2E_PASSWORD") or env.get("MARKHAND_DEV_PASSWORD") or ""
            email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
            fresh = ApiClient(victim.base_url)
            login = fresh.login(email, password)
            http_statuses.append(login.status)
            probe = fresh.post_json(
                "/api/v1/search",
                {
                    "query": seeded_token,
                    "collectionIds": [collection_id],
                    "limit": 5,
                },
            )
            http_statuses.append(probe.status)
            body_text = _body_text(probe)
            hits = ((probe.json() or {}).get("hits") or []) if probe.body else []
            posts["denied"] = probe.status in (401, 403) or (
                probe.status == 200 and not hits
            )
            posts["no_text"] = seeded_token not in body_text and not any(
                seeded_token in (h.get("snippet") or "") for h in hits
            )
            posts["seeded_document_scoped"] = True
            posts["seeded_token_used"] = True
        finally:
            cleanup.run_all()
        ok = posts.get("denied") and posts.get("no_text")
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not ok else "none",
        )

    if cid in {
        "sec-tombstone-during-query",
        "sec-tombstone-during-stream",
        "sec-historical-permission-revoke",
    }:
        if not seeded_document_id:
            return CaseResult(
                id=cid,
                matrix="security",
                status="blocked",
                notes="requires production intake wiring to create an indexed document via public API",
                severity="high",
                blocker_code="production_intake_not_wired",
            )
        return CaseResult(
            id=cid,
            matrix="security",
            status="blocked",
            notes="tombstone/history matrix requires additional public lifecycle APIs",
            severity="high",
            blocker_code="production_intake_not_wired",
        )

    if cid == "sec-idor-cross-org":
        foreign_doc = env.get("MARKHAND_E2E_FOREIGN_DOCUMENT_ID")
        foreign_ver = env.get("MARKHAND_E2E_FOREIGN_VERSION_ID")
        if not foreign_doc or not foreign_ver:
            return CaseResult(
                id=cid,
                matrix="security",
                status="blocked",
                severity="high",
                notes="IDOR requires actual foreign seeded document/version IDs",
            )
        foreign_get = foreign.get(f"/api/v1/documents/{foreign_doc}")
        http_statuses.append(foreign_get.status)
        # Victim (org A) must not read foreign (org B) seeded document.
        cross = victim.get(f"/api/v1/documents/{foreign_doc}")
        http_statuses.append(cross.status)
        posts["foreign_seeded_id_used"] = True
        posts["denied"] = cross.status in (403, 404)
        body_text = _body_text(cross)
        posts["no_title_leak"] = "title" not in body_text.lower() or cross.status in (
            403,
            404,
        )
        posts["no_version_leak"] = foreign_ver not in body_text
        ok = posts["denied"] and posts["no_title_leak"] and posts["no_version_leak"]
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not ok else "none",
        )

    if cid.startswith("sec-malformed"):
        probes = []
        if "ids" in cid:
            probes.append(admin.get("/api/v1/documents/not-a-uuid"))
            probes.append(admin.get("/api/v1/jobs/@@@"))
        elif "body" in cid:
            probes.append(admin.post_json("/api/v1/search", {"query": "", "limit": 0}))
            probes.append(admin.post_json("/api/v1/ask", {"question": ""}))
        elif "cursors" in cid:
            probes.append(admin.get("/api/v1/documents?cursor=!!!"))
        elif "last-event" in cid:
            probes.append(
                admin.request(
                    "GET",
                    "/api/v1/events/00000000-0000-4000-8000-000000000001",
                    headers={"Last-Event-ID": "not-an-int"},
                )
            )
        for probe in probes:
            http_statuses.append(probe.status)
        posts["all_4xx"] = bool(probes) and all(400 <= p.status < 500 for p in probes)
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if posts["all_4xx"] else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="medium" if not posts.get("all_4xx") else "none",
        )

    if case.get("fixtureId"):
        path = fixture_path(case["fixtureId"])
        if path is None:
            return CaseResult(
                id=cid,
                matrix="security",
                status="fail",
                severity="high",
                notes=f"missing fixture {case['fixtureId']}",
            )
        upload = admin.upload(path, collection_id=collection_id)
        http_statuses.append(upload.status)
        body = upload.json() or {}
        disposition = body.get("disposition")
        expect = case["expect"]
        if expect == "reject":
            posts["rejected"] = upload.status in (400, 413, 415, 422) or disposition == "rejected"
            ok = bool(posts["rejected"])
        elif expect == "reject_or_quarantine":
            # `accepted` must NEVER satisfy reject_or_quarantine.
            posts["contained"] = upload.status < 500 and disposition in (
                "rejected",
                "quarantined",
            )
            posts["not_accepted"] = disposition != "accepted"
            ok = bool(posts["contained"] and posts["not_accepted"])
        elif expect == "untrusted_or_quarantine_no_tool_leak":
            body_text = _body_text(upload)
            posts["http_ok_not_500"] = upload.status != 500 and upload.status < 500
            posts["grounded_or_quarantined"] = disposition in (
                "rejected",
                "quarantined",
                "accepted",
            ) and upload.status < 500
            posts["no_secret_leak"] = "MARKHAND_AUTH_SIGNING_KEY" not in body_text
            posts["no_instruction_leak"] = "ignore previous instructions" not in body_text.lower()
            # 500 never passes.
            ok = all(
                posts.get(k)
                for k in (
                    "http_ok_not_500",
                    "grounded_or_quarantined",
                    "no_secret_leak",
                    "no_instruction_leak",
                )
            )
        else:
            posts["handled"] = upload.status < 500
            ok = bool(posts["handled"])
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high" if not ok else "none",
            notes=expect,
        )

    if cid == "sec-oversize":
        oversize = (
            Path(os.environ.get("TMPDIR", "/tmp"))
            / f"markhand-e2e-oversize-{uuid.uuid4().hex}.bin"
        )
        max_bytes = int(env.get("MARKHAND_MAX_UPLOAD_BYTES", "209715200"))
        if max_bytes > 8 * 1024 * 1024:
            return CaseResult(
                id=cid,
                matrix="security",
                status="blocked",
                notes="oversize live probe requires MARKHAND_MAX_UPLOAD_BYTES<=8MiB on test stack",
                severity="medium",
            )
        oversize.write_bytes(b"A" * (max_bytes + 1024))
        try:
            upload = admin.upload(oversize, collection_id=collection_id, filename="big.bin")
        finally:
            oversize.unlink(missing_ok=True)
        http_statuses.append(upload.status)
        posts["rejected"] = upload.status in (400, 413)
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if posts["rejected"] else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high" if not posts["rejected"] else "none",
        )

    return CaseResult(
        id=cid,
        matrix="security",
        status="blocked",
        notes="unrecognized security case",
        severity="medium",
    )


def run_adversarial_case_live(
    case: dict[str, Any],
    *,
    admin: ApiClient,
    collection_id: str,
) -> CaseResult:
    cid = case["id"]
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}

    if cid == "adv-audio-silence-no-hallucination":
        path = fixture_path(case["fixtureId"])
        if path is None:
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="fail",
                severity="high",
                notes="missing silence adversarial fixture",
            )
        if "silence" not in path.name.lower():
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="fail",
                severity="critical",
                notes="adversarial silence case must use silence fixture",
            )
        upload = admin.upload(path, collection_id=collection_id)
        http_statuses.append(upload.status)
        body = upload.json() if upload.body else {}
        if not isinstance(body, dict):
            body = {}
        disposition = body.get("disposition")
        body_text = _body_text(upload).lower()

        if upload.status == 500:
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="fail",
                http_statuses=http_statuses,
                severity="high",
                notes="500 never passes silence no-hallucination case",
            )

        if disposition in ("rejected", "quarantined") or _audio_explicitly_disabled(
            upload, body
        ):
            posts["no_hallucination"] = True
            posts["contained_or_disabled"] = True
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="pass",
                http_statuses=http_statuses,
                postconditions=posts,
                notes="silence rejected/quarantined/disabled — no hallucinated transcript",
            )

        # If accepted, require production ids + prove empty/non-hallucinated transcript.
        try:
            document_id, version_id, _job_id = extract_production_intake(body)
        except ProductionIntakeNotWired:
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="blocked",
                http_statuses=http_statuses,
                severity="high",
                blocker_code="production_intake_not_wired",
                notes="cannot prove silence transcript without production intake ids",
            )

        preview = admin.get(f"/api/v1/documents/{document_id}/versions/{version_id}/preview")
        http_statuses.append(preview.status)
        markdown = ""
        if preview.status == 200 and isinstance(preview.json(), dict):
            markdown = str((preview.json() or {}).get("markdown") or "")
        lowered = markdown.lower()
        hallucinated = any(marker in lowered for marker in HALLUCINATION_MARKERS)
        posts["preview_checked"] = preview.status in (200, 404, 403)
        posts["no_hallucination"] = (not hallucinated) and (
            not markdown.strip() or len(markdown.strip()) < 8
        )
        posts["no_instruction_leak"] = "ignore previous" not in body_text
        if not posts["preview_checked"]:
            return CaseResult(
                id=cid,
                matrix="adversarial",
                status="blocked",
                http_statuses=http_statuses,
                postconditions=posts,
                severity="high",
                notes="API cannot prove silence transcript postcondition",
            )
        ok = posts["no_hallucination"] and posts["no_instruction_leak"]
        return CaseResult(
            id=cid,
            matrix="adversarial",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high" if not ok else "none",
            notes="silence must not hallucinate spoken content",
        )

    return CaseResult(
        id=cid,
        matrix="adversarial",
        status="blocked",
        notes="unrecognized adversarial case",
        severity="medium",
    )


def _job_phase_observed(payload: dict[str, Any], phase: str) -> bool:
    status = str(payload.get("status") or "").lower()
    checkpoint = payload.get("checkpoint") or payload.get("progress") or {}
    if phase == "after_claim":
        return status in {"claimed", "running", "leased", "in_progress"} or bool(
            payload.get("claimedAt") or payload.get("leaseOwner")
        )
    if phase == "after_checkpoint":
        return bool(checkpoint) or status in {"running", "checkpointed"} or bool(
            payload.get("checkpointSeq") or payload.get("lastCheckpointAt")
        )
    return False


def run_fault_case_live(
    case: dict[str, Any],
    *,
    compose: list[str],
    client: ApiClient,
    collection_id: str,
    cleanup: CleanupStack,
) -> CaseResult:
    service = case["targetService"]
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}

    if case["id"].startswith("fault-kill-"):
        # Need a live convert/index job to observe claim/checkpoint before kill.
        path = fixture_path("e2e-vi-txt")
        if path is None:
            return CaseResult(
                id=case["id"],
                matrix="fault",
                status="blocked",
                severity="high",
                notes="missing fixture for fault job observation",
            )
        upload = client.upload(
            path,
            collection_id=collection_id,
            idempotency_key=f"e2e-fault-{case['id']}-{uuid.uuid4().hex[:8]}",
        )
        http_statuses.append(upload.status)
        body = upload.json() if upload.body else {}
        if not isinstance(body, dict):
            body = {}
        try:
            document_id, version_id, job_id = extract_production_intake(body)
        except ProductionIntakeNotWired:
            return CaseResult(
                id=case["id"],
                matrix="fault",
                status="blocked",
                http_statuses=http_statuses,
                severity="critical",
                blocker_code="production_intake_not_wired",
                notes="fault kill requires production intake ids",
            )

        target_job_id = job_id
        if "index" in case["id"]:
            # Wait until an index job appears for this document/version.
            found_index: dict[str, str] = {}

            def index_job_ready() -> bool:
                jobs = client.get("/api/v1/jobs?limit=50")
                http_statuses.append(jobs.status)
                items = (jobs.json() or {}).get("items") or []
                for item in items:
                    if (
                        item.get("jobType") == "index"
                        and item.get("documentId") == document_id
                        and item.get("versionId") == version_id
                        and isinstance(item.get("id"), str)
                    ):
                        found_index["id"] = item["id"]
                        return True
                return False

            try:
                wait_until(index_job_ready, timeout_secs=120, label="index job appear")
            except TimeoutError:
                return CaseResult(
                    id=case["id"],
                    matrix="fault",
                    status="blocked",
                    http_statuses=http_statuses,
                    severity="high",
                    notes="API cannot prove index job claimed before kill",
                )
            if "id" not in found_index:
                return CaseResult(
                    id=case["id"],
                    matrix="fault",
                    status="blocked",
                    severity="high",
                    notes="index job id unavailable via supported API",
                )
            target_job_id = found_index["id"]

        # Observe claimed/running/checkpointed BEFORE kill.
        observed = {"ok": False}

        def phase_ready() -> bool:
            job = client.get(f"/api/v1/jobs/{target_job_id}")
            http_statuses.append(job.status)
            if job.status != 200:
                return False
            payload = job.json() or {}
            if _job_phase_observed(payload, case["phase"]):
                observed["ok"] = True
                posts["phase_observed_before_kill"] = True
                posts["job_status_before_kill"] = True
                return True
            # Already terminal before we could observe — cannot claim fault pass.
            if payload.get("status") in {"succeeded", "failed", "dead_letter", "cancelled"}:
                return True
            return False

        try:
            wait_until(phase_ready, timeout_secs=90, label=f"observe {case['phase']}")
        except TimeoutError:
            return CaseResult(
                id=case["id"],
                matrix="fault",
                status="blocked",
                http_statuses=http_statuses,
                postconditions=posts,
                severity="high",
                notes=f"could not observe job {case['phase']} before kill",
            )

        if not observed["ok"]:
            return CaseResult(
                id=case["id"],
                matrix="fault",
                status="blocked",
                http_statuses=http_statuses,
                postconditions=posts,
                severity="high",
                notes=f"job reached terminal state before {case['phase']} observation",
            )

        try:
            kill_and_restart_service(compose, service, cleanup)
            time.sleep(2)

            def reclaimed() -> bool:
                job = client.get(f"/api/v1/jobs/{target_job_id}")
                http_statuses.append(job.status)
                if job.status != 200:
                    return False
                payload = job.json() or {}
                status = payload.get("status")
                posts["lease_reclaim_or_retry"] = status in {
                    "queued",
                    "claimed",
                    "running",
                    "succeeded",
                    "leased",
                } or bool(payload.get("attempt") and int(payload.get("attempt") or 0) >= 1)
                return bool(posts["lease_reclaim_or_retry"])

            try:
                wait_until(reclaimed, timeout_secs=120, label="lease reclaim")
            except TimeoutError:
                posts["lease_reclaim_or_retry"] = False

            # Same idempotency identity: document/version unchanged.
            doc = client.get(f"/api/v1/documents/{document_id}")
            http_statuses.append(doc.status)
            versions = client.get(f"/api/v1/documents/{document_id}/versions")
            http_statuses.append(versions.status)
            version_items = []
            if versions.status == 200:
                version_items = (versions.json() or {}).get("items") or (
                    versions.json() or {}
                ).get("versions") or []
            if isinstance(version_items, list) and version_items:
                posts["exactly_one_visible_version"] = len(version_items) == 1 and any(
                    (v.get("id") == version_id) for v in version_items
                )
            else:
                # If versions list API unavailable, require document current pointer only.
                doc_body = doc.json() or {}
                if doc.status == 200 and doc_body.get("currentVersionId") == version_id:
                    posts["exactly_one_visible_version"] = True
                    posts["versions_api_limited"] = True
                else:
                    posts["exactly_one_visible_version"] = False

            posts["same_idempotency_identity"] = (
                doc.status == 200
                and (doc.json() or {}).get("id") == document_id
            )

            # Chunk duplicate check via search hits for this document.
            search = client.post_json(
                "/api/v1/search",
                {"query": "MAHOA", "collectionIds": [collection_id], "limit": 50},
            )
            http_statuses.append(search.status)
            hits = (search.json() or {}).get("hits") or []
            chunk_ids = [
                h.get("chunkId")
                for h in hits
                if h.get("documentId") == document_id and h.get("chunkId")
            ]
            posts["no_duplicate_chunk_ids"] = len(chunk_ids) == len(set(chunk_ids))
            if search.status not in (200, 503):
                return CaseResult(
                    id=case["id"],
                    matrix="fault",
                    status="blocked",
                    http_statuses=http_statuses,
                    postconditions=posts,
                    severity="high",
                    notes="search API cannot prove chunk uniqueness postcondition",
                )

            posts["no_partial_trusted_artifact"] = all(
                h.get("versionId") == version_id
                for h in hits
                if h.get("documentId") == document_id
            )
        finally:
            try:
                cleanup.run_all()
            except CleanupFailed as exc:
                return CaseResult(
                    id=case["id"],
                    matrix="fault",
                    status="fail",
                    http_statuses=http_statuses,
                    postconditions=posts,
                    severity="critical",
                    notes=f"cleanup_failed: {exc}",
                )

        required = [
            "phase_observed_before_kill",
            "lease_reclaim_or_retry",
            "same_idempotency_identity",
            "exactly_one_visible_version",
            "no_duplicate_chunk_ids",
            "no_partial_trusted_artifact",
        ]
        if any(k not in posts for k in required):
            return CaseResult(
                id=case["id"],
                matrix="fault",
                status="blocked",
                http_statuses=http_statuses,
                postconditions=posts,
                severity="high",
                notes="API cannot prove one or more fault postconditions",
            )
        ok = all(posts.get(k) for k in required)
        return CaseResult(
            id=case["id"],
            matrix="fault",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high" if not ok else "none",
            notes=case["expect"],
        )

    if case["id"] == "fault-dependency-outage-bounded":
        try:
            stop_service(compose, service, cleanup)
            time.sleep(1)
            search = client.post_json("/api/v1/search", {"query": "ping", "limit": 3})
            http_statuses.append(search.status)
            posts["bounded"] = search.status in (200, 503, 502)
            # 500 with trusted partial body is not acceptable.
            body = _body_text(search)
            posts["no_stack_leak"] = "postgres://" not in body and "TRACE" not in body
            posts["no_partial_trusted"] = "hits" not in body or search.status in (200, 503)
        finally:
            try:
                cleanup.run_all()
            except CleanupFailed as exc:
                return CaseResult(
                    id=case["id"],
                    matrix="fault",
                    status="fail",
                    http_statuses=http_statuses,
                    postconditions=posts,
                    severity="critical",
                    notes=f"cleanup_failed: {exc}",
                )
        try:
            wait_until(
                lambda: client.health_ready().status == 200,
                timeout_secs=120,
                label="ready after dependency restart",
            )
            posts["recovered"] = True
        except TimeoutError:
            posts["recovered"] = False
        ok = all(posts.get(k) for k in ("bounded", "no_stack_leak", "no_partial_trusted", "recovered"))
        return CaseResult(
            id=case["id"],
            matrix="fault",
            status="pass" if ok else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="high" if not ok else "none",
        )

    return CaseResult(
        id=case["id"],
        matrix="fault",
        status="blocked",
        notes="unrecognized fault case",
        severity="medium",
    )


def run_live(environ: dict[str, str] | None = None) -> dict[str, Any]:
    env = dict(os.environ if environ is None else environ)
    suite = load_suite_manifest()
    require_live_gates(confirm_phrase=suite["confirmPhrase"], environ=env)

    api_port = env.get("MARKHAND_API_PORT", "8788")
    base = env.get("MARKHAND_E2E_BASE_URL", f"http://127.0.0.1:{api_port}")
    client = ApiClient(base)
    ready = client.health_ready()
    if ready.status != 200:
        raise RuntimeError(f"API not ready (status={ready.status}); refusing live suite")

    password = env.get("MARKHAND_E2E_PASSWORD") or env.get("MARKHAND_DEV_PASSWORD")
    if not password:
        raise RuntimeError("MARKHAND_E2E_PASSWORD (or MARKHAND_DEV_PASSWORD) required")

    admin_email = env.get("MARKHAND_E2E_ADMIN_EMAIL", "admin@poc.example")
    viewer_email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
    foreign_email = env.get("MARKHAND_E2E_FOREIGN_EMAIL", "owner@org-b.example")
    collection_id = env.get(
        "MARKHAND_E2E_COLLECTION_ID", "55555555-5555-5555-5555-555555555501"
    )

    admin = ApiClient(base)
    login = admin.login(admin_email, password)
    if login.status != 200:
        raise RuntimeError(f"admin login failed status={login.status}")

    victim = ApiClient(base)
    vlogin = victim.login(viewer_email, password)
    if vlogin.status != 200:
        raise RuntimeError(f"viewer login failed status={vlogin.status}")

    foreign = ApiClient(base)
    flogin = foreign.login(foreign_email, password)
    if flogin.status != 200:
        raise RuntimeError(f"foreign login failed status={flogin.status}")

    compose_json = env.get("MARKHAND_E2E_COMPOSE_JSON")
    if not compose_json:
        raise RuntimeError("MARKHAND_E2E_COMPOSE_JSON missing (deploy script must set it)")
    compose = json.loads(compose_json)

    global_cleanup = CleanupStack()
    cases: list[CaseResult] = []
    blockers: list[str] = []

    try:
        # --- Intake probe: abort before any security mutations / worker kills ---
        wired, detail, probe_statuses = probe_production_intake(
            client=admin,
            collection_id=collection_id,
            suite=suite,
        )
        if not wired:
            blockers.append(f"production_intake_not_wired: {detail}")
            blockers.append(
                "aborted before security mutations, worker kills, and downstream calls"
            )
            cases = _abort_matrix_for_intake(
                suite, detail=detail, http_statuses=probe_statuses
            )
            claims_live, claim_errors = evaluate_claims_live_vertical_slice(suite, cases)
            if claim_errors:
                blockers.extend(claim_errors[:12])
            report = build_report(
                root=REPO_ROOT,
                mode="live",
                cases=cases,
                blockers=blockers,
                claims_live=False,
            )
            write_live_runtime(REPO_ROOT, report)
            # Keep tracked hermetic report deterministic / blocked.
            write_tracked_hermetic(
                REPO_ROOT,
                run_hermetic_blocked_report(
                    extra_blockers=[
                        "live run aborted: production_intake_not_wired",
                    ]
                ),
            )
            raise RuntimeError(
                "P1B-O04 live suite aborted: production_intake_not_wired "
                f"({detail})"
            )

        seeded_token = next(
            f["uniqueToken"]
            for f in suite["formats"]
            if f.get("requirement") == "required" and f.get("uniqueToken")
        )
        seeded_document_id: str | None = None

        for fmt in suite["formats"]:
            result = run_format_case_live(
                fmt,
                client=admin,
                collection_id=collection_id,
                env=env,
            )
            cases.append(result)
            if (
                result.status == "pass"
                and result.opaque_refs.get("documentFp")
                and seeded_document_id is None
            ):
                # Best-effort: recover document id from last successful format via jobs list.
                jobs = admin.get("/api/v1/jobs?limit=20")
                items = (jobs.json() or {}).get("items") or []
                for item in items:
                    fp = (item.get("documentId") or "").replace("-", "")[:12]
                    if fp == result.opaque_refs.get("documentFp"):
                        seeded_document_id = item.get("documentId")
                        break

        for sec in suite["security"]:
            case_cleanup = CleanupStack()
            try:
                cases.append(
                    run_security_case_live(
                        sec,
                        admin=admin,
                        victim=victim,
                        foreign=foreign,
                        compose=compose,
                        env=env,
                        collection_id=collection_id,
                        seeded_token=seeded_token,
                        seeded_document_id=seeded_document_id,
                        cleanup=case_cleanup,
                    )
                )
            except CleanupFailed as exc:
                cases.append(
                    CaseResult(
                        id=sec["id"],
                        matrix="security",
                        status="fail",
                        severity="critical",
                        notes=f"cleanup_failed: {exc}",
                    )
                )

        for adv in suite.get("adversarial") or []:
            cases.append(
                run_adversarial_case_live(
                    adv, admin=admin, collection_id=collection_id
                )
            )

        for fault in suite["fault"]:
            case_cleanup = CleanupStack()
            try:
                cases.append(
                    run_fault_case_live(
                        fault,
                        compose=compose,
                        client=admin,
                        collection_id=collection_id,
                        cleanup=case_cleanup,
                    )
                )
            except CleanupFailed as exc:
                cases.append(
                    CaseResult(
                        id=fault["id"],
                        matrix="fault",
                        status="fail",
                        severity="critical",
                        notes=f"cleanup_failed: {exc}",
                    )
                )

    finally:
        try:
            global_cleanup.run_all()
        except CleanupFailed as exc:
            blockers.append(f"cleanup_failed: {exc}")

    claims_live, claim_errors = evaluate_claims_live_vertical_slice(suite, cases)
    if not claims_live:
        blockers.append("live vertical slice incomplete or failed")
        blockers.extend(claim_errors[:20])

    report = build_report(
        root=REPO_ROOT,
        mode="live",
        cases=cases,
        blockers=blockers,
        claims_live=claims_live,
    )
    write_live_runtime(REPO_ROOT, report)
    if report["summary"]["highCritical"] > 0 or not claims_live:
        raise RuntimeError(
            "P1B-O04 live suite failed "
            f"(claimsLiveVerticalSlice={claims_live}, "
            f"highCritical={report['summary']['highCritical']})"
        )
    return report


def run_hermetic_blocked_report(extra_blockers: list[str] | None = None) -> dict[str, Any]:
    suite = load_suite_manifest()
    cases = [
        CaseResult(
            id="harness-manifest",
            matrix="harness",
            status="pass",
            notes="hermetic harness validation only — not a live vertical-slice pass",
        )
    ]
    for fmt in suite["formats"]:
        if fmt.get("requirement") == "required":
            cases.append(
                CaseResult(
                    id=fmt["id"],
                    matrix="format",
                    status="blocked",
                    notes="blocked: Docker unavailable; production intake wiring unverified",
                    severity="high",
                    blocker_code="production_intake_not_wired",
                )
            )
        else:
            cases.append(
                CaseResult(
                    id=fmt["id"],
                    matrix="format",
                    status="optional_unavailable",
                    notes=(
                        "optional spoken-audio coverage requires approved spoken-token "
                        "fixture/model; silence cannot satisfy all-formats claim"
                    ),
                    severity="none",
                )
            )
    for sec in suite["security"]:
        cases.append(
            CaseResult(
                id=sec["id"],
                matrix="security",
                status="blocked",
                notes="live stack unavailable in this environment",
                severity="medium",
            )
        )
    for adv in suite.get("adversarial") or []:
        cases.append(
            CaseResult(
                id=adv["id"],
                matrix="adversarial",
                status="blocked",
                notes="live stack unavailable in this environment",
                severity="medium",
            )
        )
    for fault in suite["fault"]:
        cases.append(
            CaseResult(
                id=fault["id"],
                matrix="fault",
                status="blocked",
                notes="live stack unavailable in this environment",
                severity="medium",
            )
        )
    blockers = [
        "Hermetic harness validation only — not a live vertical-slice pass",
        "Docker/Compose unavailable — live suite not executed",
        "production_intake_not_wired — current /api/v1/uploads returns objectId only "
        "(no documentId/versionId/jobId; no supported follow-up public API)",
        "claimsLiveVerticalSlice remains false",
        *list(extra_blockers or []),
    ]
    report = build_report(
        root=REPO_ROOT,
        mode="hermetic",
        cases=cases,
        blockers=blockers,
        claims_live=False,
    )
    write_tracked_hermetic(REPO_ROOT, report)
    return report
