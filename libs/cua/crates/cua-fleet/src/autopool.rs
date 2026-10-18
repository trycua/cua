//! Automatic Fleet pools: `Sandbox.create(image)` on Fleet without a pool.
//!
//! [`PoolManager`] turns "a sandbox from this image and shape" into a claim
//! on a reusable, autoscaled warm pool:
//!
//! - **Keyed by spec.** A [`PoolSpecKey`] (image digest, runtime, firmware,
//!   cpu, memory, services, probes) hashes to [`PoolSpecKey::spec_hash`].
//!   Equal keys share one pool per account.
//! - **Tenant-scoped deterministic names.** The pool (= namespace = template)
//!   is `cua-auto-<base32(sha256(tenant, spec_hash))[..16]>`. Pool names
//!   are global on Fleet, so a name held by another account (HTTP 403,
//!   [`SdkError::PoolAccessDenied`]) moves to `-2`, `-3`, ... (at most
//!   [`MAX_NAME_PROBES`]).
//! - **Server-authoritative discovery.** `~/.cua/fleet-pools.json` only
//!   caches the mapping; the tenant-scoped namespace list plus the pool's
//!   `cua.ai/spec-hash` label decide.
//! - **Autoscaled, never reconciled.** A new pool gets `autoscaling {min 0,
//!   initial 0 (1 when warm), max}` so KEDA scales it with claim demand and
//!   to zero when idle. An existing pool's `replicas` and spec are never
//!   written back (only labels, the backstop TTL, and a raised
//!   `maxPoolSize`).
//! - **Nothing leaks forever.** Pools carry a backstop
//!   `ttlSecondsAfterCreated` (renewed while used); claims carry a TTL
//!   renewed by a heartbeat while the [`ManagedClaim`] lives, so a crashed
//!   process's claim expires; [`PoolManager::gc`] deletes idle pools and
//!   stuck Pending/Failed claims and runs at most hourly per machine.
//! - **Concurrency.** Processes racing on the same key converge on one
//!   pool: namespace and pool creation are idempotent (409 = someone else
//!   created it) and 403 right after a namespace was created is retried as
//!   Capsule adoption lag rather than read as a foreign name.
//!
//! Fleet gaps worked around here (the cloud follow-up removes them):
//! `create_pool` drops labels, so labels are merge-patched right after
//! creation; resources have no annotations in the SDK, so `last-used` is a
//! label; there is no idle GC on the server, so the client runs one.

use crate::{BoundSandbox, ClaimOptions, Error, FleetClient, Pool, PoolSpec, Result, SdkError};
use base64::Engine as _;
use cyclops_sdk::{HttpHeader, HttpRequest, Namespace};
use cyclops_sdk_schema::{ClaimSpec, RuntimeKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Floor of a claim's `bindDeadline`: live cold binds (KEDA from zero,
/// image pull, boot) took 80 to 530 s, and a Pending claim is the
/// autoscaler's demand signal, so it must not fail first.
pub const MIN_BIND_DEADLINE_SECS: u64 = 900;
/// Default idle threshold of the automatic GC of managed pools (30 min, the
/// same as the Python SDK and the documented default).
pub const DEFAULT_IDLE_GC_SECS: u64 = 30 * 60;

/// Name prefix of managed pools.
pub const AUTO_POOL_PREFIX: &str = "cua-auto-";
/// Name prefix of legacy ephemeral pools (`Sandbox.ephemeral`), also GC'd.
pub const EPHEMERAL_POOL_PREFIX: &str = "cua-eph-";
/// Label naming the creator.
pub const LABEL_MANAGED_BY: &str = "cua.ai/managed-by";
/// Value of [`LABEL_MANAGED_BY`].
pub const MANAGED_BY: &str = "cua-sdk";
/// Label carrying the first 32 hex digits of the spec hash.
pub const LABEL_SPEC_HASH: &str = "cua.ai/spec-hash";
/// Label carrying the last use (unix seconds).
pub const LABEL_LAST_USED: &str = "cua.ai/last-used";
/// Candidate names tried per key before giving up.
pub const MAX_NAME_PROBES: usize = 8;
/// Hex digits of the spec hash stored in [`LABEL_SPEC_HASH`] (label values
/// are at most 63 bytes).
pub const SPEC_HASH_LABEL_LEN: usize = 32;

const K8S: &str = "/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces";
const CACHE_FILE: &str = "fleet-pools.json";
const GC_LOCK_FILE: &str = "fleet-gc.lock";
const GC_STAMP_FILE: &str = "fleet-gc.stamp";
/// A GC lock older than this is considered abandoned.
const GC_LOCK_STALE: Duration = Duration::from_secs(15 * 60);
/// Pool POST retries while a fresh namespace is being adopted.
const ADOPTION_RETRIES: u32 = 6;
/// Minimum spacing of `last-used` patches for one pool.
const TOUCH_EVERY: Duration = Duration::from_secs(60);
/// Resolved image digests are reused for this long.
const RESOLVE_TTL: Duration = Duration::from_secs(300);

// ------------------------------------------------------------------ config

/// Clock returning unix seconds (tests share the fake Fleet's clock).
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

/// Pool manager settings. [`AutoPoolConfig::from_env`] reads the overrides.
#[derive(Clone)]
pub struct AutoPoolConfig {
    /// New pools start with one warm replica (`CUA_FLEET_WARM`). Only
    /// affects pool creation; KEDA owns replicas afterwards.
    pub warm: bool,
    /// Autoscaling ceiling of new pools (`CUA_FLEET_MAX_POOL_SIZE`,
    /// default 10). A larger value raises an existing pool's ceiling.
    pub max_pool_size: u32,
    /// Claim TTL (`CUA_FLEET_CLAIM_TTL`, default 15 min). A claim whose
    /// heartbeat stops (crash, `detach`) is reaped this long after its last
    /// renewal.
    pub claim_ttl: Duration,
    /// Heartbeat period (default `claim_ttl / 3`).
    pub heartbeat_every: Option<Duration>,
    /// Claim `bindDeadline`: at least [`MIN_BIND_DEADLINE_SECS`] (a shorter
    /// value is raised; `acquire` waits this long plus a minute).
    pub bind_deadline: Duration,
    /// Pool backstop `ttlSecondsAfterCreated` (default 7 days).
    pub pool_ttl: Duration,
    /// Renew the backstop when less than this remains (default 1 day).
    pub pool_ttl_renew_window: Duration,
    /// Idle threshold of the automatic GC (`CUA_FLEET_POOL_IDLE_GC`,
    /// default [`DEFAULT_IDLE_GC_SECS`], 30 min; `off` or `0` disables it). [`PoolManager::gc`] takes
    /// its own threshold.
    pub idle_gc: Option<Duration>,
    /// Automatic GC period per machine (default 1 h).
    pub auto_gc_every: Duration,
    /// Where the name cache and GC lock live (`$CUA_HOME`, else `~/.cua`).
    pub home: PathBuf,
    /// Tenant override. Default: the `sub` of the Fleet bearer.
    pub tenant: Option<String>,
    /// Clock (unix seconds).
    pub clock: Clock,
}

impl std::fmt::Debug for AutoPoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoPoolConfig")
            .field("warm", &self.warm)
            .field("max_pool_size", &self.max_pool_size)
            .field("claim_ttl", &self.claim_ttl)
            .field("bind_deadline", &self.bind_deadline)
            .field("pool_ttl", &self.pool_ttl)
            .field("idle_gc", &self.idle_gc)
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

/// Default home: `$CUA_HOME`, else `~/.cua`.
pub fn default_cua_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cua")
}

impl Default for AutoPoolConfig {
    fn default() -> Self {
        Self {
            warm: false,
            max_pool_size: 10,
            claim_ttl: Duration::from_secs(15 * 60),
            heartbeat_every: None,
            bind_deadline: Duration::from_secs(MIN_BIND_DEADLINE_SECS),
            pool_ttl: Duration::from_secs(7 * 24 * 3600),
            pool_ttl_renew_window: Duration::from_secs(24 * 3600),
            idle_gc: Some(Duration::from_secs(DEFAULT_IDLE_GC_SECS)),
            auto_gc_every: Duration::from_secs(3600),
            home: default_cua_home(),
            tenant: None,
            clock: system_clock(),
        }
    }
}

fn parse_duration(v: &str) -> Option<Duration> {
    let v = v.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    humantime::parse_duration(v).ok()
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" | "" => Some(false),
        _ => None,
    }
}

