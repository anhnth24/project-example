//! Live bootstrapping for [`super::MultiOrgDenialWorld`].

use std::collections::{BTreeMap, BTreeSet};

use deadpool_postgres::Pool;
use fileconv_knowledge::ask::AnswerMode;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::CollectionVisibility;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::storage::keys::trusted_key;
use fileconv_server::storage::minio::MinioClient;
use fileconv_server::storage::ObjectIdentityMeta;
use uuid::Uuid;

use crate::common::multi_org_denial::{
    assert_object_key_identity, collection_name_for_visibility, load_denial_fixture, DenialFixture,
    DenialFixtureOrg, ForeignMarkers, MultiOrgDenialWorld, PanicSafeWorldResources,
    COLLECTION_VISIBILITY_LABELS, DENIAL_FIXTURE_REL_PATH,
};
use crate::common::worker_pipeline::{ExistingScopeDocument, WorkerPipeline, WorkerProducedDoc};
use crate::common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, build_app_state,
    build_router, login_tokens, put_bytes, seed_user_with_permissions, sha256_hex,
    test_minio_client, test_qdrant_client,
};
use fileconv_server::db::models::DocumentState;
use fileconv_server::http::router;
use fileconv_server::services::qa::provider::{ChatProvider, StreamingStaticProvider};

const PASSWORD: &str = "correct-password-1";

const OWNER_PERMISSIONS: &[&str] = &[
    "doc.upload",
    "doc.delete",
    "doc.publish",
    "qa.query",
    "qa.history",
    "member.manage",
    "audit.view",
    "jobs.system",
    "doc.quarantine.review",
];

const MEMBER_PERMISSIONS: &[&str] = &["qa.query"];

#[derive(Debug, Clone)]
pub struct BootedUser {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone)]
pub struct BootedCollection {
    pub collection_id: Uuid,
    pub visibility: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BootedDocument {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct BootedIndexedDocument {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: Uuid,
    pub object_key: String,
    pub quote: String,
}

#[derive(Debug, Clone)]
pub struct BootedOrg {
    pub org_id: Uuid,
    pub slug: String,
    pub marker: String,
    pub object_key: String,
    pub users: BTreeMap<String, BootedUser>,
    pub collections: BTreeMap<String, BootedCollection>,
    /// Worker-produced searchable document; base `document` remains the duplicate-name fixture.
    pub indexed_document: Option<BootedIndexedDocument>,
    pub document: BootedDocument,
    pub job_id: Uuid,
    pub conflict_id: Uuid,
}

impl MultiOrgDenialWorld {
    pub fn org(&self, key: &str) -> &BootedOrg {
        self.orgs
            .get(key)
            .unwrap_or_else(|| panic!("unknown org key {key}"))
    }
}

/// Live worker/Qdrant/chat runtime for indexed denial probes.
///
/// Cleans up the Qdrant collection before the world releases pool/MinIO handles.
pub struct IndexedDenialRuntime {
    pipeline: Option<WorkerPipeline>,
    chat_provider: ChatProvider,
}

impl IndexedDenialRuntime {
    /// Index both org markers through production upload + workers, then wire retrieval.
    pub async fn boot_and_index(world: &mut MultiOrgDenialWorld) -> Result<Self, String> {
        let store = world
            .store
            .clone()
            .ok_or("indexed denial runtime requires MinIO (MARKHAND_TEST_MINIO_*)")?;
        let app_database_url = world.app_database_url.clone();
        let pool = world.pool().clone();

        let pipeline = WorkerPipeline::boot(pool.clone(), store.clone(), &app_database_url).await;
        let chat_provider = ChatProvider::StreamingStatic(StreamingStaticProvider::new(
            std::iter::repeat_n("denial ".to_string(), 4_096).collect(),
            AnswerMode::LocalLlm,
        ));
        let runtime = Self {
            pipeline: Some(pipeline),
            chat_provider,
        };

        for (org_key, org) in world.orgs.iter_mut() {
            let owner = org.users.get("owner").ok_or("missing owner user")?;
            let collection_id = org.collections["org"].collection_id;
            let source = format!("# {}\n\n{}\n", org.document.title, org.marker);
            let produced = runtime
                .pipeline
                .as_ref()
                .expect("indexed pipeline")
                .produce_indexed_in_existing_scope(ExistingScopeDocument {
                    access_token: &owner.access_token,
                    org_id: org.org_id,
                    user_id: owner.user_id,
                    collection_id,
                    permissions: OWNER_PERMISSIONS,
                    filename: &format!("{org_key}-marker.txt"),
                    content_type: "text/plain",
                    source: source.as_bytes(),
                    label: &format!("denial-index-{org_key}"),
                })
                .await;
            org.indexed_document = Some(load_indexed_document(&pool, org, &produced).await?);
        }

        let qdrant = test_qdrant_client()
            .ok_or("indexed denial runtime requires MARKHAND_TEST_QDRANT_URL")?;
        let retrieval_app = router(
            build_app_state(pool, &app_database_url, Some(store))
                .with_retrieval_backends(qdrant, None)
                .with_chat_provider(runtime.chat_provider.clone()),
        );
        world.app = Some(retrieval_app);

        Ok(runtime)
    }

    pub fn chat_provider(&self) -> &ChatProvider {
        &self.chat_provider
    }

    pub async fn teardown(mut self) {
        if let Some(pipeline) = self.pipeline.take() {
            pipeline.cleanup_qdrant().await;
        }
    }
}

impl Drop for IndexedDenialRuntime {
    fn drop(&mut self) {
        let Some(pipeline) = self.pipeline.take() else {
            return;
        };
        let _ = std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                runtime.block_on(pipeline.cleanup_qdrant());
            }
        })
        .join();
    }
}

