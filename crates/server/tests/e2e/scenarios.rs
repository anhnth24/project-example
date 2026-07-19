use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use fileconv_server::db::models::JobStatus;
use fileconv_server::services::promotion::PromotionFault;
use fileconv_server::workers::convert::{ConvertWorker, ConvertWorkerPause, ConvertWorkerRun};
use fileconv_server::workers::delete::DeleteWorkerRun;
use fileconv_server::workers::index::IndexWorkerRun;
use serde_json::{json, Value};
use tokio::sync::Notify;
use uuid::Uuid;

use super::harness::*;

#[tokio::test]
async fn live_full_vertical_slice_multi_format() {
    let _llm = LlmEnvGuard::unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed_org(&env).await;
    let owner = env.token(&seeded.owner_email).await;

    for case in fixture_cases() {
        let Some(doc) = ingest_document(&env, &seeded, &owner, case).await else {
            env.drop().await;
            return;
        };
        assert_grounded_http_roundtrip(&env, &owner, &doc).await;
        eprintln!("e2e format passed: {}", case.name);
    }

    env.drop().await;
}

#[tokio::test]
async fn live_authorization_e2e_unauthorized_gets_no_text() {
    let _llm = LlmEnvGuard::unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed_org(&env).await;
    let owner = env.token(&seeded.owner_email).await;
    let no_acl = env.token(&seeded.no_acl_email).await;
    let no_query = env.token(&seeded.no_query_email).await;
    let cross = env.token(&seeded.cross_email).await;
    let marker = "E2E-AUTH-SECRET-2026";
    let cross_query = "E2E-CROSS-TENANT-SHARED-2026";
    let cross_visible_marker = "E2E-CROSS-TENANT-ORG-B-2026";
    let Some(doc) = ingest_document(
        &env,
        &seeded,
        &owner,
        FixtureCase {
            name: "auth",
            filename: "auth-secret.txt",
            content_type: "text/plain",
            title: "Authorization Secret",
            bytes: b"E2E-CROSS-TENANT-SHARED-2026 names an org-A document. E2E-AUTH-SECRET-2026 must not leak to unauthorized users.\n",
            marker,
        },
    )
    .await
    else {
        env.drop().await;
        return;
    };
    let citation = first_citation(&env, &owner, &doc).await;
    let pin = citation_pin_from(&citation, marker);
    let cross_ctx = seeded.cross_worker_ctx();
    let Some(cross_doc) = ingest_document_in_collection(
        &env,
        &cross,
        seeded.cross_collection_id,
        &cross_ctx,
        FixtureCase {
            name: "cross-tenant-visible",
            filename: "cross-visible.txt",
            content_type: "text/plain",
            title: "Cross Tenant Visible",
            bytes: b"E2E-CROSS-TENANT-SHARED-2026 names only org-B readable content. E2E-CROSS-TENANT-ORG-B-2026 is allowed.\n",
            marker: cross_visible_marker,
        },
    )
    .await
    else {
        env.drop().await;
        return;
    };

    let (status, cross_search) = search(&env, &cross, cross_query, None).await;
    assert_eq!(status, StatusCode::OK, "{cross_search}");
    let cross_hits = cross_search["hits"].as_array().expect("cross search hits");
    assert!(
        cross_hits
            .iter()
            .any(|hit| hit["documentId"] == cross_doc.document_id.to_string()),
        "cross-tenant control search did not retrieve org-B content"
    );
    assert!(
        cross_hits
            .iter()
            .all(|hit| hit["documentId"] != doc.document_id.to_string()
                && hit["collectionId"] != doc.collection_id.to_string()),
        "cross-tenant search leaked org-A identifiers: {cross_search}"
    );
    assert_value_lacks(&cross_search, marker);

    let (status, cross_ask) = ask(&env, &cross, cross_query, None).await;
    assert_eq!(status, StatusCode::OK, "{cross_ask}");
    let cross_citations = cross_ask["citations"]
        .as_array()
        .expect("cross ask citations");
    assert!(
        cross_citations
            .iter()
            .any(|citation| citation["documentId"] == cross_doc.document_id.to_string()),
        "cross-tenant control ask did not cite org-B content"
    );
    assert!(
        cross_citations.iter().all(|citation| citation["documentId"]
            != doc.document_id.to_string()
            && citation["collectionId"] != doc.collection_id.to_string()),
        "cross-tenant ask leaked org-A identifiers: {cross_ask}"
    );
    assert_value_lacks(&cross_ask, marker);

    let (status, no_acl_search) = search(&env, &no_acl, marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{no_acl_search}");
    assert_eq!(no_acl_search["code"], "empty_scope");
    assert_value_lacks(&no_acl_search, marker);

    let (status, no_acl_ask) = ask(&env, &no_acl, marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{no_acl_ask}");
    assert_eq!(no_acl_ask["code"], "empty_scope");
    assert_value_lacks(&no_acl_ask, marker);

    let (status, denied_search) = search(&env, &no_query, marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied_search}");
    assert_eq!(denied_search["code"], "permission_denied");
    assert_value_lacks(&denied_search, marker);

    let (status, denied_ask) = ask(&env, &no_query, marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied_ask}");
    assert_eq!(denied_ask["code"], "permission_denied");
    assert_value_lacks(&denied_ask, marker);

    for token in [&no_acl, &cross] {
        assert_not_found_json(
            json_request(
                env.app.clone(),
                "GET",
                &format!("/api/v1/documents/{}", doc.document_id),
                None,
                token,
            )
            .await,
            marker,
        );
        assert_not_found_json(
            json_request(
                env.app.clone(),
                "GET",
                &format!("/api/v1/jobs/{}", doc.convert_job_id),
                None,
                token,
            )
            .await,
            marker,
        );
        assert_not_found_raw(
            send_request(
                env.app.clone(),
                "GET",
                &format!(
                    "/api/v1/documents/{}/versions/{}/preview",
                    doc.document_id, doc.version_id
                ),
                Body::empty(),
                Some(token),
                None,
            )
            .await,
            marker,
        );
        assert_not_found_json(
            json_request(
                env.app.clone(),
                "POST",
                &format!(
                    "/api/v1/documents/{}/versions/{}/citations:resolve",
                    doc.document_id, doc.version_id
                ),
                Some(pin.clone()),
                token,
            )
            .await,
            marker,
        );
        assert_not_found_json(
            json_request(
                env.app.clone(),
                "POST",
                &format!(
                    "/api/v1/documents/{}/versions/{}/download",
                    doc.document_id, doc.version_id
                ),
                None,
                token,
            )
            .await,
            marker,
        );
    }

    env.drop().await;
}

#[tokio::test]
async fn live_lifecycle_delete_purge_and_revocation() {
    let _llm = LlmEnvGuard::unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed_org(&env).await;
    let owner = env.token(&seeded.owner_email).await;
    let viewer = env.token(&seeded.viewer_email).await;
    let marker = "E2E-LIFECYCLE-PURGE-2026";
    let Some(doc) = ingest_document(
        &env,
        &seeded,
        &owner,
        FixtureCase {
            name: "lifecycle",
            filename: "lifecycle.txt",
            content_type: "text/plain",
            title: "Lifecycle Purge",
            bytes: b"E2E-LIFECYCLE-PURGE-2026 should disappear after purge.\n",
            marker,
        },
    )
    .await
    else {
        env.drop().await;
        return;
    };
    assert_grounded_http_roundtrip(&env, &viewer, &doc).await;
    let pin = citation_pin_from(&first_citation(&env, &owner, &doc).await, marker);
    let stale_download_path = mint_download_path(&env, &owner, &doc).await;
    let proof_download_path = mint_download_path(&env, &owner, &doc).await;
    let proof_downloaded = send_request(
        env.app.clone(),
        "GET",
        &proof_download_path,
        Body::empty(),
        None,
        None,
    )
    .await;
    assert_eq!(proof_downloaded.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&proof_downloaded.bytes).contains(marker));

    let (status, deleted) = json_request(
        env.app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{}", doc.document_id),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{deleted}");
    assert_eq!(deleted["document"]["state"], "tombstoned");
    assert_deleted_egress_denied(&env, &owner, &doc, &pin, "after tombstone before purge").await;

    let ctx = seeded.worker_ctx();
    relay_outbox(&env, &ctx).await;
    assert!(matches!(
        run_delete_once(&env, &ctx).await,
        DeleteWorkerRun::Completed { .. }
    ));
    assert_eq!(chunk_count(&env, &ctx, doc.document_id).await, 0);
    assert_eq!(
        qdrant_points_for_doc(&env, &ctx, doc.collection_id, doc.document_id).await,
        0
    );
    assert_deleted_egress_denied(&env, &owner, &doc, &pin, "after purge").await;
    assert_not_found_raw(
        send_request(
            env.app.clone(),
            "GET",
            &stale_download_path,
            Body::empty(),
            None,
            None,
        )
        .await,
        marker,
    );

    let revoke_marker = "E2E-REVOKE-FRESH-ACL-2026";
    let Some(revoke_doc) = ingest_document(
        &env,
        &seeded,
        &owner,
        FixtureCase {
            name: "revocation",
            filename: "revocation.txt",
            content_type: "text/plain",
            title: "Revocation Fresh ACL",
            bytes: b"E2E-REVOKE-FRESH-ACL-2026 is visible until ACL revocation.\n",
            marker: revoke_marker,
        },
    )
    .await
    else {
        env.drop().await;
        return;
    };
    assert_grounded_http_roundtrip(&env, &viewer, &revoke_doc).await;
    revoke_collection_access(&env, &seeded, seeded.viewer_id).await;
    let (status, revoked_search) =
        search(&env, &viewer, revoke_marker, Some(revoke_doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{revoked_search}");
    assert_eq!(revoked_search["code"], "empty_scope");
    assert_value_lacks(&revoked_search, revoke_marker);
    let (status, revoked_ask) =
        ask(&env, &viewer, revoke_marker, Some(revoke_doc.collection_id)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{revoked_ask}");
    assert_eq!(revoked_ask["code"], "empty_scope");
    assert_value_lacks(&revoked_ask, revoke_marker);

    env.drop().await;
}

#[tokio::test]
async fn live_adversarial_malicious_input_rejected_or_contained() {
    let _llm = LlmEnvGuard::unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed_org(&env).await;
    let owner = env.token(&seeded.owner_email).await;
    let cross = env.token(&seeded.cross_email).await;
    let ctx = seeded.worker_ctx();
    let before = document_count(&env, &ctx).await;

    let too_many_parts = send_request(
        env.app.clone(),
        "POST",
        "/api/v1/uploads",
        Body::from(many_part_body(10)),
        Some(&owner),
        Some(format!("multipart/form-data; boundary={BOUNDARY}")),
    )
    .await;
    assert_eq!(too_many_parts.status, StatusCode::BAD_REQUEST);
    let too_many_body: Value =
        serde_json::from_slice(&too_many_parts.bytes).expect("structured multipart error");
    assert_eq!(too_many_body["code"], "multipart_invalid");
    assert_eq!(too_many_body["details"]["threatClass"], "multipart_invalid");
    assert_eq!(
        too_many_body["details"]["reasonCode"],
        "multipart_too_many_parts"
    );
    assert_body_lacks(&too_many_parts.bytes, "E2E-MALICIOUS");

    let (status, spoofed) = upload(
        &env,
        &owner,
        "spoofed.pdf",
        "application/pdf",
        b"E2E-MALICIOUS-SPOOFED-PDF is actually plain text.\n",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{spoofed}");
    assert_eq!(spoofed["code"], "upload_rejected");
    assert_value_lacks(&spoofed, "E2E-MALICIOUS-SPOOFED-PDF");
    assert!(spoofed.get("objectKey").is_none());

    let (status, cross_upload) = upload(
        &env,
        &cross,
        "cross.txt",
        "text/plain",
        b"cross tenant object",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{cross_upload}");
    let cross_key = cross_upload["objectKey"].as_str().unwrap();
    let (status, cross_create) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{}/documents", seeded.collection_id),
        Some(json!({ "objectKey": cross_key, "title": "cross key" })),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{cross_create}");

    let (status, traversal_create) = json_request(
        env.app.clone(),
        "POST",
        &format!("/api/v1/collections/{}/documents", seeded.collection_id),
        Some(json!({ "objectKey": "../quarantine/evil", "title": "traversal" })),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{traversal_create}");

    let marker = "E2E-CITATION-PIN-MISMATCH-2026";
    let Some(doc) = ingest_document(
        &env,
        &seeded,
        &owner,
        FixtureCase {
            name: "pin",
            filename: "pin.txt",
            content_type: "text/plain",
            title: "Citation Pin",
            bytes: b"E2E-CITATION-PIN-MISMATCH-2026 proves pin mismatch is non-disclosing.\n",
            marker,
        },
    )
    .await
    else {
        env.drop().await;
        return;
    };
    let mut pin = citation_pin_from(&first_citation(&env, &owner, &doc).await, marker);
    pin["contentSha256"] = Value::String("b".repeat(64));
    assert_not_found_json(
        json_request(
            env.app.clone(),
            "POST",
            &format!(
                "/api/v1/documents/{}/versions/{}/citations:resolve",
                doc.document_id, doc.version_id
            ),
            Some(pin),
            &owner,
        )
        .await,
        marker,
    );

    assert_eq!(document_count(&env, &ctx).await, before + 1);
    env.drop().await;
}

#[tokio::test]
async fn live_fault_injection_worker_kill_retry_consistency() {
    let _llm = LlmEnvGuard::unset_all();
    let Some(env) = LiveEnv::boot().await else {
        return;
    };
    let seeded = seed_org(&env).await;
    let owner = env.token(&seeded.owner_email).await;
    let marker = "E2E-FAULT-RETRY-CONSISTENT-2026";
    let (status, uploaded) = upload(
        &env,
        &owner,
        "fault.txt",
        "text/plain",
        b"E2E-FAULT-RETRY-CONSISTENT-2026 survives worker retry exactly once.\n",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{uploaded}");
    let (document_id, _source_version_id, job_id) = create_document(
        &env,
        &owner,
        seeded.collection_id,
        "Fault Retry",
        uploaded["objectKey"].as_str().unwrap(),
    )
    .await;

    let ctx = seeded.worker_ctx();
    let pause = ConvertWorkerPause {
        staged: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let Some(mut first_config) =
        real_convert_config(format!("e2e-convert-crash-{}", Uuid::new_v4()))
    else {
        env.drop().await;
        return;
    };
    first_config.lease_ttl = Duration::from_secs(1);
    first_config.heartbeat_interval = Duration::from_millis(100);
    first_config.promotion_fault = Some(PromotionFault::AfterStagingPut);
    first_config.pause_after_staging = Some(pause.clone());
    let first_worker =
        ConvertWorker::new(env.pool.clone(), env.storage.clone(), first_config).expect("worker");
    let first_ctx = ctx.clone();
    let first_handle = tokio::spawn(async move { first_worker.run_once(&first_ctx).await });

    if tokio::time::timeout(Duration::from_secs(5), pause.staged.notified())
        .await
        .is_err()
    {
        let first_result = first_handle.await.expect("first join");
        let (status, last_error) = job_status_and_error(&env, &ctx, job_id).await;
        panic!(
            "first worker did not reach staging: {first_result:?}; job_status={status}; last_error={last_error:?}"
        );
    }
    tokio::time::sleep(Duration::from_millis(1200)).await;
    expire_leases(&env, &ctx).await;
    make_job_available(&env, &ctx, job_id).await;

    let Some(retry_run) = run_convert_once(&env, &ctx).await else {
        pause.release.notify_waiters();
        let _ = first_handle.await;
        env.drop().await;
        return;
    };
    assert!(matches!(
        retry_run,
        ConvertWorkerRun::Completed { job_id: completed, .. } if completed == job_id
    ));
    pause.release.notify_waiters();
    let first_result = first_handle.await.expect("first join");
    assert!(
        matches!(
            first_result,
            Ok(ConvertWorkerRun::LeaseLost { job_id: lost }) if lost == job_id
        ) || matches!(
            first_result,
            Ok(ConvertWorkerRun::Failed {
                job_id: failed,
                terminal: false
            }) if failed == job_id
        ) || first_result.is_err(),
        "unexpected first worker result: {first_result:?}"
    );
    assert_eq!(
        get_job_status(&env, &ctx, job_id).await,
        JobStatus::Succeeded
    );

    relay_outbox(&env, &ctx).await;
    let index = run_index_once(&env, &ctx).await.expect("index run");
    assert!(matches!(index, IndexWorkerRun::Completed { chunks, .. } if chunks > 0));
    let version_id = current_version(&env, &ctx, document_id)
        .await
        .expect("current version");
    let doc = IngestedDoc {
        document_id,
        version_id,
        convert_job_id: job_id,
        collection_id: seeded.collection_id,
        marker: marker.to_string(),
        title: "Fault Retry".into(),
    };
    assert_grounded_http_roundtrip(&env, &owner, &doc).await;
    let chunks = chunk_count(&env, &ctx, document_id).await;
    assert!(chunks > 0);
    assert_chunk_point_identity(&env, &ctx, seeded.collection_id, document_id).await;

    env.drop().await;
}

async fn assert_grounded_http_roundtrip(env: &LiveEnv, token: &str, doc: &IngestedDoc) {
    let (status, search_response) = search(env, token, &doc.marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::OK, "{search_response}");
    let hits = search_response["hits"].as_array().expect("search hits");
    assert!(
        !hits.is_empty(),
        "search returned no hits for {}",
        doc.title
    );
    assert!(
        hits.iter().any(|hit| hit["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains(&doc.marker))),
        "search snippet did not include expected marker"
    );

    let (status, ask_response) = ask(env, token, &doc.marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::OK, "{ask_response}");
    assert!(ask_response["answer"]
        .as_str()
        .unwrap_or_default()
        .contains(&doc.marker));
    let citations = ask_response["citations"].as_array().expect("citations");
    let citation = citations
        .iter()
        .find(|citation| citation["documentId"] == doc.document_id.to_string())
        .expect("citation for ingested document");
    assert!(citation["snippet"]
        .as_str()
        .unwrap_or_default()
        .contains(&doc.marker));
    let pin = citation_pin_from(citation, &doc.marker);
    let (status, resolved) = json_request(
        env.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/citations:resolve",
            doc.document_id, doc.version_id
        ),
        Some(pin),
        token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert!(resolved["snippet"]
        .as_str()
        .unwrap_or_default()
        .contains(&doc.marker));

    let preview = send_request(
        env.app.clone(),
        "GET",
        &format!(
            "/api/v1/documents/{}/versions/{}/preview",
            doc.document_id, doc.version_id
        ),
        Body::empty(),
        Some(token),
        None,
    )
    .await;
    assert_eq!(preview.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&preview.bytes).contains(&doc.marker));

    let (status, capability) = json_request(
        env.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/download",
            doc.document_id, doc.version_id
        ),
        None,
        token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{capability}");
    let download_path = capability["downloadPath"].as_str().expect("download path");
    let downloaded = send_request(
        env.app.clone(),
        "GET",
        download_path,
        Body::empty(),
        None,
        None,
    )
    .await;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&downloaded.bytes).contains(&doc.marker));
}

async fn first_citation(env: &LiveEnv, token: &str, doc: &IngestedDoc) -> Value {
    let (status, ask_response) = ask(env, token, &doc.marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::OK, "{ask_response}");
    ask_response["citations"]
        .as_array()
        .expect("citations")
        .first()
        .expect("first citation")
        .clone()
}

async fn search(
    env: &LiveEnv,
    token: &str,
    query: &str,
    collection_id: Option<Uuid>,
) -> (StatusCode, Value) {
    let mut body = json!({ "query": query, "limit": 5 });
    if let Some(collection_id) = collection_id {
        body["collectionIds"] = json!([collection_id]);
    }
    json_request(env.app.clone(), "POST", "/api/v1/search", Some(body), token).await
}

async fn ask(
    env: &LiveEnv,
    token: &str,
    question: &str,
    collection_id: Option<Uuid>,
) -> (StatusCode, Value) {
    let mut body = json!({ "question": question, "limit": 5 });
    if let Some(collection_id) = collection_id {
        body["collectionIds"] = json!([collection_id]);
    }
    json_request(env.app.clone(), "POST", "/api/v1/ask", Some(body), token).await
}

async fn mint_download_path(env: &LiveEnv, token: &str, doc: &IngestedDoc) -> String {
    let (status, capability) = json_request(
        env.app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{}/versions/{}/download",
            doc.document_id, doc.version_id
        ),
        None,
        token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{capability}");
    capability["downloadPath"]
        .as_str()
        .expect("download path")
        .to_string()
}

async fn assert_deleted_egress_denied(
    env: &LiveEnv,
    token: &str,
    doc: &IngestedDoc,
    pin: &Value,
    phase: &str,
) {
    let (status, search_after) = search(env, token, &doc.marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::OK, "{phase}: {search_after}");
    assert!(
        search_after["hits"].as_array().unwrap().is_empty(),
        "{phase}: search still returned deleted document"
    );
    assert_value_lacks(&search_after, &doc.marker);

    let (status, ask_after) = ask(env, token, &doc.marker, Some(doc.collection_id)).await;
    assert_eq!(status, StatusCode::OK, "{phase}: {ask_after}");
    assert_value_lacks(&ask_after, &doc.marker);
    assert!(
        ask_after["citations"].as_array().unwrap().is_empty(),
        "{phase}: ask still cited deleted document"
    );

    assert_not_found_raw(
        send_request(
            env.app.clone(),
            "GET",
            &format!(
                "/api/v1/documents/{}/versions/{}/preview",
                doc.document_id, doc.version_id
            ),
            Body::empty(),
            Some(token),
            None,
        )
        .await,
        &doc.marker,
    );
    assert_not_found_json(
        json_request(
            env.app.clone(),
            "POST",
            &format!(
                "/api/v1/documents/{}/versions/{}/citations:resolve",
                doc.document_id, doc.version_id
            ),
            Some(pin.clone()),
            token,
        )
        .await,
        &doc.marker,
    );
    assert_not_found_json(
        json_request(
            env.app.clone(),
            "POST",
            &format!(
                "/api/v1/documents/{}/versions/{}/download",
                doc.document_id, doc.version_id
            ),
            None,
            token,
        )
        .await,
        &doc.marker,
    );
}

fn assert_not_found_json((status, body): (StatusCode, Value), marker: &str) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    assert_value_lacks(&body, marker);
}

fn assert_not_found_raw(response: HttpResponse, marker: &str) {
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_body_lacks(&response.bytes, marker);
}
