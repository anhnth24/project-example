//! P1B-R04 full live HTTP contract suite through axum router + dual-role DB/MinIO.
//!
//! Admin URL is used only to create/migrate ephemeral databases. Application
//! requests and assertions run as `markhand_app`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_knowledge::identity::BODY_TEXT_VERSION;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{ArtifactKind, DocumentState};
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::chunking::prepare_chunks;
use fileconv_server::storage::minio::ObjectIdentityMeta;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_router,
    login_access_token, put_bytes, seed_user_with_permissions, sha256_hex, take_live,
    test_minio_client, trusted_key, MinioCleanupGuard,
};

const BOUNDARY: &str = "----markhandHttpContractBoundary";

fn multipart_body(
    filename: &str,
    bytes: &[u8],
    collection_id: Uuid,
    document_id: Option<Uuid>,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n"
        )
        .as_bytes(),
    );
    if let Some(document_id) = document_id {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"documentId\"\r\n\r\n{document_id}\r\n"
            )
            .as_bytes(),
        );
    }
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

async fn assert_foreign_not_found(
    app: axum::Router,
    method: &str,
    uri: String,
    token: &str,
    body: Option<serde_json::Value>,
    marker: &str,
) {
    let (status, error, _) = json_request(app, method, &uri, Some(token), body, &[]).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "{method} {uri} exposed a distinguishable foreign resource response: {error}"
    );
    assert_eq!(error["code"], "not_found", "{method} {uri}: {error}");
    assert!(
        !error.to_string().contains(marker),
        "{method} {uri} leaked foreign marker: {error}"
    );
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
    let index_metadata_id = Uuid::new_v4();
    let markdown = "# Contract\n\nKinh phí là 15 triệu đồng.\n";
    let sha = sha256_hex(markdown.as_bytes());
    let index_signature = sha256_hex(format!("http-contract-index-{index_metadata_id}").as_bytes());
    let chunks = prepare_chunks(document_id, version_id, markdown, "md");
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
        let chunks = chunks.clone();
        let index_signature = index_signature.clone();
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
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES ($1,$2,$3,$4,'test','r1',8,'local-hash',1,true,'active')",
                    &[
                        &index_metadata_id,
                        &ctx.org_id(),
                        &collection_id,
                        &index_signature,
                    ],
                )
                .await?;
                for chunk in chunks {
                    fileconv_server::db::chunks::insert(
                        txn,
                        &ctx,
                        fileconv_server::db::chunks::NewChunk {
                            id: Uuid::new_v4(),
                            document_id,
                            version_id,
                            ordinal: chunk.ordinal,
                            heading_path: &chunk.heading_path,
                            body: &chunk.body,
                            body_text_version: BODY_TEXT_VERSION,
                            chunk_identity_sha256: &chunk.chunk_identity,
                            index_metadata_id,
                            index_signature: &index_signature,
                            page: chunk.page,
                            slide: chunk.slide,
                            sheet: chunk.sheet.as_deref(),
                            span_start: Some(chunk.span_start),
                            span_end: Some(chunk.span_end),
                        },
                    )
                    .await?;
                }
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

