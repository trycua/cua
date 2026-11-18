// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-oidc`: the Cua OIDC issuer service. Configured from the
//! environment (or flags); logs never contain tokens or secrets.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use cua_oidc::account::AccountIds;
use cua_oidc::api::Issuer;
use cua_oidc::devices::HttpDeviceDirectory;
use cua_oidc::keycloak::{KeycloakConfig, KeycloakValidator};
use cua_oidc::registry::{Ledger, MemoryLedger, MemoryRegistry, Registry};
use cua_oidc::signer::{load_public, load_signer, KeySet, KeySpec};

/// Cua OIDC issuer for keyless cloud federation.
#[derive(Parser, Debug)]
#[command(name = "cua-oidc", version, about)]
struct Args {
    /// Listen address (TLS terminates in front, at the ingress).
    #[arg(long, env = "CUA_OIDC_LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,
    /// Public base URL; issuers are `<base>/a/<acct>`
    /// (e.g. https://oidc.cua.ai).
    #[arg(long, env = "CUA_OIDC_BASE_URL")]
    base_url: String,
    /// Active signing key: `kms:<key id or ARN>` or `file:<PKCS#8 path>`.
    #[arg(long, env = "CUA_OIDC_SIGNING_KEY", hide_env_values = true)]
    signing_key: KeySpec,
    /// Next key, published before signing switches to it.
    #[arg(long, env = "CUA_OIDC_NEXT_KEY", hide_env_values = true)]
    next_key: Option<KeySpec>,
    /// Previous key, published for a while after signing switched away.
    #[arg(long, env = "CUA_OIDC_PREVIOUS_KEY", hide_env_values = true)]
    previous_key: Option<KeySpec>,
    /// Keycloak realm issuer of the access tokens (e.g.
    /// https://auth.cua.ai/realms/cyclops-cs).
    #[arg(long, env = "CUA_OIDC_KEYCLOAK_ISSUER", hide_env_values = true)]
    keycloak_issuer: String,
    /// JWKS URL (default: discovered from the issuer).
    #[arg(long, env = "CUA_OIDC_KEYCLOAK_JWKS_URL")]
    keycloak_jwks_url: Option<String>,
    /// Accepted access-token audiences, comma-separated.
    #[arg(long, env = "CUA_OIDC_KEYCLOAK_AUDIENCE", hide_env_values = true, default_value = cua_oidc::keycloak::DEFAULT_AUDIENCE)]
    keycloak_audience: String,
    /// Access-token claim naming the account (the relay's account claim).
    #[arg(long, env = "CUA_OIDC_ACCOUNT_CLAIM", default_value = "sub")]
    account_claim: String,
    /// The relay's in-cluster URL (its `/internal/v1/devices/{id}`).
    #[arg(long, env = "CUA_OIDC_RELAY_URL")]
    relay_url: String,
    /// File holding the relay's internal token.
    #[arg(long, env = "CUA_OIDC_RELAY_TOKEN_FILE")]
    relay_token_file: PathBuf,
    /// Postgres URL; unset: in memory (one replica, lost on restart).
    #[arg(long, env = "CUA_OIDC_DATABASE_URL", hide_env_values = true)]
    database_url: Option<String>,
    /// File holding the Postgres URL (instead of CUA_OIDC_DATABASE_URL).
    #[arg(long, env = "CUA_OIDC_DATABASE_URL_FILE")]
    database_url_file: Option<PathBuf>,
    /// CA certificate (PEM) to verify the database's TLS certificate.
    #[arg(long, env = "CUA_OIDC_DATABASE_CA_FILE")]
    database_ca_file: Option<PathBuf>,
    /// File holding the account-id secret (32+ bytes; never rotate it).
    #[arg(long, env = "CUA_OIDC_ACCOUNT_SECRET_FILE")]
    account_secret_file: PathBuf,
}

fn read_secret(path: &PathBuf, what: &str) -> Result<String, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{what} ({}): {e}", path.display()))?;
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(format!("{what} ({}) is empty", path.display()));
    }
    Ok(text)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let base_url = args.base_url.trim_end_matches('/').to_owned();
    let parsed = url::Url::parse(&base_url).map_err(|e| format!("CUA_OIDC_BASE_URL: {e}"))?;
    if parsed.scheme() != "https" && parsed.host_str() != Some("localhost") {
        return Err("CUA_OIDC_BASE_URL must be https (clouds only fetch HTTPS issuers)".into());
    }
    if parsed.query().is_some() || parsed.path() != "/" {
        return Err("CUA_OIDC_BASE_URL must be an origin, like https://oidc.cua.ai".into());
    }

    let mut keys = KeySet::new(load_signer(&args.signing_key).await?);
    if let Some(next) = &args.next_key {
        keys = keys.with_next(load_public(next).await?);
    }
    if let Some(previous) = &args.previous_key {
        keys = keys.with_previous(load_public(previous).await?);
    }

    let mut keycloak = KeycloakConfig::new(args.keycloak_issuer.clone());
    keycloak.jwks_url = args.keycloak_jwks_url.clone();
    keycloak.audiences = args
        .keycloak_audience
        .split(',')
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty())
        .collect();
    keycloak.account_claim = args.account_claim.clone();

    let secret = std::fs::read(&args.account_secret_file).map_err(|e| {
        format!(
            "CUA_OIDC_ACCOUNT_SECRET_FILE ({}): {e}",
            args.account_secret_file.display()
        )
    })?;
    let accounts = AccountIds::new(&secret)?;
    let relay_token = read_secret(&args.relay_token_file, "CUA_OIDC_RELAY_TOKEN_FILE")?;

    let database_url = match (&args.database_url, &args.database_url_file) {
        (Some(url), _) => Some(url.clone()),
        (None, Some(file)) => Some(read_secret(file, "CUA_OIDC_DATABASE_URL_FILE")?),
        (None, None) => None,
    };
    let (registry, ledger): (Arc<dyn Registry>, Arc<dyn Ledger>) = match database_url {
        Some(url) => {
            let ca = match &args.database_ca_file {
                Some(p) => Some(
                    std::fs::read(p)
                        .map_err(|e| format!("CUA_OIDC_DATABASE_CA_FILE ({}): {e}", p.display()))?,
                ),
                None => None,
            };
            let pg = Arc::new(cua_oidc::pg::PgRegistry::connect(&url, ca.as_deref()).await?);
            (pg.clone(), pg)
        }
        None => {
            tracing::warn!(
                "no CUA_OIDC_DATABASE_URL: connections, audit and spend live in memory only"
            );
            (
                Arc::new(MemoryRegistry::new()),
                Arc::new(MemoryLedger::new()),
            )
        }
    };

    tracing::info!(
        base_url = %base_url,
        keys = ?keys,
        keycloak = %args.keycloak_issuer,
        relay = %args.relay_url,
        "cua-oidc starting"
    );
    let issuer = Issuer {
        base_url,
        keys,
        keycloak: Arc::new(KeycloakValidator::new(keycloak)),
        accounts,
        devices: Arc::new(HttpDeviceDirectory::new(&args.relay_url, &relay_token)),
        registry,
        // The devices' month spend reports (`POST /v1/connections/{id}/spend`).
        ledger,
    };
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %args.listen, "cua-oidc listening");
    axum::serve(listener, issuer.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
