//! `Fleet`: pools, templates, claims and images (cyclops-sdk via cua-fleet).
//!
//! Resources cross as small records plus their full JSON (`json`), so the
//! Fleet schema can evolve without regenerating every binding.

use super::run;
use super::sandbox::{Container, ReadinessProbe, RegistrySecret};
use crate::{CuaError, Result};
use cua_fleet::{ClaimOptions, FleetClient};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// What a sandbox runs: the one model behind `Sandbox.create`, managed
/// pools and [`Fleet::apply`]. Unset fields keep Fleet's defaults (and are
/// not compared by [`Fleet::check_pool_spec`]).
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SandboxSpec {
    /// Image (a container image runs on gVisor, a containerDisk on
    /// KubeVirt).
    #[uniffi(default = "")]
    pub image: String,
    /// argv replacing the image ENTRYPOINT.
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Arguments replacing the image CMD.
    #[uniffi(default = None)]
    pub args: Option<Vec<String>>,
    /// Plain environment variables (not secrets).
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Named services (name → guest port).
    #[uniffi(default)]
    pub services: HashMap<String, u16>,
    /// Readiness probe (`port`, or a declared `service`; `http_path` for
    /// HTTP). A replica binds a claim only once it passes.
    #[uniffi(default = None)]
    pub readiness: Option<ReadinessProbe>,
    /// vCPUs.
    #[uniffi(default = None)]
    pub cpu: Option<u32>,
    /// Memory (MiB).
    #[uniffi(default = None)]
    pub memory_mb: Option<u32>,
    /// UEFI firmware (Windows images).
    #[uniffi(default = false)]
    pub efi: bool,
    /// Extra containers sharing the sandbox's network namespace.
    #[uniffi(default = [])]
    pub sidecars: Vec<Container>,
    /// Credentials for a private image: stored as the pool's
    /// `cua-registry-*` pull Secret.
    #[uniffi(default = None)]
    pub registry_secret: Option<RegistrySecret>,
    /// An existing `cua-registry-*` pull Secret in the pool namespace
    /// (instead of `registry_secret`).
    #[uniffi(default = None)]
    pub registry_secret_name: Option<String>,
    /// `Legacy` or `Run` (`vmTemplate.processMode`).
    #[uniffi(default = None)]
    pub process_mode: Option<String>,
    /// Per-claim secrets: claims may carry a token delivered at
    /// `/run/cua/env-token`.
    #[uniffi(default = false)]
    pub claim_secrets: bool,
}

/// How a pool keeps capacity for a [`SandboxSpec`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct PoolOptions {
    /// `kubevirt` or `gvisor`. Unset: from the image.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Replicas of a pool without autoscaling (default 1).
    #[uniffi(default = None)]
    pub replicas: Option<u32>,
    /// Keep one sandbox warm (`minPoolSize: 1`).
    #[uniffi(default = None)]
    pub warm: Option<bool>,
    /// Autoscaling floor.
    #[uniffi(default = None)]
    pub min_pool_size: Option<u32>,
    /// Autoscaling ceiling.
    #[uniffi(default = None)]
    pub max_pool_size: Option<u32>,
    /// Delete the pool after this many seconds without claims.
    #[uniffi(default = None)]
    pub idle_ttl_seconds: Option<u32>,
    /// `Retain` or `Cascade`: what TTL expiry deletes.
    #[uniffi(default = None)]
    pub ttl_policy: Option<String>,
    /// Pool creation-age TTL (seconds).
    #[uniffi(default = None)]
    pub pool_ttl_seconds: Option<u32>,
    /// Default TTL (seconds) of claims made on the pool through this SDK.
    #[uniffi(default = None)]
    pub claim_ttl_seconds: Option<u32>,
}

/// A pool read back as the shared model, plus its Terraform block.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FleetPoolExport {
    /// What the pool's sandboxes run.
    pub spec: SandboxSpec,
    /// The pool's capacity settings.
    pub options: PoolOptions,
    /// The runtime.
    pub runtime: String,
    /// A `fleets_pool` resource block (attributes the provider lacks yet
    /// are commented).
    pub terraform: String,
}

/// Claim options for [`Fleet::acquire_with`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct FleetClaimOptions {
    /// Claim name; an existing claim with it is reattached.
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// Claim TTL (seconds).
    #[uniffi(default = None)]
    pub ttl_seconds: Option<u32>,
    /// Per-claim env token (see [`fleet_generate_claim_token`]); the pool's
    /// spec must set `claim_secrets`. Acquire waits (at most 90 s) until
    /// the sandbox has it, else releases the claim and raises
    /// `ClaimSecretsNotDelivered`.
    #[uniffi(default = None)]
    pub claim_token: Option<String>,
}

