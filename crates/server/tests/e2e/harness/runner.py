"""Manifest-driven O04 runner (hermetic contracts + live orchestration).

Live path uses public `/api/v1` only. Missing production intake identities
(`documentId`/`versionId`/`jobId`) abort the suite with a high/critical blocker.
"""

from __future__ import annotations

import json
import os
import time
import uuid
from pathlib import Path
from typing import Any, Callable

from .api_client import ApiClient
from .compose_util import run_compose
from .confirm import require_live_gates
from .evidence import CaseResult, build_report, write_json_report, write_markdown_report
from .intake import ProductionIntakeNotWired, extract_production_intake

E2E_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = E2E_ROOT.parents[3]

AUDIO_DISABLE_MARKERS = (
    "audioconversiondisabled",
    "audio_conversion_disabled",
    "convert audio disabled",
    "audio conversion is disabled",
)


def load_suite_manifest(path: Path | None = None) -> dict[str, Any]:
    manifest_path = path or (E2E_ROOT / "manifest.json")
    return json.loads(manifest_path.read_text(encoding="utf-8"))


def load_fixture_manifest() -> dict[str, Any]:
    return json.loads((E2E_ROOT / "fixtures" / "manifest.json").read_text(encoding="utf-8"))


def fixture_path(fixture_id: str) -> Path:
    fixtures = load_fixture_manifest()["fixtures"]
    for item in fixtures:
        if item["id"] == fixture_id:
            return E2E_ROOT / "fixtures" / item["path"]
    raise KeyError(fixture_id)


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
    token = case["uniqueToken"]
    path = fixture_path(case["fixtureId"])

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
            blocker_code=None,
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

    # Fail-honest: require production identities from the public upload contract.
    document_id, version_id, job_id = extract_production_intake(body)
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

    # Await index via document/jobs for this exact document/version.
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
    # Exact postconditions only — never accept an unrelated hit.
    posts["search_hit"] = search.status == 200 and any(
        h.get("documentId") == document_id
        and h.get("versionId") == version_id
        and token in (h.get("snippet") or "")
        for h in hits
    )

    ask = client.post_json(
        "/api/v1/ask",
        {
            "question": f"Mã truy vết trong tài liệu là gì? {token}",
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
            posts["citation_hash_present"] = len(item.get("contentSha256") or "") == 64
            posts["citation_span_exact"] = bool(
                isinstance(item.get("spanStart"), int)
                and isinstance(item.get("spanEnd"), int)
                and item["spanEnd"] >= item["spanStart"]
            )
    else:
        posts["citation_resolves"] = False

    preview = client.get(f"/api/v1/documents/{document_id}/versions/{version_id}/preview")
    http_statuses.append(preview.status)
    preview_body = preview.json() or {}
    # Do not retain markdown in evidence; assert token presence then drop.
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
) -> CaseResult:
    cid = case["id"]
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}

    if cid in {"sec-user-disabled", "sec-user-suspended"}:
        email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                f"UPDATE users SET disabled_at = now() WHERE email = '{email}';",
            ],
        )
        probe = victim.post_json("/api/v1/search", {"query": "x", "limit": 5})
        http_statuses.append(probe.status)
        posts["denied"] = probe.status in (401, 403)
        posts["no_hits_body"] = "hits" not in ((probe.json() or {}) if probe.body else {})
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                f"UPDATE users SET disabled_at = NULL WHERE email = '{email}';",
            ],
        )
        ok = posts["denied"]
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
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                f"DELETE FROM org_memberships WHERE org_id = '{org}' AND user_id = ("
                f"SELECT id FROM users WHERE email = '{email}');",
            ],
        )
        probe = victim.get("/api/v1/auth/me")
        http_statuses.append(probe.status)
        posts["denied"] = probe.status in (401, 403)
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                (
                    "INSERT INTO org_memberships (org_id, user_id, role) "
                    f"SELECT '{org}', id, 'viewer' FROM users WHERE email = '{email}' "
                    "ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;"
                ),
            ],
        )
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if posts["denied"] else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not posts["denied"] else "none",
        )

    if cid == "sec-collection-acl-revoke":
        org = env.get("MARKHAND_E2E_ORG_ID", "11111111-1111-1111-1111-111111111111")
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                (
                    f"SELECT set_config('app.org_id', '{org}', true); "
                    f"UPDATE collections SET visibility = 'private' WHERE id = '{collection_id}'; "
                    f"DELETE FROM collection_user_access WHERE collection_id = '{collection_id}';"
                ),
            ],
        )
        password = env.get("MARKHAND_E2E_PASSWORD") or env.get("MARKHAND_DEV_PASSWORD") or ""
        email = env.get("MARKHAND_E2E_VIEWER_EMAIL", "viewer-e2e@poc.example")
        fresh = ApiClient(victim.base_url)
        login = fresh.login(email, password)
        http_statuses.append(login.status)
        probe = fresh.post_json(
            "/api/v1/search",
            {"query": "x", "collectionIds": [collection_id], "limit": 5},
        )
        http_statuses.append(probe.status)
        posts["denied"] = probe.status in (401, 403) or (
            probe.status == 200 and not ((probe.json() or {}).get("hits") or [])
        )
        run_compose(
            compose,
            [
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                env.get("MARKHAND_POSTGRES_USER", "markhand"),
                "-d",
                env["MARKHAND_POSTGRES_DB"],
                "--set",
                "ON_ERROR_STOP=1",
                "-c",
                (
                    f"SELECT set_config('app.org_id', '{org}', true); "
                    f"UPDATE collections SET visibility = 'org' WHERE id = '{collection_id}';"
                ),
            ],
        )
        return CaseResult(
            id=cid,
            matrix="security",
            status="pass" if posts["denied"] else "fail",
            http_statuses=http_statuses,
            postconditions=posts,
            severity="critical" if not posts["denied"] else "none",
        )

    if cid in {
        "sec-tombstone-during-query",
        "sec-tombstone-during-stream",
        "sec-historical-permission-revoke",
    }:
        return CaseResult(
            id=cid,
            matrix="security",
            status="blocked",
            notes="requires production intake wiring to create an indexed document via public API",
            severity="high",
            blocker_code="production_intake_not_wired",
        )

    if cid == "sec-idor-cross-org":
        # Probe a random UUID — foreign org must not learn existence via body leak.
        fake = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        foreign_get = foreign.get(f"/api/v1/documents/{fake}")
        http_statuses.append(foreign_get.status)
        posts["denied"] = foreign_get.status in (403, 404)
        body_text = _body_text(foreign_get)
        posts["no_title_leak"] = "title" not in body_text or foreign_get.status in (403, 404)
        ok = posts["denied"]
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
        upload = admin.upload(path, collection_id=collection_id)
        http_statuses.append(upload.status)
        body = upload.json() or {}
        disposition = body.get("disposition")
        expect = case["expect"]
        if expect == "reject":
            posts["rejected"] = upload.status in (400, 413, 415, 422) or disposition == "rejected"
        elif expect == "reject_or_quarantine":
            posts["contained"] = upload.status < 500 and disposition in (
                "rejected",
                "quarantined",
                "accepted",
            )
        elif expect == "untrusted_or_quarantine_no_tool_leak":
            posts["contained"] = upload.status < 500
            posts["no_tool_leak"] = "MARKHAND_AUTH_SIGNING_KEY" not in _body_text(upload)
        else:
            posts["handled"] = upload.status < 500
        ok = any(posts.values())
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
        oversize = Path(os.environ.get("TMPDIR", "/tmp")) / f"markhand-e2e-oversize-{uuid.uuid4().hex}.bin"
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


