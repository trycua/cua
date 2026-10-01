// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-relay`: public rendezvous for cua-spacesd machines behind NAT.

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use cua_relay::server::{Relay, RelayConfig};

/// Reverse-tunnel relay for cua-spacesd.
#[derive(Parser, Debug)]
#[command(name = "cua-relay", version, about)]
struct Args {
    /// Listen address (terminate TLS in front of the relay).
    #[arg(long, env = "CUA_RELAY_LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,
    /// Comma-separated machine-registration tokens.
    #[arg(long, env = "CUA_RELAY_TOKENS", hide_env_values = true)]
    tokens: Option<String>,
    /// File with one machine-registration token per line.
    #[arg(long, env = "CUA_RELAY_TOKEN_FILE")]
    token_file: Option<std::path::PathBuf>,
    /// Token for GET /relay/v1/machines.
    #[arg(long, env = "CUA_RELAY_ADMIN_TOKEN", hide_env_values = true)]
    admin_token: Option<String>,
    /// Host-mode domain: `<machine-id>.<domain>` routes to that machine.
    #[arg(long, env = "CUA_RELAY_HOST_DOMAIN")]
    host_domain: Option<String>,
    /// Maximum machines per registration token.
    #[arg(long, env = "CUA_RELAY_MAX_MACHINES_PER_TOKEN", default_value_t = 16)]
    max_machines_per_token: usize,
    /// Maximum concurrent client streams per machine.
    #[arg(
        long,
        env = "CUA_RELAY_MAX_STREAMS",
        default_value_t = cua_relay::mux::MAX_STREAMS as u32
    )]
    max_streams_per_machine: u32,
    /// yamux's own per-machine-connection stream cap (S7); must satisfy
    /// machine-window-bytes >= this * 256 KiB.
    #[arg(
        long,
        env = "CUA_RELAY_MACHINE_MAX_STREAMS",
        default_value_t = cua_relay::mux::MAX_STREAMS
    )]
    machine_max_streams: usize,
    /// yamux's per-machine connection receive window in bytes: the most
    /// one machine's connection buffers across all of its streams (S7).
    #[arg(
        long,
        env = "CUA_RELAY_MACHINE_WINDOW_BYTES",
        default_value_t = cua_relay::mux::DEFAULT_WINDOW_BYTES
    )]
    machine_window_bytes: usize,
    /// Largest number of concurrent client streams across every machine at
    /// once (S7): the relay-wide memory budget, enforced with
    /// backpressure. Tune to the deployed pod's memory limit.
    #[arg(long, env = "CUA_RELAY_MAX_GLOBAL_STREAMS", default_value_t = 4096)]
    max_global_streams: u64,
    /// Seconds without a heartbeat before a machine is dropped.
    #[arg(long, env = "CUA_RELAY_IDLE_SECS", default_value_t = 90)]
    idle_secs: u64,
    /// Forward requests that carry no spacesd credential.
    #[arg(long, env = "CUA_RELAY_ALLOW_ANONYMOUS_CLIENTS")]
    allow_anonymous_clients: bool,
    /// Account mode: OIDC issuer of the account tokens (e.g.
    /// https://auth.cua.ai/realms/cyclops-cs).
    #[arg(long, env = "CUA_RELAY_OIDC_ISSUER")]
    oidc_issuer: Option<String>,
    /// JWKS URL (default: discovered from the issuer).
    #[arg(long, env = "CUA_RELAY_OIDC_JWKS_URL")]
    oidc_jwks_url: Option<String>,
    /// Accepted audiences, comma-separated.
    #[arg(long, env = "CUA_RELAY_OIDC_AUDIENCE", default_value = cua_relay::oidc::DEFAULT_AUDIENCE)]
    oidc_audience: String,
    /// Token claim naming the account (default: sub).
    #[arg(long, env = "CUA_RELAY_ACCOUNT_CLAIM", default_value = "sub")]
    account_claim: String,
    /// Machine directory file (in memory when unset).
    #[arg(long, env = "CUA_RELAY_STATE_FILE")]
    state_file: Option<std::path::PathBuf>,
    /// Ed25519 PKCS#8 assertion signing key, created on first use (a fresh
    /// key per process when unset).
    #[arg(long, env = "CUA_RELAY_SIGNING_KEY_FILE")]
    signing_key_file: Option<std::path::PathBuf>,
    /// Public base URL (assertion issuer and machine URLs).
    #[arg(long, env = "CUA_RELAY_PUBLIC_URL")]
    public_url: Option<String>,
    /// Maximum machines per account.
    #[arg(long, env = "CUA_RELAY_MAX_MACHINES_PER_ACCOUNT", default_value_t = 32)]
    max_machines_per_account: usize,
    /// Account mode: require enrolled client devices (`on` / `off`).
    #[arg(long, env = "CUA_RELAY_DEVICE_ENROLLMENT", default_value = "on")]
    device_enrollment: String,
    /// Days a device enrollment lasts before one approval re-verifies it.
    #[arg(long, env = "CUA_RELAY_DEVICE_TTL_DAYS", default_value_t = 30)]
    device_ttl_days: u64,
    /// Days after the first start with device enrollment during which
    /// unenrolled devices keep access (flagged and audited). Safe default:
    /// 0 (ended). Set this only for a time-boxed rollout window; a stolen
    /// account token alone reaches every machine while it is open (S3).
    #[arg(long, env = "CUA_RELAY_DEVICE_GRACE_DAYS", default_value_t = 0)]
    device_grace_days: u64,
    /// Maximum age of the sign-in (`auth_time`) that enrolls a device
    /// without an approval.
    #[arg(
        long,
        env = "CUA_RELAY_DEVICE_BOOTSTRAP_MAX_AUTH_AGE_SECS",
        default_value_t = 600
    )]
    device_bootstrap_max_auth_age_secs: u64,
    /// Devices and audit log file (default: `<state file>.devices.json`).
    #[arg(long, env = "CUA_RELAY_DEVICES_FILE")]
    devices_file: Option<std::path::PathBuf>,
    /// Seconds to let open requests finish after SIGTERM / SIGINT before
    /// exiting (machines and clients reconnect on their own).
    #[arg(long, env = "CUA_RELAY_SHUTDOWN_GRACE_SECS", default_value_t = 10)]
    shutdown_grace_secs: u64,
}