/// A fresh per-claim env token (64 hex characters).
#[uniffi::export]
pub fn fleet_generate_claim_token() -> String {
    cua_fleet::claim_secrets::generate_claim_token()
}

fn opt_runtime(r: Option<&str>) -> Result<Option<cua_fleet::RuntimeKind>> {
    match r.filter(|r| !r.is_empty()) {
        Some(r) => Ok(cua_fleet::parse_runtime(r)?),
        None => Ok(None),
    }
}

impl SandboxSpec {
    /// The core spec and the credentials to write (resolved in this
    /// process: env, AWS CLI).
    pub(crate) async fn to_core(
        &self,
    ) -> Result<(
        cua_fleet::SandboxSpec,
        Option<cua_fleet::RegistryCredentials>,
    )> {
        let services: std::collections::BTreeMap<String, u16> =
            self.services.clone().into_iter().collect();
        let readiness = self
            .readiness
            .as_ref()
            .map(|p| {
                let p = p.resolve(&self.services)?;
                Ok::<_, CuaError>(match p {
                    cua_sandbox_core::Probe::Tcp(port) => cua_fleet::ReadinessProbe::Tcp { port },
                    cua_sandbox_core::Probe::Http { port, path, .. } => {
                        cua_fleet::ReadinessProbe::Http { port, path }
                    }
                })
            })
            .transpose()?;
        let creds = match &self.registry_secret {
            Some(s) => Some(s.credentials(&self.image).await?),
            None => None,
        };
        let registry_secret = match (&self.registry_secret_name, &creds) {
            (Some(n), _) => Some(n.clone()),
            (None, Some(c)) => Some(cua_fleet::registry_secret_name(
                c.registry
                    .as_deref()
                    .unwrap_or_else(|| cua_fleet::registry_of(&self.image)),
                &c.username,
            )),
            (None, None) => None,
        };
        let spec = cua_fleet::SandboxSpec {
            image: self.image.clone(),
            command: self.command.clone().filter(|c| !c.is_empty()),
            args: self.args.clone().filter(|a| !a.is_empty()),
            env: self.env.clone().into_iter().collect(),
            services,
            readiness,
            cpu: self.cpu,
            memory_mb: self.memory_mb,
            efi: self.efi,
            sidecars: self.sidecars.iter().map(Container::to_core).collect(),
            registry_secret,
            process_mode: self
                .process_mode
                .as_deref()
                .filter(|m| !m.is_empty())
                .map(cua_fleet::ProcessMode::parse)
                .transpose()?,
            claim_secrets: self.claim_secrets,
        };
        Ok((spec, creds))
    }

    fn from_core(s: cua_fleet::SandboxSpec) -> Self {
        Self {
            image: s.image,
            command: s.command,
            args: s.args,
            env: s.env.into_iter().collect(),
            services: s.services.into_iter().collect(),
            readiness: s.readiness.map(|r| match r {
                cua_fleet::ReadinessProbe::Tcp { port } => ReadinessProbe {
                    port,
                    ..Default::default()
                },
                cua_fleet::ReadinessProbe::Http { port, path } => ReadinessProbe {
                    port,
                    http_path: Some(path),
                    ..Default::default()
                },
            }),
            cpu: s.cpu,
            memory_mb: s.memory_mb,
            efi: s.efi,
            sidecars: s.sidecars.into_iter().map(Container::from_core).collect(),
            registry_secret: None,
            registry_secret_name: s.registry_secret,
            process_mode: s.process_mode.map(|m| m.as_str().to_string()),
            claim_secrets: s.claim_secrets,
        }
    }
}

impl PoolOptions {
    pub(crate) fn to_core(&self) -> Result<cua_fleet::PoolOptions> {
        let secs = |s: Option<u32>| s.map(|s| Duration::from_secs(u64::from(s)));
        Ok(cua_fleet::PoolOptions {
            runtime: opt_runtime(self.runtime.as_deref())?,
            replicas: self.replicas,
            warm: self.warm,
            min_pool_size: self.min_pool_size,
            max_pool_size: self.max_pool_size,
            idle_ttl: secs(self.idle_ttl_seconds),
            ttl_policy: self
                .ttl_policy
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(cua_fleet::TtlPolicy::parse)
                .transpose()?,
            pool_ttl: secs(self.pool_ttl_seconds),
            claim_ttl: secs(self.claim_ttl_seconds),
        })
    }

