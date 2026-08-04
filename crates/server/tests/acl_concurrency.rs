//! DB-gated concurrency tests for migration `0036_expand_acl_groups_invariants.sql`.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` is unset; runs in GitHub
//! `rust-integration` with live PostgreSQL.

mod common;

use common::admin_database_url;
use fileconv_server::database::apply_migrations;
use std::sync::Arc;
use tokio::sync::Barrier;
use tokio_postgres::GenericClient;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

fn pg_error_text(error: &tokio_postgres::Error) -> String {
    if let Some(db) = error.as_db_error() {
        format!("{} {}", db.message(), db.detail().unwrap_or(""))
    } else {
        format!("{error:?}")
    }
}

fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
    let (without_query, query) = match base_url.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (base_url, None),
    };
    let prefix = without_query
        .rsplit_once('/')
        .map(|(head, _)| head)
        .expect("database URL must include a path");
    match query {
        Some(tail) => format!("{prefix}/{database_name}?{tail}"),
        None => format!("{prefix}/{database_name}"),
    }
}

async fn connect(database_url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

struct EphemeralDb {
    admin_url: String,
    db_name: String,
    url: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let db_name = format!("markhand_it_{}", Uuid::new_v4().simple());
        let admin_url = rewrite_database_url(base_url, "postgres");
        let admin = connect(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        Self {
            admin_url,
            db_name: db_name.clone(),
            url: rewrite_database_url(base_url, &db_name),
        }
    }

    async fn drop(self) {
        let admin = connect(&self.admin_url).await;
        admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = '{}' AND pid <> pg_backend_pid()",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("terminate backends failed: {error}"));
        admin
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("DROP DATABASE WITH (FORCE) failed: {error}"));
    }
}

async fn set_org<C: GenericClient>(client: &C, org_id: Uuid) {
    client
        .batch_execute(&format!("SET LOCAL app.org_id = '{org_id}'"))
        .await
        .expect("SET LOCAL app.org_id");
}

struct AclFixture {
    org: Uuid,
    _owner: Uuid,
    collection: Uuid,
    group_id: Uuid,
    role_id: Uuid,
}

async fn seed_acl_fixture(client: &mut Client, visibility: &str) -> AclFixture {
    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let collection = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let role_id = Uuid::new_v4();

    let tx = client.transaction().await.unwrap();
    let slug = format!("org-{}", &org.simple().to_string()[..8]);
    let org_name = "ACL IT";
    tx.execute(
        "INSERT INTO orgs (id, slug, name) VALUES ($1, $2, $3)",
        &[&org, &slug, &org_name],
    )
    .await
    .unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Owner')",
        &[&owner, &format!("owner-{}@acl-it.test", owner.simple())],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO org_memberships (org_id, user_id, role) VALUES ($1, $2, 'owner')",
        &[&org, &owner],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES ($1, $2, 'Docs', $3, $4, $5)",
        &[
            &collection,
            &org,
            &format!("docs-{}", &collection.simple().to_string()[..8]),
            &owner,
            &visibility,
        ],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'Editors')",
        &[&group_id, &org],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO roles (id, org_id, code, name, is_system) VALUES ($1, $2, 'viewer', 'Viewer', true)",
        &[&role_id, &org],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    AclFixture {
        org,
        _owner: owner,
        collection,
        group_id,
        role_id,
    }
}

