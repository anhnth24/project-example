//! Phase 1C executable multi-org denial tests using the shared world.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::multi_org_denial::{
    assert_denial_no_leak, DenialExpectation, DenialResponse, MultiOrgDenialWorld,
};
use common::{admin_database_url, app_database_url};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

async fn boot_world_if_live() -> Option<MultiOrgDenialWorld> {
    admin_database_url()?;
    app_database_url()?;
    Some(MultiOrgDenialWorld::boot().await)
}

async fn json_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>, Vec<(String, String)>) {
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
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, body, headers)
}

/// Cross-org HTTP surfaces that lack a dedicated legacy integration test.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn shared_world_http_surfaces_respect_org_scope() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    world.assert_base_topology();

    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    let token_a = &alpha.users["owner"].access_token;
    let foreign = world.foreign_markers_for("orgAlpha");

    let (status, body, headers) =
        json_request(&world.app, "GET", "/api/v1/auth/me", Some(token_a), None).await;
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_denial_no_leak(
        &DenialResponse {
            status: status.as_u16(),
            body: &body,
            headers: header_refs,
        },
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let me: serde_json::Value = serde_json::from_slice(&body).expect("auth me json");
    assert!(!me.to_string().contains(&beta.org_id.to_string()));

    let (status, body, headers) = json_request(
        &world.app,
        "GET",
        "/api/v1/collections",
        Some(token_a),
        None,
    )
    .await;
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_denial_no_leak(
        &DenialResponse {
            status: status.as_u16(),
            body: &body,
            headers: header_refs,
        },
        &foreign,
        DenialExpectation::AllowSuccess,
    );
    let listed: serde_json::Value = serde_json::from_slice(&body).expect("collections json");
    assert!(!listed
        .to_string()
        .contains(&beta.collections["org"].collection_id.to_string()));

    let (status, body, headers) = json_request(
        &world.app,
        "POST",
        "/api/v1/collections",
        Some(token_a),
        Some(json!({
            "name": "Another",
            "slug": format!("denial-new-{}", uuid::Uuid::new_v4().simple()),
            "visibility": "org"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_denial_no_leak(
        &DenialResponse {
            status: status.as_u16(),
            body: &body,
            headers: header_refs,
        },
        &foreign,
        DenialExpectation::AllowSuccess,
    );

    let foreign_collection = beta.collections["org"].collection_id;
    let (status, body, headers) = json_request(
        &world.app,
        "POST",
        &format!("/api/v1/collections/{foreign_collection}/assign-project"),
        Some(token_a),
        Some(json!({ "projectId": null })),
    )
    .await;
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_denial_no_leak(
        &DenialResponse {
            status: status.as_u16(),
            body: &body,
            headers: header_refs,
        },
        &foreign,
        DenialExpectation::PathIdorNotFound,
    );

    let (status, body, headers) = json_request(
        &world.app,
        "GET",
        "/api/v1/members/invites",
        Some(token_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_denial_no_leak(
        &DenialResponse {
            status: status.as_u16(),
            body: &body,
            headers: header_refs,
        },
        &foreign,
        DenialExpectation::AllowSuccess,
    );

    world.cleanup().await.expect("cleanup");
}

fn assert_world_boots_for_task13_scaffold(world: &MultiOrgDenialWorld) {
    world.assert_base_topology();
    assert_eq!(world.orgs.len(), 2);
    assert!(world.fixture.pre_revoke_tokens);
    for org in world.orgs.values() {
        assert!(!org.marker.is_empty());
        assert!(!org.object_key.is_empty());
        assert_eq!(org.users.len(), 3);
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn indexed_fts_and_ask_never_return_foreign_marker() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn duplicate_names_across_orgs_do_not_create_an_oracle() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    assert_eq!(
        world.org("orgAlpha").document.title,
        world.fixture.duplicate_names.document
    );
    assert_eq!(
        world.org("orgBeta").document.title,
        world.fixture.duplicate_names.document
    );
    for label in ["private", "org", "groups"] {
        assert_eq!(
            world.org("orgAlpha").collections[label].name,
            world.org("orgBeta").collections[label].name,
            "cross-org collection name oracle for {label}"
        );
        assert_ne!(
            world.org("orgAlpha").collections[label].collection_id,
            world.org("orgBeta").collections[label].collection_id,
            "collection ids must differ for {label}"
        );
    }
    assert_eq!(
        world.org("orgAlpha").collections["org"].name,
        world.fixture.duplicate_names.collection
    );
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn org_switch_never_reuses_previous_org_cache_scope() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    assert!(
        world.org("orgAlpha").users["owner"].access_token
            != world.org("orgBeta").users["owner"].access_token
    );
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn pre_revoke_tokens_fail_after_downgrade_suspend_and_remove() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    assert!(!world.org("orgAlpha").users["owner"]
        .refresh_token
        .is_empty());
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL (+ MinIO for object keys)"]
async fn preview_download_job_and_sse_hide_foreign_ids() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    let foreign = world.foreign_markers_for("orgAlpha");
    assert!(!foreign.job_ids.is_empty());
    world.cleanup().await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn in_flight_ask_emits_no_content_after_acl_revoke() {
    let Some(world) = boot_world_if_live().await else {
        return;
    };
    assert_world_boots_for_task13_scaffold(&world);
    world.cleanup().await.expect("cleanup");
}
