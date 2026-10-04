//! `Fleet`: Cua Cloud pools, templates, claims and images.
//!
//! Cua Cloud (Fleet) has closed. Every call that needed it fails with
//! `CuaError::Fleet` and [`cua_sandbox_core::CLOUD_CLOSED`]; the API shape is
//! kept so existing callers get that message instead of a missing symbol.
//! [`Fleet::billing_status`] still reads the account's billing from the Cua
//! account API ([`cua_auth::account::AccountApi`]).

use super::run;
use super::sandbox::{Container, ReadinessProbe, RegistrySecret};
use crate::{CuaError, Result};
use cua_auth::account::AccountApi;
use std::{collections::HashMap, sync::Arc};

fn closed<T>() -> Result<T> {
    Err(CuaError::Fleet(cua_sandbox_core::CLOUD_CLOSED.into()))
}

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
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
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

/// The everyday sizes of a cloud sandbox, which the Cua apps offered.
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
/// Cua apps offered.
#[uniffi::export]
pub fn fleet_size_limits() -> FleetSizeLimits {
    FleetSizeLimits {
        min_cpus: 1,
        max_cpus: 8,
        min_memory_mb: 1024,
        max_memory_mb: 32 * 1024,
    }
}

/// Fleet control plane (Cua Cloud, closed) and the account's billing.
#[derive(uniffi::Object)]
pub struct Fleet {
    pub(crate) account: AccountApi,
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

/// Managed pools (Cua Cloud, closed).
#[derive(uniffi::Object)]
pub struct FleetPools {}

#[uniffi::export]
impl FleetPools {
    /// This account's managed pools. Cua Cloud has closed.
    pub async fn list(&self) -> Result<Vec<FleetManagedPool>> {
        run(async { closed() }).await
    }

    /// Deletes idle managed pools. Cua Cloud has closed.
    pub async fn gc(&self, idle_seconds: Option<u32>) -> Result<FleetGcReport> {
        let _ = idle_seconds;
        run(async { closed() }).await
    }

    /// Deletes the named managed pools. Cua Cloud has closed.
    pub async fn gc_pools(
        &self,
        names: Vec<String>,
        idle_seconds: Option<u32>,
    ) -> Result<FleetGcReport> {
        let _ = (names, idle_seconds);
        run(async { closed() }).await
    }
}

#[uniffi::export]
impl Fleet {
    /// The Cua account API base URL.
    pub fn base_url(&self) -> String {
        self.account.base_url().to_string()
    }

    /// This account's Cua Cloud rates. Cua Cloud has closed: `None`.
    pub async fn usage_pricing(&self) -> Result<Option<FleetUsagePricing>> {
        run(async { Ok(None) }).await
    }

