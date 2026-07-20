//! Storage adapter integration tests (Qdrant + MinIO).
//!
//! Live-service tests skip cleanly when `MARKHAND_TEST_QDRANT_URL` /
//! `MARKHAND_TEST_MINIO_*` are unset. The missing-scope test does **not**
//! require live services (unreachable endpoint proves no network call).

use std::collections::BTreeSet;

use bytes::Bytes;
use fileconv_knowledge::identity::{
    chunk_identity, IndexSignature, BODY_TEXT_VERSION, DEFAULT_CHUNKING_VERSION,
    QUERY_NORMALIZATION_VERSION, RUNTIME_LOCAL_HASH,
};
use fileconv_server::config::{MinioConfig, SecretString};
use fileconv_server::services::index_signature::parse_collection_name;
use fileconv_server::storage::keys::{parse_key_for_org, quarantine_key, trusted_key};
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantAdminApiKey,
    QdrantAdminClient, QdrantClient, UpsertPoint, VectorScope,
};
use fileconv_server::storage::StorageError;
use uuid::Uuid;

fn test_qdrant_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_QDRANT_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_QDRANT_URL unset — Qdrant integration tests require a live instance"
            );
            None
        }
    }
}

fn test_admin_client(url: &str) -> QdrantAdminClient {
    // Local Qdrant ignores api-key when auth is disabled; construction still
    // requires a distinct non-empty operator credential.
    let key = std::env::var("MARKHAND_TEST_QDRANT_ADMIN_API_KEY")
        .unwrap_or_else(|_| "test-operator-admin-key".into());
    QdrantAdminClient::new(url, QdrantAdminApiKey::new(SecretString::new(key)).unwrap())
        .expect("admin client")
}

struct TestMinioEnv {
    endpoint: String,
    access_key: String,
    secret_key: String,
    region: String,
}

fn test_minio_env() -> Option<TestMinioEnv> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_MINIO_ENDPOINT unset — MinIO integration tests require a live instance"
            );
            return None;
        }
    };
    let access_key = std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY").ok()?;
    let secret_key = std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY").ok()?;
    if access_key.is_empty() || secret_key.is_empty() {
        eprintln!("skipped: MinIO test credentials empty");
        return None;
    }
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION").unwrap_or_else(|_| "us-east-1".into());
    Some(TestMinioEnv {
        endpoint,
        access_key,
        secret_key,
        region,
    })
}

fn unit_vector(dimensions: usize, hot: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimensions];
    if dimensions > 0 {
        vector[hot % dimensions] = 1.0;
    }
    vector
}

