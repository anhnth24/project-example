//! Live PostgreSQL HTTP contract tests for P2-19 private per-user Q&A chat
//! history (`qa_chat_sessions`/`qa_chat_turns`, migrations/0034).
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset (see `common::boot_app_pool`). No MinIO, no Qdrant needed anywhere in
//! this file: chat history never touches retrieval/vector storage — it is a
//! plain, permission-gated CRUD surface over two Postgres tables.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_database_url, app_database_url, boot_app_pool, build_router};
use deadpool_postgres::Pool;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correct-password-1";

async fn boot_pool() -> Option<(common::DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(match body {
            Some(value) => Body::from(value.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json)
}

const FULL_PERMS: &[&str] = &["qa.query"];

async fn seed_caller(pool: &Pool, org: Uuid, user: Uuid, email: &str) -> String {
    common::seed_user_with_permissions(pool, org, user, email, PASSWORD, FULL_PERMS).await;
    common::login_access_token(pool, email, PASSWORD).await
}

async fn seed_caller_without_permissions(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    email: &str,
) -> String {
    // Intentionally omits `qa.query`: history CRUD 403 tests, not collection scope.
    common::seed_user_with_permissions(pool, org, user, email, PASSWORD, &[]).await;
    common::login_access_token(pool, email, PASSWORD).await
}

// ---------------------------------------------------------------------
// CRUD happy path + title bounds
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_list_get_rename_delete_round_trip() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-crud@chat-history.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "Câu hỏi về hợp đồng" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["title"], "Câu hỏi về hợp đồng");
    let session_id = created["id"].as_str().unwrap().to_string();

    let (status, list) = send(&app, "GET", "/api/v1/chat-sessions", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&session_id));

    let (status, detail) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["id"], session_id);
    assert_eq!(detail["turns"].as_array().unwrap().len(), 0);

    let (status, renamed) = send(
        &app,
        "PATCH",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        Some(json!({ "title": "Đổi tên phiên" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["title"], "Đổi tên phiên");

    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, gone) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn create_and_rename_reject_invalid_title_bounds() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-title-bounds@chat-history.test").await;

    let (status, empty) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty}");
    assert_eq!(empty["code"], "validation_failed");

    let too_long = "x".repeat(201);
    let (status, oversized) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": too_long })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{oversized}");

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "Valid Title" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session_id = created["id"].as_str().unwrap().to_string();

    let (status, bad_rename) = send(
        &app,
        "PATCH",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        Some(json!({ "title": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_rename}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn chat_endpoints_denied_without_qa_query_permission() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token =
        seed_caller_without_permissions(&pool, org, user, "chat-no-perm@chat-history.test").await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "Should Be Denied" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "forbidden");

    let (status, body) = send(&app, "GET", "/api/v1/chat-sessions", Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Turns: seq ordering, citations/warnings jsonb round-trip, question bounds.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn append_turns_assigns_sequential_seq_and_round_trips_citations() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-turns@chat-history.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "Turns Session" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session_id = created["id"].as_str().unwrap().to_string();

    let citations = json!([
        {
            "citeId": "CITE-0001",
            "documentId": Uuid::new_v4().to_string(),
            "quote": "Kinh phí hiện tại là 15 triệu đồng.",
        }
    ]);
    let warnings =
        json!(["Structured entailment unavailable; fail-closed extractive-only grounding."]);

    let (status, turn1) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": "Ngân sách hiện tại là bao nhiêu?",
            "answer": "Ngân sách hiện tại là 15 triệu đồng [CITE-0001].",
            "answerMode": "offline_extractive",
            "citations": citations,
            "warnings": warnings,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{turn1}");
    assert_eq!(turn1["seq"], 1);
    assert_eq!(turn1["citations"], citations);
    assert_eq!(turn1["warnings"], warnings);

    let (status, turn2) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": "Còn câu hỏi thứ hai thì sao?",
            "answer": "Đây là câu trả lời thứ hai.",
            "answerMode": "fallback_extractive",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{turn2}");
    assert_eq!(turn2["seq"], 2);
    // citations/warnings default to [] when omitted.
    assert_eq!(turn2["citations"], json!([]));
    assert_eq!(turn2["warnings"], json!([]));

    let (status, detail) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let turns = detail["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["seq"], 1);
    assert_eq!(turns[1]["seq"], 2);
    assert_eq!(turns[0]["citations"], citations);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn append_turn_rejects_invalid_question_and_answer_mode() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-turn-bounds@chat-history.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token),
        Some(json!({ "title": "Bounds Session" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session_id = created["id"].as_str().unwrap().to_string();

    let (status, empty_question) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": "   ",
            "answer": "answer",
            "answerMode": "offline_extractive",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty_question}");

    let too_long_question = "x".repeat(8_193);
    let (status, oversized) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": too_long_question,
            "answer": "answer",
            "answerMode": "offline_extractive",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{oversized}");

    let (status, bad_mode) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": "A valid question?",
            "answer": "answer",
            "answerMode": "not_a_real_mode",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_mode}");

    let (status, not_array) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token),
        Some(json!({
            "question": "A valid question?",
            "answer": "answer",
            "answerMode": "offline_extractive",
            "citations": { "not": "an array" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{not_array}");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn append_turn_404_for_unknown_session() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-turn-ghost@chat-history.test").await;

    let ghost = Uuid::new_v4();
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{ghost}/turns"),
        Some(&token),
        Some(json!({
            "question": "Anything?",
            "answer": "answer",
            "answerMode": "offline_extractive",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Cursor pagination.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn list_chat_sessions_paginates_with_cursor() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = seed_caller(&pool, org, user, "chat-pagination@chat-history.test").await;

    let mut created_ids = Vec::new();
    for index in 0..3 {
        let (status, created) = send(
            &app,
            "POST",
            "/api/v1/chat-sessions",
            Some(&token),
            Some(json!({ "title": format!("Session {index}") })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created_ids.push(created["id"].as_str().unwrap().to_string());
    }

    let (status, page1) = send(
        &app,
        "GET",
        "/api/v1/chat-sessions?limit=2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    let items1 = page1["items"].as_array().unwrap();
    assert_eq!(items1.len(), 2);
    assert_eq!(page1["page"]["hasMore"], true);
    let cursor = page1["page"]["nextCursor"].as_str().unwrap().to_string();

    let (status, page2) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions?limit=2&cursor={cursor}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    let items2 = page2["items"].as_array().unwrap();
    assert_eq!(items2.len(), 1);
    assert_eq!(page2["page"]["hasMore"], false);

    let mut all_ids: Vec<String> = items1
        .iter()
        .chain(items2.iter())
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    all_ids.sort();
    let mut expected = created_ids.clone();
    expected.sort();
    assert_eq!(all_ids, expected);

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Ownership: user B (same org) must never see/open/delete user A's session.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn user_b_cannot_see_open_or_delete_user_a_session_same_org() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let token_a = seed_caller(&pool, org, user_a, "chat-user-a@chat-history.test").await;
    let token_b = seed_caller(&pool, org, user_b, "chat-user-b@chat-history.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token_a),
        Some(json!({ "title": "User A Private Session" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session_id = created["id"].as_str().unwrap().to_string();

    // User B's list must not include user A's session.
    let (status, list_b) = send(&app, "GET", "/api/v1/chat-sessions", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "{list_b}");
    let ids_b: Vec<String> = list_b["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !ids_b.contains(&session_id),
        "user B must never see user A's session: {list_b}"
    );

    // User B opening it directly must 404, not 403 (no existence oracle).
    let (status, get_b) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{get_b}");

    // User B renaming it must 404.
    let (status, rename_b) = send(
        &app,
        "PATCH",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token_b),
        Some(json!({ "title": "Hijacked" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{rename_b}");

    // User B appending a turn to it must 404.
    let (status, append_b) = send(
        &app,
        "POST",
        &format!("/api/v1/chat-sessions/{session_id}/turns"),
        Some(&token_b),
        Some(json!({
            "question": "Can I sneak a turn in?",
            "answer": "no",
            "answerMode": "offline_extractive",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{append_b}");

    // User B deleting it must 404, and it must still exist for user A.
    let (status, delete_b) = send(
        &app,
        "DELETE",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{delete_b}");

    let (status, still_there) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token_a),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "user A's session must survive user B's denied delete attempt: {still_there}"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// Org isolation.
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn org_b_cannot_see_or_open_org_a_session() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let app = build_router(pool.clone(), &ephemeral.app_url, None);
    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let token_a = seed_caller(&pool, org_a, user_a, "chat-org-a@chat-history.test").await;

    let (status, created) = send(
        &app,
        "POST",
        "/api/v1/chat-sessions",
        Some(&token_a),
        Some(json!({ "title": "Org A Session" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session_id = created["id"].as_str().unwrap().to_string();

    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let token_b = seed_caller(&pool, org_b, user_b, "chat-org-b@chat-history.test").await;

    let (status, list_b) = send(&app, "GET", "/api/v1/chat-sessions", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK, "{list_b}");
    assert_eq!(
        list_b["items"].as_array().unwrap().len(),
        0,
        "org B must see zero sessions: {list_b}"
    );

    let (status, get_b) = send(
        &app,
        "GET",
        &format!("/api/v1/chat-sessions/{session_id}"),
        Some(&token_b),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{get_b}");

    ephemeral.drop().await;
}