/// Resolves on SIGINT or, on Unix, SIGTERM: the stop signal Kubernetes and
/// Docker send. Without a SIGTERM handler the relay, as PID 1 in a
/// container, ignores it and is only killed after the grace period.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => tracing::warn!(%e, "cannot handle SIGTERM"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let mut tokens: Vec<String> = args
        .tokens
        .iter()
        .flat_map(|t| t.split(','))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    if let Some(file) = &args.token_file {
        tokens.extend(
            std::fs::read_to_string(file)?
                .lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty() && !l.starts_with('#')),
        );
    }
    let oidc = args.oidc_issuer.as_ref().map(|issuer| {
        let mut config = cua_relay::oidc::OidcConfig::new(issuer.clone());
        config.jwks_url = args.oidc_jwks_url.clone();
        config.audiences = args
            .oidc_audience
            .split(',')
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty())
            .collect();
        config.account_claim = args.account_claim.clone();
        std::sync::Arc::new(cua_relay::oidc::OidcValidator::new(config))
    });
    if tokens.is_empty() && oidc.is_none() {
        return Err("no way to register machines: set CUA_RELAY_TOKENS / --token-file (static) or CUA_RELAY_OIDC_ISSUER (accounts)".into());
    }
    let signing_key = match &args.signing_key_file {
        Some(path) => Some(std::sync::Arc::new(
            cua_relay::assertion::RelayKey::load_or_create(path)?,
        )),
        None => None,
    };
    let device_enrollment = match args.device_enrollment.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => true,
        "off" | "false" | "0" | "no" => false,
        other => {
            return Err(format!("CUA_RELAY_DEVICE_ENROLLMENT={other:?}: use on or off").into())
        }
    };
    if !device_enrollment && oidc.is_some() {
        tracing::warn!("device enrollment is off: an account token alone reaches machines");
    }
    let relay = Relay::try_new(RelayConfig {
        tokens,
        max_machines_per_token: args.max_machines_per_token,
        max_streams_per_machine: args.max_streams_per_machine,
        max_global_streams: args.max_global_streams,
        machine_max_streams: args.machine_max_streams,
        machine_window_bytes: args.machine_window_bytes,
        machine_idle_timeout: Duration::from_secs(args.idle_secs.max(5)),
        host_domain: args.host_domain,
        admin_token: args.admin_token.filter(|t| !t.is_empty()),
        require_client_credentials: !args.allow_anonymous_clients,
        oidc,
        state_file: args.state_file,
        signing_key,
        public_url: args.public_url,
        max_machines_per_account: args.max_machines_per_account,
        device_enrollment,
        device_policy: cua_relay::devices::DevicePolicy {
            ttl_secs: args.device_ttl_days.max(1) * 86_400,
            grace_secs: args.device_grace_days * 86_400,
            bootstrap_max_auth_age_secs: args.device_bootstrap_max_auth_age_secs,
            ..cua_relay::devices::DevicePolicy::default()
        },
        devices_file: args.devices_file,
    })?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %args.listen, "cua-relay listening");
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let serve = relay.serve(listener, async move {
        shutdown_signal().await;
        tracing::info!("shutting down");
        let _ = stop_tx.send(true);
    });
    // Long-lived streams (gRPC, tunnels) would hold a graceful shutdown
    // open indefinitely: bound it.
    let grace = Duration::from_secs(args.shutdown_grace_secs);
    tokio::select! {
        served = serve => served?,
        _ = async {
            let _ = stop_rx.wait_for(|stopped| *stopped).await;
            tokio::time::sleep(grace).await;
        } => tracing::info!(?grace, "shutdown grace elapsed with open connections; exiting"),
    }
    Ok(())
}