    fn from_core(o: cua_fleet::PoolOptions) -> Self {
        let secs = |d: Option<Duration>| d.map(|d| d.as_secs().min(u64::from(u32::MAX)) as u32);
        Self {
            runtime: o
                .runtime
                .as_ref()
                .map(|r| cua_fleet::runtime_name(r).into()),
            replicas: o.replicas,
            warm: o.warm,
            min_pool_size: o.min_pool_size,
            max_pool_size: o.max_pool_size,
            idle_ttl_seconds: secs(o.idle_ttl),
            ttl_policy: o.ttl_policy.map(|p| p.as_str().to_string()),
            pool_ttl_seconds: secs(o.pool_ttl),
            claim_ttl_seconds: secs(o.claim_ttl),
        }
    }
}

/// A warm pool spec (`Pool.apply` semantics: pool, namespace and template
/// share `name`). Deprecated: use [`SandboxSpec`] + [`PoolOptions`] with
/// [`Fleet::apply`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetPoolSpec {
    /// Pool name (globally unique on Fleet).
    pub name: String,
    /// containerDisk (kubevirt) or pod image (gvisor).
    pub image: String,
    /// `kubevirt` (default) or `gvisor`.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Warm replicas (default 1).
    #[uniffi(default = None)]
    pub replicas: Option<u32>,
    /// vCPUs.
    #[uniffi(default = None)]
    pub cpu: Option<u32>,
    /// Memory (MiB).
    #[uniffi(default = None)]
    pub memory_mb: Option<u32>,
    /// Services (name → port). Default `{"env": 3211}`.
    #[uniffi(default)]
    pub services: HashMap<String, u16>,
    /// Optional TCP readiness probe port.
    #[uniffi(default = None)]
    pub readiness_tcp_port: Option<u16>,
    /// UEFI firmware (Windows images).
    #[uniffi(default = false)]
    pub efi: bool,
    /// Pod runtimes: entrypoint override.
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Pool TTL after creation (seconds).
    #[uniffi(default = None)]
    pub ttl_seconds_after_created: Option<u32>,
}

/// A pool.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetPool {
    /// Name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Desired replicas.
    pub replicas: u32,
    /// Ready replicas, when reported.
    pub ready_replicas: Option<u32>,
    /// The resource as JSON.
    pub json: String,
}

/// A claim.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetClaim {
    /// Claim name.
    pub name: String,
    /// Namespace (the pool's).
    pub namespace: String,
    /// The resource as JSON.
    pub json: String,
}

/// A bound sandbox.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetSandbox {
    /// Sandbox name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Claim name.
    pub claim: String,
    /// Declared service names.
    pub services: Vec<String>,
}

/// A signed, shareable service URL.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetSignedUrl {
    /// The URL.
    pub url: String,
    /// The resource as JSON (for `revoke_signed_service_url`).
    pub json: String,
}

fn pool_of(p: &cua_fleet::Pool) -> Result<FleetPool> {
    Ok(FleetPool {
        name: p.metadata.name.clone(),
        namespace: p.metadata.namespace.clone(),
        replicas: p.spec.replicas,
        ready_replicas: p.status.as_ref().and_then(|s| s.ready_replicas),
        json: serde_json::to_string(p)?,
    })
}

fn claim_of(c: &cua_fleet::Claim) -> Result<FleetClaim> {
    Ok(FleetClaim {
        name: c.metadata.name.clone(),
        namespace: c.metadata.namespace.clone(),
        json: serde_json::to_string(c)?,
    })
}

fn bound_of(b: cua_fleet::BoundSandbox) -> FleetSandbox {
    FleetSandbox {
        name: b.name,
        namespace: b.namespace,
        claim: b.claim,
        services: b.services,
    }
}

fn bound_to(b: &FleetSandbox) -> cua_fleet::BoundSandbox {
    cua_fleet::BoundSandbox {
        namespace: b.namespace.clone(),
        claim: b.claim.clone(),
        name: b.name.clone(),
        services: b.services.clone(),
    }
}

/// This account's Cua Cloud rates ([`Fleet::usage_pricing`]). Fleet bills
/// the vCPUs and memory a sandbox reserves, per hour; disk is not metered.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FleetUsagePricing {
    /// USD per reserved vCPU per hour.
    pub vcpu_hour_usd: f64,
    /// USD per reserved GiB of memory per hour.
    pub memory_gib_hour_usd: f64,
}

/// The saved card ([`Fleet::billing_status`]): never its number.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetBillingCard {
    /// `visa`, `mastercard`, ...
    pub brand: String,
    /// Last four digits.
    pub last4: String,
    /// Expiry month.
    pub exp_month: u32,
    /// Expiry year.
    pub exp_year: u32,
}