def run_fault_case_live(
    case: dict[str, Any],
    *,
    compose: list[str],
    client: ApiClient,
) -> CaseResult:
    service = case["targetService"]
    http_statuses: list[int] = []
    posts: dict[str, bool] = {}

    if case["id"].startswith("fault-kill-"):
        run_compose(compose, ["kill", service], check=False)
        time.sleep(2)
        run_compose(compose, ["up", "-d", service])

        def ready() -> bool:
            health = client.health_ready()
            http_statuses.append(health.status)
            return health.status == 200

        try:
            wait_until(ready, timeout_secs=120, label=f"ready after kill {service}")
            posts["api_ready"] = True
        except TimeoutError:
            posts["api_ready"] = False
        ps = run_compose(compose, ["ps", "--status", "running", service], check=False)
        posts["worker_running"] = service in ps
        jobs = client.get("/api/v1/jobs?limit=5")
        http_statuses.append(jobs.status)
        posts["jobs_ok"] = jobs.status == 200
        ok = posts.get("api_ready") and posts.get("worker_running") and posts.get("jobs_ok")
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
        run_compose(compose, ["stop", service], check=False)
        time.sleep(1)
        search = client.post_json("/api/v1/search", {"query": "ping", "limit": 3})
        http_statuses.append(search.status)
        posts["bounded"] = search.status in (200, 503, 502, 500)
        body = _body_text(search)
        posts["no_stack_leak"] = "postgres://" not in body and "TRACE" not in body
        run_compose(compose, ["start", service], check=False)
        try:
            wait_until(
                lambda: client.health_ready().status == 200,
                timeout_secs=120,
                label="ready after dependency restart",
            )
            posts["recovered"] = True
        except TimeoutError:
            posts["recovered"] = False
        ok = posts["bounded"] and posts["no_stack_leak"] and posts["recovered"]
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


