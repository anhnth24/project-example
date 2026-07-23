//! PostgreSQL connectivity and immutable migration application.

use rustls::{ClientConfig, RootCertStore};
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_expand_orgs_users.sql",
        include_str!("../migrations/0001_expand_orgs_users.sql"),
    ),
    (
        "0002_expand_org_membership_rls.sql",
        include_str!("../migrations/0002_expand_org_membership_rls.sql"),
    ),
    (
        "0003_expand_auth_sessions_rbac.sql",
        include_str!("../migrations/0003_expand_auth_sessions_rbac.sql"),
    ),
    (
        "0004_expand_collections.sql",
        include_str!("../migrations/0004_expand_collections.sql"),
    ),
    (
        "0005_expand_documents_versions_artifacts.sql",
        include_str!("../migrations/0005_expand_documents_versions_artifacts.sql"),
    ),
    (
        "0006_expand_chunks_claims.sql",
        include_str!("../migrations/0006_expand_chunks_claims.sql"),
    ),
    (
        "0007_expand_conflicts_lifecycle.sql",
        include_str!("../migrations/0007_expand_conflicts_lifecycle.sql"),
    ),
    (
        "0008_expand_jobs_outbox_events.sql",
        include_str!("../migrations/0008_expand_jobs_outbox_events.sql"),
    ),
    (
        "0009_expand_quota_audit_index.sql",
        include_str!("../migrations/0009_expand_quota_audit_index.sql"),
    ),
    (
        "0010_expand_tenant_rls.sql",
        include_str!("../migrations/0010_expand_tenant_rls.sql"),
    ),
    (
        "0011_expand_poc_seed.sql",
        include_str!("../migrations/0011_expand_poc_seed.sql"),
    ),
    (
        "0012_index_generation_embedding_batches.sql",
        include_str!("../migrations/0012_index_generation_embedding_batches.sql"),
    ),
    (
        "0013_expand_index_generation_rls.sql",
        include_str!("../migrations/0013_expand_index_generation_rls.sql"),
    ),
    (
        "0014_expand_vector_cleanup_intents.sql",
        include_str!("../migrations/0014_expand_vector_cleanup_intents.sql"),
    ),
    (
        "0015_expand_vector_cleanup_intent_states.sql",
        include_str!("../migrations/0015_expand_vector_cleanup_intent_states.sql"),
    ),
    (
        "0016_expand_chunks_accent_fold_tsv.sql",
        include_str!("../migrations/0016_expand_chunks_accent_fold_tsv.sql"),
    ),
    (
        "0017_expand_qa_history_permission.sql",
        include_str!("../migrations/0017_expand_qa_history_permission.sql"),
    ),
    (
        "0018_expand_download_capability_redemptions.sql",
        include_str!("../migrations/0018_expand_download_capability_redemptions.sql"),
    ),
    (
        "0019_expand_ops_fences_jobs_system.sql",
        include_str!("../migrations/0019_expand_ops_fences_jobs_system.sql"),
    ),
    (
        "0020_expand_hash_semantics_readiness_ops.sql",
        include_str!("../migrations/0020_expand_hash_semantics_readiness_ops.sql"),
    ),
    (
        "0021_expand_audit_intent_outcome.sql",
        include_str!("../migrations/0021_expand_audit_intent_outcome.sql"),
    ),
    (
        "0022_expand_lifecycle_refresh_job.sql",
        include_str!("../migrations/0022_expand_lifecycle_refresh_job.sql"),
    ),
    (
        "0023_expand_upload_operations.sql",
        include_str!("../migrations/0023_expand_upload_operations.sql"),
    ),
    (
        "0024_expand_ask_stream_sessions.sql",
        include_str!("../migrations/0024_expand_ask_stream_sessions.sql"),
    ),
    (
        "0025_backfill_event_log_ids_ask_stream_ops.sql",
        include_str!("../migrations/0025_backfill_event_log_ids_ask_stream_ops.sql"),
    ),
];

/// Embedded migration sources in apply order (name, SQL). Used by integration tests.
pub fn embedded_migrations() -> &'static [(&'static str, &'static str)] {
    MIGRATIONS
}

