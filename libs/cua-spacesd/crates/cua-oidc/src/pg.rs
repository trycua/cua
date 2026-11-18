// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Postgres [`Registry`] (tokio-postgres), with the crate's migrations
//! (`migrations/*.sql`) applied at start under an advisory lock, so
//! replicas starting together migrate once.
//!
//! TLS: with a CA file (`CUA_OIDC_DATABASE_CA_FILE`, e.g. a CloudNativePG
//! cluster's `ca.crt`) connections use TLS verified against that CA, and
//! the URL's `sslmode` decides whether it is required. Without one they
//! are plain TCP (an in-cluster network policy is then the boundary).

use std::sync::Arc;

use async_trait::async_trait;
use tokio_postgres::Client;

use crate::model::{AuditRecord, Connection, SealedToken};
use crate::registry::{Ledger, Registry, StoreError};

/// The migrations, in order. Never edit an applied one: add a new file.
pub const MIGRATIONS: &[(i32, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_spend.sql")),
];

/// Advisory lock key of the migration run (`cua-oidc` in ASCII).
const MIGRATION_LOCK: i64 = 0x6375_612d_6f69_6463;

enum Tls {
    None,
    Native(postgres_native_tls::MakeTlsConnector),
}

/// Connections, audit log and replay cache in Postgres.
pub struct PgRegistry {
    url: String,
    tls: Tls,
    client: tokio::sync::RwLock<Option<Arc<Client>>>,
}

impl std::fmt::Debug for PgRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgRegistry(..)")
    }
}

fn unavailable(e: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable(e.to_string())
}

impl PgRegistry {
    /// Connects to `url` (TLS verified against `ca_pem` when given) and
    /// applies pending migrations.
    pub async fn connect(url: &str, ca_pem: Option<&[u8]>) -> Result<Self, StoreError> {
        let tls = match ca_pem {
            Some(pem) => {
                let ca = native_tls::Certificate::from_pem(pem).map_err(unavailable)?;
                let connector = native_tls::TlsConnector::builder()
                    .add_root_certificate(ca)
                    .build()
                    .map_err(unavailable)?;
                Tls::Native(postgres_native_tls::MakeTlsConnector::new(connector))
            }
            None => Tls::None,
        };
        let registry = Self {
            url: url.to_owned(),
            tls,
            client: tokio::sync::RwLock::new(None),
        };
        let mut migrator = registry.open().await?;
        migrate(&mut migrator).await?;
        Ok(registry)
    }

    async fn open(&self) -> Result<Client, StoreError> {
        match &self.tls {
            Tls::None => {
                let (client, conn) = tokio_postgres::connect(&self.url, tokio_postgres::NoTls)
                    .await
                    .map_err(unavailable)?;
                tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        tracing::warn!(%e, "database connection ended");
                    }
                });
                Ok(client)
            }
            Tls::Native(tls) => {
                let (client, conn) = tokio_postgres::connect(&self.url, tls.clone())
                    .await
                    .map_err(unavailable)?;
                tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        tracing::warn!(%e, "database connection ended");
                    }
                });
                Ok(client)
            }
        }
    }

    /// The shared client, reconnecting when it closed. tokio-postgres
    /// pipelines concurrent queries on one connection.
    async fn client(&self) -> Result<Arc<Client>, StoreError> {
        if let Some(c) = self.client.read().await.as_ref() {
            if !c.is_closed() {
                return Ok(c.clone());
            }
        }
        let mut slot = self.client.write().await;
        if let Some(c) = slot.as_ref() {
            if !c.is_closed() {
                return Ok(c.clone());
            }
        }
        let c = Arc::new(self.open().await?);
        *slot = Some(c.clone());
        Ok(c)
    }

    /// Whether the database answers (readiness).
    pub async fn ping(&self) -> Result<(), StoreError> {
        self.client()
            .await?
            .simple_query("SELECT 1")
            .await
            .map(|_| ())
            .map_err(unavailable)
    }
}

