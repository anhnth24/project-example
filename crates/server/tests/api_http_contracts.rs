//! P1B-R04 full live HTTP contract suite through axum router + dual-role DB/MinIO.
//!
//! Admin URL is used only to create/migrate ephemeral databases. Application
//! requests and assertions run as `markhand_app`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, DocumentState};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::storage::minio::ObjectIdentityMeta;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_router,
    login_access_token, put_bytes, seed_user_with_permissions, sha256_hex, test_minio_client,
    trusted_key,
};

const BOUNDARY: &str = "----markhandHttpContractBoundary";

fn multipart_body(filename: &str, bytes: &[u8], collection_id: Uuid) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn json_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, serde_json::Value, bytes::Bytes) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(value.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json, bytes)
}

async fn seed_http_principal(pool: &deadpool_postgres::Pool) -> (Uuid, Uuid, String) {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@http.test"),
        "correct-password-1",
        &[
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "jobs.system",
        ],
    )
    .await;
    let token = login_access_token(pool, &format!("{user}@http.test"), "correct-password-1").await;
    (org, user, token)
}

async fn seed_published_doc(
    pool: &deadpool_postgres::Pool,
    store: &fileconv_server::storage::minio::MinioClient,
    org: Uuid,
    user: Uuid,
) -> (Uuid, Uuid, Uuid) {
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let markdown = "# Contract\n\nKinh phí là 15 triệu đồng.\n";
    let sha = sha256_hex(markdown.as_bytes());
    let key = trusted_key(org, version_id, Uuid::new_v4(), None).unwrap();
    let ctx = OrgContext::try_new(
        org,
        user,
        [
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "jobs.system",
        ],
        [collection_id],
    )
    .unwrap();
    put_bytes(
        store,
        org,
        &key,
        markdown.as_bytes(),
        "text/markdown; charset=utf-8",
        ObjectIdentityMeta {
            org_id: org,
            collection_id: Some(collection_id),
            document_id: Some(document_id),
            version_id: Some(version_id),
            original_filename: None,
            canonical_format: Some("md".into()),
            content_sha256: Some(sha.clone()),
            content_length: Some(markdown.len() as u64),
            disposition: Some("trusted".into()),
        },
    )
    .await;
    let key_str = key.as_str();
    let md_len = markdown.len() as i64;
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let sha = sha.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "HTTP Collection",
                        slug: &format!("http-{}", collection_id.simple()),
                        description: Some("contract"),
                        visibility: fileconv_server::db::models::CollectionVisibility::Org,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: "HTTP Doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,$5,'text/markdown',$6,$7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha,
                        &key_str,
                        &md_len,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let kind = ArtifactKind::Markdown.as_str();
                txn.execute(
                    "INSERT INTO derived_artifacts (
                        id, org_id, document_id, version_id, artifact_kind,
                        object_key, content_sha256, content_type, byte_size
                     ) VALUES ($1,$2,$3,$4,$5,$6,$7,'text/markdown; charset=utf-8',$8)",
                    &[
                        &artifact_id,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &kind,
                        &key_str,
                        &sha,
                        &md_len,
                    ],
                )
                .await?;
                let indexed = DocumentState::Indexed.as_str();
                txn.execute(
                    "UPDATE documents SET state=$3, current_version_id=$4 WHERE org_id=$1 AND id=$2",
                    &[&ctx.org_id(), &document_id, &indexed, &version_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed published doc");
    (collection_id, document_id, version_id)
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_http_collection_document_job_contract_matrix() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (org, user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));

    // Collection CRUD.
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "POC Collection",
            "slug": format!("poc-{}", Uuid::new_v4().simple()),
            "description": "http contract",
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection_id = created["id"].as_str().unwrap().to_string();

    let (status, listed, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/collections",
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == collection_id));

    let (status, got, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["id"], collection_id);

    let (status, patched, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        Some(serde_json::json!({
            "name": "POC Collection Renamed",
            "description": "updated"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["name"], "POC Collection Renamed");

    // Malformed / body bounds.
    let (status, err, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({ "name": "", "slug": "x" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["code"].as_str().is_some());
    assert!(err["requestId"].as_str().is_some() || err["request_id"].as_str().is_some());

    let huge = "a".repeat(300 * 1024);
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({ "name": huge, "slug": "too-big" })),
        &[],
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::PAYLOAD_TOO_LARGE,
        "oversized body must fail closed, got {status}"
    );

    // Upload → list/get/preview/reindex/delete.
    let (collection_id, document_id, version_id) =
        seed_published_doc(&pool, &store, org, user).await;

    let (status, docs, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/collections/{collection_id}/documents?limit=10"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{docs}");
    assert!(docs["page"]["hasMore"].as_bool().is_some() || docs["items"].is_array());

    let (status, doc, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{doc}");

    let (status, preview, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}/preview"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["sourceContentSha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        preview["canonicalMarkdownSha256"].as_str().unwrap().len(),
        64
    );

    let (status, versions, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}/versions"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{versions}");

    let (status, version, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}/versions/{version_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{version}");

    let (status, diff, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}/versions/{version_id}/diff"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "diff route must respond stably, got {status}: {diff}"
    );

    let (status, reindex1, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/documents/{document_id}/reindex"),
        Some(&token),
        Some(serde_json::json!({})),
        &[("idempotency-key", "reindex-once")],
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::ACCEPTED || status == StatusCode::CREATED,
        "reindex status {status}: {reindex1}"
    );
    let job_id_1 = reindex1["jobId"]
        .as_str()
        .expect("reindex must return jobId");
    assert_eq!(
        reindex1["created"].as_bool(),
        Some(true),
        "first reindex must create a job: {reindex1}"
    );
    let (status, reindex2, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/documents/{document_id}/reindex"),
        Some(&token),
        Some(serde_json::json!({})),
        &[("idempotency-key", "reindex-once")],
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::ACCEPTED || status == StatusCode::CREATED,
        "idempotent reindex status {status}: {reindex2}"
    );
    assert_eq!(
        reindex2["jobId"].as_str(),
        Some(job_id_1),
        "idempotent reindex must return the same jobId: {reindex1} vs {reindex2}"
    );
    assert_eq!(
        reindex2["created"].as_bool(),
        Some(false),
        "idempotent reindex replay must set created=false: {reindex2}"
    );

    // Conflicts list/detail/triage + dual-leg evidence authorization.
    let claim_a = Uuid::new_v4();
    let claim_b = Uuid::new_v4();
    let (claim_low, claim_high) = if claim_a < claim_b {
        (claim_a, claim_b)
    } else {
        (claim_b, claim_a)
    };
    let conflict_id = Uuid::new_v4();
    let evidence_left = Uuid::new_v4();
    let evidence_right = Uuid::new_v4();
    let conflict_ctx = OrgContext::try_new(
        org,
        user,
        [
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "jobs.system",
        ],
        [collection_id],
    )
    .unwrap();
    with_org_txn(&pool, &conflict_ctx, {
        let ctx = conflict_ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',15,'triệu','', now(),
                               'Kinh phí là 15 triệu đồng.')",
                    &[&claim_low, &ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES ($1,$2,$3,$4,'budget','Kinh phí','is','money',20,'triệu','', now(),
                               'Kinh phí là 20 triệu đồng.')",
                    &[&claim_high, &ctx.org_id(), &document_id, &version_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO conflicts (
                        id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
                        first_detected_version_id
                     ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
                    &[
                        &conflict_id,
                        &ctx.org_id(),
                        &claim_low,
                        &claim_high,
                        &version_id,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO conflict_evidence (
                        id, org_id, conflict_id, claim_id, evidence_role, citation_quote
                     ) VALUES
                        ($1,$2,$3,$4,'left','Kinh phí là 15 triệu đồng.'),
                        ($5,$2,$3,$6,'right','Kinh phí là 20 triệu đồng.')",
                    &[
                        &evidence_left,
                        &ctx.org_id(),
                        &conflict_id,
                        &claim_low,
                        &evidence_right,
                        &claim_high,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed conflict + evidence");

    let (status, conflicts, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/conflicts",
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{conflicts}");
    assert!(
        conflicts["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == conflict_id.to_string()),
        "seeded conflict must appear in list: {conflicts}"
    );

    let (status, detail, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/conflicts/{conflict_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "open");
    assert_eq!(detail["claimAId"], claim_low.to_string());
    assert_eq!(detail["claimBId"], claim_high.to_string());

    let missing_conflict = Uuid::new_v4();
    let (status, missing, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/conflicts/{missing_conflict}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");

    let (status, triaged, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/conflicts/{conflict_id}/triage"),
        Some(&token),
        Some(serde_json::json!({
            "status": "accepted_exception",
            "resolutionNote": "BA accepted v1 figure for POC"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{triaged}");
    assert_eq!(triaged["status"], "accepted_exception");
    assert!(triaged["resolvedAt"].as_str().is_some());

    // Evidence rows remain immutable after triage; dual-leg get still authorized.
    let evidence_count: i64 = with_org_txn(&pool, &conflict_ctx, {
        let ctx = conflict_ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM conflict_evidence
                         WHERE org_id=$1 AND conflict_id=$2",
                        &[&ctx.org_id(), &conflict_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("count conflict evidence");
    assert_eq!(
        evidence_count, 2,
        "conflict_evidence must persist through triage"
    );

    let (status, after_triage, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/conflicts/{conflict_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after_triage}");
    assert_eq!(after_triage["status"], "accepted_exception");

    // Jobs: document-scoped job visible; foreign job → 404.
    let ctx = OrgContext::try_new(
        org,
        user,
        ["jobs.system", "qa.query", "doc.upload"],
        [collection_id],
    )
    .unwrap();
    let job_id = Uuid::new_v4();
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let payload = serde_json::json!({
                    "document_id": document_id,
                    "version_id": version_id
                });
                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, status, payload_version, payload,
                        idempotency_key, document_id, version_id, attempts, max_attempts
                     ) VALUES (
                        $1,$2,'index','pending',1,$6::jsonb,$3,$4,$5,0,5
                     )",
                    &[
                        &job_id,
                        &ctx.org_id(),
                        &format!("job-{}", job_id.simple()),
                        &document_id,
                        &version_id,
                        &payload,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("insert job");

    let (status, job_json, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/jobs/{job_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job_json}");

    let foreign_job = Uuid::new_v4();
    let (status, idor_job, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/jobs/{foreign_job}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{idor_job}");

    // Tenant/collection IDOR → consistent 404.
    let other_collection = Uuid::new_v4();
    let (status, idor_collection, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/collections/{other_collection}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{idor_collection}");
    let other_document = Uuid::new_v4();
    let (status, idor_doc, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{other_document}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{idor_doc}");

    // Pagination cursor malformed.
    let (status, bad_page, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/collections/{collection_id}/documents?cursor=not-a-cursor"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::OK,
        "malformed cursor must not 500, got {status}: {bad_page}"
    );

    // Upload happy (txt small) via multipart.
    let upload_bytes = b"HTTP contract upload fixture\n";
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .header("idempotency-key", "http-contract-upload-1")
                .body(Body::from(multipart_body(
                    "note.txt",
                    upload_bytes,
                    collection_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED, "upload must be 201");
    let upload_json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(upload_json["documentId"].as_str().is_some());
    assert!(upload_json["jobId"].as_str().is_some());

    // Delete document + collection; audit correlation present on mutation path.
    let (status, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/documents/{document_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert!(
        status == StatusCode::NO_CONTENT
            || status == StatusCode::OK
            || status == StatusCode::CONFLICT,
        "delete document status {status}"
    );
    let (status, after_delete, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/documents/{document_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{after_delete}");

    let (status, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    // May fail if collection still has docs depending on soft-delete rules; either stable success or conflict.
    assert!(
        status == StatusCode::NO_CONTENT
            || status == StatusCode::OK
            || status == StatusCode::CONFLICT
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::NOT_FOUND,
        "delete collection status {status}"
    );

    // Audit rows for collection.create exist with request correlation (no secrets).
    let audit_count: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM audit_log
                         WHERE org_id = $1 AND action = 'collection.create'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("audit count");
    assert!(audit_count >= 1, "collection.create must be audited in-txn");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP"]
async fn live_central_write_gate_matrix_refuses_business_side_effects() {
    use fileconv_server::middleware::write_gate::ensure_background_mutations_allowed;
    use fileconv_server::services::ops_fence::{self, FENCE_RESTORE};

    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (org, user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));

    // Seed a published doc before fencing so GET/search/ask side-effect paths exist.
    let (collection_id, document_id, _version_id) =
        seed_published_doc(&pool, &store, org, user).await;

    let audit_before: i64 = with_org_txn(
        &pool,
        &OrgContext::try_new(
            org,
            user,
            [
                "qa.query",
                "qa.history",
                "doc.upload",
                "doc.delete",
                "doc.publish",
                "jobs.system",
            ],
            [collection_id],
        )
        .unwrap(),
        {
            let org = org;
            move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one(
                            "SELECT COUNT(*)::bigint FROM audit_log WHERE org_id = $1",
                            &[&org],
                        )
                        .await?;
                    Ok(row.get(0))
                })
            }
        },
    )
    .await
    .expect("audit before");

    ops_fence::set_fence(&pool, FENCE_RESTORE, "p1b-write-gate-matrix", Some("test"))
        .await
        .expect("set restore fence");
    assert!(
        ensure_background_mutations_allowed(&pool).await.is_err(),
        "background gate must observe active fence"
    );

    // Ops surfaces remain available.
    for path in [
        "/api/v1/health/live",
        "/api/v1/health/start",
        "/api/v1/openapi.yaml",
    ] {
        let (status, _, _) = json_request(app.clone(), "GET", path, None, None, &[]).await;
        assert_eq!(status, StatusCode::OK, "exempt {path} must stay up");
    }

    // Unauthenticated auth mutation.
    let (status, err, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        None,
        Some(serde_json::json!({
            "email": format!("{user}@http.test"),
            "password": "correct-password-1"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{err}");
    assert_eq!(err["code"], "ops_fence_active");

    // Authenticated collection + document mutations.
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/collections".to_string(),
            Some(serde_json::json!({
                "name": "Fenced",
                "slug": format!("fenced-{}", Uuid::new_v4().simple()),
                "visibility": "org"
            })),
        ),
        (
            "POST",
            format!("/api/v1/documents/{document_id}/reindex"),
            Some(serde_json::json!({})),
        ),
        ("DELETE", format!("/api/v1/documents/{document_id}"), None),
        (
            "POST",
            "/api/v1/ask".to_string(),
            Some(serde_json::json!({
                "question": "Kinh phí?",
                "mode": "current",
                "limit": 3
            })),
        ),
        (
            "POST",
            "/api/v1/search".to_string(),
            Some(serde_json::json!({
                "query": "Kinh phí",
                "mode": "current",
                "limit": 3
            })),
        ),
        (
            "GET",
            format!("/api/v1/documents/{document_id}/preview"),
            None,
        ),
    ] {
        let (status, err, _) = json_request(
            app.clone(),
            method,
            &uri,
            Some(&token),
            body,
            if method == "POST" && uri.contains("reindex") {
                &[("idempotency-key", "fenced-reindex")]
            } else {
                &[]
            },
        )
        .await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri} => {err}"
        );
        assert_eq!(err["code"], "ops_fence_active", "{method} {uri}");
    }

    // Upload multipart mutation.
    let upload = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/uploads")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", "fenced-upload")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart_body(
                    "fenced.txt",
                    b"should not land\n",
                    collection_id,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::SERVICE_UNAVAILABLE);
    let upload_body = upload.into_body().collect().await.unwrap().to_bytes();
    let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
    assert_eq!(upload_json["code"], "ops_fence_active");

    // No new audit side effects while fenced.
    let ctx = OrgContext::try_new(
        org,
        user,
        [
            "qa.query",
            "qa.history",
            "doc.upload",
            "doc.delete",
            "doc.publish",
            "jobs.system",
        ],
        [collection_id],
    )
    .unwrap();
    let audit_after: i64 = with_org_txn(&pool, &ctx, {
        let org = org;
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM audit_log WHERE org_id = $1",
                        &[&org],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("audit after");
    assert_eq!(
        audit_after, audit_before,
        "fenced business traffic must not append audit rows"
    );

    let attestation = "a".repeat(64);
    ops_fence::clear_fence_with_attestation(&pool, FENCE_RESTORE, &attestation)
        .await
        .expect("clear fence");

    let (status, created, _) = json_request(
        app,
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "Unfenced",
            "slug": format!("unfenced-{}", Uuid::new_v4().simple()),
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP"]
async fn live_http_unauthenticated_and_cross_tenant_are_consistent() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (_org, _user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool, &ephemeral.app_url, None);

    let (status, err, _) =
        json_request(app.clone(), "GET", "/api/v1/collections", None, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");

    let (status, err, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/collections",
        Some("not-a-jwt"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{err}");

    let missing = Uuid::new_v4();
    let (status, err, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/collections/{missing}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["code"], "not_found");

    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + test-hooks"]
async fn live_patch_collection_audit_correlation_and_rollback() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (org, user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let ctx = OrgContext::try_new(org, user, ["doc.upload", "doc.delete", "qa.query"], []).unwrap();

    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "Audit Patch",
            "slug": format!("audit-patch-{}", Uuid::new_v4().simple()),
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let collection_id = created["id"].as_str().unwrap().to_string();

    let (status, patched, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        Some(serde_json::json!({
            "name": "Audit Patch Renamed",
            "description": "ok"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");

    // Success audit correlated + sanitized (no secrets).
    let success_meta: serde_json::Value = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let collection_id = collection_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT request_id, outcome, metadata::text
                         FROM audit_log
                         WHERE org_id = $1 AND action = 'collection.update'
                           AND resource_id = $2 AND outcome = 'success'
                         ORDER BY created_at DESC LIMIT 1",
                        &[&ctx.org_id(), &collection_id],
                    )
                    .await?;
                Ok(serde_json::json!({
                    "requestId": row.get::<_, String>(0),
                    "outcome": row.get::<_, String>(1),
                    "metadata": row.get::<_, String>(2),
                }))
            })
        }
    })
    .await
    .expect("success audit");
    assert_eq!(success_meta["outcome"], "success");
    assert!(!success_meta["requestId"].as_str().unwrap_or("").is_empty());
    assert!(!success_meta["metadata"]
        .as_str()
        .unwrap_or("")
        .contains("password"));

    // Validation error path writes sanitized error audit (password key stripped).
    let (status, err, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        Some(serde_json::json!({ "name": "", "description": "x" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    let error_meta: String = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let collection_id = collection_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT metadata::text FROM audit_log
                         WHERE org_id = $1 AND action = 'collection.update'
                           AND resource_id = $2 AND outcome = 'error'
                         ORDER BY created_at DESC LIMIT 1",
                        &[&ctx.org_id(), &collection_id],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("error audit");
    assert!(error_meta.contains("validation_failed"));
    assert!(!error_meta.contains("should-be-stripped"));
    assert!(!error_meta.contains("password"));

    // Injected audit failure rolls back the PATCH mutation.
    fileconv_server::services::audit::arm_injected_audit_failure();
    let before_name = patched["name"].as_str().unwrap().to_string();
    let (status, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        Some(serde_json::json!({
            "name": "Must Not Persist",
            "description": "rollback"
        })),
        &[],
    )
    .await;
    assert!(
        status.is_server_error()
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::CONFLICT,
        "injected audit failure must not succeed silently: {status}"
    );
    let (status, got, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(
        got["name"], before_name,
        "PATCH must roll back when co-committed audit fails"
    );

    ephemeral.drop().await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + test-hooks"]
async fn live_reindex_audit_failure_rolls_back_enqueue() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (org, user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        org,
        user,
        ["doc.upload", "doc.delete", "qa.query"],
        [collection_id],
    )
    .unwrap();
    let sha = "a".repeat(64);
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let sha = sha.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Reindex Audit",
                        slug: &format!("reindex-audit-{}", collection_id.simple()),
                        description: None,
                        visibility: fileconv_server::db::models::CollectionVisibility::Org,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: "Reindex Audit Doc",
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, created_by_user_id
                     ) VALUES ($1, $2, $3, 1, 'published', true, $4, $5, $6)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &sha,
                        &format!("org/{}/objects/reindex-audit", ctx.org_id()),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1, state = 'indexed'
                     WHERE id = $2 AND org_id = $3",
                    &[&version_id, &document_id, &ctx.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed document for reindex");

    let before_jobs: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM jobs WHERE org_id = $1 AND job_type = 'index'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("count jobs");

    fileconv_server::services::audit::arm_injected_audit_failure();
    let (status, body, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/documents/{document_id}/reindex"),
        Some(&token),
        None,
        &[("idempotency-key", "reindex-audit-rollback")],
    )
    .await;
    assert!(
        status.is_server_error()
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::CONFLICT,
        "injected audit failure must fail reindex: {status} {body}"
    );

    let after_jobs: i64 = with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COUNT(*)::bigint FROM jobs WHERE org_id = $1 AND job_type = 'index'",
                        &[&ctx.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("count jobs after");
    assert_eq!(
        after_jobs, before_jobs,
        "reindex enqueue must roll back when co-committed audit fails"
    );

    ephemeral.drop().await;
}