/// The account's Cua Cloud credit.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetBillingCredit {
    /// What is left, US cents.
    pub balance_usd_cents: i64,
    /// The signup grant's amount, US cents (0: none).
    pub signup_grant_usd_cents: i64,
    /// A signup grant exists and none of it has been used.
    pub signup_grant_unused: bool,
}

/// The account's Cua Cloud billing (`GET /api/billing/status`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FleetBillingStatus {
    /// Billing is on for this account.
    pub billing_enabled: bool,
    /// A default card is saved.
    pub payment_method_present: bool,
    /// That card.
    pub card: Option<FleetBillingCard>,
    /// `none`, `payg`, or a plan key.
    pub plan: String,
    /// A card can be added for pay as you go.
    pub payg_available: bool,
    /// The account's credit, when Fleet has credit for it.
    pub credit: Option<FleetBillingCredit>,
    /// The website billing page (credit, cards, plans).
    pub billing_url: Option<String>,
}

/// The everyday sizes of a cloud sandbox, which the Cua apps offer
/// (`cua_fleet::CLOUD_DEFAULT_RANGE_CPUS`,
/// `cua_fleet::CLOUD_DEFAULT_RANGE_MEMORY_MB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FleetSizeLimits {
    /// Fewest vCPUs.
    pub min_cpus: u32,
    /// Most vCPUs.
    pub max_cpus: u32,
    /// Least memory, MiB.
    pub min_memory_mb: u32,
    /// Most memory, MiB.
    pub max_memory_mb: u32,
}

/// The everyday sizes of a cloud sandbox (1-8 vCPUs, 1-32 GiB), which the
/// Cua apps offer. The SDK does not enforce them: it accepts up to 64 vCPUs
/// and 512 MiB to 512 GiB, and Fleet decides what an account may run (a
/// size over the account's limits fails with `FleetAdmissionDenied`).
#[uniffi::export]
pub fn fleet_size_limits() -> FleetSizeLimits {
    FleetSizeLimits {
        min_cpus: *cua_fleet::CLOUD_DEFAULT_RANGE_CPUS.start(),
        max_cpus: *cua_fleet::CLOUD_DEFAULT_RANGE_CPUS.end(),
        min_memory_mb: *cua_fleet::CLOUD_DEFAULT_RANGE_MEMORY_MB.start(),
        max_memory_mb: *cua_fleet::CLOUD_DEFAULT_RANGE_MEMORY_MB.end(),
    }
}

/// Fleet control plane.
#[derive(uniffi::Object)]
pub struct Fleet {
    pub(crate) client: FleetClient,
    pub(crate) pools: cua_fleet::PoolManager,
}

/// A managed pool (`cua-auto-*`, or a legacy `cua-eph-*`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetManagedPool {
    /// Pool (= namespace = template) name.
    pub name: String,
    /// Created by the SDK (labeled), not only matching the prefix.
    pub managed: bool,
    /// Spec hash label (first 32 hex digits).
    pub spec_hash: Option<String>,
    /// Template image.
    pub image: Option<String>,
    /// Current replicas (KEDA-owned).
    pub replicas: u32,
    /// Ready replicas.
    pub ready_replicas: Option<u32>,
    /// Autoscaling ceiling.
    pub max_pool_size: Option<u32>,
    /// Claims in the pool.
    pub claims: u32,
    /// Bound claims.
    pub bound_claims: u32,
    /// Last use (unix seconds).
    pub last_used_unix: Option<i64>,
    /// Creation (unix seconds).
    pub created_unix: Option<i64>,
    /// Backstop expiry (unix seconds).
    pub expires_unix: Option<i64>,
    /// Being deleted.
    pub terminating: bool,
}

/// What a pool GC did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetGcReport {
    /// Deleted pools.
    pub deleted_pools: Vec<String>,
    /// Deleted leftover namespaces.
    pub deleted_namespaces: Vec<String>,
    /// Deleted stuck claims (`namespace/name`).
    pub deleted_claims: Vec<String>,
    /// Kept pools.
    pub kept: Vec<String>,
    /// Non-fatal errors.
    pub errors: Vec<String>,
}

/// Managed pools: what `Sandboxes.create` without a pool uses.
#[derive(uniffi::Object)]
pub struct FleetPools {
    pools: cua_fleet::PoolManager,
}