async fn dormant_grant_count(client: &Client) -> i64 {
    client
        .query_one(
            "SELECT count(*)::bigint
             FROM collections c
             WHERE c.visibility <> 'groups'
               AND (
                   EXISTS (
                       SELECT 1 FROM collection_group_access g
                       WHERE g.org_id = c.org_id AND g.collection_id = c.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM collection_role_access r
                       WHERE r.org_id = c.org_id AND r.collection_id = c.id
                   )
               )",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

async fn collection_group_grant_state(
    client: &Client,
    org: Uuid,
    collection: Uuid,
) -> (String, i64) {
    let visibility: String = client
        .query_one(
            "SELECT visibility FROM collections WHERE org_id = $1 AND id = $2",
            &[&org, &collection],
        )
        .await
        .unwrap()
        .get(0);
    let grants: i64 = client
        .query_one(
            "SELECT count(*)::bigint FROM collection_group_access
             WHERE org_id = $1 AND collection_id = $2",
            &[&org, &collection],
        )
        .await
        .unwrap()
        .get(0);
    (visibility, grants)
}

async fn org_acl_version(client: &Client, org: Uuid) -> i64 {
    client
        .query_one("SELECT acl_version FROM orgs WHERE id = $1", &[&org])
        .await
        .unwrap()
        .get(0)
}

async fn seed_second_groups_collection(client: &mut Client, fixture: &AclFixture) -> Uuid {
    let collection = Uuid::new_v4();
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES ($1, $2, 'Archive', $3, $4, 'groups')",
        &[
            &collection,
            &fixture.org,
            &format!("archive-{}", &collection.simple().to_string()[..8]),
            &fixture._owner,
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    collection
}

async fn boot_db() -> Option<EphemeralDb> {
    let base = admin_database_url()?;
    let ephemeral = EphemeralDb::create(&base).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    Some(ephemeral)
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn group_grant_on_private_collection_is_rejected() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "private").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    let err = tx
        .execute(
            "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
             VALUES ($1, $2, $3, 'read')",
            &[&fixture.org, &fixture.collection, &fixture.group_id],
        )
        .await
        .expect_err("group grant on private collection must fail");
    assert!(
        pg_error_text(&err).contains("visibility groups"),
        "{}",
        pg_error_text(&err)
    );
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn role_grant_on_org_collection_is_rejected() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "org").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    let err = tx
        .execute(
            "INSERT INTO collection_role_access (org_id, collection_id, role_id, access_level)
             VALUES ($1, $2, $3, 'read')",
            &[&fixture.org, &fixture.collection, &fixture.role_id],
        )
        .await
        .expect_err("role grant on org collection must fail");
    assert!(
        pg_error_text(&err).contains("visibility groups"),
        "{}",
        pg_error_text(&err)
    );
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn groups_visibility_accepts_group_and_role_grants() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO collection_role_access (org_id, collection_id, role_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn groups_to_private_fails_while_role_grants_remain() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_role_access (org_id, collection_id, role_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    let err = tx
        .execute(
            "UPDATE collections SET visibility = 'private'
             WHERE org_id = $1 AND id = $2",
            &[&fixture.org, &fixture.collection],
        )
        .await
        .expect_err("visibility flip with remaining role grants must fail");
    assert!(
        pg_error_text(&err).contains("grants remain"),
        "{}",
        pg_error_text(&err)
    );
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn groups_to_private_fails_while_grants_remain() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    let err = tx
        .execute(
            "UPDATE collections SET visibility = 'private'
             WHERE org_id = $1 AND id = $2",
            &[&fixture.org, &fixture.collection],
        )
        .await
        .expect_err("visibility flip with remaining grants must fail");
    assert!(
        pg_error_text(&err).contains("grants remain"),
        "{}",
        pg_error_text(&err)
    );
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_grant_vs_visibility_flip_cannot_leave_dormant_rows() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let url = ephemeral.url.clone();
    let barrier = Arc::new(Barrier::new(2));

    let flip = {
        let url = url.clone();
        let barrier = Arc::clone(&barrier);
        let org = fixture.org;
        let collection = fixture.collection;
        tokio::spawn(async move {
            let mut client = connect(&url).await;
            let tx = client.transaction().await.unwrap();
            set_org(&tx, org).await;
            barrier.wait().await;
            let dml = tx
                .execute(
                    "UPDATE collections SET visibility = 'private'
                     WHERE org_id = $1 AND id = $2",
                    &[&org, &collection],
                )
                .await;
            match dml {
                Ok(_) => tx.commit().await.is_ok(),
                Err(_) => {
                    tx.rollback().await.ok();
                    false
                }
            }
        })
    };

    let grant = {
        let url = url.clone();
        let barrier = Arc::clone(&barrier);
        let org = fixture.org;
        let collection = fixture.collection;
        let group_id = fixture.group_id;
        tokio::spawn(async move {
            let mut client = connect(&url).await;
            let tx = client.transaction().await.unwrap();
            set_org(&tx, org).await;
            barrier.wait().await;
            let dml = tx
                .execute(
                    "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
                     VALUES ($1, $2, $3, 'read')",
                    &[&org, &collection, &group_id],
                )
                .await;
            match dml {
                Ok(_) => tx.commit().await.is_ok(),
                Err(_) => {
                    tx.rollback().await.ok();
                    false
                }
            }
        })
    };

    let flip_committed = flip.await.expect("flip task join");
    let grant_committed = grant.await.expect("grant task join");

    assert_eq!(
        usize::from(flip_committed) + usize::from(grant_committed),
        1,
        "grant-vs-flip race must leave exactly one durable winner: flip={flip_committed} grant={grant_committed}"
    );

    let client = connect(&ephemeral.url).await;
    assert_eq!(
        dormant_grant_count(&client).await,
        0,
        "no interleaving may leave group/role grants on non-groups collections"
    );
    let (visibility, grants) =
        collection_group_grant_state(&client, fixture.org, fixture.collection).await;
    match (visibility.as_str(), grants) {
        ("private", 0) | ("groups", 1) => {}
        other => panic!("incoherent durable final state: {other:?}"),
    }
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn grant_retargeting_collection_id_is_rejected() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;
    let other_collection = seed_second_groups_collection(&mut client, &fixture).await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    let err = tx
        .execute(
            "UPDATE collection_group_access
             SET collection_id = $4
             WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
            &[
                &fixture.org,
                &fixture.collection,
                &fixture.group_id,
                &other_collection,
            ],
        )
        .await
        .expect_err("in-place collection retargeting must fail");
    let message = pg_error_text(&err);
    assert!(
        message.contains("delete and re-insert") || message.contains("immutable"),
        "{message}"
    );
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn grant_access_level_update_still_validates_parent() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.execute(
        "UPDATE collection_group_access
         SET access_level = 'write'
         WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    let level: String = tx
        .query_one(
            "SELECT access_level FROM collection_group_access
             WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
            &[&fixture.org, &fixture.collection, &fixture.group_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(level, "write");
    tx.commit().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_group_grant() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let before: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let after: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before + 1);
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_role_grant() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let before: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_role_access (org_id, collection_id, role_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let after: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before + 1);
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_group_grant_update_and_delete() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before_update = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "UPDATE collection_group_access
         SET access_level = 'write'
         WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_update + 1,
        "group grant UPDATE must bump acl_version once"
    );

    let before_delete = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "DELETE FROM collection_group_access
         WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
        &[&fixture.org, &fixture.collection, &fixture.group_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_delete + 1,
        "group grant DELETE must bump acl_version once"
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_role_grant_update_and_delete() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO collection_role_access (org_id, collection_id, role_id, access_level)
         VALUES ($1, $2, $3, 'read')",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before_update = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "UPDATE collection_role_access
         SET access_level = 'write'
         WHERE org_id = $1 AND collection_id = $2 AND role_id = $3",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_update + 1,
        "role grant UPDATE must bump acl_version once"
    );

    let before_delete = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "DELETE FROM collection_role_access
         WHERE org_id = $1 AND collection_id = $2 AND role_id = $3",
        &[&fixture.org, &fixture.collection, &fixture.role_id],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_delete + 1,
        "role grant DELETE must bump acl_version once"
    );
    ephemeral.drop().await;
}

// Grant tables forbid in-place org_id/collection_id retargeting (see
// `grant_retargeting_collection_id_is_rejected`), so cross-org dual-bump
// behavior of `bump_org_acl_version` is pinned by the migration shape test and
// exercised here through same-org membership UPDATE/DELETE on `group_memberships`.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_group_membership_update_and_delete() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;
    let member = Uuid::new_v4();
    let second_group = Uuid::new_v4();

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Member')",
        &[&member, &format!("member-{}@acl-it.test", member.simple())],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO org_memberships (org_id, user_id, role) VALUES ($1, $2, 'viewer')",
        &[&fixture.org, &member],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'Reviewers')",
        &[&second_group, &fixture.org],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO group_memberships (org_id, group_id, user_id) VALUES ($1, $2, $3)",
        &[&fixture.org, &fixture.group_id, &member],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before_update = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "UPDATE group_memberships
         SET group_id = $4
         WHERE org_id = $1 AND group_id = $2 AND user_id = $3",
        &[&fixture.org, &fixture.group_id, &member, &second_group],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_update + 1,
        "group membership UPDATE within the same org must bump acl_version once"
    );

    let before_delete = org_acl_version(&client, fixture.org).await;
    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "DELETE FROM group_memberships
         WHERE org_id = $1 AND group_id = $2 AND user_id = $3",
        &[&fixture.org, &second_group, &member],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        org_acl_version(&client, fixture.org).await,
        before_delete + 1,
        "group membership DELETE must bump acl_version once"
    );
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_version_bumps_on_group_membership_change() {
    let Some(ephemeral) = boot_db().await else {
        return;
    };
    let mut client = connect(&ephemeral.url).await;
    let fixture = seed_acl_fixture(&mut client, "groups").await;
    let member = Uuid::new_v4();

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Member')",
        &[&member, &format!("member-{}@acl-it.test", member.simple())],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO org_memberships (org_id, user_id, role) VALUES ($1, $2, 'viewer')",
        &[&fixture.org, &member],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let before: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);

    let tx = client.transaction().await.unwrap();
    set_org(&tx, fixture.org).await;
    tx.execute(
        "INSERT INTO group_memberships (org_id, group_id, user_id) VALUES ($1, $2, $3)",
        &[&fixture.org, &fixture.group_id, &member],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let after: i64 = client
        .query_one(
            "SELECT acl_version FROM orgs WHERE id = $1",
            &[&fixture.org],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before + 1);
    ephemeral.drop().await;
}