async fn migrate(client: &mut Client) -> Result<(), StoreError> {
    let tx = client.transaction().await.map_err(unavailable)?;
    // Replicas starting together queue here; the first one migrates.
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK])
        .await
        .map_err(unavailable)?;
    tx.batch_execute(
        "CREATE TABLE IF NOT EXISTS oidc_schema_migrations (
            version integer PRIMARY KEY,
            applied_at timestamptz NOT NULL DEFAULT now()
        )",
    )
    .await
    .map_err(unavailable)?;
    for (version, sql) in MIGRATIONS {
        let applied = tx
            .query_opt(
                "SELECT 1 FROM oidc_schema_migrations WHERE version = $1",
                &[version],
            )
            .await
            .map_err(unavailable)?
            .is_some();
        if applied {
            continue;
        }
        tx.batch_execute(sql).await.map_err(unavailable)?;
        tx.execute(
            "INSERT INTO oidc_schema_migrations (version) VALUES ($1)",
            &[version],
        )
        .await
        .map_err(unavailable)?;
        tracing::info!(version, "applied database migration");
    }
    tx.commit().await.map_err(unavailable)
}

fn record_json(c: &Connection) -> Result<String, StoreError> {
    let mut plain = c.clone();
    plain.sealed_token = None;
    serde_json::to_string(&plain).map_err(unavailable)
}

fn from_row(row: &tokio_postgres::Row) -> Result<Connection, StoreError> {
    let record: String = row.get("record");
    let mut c: Connection = serde_json::from_str(&record).map_err(unavailable)?;
    c.version = row.get::<_, i64>("version") as u64;
    let ciphertext: Option<Vec<u8>> = row.get("sealed_ciphertext");
    let generation: Option<i64> = row.get("sealed_generation");
    c.sealed_token = match (ciphertext, generation) {
        (Some(ciphertext), Some(generation)) => Some(SealedToken {
            ciphertext,
            generation: generation as u64,
        }),
        _ => None,
    };
    Ok(c)
}

const SELECT: &str = "SELECT record::text AS record, version, sealed_ciphertext, sealed_generation FROM oidc_connections";

fn is_unique_violation(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
}