pub fn migration_checksum(source: &str) -> String {
    Sha256::digest(source.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub async fn apply_migrations(database_url: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
    client
        .batch_execute("SET lock_timeout = '5s'; SET statement_timeout = '30s';")
        .await
        .map_err(|error| format!("cannot configure migration timeouts: {error}"))?;
    client
        .query_one(
            "SELECT pg_advisory_lock(hashtext($1))",
            &[&"markhand_schema_migrations"],
        )
        .await
        .map_err(|error| format!("cannot acquire migration lock: {error}"))?;
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS markhand_schema_migrations (
                name text PRIMARY KEY,
                checksum text NOT NULL,
                applied_at timestamptz NOT NULL DEFAULT now()
            )",
        )
        .await
        .map_err(|error| format!("cannot initialize migration history: {error}"))?;

    let result = apply_all_migrations(&mut client).await;
    let unlock = client
        .query_one(
            "SELECT pg_advisory_unlock(hashtext($1))",
            &[&"markhand_schema_migrations"],
        )
        .await;
    if let Err(error) = unlock {
        return Err(format!("cannot release migration lock: {error}"));
    }
    result
}

async fn apply_all_migrations(client: &mut Client) -> Result<(), String> {
    for &(name, source) in MIGRATIONS {
        let checksum = migration_checksum(source);
        let prior = client
            .query_opt(
                "SELECT checksum FROM markhand_schema_migrations WHERE name = $1",
                &[&name],
            )
            .await
            .map_err(|error| format!("cannot inspect migration history: {error}"))?;

        match prior {
            Some(row) if row.get::<_, String>(0) == checksum => {}
            Some(_) => return Err(format!("migration checksum mismatch for {name}")),
            None => {
                let transaction = client
                    .transaction()
                    .await
                    .map_err(|error| format!("cannot start migration transaction: {error}"))?;
                transaction
                    .batch_execute(source)
                    .await
                    .map_err(|error| format!("cannot apply migration {name}: {error}"))?;
                transaction
                    .execute(
                        "INSERT INTO markhand_schema_migrations (name, checksum) VALUES ($1, $2)",
                        &[&name, &checksum],
                    )
                    .await
                    .map_err(|error| format!("cannot record migration {name}: {error}"))?;
                transaction
                    .commit()
                    .await
                    .map_err(|error| format!("cannot commit migration {name}: {error}"))?;
            }
        }
    }
    Ok(())
}

pub async fn check_connection(database_url: &str) -> Result<(), String> {
    let client = connect(database_url).await?;
    client
        .simple_query("SELECT 1")
        .await
        .map_err(|error| format!("PostgreSQL query failed: {error}"))?;
    Ok(())
}

async fn connect(database_url: &str) -> Result<Client, String> {
    if database_requires_tls(database_url)? {
        return connect_with_tls(database_url).await;
    }
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .map_err(|error| format!("PostgreSQL connection failed: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

async fn connect_with_tls(database_url: &str) -> Result<Client, String> {
    let connector = make_rustls_connect()?;
    let (client, connection) = tokio_postgres::connect(database_url, connector)
        .await
        .map_err(|error| format!("PostgreSQL connection failed: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

/// Whether the URL requests TLS (`sslmode` present and not `disable`).
pub fn database_requires_tls(database_url: &str) -> Result<bool, String> {
    let parsed = reqwest::Url::parse(database_url)
        .map_err(|_| "MARKHAND_DATABASE_URL must be an absolute URL".to_string())?;
    Ok(parsed
        .query_pairs()
        .any(|(key, value)| key == "sslmode" && value != "disable"))
}

/// Builds a rustls connector for tokio-postgres / deadpool-postgres.
pub fn make_rustls_connect() -> Result<MakeRustlsConnect, String> {
    Ok(MakeRustlsConnect::new(tls_config()?))
}

fn tls_config() -> Result<ClientConfig, String> {
    let certificates = rustls_native_certs::load_native_certs();
    if !certificates.errors.is_empty() {
        return Err("cannot load native PostgreSQL root certificates".into());
    }
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(certificates.certs);
    if roots.is_empty() {
        return Err("no native PostgreSQL root certificates are available".into());
    }
    Ok(ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::MIGRATIONS;

    #[test]
    fn embedded_migrations_match_the_immutable_manifest() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../migrations/manifest.json")).unwrap();
        let manifest_names = manifest["migrations"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let embedded_names = MIGRATIONS
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(embedded_names, manifest_names);
    }

    #[test]
    fn generation_tables_have_mandatory_tenant_rls() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0013_expand_index_generation_rls.sql")
            .expect("generation RLS migration")
            .1;
        for table in ["index_generation_backfills", "embedding_batches"] {
            assert!(
                source.contains(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;")),
                "{table} must enable RLS"
            );
            assert!(
                source.contains(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY;")),
                "{table} must force RLS"
            );
            assert!(
                source.contains(&format!("CREATE POLICY {table}_org_isolation ON {table}")),
                "{table} must have an org-isolation policy"
            );
        }
    }

    #[test]
    fn vector_cleanup_intents_have_mandatory_tenant_rls() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0014_expand_vector_cleanup_intents.sql")
            .expect("vector cleanup intents migration")
            .1;
        let table = "vector_cleanup_intents";
        assert!(source.contains(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;")));
        assert!(source.contains(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY;")));
        assert!(source.contains(&format!("CREATE POLICY {table}_org_isolation ON {table}")));
    }

    #[test]
    fn vector_cleanup_intent_states_expand_to_cas_lifecycle() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0015_expand_vector_cleanup_intent_states.sql")
            .expect("intent state migration")
            .1;
        assert!(source.contains("'pending', 'writing', 'cleaned', 'committed'"));
        assert!(source.contains("status = 'committed'"));
    }
}