#[uniffi::export]
impl FleetPools {
    /// This account's managed pools.
    pub async fn list(&self) -> Result<Vec<FleetManagedPool>> {
        let m = self.pools.clone();
        run(async move {
            Ok(m.list()
                .await?
                .into_iter()
                .map(|p| FleetManagedPool {
                    name: p.name,
                    managed: p.managed,
                    spec_hash: p.spec_hash,
                    image: p.image,
                    replicas: p.replicas,
                    ready_replicas: p.ready_replicas,
                    max_pool_size: p.max_pool_size,
                    claims: p.claims,
                    bound_claims: p.bound_claims,
                    last_used_unix: p.last_used,
                    created_unix: p.created,
                    expires_unix: p.expires_at,
                    terminating: p.terminating,
                })
                .collect())
        })
        .await
    }

    /// Deletes managed pools idle for `idle_seconds` (default 1800) and
    /// stuck Pending/Failed managed claims past their TTL.
    pub async fn gc(&self, idle_seconds: Option<u32>) -> Result<FleetGcReport> {
        let m = self.pools.clone();
        let idle = super::secs(idle_seconds.unwrap_or(1800));
        run(async move {
            let r = m.gc(idle).await?;
            Ok(FleetGcReport {
                deleted_pools: r.deleted_pools,
                deleted_namespaces: r.deleted_namespaces,
                deleted_claims: r.deleted_claims,
                kept: r.kept,
                errors: r.errors,
            })
        })
        .await
    }

    /// [`FleetPools::gc`] restricted to the named managed pools: each is
    /// deleted (with its namespace) once it has no claims and has been idle
    /// for `idle_seconds` (default 0, i.e. now). Pools with live claims are
    /// kept, so a pool another process is using survives. Tests use this to
    /// remove the pools they created.
    pub async fn gc_pools(
        &self,
        names: Vec<String>,
        idle_seconds: Option<u32>,
    ) -> Result<FleetGcReport> {
        let m = self.pools.clone();
        let idle = super::secs(idle_seconds.unwrap_or(0));
        run(async move {
            let r = m.gc_pools(idle, &names).await?;
            Ok(FleetGcReport {
                deleted_pools: r.deleted_pools,
                deleted_namespaces: r.deleted_namespaces,
                deleted_claims: r.deleted_claims,
                kept: r.kept,
                errors: r.errors,
            })
        })
        .await
    }
}

macro_rules! fleet_call {
    ($self:ident, |$c:ident| $body:expr) => {{
        let $c = $self.client.clone();
        run(async move { $body }).await
    }};
}

#[uniffi::export]
impl Fleet {
    /// Fleet API base URL.
    pub fn base_url(&self) -> String {
        self.client.config().base_url.clone()
    }

    /// This account's Cua Cloud rates (`GET /api/config`), reused for five
    /// minutes. `None` when Fleet answers without rates: show no price
    /// rather than a guess.
    pub async fn usage_pricing(&self) -> Result<Option<FleetUsagePricing>> {
        let client = self.client.clone();
        run(async move {
            Ok(client.usage_pricing().await?.map(|p| FleetUsagePricing {
                vcpu_hour_usd: p.vcpu_hour_usd,
                memory_gib_hour_usd: p.memory_gib_hour_usd,
            }))
        })
        .await
    }

    /// The account's Cua Cloud billing: its credit, card and the website
    /// billing page. A Fleet without billing answers `billing_enabled:
    /// false`.
    pub async fn billing_status(&self) -> Result<FleetBillingStatus> {
        let client = self.client.clone();
        run(async move {
            let s = client.billing_status().await?;
            Ok(FleetBillingStatus {
                billing_enabled: s.billing_enabled,
                payment_method_present: s.payment_method_present,
                card: s.card.map(|c| FleetBillingCard {
                    brand: c.brand,
                    last4: c.last4,
                    exp_month: c.exp_month,
                    exp_year: c.exp_year,
                }),
                plan: s.plan,
                payg_available: s.payg_available,
                credit: s.credit.map(|c| FleetBillingCredit {
                    balance_usd_cents: c.balance_usd_cents,
                    signup_grant_usd_cents: c.signup_grant_usd_cents,
                    signup_grant_unused: c.signup_grant_unused,
                }),
                billing_url: s.billing_url,
            })
        })
        .await
    }

    /// Managed pools (list, gc).
    pub fn pools(&self) -> Arc<FleetPools> {
        Arc::new(FleetPools {
            pools: self.pools.clone(),
        })
    }

    /// A random ephemeral pool name (`cua-eph-<hex>`).
    pub fn ephemeral_pool_name(&self) -> String {
        cua_fleet::ephemeral_pool_name()
    }