def _fail_formats_intake_not_wired(
    suite: dict[str, Any],
    *,
    detail: str,
    http_statuses: list[int] | None = None,
) -> list[CaseResult]:
    cases: list[CaseResult] = []
    for fmt in suite["formats"]:
        cases.append(
            CaseResult(
                id=fmt["id"],
                matrix="format",
                status="fail",
                http_statuses=list(http_statuses or []),
                postconditions={"production_intake_ids": False},
                severity="critical",
                blocker_code="production_intake_not_wired",
                notes=detail,
            )
        )
    return cases


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

    cases: list[CaseResult] = []
    blockers: list[str] = []
    intake_wired = True

    try:
        for fmt in suite["formats"]:
            cases.append(
                run_format_case_live(
                    fmt,
                    client=admin,
                    collection_id=collection_id,
                    env=env,
                )
            )
    except ProductionIntakeNotWired as exc:
        intake_wired = False
        detail = str(exc)
        blockers.append(f"production_intake_not_wired: {detail}")
        # Replace any partial format results with a full fail-honest matrix.
        cases = [
            c for c in cases if c.matrix != "format"
        ] + _fail_formats_intake_not_wired(suite, detail=detail)

    if intake_wired:
        for sec in suite["security"]:
            cases.append(
                run_security_case_live(
                    sec,
                    admin=admin,
                    victim=victim,
                    foreign=foreign,
                    compose=compose,
                    env=env,
                    collection_id=collection_id,
                )
            )
        for fault in suite["fault"]:
            cases.append(run_fault_case_live(fault, compose=compose, client=admin))
    else:
        # Do not continue mutating document lifecycle without production intake.
        # Upload-abuse / malformed probes are still public-API-only and useful.
        for sec in suite["security"]:
            if sec["id"].startswith("sec-malformed") or sec.get("fixtureId") or sec["id"] == "sec-oversize":
                cases.append(
                    run_security_case_live(
                        sec,
                        admin=admin,
                        victim=victim,
                        foreign=foreign,
                        compose=compose,
                        env=env,
                        collection_id=collection_id,
                    )
                )
            elif sec["id"] in {
                "sec-tombstone-during-query",
                "sec-tombstone-during-stream",
                "sec-historical-permission-revoke",
            }:
                cases.append(
                    CaseResult(
                        id=sec["id"],
                        matrix="security",
                        status="blocked",
                        severity="high",
                        blocker_code="production_intake_not_wired",
                        notes="blocked: production intake not wired",
                    )
                )
            else:
                cases.append(
                    run_security_case_live(
                        sec,
                        admin=admin,
                        victim=victim,
                        foreign=foreign,
                        compose=compose,
                        env=env,
                        collection_id=collection_id,
                    )
                )
        for fault in suite["fault"]:
            cases.append(run_fault_case_live(fault, compose=compose, client=admin))

    high = sum(
        1
        for c in cases
        if c.severity in {"high", "critical"} and c.status in {"fail", "blocked"}
    )
    failed = sum(1 for c in cases if c.status == "fail")
    claims_live = (
        intake_wired
        and failed == 0
        and high == 0
        and all(
            c.status in {"pass", "optional_unavailable"}
            for c in cases
            if c.matrix == "format"
        )
    )
    if not claims_live:
        blockers.append("live vertical slice incomplete or failed")
    if not intake_wired:
        blockers.append(
            "production upload contract returns objectId only — "
            "documentId/versionId/jobId required for vertical slice"
        )

    report = build_report(
        root=REPO_ROOT,
        mode="live",
        cases=cases,
        blockers=blockers,
        claims_live=claims_live,
    )
    write_json_report(
        report, REPO_ROOT / "bench/markhand_web/reports/p1b-o04-vertical-slice.json"
    )
    write_markdown_report(report, REPO_ROOT / suite["evidence"]["reportPath"])
    if report["summary"]["highCritical"] > 0 or failed or not intake_wired:
        raise RuntimeError(
            "P1B-O04 live suite failed "
            f"(failed={failed}, highCritical={report['summary']['highCritical']}, "
            f"intake_wired={intake_wired})"
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
    write_json_report(
        report, REPO_ROOT / "bench/markhand_web/reports/p1b-o04-vertical-slice.json"
    )
    write_markdown_report(report, REPO_ROOT / suite["evidence"]["reportPath"])
    return report