async fn load_indexed_document(
    pool: &Pool,
    org: &BootedOrg,
    produced: &WorkerProducedDoc,
) -> Result<BootedIndexedDocument, String> {
    let owner = org.users.get("owner").ok_or("missing owner user")?;
    let ctx = OrgContext::try_new(
        org.org_id,
        owner.user_id,
        OWNER_PERMISSIONS.iter().copied(),
        [org.collections["org"].collection_id],
    )
    .map_err(|err| err.to_string())?;
    let (object_key, state): (String, String) = with_org_txn(pool, &ctx, {
        let document_id = produced.document_id;
        let org_id = ctx.org_id();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT dv.original_object_key, d.state::text
                         FROM documents d
                         JOIN document_versions dv
                           ON dv.org_id = d.org_id AND dv.id = d.current_version_id
                         WHERE d.org_id = $1 AND d.id = $2",
                        &[&org_id, &document_id],
                    )
                    .await?;
                Ok((row.get::<_, String>(0), row.get::<_, String>(1)))
            })
        }
    })
    .await
    .map_err(|err| format!("load indexed document identity: {err}"))?;
    if state != DocumentState::Indexed.as_str() {
        return Err(format!(
            "org {} document {} expected indexed state after worker pipeline, got {state}",
            org.slug, produced.document_id
        ));
    }
    if !object_key.starts_with("trusted/") {
        return Err(format!(
            "worker-produced object key must be trusted: {object_key}"
        ));
    }
    Ok(BootedIndexedDocument {
        document_id: produced.document_id,
        version_id: produced.version_id,
        chunk_id: produced.chunk_id,
        object_key,
        quote: produced.quote.clone(),
    })
}