    /// Reconciles pool `name` (= namespace = template) to run `spec` with
    /// `options`: the one pool writer (rolls a new pool back if the
    /// template fails). The image is pinned to the variant the runtime
    /// runs; `spec.registry_secret` is written as the pool's pull Secret.
    pub async fn apply(
        &self,
        name: String,
        spec: SandboxSpec,
        options: PoolOptions,
    ) -> Result<FleetPool> {
        let options = options.to_core()?;
        fleet_call!(self, |c| {
            let (spec, creds) = spec.to_core().await?;
            pool_of(
                &c.apply_with_credentials(&name, &spec, &options, creds.as_ref())
                    .await?
                    .pool,
            )
        })
    }

    /// Compares the set fields of `spec` with pool `pool`'s template:
    /// `PoolSpecMismatch` (with a readable diff) when they differ.
    pub async fn check_pool_spec(&self, pool: String, spec: SandboxSpec) -> Result<()> {
        fleet_call!(self, |c| {
            let (spec, _) = spec.to_core().await?;
            Ok(c.check_pool_spec(&pool, &spec).await?)
        })
    }

    /// Lays the set fields of `spec` over pool `pool`'s template and writes
    /// it (the pool's capacity is kept; a no-op when nothing differs).
    pub async fn apply_pool_template(&self, pool: String, spec: SandboxSpec) -> Result<()> {
        fleet_call!(self, |c| {
            let (spec, creds) = spec.to_core().await?;
            Ok(c.apply_pool_template(&pool, &spec, creds.as_ref()).await?)
        })
    }

    /// Reads pool `name` back as the shared model, with its `fleets_pool`
    /// Terraform block.
    pub async fn export_pool(&self, name: String) -> Result<FleetPoolExport> {
        fleet_call!(self, |c| {
            let (spec, options, runtime) = c.export_pool(&name).await?;
            let terraform = cua_fleet::terraform_pool_block(&name, &spec, &options, &runtime);
            Ok(FleetPoolExport {
                spec: SandboxSpec::from_core(spec),
                options: PoolOptions::from_core(options),
                runtime: cua_fleet::runtime_name(&runtime).into(),
                terraform,
            })
        })
    }

    /// Deprecated: use [`Fleet::apply`]. Reconciles a pool and its template
    /// from the flat spec (converted to [`SandboxSpec`] + [`PoolOptions`]).
    pub async fn apply_pool(&self, spec: FleetPoolSpec) -> Result<FleetPool> {
        let runtime = opt_runtime(spec.runtime.as_deref())?;
        let mut sandbox = cua_fleet::SandboxSpec::new(spec.image);
        sandbox.cpu = spec.cpu;
        sandbox.memory_mb = spec.memory_mb;
        sandbox.services = if spec.services.is_empty() {
            [("env".to_string(), 3211u16)].into()
        } else {
            spec.services.into_iter().collect()
        };
        sandbox.readiness = spec
            .readiness_tcp_port
            .map(|port| cua_fleet::ReadinessProbe::Tcp { port });
        sandbox.efi = spec.efi;
        sandbox.command = spec.command.filter(|c| !c.is_empty());
        let options = cua_fleet::PoolOptions {
            runtime,
            replicas: Some(spec.replicas.unwrap_or(1)),
            pool_ttl: spec
                .ttl_seconds_after_created
                .map(|s| Duration::from_secs(u64::from(s))),
            ..Default::default()
        };
        let name = spec.name;
        fleet_call!(self, |c| pool_of(
            &c.apply(&name, &sandbox, &options).await?.pool
        ))
    }

    /// The image pool `name`'s template runs, as a claim on it reports it
    /// (`Sandbox.image_info`): pinned by the resolver (cached per pool), or
    /// the template reference with empty `pinned_ref`/`digest` when the
    /// registry cannot be read. `None` when the template names no image.
    pub async fn pool_image_info(&self, name: String) -> Result<Option<super::sandbox::ImageInfo>> {
        fleet_call!(self, |c| Ok(cua_sandbox_core::pool_image_info(
            &c, &name, None
        )
        .await?
        .map(Into::into)))
    }

    /// Looks up a pool.
    pub async fn get_pool(&self, name: String) -> Result<FleetPool> {
        fleet_call!(self, |c| pool_of(&c.get_pool(&name).await?.pool))
    }

    /// Lists pools in a namespace.
    pub async fn list_pools(&self, namespace: String) -> Result<Vec<FleetPool>> {
        fleet_call!(self, |c| {
            c.sdk()
                .list_pools(namespace)
                .await
                .map_err(cua_fleet::Error::from)?
                .iter()
                .map(pool_of)
                .collect()
        })
    }

    /// Deletes a pool, its namespace and its same-named template.
    pub async fn delete_pool(&self, name: String) -> Result<()> {
        fleet_call!(self, |c| {
            let mut h = c.get_pool(&name).await?;
            let templates = c
                .sdk()
                .list_templates(h.pool.metadata.namespace.clone())
                .await
                .map_err(cua_fleet::Error::from)?;
            h.template = templates.into_iter().find(|t| t.metadata.name == name);
            c.delete_pool(h).await?;
            Ok(())
        })
    }