impl AutoPoolConfig {
    /// Defaults plus `CUA_FLEET_MAX_POOL_SIZE`, `CUA_FLEET_CLAIM_TTL`
    /// (seconds or `15m`), `CUA_FLEET_POOL_IDLE_GC` (`30m`, `off`),
    /// `CUA_FLEET_WARM` and `CUA_HOME`.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// [`AutoPoolConfig::from_env`] over an arbitrary lookup.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = Self::default();
        if let Some(n) = get("CUA_FLEET_MAX_POOL_SIZE").and_then(|v| v.trim().parse().ok()) {
            c.max_pool_size = n;
        }
        if let Some(d) = get("CUA_FLEET_CLAIM_TTL")
            .and_then(|v| parse_duration(&v))
            .filter(|d| !d.is_zero())
        {
            c.claim_ttl = d;
        }
        if let Some(v) = get("CUA_FLEET_POOL_IDLE_GC") {
            c.idle_gc = match v.trim().to_ascii_lowercase().as_str() {
                "off" | "none" | "never" | "false" => None,
                other => match parse_duration(other) {
                    Some(d) if d.is_zero() => None,
                    Some(d) => Some(d),
                    None => c.idle_gc,
                },
            };
        }
        if let Some(w) = get("CUA_FLEET_WARM").and_then(|v| parse_bool(&v)) {
            c.warm = w;
        }
        if let Some(h) = get("CUA_HOME").filter(|h| !h.trim().is_empty()) {
            c.home = PathBuf::from(h);
        }
        c
    }

    /// Puts the cache and GC lock next to a sandbox state directory:
    /// `<home>/sandboxes` uses `<home>`, any other directory itself. A temp
    /// state dir therefore never touches the real `~/.cua`.
    pub fn with_state_dir(mut self, dir: &Path) -> Self {
        self.home = match (dir.file_name(), dir.parent()) {
            (Some(n), Some(parent)) if n == "sandboxes" => parent.to_path_buf(),
            _ => dir.to_path_buf(),
        };
        self
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn heartbeat_period(&self, claim_ttl: Duration) -> Duration {
        self.heartbeat_every
            .unwrap_or(claim_ttl / 3)
            .max(Duration::from_millis(10))
    }
}

// --------------------------------------------------------------------- key

/// What makes two sandboxes interchangeable: they may share a pool.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolSpecKey {
    /// Image reference. Tags are resolved to a digest when a resolver is
    /// set ([`PoolManager::with_resolver`]).
    pub image: String,
    /// Runtime.
    pub runtime: RuntimeKind,
    /// UEFI firmware.
    pub efi: bool,
    /// vCPUs.
    pub cpu: Option<u32>,
    /// Memory (MiB).
    pub memory_mb: Option<u32>,
    /// Services (name to guest port).
    pub services: BTreeMap<String, u16>,
    /// TCP readiness probe port.
    pub readiness_tcp_port: Option<u16>,
    /// Entrypoint override (`processMode: Run` on every runtime).
    pub command: Option<Vec<String>>,
    /// Environment (`processMode: Run` on every runtime).
    pub env: BTreeMap<String, String>,
    /// Sidecar containers, on every runtime (see [`crate::parity`]).
    pub sidecars: Vec<crate::Sidecar>,
    /// A `cua-registry-*` pull Secret for private images. Its name derives
    /// from the registry and user, not the password, so rotating a password
    /// keeps the pool.
    pub pull_secret: Option<String>,
    /// Template opt-in to per-claim Secrets (`vmTemplate.claimSecrets`):
    /// claims then carry [`AcquireOpts::claim_token`], which the guest's
    /// cua-spacesd reads from `/run/cua/env-token`. Without it a Spaces
    /// image mints its own token that no client knows.
    pub claim_secrets: bool,
}

impl PoolSpecKey {
    /// A key with the same defaults as [`PoolSpec::new`] (kubevirt,
    /// service `env` on 3211).
    pub fn new(image: impl Into<String>) -> Self {
        let spec = PoolSpec::new("x", image);
        Self {
            image: spec.image,
            runtime: spec.runtime,
            efi: spec.efi,
            cpu: None,
            memory_mb: None,
            services: spec.services,
            readiness_tcp_port: None,
            command: None,
            env: BTreeMap::new(),
            sidecars: vec![],
            pull_secret: None,
            claim_secrets: false,
        }
    }

    /// Sets the runtime.
    pub fn runtime(mut self, runtime: RuntimeKind) -> Self {
        self.runtime = runtime;
        self
    }

    /// Sets vCPUs and memory.
    pub fn resources(mut self, cpu: Option<u32>, memory_mb: Option<u32>) -> Self {
        self.cpu = cpu;
        self.memory_mb = memory_mb;
        self
    }

    /// Replaces the services.
    pub fn services<I, S>(mut self, services: I) -> Self
    where
        I: IntoIterator<Item = (S, u16)>,
        S: Into<String>,
    {
        self.services = services.into_iter().map(|(k, v)| (k.into(), v)).collect();
        self
    }

    /// Canonical encoding (versioned JSON with a fixed field order).
    pub fn canonical(&self) -> String {
        let runtime = serde_json::to_value(&self.runtime)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let services: Vec<Value> = self.services.iter().map(|(k, v)| json!([k, v])).collect();
        // An array keeps the order independent of serde_json's map flavor.
        let mut fields = vec![
            json!("cua-autopool/v1"),
            json!(["image", self.image.trim()]),
            json!(["runtime", runtime]),
            json!(["efi", self.efi]),
            json!(["cpu", self.cpu]),
            json!(["memory_mb", self.memory_mb]),
            json!(["services", services]),
            json!(["readiness_tcp_port", self.readiness_tcp_port]),
            json!(["command", self.command]),
        ];
        // Only when set, so the keys (and pools) of specs without env stay
        // what they were.
        if !self.env.is_empty() {
            let env: Vec<Value> = self.env.iter().map(|(k, v)| json!([k, v])).collect();
            fields.push(json!(["env", env]));
        }
        // Same rule for the parity fields: only when set.
        if !self.sidecars.is_empty() {
            let sidecars: Vec<Value> = self
                .sidecars
                .iter()
                .map(|s| {
                    let env: Vec<Value> = s.env.iter().map(|(k, v)| json!([k, v])).collect();
                    json!([s.name, s.image.trim(), s.command, env, s.ports])
                })
                .collect();
            fields.push(json!(["sidecars", sidecars]));
        }
        if let Some(secret) = &self.pull_secret {
            fields.push(json!(["pull_secret", secret]));
        }
        if self.claim_secrets {
            fields.push(json!(["claim_secrets", true]));
        }
        Value::Array(fields).to_string()
    }

    /// sha256 (hex) of [`PoolSpecKey::canonical`].
    pub fn spec_hash(&self) -> String {
        hex::encode(Sha256::digest(self.canonical().as_bytes()))
    }

    /// What sandboxes of this key run (the shared model).
    pub fn sandbox_spec(&self) -> crate::SandboxSpec {
        crate::SandboxSpec {
            image: self.image.clone(),
            command: self.command.clone(),
            args: None,
            env: self.env.clone(),
            services: self.services.clone(),
            readiness: self
                .readiness_tcp_port
                .map(|port| crate::ReadinessProbe::Tcp { port }),
            cpu: self.cpu,
            memory_mb: self.memory_mb,
            efi: self.efi,
            sidecars: self.sidecars.clone(),
            registry_secret: self.pull_secret.clone(),
            process_mode: None,
            claim_secrets: self.claim_secrets,
        }
    }

    /// The capacity of a managed pool: autoscaled, warm (an explicit floor
    /// of one, `minPoolSize: 1`) when `initial > 0`, else from zero; the
    /// backstop TTL, and `ttlPolicy: Cascade` so an expired pool takes its
    /// dead unbound claims with it.
    pub fn pool_options(&self, initial: u32, max: u32, ttl: Duration) -> crate::PoolOptions {
        crate::PoolOptions {
            runtime: Some(self.runtime.clone()),
            replicas: Some(initial),
            warm: Some(initial > 0),
            min_pool_size: Some(u32::from(initial > 0)),
            max_pool_size: Some(max.max(1).max(initial)),
            idle_ttl: None,
            ttl_policy: Some(crate::TtlPolicy::Cascade),
            pool_ttl: Some(ttl),
            claim_ttl: None,
        }
    }

    /// The pool spec for `name` ([`Self::sandbox_spec`] +
    /// [`Self::pool_options`]).
    pub fn pool_spec(&self, name: &str, initial: u32, max: u32, ttl: Duration) -> PoolSpec {
        PoolSpec::from_parts(
            name,
            &self.sandbox_spec(),
            &self.pool_options(initial, max, ttl),
        )
    }
}

fn written_secrets() -> &'static Mutex<std::collections::HashSet<String>> {
    static W: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    W.get_or_init(Default::default)
}

/// Lowercase RFC 4648 base32 without padding (DNS-1123 safe).
pub(crate) fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The base pool name for a tenant and spec hash.
pub fn auto_pool_name(tenant: &str, spec_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(tenant.as_bytes());
    h.update([0u8]);
    h.update(spec_hash.as_bytes());
    format!("{AUTO_POOL_PREFIX}{}", &base32(&h.finalize())[..16])
}

