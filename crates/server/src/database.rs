//! PostgreSQL connectivity and immutable migration application.

use sha2::{Digest, Sha256};
use tokio_postgres::{Client, NoTls};

const MIGRATIONS: [(&str, &str); 1] = [(
    "0001_expand_orgs_users.sql",
    include_str!("../migrations/0001_expand_orgs_users.sql"),
)];

pub async fn apply_migrations(database_url: &str) -> Result<(), String> {
    let mut client = connect(database_url).await?;
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

    for (name, source) in MIGRATIONS {
        let checksum = format!("{:x}", Sha256::digest(source.as_bytes()));
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
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .map_err(|error| format!("PostgreSQL connection failed: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}