    /// Sets warm replicas (0 suspends).
    pub async fn set_pool_replicas(&self, name: String, replicas: u32) -> Result<FleetPool> {
        fleet_call!(self, |c| {
            let mut h = c.get_pool(&name).await?;
            c.set_pool_replicas(&mut h, replicas).await?;
            pool_of(&h.pool)
        })
    }

    /// Waits for at least one ready replica.
    pub async fn wait_pool_ready(&self, name: String, timeout_ms: u32) -> Result<FleetPool> {
        fleet_call!(self, |c| {
            let h = c.get_pool(&name).await?;
            pool_of(&c.wait_pool_ready(&h, super::millis(timeout_ms)).await?)
        })
    }

    /// Lists templates in a namespace as JSON resources.
    pub async fn list_templates(&self, namespace: String) -> Result<Vec<String>> {
        fleet_call!(self, |c| {
            c.sdk()
                .list_templates(namespace)
                .await
                .map_err(cua_fleet::Error::from)?
                .iter()
                .map(|t| serde_json::to_string(t).map_err(CuaError::from))
                .collect()
        })
    }

    /// Claims a sandbox from a pool and waits for it to bind. A claim named
    /// `name` that already exists is reattached.
    pub async fn acquire(
        &self,
        pool: String,
        name: Option<String>,
        ttl_seconds: Option<u32>,
    ) -> Result<FleetSandbox> {
        fleet_call!(self, |c| {
            let p = c.get_pool(&pool).await?.pool;
            let b = c
                .acquire(
                    &p,
                    ClaimOptions {
                        name,
                        ttl_seconds_after_created: ttl_seconds,
                        ..Default::default()
                    },
                )
                .await?;
            Ok(bound_of(b))
        })
    }

    /// [`Fleet::acquire`] with claim options, including a per-claim env
    /// token (bounded wait for its delivery; `ClaimSecretsNotDelivered`
    /// releases the claim).
    pub async fn acquire_with(
        &self,
        pool: String,
        options: FleetClaimOptions,
    ) -> Result<FleetSandbox> {
        fleet_call!(self, |c| {
            let p = c.get_pool(&pool).await?.pool;
            let b = c
                .acquire(
                    &p,
                    ClaimOptions {
                        name: options.name,
                        ttl_seconds_after_created: options.ttl_seconds,
                        claim_token: options.claim_token,
                        ..Default::default()
                    },
                )
                .await?;
            Ok(bound_of(b))
        })
    }

    /// Creates a claim without waiting.
    pub async fn claim(
        &self,
        pool: String,
        name: Option<String>,
        ttl_seconds: Option<u32>,
    ) -> Result<FleetClaim> {
        fleet_call!(self, |c| {
            let p = c.get_pool(&pool).await?.pool;
            let (claim, _) = c
                .claim(
                    &p,
                    ClaimOptions {
                        name,
                        ttl_seconds_after_created: ttl_seconds,
                        ..Default::default()
                    },
                )
                .await?;
            claim_of(&claim)
        })
    }

    /// Waits for a named claim to bind.
    pub async fn attach_claim(&self, namespace: String, name: String) -> Result<FleetSandbox> {
        fleet_call!(self, |c| Ok(bound_of(
            c.attach_claim(&namespace, &name).await?
        )))
    }

    /// Lists claims in a namespace.
    pub async fn list_claims(&self, namespace: String) -> Result<Vec<FleetClaim>> {
        fleet_call!(self, |c| c
            .list_claims(&namespace)
            .await?
            .iter()
            .map(claim_of)
            .collect())
    }

    /// Releases a claim (missing claims are fine).
    pub async fn release(&self, namespace: String, name: String) -> Result<()> {
        fleet_call!(self, |c| Ok(c.release(&namespace, &name).await?))
    }

    /// Extends a claim's lease; returns the RFC 3339 shutdown time.
    pub async fn keep_alive(
        &self,
        namespace: String,
        name: String,
        seconds: u32,
    ) -> Result<String> {
        fleet_call!(self, |c| Ok(c
            .keep_alive(&namespace, &name, super::secs(seconds))
            .await?))
    }

    /// The gateway URL of a sandbox service (needs the Fleet bearer).
    pub fn service_url(&self, sandbox: FleetSandbox, service: String) -> Result<String> {
        Ok(self.client.service_url(&bound_to(&sandbox), &service)?)
    }