/// Every candidate name, in probe order (`base`, `base-2`, ...).
pub fn candidate_names(tenant: &str, spec_hash: &str) -> Vec<String> {
    let base = auto_pool_name(tenant, spec_hash);
    (0..MAX_NAME_PROBES)
        .map(|i| {
            if i == 0 {
                base.clone()
            } else {
                format!("{base}-{}", i + 1)
            }
        })
        .collect()
}

fn hash_label(spec_hash: &str) -> &str {
    &spec_hash[..SPEC_HASH_LABEL_LEN.min(spec_hash.len())]
}

/// The tenant a bearer belongs to: the JWT `sub`, else a hash of the token.
pub fn tenant_from_token(token: &str) -> String {
    let mut parts = token.split('.');
    if let (Some(_), Some(payload), Some(_)) = (parts.next(), parts.next(), parts.next())
        && let Ok(bytes) =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))
        && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
        && let Some(sub) = v["sub"].as_str().filter(|s| !s.is_empty())
    {
        return sub.to_string();
    }
    format!(
        "token:{}",
        &hex::encode(Sha256::digest(token.as_bytes()))[..32]
    )
}

// ------------------------------------------------------------ image digest

/// Resolves an image tag to a digest-pinned reference (`repo@sha256:...`).
/// `None` keeps the reference as given (private registry, offline).
#[async_trait::async_trait]
pub trait ImageResolver: Send + Sync {
    /// Resolves `image`.
    async fn resolve(&self, image: &str) -> Option<String>;
}

// ------------------------------------------------------------- raw helpers

fn parse_time(s: Option<&str>) -> Option<i64> {
    humantime::parse_rfc3339_weak(s?)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn is_status(e: &Error, code: u16) -> bool {
    matches!(e, Error::Sdk(SdkError::Status { status, .. }) if *status == code)
}

fn is_denied(e: &Error) -> bool {
    matches!(
        e,
        Error::Sdk(SdkError::PoolAccessDenied { .. })
            | Error::Sdk(SdkError::Status { status: 403, .. })
    )
}

impl FleetClient {
    pub(crate) fn k8s_url(&self, ns: &str, plural: &str, name: Option<&str>) -> String {
        let base = self.config().base_url.trim_end_matches('/');
        match name {
            Some(n) => format!("{base}{K8S}/{ns}/{plural}/{n}"),
            None => format!("{base}{K8S}/{ns}/{plural}"),
        }
    }

    /// One control-plane request; returns the status and JSON body for any
    /// HTTP status (transport and token failures are errors).
    pub(crate) async fn raw(
        &self,
        method: &str,
        url: String,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let content_type = if method == "PATCH" {
            "application/merge-patch+json"
        } else {
            "application/json"
        };
        let request = HttpRequest {
            method: method.into(),
            url,
            headers: vec![
                HttpHeader {
                    name: "accept".into(),
                    value: "application/json".into(),
                },
                HttpHeader {
                    name: "content-type".into(),
                    value: content_type.into(),
                },
            ],
            body: body.map(|b| b.to_string().into_bytes()),
            timeout_secs: Some(30),
            max_response_bytes: Some(16 * 1024 * 1024),
        };
        match self.sdk().execute_authenticated(request).await {
            Ok(r) => Ok((
                r.status,
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )),
            Err(SdkError::Status { status, body, .. }) => Ok((
                status,
                serde_json::from_str(&body).unwrap_or(Value::String(body)),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// The warm pool resource as raw JSON (labels, deletionTimestamp and
    /// all). `Ok(None)` when absent; 403 is an error (not ours).
    pub async fn get_pool_json(&self, name: &str) -> Result<Option<Value>> {
        cyclops_sdk::validate_dns_label(name)?;
        let url = self.k8s_url(name, "osgymsandboxwarmpools", Some(name));
        match self.raw("GET", url, None).await? {
            (200, v) => Ok(Some(v)),
            (404, _) => Ok(None),
            (status, v) => {
                Err(SdkError::status("get pool", status, v.to_string().as_bytes()).into())
            }
        }
    }

    /// JSON merge patch on a pool (never touches fields not in `patch`).
    pub async fn patch_pool_json(&self, name: &str, patch: Value) -> Result<Value> {
        cyclops_sdk::validate_dns_label(name)?;
        let url = self.k8s_url(name, "osgymsandboxwarmpools", Some(name));
        match self.raw("PATCH", url, Some(patch)).await? {
            (200, v) => Ok(v),
            (status, v) => {
                Err(SdkError::status("patch pool", status, v.to_string().as_bytes()).into())
            }
        }
    }
}

// ------------------------------------------------------------------- types

/// Per-acquire options.
#[derive(Clone, Debug, Default)]
pub struct AcquireOpts {
    /// Claim name. An existing claim with this name is reattached.
    pub name: Option<String>,
    /// Override [`AutoPoolConfig::warm`] (pool creation only).
    pub warm: Option<bool>,
    /// Credentials written to the key's `pull_secret` in the pool
    /// namespace before the template (and refreshed once per process).
    pub registry_credentials: Option<crate::RegistryCredentials>,
    /// Override [`AutoPoolConfig::max_pool_size`].
    pub max_pool_size: Option<u32>,
    /// Override [`AutoPoolConfig::claim_ttl`].
    pub claim_ttl: Option<Duration>,
    /// Extra claim labels.
    pub labels: BTreeMap<String, String>,
    /// Bind budget for this claim (for example the caller's
    /// `time_to_start`); the effective `bindDeadline` is the largest of
    /// this, [`AutoPoolConfig::bind_deadline`] and
    /// [`MIN_BIND_DEADLINE_SECS`].
    pub bind_deadline: Option<Duration>,
    /// Per-claim env token, delivered through the claim's Secret (the key
    /// must set [`PoolSpecKey::claim_secrets`]). A new claim is returned
    /// only once the guest's cua-spacesd accepts it (bounded, see
    /// [`crate::claim_secrets::DEFAULT_WAIT`]); a reattached claim keeps
    /// the token it was created with.
    pub claim_token: Option<String>,
}

/// A managed pool as [`PoolManager::list`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedPoolInfo {
    /// Pool (= namespace = template) name.
    pub name: String,
    /// Created by the SDK (label) rather than only matching the prefix.
    pub managed: bool,
    /// [`LABEL_SPEC_HASH`], when set.
    pub spec_hash: Option<String>,
    /// Template image, when readable.
    pub image: Option<String>,
    /// `spec.replicas` (KEDA-owned).
    pub replicas: u32,
    /// Ready replicas.
    pub ready_replicas: Option<u32>,
    /// Autoscaling ceiling.
    pub max_pool_size: Option<u32>,
    /// Claims in the pool.
    pub claims: u32,
    /// Bound claims.
    pub bound_claims: u32,
    /// Last use (unix seconds): the label, else creation.
    pub last_used: Option<i64>,
    /// Creation (unix seconds).
    pub created: Option<i64>,
    /// Backstop expiry (unix seconds).
    pub expires_at: Option<i64>,
    /// Being deleted.
    pub terminating: bool,
}

/// What [`PoolManager::gc`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Deleted pools (with their namespaces).
    pub deleted_pools: Vec<String>,
    /// Deleted leftover namespaces without a pool.
    pub deleted_namespaces: Vec<String>,
    /// Deleted stuck claims (`namespace/name`).
    pub deleted_claims: Vec<String>,
    /// Pools kept (in use or not idle long enough).
    pub kept: Vec<String>,
    /// Non-fatal failures.
    pub errors: Vec<String>,
}

enum Probe {
    Usable(Box<Pool>, Value),
    /// Our namespace, no pool resource (for example after the backstop TTL).
    Missing,
    /// Terminating, or labeled with another spec.
    Unusable,
    /// Another tenant's name.
    Foreign,
}

enum Created {
    New(Box<Pool>),
    Existing(Box<Pool>, Value),
    Foreign,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct CacheFile {
    #[serde(default)]
    version: u32,
    /// sha256(tenant)[..16] -> spec hash -> pool name.
    #[serde(default)]
    tenants: BTreeMap<String, BTreeMap<String, String>>,
}

struct Inner {
    fleet: FleetClient,
    cfg: AutoPoolConfig,
    resolver: Mutex<Option<Arc<dyn ImageResolver>>>,
    tenant: tokio::sync::OnceCell<String>,
    names: Mutex<HashMap<String, String>>,
    key_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    templates_ok: Mutex<HashSet<String>>,
    touched: Mutex<HashMap<String, Instant>>,
    resolved: Mutex<HashMap<String, (String, Instant)>>,
    auto_gc_started: AtomicBool,
}

/// The auto pool manager. Cheap to clone; clones share caches and locks.
#[derive(Clone)]
pub struct PoolManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PoolManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolManager")
            .field("cfg", &self.inner.cfg)
            .finish_non_exhaustive()
    }
}

/// A claim on a managed pool. While it lives, a heartbeat renews its TTL.
///
/// [`ManagedClaim::release`] deletes the claim; [`ManagedClaim::detach`]
/// keeps it and lets the TTL run; dropping it releases in the background
/// (or detaches, see [`ManagedClaim::set_release_on_drop`]).
pub struct ManagedClaim {
    /// Pool (= namespace) name.
    pub pool: String,
    /// Claim name.
    pub claim: String,
    /// The bound sandbox.
    pub sandbox: BoundSandbox,
    /// This call created the pool.
    pub created_pool: bool,
    /// An existing claim with the requested name was reattached.
    pub reattached: bool,
    /// Time from `acquire` to bound.
    pub bind_time: Duration,
    /// The claim TTL the heartbeat renews.
    pub claim_ttl: Duration,
    lease: Option<Lease>,
    release_on_drop: bool,
}

struct Lease {
    mgr: PoolManager,
    heartbeat: tokio::task::JoinHandle<()>,
    /// Earliest shutdown time (unix s) the heartbeat may renew to; raised
    /// by [`ManagedClaim::extend_until`] so it never shortens a longer
    /// lease the caller asked for.
    floor: Arc<AtomicI64>,
}

impl std::fmt::Debug for ManagedClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedClaim")
            .field("pool", &self.pool)
            .field("claim", &self.claim)
            .field("created_pool", &self.created_pool)
            .field("bind_time", &self.bind_time)
            .finish_non_exhaustive()
    }
}

