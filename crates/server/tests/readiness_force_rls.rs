//! Cross-org readiness aggregates under FORCE RLS (P1B-R06).
//!
//! Requires `MARKHAND_TEST_DATABASE_URL` pointing at a migrated database where
//! the app role is `markhand_app` (or the connected role can EXECUTE the
//! SECURITY DEFINER helpers). Skips when unset.

use fileconv_server::database::{apply_migrations, check_connection};

fn database_url() -> Option<String> {
    std::env::var("MARKHAND_TEST_DATABASE_URL")
        .ok()
        .filter(|u| {
            !u.is_empty() && (u.starts_with("postgres://") || u.starts_with("postgresql://"))
        })
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL with markhand_app role"]
async fn app_role_sees_two_org_generation_consistency_via_definer() {
    let Some(url) = database_url() else {
        eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
        return;
    };
    apply_migrations(&url).await.expect("migrations");
    check_connection(&url).await.expect("connect");

    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("pg connect");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    // Functions must be callable without setting app.org_id (cross-org aggregate).
    let ok: bool = client
        .query_one(
            "SELECT markhand_index_generation_consistent($1)",
            &[&"a".repeat(64)],
        )
        .await
        .expect("definer callable")
        .get(0);
    let _ = ok;
    let fence: bool = client
        .query_one("SELECT markhand_any_blocking_fence_active()", &[])
        .await
        .expect("fence definer")
        .get(0);
    let reconcile: bool = client
        .query_one("SELECT markhand_any_reconcile_running()", &[])
        .await
        .expect("reconcile definer")
        .get(0);
    let _ = (fence, reconcile);
}