    /// Mints a signed, shareable service URL.
    pub async fn create_signed_service_url(
        &self,
        sandbox: FleetSandbox,
        service: String,
        label: Option<String>,
        expires_in_seconds: u32,
    ) -> Result<FleetSignedUrl> {
        fleet_call!(self, |c| {
            let u = c
                .create_signed_service_url(
                    &bound_to(&sandbox),
                    &service,
                    label,
                    super::secs(expires_in_seconds),
                )
                .await?;
            Ok(FleetSignedUrl {
                url: u.url.clone(),
                json: serde_json::to_string(&u)?,
            })
        })
    }

    /// Lists image resources (JSON) in a namespace.
    pub async fn list_images(&self, namespace: String) -> Result<Vec<String>> {
        fleet_call!(self, |c| c
            .list_images(&namespace)
            .await?
            .iter()
            .map(|v| Ok(v.to_string()))
            .collect())
    }

    /// Gets an image resource (JSON).
    pub async fn get_image(&self, namespace: String, name: String) -> Result<String> {
        fleet_call!(self, |c| Ok(c
            .get_image(&namespace, &name)
            .await?
            .to_string()))
    }

    /// Creates an image resource (remote build) from a JSON manifest.
    pub async fn create_image(&self, namespace: String, manifest_json: String) -> Result<String> {
        let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;
        fleet_call!(self, |c| Ok(c
            .create_image(&namespace, manifest)
            .await?
            .to_string()))
    }

    /// Deletes an image resource.
    pub async fn delete_image(&self, namespace: String, name: String) -> Result<()> {
        fleet_call!(self, |c| Ok(c.delete_image(&namespace, &name).await?))
    }
}

/// The Fleet runtime for `image`: `runtime` (`kubevirt`, `gvisor`) when
/// given, else the one the image needs. What the image is
/// comes from its registry manifest (read with docker, ghcr and ECR
/// credentials): a KubeVirt containerDisk (a `/disk/disk.img` layer or
/// trycua containerDisk media types) runs on `kubevirt`, a container rootfs
/// on `gvisor`. When the manifest cannot be read and no runtime is given,
/// the runtime is guessed from the reference with a warning (a `docker-`
/// tag runs on `gvisor`, anything else on `kubevirt`). Raises
/// `InvalidArgument` only for a runtime the image cannot run on, and
/// `Unsupported` for a macOS image or runtime `macos` (Fleet does not offer
/// macOS in this SDK). Blocks
/// while the registry is read (at most 20 s; `CUA_FLEET_IMAGE_INSPECT=0`
/// skips it). The one copy of this rule; cua-sandbox calls it.
#[uniffi::export]
pub fn fleet_resolve_runtime(runtime: Option<String>, image: String) -> Result<String> {
    let runtime = match runtime.as_deref() {
        Some(r) => cua_fleet::parse_runtime(r)?,
        None => None,
    };
    // A plain thread: the caller may itself be on a Tokio runtime.
    let resolved = std::thread::spawn(move || {
        super::runtime().block_on(cua_fleet::resolve_runtime(runtime, &image))
    })
    .join()
    .map_err(|_| CuaError::Internal("fleet_resolve_runtime panicked".into()))??;
    Ok(cua_fleet::runtime_name(&resolved).into())
}

/// The variant of an image from its registry documents (`manifest`: an
/// image manifest or index, `config`: the platform manifest's config blob):
/// `container-disk`, `rootfs` or `other`. Pure (no registry read). A macOS
/// image raises `Unsupported`.
#[uniffi::export]
pub fn fleet_image_variant(manifest: String, config: Option<String>) -> Result<String> {
    Ok(
        cua_fleet::runtime::classify_manifest_json(&manifest, config.as_deref())?
            .as_str()
            .into(),
    )
}

/// [`fleet_resolve_runtime`] with the image variant already known
/// (`variant` as [`fleet_image_variant`] returns it); `None` means the
/// manifest could not be read (an unset runtime then falls back to the
/// reference). Pure (no registry read).
#[uniffi::export]
pub fn fleet_check_runtime(
    runtime: Option<String>,
    image: String,
    variant: Option<String>,
) -> Result<String> {
    let runtime = match runtime.as_deref() {
        Some(r) => cua_fleet::parse_runtime(r)?,
        None => None,
    };
    let evidence = match variant {
        Some(v) => cua_fleet::ImageEvidence::Known(cua_fleet::ImageVariant::parse(&v)?),
        None => cua_fleet::ImageEvidence::Unavailable("image variant not given".into()),
    };
    Ok(cua_fleet::runtime_name(&cua_fleet::check_runtime(runtime, &image, &evidence)?).into())
}