impl ManagedClaim {
    /// Whether dropping the handle releases the claim (default true).
    pub fn set_release_on_drop(&mut self, release: bool) {
        self.release_on_drop = release;
    }

    /// Records that the caller extended the lease to `until` (unix seconds)
    /// with `keep_alive`: the heartbeat never renews to an earlier time.
    pub fn extend_until(&self, until: i64) {
        if let Some(l) = &self.lease {
            l.floor.fetch_max(until, Ordering::SeqCst);
        }
    }

    /// Whether the heartbeat task is running.
    pub fn heartbeat_active(&self) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|l| !l.heartbeat.is_finished())
    }

    /// Deletes the claim and records the pool's last use.
    pub async fn release(mut self) -> Result<()> {
        let Some(lease) = self.lease.take() else {
            return Ok(());
        };
        lease.heartbeat.abort();
        lease
            .mgr
            .inner
            .fleet
            .release(&self.pool, &self.claim)
            .await?;
        lease.mgr.touch_by_name(&self.pool, true).await;
        Ok(())
    }

    /// Keeps the claim, stops the heartbeat and lets its TTL run.
    pub fn detach(mut self) -> BoundSandbox {
        if let Some(l) = self.lease.take() {
            l.heartbeat.abort();
        }
        self.sandbox.clone()
    }
}

impl Drop for ManagedClaim {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        lease.heartbeat.abort();
        if !self.release_on_drop {
            return;
        }
        let (pool, claim) = (self.pool.clone(), self.claim.clone());
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = lease.mgr.inner.fleet.release(&pool, &claim).await {
                    tracing::warn!(%pool, %claim, error = %e,
                        "failed to release dropped Fleet claim; its TTL will reap it");
                }
                lease.mgr.touch_by_name(&pool, true).await;
            });
        }
    }
}

// ----------------------------------------------------------------- manager