async fn poll_latest_ask_stream_session(
    pool: &Pool,
    org_id: Uuid,
    user_id: Uuid,
    cited_document_id: Uuid,
) -> Result<Uuid, String> {
    for attempt in 0..60 {
        let client = pool.get().await.map_err(|err| err.to_string())?;
        let row = client
            .query_opt(
                "SELECT id FROM ask_stream_sessions
                 WHERE org_id = $1 AND user_id = $2
                   AND $3 = ANY(cited_document_ids)
                 ORDER BY created_at DESC
                 LIMIT 1",
                &[&org_id, &user_id, &cited_document_id],
            )
            .await
            .map_err(|err| err.to_string())?;
        if let Some(row) = row {
            return Ok(row.get(0));
        }
        if attempt + 1 < 60 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    Err("timed out waiting for ask_stream_sessions row".into())
}

/// Poll durable session state instead of draining SSE before ACL revoke.
pub async fn await_ask_stream_session(
    pool: &Pool,
    org_id: Uuid,
    user_id: Uuid,
    cited_document_id: Uuid,
) -> Result<Uuid, String> {
    poll_latest_ask_stream_session(pool, org_id, user_id, cited_document_id).await
}

/// Boot when live DB URLs are configured; callers in `#[ignore]` tests should gate
/// on [`crate::admin_database_url`] / [`crate::app_database_url`] first when not in
/// required mode so missing deps skip cleanly instead of panicking twice.
pub async fn boot_world() -> MultiOrgDenialWorld {
    let admin = admin_database_url().expect("MARKHAND_TEST_DATABASE_URL");
    let app_url = app_database_url().expect("MARKHAND_TEST_APP_DATABASE_URL");
    boot_world_with_urls(&admin, &app_url).await
}

pub async fn boot_world_with_urls(admin: &str, app_url: &str) -> MultiOrgDenialWorld {
    let fixture = load_denial_fixture().expect("denial fixture must parse in boot");
    if fixture.orgs.len() < 2 {
        panic!(
            "MultiOrgDenialWorld::boot requires two orgs in {}; found {}",
            DENIAL_FIXTURE_REL_PATH,
            fixture.orgs.len()
        );
    }

    let (ephemeral, pool) = boot_app_pool(admin, app_url).await;
    assert_markhand_app_role(&pool).await;
    let router_app_url = ephemeral.app_url.clone();

    let store = test_minio_client();
    let minio_guard = store.clone().map(crate::common::MinioCleanupGuard::new);
    let app = build_router(pool.clone(), &router_app_url, store.clone());
    let resources = PanicSafeWorldResources::new(ephemeral, minio_guard);

    let mut orgs = BTreeMap::new();
    for org_template in &fixture.orgs {
        let booted = seed_org_world(&pool, store.as_ref(), org_template, &fixture).await;
        assert_object_key_identity(&booted);
        orgs.insert(org_template.key.clone(), booted);
    }

    MultiOrgDenialWorld {
        fixture,
        orgs,
        store,
        app: Some(app),
        pool: Some(pool),
        app_database_url: router_app_url,
        resources,
    }
}

pub fn foreign_markers_for(world: &MultiOrgDenialWorld, actor_org_key: &str) -> ForeignMarkers {
    let foreign_key = world
        .fixture
        .orgs
        .iter()
        .map(|org| org.key.as_str())
        .find(|key| *key != actor_org_key)
        .expect("foreign org key");
    foreign_markers_between_orgs(
        world.org(actor_org_key),
        world.org(foreign_key),
        &world.fixture,
    )
}

/// Build foreign-identifying markers for leakage scans.
///
/// Display names shared with the actor org (cross-org duplicate oracles) are
/// excluded — they cannot distinguish foreign tenancy. IDs, object keys, and
/// unique marker strings always remain.
pub fn foreign_markers_between_orgs(
    actor: &BootedOrg,
    foreign: &BootedOrg,
    fixture: &DenialFixture,
) -> ForeignMarkers {
    let local_names = local_display_name_needles(actor, fixture);
    let mut markers = ForeignMarkers::default();
    markers.org_ids.push(foreign.org_id.to_string());
    for user in foreign.users.values() {
        markers.user_ids.push(user.user_id.to_string());
        push_foreign_display_name(&mut markers.names, &local_names, &user.email);
    }
    for collection in foreign.collections.values() {
        markers
            .collection_ids
            .push(collection.collection_id.to_string());
        push_foreign_display_name(&mut markers.names, &local_names, &collection.name);
    }
    markers
        .document_ids
        .push(foreign.document.document_id.to_string());
    markers
        .version_ids
        .push(foreign.document.version_id.to_string());
    if let Some(indexed) = &foreign.indexed_document {
        markers.document_ids.push(indexed.document_id.to_string());
        markers.version_ids.push(indexed.version_id.to_string());
        markers.chunk_ids.push(indexed.chunk_id.to_string());
        markers.object_keys.push(indexed.object_key.clone());
    }
    push_foreign_display_name(
        &mut markers.names,
        &local_names,
        &fixture.duplicate_names.document,
    );
    markers.job_ids.push(foreign.job_id.to_string());
    markers.conflict_ids.push(foreign.conflict_id.to_string());
    markers.object_keys.push(foreign.object_key.clone());
    markers.marker_strings.push(foreign.marker.clone());
    markers
}

fn local_display_name_needles(actor: &BootedOrg, fixture: &DenialFixture) -> BTreeSet<String> {
    let mut needles = BTreeSet::new();
    for user in actor.users.values() {
        needles.insert(user.email.to_lowercase());
    }
    for collection in actor.collections.values() {
        needles.insert(collection.name.to_lowercase());
    }
    needles.insert(actor.document.title.to_lowercase());
    needles.insert(fixture.duplicate_names.document.to_lowercase());
    needles
}

fn push_foreign_display_name(
    names: &mut Vec<String>,
    local_needles: &BTreeSet<String>,
    candidate: &str,
) {
    if !local_needles.contains(&candidate.to_lowercase()) {
        names.push(candidate.to_string());
    }
}

pub async fn cleanup_world(world: MultiOrgDenialWorld) -> Result<(), String> {
    let mut world = std::mem::ManuallyDrop::new(world);
    world.release_runtime_handles();
    let resources = std::mem::replace(&mut world.resources, PanicSafeWorldResources::empty());
    let result = resources.cleanup().await;
    let _ = std::mem::ManuallyDrop::into_inner(world);
    result
}

pub fn assert_base_topology(world: &MultiOrgDenialWorld) {
    assert_eq!(world.orgs.len(), 2, "expected two booted orgs");
    for org in world.orgs.values() {
        assert_eq!(org.users.len(), 3, "expected three users per org");
        assert_eq!(
            org.collections.len(),
            3,
            "expected private/org/groups collections"
        );
        for label in COLLECTION_VISIBILITY_LABELS {
            assert!(
                org.collections.contains_key(*label),
                "visibility matrix missing {label}"
            );
        }
        let names: BTreeSet<&str> = org.collections.values().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names.len(),
            org.collections.len(),
            "collection names must be unique within org {}",
            org.slug
        );
        for label in COLLECTION_VISIBILITY_LABELS {
            let expected = collection_name_for_visibility(&world.fixture.duplicate_names, label);
            assert_eq!(
                org.collections[*label].name, expected,
                "org {} collection {label} name mismatch",
                org.slug
            );
        }
    }
    let alpha = world.org("orgAlpha");
    let beta = world.org("orgBeta");
    for label in COLLECTION_VISIBILITY_LABELS {
        assert_eq!(
            alpha.collections[*label].name, beta.collections[*label].name,
            "cross-org duplicate oracle for {label}"
        );
        assert_ne!(
            alpha.collections[*label].collection_id, beta.collections[*label].collection_id,
            "same-name collections must not share ids for {label}"
        );
    }
    assert!(world.fixture.pre_revoke_tokens);
}

