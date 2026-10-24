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
    /// Comma-separated registration tokens for computers.
    #[arg(long, env = "CUA_RELAY_TOKENS", hide_env_values = true)]
    tokens: Option<String>,
    /// File with one computer registration token per line.
    #[arg(long, env = "CUA_RELAY_TOKEN_FILE")]
    token_file: Option<std::path::PathBuf>,
    /// Token for GET /relay/v1/machines.
    #[arg(long, env = "CUA_RELAY_ADMIN_TOKEN", hide_env_values = true)]
    admin_token: Option<String>,
    /// Host-mode domain: `<computer-id>.<domain>` routes to that computer.
    #[arg(long, env = "CUA_RELAY_HOST_DOMAIN")]
    host_domain: Option<String>,
    /// Maximum computers per registration token.
    #[arg(long, env = "CUA_RELAY_MAX_MACHINES_PER_TOKEN", default_value_t = 16)]
    max_machines_per_token: usize,
    /// Maximum concurrent client streams per computer.
    #[arg(
        long,
        env = "CUA_RELAY_MAX_STREAMS",
        default_value_t = cua_relay::mux::MAX_STREAMS as u32
    )]
    max_streams_per_machine: u32,
    /// yamux's own per-computer-connection stream cap (S7); must satisfy
    /// machine-window-bytes >= this * 256 KiB.
    #[arg(
        long,
        env = "CUA_RELAY_MACHINE_MAX_STREAMS",
        default_value_t = cua_relay::mux::MAX_STREAMS
    )]
    machine_max_streams: usize,
    /// yamux's per-computer connection receive window in bytes: the most
    /// one computer's connection buffers across all of its streams (S7).
    #[arg(
        long,
        env = "CUA_RELAY_MACHINE_WINDOW_BYTES",
        default_value_t = cua_relay::mux::DEFAULT_WINDOW_BYTES
    )]
    machine_window_bytes: usize,
    /// Largest number of concurrent client streams across every computer at
    /// once (S7): the relay-wide memory budget, enforced with
    /// backpressure. Tune to the deployed pod's memory limit.
    #[arg(long, env = "CUA_RELAY_MAX_GLOBAL_STREAMS", default_value_t = 4096)]
    max_global_streams: u64,
    /// Seconds without a heartbeat before a computer is dropped.
    #[arg(long, env = "CUA_RELAY_IDLE_SECS", default_value_t = 90)]
    idle_secs: u64,
    /// Forward requests that carry no spacesd credential.
    #[arg(long, env = "CUA_RELAY_ALLOW_ANONYMOUS_CLIENTS")]
    allow_anonymous_clients: bool,
    /// Account mode: OIDC issuer of the account tokens (e.g.
    /// https://auth.cua.ai/realms/<realm>).
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
    /// Computer directory file (in memory when unset).
    #[arg(long, env = "CUA_RELAY_STATE_FILE")]
    state_file: Option<std::path::PathBuf>,
    /// Ed25519 PKCS#8 assertion signing key, created on first use (a fresh
    /// key per process when unset).
    #[arg(long, env = "CUA_RELAY_SIGNING_KEY_FILE")]
    signing_key_file: Option<std::path::PathBuf>,
    /// Public base URL (assertion issuer and computer URLs).
    #[arg(long, env = "CUA_RELAY_PUBLIC_URL")]
    public_url: Option<String>,
    /// Maximum computers per account.
    #[arg(long, env = "CUA_RELAY_MAX_MACHINES_PER_ACCOUNT", default_value_t = 32)]
    max_machines_per_account: usize,
    /// Account mode: require enrolled client devices (`on` / `off`).
    #[arg(long, env = "CUA_RELAY_DEVICE_ENROLLMENT", default_value = "on")]
    device_enrollment: String,
    /// Days a device enrollment lasts without use (each session renews it)
    /// before a fresh sign-in or one approval re-verifies it.
    #[arg(long, env = "CUA_RELAY_DEVICE_TTL_DAYS", default_value_t = 30)]
    device_ttl_days: u64,
    /// Days after the first start with device enrollment during which
    /// unenrolled devices keep access (flagged and audited). Safe default:
    /// 0 (ended). Set this only for a time-boxed rollout window; a stolen
    /// account token alone reaches every computer while it is open (S3).
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
    /// Strict enrollment: a fresh sign-in enrolls only an account's first
    /// device, a re-key of a computer it already has, or (verified email) a
    /// sign-in proving MFA; any other new device needs an approval from an
    /// enrolled device. Off by default: a fresh sign-in enrolls any device
    /// of the account.
    #[arg(long, env = "CUA_RELAY_DEVICE_REQUIRE_APPROVAL")]
    device_require_approval: bool,
    /// Devices and audit log file (default: `<state file>.devices.json`).
    #[arg(long, env = "CUA_RELAY_DEVICES_FILE")]
    devices_file: Option<std::path::PathBuf>,
    /// Seconds to let open requests finish after SIGTERM / SIGINT before
    /// exiting (computers and clients reconnect on their own).
    #[arg(long, env = "CUA_RELAY_SHUTDOWN_GRACE_SECS", default_value_t = 10)]
    shutdown_grace_secs: u64,
    /// The mesh coordinator and encrypted relay (`on` / `off`). Off unless
    /// set: a relay serves the mesh only when its deployment opts in.
    #[arg(long, env = "CUA_RELAY_MESH", default_value = "off")]
    mesh: String,
    /// Relay mesh packets between peers whose direct path fails (`on` /
    /// `off`; the relay only ever forwards ciphertext).
    #[arg(long, env = "CUA_RELAY_MESH_RELAY", default_value = "on")]
    mesh_relay: String,
    /// UDP address to answer STUN on (e.g. `0.0.0.0:3478`), so mesh nodes
    /// learn their public address.
    #[arg(long, env = "CUA_RELAY_MESH_STUN_LISTEN")]
    mesh_stun_listen: Option<std::net::SocketAddr>,
    /// STUN servers to advertise (`host:port`, comma-separated). Default:
    /// this relay's public host on the `--mesh-stun-listen` port.
    #[arg(long, env = "CUA_RELAY_MESH_STUN")]
    mesh_stun: Option<String>,
    /// Where the mesh registry persists (default: `<state>.mesh.json`).
    #[arg(long, env = "CUA_RELAY_MESH_FILE")]
    mesh_file: Option<std::path::PathBuf>,
    /// Pool writes leave at least this percent of the state volume free
    /// (device, session and directory writes may use it).
    #[arg(long, env = "CUA_RELAY_STATE_RESERVE_PERCENT", default_value_t = 15)]
    state_reserve_percent: u64,
    /// ... and at least this many MiB.
    #[arg(long, env = "CUA_RELAY_STATE_RESERVE_MIB", default_value_t = 256)]
    state_reserve_mib: u64,
    /// Most MiB one account's pool keeps on the state volume.
    #[arg(long, env = "CUA_RELAY_POOL_ACCOUNT_QUOTA_MIB", default_value_t = 512)]
    pool_account_quota_mib: u64,
    /// Probe address answering `GET /healthz` from the moment the relay
    /// starts, standby included (e.g. `0.0.0.0:8081`). See
    /// `cua_relay::handover`.
    #[arg(long, env = "CUA_RELAY_HEALTH_LISTEN")]
    health_listen: Option<SocketAddr>,
    /// Seconds after start during which a request for a known computer that
    /// is not connected yet waits for it to reconnect instead of failing.
    #[arg(long, env = "CUA_RELAY_RECONNECT_GRACE_SECS", default_value_t = 15)]
    reconnect_grace_secs: u64,
    /// OIDC client id of the browser sign-in for `cua share` links (a
    /// public client with PKCE; its redirect URI is
    /// `<public url>/v1/web/callback`). Needs `--public-url`,
    /// `--host-domain` and account mode. Unset: no browser sign-in.
    #[arg(long, env = "CUA_RELAY_WEB_CLIENT_ID")]
    web_client_id: Option<String>,
    /// Client secret of a confidential browser sign-in client.
    #[arg(long, env = "CUA_RELAY_WEB_CLIENT_SECRET", hide_env_values = true)]
    web_client_secret: Option<String>,
    /// Sustained requests per second one address may send to public share
    /// links.
    #[arg(
        long,
        env = "CUA_RELAY_SHARE_PUBLIC_RPS_PER_IP",
        default_value_t = 20.0
    )]
    share_public_rps_per_ip: f64,
    /// Burst of requests one address may send to public share links.
    #[arg(
        long,
        env = "CUA_RELAY_SHARE_PUBLIC_BURST_PER_IP",
        default_value_t = 60.0
    )]
    share_public_burst_per_ip: f64,
    /// Sustained requests per second one machine's public share links take
    /// from everyone together.
    #[arg(
        long,
        env = "CUA_RELAY_SHARE_PUBLIC_RPS_PER_MACHINE",
        default_value_t = 200.0
    )]
    share_public_rps_per_machine: f64,
    /// Burst of requests one machine's public share links take.
    #[arg(
        long,
        env = "CUA_RELAY_SHARE_PUBLIC_BURST_PER_MACHINE",
        default_value_t = 600.0
    )]
    share_public_burst_per_machine: f64,
    /// File holding the 32-byte key behind analytics ids and log tags;
    /// created 0600 on first use. Default: `<state file>.analytics-key`.
    #[arg(long, env = "CUA_RELAY_ANALYTICS_KEY_FILE")]
    analytics_key_file: Option<std::path::PathBuf>,
    /// Days audit log entries are kept (0: only the per-account count limit).
    #[arg(
        long,
        env = "CUA_RELAY_AUDIT_RETENTION_DAYS",
        default_value_t = cua_relay::server::DEFAULT_AUDIT_RETENTION_DAYS
    )]
    audit_retention_days: u64,
    /// Days a mesh node may be offline before its stored endpoints are
    /// dropped (its key registration stays; 0: never).
    #[arg(
        long,
        env = "CUA_RELAY_MESH_ENDPOINT_RETENTION_DAYS",
        default_value_t = cua_relay::server::DEFAULT_MESH_ENDPOINT_RETENTION_DAYS
    )]
    mesh_endpoint_retention_days: u64,
    /// Reverse proxies (comma-separated CIDRs) whose `X-Forwarded-For` the
    /// relay believes, e.g. the ingress pod network. Empty: the header is
    /// ignored and the connecting address is the client.
    #[arg(long, env = "CUA_RELAY_TRUSTED_PROXIES", default_value = "")]
    trusted_proxies: String,
    /// Serve only STUN on `--mesh-stun-listen` (and `/healthz` on
    /// `--listen`): a separate process owns the STUN host port, so relay
    /// pods can overlap during a deploy.
    #[arg(long, env = "CUA_RELAY_STUN_ONLY")]
    stun_only: bool,
    /// Optional hosted-relay features: plan metering, Teams and the Spaces
    /// API.
    #[cfg(feature = "hosted")]
    #[command(flatten)]
    hosted: cua_relay::hosted::HostedArgs,
}