/// Missing scope must fail closed **without** any network I/O.
#[tokio::test]
async fn missing_scope_rejects_without_network_side_effects() {
    let client = QdrantClient::new("http://127.0.0.1:1").expect("client builds");
    let collection = parse_collection_name(
        "markhand_chunks_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    )
    .unwrap();
    let empty_org = VectorScope::new(Uuid::nil(), [Uuid::new_v4()]);
    let empty_collections = VectorScope::new(Uuid::new_v4(), BTreeSet::new());
    let vector = unit_vector(8, 0);
    let org = Uuid::new_v4();
    let collection_id = Uuid::new_v4();
    let payload = ChunkPointPayload {
        org_id: org,
        collection_id,
        document_id: Uuid::new_v4(),
        version_id: Uuid::new_v4(),
        chunk_id: "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".into(),
        ordinal: 0,
        is_current: true,
        is_effective: true,
        index_generation: 1,
    };
    let points = [UpsertPoint {
        chunk_identity: payload.chunk_id.clone(),
        vector: vector.clone(),
        payload,
    }];

    assert!(matches!(
        client.upsert_points(&collection, &empty_org, &points).await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client
            .upsert_points(&collection, &empty_collections, &points)
            .await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client.search(&collection, &empty_org, &vector, 5).await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client
            .search(&collection, &empty_collections, &vector, 5)
            .await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client.delete_by_scope(&collection, &empty_org, &[]).await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client
            .delete_by_scope(&collection, &empty_collections, &[])
            .await,
        Err(StorageError::MissingScope)
    ));
    assert!(matches!(
        client.get_points(&collection, &empty_org, &[]).await,
        Err(StorageError::MissingScope)
    ));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_QDRANT_URL"]
async fn qdrant_tenant_isolation_and_deterministic_points() {
    let Some(url) = test_qdrant_url() else {
        return;
    };
    let client = QdrantClient::new(&url).expect("qdrant client");
    let admin = test_admin_client(&url);
    let unique_family = format!("storage-itest-{}", Uuid::new_v4().simple());
    let signature = IndexSignature {
        runtime_path: RUNTIME_LOCAL_HASH,
        embedding_family: &unique_family,
        embedding_revision: "r1",
        dimensions: 8,
        normalized: true,
        chunking_version: DEFAULT_CHUNKING_VERSION,
        body_text_version: BODY_TEXT_VERSION,
        query_normalization_version: QUERY_NORMALIZATION_VERSION,
    };
    let digest = signature.digest();
    let collection = client
        .ensure_collection_for_digest(&digest, signature.dimensions, signature.normalized)
        .await
        .expect("ensure collection");
    assert!(collection.as_str().contains(&digest));

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let collection_a = Uuid::new_v4();
    let collection_b = Uuid::new_v4();
    let document_a = Uuid::new_v4();
    let version_a = Uuid::new_v4();
    let document_b = Uuid::new_v4();
    let version_b = Uuid::new_v4();

    let chunk_a = chunk_identity(
        &document_a.to_string(),
        &version_a.to_string(),
        0,
        "Chương I",
        "Nội dung org A",
        BODY_TEXT_VERSION,
    );
    let chunk_b = chunk_identity(
        &document_b.to_string(),
        &version_b.to_string(),
        0,
        "Chương I",
        "Nội dung org B",
        BODY_TEXT_VERSION,
    );

    let scope_a = VectorScope::new(org_a, [collection_a]);
    let scope_b = VectorScope::new(org_b, [collection_b]);

    let point_a = UpsertPoint {
        chunk_identity: chunk_a.clone(),
        vector: unit_vector(8, 0),
        payload: ChunkPointPayload {
            org_id: org_a,
            collection_id: collection_a,
            document_id: document_a,
            version_id: version_a,
            chunk_id: chunk_a.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    let point_b = UpsertPoint {
        chunk_identity: chunk_b.clone(),
        vector: unit_vector(8, 1),
        payload: ChunkPointPayload {
            org_id: org_b,
            collection_id: collection_b,
            document_id: document_b,
            version_id: version_b,
            chunk_id: chunk_b.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };

    let ids_a1 = client
        .upsert_points(&collection, &scope_a, &[point_a.clone()])
        .await
        .expect("upsert A");
    let ids_a2 = client
        .upsert_points(&collection, &scope_a, &[point_a.clone()])
        .await
        .expect("idempotent upsert A");
    assert_eq!(ids_a1, ids_a2);
    assert_eq!(
        ids_a1[0],
        point_id_from_org_collection_and_chunk(org_a, collection_a, &chunk_a).expect("point id")
    );

    client
        .upsert_points(&collection, &scope_b, &[point_b])
        .await
        .expect("upsert B");

    let fetched = client
        .get_points(&collection, &scope_a, &ids_a1)
        .await
        .expect("get points");
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].1.org_id, org_a);
    assert_eq!(fetched[0].1.collection_id, collection_a);

    let hits_a = client
        .search(&collection, &scope_a, &unit_vector(8, 0), 10)
        .await
        .expect("search A");
    assert!(hits_a.iter().all(|hit| hit.payload.org_id == org_a));
    assert!(hits_a.iter().any(|hit| hit.payload.chunk_id == chunk_a));

    client
        .delete_by_scope(&collection, &scope_a, &[])
        .await
        .expect("delete A");
    assert!(client
        .search(&collection, &scope_a, &unit_vector(8, 0), 10)
        .await
        .unwrap()
        .is_empty());
    assert!(client
        .search(&collection, &scope_b, &unit_vector(8, 1), 10)
        .await
        .unwrap()
        .iter()
        .any(|hit| hit.payload.chunk_id == chunk_b));

    admin
        .delete_collection(&collection)
        .await
        .expect("admin cleanup");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_QDRANT_URL"]
async fn cross_org_point_overwrite_rejected() {
    let Some(url) = test_qdrant_url() else {
        return;
    };
    let client = QdrantClient::new(&url).expect("qdrant client");
    let admin = test_admin_client(&url);
    let unique_family = format!("storage-xorg-{}", Uuid::new_v4().simple());
    let signature = IndexSignature {
        runtime_path: RUNTIME_LOCAL_HASH,
        embedding_family: &unique_family,
        embedding_revision: "r1",
        dimensions: 8,
        normalized: true,
        chunking_version: DEFAULT_CHUNKING_VERSION,
        body_text_version: BODY_TEXT_VERSION,
        query_normalization_version: QUERY_NORMALIZATION_VERSION,
    };
    let digest = signature.digest();
    let collection = client
        .ensure_collection_for_digest(&digest, 8, true)
        .await
        .expect("ensure");

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let col_a = Uuid::new_v4();
    let col_b = Uuid::new_v4();
    let doc = Uuid::new_v4();
    let version = Uuid::new_v4();
    let chunk = chunk_identity(
        &doc.to_string(),
        &version.to_string(),
        0,
        "H",
        "shared body",
        BODY_TEXT_VERSION,
    );

    let scope_a = VectorScope::new(org_a, [col_a]);
    let scope_b = VectorScope::new(org_b, [col_b]);

    let point_a = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector: unit_vector(8, 0),
        payload: ChunkPointPayload {
            org_id: org_a,
            collection_id: col_a,
            document_id: doc,
            version_id: version,
            chunk_id: chunk.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    let ids_a = client
        .upsert_points(&collection, &scope_a, &[point_a])
        .await
        .expect("upsert A");

    let id_a = point_id_from_org_collection_and_chunk(org_a, col_a, &chunk).unwrap();
    let id_b = point_id_from_org_collection_and_chunk(org_b, col_b, &chunk).unwrap();
    assert_ne!(id_a, id_b);
    assert_eq!(ids_a[0], id_a);

    let forged = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector: unit_vector(8, 7),
        payload: ChunkPointPayload {
            org_id: org_a,
            collection_id: col_a,
            document_id: doc,
            version_id: version,
            chunk_id: chunk.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    assert!(matches!(
        client.upsert_points(&collection, &scope_b, &[forged]).await,
        Err(StorageError::MissingScope)
    ));

    let point_b = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector: unit_vector(8, 1),
        payload: ChunkPointPayload {
            org_id: org_b,
            collection_id: col_b,
            document_id: doc,
            version_id: version,
            chunk_id: chunk.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    client
        .upsert_points(&collection, &scope_b, &[point_b])
        .await
        .expect("upsert B");

    let still_a = client
        .get_points(&collection, &scope_a, &ids_a)
        .await
        .expect("get A");
    assert_eq!(still_a.len(), 1);
    assert_eq!(still_a[0].1.org_id, org_a);

    admin.delete_collection(&collection).await.ok();
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_QDRANT_URL"]
async fn same_org_different_collection_cannot_overwrite() {
    let Some(url) = test_qdrant_url() else {
        return;
    };
    let client = QdrantClient::new(&url).expect("qdrant client");
    let admin = test_admin_client(&url);
    let unique_family = format!("storage-xcol-{}", Uuid::new_v4().simple());
    let signature = IndexSignature {
        runtime_path: RUNTIME_LOCAL_HASH,
        embedding_family: &unique_family,
        embedding_revision: "r1",
        dimensions: 8,
        normalized: true,
        chunking_version: DEFAULT_CHUNKING_VERSION,
        body_text_version: BODY_TEXT_VERSION,
        query_normalization_version: QUERY_NORMALIZATION_VERSION,
    };
    let collection = client
        .ensure_collection_for_digest(&signature.digest(), 8, true)
        .await
        .expect("ensure");

    let org = Uuid::new_v4();
    let col_a = Uuid::new_v4();
    let col_b = Uuid::new_v4();
    let doc = Uuid::new_v4();
    let version = Uuid::new_v4();
    let chunk = chunk_identity(
        &doc.to_string(),
        &version.to_string(),
        0,
        "H",
        "same org shared chunk",
        BODY_TEXT_VERSION,
    );

    let id_a = point_id_from_org_collection_and_chunk(org, col_a, &chunk).unwrap();
    let id_b = point_id_from_org_collection_and_chunk(org, col_b, &chunk).unwrap();
    assert_ne!(
        id_a, id_b,
        "same org + same chunk must still differ by collection_id"
    );

    let scope_a = VectorScope::new(org, [col_a]);
    let scope_b = VectorScope::new(org, [col_b]);
    // Unauthorized: org has col_a in scope only — cannot write col_b payload.
    let scope_a_only = VectorScope::new(org, [col_a]);

    let point_a = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector: unit_vector(8, 0),
        payload: ChunkPointPayload {
            org_id: org,
            collection_id: col_a,
            document_id: doc,
            version_id: version,
            chunk_id: chunk.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    let ids_a = client
        .upsert_points(&collection, &scope_a, &[point_a])
        .await
        .expect("upsert A");
    assert_eq!(ids_a[0], id_a);

    // Attempt to write col_b payload under scope that only authorizes col_a.
    let forged = UpsertPoint {
        chunk_identity: chunk.clone(),
        vector: unit_vector(8, 7),
        payload: ChunkPointPayload {
            org_id: org,
            collection_id: col_b,
            document_id: doc,
            version_id: version,
            chunk_id: chunk.clone(),
            ordinal: 0,
            is_current: true,
            is_effective: true,
            index_generation: 1,
        },
    };
    assert!(matches!(
        client
            .upsert_points(&collection, &scope_a_only, &[forged.clone()])
            .await,
        Err(StorageError::MissingScope)
    ));

    // Legitimate col_b upsert uses a different point id; col_a point unchanged.
    client
        .upsert_points(&collection, &scope_b, &[forged])
        .await
        .expect("upsert B");
    let still_a = client
        .get_points(&collection, &scope_a, &ids_a)
        .await
        .expect("get A");
    assert_eq!(still_a.len(), 1);
    assert_eq!(still_a[0].1.collection_id, col_a);
    let hits_a = client
        .search(&collection, &scope_a, &unit_vector(8, 0), 5)
        .await
        .unwrap();
    assert!(hits_a.iter().all(|h| h.payload.collection_id == col_a));
    assert!(hits_a.iter().all(|h| h.point_id != id_b));

    admin.delete_collection(&collection).await.ok();
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_QDRANT_URL"]
async fn existing_collection_dimension_mismatch_rejected() {
    let Some(url) = test_qdrant_url() else {
        return;
    };
    let client = QdrantClient::new(&url).expect("qdrant client");
    let admin = test_admin_client(&url);
    let unique_family = format!("storage-mismatch-{}", Uuid::new_v4().simple());
    let signature = IndexSignature {
        runtime_path: RUNTIME_LOCAL_HASH,
        embedding_family: &unique_family,
        embedding_revision: "r1",
        dimensions: 8,
        normalized: true,
        chunking_version: DEFAULT_CHUNKING_VERSION,
        body_text_version: BODY_TEXT_VERSION,
        query_normalization_version: QUERY_NORMALIZATION_VERSION,
    };
    let digest = signature.digest();
    let collection = client
        .ensure_collection_for_digest(&digest, 8, true)
        .await
        .expect("create 8-dim cosine");

    assert!(matches!(
        client.ensure_collection_for_digest(&digest, 16, true).await,
        Err(StorageError::CollectionMismatch)
    ));
    assert!(matches!(
        client.ensure_collection_for_digest(&digest, 8, false).await,
        Err(StorageError::CollectionMismatch)
    ));
    client
        .ensure_collection_for_digest(&digest, 8, true)
        .await
        .expect("idempotent ensure");

    admin.delete_collection(&collection).await.ok();
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn minio_put_exists_get_delete_round_trip() {
    let Some(env) = test_minio_env() else {
        return;
    };
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");

    let bucket_name = format!("markhand-storage-it-{}", Uuid::new_v4().simple());
    let config = MinioConfig::new(
        env.endpoint,
        SecretString::new(env.access_key),
        SecretString::new(env.secret_key),
        bucket_name,
        env.region,
        true,
    )
    .expect("minio config");
    let debug = format!("{config:?}");
    assert!(!debug.contains("minioadmin"));
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("127.0.0.1"));

    let client = MinioClient::from_config(&config).expect("minio client");
    client.ensure_bucket().await.expect("ensure bucket");

    let org = Uuid::new_v4();
    let version = Uuid::new_v4();
    let object = Uuid::new_v4();
    let key = quarantine_key(org, object, Some("../../etc/passwd")).unwrap();
    assert_eq!(parse_key_for_org(&key.as_str(), org).unwrap(), key);

    let meta = ObjectIdentityMeta {
        org_id: org,
        collection_id: Some(Uuid::new_v4()),
        document_id: Some(Uuid::new_v4()),
        version_id: Some(version),
        original_filename: Some("../../etc/passwd".into()),
        canonical_format: None,
        content_sha256: None,
        content_length: None,
        disposition: None,
    };
    let body = Bytes::from_static(b"markhand-storage-bytes");
    client
        .put_object(org, &key, body.clone(), &meta, "application/octet-stream")
        .await
        .expect("put");
    assert!(client.object_exists(org, &key).await.expect("exists"));
    assert_eq!(client.get_object(org, &key).await.expect("get"), body);

    client.delete_object(org, &key).await.expect("delete");
    assert!(!client
        .object_exists(org, &key)
        .await
        .expect("exists after delete"));
    assert!(matches!(
        client.get_object(org, &key).await,
        Err(StorageError::NotFound)
    ));

    let trusted = trusted_key(org, version, Uuid::new_v4(), Some("/abs/evil.pdf")).unwrap();
    client
        .put_object(
            org,
            &trusted,
            Bytes::from_static(b"trusted"),
            &meta,
            "text/markdown",
        )
        .await
        .expect("put trusted");
    client
        .delete_object(org, &trusted)
        .await
        .expect("delete trusted");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_MINIO_*"]
async fn cross_org_object_key_operation_rejected() {
    let Some(env) = test_minio_env() else {
        return;
    };
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");

    let config = MinioConfig::new(
        env.endpoint,
        SecretString::new(env.access_key),
        SecretString::new(env.secret_key),
        format!("markhand-xorg-{}", Uuid::new_v4().simple()),
        env.region,
        true,
    )
    .expect("minio config");
    let client = MinioClient::from_config(&config).expect("client");
    client.ensure_bucket().await.expect("bucket");

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let key_a = quarantine_key(org_a, Uuid::new_v4(), None).unwrap();
    let meta_a = ObjectIdentityMeta {
        org_id: org_a,
        collection_id: None,
        document_id: None,
        version_id: None,
        original_filename: None,
        canonical_format: None,
        content_sha256: None,
        content_length: None,
        disposition: None,
    };
    client
        .put_object(
            org_a,
            &key_a,
            Bytes::from_static(b"secret-a"),
            &meta_a,
            "application/octet-stream",
        )
        .await
        .expect("put A");

    assert!(matches!(
        parse_key_for_org(&key_a.as_str(), org_b),
        Err(StorageError::KeyOrgMismatch)
    ));
    assert!(matches!(
        client.get_object(org_b, &key_a).await,
        Err(StorageError::KeyOrgMismatch)
    ));
    assert!(matches!(
        client.delete_object(org_b, &key_a).await,
        Err(StorageError::KeyOrgMismatch)
    ));
    assert!(matches!(
        client.object_exists(org_b, &key_a).await,
        Err(StorageError::KeyOrgMismatch)
    ));
    let forged_meta = ObjectIdentityMeta {
        org_id: org_b,
        collection_id: None,
        document_id: None,
        version_id: None,
        original_filename: None,
        canonical_format: None,
        content_sha256: None,
        content_length: None,
        disposition: None,
    };
    assert!(matches!(
        client
            .put_object(
                org_b,
                &key_a,
                Bytes::from_static(b"evil"),
                &forged_meta,
                "application/octet-stream",
            )
            .await,
        Err(StorageError::KeyOrgMismatch)
    ));

    assert_eq!(
        client
            .get_object(org_a, &key_a)
            .await
            .expect("get A")
            .as_ref(),
        b"secret-a"
    );
    client.delete_object(org_a, &key_a).await.expect("cleanup");
}

#[test]
fn object_keys_reject_traversal_and_omit_filenames() {
    let org = Uuid::new_v4();
    let version = Uuid::new_v4();
    let object = Uuid::new_v4();
    for name in [
        "../../etc/passwd",
        "/absolute/path",
        "C:\\Windows\\system32",
        "file\nname.txt",
        "unicode-файл.docx",
    ] {
        let q = quarantine_key(org, object, Some(name)).unwrap();
        let t = trusted_key(org, version, object, Some(name)).unwrap();
        assert!(!q.as_str().contains(name));
        parse_key_for_org(&q.as_str(), org).unwrap();
        parse_key_for_org(&t.as_str(), org).unwrap();
    }
    assert!(parse_key_for_org("quarantine/../aa/bb", org).is_err());
}

#[test]
fn collection_name_rejects_path_injection() {
    assert!(parse_collection_name("markhand_chunks_../passwd").is_err());
    assert!(parse_collection_name(&format!("markhand_chunks_{}?x=1", "a".repeat(64))).is_err());
    assert!(parse_collection_name(&format!("markhand_chunks_{}#x", "a".repeat(64))).is_err());
}