async fn seed_org_world(
    pool: &Pool,
    store: Option<&MinioClient>,
    org_template: &DenialFixtureOrg,
    fixture: &DenialFixture,
) -> BootedOrg {
    let org_id = Uuid::new_v4();
    let slug = org_template
        .slug
        .clone()
        .unwrap_or_else(|| format!("denial-{}", org_template.key.to_lowercase()));
    let marker_base = fixture
        .indexed_markers
        .get(&org_template.key)
        .cloned()
        .unwrap_or_else(|| format!("phase1c-marker-{}", org_template.key));
    let marker = format!("{marker_base}-{}", Uuid::new_v4().simple());

    let owner_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();
    let member_id = Uuid::new_v4();

    let owner_email = format!("owner-{}@{slug}.denial.test", owner_id.simple());
    let admin_email = format!("admin-{}@{slug}.denial.test", admin_id.simple());
    let member_email = format!("member-{}@{slug}.denial.test", member_id.simple());

    seed_user_with_permissions(
        pool,
        org_id,
        owner_id,
        &owner_email,
        PASSWORD,
        OWNER_PERMISSIONS,
    )
    .await;
    seed_admin_member(pool, org_id, admin_id, &admin_email).await;
    seed_viewer_member(pool, org_id, member_id, &member_email).await;

    let owner_ctx =
        OrgContext::try_new(org_id, owner_id, OWNER_PERMISSIONS.iter().copied(), []).unwrap();

    let group_id = Uuid::new_v4();
    let private_id = Uuid::new_v4();
    let org_collection_id = Uuid::new_v4();
    let groups_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let conflict_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let trusted_object_key =
        trusted_key(org_id, version_id, object_id, None).expect("trusted object key");
    let object_key_str = trusted_object_key.as_str().to_string();

    let private_collection_name =
        collection_name_for_visibility(&fixture.duplicate_names, "private").to_string();
    let org_collection_name =
        collection_name_for_visibility(&fixture.duplicate_names, "org").to_string();
    let groups_collection_name =
        collection_name_for_visibility(&fixture.duplicate_names, "groups").to_string();
    let duplicate_document_title = fixture.duplicate_names.document.clone();
    let content_sha = sha256_hex(marker.as_bytes());

    if let Some(store) = store {
        put_bytes(
            store,
            org_id,
            &trusted_object_key,
            marker.as_bytes(),
            "text/plain",
            ObjectIdentityMeta {
                org_id,
                collection_id: Some(org_collection_id),
                document_id: Some(document_id),
                version_id: Some(version_id),
                original_filename: Some(format!("{marker}.txt")),
                canonical_format: Some("txt".into()),
                content_sha256: Some(content_sha.clone()),
                content_length: Some(marker.len() as u64),
                disposition: Some("trusted".into()),
            },
        )
        .await;
    }

    with_org_txn(pool, &owner_ctx, {
        let slug = slug.clone();
        let marker = marker.clone();
        let owner_ctx = owner_ctx.clone();
        let private_collection_name = private_collection_name.clone();
        let org_collection_name = org_collection_name.clone();
        let groups_collection_name = groups_collection_name.clone();
        let duplicate_document_title = duplicate_document_title.clone();
        let object_key_str = object_key_str.clone();
        let document_state = DocumentState::Converted.as_str().to_string();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &owner_ctx, &slug, &format!("Denial {slug}")).await?;

                txn.execute(
                    "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'Denial Editors')",
                    &[&group_id, &org_id],
                )
                .await?;

                insert_collection_row(
                    txn,
                    &owner_ctx,
                    private_id,
                    &private_collection_name,
                    "private",
                    CollectionVisibility::Private,
                )
                .await?;
                insert_collection_row(
                    txn,
                    &owner_ctx,
                    org_collection_id,
                    &org_collection_name,
                    "org",
                    CollectionVisibility::Org,
                )
                .await?;
                insert_collection_row(
                    txn,
                    &owner_ctx,
                    groups_id,
                    &groups_collection_name,
                    "groups",
                    CollectionVisibility::Groups,
                )
                .await?;

                txn.execute(
                    "INSERT INTO collection_group_access (id, org_id, collection_id, group_id, access_level)
                     VALUES ($1, $2, $3, $4, 'read')",
                    &[&Uuid::new_v4(), &org_id, &groups_id, &group_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO group_memberships (org_id, group_id, user_id) VALUES ($1, $2, $3)",
                    &[&org_id, &group_id, &member_id],
                )
                .await?;

                documents::insert(
                    txn,
                    &owner_ctx,
                    NewDocument {
                        id: document_id,
                        collection_id: org_collection_id,
                        title: &duplicate_document_title,
                    },
                )
                .await?;

                let object_key_str = object_key_str.clone();
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key, markdown_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,$5,NULL,'text/plain',$6,$7)",
                    &[
                        &version_id,
                        &org_id,
                        &document_id,
                        &content_sha,
                        &object_key_str,
                        &(marker.len() as i64),
                        &owner_id,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET state=$3, current_version_id=$4
                     WHERE org_id=$1 AND id=$2",
                    &[&org_id, &document_id, &document_state, &version_id],
                )
                .await?;

                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, status, payload_version, payload,
                        idempotency_key, document_id, version_id, attempts, max_attempts,
                        started_at, finished_at
                     ) VALUES (
                        $1,$2,'index','succeeded',1,$6::jsonb,$3,$4,$5,1,5,now(),now()
                     )",
                    &[
                        &job_id,
                        &org_id,
                        &format!("denial-job-{}", job_id.simple()),
                        &document_id,
                        &version_id,
                        &serde_json::json!({"marker": marker, "document_id": document_id}),
                    ],
                )
                .await?;

                let claim_low = Uuid::new_v4();
                let claim_high = Uuid::new_v4();
                let (claim_a, claim_b) = if claim_low < claim_high {
                    (claim_low, claim_high)
                } else {
                    (claim_high, claim_low)
                };
                txn.execute(
                    "INSERT INTO claims (
                        id, org_id, document_id, version_id, claim_key, subject, predicate,
                        value_type, value_money, unit, scope, effective_from, citation_quote
                     ) VALUES
                        ($1,$2,$3,$4,$5,$5,'is','money',15,'triệu','',now(),$5),
                        ($6,$2,$3,$4,$5,$5,'is','money',20,'triệu','',now(),$5)",
                    &[
                        &claim_a,
                        &org_id,
                        &document_id,
                        &version_id,
                        &marker,
                        &claim_b,
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
                        &org_id,
                        &claim_a,
                        &claim_b,
                        &version_id,
                    ],
                )
                .await?;

                Ok(())
            })
        }
    })
    .await
    .expect("seed org world");

    let owner_tokens = login_tokens(pool, &owner_email, PASSWORD).await;
    let admin_tokens = login_tokens(pool, &admin_email, PASSWORD).await;
    let member_tokens = login_tokens(pool, &member_email, PASSWORD).await;

    let mut users = BTreeMap::new();
    users.insert(
        "owner".into(),
        BootedUser {
            user_id: owner_id,
            email: owner_email,
            role: "owner".into(),
            access_token: owner_tokens.0,
            refresh_token: owner_tokens.1,
        },
    );
    users.insert(
        "admin".into(),
        BootedUser {
            user_id: admin_id,
            email: admin_email,
            role: "admin".into(),
            access_token: admin_tokens.0,
            refresh_token: admin_tokens.1,
        },
    );
    users.insert(
        "member".into(),
        BootedUser {
            user_id: member_id,
            email: member_email,
            role: "viewer".into(),
            access_token: member_tokens.0,
            refresh_token: member_tokens.1,
        },
    );

    let mut collections_map = BTreeMap::new();
    collections_map.insert(
        "private".into(),
        BootedCollection {
            collection_id: private_id,
            visibility: "private".into(),
            name: private_collection_name,
        },
    );
    collections_map.insert(
        "org".into(),
        BootedCollection {
            collection_id: org_collection_id,
            visibility: "org".into(),
            name: org_collection_name,
        },
    );
    collections_map.insert(
        "groups".into(),
        BootedCollection {
            collection_id: groups_id,
            visibility: "groups".into(),
            name: groups_collection_name,
        },
    );

    BootedOrg {
        org_id,
        slug,
        marker,
        object_key: object_key_str,
        users,
        collections: collections_map,
        indexed_document: None,
        document: BootedDocument {
            document_id,
            version_id,
            title: duplicate_document_title,
        },
        job_id,
        conflict_id,
    }
}