impl PoolManager {
    /// A manager over `fleet`. Performs no I/O until used.
    pub fn new(fleet: FleetClient, cfg: AutoPoolConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                fleet,
                cfg,
                resolver: Mutex::new(None),
                tenant: tokio::sync::OnceCell::new(),
                names: Mutex::default(),
                key_locks: Mutex::default(),
                templates_ok: Mutex::default(),
                touched: Mutex::default(),
                resolved: Mutex::default(),
                auto_gc_started: AtomicBool::new(false),
            }),
        }
    }

    /// Sets the image resolver (tags to digests).
    pub fn with_resolver(self, resolver: Arc<dyn ImageResolver>) -> Self {
        *self.inner.resolver.lock().unwrap() = Some(resolver);
        self
    }

    /// The Fleet client.
    pub fn fleet(&self) -> &FleetClient {
        &self.inner.fleet
    }

    /// The configuration.
    pub fn config(&self) -> &AutoPoolConfig {
        &self.inner.cfg
    }

    /// The caller's tenant (JWT `sub`, or the configured override).
    pub async fn tenant(&self) -> Result<String> {
        self.inner
            .tenant
            .get_or_try_init(|| async {
                if let Some(t) = &self.inner.cfg.tenant {
                    return Ok(t.clone());
                }
                let token = self.inner.fleet.access_token(false).await?;
                Ok(tenant_from_token(&token))
            })
            .await
            .cloned()
    }

    /// Resolves the key's image to a digest (cached for a few minutes).
    pub async fn resolve_key(&self, mut key: PoolSpecKey) -> PoolSpecKey {
        if key.image.contains('@') {
            return key;
        }
        let Some(resolver) = self.inner.resolver.lock().unwrap().clone() else {
            return key;
        };
        if let Some((d, at)) = self.inner.resolved.lock().unwrap().get(&key.image)
            && at.elapsed() < RESOLVE_TTL
        {
            key.image = d.clone();
            return key;
        }
        if let Some(d) = resolver.resolve(&key.image).await {
            self.inner
                .resolved
                .lock()
                .unwrap()
                .insert(key.image.clone(), (d.clone(), Instant::now()));
            key.image = d;
        }
        key
    }

    fn key_lock(&self, hash: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.inner
                .key_locks
                .lock()
                .unwrap()
                .entry(hash.to_string())
                .or_default(),
        )
    }

    /// Claims a sandbox of `key` from its managed pool, creating the pool on
    /// first use, and waits until it is bound.
    pub async fn acquire(&self, key: PoolSpecKey, opts: AcquireOpts) -> Result<ManagedClaim> {
        let started = Instant::now();
        // Parity fields fail before any namespace or pool exists.
        key.pool_spec("cua-auto-check", 0, 1, self.inner.cfg.pool_ttl)
            .validate_parity()?;
        self.maybe_auto_gc();
        let key = self.resolve_key(key).await;
        let hash = key.spec_hash();
        let claim_ttl = opts
            .claim_ttl
            .unwrap_or(self.inner.cfg.claim_ttl)
            .max(Duration::from_secs(1));
        if opts.claim_token.is_some() && !key.claim_secrets {
            return Err(Error::InvalidArgument(
                "claim_token needs a key with claim_secrets (the template must opt in)".into(),
            ));
        }
        let mut labels: HashMap<String, String> = opts.labels.clone().into_iter().collect();
        labels.insert(LABEL_MANAGED_BY.into(), MANAGED_BY.into());
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (pool, created_pool) = self.ensure_pool(&key, &hash, &opts).await?;
            let name = pool.metadata.name.clone();
            let spec = ClaimSpec {
                sandbox_template_ref: pool.spec.sandbox_template_ref.clone(),
                warmpool: None,
                bind_deadline: Some(self.bind_deadline(opts.bind_deadline).as_secs() as u32),
                lifecycle: None,
                ttl_seconds_after_created: Some(claim_ttl.as_secs().max(1) as u32),
                secret_ref: None,
            };
            let claimed = self
                .inner
                .fleet
                .claim(
                    &pool,
                    ClaimOptions {
                        name: opts.name.clone(),
                        spec: Some(spec),
                        ttl_seconds_after_created: None,
                        labels: Some(labels.clone()),
                        claim_token: opts.claim_token.clone(),
                    },
                )
                .await;
            let (claim, created_claim) = match claimed {
                Ok(c) => c,
                // The pool vanished between discovery and claim (GC race or
                // backstop TTL): rediscover once.
                Err(e) if attempt < 2 && (is_status(&e, 404) || is_denied(&e)) => {
                    tracing::debug!(pool = %name, error = %e, "managed pool gone; rediscovering");
                    self.forget(&hash);
                    continue;
                }
                Err(e) => return Err(e),
            };
            // Renew from the start: a cold bind (KEDA from zero, image pull,
            // boot) can take longer than a short claim TTL, and the claim
            // would be reaped the moment it binds.
            let floor = Arc::new(AtomicI64::new(0));
            let heartbeat =
                self.heartbeat(&name, &claim.metadata.name, claim_ttl, Arc::clone(&floor));
            let bound = match self.wait_bound(&claim, opts.bind_deadline).await {
                Ok(b)
                    if b.namespace == claim.metadata.namespace
                        && b.claim == claim.metadata.name =>
                {
                    b
                }
                Ok(_) => {
                    heartbeat.abort();
                    return Err(Error::InvalidArgument(
                        "Fleet returned a sandbox bound to a different claim".into(),
                    ));
                }
                Err(e) => {
                    heartbeat.abort();
                    if created_claim {
                        let _ = self.inner.fleet.release(&name, &claim.metadata.name).await;
                    }
                    return Err(e);
                }
            };
            // A new claim's token arrives asynchronously after Bound; hand
            // the sandbox out only once its driver has it.
            if let Some(token) = opts.claim_token.as_deref().filter(|_| created_claim)
                && let Err(e) = self
                    .inner
                    .fleet
                    .await_claim_secrets(&bound, token, &key.runtime)
                    .await
            {
                heartbeat.abort();
                let _ = self.inner.fleet.release(&name, &claim.metadata.name).await;
                return Err(e);
            }
            return Ok(self.lease_with(
                bound,
                claim_ttl,
                created_pool,
                !created_claim,
                started,
                heartbeat,
                floor,
            ));
        }
    }

    fn bind_deadline(&self, requested: Option<Duration>) -> Duration {
        self.inner
            .cfg
            .bind_deadline
            .max(requested.unwrap_or_default())
            .max(Duration::from_secs(MIN_BIND_DEADLINE_SECS))
    }

    /// Waits for the claim to bind for up to `bindDeadline` + 1 min, beyond
    /// the client's own poll budget (a cold start can take several minutes).
    async fn wait_bound(
        &self,
        claim: &crate::Claim,
        requested: Option<Duration>,
    ) -> Result<BoundSandbox> {
        let deadline = Instant::now() + self.bind_deadline(requested) + Duration::from_secs(60);
        loop {
            match self.inner.fleet.wait_claim(claim).await {
                Err(Error::Sdk(SdkError::ClaimTimeout)) if Instant::now() < deadline => {
                    tracing::debug!(claim = %claim.metadata.name, "claim still Pending (cold start)");
                }
                other => return other,
            }
        }
    }

    /// Takes over an already bound claim (for example a named sandbox
    /// reattached from its state file): starts the heartbeat. Dropping the
    /// returned handle detaches.
    pub fn adopt(&self, sandbox: BoundSandbox, claim_ttl: Option<Duration>) -> ManagedClaim {
        let ttl = claim_ttl.unwrap_or(self.inner.cfg.claim_ttl);
        let mut c = self.lease(sandbox, ttl, false, true, Instant::now());
        c.release_on_drop = false;
        c
    }

    fn lease(
        &self,
        bound: BoundSandbox,
        claim_ttl: Duration,
        created_pool: bool,
        reattached: bool,
        started: Instant,
    ) -> ManagedClaim {
        let floor = Arc::new(AtomicI64::new(0));
        let heartbeat = self.heartbeat(
            &bound.namespace,
            &bound.claim,
            claim_ttl,
            Arc::clone(&floor),
        );
        self.lease_with(
            bound,
            claim_ttl,
            created_pool,
            reattached,
            started,
            heartbeat,
            floor,
        )
    }

    /// Renews `ns/claim` to now + `claim_ttl` every `claim_ttl / 3` until
    /// aborted or the claim is gone.
    fn heartbeat(
        &self,
        ns: &str,
        claim: &str,
        claim_ttl: Duration,
        floor: Arc<AtomicI64>,
    ) -> tokio::task::JoinHandle<()> {
        let every = self.inner.cfg.heartbeat_period(claim_ttl);
        let sdk = self.inner.fleet.sdk();
        let clock = Arc::clone(&self.inner.cfg.clock);
        let (ns, name) = (ns.to_string(), claim.to_string());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                // Renew to now + TTL (`keep_alive`), on the manager's clock.
                let until =
                    (clock() + claim_ttl.as_secs() as i64).max(floor.load(Ordering::SeqCst));
                let until = humantime::format_rfc3339_seconds(
                    UNIX_EPOCH + Duration::from_secs(until.max(0) as u64),
                )
                .to_string();
                let renewed = Arc::clone(&sdk)
                    .renew_claim(crate::pool::claim_stub(&ns, &name), until)
                    .await
                    .map_err(Error::from);
                match renewed {
                    Ok(_) => {}
                    Err(e) if is_status(&e, 404) => {
                        tracing::info!(claim = %name, "Fleet claim is gone; heartbeat stops");
                        break;
                    }
                    Err(e) => tracing::warn!(claim = %name, error = %e, "claim heartbeat failed"),
                }
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn lease_with(
        &self,
        bound: BoundSandbox,
        claim_ttl: Duration,
        created_pool: bool,
        reattached: bool,
        started: Instant,
        heartbeat: tokio::task::JoinHandle<()>,
        floor: Arc<AtomicI64>,
    ) -> ManagedClaim {
        ManagedClaim {
            pool: bound.namespace.clone(),
            claim: bound.claim.clone(),
            sandbox: bound,
            created_pool,
            reattached,
            bind_time: started.elapsed(),
            claim_ttl,
            lease: Some(Lease {
                mgr: self.clone(),
                heartbeat,
                floor,
            }),
            release_on_drop: true,
        }
    }

    // ------------------------------------------------------ pool discovery

    /// Finds or creates the pool for `key`. Returns it and whether this call
    /// created it. Never rewrites an existing pool's replicas or spec.
    pub async fn ensure_pool(
        &self,
        key: &PoolSpecKey,
        hash: &str,
        opts: &AcquireOpts,
    ) -> Result<(Pool, bool)> {
        let lock = self.key_lock(hash);
        let _guard = lock.lock().await;
        let tenant = self.tenant().await?;
        let candidates = candidate_names(&tenant, hash);
        let max = opts.max_pool_size.unwrap_or(self.inner.cfg.max_pool_size);
        crate::check_pool_size("max_pool_size", max)?;
        crate::check_cloud_size(key.cpu, key.memory_mb)?;

        // 1. Cached mapping, verified against the server.
        if let Some(name) = self.cached_name(&tenant, hash) {
            match self.probe(&name, hash, &candidates).await? {
                Probe::Usable(pool, raw) => {
                    self.on_reuse(&name, key, &raw, max, opts.registry_credentials.as_ref())
                        .await?;
                    return Ok((*pool, false));
                }
                _ => self.forget(hash),
            }
        }

        // 2. Discovery: the tenant's namespaces are authoritative.
        let mine = self.my_namespaces().await?;
        for name in candidates.iter().filter(|n| mine.contains_key(*n)) {
            if let Probe::Usable(pool, raw) = self.probe(name, hash, &candidates).await? {
                self.remember(&tenant, hash, name);
                self.on_reuse(name, key, &raw, max, opts.registry_credentials.as_ref())
                    .await?;
                return Ok((*pool, false));
            }
        }
        // A pool created for the same spec under another candidate
        // derivation (for example another key of the same account).
        for name in mine
            .keys()
            .filter(|n| n.starts_with(AUTO_POOL_PREFIX) && !candidates.contains(n))
        {
            let Ok(Some(raw)) = self.inner.fleet.get_pool_json(name).await else {
                continue;
            };
            if label(&raw, LABEL_SPEC_HASH) == Some(hash_label(hash)) && !terminating(&raw) {
                let pool: Pool = to_pool(&raw)?;
                self.remember(&tenant, hash, name);
                self.on_reuse(name, key, &raw, max, opts.registry_credentials.as_ref())
                    .await?;
                return Ok((pool, false));
            }
        }

        // 3. Create at the first name that is free or ours without a pool.
        let warm = opts.warm.unwrap_or(self.inner.cfg.warm);
        for name in &candidates {
            if mine.contains_key(name) {
                // Re-probe: a concurrent creator may have finished since
                // the discovery pass.
                match self.probe(name, hash, &candidates).await? {
                    Probe::Missing => {}
                    Probe::Usable(pool, raw) => {
                        self.remember(&tenant, hash, name);
                        self.on_reuse(name, key, &raw, max, opts.registry_credentials.as_ref())
                            .await?;
                        return Ok((*pool, false));
                    }
                    Probe::Unusable | Probe::Foreign => continue,
                }
            }
            let mut spec = key.pool_spec(name, u32::from(warm), max, self.inner.cfg.pool_ttl);
            spec.registry_credentials = opts.registry_credentials.clone();
            // Fleet reaps an idle pool itself (#7886); the client GC stays
            // as the backstop (the operator may run in dry-run).
            spec.idle_ttl_seconds = self
                .inner
                .cfg
                .idle_gc
                .map(|d| d.as_secs().min(u64::from(u32::MAX)) as u32);
            match self.create_at(&spec, hash).await? {
                Created::New(pool) => {
                    self.remember(&tenant, hash, name);
                    return Ok((*pool, true));
                }
                Created::Existing(pool, raw) => {
                    self.remember(&tenant, hash, name);
                    self.on_reuse(name, key, &raw, max, opts.registry_credentials.as_ref())
                        .await?;
                    return Ok((*pool, false));
                }
                Created::Foreign => {
                    tracing::debug!(pool = %name, "pool name held by another account; probing next");
                }
            }
        }
        Err(Error::InvalidArgument(format!(
            "no free managed pool name after {MAX_NAME_PROBES} candidates ({}...)",
            candidates[0]
        )))
    }

    async fn my_namespaces(&self) -> Result<BTreeMap<String, Namespace>> {
        Ok(self
            .inner
            .fleet
            .sdk()
            .list_namespaces()
            .await?
            .into_iter()
            .map(|n| (n.name.clone(), n))
            .collect())
    }

    async fn probe(&self, name: &str, hash: &str, candidates: &[String]) -> Result<Probe> {
        match self.inner.fleet.get_pool_json(name).await {
            Ok(Some(raw)) => {
                if terminating(&raw) {
                    return Ok(Probe::Unusable);
                }
                match label(&raw, LABEL_SPEC_HASH) {
                    Some(h) if h != hash_label(hash) => return Ok(Probe::Unusable),
                    // An unlabeled pool is ours only by its deterministic name.
                    None if !candidates.iter().any(|c| c == name) => return Ok(Probe::Unusable),
                    _ => {}
                }
                let pool: Pool = to_pool(&raw)?;
                Ok(Probe::Usable(Box::new(pool), raw))
            }
            Ok(None) => Ok(Probe::Missing),
            Err(e) if is_denied(&e) => Ok(Probe::Foreign),
            Err(e) => Err(e),
        }
    }

    async fn create_at(&self, spec: &PoolSpec, hash: &str) -> Result<Created> {
        let name = spec.name.clone();
        let sdk = self.inner.fleet.sdk();
        // Create the namespace first: the SDK's create_pool deletes a
        // namespace *it* created when the pool POST fails, which would wipe
        // a concurrent creator's pool. With the namespace already present
        // it never rolls back.
        let ns_created = match Arc::clone(&sdk).create_namespace(name.clone()).await {
            Ok(_) => true,
            Err(SdkError::Status { status: 409, .. }) => false,
            Err(SdkError::Status { status: 403, .. }) => return Ok(Created::Foreign),
            Err(e) => return Err(e.into()),
        };
        let mut delay = Duration::from_millis(250);
        for attempt in 0..=ADOPTION_RETRIES {
            match Arc::clone(&sdk).create_pool(spec.pool_request()).await {
                Ok(pool) => {
                    let labels = self.managed_labels(hash);
                    // Labels, and the #7886 lifecycle fields the typed
                    // create cannot carry yet, in one patch.
                    let mut patch = json!({"metadata": {"labels": labels}});
                    let lifecycle = spec.parts().1.lifecycle_patch();
                    if !lifecycle.is_empty() {
                        patch["spec"] = Value::Object(lifecycle);
                    }
                    if let Err(e) = self.inner.fleet.patch_pool_json(&name, patch).await {
                        tracing::warn!(pool = %name, error = %e,
                            "could not label managed pool; discovery falls back to its name");
                    }
                    if let Err(e) = self.ensure_template(&name, spec).await {
                        let _ = Arc::clone(&sdk).delete_pool(pool).await;
                        return Err(e);
                    }
                    return Ok(Created::New(Box::new(pool)));
                }
                Err(SdkError::Status { status: 409, .. }) => {
                    // Someone (another process of this account) won the race.
                    for _ in 0..10 {
                        if let Some(raw) = self.inner.fleet.get_pool_json(&name).await? {
                            let pool: Pool = to_pool(&raw)?;
                            return Ok(Created::Existing(Box::new(pool), raw));
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    return Err(Error::Timeout(format!(
                        "pool {name} exists but could not be read"
                    )));
                }
                Err(
                    e @ (SdkError::PoolAccessDenied { .. } | SdkError::Status { status: 403, .. }),
                ) => {
                    // 403 right after this account created the namespace (or
                    // while it shows in our namespace list) is Capsule
                    // adoption lag; otherwise the name is someone else's.
                    let ours = ns_created || self.my_namespaces().await?.contains_key(&name);
                    if !ours {
                        return Ok(Created::Foreign);
                    }
                    if attempt == ADOPTION_RETRIES {
                        return Err(e.into());
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(4));
                }
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    async fn ensure_template(&self, name: &str, spec: &PoolSpec) -> Result<()> {
        // The pull secret first, so no replica pulls without it. Written
        // once per process and password (a rotated password is rewritten).
        if let (Some(secret), Some(creds)) = (&spec.image_pull_secret, &spec.registry_credentials) {
            let memo = format!(
                "{name}/{secret}/{}",
                hex::encode(Sha256::digest(creds.password.as_bytes()))
            );
            if !written_secrets().lock().unwrap().contains(&memo) {
                self.inner.fleet.write_pull_secret(spec).await?;
                written_secrets().lock().unwrap().insert(memo);
            }
        }
        if self.inner.templates_ok.lock().unwrap().contains(name) {
            return Ok(());
        }
        // The one template mapping; an existing template is left as it is
        // (a managed pool's spec never changes: it is its name).
        self.inner
            .fleet
            .create_template_json(spec.template_json()?)
            .await?;
        self.inner
            .templates_ok
            .lock()
            .unwrap()
            .insert(name.to_string());
        Ok(())
    }

    /// Reuse: make sure the template exists, label an unlabeled pool, raise
    /// a lower autoscaling ceiling. Never writes `replicas`.
    async fn on_reuse(
        &self,
        name: &str,
        key: &PoolSpecKey,
        raw: &Value,
        max: u32,
        creds: Option<&crate::RegistryCredentials>,
    ) -> Result<()> {
        let mut spec = key.pool_spec(name, 0, max, self.inner.cfg.pool_ttl);
        spec.registry_credentials = creds.cloned();
        self.ensure_template(name, &spec).await?;
        let mut patch = json!({});
        if label(raw, LABEL_SPEC_HASH).is_none() {
            patch["metadata"]["labels"] = json!(self.managed_labels(&key.spec_hash()));
        }
        let current_max = raw["spec"]["autoscaling"]["maxPoolSize"].as_u64();
        if current_max.is_some_and(|m| m < u64::from(max)) {
            patch["spec"]["autoscaling"]["maxPoolSize"] = json!(max);
        }
        if patch.as_object().is_some_and(|o| !o.is_empty())
            && let Err(e) = self.inner.fleet.patch_pool_json(name, patch).await
        {
            tracing::warn!(pool = %name, error = %e, "could not update managed pool metadata");
        }
        self.touch(name, raw, false).await;
        Ok(())
    }

    fn managed_labels(&self, hash: &str) -> BTreeMap<String, String> {
        [
            (LABEL_MANAGED_BY.to_string(), MANAGED_BY.to_string()),
            (LABEL_SPEC_HASH.to_string(), hash_label(hash).to_string()),
            (
                LABEL_LAST_USED.to_string(),
                self.inner.cfg.now().to_string(),
            ),
        ]
        .into()
    }

    /// Records the pool's last use and renews its backstop TTL when it is
    /// within the renewal window. Best effort; throttled per pool unless
    /// `force`.
    async fn touch(&self, name: &str, raw: &Value, force: bool) {
        {
            let mut touched = self.inner.touched.lock().unwrap();
            if !force && touched.get(name).is_some_and(|t| t.elapsed() < TOUCH_EVERY) {
                return;
            }
            touched.insert(name.to_string(), Instant::now());
        }
        let now = self.inner.cfg.now();
        let mut patch = json!({"metadata": {"labels": {LABEL_LAST_USED: now.to_string()}}});
        let created = parse_time(raw["metadata"]["creationTimestamp"].as_str());
        if let Some(created) = created {
            let age = (now - created).max(0);
            let ttl = raw["spec"]["ttlSecondsAfterCreated"].as_i64();
            let window = self.inner.cfg.pool_ttl_renew_window.as_secs() as i64;
            if ttl.is_none_or(|t| t - age < window) {
                let renewed = age + self.inner.cfg.pool_ttl.as_secs() as i64;
                patch["spec"] = json!({"ttlSecondsAfterCreated": renewed.min(u32::MAX as i64)});
            }
        }
        if let Err(e) = self.inner.fleet.patch_pool_json(name, patch).await {
            tracing::debug!(pool = %name, error = %e, "could not record pool use");
        }
    }

    async fn touch_by_name(&self, name: &str, force: bool) {
        match self.inner.fleet.get_pool_json(name).await {
            Ok(Some(raw)) => self.touch(name, &raw, force).await,
            Ok(None) => {}
            Err(e) => tracing::debug!(pool = %name, error = %e, "could not read pool"),
        }
    }

    // --------------------------------------------------------------- cache

    fn tenant_key(tenant: &str) -> String {
        hex::encode(Sha256::digest(tenant.as_bytes()))[..16].to_string()
    }

    fn cache_path(&self) -> PathBuf {
        self.inner.cfg.home.join(CACHE_FILE)
    }

    fn cached_name(&self, tenant: &str, hash: &str) -> Option<String> {
        if let Some(n) = self.inner.names.lock().unwrap().get(hash) {
            return Some(n.clone());
        }
        let file: CacheFile = std::fs::read(self.cache_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())?;
        file.tenants
            .get(&Self::tenant_key(tenant))?
            .get(hash)
            .cloned()
    }

    fn remember(&self, tenant: &str, hash: &str, name: &str) {
        self.inner
            .names
            .lock()
            .unwrap()
            .insert(hash.into(), name.into());
        let path = self.cache_path();
        let mut file: CacheFile = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        file.version = 1;
        file.tenants
            .entry(Self::tenant_key(tenant))
            .or_default()
            .insert(hash.into(), name.into());
        if let Err(e) = write_private(&path, &serde_json::to_vec_pretty(&file).unwrap_or_default())
        {
            tracing::debug!(error = %e, "could not write the Fleet pool cache");
        }
    }

    fn forget(&self, hash: &str) {
        self.inner.names.lock().unwrap().remove(hash);
    }

    // ------------------------------------------------------------ list/gc

    async fn managed_namespaces(&self) -> Result<Vec<Namespace>> {
        Ok(self
            .my_namespaces()
            .await?
            .into_values()
            .filter(|n| {
                n.name.starts_with(AUTO_POOL_PREFIX) || n.name.starts_with(EPHEMERAL_POOL_PREFIX)
            })
            .collect())
    }

    /// This account's `cua-auto-*` / `cua-eph-*` pools.
    pub async fn list(&self) -> Result<Vec<ManagedPoolInfo>> {
        self.maybe_auto_gc();
        let mut out = vec![];
        for ns in self.managed_namespaces().await? {
            let raw = match self.inner.fleet.get_pool_json(&ns.name).await {
                Ok(Some(raw)) => raw,
                Ok(None) => continue,
                // A namespace being deleted (or not yet adopted) answers 403.
                Err(e) if is_denied(&e) => continue,
                Err(e) => return Err(e),
            };
            let claims = self
                .inner
                .fleet
                .list_claims(&ns.name)
                .await
                .unwrap_or_default();
            let bound = claims.iter().filter(|c| phase(c) == "Bound").count() as u32;
            let image = self
                .inner
                .fleet
                .sdk()
                .get_template(ns.name.clone(), ns.name.clone())
                .await
                .ok()
                .map(|t| t.spec.vm_template.container_disk_image);
            out.push(pool_info(&ns.name, &raw, claims.len() as u32, bound, image));
        }
        Ok(out)
    }

    /// Deletes idle managed pools and stuck managed claims.
    ///
    /// - A managed claim (label [`LABEL_MANAGED_BY`]) that is Pending or
    ///   Failed past its TTL is deleted (the operator only reaps Bound
    ///   claims, and Pending/Failed ones keep counting as KEDA demand).
    /// - A `cua-auto-*` / `cua-eph-*` pool with no remaining claims whose
    ///   last use (label, else creation) is at least `idle_after` ago is
    ///   deleted with its namespace. The claim list and label are re-read
    ///   right before deleting.
    /// - A leftover namespace of ours with no pool and no claims, older than
    ///   `idle_after`, is deleted.
    pub async fn gc(&self, idle_after: Duration) -> Result<GcReport> {
        self.gc_scoped(idle_after, None).await
    }

    /// [`PoolManager::gc`] restricted to the named pools (tests, `cua fleet
    /// pools gc --pool`).
    pub async fn gc_pools(&self, idle_after: Duration, only: &[String]) -> Result<GcReport> {
        self.gc_scoped(idle_after, Some(only)).await
    }

    async fn gc_scoped(&self, idle_after: Duration, only: Option<&[String]>) -> Result<GcReport> {
        let now = self.inner.cfg.now();
        let idle = idle_after.as_secs() as i64;
        let mut report = GcReport::default();
        for ns in self.managed_namespaces().await? {
            if only.is_some_and(|o| !o.contains(&ns.name)) {
                continue;
            }
            let name = ns.name.clone();
            let raw = match self.inner.fleet.get_pool_json(&name).await {
                Ok(r) => r,
                Err(e) => {
                    report.errors.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let claims = match self.inner.fleet.list_claims(&name).await {
                Ok(c) => c,
                Err(e) => {
                    report.errors.push(format!("{name}: list claims: {e}"));
                    continue;
                }
            };
            let mut live = 0usize;
            for c in &claims {
                if self.is_stuck(c, now) {
                    match self.inner.fleet.release(&name, &c.metadata.name).await {
                        Ok(()) => report
                            .deleted_claims
                            .push(format!("{name}/{}", c.metadata.name)),
                        Err(e) => {
                            live += 1;
                            report
                                .errors
                                .push(format!("{name}/{}: {e}", c.metadata.name));
                        }
                    }
                } else {
                    live += 1;
                }
            }
            let Some(raw) = raw else {
                let created = parse_time(Some(&ns.created_at));
                if live == 0 && created.is_some_and(|c| now - c >= idle) {
                    match self.inner.fleet.sdk().delete_namespace(name.clone()).await {
                        Ok(()) => report.deleted_namespaces.push(name),
                        Err(e) => report.errors.push(format!("{name}: {e}")),
                    }
                } else {
                    report.kept.push(name);
                }
                continue;
            };
            if live > 0 || terminating(&raw) || now - last_used(&raw).unwrap_or(now) < idle {
                report.kept.push(name);
                continue;
            }
            // Re-check right before deleting: a concurrent acquire patches
            // last-used before it claims.
            let fresh = match self.inner.fleet.get_pool_json(&name).await {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(e) => {
                    report.errors.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let still_idle = now - last_used(&fresh).unwrap_or(now) >= idle
                && self
                    .inner
                    .fleet
                    .list_claims(&name)
                    .await
                    .map(|c| c.iter().all(|c| self.is_stuck(c, now)))
                    .unwrap_or(false);
            if !still_idle {
                report.kept.push(name);
                continue;
            }
            let pool: Pool = match to_pool(&fresh) {
                Ok(p) => p,
                Err(e) => {
                    report.errors.push(format!("{name}: {e}"));
                    continue;
                }
            };
            match self.inner.fleet.sdk().delete_pool(pool).await {
                Ok(()) => {
                    self.inner.templates_ok.lock().unwrap().remove(&name);
                    self.inner.names.lock().unwrap().retain(|_, v| *v != name);
                    report.deleted_pools.push(name);
                }
                Err(e) => report.errors.push(format!("{name}: {e}")),
            }
        }
        Ok(report)
    }

    fn is_stuck(&self, c: &crate::Claim, now: i64) -> bool {
        let managed = c
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(LABEL_MANAGED_BY))
            .is_some_and(|v| v == MANAGED_BY);
        if !managed || !matches!(phase(c), "Pending" | "Failed") {
            return false;
        }
        let ttl = c
            .spec
            .ttl_seconds_after_created
            .map(i64::from)
            .unwrap_or(self.inner.cfg.claim_ttl.as_secs() as i64);
        parse_time(c.metadata.creation_timestamp.as_deref()).is_some_and(|t| now - t > ttl)
    }

    /// Runs [`PoolManager::gc`] with the configured idle threshold when the
    /// machine-wide stamp is older than [`AutoPoolConfig::auto_gc_every`]
    /// and no other process holds the lock. Returns `None` when skipped.
    pub async fn gc_if_due(&self) -> Option<GcReport> {
        let idle = self.inner.cfg.idle_gc?;
        let home = self.inner.cfg.home.clone();
        let now = self.inner.cfg.now();
        let every = self.inner.cfg.auto_gc_every.as_secs() as i64;
        let stamp = home.join(GC_STAMP_FILE);
        let last = std::fs::read_to_string(&stamp)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok());
        if last.is_some_and(|t| now - t < every) {
            return None;
        }
        let _lock = GcLock::acquire(&home.join(GC_LOCK_FILE))?;
        // Re-read under the lock: another process may have just run.
        let last = std::fs::read_to_string(&stamp)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok());
        if last.is_some_and(|t| now - t < every) {
            return None;
        }
        let _ = write_private(&stamp, now.to_string().as_bytes());
        match self.gc(idle).await {
            Ok(r) => {
                if !r.deleted_pools.is_empty() || !r.deleted_claims.is_empty() {
                    tracing::info!(pools = ?r.deleted_pools, claims = ?r.deleted_claims,
                        "Fleet idle pool GC");
                }
                Some(r)
            }
            Err(e) => {
                tracing::debug!(error = %e, "Fleet idle pool GC failed");
                None
            }
        }
    }

    fn maybe_auto_gc(&self) {
        if self.inner.cfg.idle_gc.is_none()
            || self.inner.auto_gc_started.swap(true, Ordering::SeqCst)
        {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let me = self.clone();
            handle.spawn(async move {
                me.gc_if_due().await;
            });
        }
    }
}

fn to_pool(raw: &Value) -> Result<Pool> {
    serde_json::from_value(raw.clone()).map_err(|e| {
        SdkError::Body {
            reason: format!("warm pool: {e}"),
        }
        .into()
    })
}

fn label<'a>(raw: &'a Value, key: &str) -> Option<&'a str> {
    raw["metadata"]["labels"][key].as_str()
}

fn terminating(raw: &Value) -> bool {
    raw["metadata"]["deletionTimestamp"].is_string()
}

fn last_used(raw: &Value) -> Option<i64> {
    label(raw, LABEL_LAST_USED)
        .and_then(|v| v.parse().ok())
        .or_else(|| parse_time(raw["metadata"]["creationTimestamp"].as_str()))
}

fn phase(c: &crate::Claim) -> &str {
    c.status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Pending")
}

fn pool_info(
    name: &str,
    raw: &Value,
    claims: u32,
    bound_claims: u32,
    image: Option<String>,
) -> ManagedPoolInfo {
    let created = parse_time(raw["metadata"]["creationTimestamp"].as_str());
    let ttl = raw["spec"]["ttlSecondsAfterCreated"].as_i64();
    ManagedPoolInfo {
        name: name.into(),
        managed: label(raw, LABEL_MANAGED_BY) == Some(MANAGED_BY),
        spec_hash: label(raw, LABEL_SPEC_HASH).map(str::to_string),
        image,
        replicas: raw["spec"]["replicas"].as_u64().unwrap_or(0) as u32,
        ready_replicas: raw["status"]["readyReplicas"].as_u64().map(|r| r as u32),
        max_pool_size: raw["spec"]["autoscaling"]["maxPoolSize"]
            .as_u64()
            .map(|r| r as u32),
        claims,
        bound_claims,
        last_used: last_used(raw),
        created,
        expires_at: created.zip(ttl).map(|(c, t)| c + t),
        terminating: terminating(raw),
    }
}

/// Writes `bytes` atomically with mode 0600 (directory 0700).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // A test must never write the real ~/.cua pool cache.
    cua_home::guard_write(path)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        use std::io::Write as _;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, path)
}

/// A machine-wide lock file (create-exclusive; stale after 15 minutes).
struct GcLock(PathBuf);

impl GcLock {
    fn acquire(path: &Path) -> Option<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    use std::io::Write as _;
                    let _ = write!(f, "{}", std::process::id());
                    return Some(Self(path.to_path_buf()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|age| age > GC_LOCK_STALE);
                    if !stale {
                        return None;
                    }
                    let _ = std::fs::remove_file(path);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for GcLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_matches_rfc4648_lowercase() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "my");
        assert_eq!(base32(b"foobar"), "mzxw6ytboi");
    }

    #[test]
    fn names_are_dns_labels_and_tenant_scoped() {
        let k = PoolSpecKey::new("public.ecr.aws/x/y@sha256:abc");
        let h = k.spec_hash();
        let a = candidate_names("alice", &h);
        let b = candidate_names("bob", &h);
        assert_eq!(a.len(), MAX_NAME_PROBES);
        assert_ne!(a[0], b[0]);
        assert_eq!(a[0].len(), AUTO_POOL_PREFIX.len() + 16);
        assert_eq!(a[1], format!("{}-2", a[0]));
        for n in a.iter().chain(&b) {
            cyclops_sdk::validate_dns_label(n).unwrap();
        }
        assert_eq!(candidate_names("alice", &h), a, "deterministic");
    }

    #[test]
    fn spec_hash_covers_every_field() {
        let base = PoolSpecKey::new("img@sha256:1");
        let variants = [
            PoolSpecKey::new("img@sha256:2"),
            base.clone().runtime(RuntimeKind::Gvisor),
            base.clone().resources(Some(4), None),
            base.clone().resources(None, Some(8192)),
            base.clone().services([("env", 3211u16), ("vnc", 5900)]),
            PoolSpecKey {
                efi: true,
                ..base.clone()
            },
            PoolSpecKey {
                readiness_tcp_port: Some(8000),
                ..base.clone()
            },
            PoolSpecKey {
                command: Some(vec!["/init".into()]),
                ..base.clone()
            },
            PoolSpecKey {
                env: [("K".to_string(), "v".to_string())].into(),
                ..base.clone()
            },
        ];
        let mut seen = HashSet::from([base.spec_hash()]);
        for v in variants {
            assert!(seen.insert(v.spec_hash()), "{v:?} collides");
        }
        assert_eq!(base.spec_hash(), base.clone().spec_hash());
        assert_eq!(base.spec_hash().len(), 64);
    }

    #[test]
    fn spec_hash_of_specs_without_env_is_unchanged() {
        // The v1 encoding before `env` existed: existing managed pools keep
        // their names.
        let k = PoolSpecKey::new("img@sha256:1");
        assert!(
            k.canonical().ends_with(r#",["command",null]]"#),
            "{}",
            k.canonical()
        );
    }

    #[test]
    fn pool_spec_autoscales_from_zero() {
        let k = PoolSpecKey::new("img");
        let s = k.pool_spec("cua-auto-x", 0, 10, Duration::from_secs(60));
        assert_eq!(s.replicas, 0);
        let a = s.autoscaling.unwrap();
        assert_eq!(
            (a.min_pool_size, a.initial_pool_size, a.max_pool_size),
            (Some(0), Some(0), Some(10))
        );
        assert_eq!(s.ttl_seconds_after_created, Some(60));
        let s = k.pool_spec("cua-auto-x", 1, 0, Duration::from_secs(60));
        assert_eq!(s.replicas, 1);
        assert_eq!(s.autoscaling.unwrap().max_pool_size, Some(1));
    }

    #[test]
    fn tenant_from_jwt_sub_or_token_hash() {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let jwt = format!(
            "{}.{}.sig",
            b64.encode("{}"),
            b64.encode(r#"{"sub":"user-123"}"#)
        );
        assert_eq!(tenant_from_token(&jwt), "user-123");
        let t = tenant_from_token("opaque");
        assert!(t.starts_with("token:"));
        assert_eq!(t, tenant_from_token("opaque"));
        assert_ne!(t, tenant_from_token("other"));
    }

    #[test]
    fn config_env_overrides() {
        let env: HashMap<&str, &str> = [
            ("CUA_FLEET_MAX_POOL_SIZE", "25"),
            ("CUA_FLEET_CLAIM_TTL", "3m"),
            ("CUA_FLEET_POOL_IDLE_GC", "off"),
            ("CUA_FLEET_WARM", "true"),
            ("CUA_HOME", "/tmp/cua-home-test"),
        ]
        .into();
        let c = AutoPoolConfig::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert_eq!(c.max_pool_size, 25);
        assert_eq!(c.claim_ttl, Duration::from_secs(180));
        assert_eq!(c.idle_gc, None);
        assert!(c.warm);
        assert_eq!(c.home, PathBuf::from("/tmp/cua-home-test"));
        let c = AutoPoolConfig::from_lookup(|k| {
            (k == "CUA_FLEET_CLAIM_TTL" || k == "CUA_FLEET_POOL_IDLE_GC").then(|| "90".into())
        });
        assert_eq!(c.claim_ttl, Duration::from_secs(90));
        assert_eq!(c.idle_gc, Some(Duration::from_secs(90)));
        let d = AutoPoolConfig::from_lookup(|_| None);
        assert_eq!(d.max_pool_size, 10);
        assert_eq!(d.claim_ttl, Duration::from_secs(900));
        assert_eq!(d.bind_deadline, Duration::from_secs(900));
        assert_eq!(d.pool_ttl, Duration::from_secs(7 * 86400));
        // The idle GC default matches the Python SDK and the docs: 30 min.
        assert_eq!(d.idle_gc, Some(Duration::from_secs(30 * 60)));
        assert!(!d.warm);
    }

    #[test]
    fn gc_lock_is_exclusive_and_released() {
        let dir = std::env::temp_dir().join(format!("cua-gc-lock-{}", std::process::id()));
        let path = dir.join(GC_LOCK_FILE);
        let a = GcLock::acquire(&path).expect("first");
        assert!(GcLock::acquire(&path).is_none(), "second holder refused");
        drop(a);
        assert!(GcLock::acquire(&path).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