#[async_trait]
impl Registry for PgRegistry {
    async fn create(&self, c: &Connection) -> Result<(), StoreError> {
        let client = self.client().await?;
        let record = record_json(c)?;
        let sealed = c.sealed_token.as_ref();
        let result = client
            .execute(
                "INSERT INTO oidc_connections (id, account, cloud, primary_id, status, version, record, sealed_ciphertext, sealed_generation, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7::text::jsonb, $8, $9, $10, $11)",
                &[
                    &c.id,
                    &c.account,
                    &c.cloud.as_str(),
                    &c.primary_id(),
                    &c.status.as_str(),
                    &(c.version as i64),
                    &record,
                    &sealed.map(|s| s.ciphertext.as_slice()),
                    &sealed.map(|s| s.generation as i64),
                    &(c.created_at as i64),
                    &(c.updated_at as i64),
                ],
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => {
                let row = client
                    .query_opt(
                        "SELECT id FROM oidc_connections WHERE account = $1 AND cloud = $2 AND primary_id = $3 AND status <> 'revoked'",
                        &[&c.account, &c.cloud.as_str(), &c.primary_id()],
                    )
                    .await
                    .map_err(unavailable)?;
                match row {
                    Some(row) => Err(StoreError::Exists(row.get(0))),
                    None => Err(StoreError::Conflict),
                }
            }
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn list(&self, account: &str) -> Result<Vec<Connection>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(
                &format!("{SELECT} WHERE account = $1 ORDER BY created_at, id"),
                &[&account],
            )
            .await
            .map_err(unavailable)?;
        rows.iter().map(from_row).collect()
    }

    async fn get(&self, account: &str, id: &str) -> Result<Option<Connection>, StoreError> {
        let row = self
            .client()
            .await?
            .query_opt(
                &format!("{SELECT} WHERE account = $1 AND id = $2"),
                &[&account, &id],
            )
            .await
            .map_err(unavailable)?;
        row.as_ref().map(from_row).transpose()
    }

    async fn update(&self, c: &Connection) -> Result<(), StoreError> {
        let client = self.client().await?;
        let record = record_json(c)?;
        let sealed = c.sealed_token.as_ref();
        let result = client
            .execute(
                "UPDATE oidc_connections SET status = $3, version = $4::bigint, record = $5::text::jsonb,
                     sealed_ciphertext = $6, sealed_generation = $7, updated_at = $8
                 WHERE account = $1 AND id = $2 AND version = $4::bigint - 1",
                &[
                    &c.account,
                    &c.id,
                    &c.status.as_str(),
                    &(c.version as i64),
                    &record,
                    &sealed.map(|s| s.ciphertext.as_slice()),
                    &sealed.map(|s| s.generation as i64),
                    &(c.updated_at as i64),
                ],
            )
            .await;
        match result {
            Ok(1) => Ok(()),
            Ok(_) => match self.get(&c.account, &c.id).await? {
                Some(_) => Err(StoreError::Conflict),
                None => Err(StoreError::NotFound),
            },
            Err(e) if is_unique_violation(&e) => Err(StoreError::Conflict),
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn delete(&self, account: &str, id: &str) -> Result<bool, StoreError> {
        let n = self
            .client()
            .await?
            .execute(
                "DELETE FROM oidc_connections WHERE account = $1 AND id = $2",
                &[&account, &id],
            )
            .await
            .map_err(unavailable)?;
        Ok(n > 0)
    }

    async fn append_audit(&self, r: &AuditRecord) -> Result<(), StoreError> {
        self.client()
            .await?
            .execute(
                "INSERT INTO oidc_audit (at, account, kind, connection, device, agent, \"user\", purpose, audience, jti, code)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                &[
                    &(r.at as i64),
                    &r.account,
                    &r.kind,
                    &r.connection,
                    &r.device,
                    &r.agent,
                    &r.user,
                    &r.purpose,
                    &r.audience,
                    &r.jti,
                    &r.code,
                ],
            )
            .await
            .map(|_| ())
            .map_err(unavailable)
    }

    async fn audit(
        &self,
        account: &str,
        connection: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, StoreError> {
        let rows = self
            .client()
            .await?
            .query(
                "SELECT at, account, kind, connection, device, agent, \"user\", purpose, audience, jti, code
                 FROM oidc_audit WHERE account = $1 AND ($2::text IS NULL OR connection = $2)
                 ORDER BY seq DESC LIMIT $3",
                &[&account, &connection, &(limit as i64)],
            )
            .await
            .map_err(unavailable)?;
        Ok(rows
            .iter()
            .map(|row| AuditRecord {
                at: row.get::<_, i64>("at") as u64,
                account: row.get("account"),
                kind: row.get("kind"),
                connection: row.get("connection"),
                device: row.get("device"),
                agent: row.get("agent"),
                user: row.get("user"),
                purpose: row.get("purpose"),
                audience: row.get("audience"),
                jti: row.get("jti"),
                code: row.get("code"),
            })
            .collect())
    }

    async fn claim_proof(&self, key: &str, expires_at: u64) -> Result<bool, StoreError> {
        let client = self.client().await?;
        let now = crate::now_secs() as i64;
        // Expired entries go now and then; the primary key keeps it exact.
        if rand::random::<u8>() < 8 {
            let _ = client
                .execute("DELETE FROM oidc_proofs WHERE expires_at <= $1", &[&now])
                .await;
        }
        let row = client
            .query_opt(
                "INSERT INTO oidc_proofs (key, expires_at) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET expires_at = EXCLUDED.expires_at
                     WHERE oidc_proofs.expires_at <= $3
                 RETURNING key",
                &[&key, &(expires_at as i64), &now],
            )
            .await
            .map_err(unavailable)?;
        Ok(row.is_some())
    }

    async fn ready(&self) -> Result<(), StoreError> {
        self.ping().await
    }
}

#[async_trait]
impl Ledger for PgRegistry {
    async fn spent_month(&self, account: &str, connection: &str) -> Result<f64, StoreError> {
        let row = self
            .client()
            .await?
            .query_one(
                "SELECT COALESCE(SUM(usd), 0)::double precision FROM oidc_spend
                 WHERE account = $1 AND connection = $2 AND month = $3",
                &[&account, &connection, &crate::this_month()],
            )
            .await
            .map_err(unavailable)?;
        Ok(row.get::<_, f64>(0))
    }

    async fn record_spend(
        &self,
        account: &str,
        connection: &str,
        device: &str,
        month: &str,
        usd: f64,
    ) -> Result<(), StoreError> {
        self.client()
            .await?
            .execute(
                "INSERT INTO oidc_spend (account, connection, device, month, usd, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (account, connection, month, device)
                 DO UPDATE SET usd = EXCLUDED.usd, updated_at = EXCLUDED.updated_at",
                &[
                    &account,
                    &connection,
                    &device,
                    &month,
                    &usd,
                    &(crate::now_secs() as i64),
                ],
            )
            .await
            .map(|_| ())
            .map_err(unavailable)
    }
}
