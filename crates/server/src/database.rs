//! PostgreSQL connectivity and immutable migration application.

use rustls::{ClientConfig, RootCertStore};
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

const MIGRATIONS: [(&str, &str); 2] = [
    (
        "0001_expand_orgs_users.sql",
        include_str!("../migrations/0001_expand_orgs_users.sql"),
    ),
    (
        "0002_expand_org_membership_rls.sql",
        include_str!("../migrations/0002_expand_org_membership_rls.sql"),
    ),
];

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
    for (name, source) in MIGRATIONS {
        let checksum = Sha256::digest(source.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
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
    let connector = MakeRustlsConnect::new(tls_config()?);
    let (client, connection) = tokio_postgres::connect(database_url, connector)
        .await
        .map_err(|error| format!("PostgreSQL connection failed: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

fn database_requires_tls(database_url: &str) -> Result<bool, String> {
    let parsed = reqwest::Url::parse(database_url)
        .map_err(|_| "MARKHAND_DATABASE_URL must be an absolute URL".to_string())?;
    Ok(parsed
        .query_pairs()
        .any(|(key, value)| key == "sslmode" && value != "disable"))
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
}