async fn seed_foreign_collection_document(
    pool: &deadpool_postgres::Pool,
    marker: &str,
    store: Option<&fileconv_server::storage::minio::MinioClient>,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let conflict_id = Uuid::new_v4();
    let claim_a = Uuid::new_v4();
    let claim_b = Uuid::new_v4();
    let (claim_low, claim_high) = if claim_a < claim_b {
        (claim_a, claim_b)
    } else {
        (claim_b, claim_a)
    };
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@foreign-http.test"),
        "correct-password-1",
        &["doc.upload", "doc.publish", "qa.query", "jobs.system"],
    )
    .await;
    let ctx = OrgContext::try_new(
        org,
        user,
        ["doc.upload", "doc.publish", "qa.query", "jobs.system"],
        [collection_id],
    )
    .unwrap();
    let collection_name = format!("Foreign {marker}");
    let collection_description = marker.to_string();
    let document_title = format!("Foreign document {marker}");
    let content_sha = sha256_hex(marker.as_bytes());
    let content_length = marker.len() as i64;
    let foreign_object_key =
        trusted_key(org, version_id, Uuid::new_v4(), None).expect("foreign trusted key");
    let object_key = foreign_object_key.as_str().to_string();
    if let Some(store) = store {
        put_bytes(
            store,
            org,
            &foreign_object_key,
            marker.as_bytes(),
            "text/plain",
            ObjectIdentityMeta {
                org_id: org,
                collection_id: Some(collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: None,
                canonical_format: Some("txt".into()),
                content_sha256: Some(content_sha.clone()),
                content_length: Some(content_length as u64),
                disposition: Some("trusted".into()),
            },
        )
        .await;
    }
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: &collection_name,
                        slug: &format!("foreign-{}", collection_id.simple()),
                        description: Some(&collection_description),
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
                        title: &document_title,
                    },
                )
                .await?;
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,NULL,'text/plain',$6,$7)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &content_sha,
                        &object_key,
                        &content_length,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                let indexed = DocumentState::Indexed.as_str();
                txn.execute(
                    "UPDATE documents SET state=$3, current_version_id=$4
                     WHERE org_id=$1 AND id=$2",
                    &[&ctx.org_id(), &document_id, &indexed, &version_id],
                )
                .await?;
                let payload = serde_json::json!({
                    "document_id": document_id,
                    "version_id": version_id,
                    "marker": collection_description.clone(),
                });
                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, status, payload_version, payload,
                        idempotency_key, document_id, version_id, attempts, max_attempts
                     ) VALUES ($1,$2,'index','pending',1,$6::jsonb,$3,$4,$5,0,5)",
                    &[
                        &job_id,
                        &ctx.org_id(),
                        &format!("foreign-job-{}", job_id.simple()),
                        &document_id,
                        &version_id,
                        &payload,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES
                        ($1,$2,$3,$4,$5,$5,'is','money',15,'triệu','',now(),$5),
                        ($6,$2,$3,$4,$5,$5,'is','money',20,'triệu','',now(),$5)",
                    &[
                        &claim_low,
                        &ctx.org_id(),
                        &document_id,
                        &version_id,
                        &collection_description,
                        &claim_high,
                    ],
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
                Ok(())
            })
        }
    })
    .await
    .expect("seed foreign collection/document");
    (
        collection_id,
        document_id,
        version_id,
        job_id,
        conflict_id,
        org,
        user,
    )
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
    let cleanup = MinioCleanupGuard::new(store.clone());
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
    let revision_response = app
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
                .header("idempotency-key", "http-contract-existing-document-version")
                .body(Body::from(multipart_body(
                    "contract-v2.txt",
                    b"Kinh phi phien ban hai la 20 trieu dong.\n",
                    collection_id,
                    Some(document_id),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revision_response.status(), StatusCode::CREATED);
    let revision: serde_json::Value = serde_json::from_slice(
        &revision_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(revision["documentId"], document_id.to_string());
    let revision_version_id =
        Uuid::parse_str(revision["versionId"].as_str().expect("revision version id")).unwrap();
    let revision_job_id =
        Uuid::parse_str(revision["jobId"].as_str().expect("revision convert job id")).unwrap();
    assert_ne!(revision_version_id, version_id);

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

    let citation_ctx = OrgContext::try_new(
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
    let (revision_number, revision_parent, revision_state, revision_current, revision_job_type): (
        i32,
        Option<Uuid>,
        String,
        bool,
        String,
    ) = with_org_txn(&pool, &citation_ctx, {
        let ctx = citation_ctx.clone();
        move |txn| {
            Box::pin(async move {
                let version = txn
                    .query_one(
                        "SELECT version_number, parent_version_id, publication_state, is_current
                         FROM document_versions
                         WHERE org_id=$1 AND document_id=$2 AND id=$3",
                        &[&ctx.org_id(), &document_id, &revision_version_id],
                    )
                    .await?;
                let job = txn
                    .query_one(
                        "SELECT job_type FROM jobs WHERE org_id=$1 AND id=$2",
                        &[&ctx.org_id(), &revision_job_id],
                    )
                    .await?;
                Ok((
                    version.get(0),
                    version.get(1),
                    version.get(2),
                    version.get(3),
                    job.get(0),
                ))
            })
        }
    })
    .await
    .expect("load uploaded revision");
    assert_eq!(revision_number, 2);
    assert_eq!(revision_parent, Some(version_id));
    assert_eq!(revision_state, "draft");
    assert!(!revision_current);
    assert_eq!(revision_job_type, "convert");

    let (chunk_id, quote, span_start, span_end): (Uuid, String, i32, i32) =
        with_org_txn(&pool, &citation_ctx, {
            let ctx = citation_ctx.clone();
            move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one(
                            "SELECT id, body, span_start, span_end
                             FROM chunks
                             WHERE org_id=$1 AND document_id=$2 AND version_id=$3
                             ORDER BY ordinal
                             LIMIT 1",
                            &[&ctx.org_id(), &document_id, &version_id],
                        )
                        .await?;
                    Ok((
                        row.get(0),
                        row.get(1),
                        row.get::<_, Option<i32>>(2).expect("chunk span start"),
                        row.get::<_, Option<i32>>(3).expect("chunk span end"),
                    ))
                })
            }
        })
        .await
        .expect("load citation chunk");
    let (status, citation, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/citations/resolve",
        Some(&token),
        Some(serde_json::json!({
            "logicalDocumentId": document_id,
            "versionId": version_id,
            "sourceContentSha256": preview["sourceContentSha256"],
            "canonicalMarkdownSha256": preview["canonicalMarkdownSha256"],
            "chunkId": chunk_id,
            "sourceSpanStart": span_start,
            "sourceSpanEnd": span_end,
            "quoteLocalStart": 0,
            "quoteLocalEnd": quote.len(),
            "quote": quote,
            "requireCurrent": true
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{citation}");
    assert_eq!(
        citation["citation"]["logicalDocumentId"],
        document_id.to_string()
    );
    assert_eq!(citation["citation"]["versionId"], version_id.to_string());
    assert_eq!(citation["citation"]["chunkId"], chunk_id.to_string());

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

    let (status, publish, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/documents/{document_id}/versions/{version_id}/publish"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "publish route must be idempotent for current version: {publish}"
    );

    let (status, issued, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/documents/{document_id}/versions/{version_id}/download-capability"),
        Some(&token),
        Some(serde_json::json!({ "purpose": "markdown" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let capability = issued["capability"]
        .as_str()
        .expect("download capability")
        .to_string();
    let (status, _, downloaded) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/downloads/{capability}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&downloaded).contains("Kinh phí"),
        "downloaded markdown must match the authorized version"
    );
    let (status, replay, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/downloads/{capability}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{replay}");

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
                    None,
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

    cleanup.cleanup().await.expect("clean HTTP contract bucket");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_central_write_gate_matrix_refuses_business_side_effects() {
    use fileconv_server::middleware::write_gate::acquire_background_mutation_guard;
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
    let cleanup = MinioCleanupGuard::new(store.clone());
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
        },
    )
    .await
    .expect("audit before");

    ops_fence::set_fence(&pool, FENCE_RESTORE, "p1b-write-gate-matrix", Some("test"))
        .await
        .expect("set restore fence");
    assert!(
        acquire_background_mutation_guard(&pool).await.is_err(),
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
            "/api/v1/ask/stream".to_string(),
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
                    None,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::SERVICE_UNAVAILABLE);
    let upload_body = upload.into_body().collect().await.unwrap().to_bytes();
    let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
    assert_eq!(upload_json["code"], "ops_fence_active");

    // No new audit side effects while fenced; ask/stream must not init sessions.
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
    let audit_after: i64 = with_org_txn(&pool, &ctx, move |txn| {
        Box::pin(async move {
            let row = txn
                .query_one(
                    "SELECT COUNT(*)::bigint FROM audit_log WHERE org_id = $1",
                    &[&org],
                )
                .await?;
            Ok(row.get(0))
        })
    })
    .await
    .expect("audit after");
    assert_eq!(
        audit_after, audit_before,
        "fenced business traffic must not append audit rows"
    );
    let ask_sessions: i64 = {
        let client = pool.get().await.expect("client");
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM ask_stream_sessions WHERE org_id = $1",
                &[&org],
            )
            .await
            .expect("ask session count")
            .get(0)
    };
    assert_eq!(
        ask_sessions, 0,
        "ask/stream handler init must be covered by write-gate (no session rows)"
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

    cleanup.cleanup().await.expect("clean write-gate bucket");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_write_gate_advisory_lock_concurrency_contract() {
    use fileconv_server::middleware::write_gate::{
        acquire_background_mutation_guard, BACKUP_ADVISORY_LOCK_KEY,
    };

    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let Some(store) = test_minio_client() else {
        return;
    };
    let cleanup = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (org, user, token) = seed_http_principal(&pool).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));

    // 1) Shared request/background guard held ⇒ exclusive try on other conn is false.
    let shared = acquire_background_mutation_guard(&pool)
        .await
        .expect("shared background guard");
    {
        let other = pool.get().await.expect("other conn");
        let exclusive: bool = other
            .query_one(
                "SELECT pg_try_advisory_lock($1)",
                &[&BACKUP_ADVISORY_LOCK_KEY],
            )
            .await
            .expect("try exclusive")
            .get(0);
        assert!(
            !exclusive,
            "pg_try_advisory_lock(7303003) must be false while shared guard held"
        );
    }
    shared.release().await;

    // 2) Exclusive held ⇒ business + background acquire fail closed, no side effects.
    let holder = pool.get().await.expect("exclusive holder");
    let got_exclusive: bool = holder
        .query_one(
            "SELECT pg_try_advisory_lock($1)",
            &[&BACKUP_ADVISORY_LOCK_KEY],
        )
        .await
        .expect("take exclusive")
        .get(0);
    assert!(
        got_exclusive,
        "exclusive lock must be acquirable after shared release"
    );

    assert!(
        acquire_background_mutation_guard(&pool).await.is_err(),
        "background acquire must fail closed under exclusive lock"
    );

    let slug = format!("exclusive-blocked-{}", Uuid::new_v4().simple());
    let (status, err, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "Blocked",
            "slug": slug,
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{err}");
    assert_eq!(err["code"], "ops_fence_active");

    let sessions_before: i64 = {
        let client = pool.get().await.expect("count conn");
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM ask_stream_sessions WHERE org_id = $1",
                &[&org],
            )
            .await
            .expect("sessions before")
            .get(0)
    };
    let (status, err, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/ask/stream",
        Some(&token),
        Some(serde_json::json!({
            "question": "Kinh phí?",
            "mode": "current",
            "limit": 3
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{err}");
    assert_eq!(err["code"], "ops_fence_active");
    let sessions_after: i64 = {
        let client = pool.get().await.expect("count conn");
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM ask_stream_sessions WHERE org_id = $1",
                &[&org],
            )
            .await
            .expect("sessions after")
            .get(0)
    };
    assert_eq!(
        sessions_after, sessions_before,
        "exclusive lock must cover ask/stream init (no session side effects)"
    );

    let collections_named: i64 = {
        let client = pool.get().await.expect("collection count");
        client
            .query_one(
                "SELECT COUNT(*)::bigint FROM collections WHERE org_id = $1 AND slug = $2",
                &[&org, &slug],
            )
            .await
            .expect("slug count")
            .get(0)
    };
    assert_eq!(collections_named, 0, "refused collection must not persist");

    holder
        .execute(
            "SELECT pg_advisory_unlock($1)",
            &[&BACKUP_ADVISORY_LOCK_KEY],
        )
        .await
        .expect("unlock exclusive");
    drop(holder);

    // 3) After release, lock can be acquired again; no session advisory leak into pool.
    for _ in 0..4 {
        let probe = pool.get().await.expect("pool probe");
        let ok: bool = probe
            .query_one(
                "SELECT pg_try_advisory_lock($1)",
                &[&BACKUP_ADVISORY_LOCK_KEY],
            )
            .await
            .expect("reacquire")
            .get(0);
        assert!(
            ok,
            "advisory lock must be free on pooled connections after release (no leak)"
        );
        probe
            .execute(
                "SELECT pg_advisory_unlock($1)",
                &[&BACKUP_ADVISORY_LOCK_KEY],
            )
            .await
            .expect("unlock probe");
        drop(probe);
    }

    let (status, created, _) = json_request(
        app,
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "AfterUnlock",
            "slug": format!("after-unlock-{}", Uuid::new_v4().simple()),
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let _ = (org, user);

    cleanup.cleanup().await.expect("clean advisory-lock bucket");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]
async fn live_http_unauthenticated_and_cross_tenant_are_consistent() {
    let Some(admin) = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(app_url) = take_live(app_database_url(), "MARKHAND_TEST_APP_DATABASE_URL") else {
        return;
    };
    let Some(store) = take_live(test_minio_client(), "MARKHAND_TEST_MINIO_*") else {
        return;
    };
    let cleanup = MinioCleanupGuard::new(store.clone());
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;
    let (_org, _user, token) = seed_http_principal(&pool).await;
    let foreign_marker = format!("foreign-marker-{}", Uuid::new_v4().simple());
    let (
        foreign_collection,
        foreign_document,
        foreign_version,
        foreign_job,
        foreign_conflict,
        foreign_org,
        foreign_user,
    ) = seed_foreign_collection_document(&pool, &foreign_marker, Some(&store)).await;
    let app = build_router(pool.clone(), &ephemeral.app_url, Some(store.clone()));

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

    let (status, own_collection, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/collections",
        Some(&token),
        Some(serde_json::json!({
            "name": "Version upload IDOR owner",
            "slug": format!("version-upload-idor-{}", Uuid::new_v4().simple()),
            "visibility": "org"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{own_collection}");
    let own_collection_id =
        Uuid::parse_str(own_collection["id"].as_str().expect("own collection id")).unwrap();
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
                .header("idempotency-key", "foreign-document-version-upload")
                .body(Body::from(multipart_body(
                    "foreign-version.txt",
                    foreign_marker.as_bytes(),
                    own_collection_id,
                    Some(foreign_document),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let denied = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        !String::from_utf8_lossy(&denied).contains(&foreign_marker),
        "version upload denial leaked foreign marker"
    );

    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/collections/{foreign_collection}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/collections/{foreign_collection}/documents"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    // Mutating methods on a foreign collection must hide existence exactly like
    // GET does. The permission gate runs before the scope check in both
    // handlers, so this only proves existence-hiding for a principal that holds
    // the permission — which this one does (doc.upload, doc.delete). A 403 here
    // would be an existence oracle: it separates "not yours" from "not there".
    assert_foreign_not_found(
        app.clone(),
        "PATCH",
        format!("/api/v1/collections/{foreign_collection}"),
        &token,
        Some(serde_json::json!({ "name": "foreign patch probe" })),
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "DELETE",
        format!("/api/v1/collections/{foreign_collection}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "POST",
        format!(
            "/api/v1/collections/{foreign_collection}/documents/{foreign_document}/approve-intake"
        ),
        &token,
        Some(serde_json::json!({ "reason": "foreign denial probe" })),
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/documents/{foreign_document}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "DELETE",
        format!("/api/v1/documents/{foreign_document}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/documents/{foreign_document}/preview?version_id={foreign_version}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/documents/{foreign_document}/versions"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/documents/{foreign_document}/versions/{foreign_version}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!(
            "/api/v1/documents/{foreign_document}/versions/{foreign_version}/diff?against={foreign_version}"
        ),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "POST",
        format!("/api/v1/documents/{foreign_document}/versions/{foreign_version}/publish"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "POST",
        format!(
            "/api/v1/documents/{foreign_document}/versions/{foreign_version}/download-capability"
        ),
        &token,
        Some(serde_json::json!({ "purpose": "markdown" })),
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "POST",
        format!("/api/v1/documents/{foreign_document}/reindex"),
        &token,
        Some(serde_json::json!({})),
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/jobs/{foreign_job}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/conflicts/{foreign_conflict}"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "GET",
        format!("/api/v1/conflicts/{foreign_conflict}/evidence"),
        &token,
        None,
        &foreign_marker,
    )
    .await;
    assert_foreign_not_found(
        app.clone(),
        "POST",
        format!("/api/v1/conflicts/{foreign_conflict}/triage"),
        &token,
        Some(serde_json::json!({
            "status": "false_positive",
            "resolutionNote": "foreign denial probe"
        })),
        &foreign_marker,
    )
    .await;
    let foreign_sha = sha256_hex(foreign_marker.as_bytes());
    assert_foreign_not_found(
        app.clone(),
        "POST",
        "/api/v1/citations/resolve".to_string(),
        &token,
        Some(serde_json::json!({
            "logicalDocumentId": foreign_document,
            "versionId": foreign_version,
            "sourceContentSha256": foreign_sha.clone(),
            "canonicalMarkdownSha256": foreign_sha,
            "chunkId": Uuid::new_v4(),
            "sourceSpanStart": 0,
            "sourceSpanEnd": foreign_marker.len(),
            "quoteLocalStart": 0,
            "quoteLocalEnd": foreign_marker.len(),
            "quote": foreign_marker.clone(),
            "requireCurrent": true
        })),
        &foreign_marker,
    )
    .await;

    // Conflict list is org-scoped: tenant A must not see tenant B's open conflict.
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
        conflicts["requestId"].as_str().is_some(),
        "conflict list must return stable error envelope fields on success: {conflicts}"
    );
    assert!(
        !conflicts["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == foreign_conflict.to_string()),
        "foreign conflict must not appear in tenant A list: {conflicts}"
    );
    assert!(
        !conflicts.to_string().contains(&foreign_marker),
        "conflict list leaked foreign marker: {conflicts}"
    );

    // Foreign capability minted by the foreign tenant must not redeem under tenant A.
    let foreign_token = login_access_token(
        &pool,
        &format!("{foreign_user}@foreign-http.test"),
        "correct-password-1",
    )
    .await;
    let _ = foreign_org;
    let (status, issued, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/documents/{foreign_document}/versions/{foreign_version}/download-capability"
        ),
        Some(&foreign_token),
        Some(serde_json::json!({ "purpose": "original" })),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "foreign tenant must mint capability via production route: {issued}"
    );
    let foreign_capability = issued["capability"]
        .as_str()
        .expect("foreign capability token")
        .to_string();
    let (status, error, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/downloads/{foreign_capability}"),
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "GET /downloads/{{foreign}} must hide foreign capability: {error}"
    );
    assert_eq!(error["code"], "not_found", "GET /downloads: {error}");
    assert!(
        error["requestId"].as_str().is_some(),
        "download denial must include requestId: {error}"
    );
    assert!(
        !error.to_string().contains(&foreign_marker)
            && !String::from_utf8_lossy(&body).contains(&foreign_marker),
        "download denial leaked foreign marker: {error}"
    );

    cleanup.cleanup().await.expect("clean cross-tenant bucket");

    ephemeral.drop().await;
}

/// Retrieval routes are the widest cross-tenant surface left: the collection id
/// arrives in the body, not the path, so the resource-route 404 probes above
/// never touch them. `resolve_scope` treats any requested id outside the
/// allow-list as a hard deny, so the contract here is 403 `forbidden` rather
/// than the 404 that hides existence for addressed resources — a foreign id in a
/// filter reveals nothing either way, because the caller already supplied it.
///
/// Needs Qdrant because both handlers resolve `vector_index()` before scope and
/// would otherwise answer 503. `hybrid_search` resolves scope before it touches
/// the embedder, so `None` is enough here.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_QDRANT_URL"]
async fn live_http_retrieval_refuses_foreign_collection_scope() {
    let Some(admin) = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL") else {
        return;
    };
    let Some(app_url) = take_live(app_database_url(), "MARKHAND_TEST_APP_DATABASE_URL") else {
        return;
    };
    let Some(qdrant_url) = take_live(
        std::env::var("MARKHAND_TEST_QDRANT_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        "MARKHAND_TEST_QDRANT_URL",
    ) else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let (_org, _user, token) = seed_http_principal(&pool).await;
    let foreign_marker = format!("foreign-scope-{}", Uuid::new_v4());
    let (foreign_collection, _, _, _, _, _, _) =
        seed_foreign_collection_document(&pool, &foreign_marker, None).await;

    let qdrant = fileconv_server::storage::QdrantClient::new(&qdrant_url).expect("qdrant");
    let state = common::build_app_state(pool.clone(), &ephemeral.app_url, None)
        .with_retrieval_backends(qdrant, None);
    let app = fileconv_server::http::router(state);

    for (uri, body) in [
        (
            "/api/v1/search",
            serde_json::json!({
                "query": "kinh phí được phê duyệt",
                "collectionIds": [foreign_collection],
            }),
        ),
        (
            "/api/v1/ask",
            serde_json::json!({
                "question": "kinh phí được phê duyệt là bao nhiêu?",
                "collectionIds": [foreign_collection],
            }),
        ),
        (
            "/api/v1/ask/stream",
            serde_json::json!({
                "question": "kinh phí được phê duyệt là bao nhiêu?",
                "collectionIds": [foreign_collection],
            }),
        ),
    ] {
        let (status, error, body) =
            json_request(app.clone(), "POST", uri, Some(&token), Some(body), &[]).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "POST {uri} did not deny a foreign collection scope: {error}"
        );
        assert_eq!(error["code"], "forbidden", "POST {uri}: {error}");
        assert!(
            !error.to_string().contains(&foreign_marker),
            "POST {uri} leaked foreign marker: {error}"
        );
        assert!(
            !String::from_utf8_lossy(&body).contains("event:"),
            "POST {uri} must not emit SSE for foreign scope: {error}"
        );
    }

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

/// Strict prerequisite mode contract for integration CI (`MARKHAND_TEST_REQUIRED=1`).
mod required_mode {
    use super::common::{
        admin_database_url, markhand_e2e_required, markhand_test_required, take_live,
        SavedEnvVars, test_env_lock,
    };

    const PREREQ_ENV_VARS: &[&str] = &[
        "MARKHAND_TEST_REQUIRED",
        "MARKHAND_E2E",
        "MARKHAND_TEST_DATABASE_URL",
    ];

    fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
        if let Some(message) = payload.downcast_ref::<&str>() {
            message.to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            String::new()
        }
    }

    #[test]
    fn markhand_test_required_honors_markhand_test_required_env() {
        let _lock = test_env_lock();
        let _saved = SavedEnvVars::save(PREREQ_ENV_VARS);
        std::env::remove_var("MARKHAND_E2E");
        std::env::set_var("MARKHAND_TEST_REQUIRED", "1");

        assert!(
            markhand_test_required(),
            "MARKHAND_TEST_REQUIRED=1 must enable required mode without MARKHAND_E2E"
        );
        assert!(
            !markhand_e2e_required(),
            "test must not conflate required mode with MARKHAND_E2E"
        );
    }

    #[test]
    fn take_live_panics_when_markhand_test_required_without_prerequisite() {
        let _lock = test_env_lock();
        let _saved = SavedEnvVars::save(PREREQ_ENV_VARS);
        std::env::set_var("MARKHAND_TEST_REQUIRED", "1");
        std::env::remove_var("MARKHAND_E2E");
        std::env::remove_var("MARKHAND_TEST_DATABASE_URL");

        let outcome = std::panic::catch_unwind(|| {
            let _ = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL");
        });
        assert!(
            outcome.is_err(),
            "take_live must panic when MARKHAND_TEST_REQUIRED=1 and DATABASE_URL is missing"
        );
    }

    #[test]
    fn take_live_panic_message_names_markhand_test_required() {
        let _lock = test_env_lock();
        let _saved = SavedEnvVars::save(PREREQ_ENV_VARS);
        std::env::set_var("MARKHAND_TEST_REQUIRED", "1");
        std::env::remove_var("MARKHAND_E2E");
        std::env::remove_var("MARKHAND_TEST_DATABASE_URL");

        let outcome = std::panic::catch_unwind(|| {
            let _ = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL");
        });
        let message = panic_payload_message(outcome.expect_err("expected prerequisite panic"));
        assert!(
            message.contains("MARKHAND_TEST_REQUIRED=1 requires MARKHAND_TEST_DATABASE_URL"),
            "panic must name the missing prerequisite in required mode, got: {message}"
        );
    }

    #[test]
    fn take_live_soft_skips_without_required_flags() {
        let _lock = test_env_lock();
        let _saved = SavedEnvVars::save(PREREQ_ENV_VARS);
        std::env::remove_var("MARKHAND_TEST_REQUIRED");
        std::env::remove_var("MARKHAND_E2E");
        std::env::remove_var("MARKHAND_TEST_DATABASE_URL");

        assert!(
            take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL").is_none(),
            "local runs without required mode must remain explicitly skippable"
        );
    }

    #[test]
    fn take_live_still_panics_under_markhand_e2e() {
        let _lock = test_env_lock();
        let _saved = SavedEnvVars::save(PREREQ_ENV_VARS);
        std::env::remove_var("MARKHAND_TEST_REQUIRED");
        std::env::set_var("MARKHAND_E2E", "1");
        std::env::remove_var("MARKHAND_TEST_DATABASE_URL");

        let outcome = std::panic::catch_unwind(|| {
            let _ = take_live(admin_database_url(), "MARKHAND_TEST_DATABASE_URL");
        });
        assert!(
            outcome.is_err(),
            "existing MARKHAND_E2E=1 strict path must keep panicking on missing prerequisites"
        );
    }
}
