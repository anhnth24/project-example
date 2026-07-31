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
    (
        "0026_expand_audit_append_only.sql",
        include_str!("../migrations/0026_expand_audit_append_only.sql"),
    ),
    (
        "0027_expand_migrator_app_grants.sql",
        include_str!("../migrations/0027_expand_migrator_app_grants.sql"),
    ),
    (
        "0028_expand_audit_ownership_migrator.sql",
        include_str!("../migrations/0028_expand_audit_ownership_migrator.sql"),
    ),
    (
        "0029_expand_org_membership_state.sql",
        include_str!("../migrations/0029_expand_org_membership_state.sql"),
    ),
    (
        "0030_expand_global_role_catalog.sql",
        include_str!("../migrations/0030_expand_global_role_catalog.sql"),
    ),
    (
        "0031_expand_org_acl_version.sql",
        include_str!("../migrations/0031_expand_org_acl_version.sql"),
    ),
    (
        "0032_expand_projects.sql",
        include_str!("../migrations/0032_expand_projects.sql"),
    ),
    (
        "0033_expand_acl_version_triggers.sql",
        include_str!("../migrations/0033_expand_acl_version_triggers.sql"),
    ),
    (
        "0034_expand_qa_chat_history.sql",
        include_str!("../migrations/0034_expand_qa_chat_history.sql"),
    ),
    (
        "0035_expand_worker_role.sql",
        include_str!("../migrations/0035_expand_worker_role.sql"),
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
                    .map_err(|error| format!("cannot apply migration {name}: {error:?}"))?;
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
    fn role_catalog_is_global_and_immutable() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0030_expand_global_role_catalog.sql")
            .expect("global role catalog migration")
            .1;
        for table in ["role_catalog", "role_catalog_permissions"] {
            assert!(
                source.contains(&format!(
                    "BEFORE UPDATE OR DELETE ON {table}\n    FOR EACH ROW\n    EXECUTE FUNCTION role_catalog_enforce_immutability();"
                )),
                "{table} must have a row-level immutability trigger"
            );
            assert!(
                source.contains(&format!(
                    "BEFORE TRUNCATE ON {table}\n    FOR EACH STATEMENT\n    EXECUTE FUNCTION role_catalog_enforce_immutability();"
                )),
                "{table} must have a statement-level truncate guard"
            );
        }
        assert!(
            source.contains("CREATE OR REPLACE FUNCTION provision_org_role_catalog(p_org_id uuid)")
        );
    }

    #[test]
    fn worker_role_grants_are_guarded_scoped_and_append_only_audit() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0035_expand_worker_role.sql")
            .expect("worker role migration")
            .1;
        // Grants apply only when ops pre-provisioned the role (0027 pattern).
        assert!(
            source.contains("IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_worker')"),
            "worker grants must be guarded on role existence"
        );
        // Audit stays append-only for the worker, like markhand_app.
        assert!(source
            .contains("REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_worker"));
        assert!(source.contains("GRANT SELECT, INSERT ON TABLE audit_log TO markhand_worker"));
        // No schema mutation rights.
        assert!(source.contains("REVOKE CREATE ON SCHEMA public FROM markhand_worker"));
        // Least privilege: auth/ACL/chat/upload/capability tables are never
        // mentioned, so no grant can reach them.
        for denied in [
            "refresh_tokens",
            "org_memberships",
            "org_invites",
            "collection_user_access",
            "collection_group_access",
            "collection_role_access",
            "role_permissions",
            "qa_chat_sessions",
            "qa_chat_turns",
            "ask_stream_sessions",
            "upload_operations",
            "download_capability_redemptions",
        ] {
            assert!(
                !source.contains(denied),
                "worker migration must not touch {denied}"
            );
        }
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

    #[test]
    fn acl_groups_invariants_migration_shape() {
        let source = MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "0036_expand_acl_groups_invariants.sql")
            .expect("0036 ACL groups invariants migration")
            .1;

        assert!(
            source.contains("ORDER BY bad.id"),
            "preflight must report sorted collection IDs"
        );
        assert!(
            source.contains("preflight"),
            "migration must include a preflight guard for dormant grants"
        );
        assert!(
            source.contains("FOR NO KEY UPDATE"),
            "grant triggers must lock the parent collection row"
        );

        for table in ["collection_group_access", "collection_role_access"] {
            assert!(
                source.contains(&format!(
                    "BEFORE INSERT OR UPDATE ON {table}\n    FOR EACH ROW"
                )),
                "{table} must have a BEFORE INSERT OR UPDATE visibility guard"
            );
            assert!(
                source.contains(&format!(
                    "WHERE org_id = NEW.org_id AND id = NEW.collection_id\n    FOR NO KEY UPDATE"
                )) || source.contains(
                    "WHERE org_id = NEW.org_id AND id = NEW.collection_id FOR NO KEY UPDATE"
                ),
                "{table} grant trigger must lock parent by org_id + collection_id"
            );
        }

        assert!(
            source.contains("BEFORE UPDATE OF visibility ON collections"),
            "collections must guard visibility transitions away from groups"
        );
        assert!(
            source.contains("NEW.visibility <> 'groups'"),
            "visibility guard must fire when leaving groups"
        );

        for table in [
            "collection_group_access",
            "collection_role_access",
            "group_memberships",
        ] {
            assert!(
                source.contains(&format!(
                    "ON {table}\n    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version()"
                )),
                "{table} must bump org acl_version"
            );
        }
    }
}