    /// The account's billing: its credit, card and the website billing
    /// page. An account API without billing answers `billing_enabled:
    /// false`.
    pub async fn billing_status(&self) -> Result<FleetBillingStatus> {
        let account = self.account.clone();
        run(async move {
            let s = account
                .billing_status()
                .await
                .map_err(|e| CuaError::Fleet(e.to_string()))?;
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

    /// Managed pools (Cua Cloud, closed).
    pub fn pools(&self) -> Arc<FleetPools> {
        Arc::new(FleetPools {})
    }

    /// A random ephemeral pool name (`cua-eph-<hex>`).
    pub fn ephemeral_pool_name(&self) -> String {
        format!("cua-eph-{:08x}", rand::random::<u32>())
    }

    /// Reconciles a pool. Cua Cloud has closed.
    pub async fn apply(
        &self,
        name: String,
        spec: SandboxSpec,
        options: PoolOptions,
    ) -> Result<FleetPool> {
        let _ = (name, spec, options);
        run(async { closed() }).await
    }

    /// Compares a spec with a pool's template. Cua Cloud has closed.
    pub async fn check_pool_spec(&self, pool: String, spec: SandboxSpec) -> Result<()> {
        let _ = (pool, spec);
        run(async { closed() }).await
    }

    /// Writes a spec over a pool's template. Cua Cloud has closed.
    pub async fn apply_pool_template(&self, pool: String, spec: SandboxSpec) -> Result<()> {
        let _ = (pool, spec);
        run(async { closed() }).await
    }

    /// Reads a pool back. Cua Cloud has closed.
    pub async fn export_pool(&self, name: String) -> Result<FleetPoolExport> {
        let _ = name;
        run(async { closed() }).await
    }

    /// Deprecated pool writer. Cua Cloud has closed.
    pub async fn apply_pool(&self, spec: FleetPoolSpec) -> Result<FleetPool> {
        let _ = spec;
        run(async { closed() }).await
    }

    /// The image a pool's template runs. Cua Cloud has closed.
    pub async fn pool_image_info(&self, name: String) -> Result<Option<super::sandbox::ImageInfo>> {
        let _ = name;
        run(async { closed() }).await
    }

    /// Looks up a pool. Cua Cloud has closed.
    pub async fn get_pool(&self, name: String) -> Result<FleetPool> {
        let _ = name;
        run(async { closed() }).await
    }

    /// Lists pools. Cua Cloud has closed.
    pub async fn list_pools(&self, namespace: String) -> Result<Vec<FleetPool>> {
        let _ = namespace;
        run(async { closed() }).await
    }

    /// Deletes a pool. Cua Cloud has closed.
    pub async fn delete_pool(&self, name: String) -> Result<()> {
        let _ = name;
        run(async { closed() }).await
    }

    /// Sets warm replicas. Cua Cloud has closed.
    pub async fn set_pool_replicas(&self, name: String, replicas: u32) -> Result<FleetPool> {
        let _ = (name, replicas);
        run(async { closed() }).await
    }

    /// Waits for a ready replica. Cua Cloud has closed.
    pub async fn wait_pool_ready(&self, name: String, timeout_ms: u32) -> Result<FleetPool> {
        let _ = (name, timeout_ms);
        run(async { closed() }).await
    }

    /// Lists templates. Cua Cloud has closed.
    pub async fn list_templates(&self, namespace: String) -> Result<Vec<String>> {
        let _ = namespace;
        run(async { closed() }).await
    }

    /// Claims a sandbox from a pool. Cua Cloud has closed.
    pub async fn acquire(
        &self,
        pool: String,
        name: Option<String>,
        ttl_seconds: Option<u32>,
    ) -> Result<FleetSandbox> {
        let _ = (pool, name, ttl_seconds);
        run(async { closed() }).await
    }

    /// Claims a sandbox with options. Cua Cloud has closed.
    pub async fn acquire_with(
        &self,
        pool: String,
        options: FleetClaimOptions,
    ) -> Result<FleetSandbox> {
        let _ = (pool, options);
        run(async { closed() }).await
    }

    /// Creates a claim. Cua Cloud has closed.
    pub async fn claim(
        &self,
        pool: String,
        name: Option<String>,
        ttl_seconds: Option<u32>,
    ) -> Result<FleetClaim> {
        let _ = (pool, name, ttl_seconds);
        run(async { closed() }).await
    }

    /// Waits for a named claim to bind. Cua Cloud has closed.
    pub async fn attach_claim(&self, namespace: String, name: String) -> Result<FleetSandbox> {
        let _ = (namespace, name);
        run(async { closed() }).await
    }

    /// Lists claims. Cua Cloud has closed.
    pub async fn list_claims(&self, namespace: String) -> Result<Vec<FleetClaim>> {
        let _ = namespace;
        run(async { closed() }).await
    }

    /// Releases a claim. Cua Cloud has closed.
    pub async fn release(&self, namespace: String, name: String) -> Result<()> {
        let _ = (namespace, name);
        run(async { closed() }).await
    }

    /// Extends a claim's lease. Cua Cloud has closed.
    pub async fn keep_alive(
        &self,
        namespace: String,
        name: String,
        seconds: u32,
    ) -> Result<String> {
        let _ = (namespace, name, seconds);
        run(async { closed() }).await
    }

    /// The gateway URL of a sandbox service. Cua Cloud has closed.
    pub fn service_url(&self, sandbox: FleetSandbox, service: String) -> Result<String> {
        let _ = (sandbox, service);
        closed()
    }

    /// Mints a signed service URL. Cua Cloud has closed.
    pub async fn create_signed_service_url(
        &self,
        sandbox: FleetSandbox,
        service: String,
        label: Option<String>,
        expires_in_seconds: u32,
    ) -> Result<FleetSignedUrl> {
        let _ = (sandbox, service, label, expires_in_seconds);
        run(async { closed() }).await
    }

    /// Lists image resources. Cua Cloud has closed.
    pub async fn list_images(&self, namespace: String) -> Result<Vec<String>> {
        let _ = namespace;
        run(async { closed() }).await
    }

    /// Gets an image resource. Cua Cloud has closed.
    pub async fn get_image(&self, namespace: String, name: String) -> Result<String> {
        let _ = (namespace, name);
        run(async { closed() }).await
    }

    /// Creates an image resource. Cua Cloud has closed.
    pub async fn create_image(&self, namespace: String, manifest_json: String) -> Result<String> {
        let _ = (namespace, manifest_json);
        run(async { closed() }).await
    }

    /// Deletes an image resource. Cua Cloud has closed.
    pub async fn delete_image(&self, namespace: String, name: String) -> Result<()> {
        let _ = (namespace, name);
        run(async { closed() }).await
    }
}

/// The Fleet runtime for `image`. Cua Cloud has closed.
#[uniffi::export]
pub fn fleet_resolve_runtime(runtime: Option<String>, image: String) -> Result<String> {
    let _ = (runtime, image);
    closed()
}

/// The Fleet variant of an image from its registry documents. Cua Cloud
/// has closed.
#[uniffi::export]
pub fn fleet_image_variant(manifest: String, config: Option<String>) -> Result<String> {
    let _ = (manifest, config);
    closed()
}

/// [`fleet_resolve_runtime`] with the image variant already known. Cua
/// Cloud has closed.
#[uniffi::export]
pub fn fleet_check_runtime(
    runtime: Option<String>,
    image: String,
    variant: Option<String>,
) -> Result<String> {
    let _ = (runtime, image, variant);
    closed()
}