async fn insert_collection_row(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    id: Uuid,
    name: &str,
    slug_suffix: &str,
    visibility: CollectionVisibility,
) -> Result<(), fileconv_server::db::error::DbError> {
    collections::insert(
        txn,
        ctx,
        NewCollection {
            id,
            name,
            slug: &format!("denial-{slug_suffix}-{}", id.simple()),
            description: Some(name),
            visibility,
        },
    )
    .await?;
    Ok(())
}

async fn seed_admin_member(pool: &Pool, org: Uuid, user: Uuid, email: &str) {
    seed_user_with_permissions(pool, org, user, email, PASSWORD, &["member.manage"]).await;
    let ctx = OrgContext::try_new(org, user, ["member.manage"], []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "UPDATE org_memberships SET role = 'admin' WHERE org_id = $1 AND user_id = $2",
                &[&org, &user],
            )
            .await?;
            Ok(())
        })
    })
    .await
    .expect("promote admin membership");
}

async fn seed_viewer_member(pool: &Pool, org: Uuid, user: Uuid, email: &str) {
    // Fixture role label `member` maps to DB membership role `viewer`.
    seed_user_with_permissions(pool, org, user, email, PASSWORD, MEMBER_PERMISSIONS).await;
    let ctx = OrgContext::try_new(org, user, MEMBER_PERMISSIONS.iter().copied(), []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            txn.execute(
                "UPDATE org_memberships SET role = 'viewer' WHERE org_id = $1 AND user_id = $2",
                &[&org, &user],
            )
            .await?;
            Ok(())
        })
    })
    .await
    .expect("promote viewer membership");
}