fn on_off(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        other => Err(format!("{name}={other:?}: use on or off")),
    }
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
    if args.stun_only {
        let addr = args
            .mesh_stun_listen
            .ok_or("CUA_RELAY_STUN_ONLY needs CUA_RELAY_MESH_STUN_LISTEN")?;
        let sock = tokio::net::UdpSocket::bind(addr).await?;
        let listener = tokio::net::TcpListener::bind(args.listen).await?;
        tracing::info!(%addr, health = %args.listen, "STUN only");
        tokio::select! {
            _ = cua_relay::mesh::serve_stun(sock) => {}
            served = cua_relay::handover::serve_health(listener) => served?,
            _ = shutdown_signal() => tracing::info!("shutting down"),
        }
        return Ok(());
    }
    // Ports first: from here on connections queue (the listen backlog)
    // instead of being refused, while this relay waits its turn below.
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    if let Some(addr) = args.health_listen {
        let health = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!(%addr, "health probe listening");
        tokio::spawn(async move {
            if let Err(e) = cua_relay::handover::serve_health(health).await {
                tracing::error!(%e, "health probe stopped");
            }
        });
    }
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
    let share_web = match args.web_client_id.clone().filter(|c| !c.is_empty()) {
        Some(client_id) => {
            if oidc.is_none() || args.public_url.is_none() || args.host_domain.is_none() {
                return Err("CUA_RELAY_WEB_CLIENT_ID needs CUA_RELAY_OIDC_ISSUER, CUA_RELAY_PUBLIC_URL and CUA_RELAY_HOST_DOMAIN (share links sign in on the relay's origin and open on their own)".into());
            }
            Some(cua_relay::share_web::ShareWebConfig {
                client_id,
                client_secret: args.web_client_secret.clone().filter(|s| !s.is_empty()),
            })
        }
        None => None,
    };
    if tokens.is_empty() && oidc.is_none() {
        return Err("no way to register computers: set CUA_RELAY_TOKENS / --token-file (static) or CUA_RELAY_OIDC_ISSUER (accounts)".into());
    }
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
    let mesh_enabled = on_off("CUA_RELAY_MESH", &args.mesh)?;
    let mut mesh_stun: Vec<String> = args
        .mesh_stun
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if mesh_stun.is_empty() {
        if let (Some(listen), Some(public)) = (args.mesh_stun_listen, args.public_url.as_deref()) {
            if let Some(host) = url::Url::parse(public)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
            {
                mesh_stun.push(format!("{host}:{}", listen.port()));
            }
        }
    }
    #[cfg(feature = "hosted")]
    let (meter, org_access) = args
        .hosted
        .configs(oidc.is_some(), args.oidc_issuer.as_deref())?;
    // One relay per state directory (CUA-1774): wait here, in standby, until
    // the relay before this one exits. Every config error above surfaces
    // before this point, so a bad deploy never takes over.
    let state_dir = args
        .state_file
        .as_ref()
        .or(args.devices_file.as_ref())
        .map(|p| match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => std::path::PathBuf::from("."),
        });
    let _state_lock = match &state_dir {
        Some(dir) => Some(
            cua_relay::handover::StateLock::acquire(&cua_relay::handover::lock_path(dir)).await?,
        ),
        None => None,
    };
    let signing_key = match &args.signing_key_file {
        Some(path) => Some(std::sync::Arc::new(
            cua_relay::assertion::RelayKey::load_or_create(path)?,
        )),
        None => None,
    };
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
            require_approval: args.device_require_approval,
            ..cua_relay::devices::DevicePolicy::default()
        },
        devices_file: args.devices_file,
        mesh: cua_relay::mesh::MeshConfig {
            enabled: mesh_enabled,
            relay: on_off("CUA_RELAY_MESH_RELAY", &args.mesh_relay)?,
            stun: mesh_stun,
        },
        mesh_file: args.mesh_file,
        storage_limits: cua_relay::storage::StorageLimits {
            reserve_percent: args.state_reserve_percent.min(90),
            reserve_min_bytes: args.state_reserve_mib << 20,
            pool_account_quota_bytes: args.pool_account_quota_mib.max(1) << 20,
        },
        #[cfg(feature = "hosted")]
        meter,
        #[cfg(feature = "hosted")]
        org_access,
        reconnect_grace: Duration::from_secs(args.reconnect_grace_secs),
        share_web,
        analytics_key_file: args.analytics_key_file,
        audit_retention_days: args.audit_retention_days,
        mesh_endpoint_retention_days: args.mesh_endpoint_retention_days,
        trusted_proxies: cua_relay::proxies::TrustedProxies::parse(&args.trusted_proxies)
            .map_err(|e| format!("--trusted-proxies: {e}"))?,
        share_limits: cua_relay::share_web::ShareLimits {
            per_ip_rps: args.share_public_rps_per_ip.max(0.1),
            per_ip_burst: args.share_public_burst_per_ip.max(1.0),
            per_machine_rps: args.share_public_rps_per_machine.max(0.1),
            per_machine_burst: args.share_public_burst_per_machine.max(1.0),
        },
    })?;
    if mesh_enabled {
        if let Some(addr) = args.mesh_stun_listen {
            let sock = tokio::net::UdpSocket::bind(addr).await?;
            tracing::info!(%addr, "mesh STUN listening");
            tokio::spawn(cua_relay::mesh::serve_stun(sock));
        }
    }
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
