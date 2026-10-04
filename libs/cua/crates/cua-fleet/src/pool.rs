//! Pools, templates and claims with cua-sandbox's `Pool` semantics.

use crate::{BoundSandbox, Claim, Error, FleetClient, Pool, Result, SdkError, Template};
use cyclops_sdk::{CreateClaimRequest, CreatePoolRequest, CreateTemplateRequest, ResourceMetadata};
use cyclops_sdk_schema::{
    ClaimSpec, OSGymSandboxWarmPoolSpec, SandboxTemplateRef, WarmPoolAutoscaling,
};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime},
};

pub use cyclops_sdk_schema::RuntimeKind;

/// Name of an ephemeral pool: `cua-eph-<12 hex>` (as `Sandbox.ephemeral`).
pub fn ephemeral_pool_name() -> String {
    format!("cua-eph-{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

/// Everything needed to reconcile a pool and its template, as one flat
/// record.
///
/// Deprecated: use [`crate::SandboxSpec`] + [`crate::PoolOptions`] with
/// [`FleetClient::apply`]. A `PoolSpec` is converted with
/// [`PoolSpec::parts`] and written by the same code.
#[derive(Clone, Debug)]
pub struct PoolSpec {
    /// Pool (and namespace, and template) name. Globally unique on Fleet.
    pub name: String,
    /// containerDisk (kubevirt) or pod image (gvisor/macos) reference.
    pub image: String,
    /// Backend runtime.
    pub runtime: RuntimeKind,
    /// Warm replicas.
    pub replicas: u32,
    /// vCPUs.
    pub cpu: Option<u32>,
    /// Memory in MiB.
    pub memory_mb: Option<u32>,
    /// Logical service name → guest TCP port. Default: `{"env": 3211}`.
    pub services: BTreeMap<String, u16>,
    /// Optional TCP readiness probe port. Unset: Fleet readiness only (no
    /// daemon is assumed).
    pub readiness_tcp_port: Option<u16>,
    /// Readiness probe (TCP or HTTP); wins over `readiness_tcp_port`.
    pub readiness: Option<crate::ReadinessProbe>,
    /// UEFI firmware (Windows images).
    pub efi: bool,
    /// Entrypoint override.
    pub command: Option<Vec<String>>,
    /// Arguments replacing the image CMD (`vmTemplate.args`).
    pub args: Option<Vec<String>>,
    /// Environment variables (`vmTemplate.env`).
    pub env: BTreeMap<String, String>,
    /// `vmTemplate.processMode`.
    pub process_mode: Option<crate::ProcessMode>,
    /// Autoscaling extension.
    pub autoscaling: Option<WarmPoolAutoscaling>,
    /// Pool creation-age TTL.
    pub ttl_seconds_after_created: Option<u32>,
    /// Idle TTL (`spec.idleTtlSeconds`, trycua/cloud#7886).
    pub idle_ttl_seconds: Option<u32>,
    /// What TTL expiry deletes (`spec.ttlPolicy`).
    pub ttl_policy: Option<crate::TtlPolicy>,
    /// Template opt-in to per-claim Secrets (`vmTemplate.claimSecrets`), so
    /// claims can carry [`ClaimOptions::claim_token`].
    pub claim_secrets: bool,
    /// Extra containers next to the sandbox (`vmTemplate.sidecars`), on
    /// every runtime.
    pub sidecars: Vec<crate::Sidecar>,
    /// A `cua-registry-*` pull Secret in the pool namespace for private
    /// images (`vmTemplate.imagePullSecret`).
    pub image_pull_secret: Option<String>,
    /// Credentials written to `image_pull_secret` before the template
    /// (never serialized; `Debug` redacts the password). The registry is
    /// the image's.
    pub registry_credentials: Option<crate::RegistryCredentials>,
}

impl PoolSpec {
    /// A spec with cua defaults (1 replica, service `env` on 3211).
    pub fn new(name: impl Into<String>, image: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: image.into(),
            runtime: RuntimeKind::Kubevirt,
            replicas: 1,
            cpu: None,
            memory_mb: None,
            services: [("env".to_string(), cua_proto_port())].into(),
            readiness_tcp_port: None,
            readiness: None,
            efi: false,
            command: None,
            args: None,
            env: BTreeMap::new(),
            process_mode: None,
            autoscaling: None,
            ttl_seconds_after_created: None,
            idle_ttl_seconds: None,
            ttl_policy: None,
            claim_secrets: false,
            sidecars: vec![],
            image_pull_secret: None,
            registry_credentials: None,
        }
    }

    /// The flat spec as the shared model: what the sandbox runs and how
    /// the pool keeps capacity for it.
    pub fn parts(&self) -> (crate::SandboxSpec, crate::PoolOptions) {
        let spec = crate::SandboxSpec {
            image: self.image.clone(),
            command: self.command.clone().filter(|c| !c.is_empty()),
            args: self.args.clone().filter(|a| !a.is_empty()),
            env: self.env.clone(),
            services: self.services.clone(),
            readiness: self.readiness.clone().or(self
                .readiness_tcp_port
                .map(|port| crate::ReadinessProbe::Tcp { port })),
            cpu: self.cpu,
            memory_mb: self.memory_mb,
            efi: self.efi,
            sidecars: self.sidecars.clone(),
            registry_secret: self.image_pull_secret.clone(),
            process_mode: self.process_mode,
            claim_secrets: self.claim_secrets,
        };
        let a = self.autoscaling.as_ref();
        let options = crate::PoolOptions {
            runtime: Some(self.runtime.clone()),
            replicas: Some(self.replicas),
            warm: None,
            min_pool_size: a.map(|a| a.min_pool_size.unwrap_or(0)),
            max_pool_size: a.and_then(|a| a.max_pool_size),
            idle_ttl: self
                .idle_ttl_seconds
                .map(|s| Duration::from_secs(u64::from(s))),
            ttl_policy: self.ttl_policy,
            pool_ttl: self
                .ttl_seconds_after_created
                .map(|s| Duration::from_secs(u64::from(s))),
            claim_ttl: None,
        };
        (spec, options)
    }

    /// A flat spec from the shared model (`options.runtime` defaults to
    /// kubevirt; resolve it from the image first when unset).
    pub fn from_parts(
        name: impl Into<String>,
        spec: &crate::SandboxSpec,
        options: &crate::PoolOptions,
    ) -> Self {
        let mut s = Self::new(name, spec.image.clone());
        s.runtime = options.runtime.clone().unwrap_or(RuntimeKind::Kubevirt);
        s.replicas = options.spec_replicas();
        s.autoscaling = options.autoscaling();
        s.ttl_seconds_after_created = options.pool_ttl.map(secs_u32);
        s.idle_ttl_seconds = options.idle_ttl.map(secs_u32);
        s.ttl_policy = options.ttl_policy;
        s.cpu = spec.cpu;
        s.memory_mb = spec.memory_mb;
        s.services = spec.services.clone();
        s.readiness_tcp_port = match &spec.readiness {
            Some(crate::ReadinessProbe::Tcp { port }) => Some(*port),
            _ => None,
        };
        s.readiness = spec.readiness.clone();
        s.efi = spec.efi;
        s.command = spec.command.clone();
        s.args = spec.args.clone();
        s.env = spec.env.clone();
        s.process_mode = spec.process_mode;
        s.claim_secrets = spec.claim_secrets;
        s.sidecars = spec.sidecars.clone();
        s.image_pull_secret = spec.registry_secret.clone();
        s
    }

    /// Whether the spec uses sidecars or a tenant pull Secret.
    pub fn uses_parity(&self) -> bool {
        !self.sidecars.is_empty() || self.image_pull_secret.is_some()
    }

    /// Checks the spec against its runtime and this build (see
    /// [`crate::SandboxSpec::validate`]).
    pub fn validate_parity(&self) -> Result<()> {
        self.parts().0.validate(&self.runtime)
    }

    /// The template resource ([`crate::SandboxSpec::template_json`]).
    pub fn template_json(&self) -> Result<serde_json::Value> {
        self.parts().0.template_json(&self.name, &self.runtime)
    }

    /// Opts the template into per-claim Secrets (see [`Self::claim_secrets`]).
    pub fn claim_secrets(mut self, enabled: bool) -> Self {
        self.claim_secrets = enabled;
        self
    }

    /// Sets the runtime.
    pub fn runtime(mut self, runtime: RuntimeKind) -> Self {
        self.runtime = runtime;
        self
    }

    /// Replaces the services map.
    pub fn services<I, S>(mut self, services: I) -> Self
    where
        I: IntoIterator<Item = (S, u16)>,
        S: Into<String>,
    {
        self.services = services.into_iter().map(|(k, v)| (k.into(), v)).collect();
        self
    }

    /// Adds a TCP readiness probe.
    pub fn readiness_tcp(mut self, port: u16) -> Self {
        self.readiness_tcp_port = Some(port);
        self
    }

    fn validate(&self) -> Result<()> {
        cyclops_sdk::validate_dns_label(&self.name)?;
        if self.image.trim().is_empty() {
            return Err(Error::InvalidArgument("image must not be empty".into()));
        }
        if self.services.is_empty() || self.services.iter().any(|(k, p)| k.is_empty() || *p == 0) {
            return Err(Error::InvalidArgument(
                "services must map non-empty names to TCP ports".into(),
            ));
        }
        Ok(())
    }

    /// The `CreatePoolRequest` (pool, namespace and template share `name`).
    pub fn pool_request(&self) -> CreatePoolRequest {
        let mut r = pool_request(&self.name, &self.parts().1);
        // The flat spec states replicas and autoscaling as given.
        r.spec.replicas = self.replicas;
        r.spec.autoscaling = self.autoscaling.clone();
        r
    }

    /// The typed `CreateTemplateRequest`: only the fields the `libs/fleet`
    /// mirror carries. Writers use [`Self::template_json`], which adds the
    /// rest.
    pub fn template_request(&self) -> Result<CreateTemplateRequest> {
        let spec = self.parts().0;
        spec.validate(&self.runtime)?;
        Ok(CreateTemplateRequest {
            namespace: self.name.clone(),
            name: self.name.clone(),
            spec: spec.typed_template(&self.runtime)?,
        })
    }
}

fn secs_u32(d: Duration) -> u32 {
    d.as_secs().min(u64::from(u32::MAX)) as u32
}

/// The pool resource request for pool `name` with `options`.
pub(crate) fn pool_request(name: &str, options: &crate::PoolOptions) -> CreatePoolRequest {
    CreatePoolRequest {
        namespace: name.to_string(),
        spec: OSGymSandboxWarmPoolSpec {
            replicas: options.spec_replicas(),
            sandbox_template_ref: SandboxTemplateRef {
                name: name.to_string(),
            },
            autoscaling: options.autoscaling(),
            ttl_seconds_after_created: options.pool_ttl.map(secs_u32),
            idle_ttl_seconds: None,
            ttl_policy: None,
        },
    }
}

fn cua_proto_port() -> u16 {
    3211
}

/// Same rule as cua-sandbox's `_needs_ecr_pull_secret`.
pub(crate) fn needs_ecr_pull_secret(image: &str) -> bool {
    let host = image.split('/').next().unwrap_or_default();
    host.contains(".dkr.ecr.") && host.ends_with(".amazonaws.com")
}

/// A reconciled pool plus the template it owns (when created by
/// [`FleetClient::apply`]).
#[derive(Clone, Debug)]
pub struct PoolHandle {
    /// The warm pool resource.
    pub pool: Pool,
    /// The template this handle owns and deletes with the pool.
    pub template: Option<Template>,
    /// The template's runtime, when known.
    pub runtime: Option<RuntimeKind>,
    /// Default claim TTL ([`crate::PoolOptions::claim_ttl`]).
    pub claim_ttl: Option<Duration>,
}

impl PoolHandle {
    /// Pool name.
    pub fn name(&self) -> &str {
        &self.pool.metadata.name
    }
}

/// Claim options.
#[derive(Clone, Debug, Default)]
pub struct ClaimOptions {
    /// Claim name. An existing claim with this name is reattached.
    pub name: Option<String>,
    /// Explicit claim spec.
    pub spec: Option<ClaimSpec>,
    /// Claim TTL (mutually exclusive with `spec`, as in cua-sandbox).
    pub ttl_seconds_after_created: Option<u32>,
    /// Labels for the created claim.
    pub labels: Option<std::collections::HashMap<String, String>>,
    /// Per-claim env token, delivered into the guest at
    /// `/run/cua/env-token` through the claim's Secret `cua-claim-<claim>`
    /// (key `env-token`); cua-spacesd in await-token-file mode adopts
    /// it. The pool's template must set [`crate::SandboxSpec::claim_secrets`].
    /// Only used when a claim is created (a reattached claim keeps its
    /// token). [`FleetClient::acquire`] waits (bounded) until the driver has
    /// it; see [`crate::claim_secrets`].
    pub claim_token: Option<String>,
}

/// A stub claim for lifecycle calls by name (cua-sandbox's `_claim_stub`).
pub(crate) fn claim_stub(namespace: &str, name: &str) -> Claim {
    Claim {
        api_version: "osgym.cua.ai/v1alpha1".into(),
        kind: "OSGymSandboxClaim".into(),
        metadata: ResourceMetadata {
            namespace: namespace.into(),
            name: name.into(),
            labels: None,
            creation_timestamp: None,
        },
        spec: ClaimSpec {
            sandbox_template_ref: SandboxTemplateRef {
                name: String::new(),
            },
            warmpool: None,
            bind_deadline: None,
            lifecycle: None,
            ttl_seconds_after_created: None,
            secret_ref: None,
        },
        status: None,
    }
}

fn is_status(e: &SdkError, code: u16) -> bool {
    matches!(e, SdkError::Status { status, .. } if *status == code)
}

impl FleetClient {
    /// Reconciles the pool and then the template; if the template fails the
    /// pool is deleted again (cua-sandbox `Pool.apply`).
    ///
    /// Deprecated: use [`FleetClient::apply`] with a [`crate::SandboxSpec`]
    /// and [`crate::PoolOptions`]; this converts with [`PoolSpec::parts`].
    #[deprecated(note = "use FleetClient::apply(name, &SandboxSpec, &PoolOptions)")]
    pub async fn apply_pool(&self, spec: &PoolSpec) -> Result<PoolHandle> {
        spec.validate()?;
        let (sandbox, mut options) = spec.parts();
        // The flat spec states replicas and autoscaling as given.
        if let Some(a) = &spec.autoscaling {
            options.replicas = a.initial_pool_size.or(Some(spec.replicas));
        }
        self.apply_with_credentials(
            &spec.name,
            &sandbox,
            &options,
            spec.registry_credentials.as_ref(),
        )
        .await
    }

    /// Writes `spec.registry_credentials` to `spec.image_pull_secret` in the
    /// pool namespace (a no-op without both).
    pub(crate) async fn write_pull_secret(&self, spec: &PoolSpec) -> Result<()> {
        let (Some(name), Some(creds)) = (&spec.image_pull_secret, &spec.registry_credentials)
        else {
            return Ok(());
        };
        let registry = creds
            .registry
            .clone()
            .unwrap_or_else(|| cua_image::registry_of(&spec.image).to_string());
        self.put_registry_secret(&spec.name, name, &registry, creds)
            .await
    }

    /// Looks up an existing pool (no owned template).
    pub async fn get_pool(&self, name: &str) -> Result<PoolHandle> {
        Ok(PoolHandle {
            pool: self.sdk().get_pool(name.into()).await?,
            template: None,
            runtime: None,
            claim_ttl: None,
        })
    }

    /// The runtime of the template `pool` serves (`None` when the template
    /// is unreadable or does not say; Fleet then runs KubeVirt).
    pub async fn pool_runtime(&self, pool: &Pool) -> Option<RuntimeKind> {
        self.sdk()
            .get_template(
                pool.metadata.namespace.clone(),
                pool.spec.sandbox_template_ref.name.clone(),
            )
            .await
            .ok()
            .map(|t| t.spec.vm_template.runtime.unwrap_or(RuntimeKind::Kubevirt))
    }

    /// The services (name → target port) of the template `pool` serves.
    /// Claims only list service *names*; the ports live on the template.
    pub async fn pool_services(&self, pool: &Pool) -> Result<BTreeMap<String, u16>> {
        let template = self
            .sdk()
            .get_template(
                pool.metadata.namespace.clone(),
                pool.spec.sandbox_template_ref.name.clone(),
            )
            .await?;
        Ok(template
            .spec
            .vm_template
            .services
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.name, s.target_port))
            .collect())
    }

    /// Deletes the pool (and its namespace) and the owned template.
    pub async fn delete_pool(&self, handle: PoolHandle) -> Result<()> {
        self.sdk().delete_pool(handle.pool).await?;
        if let Some(t) = handle.template {
            match self.sdk().delete_template(t).await {
                Ok(()) => {}
                Err(e) if is_status(&e, 404) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Sets `spec.replicas` (suspend = 0, resume = 1 in cua-sandbox).
    pub async fn set_pool_replicas(&self, handle: &mut PoolHandle, replicas: u32) -> Result<()> {
        crate::check_pool_size("replicas", replicas)?;
        let mut pool = handle.pool.clone();
        pool.spec.replicas = replicas;
        handle.pool = self.sdk().update_pool(pool).await?;
        Ok(())
    }

    /// Waits until the pool reports at least one ready replica.
    pub async fn wait_pool_ready(&self, handle: &PoolHandle, timeout: Duration) -> Result<Pool> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let pools = self
                .sdk()
                .list_pools(handle.pool.metadata.namespace.clone())
                .await?;
            if let Some(p) = pools
                .into_iter()
                .find(|p| p.metadata.name == handle.pool.metadata.name)
                && p.status
                    .as_ref()
                    .and_then(|s| s.ready_replicas)
                    .unwrap_or(0)
                    >= 1
            {
                return Ok(p);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "pool {} did not warm up within {timeout:?}",
                    handle.name()
                )));
            }
            tokio::time::sleep(Duration::from_millis(self.config.pool_poll_interval_ms)).await;
        }
    }

    /// Refuses to claim from a pool whose template has a runtime this SDK
    /// does not offer (`macos`). An unreadable template is not a refusal.
    async fn ensure_pool_offered(&self, pool: &Pool) -> Result<()> {
        match self
            .sdk()
            .get_template(
                pool.metadata.namespace.clone(),
                pool.spec.sandbox_template_ref.name.clone(),
            )
            .await
        {
            Ok(t) => match &t.spec.vm_template.runtime {
                Some(rt) => crate::runtime::ensure_runtime_offered(rt),
                None => Ok(()),
            },
            Err(_) => Ok(()),
        }
    }

    /// Creates a claim, or reattaches to an existing claim with
    /// `options.name`. Returns the claim and whether it was created.
    pub async fn claim(&self, pool: &Pool, options: ClaimOptions) -> Result<(Claim, bool)> {
        let spec = match (options.spec, options.ttl_seconds_after_created) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidArgument(
                    "pass ttl_seconds_after_created inside spec when supplying an explicit ClaimSpec"
                        .into(),
                ));
            }
            (spec, None) => spec,
            (None, Some(ttl)) => Some(ClaimSpec {
                sandbox_template_ref: pool.spec.sandbox_template_ref.clone(),
                warmpool: None,
                bind_deadline: None,
                lifecycle: None,
                ttl_seconds_after_created: Some(ttl),
                secret_ref: None,
            }),
        };
        if let Some(token) = &options.claim_token {
            crate::claim_secrets::validate_claim_token(token)?;
        }
        if let Some(name) = &options.name {
            let existing = self
                .sdk()
                .list_claims(pool.metadata.namespace.clone())
                .await?
                .into_iter()
                .find(|c| &c.metadata.name == name);
            if let Some(c) = existing {
                return Ok((c, false));
            }
            // Cloud sandbox names are unique within the account (`cloud:<name>`
            // never names a pool): refuse a name another pool already uses.
            if let Some(other) = self
                .find_claims_best_effort(name)
                .await
                .into_iter()
                .find(|c| c.metadata.namespace != pool.metadata.namespace)
            {
                return Err(Error::InvalidArgument(format!(
                    "a cloud sandbox named {name:?} already exists (cloud:{name}, in pool {}); \
                     cloud sandbox names are unique within the account: pick another name, or \
                     delete that one first",
                    other.metadata.namespace
                )));
            }
        }
        self.ensure_pool_offered(pool).await?;
        if let Some(token) = options.claim_token {
            let claim = self
                .create_claim_with_secret(pool, spec, options.name, options.labels, token)
                .await?;
            return Ok((claim, true));
        }
        let claim = self
            .sdk()
            .create_claim(CreateClaimRequest {
                pool: pool.clone(),
                spec,
                name: options.name,
                labels: options.labels,
                // Claim secrets are written as JSON (see `claim_secrets`).
                secret_files: None,
            })
            .await?;
        Ok((claim, true))
    }

    /// Waits for a claim to bind.
    pub async fn wait_claim(&self, claim: &Claim) -> Result<BoundSandbox> {
        Ok(self.sdk().wait_claim(claim.clone()).await?)
    }

    /// Claim + wait. A claim created here is released if binding fails.
    ///
    /// With [`ClaimOptions::claim_token`], it also waits (at most
    /// [`crate::claim_secrets::DEFAULT_WAIT`] after Bound) until the
    /// sandbox's spacesd has the token; if it never arrives the claim is
    /// released and [`Error::ClaimSecretsNotDelivered`] returned.
    pub async fn acquire(&self, pool: &Pool, options: ClaimOptions) -> Result<BoundSandbox> {
        let token = options.claim_token.clone();
        let (claim, created) = self.claim(pool, options).await?;
        // A caller that gives up (a cancelled create drops this future)
        // after the claim exists releases it: nothing else would.
        let mut guard = ClaimGuard {
            fleet: self.clone(),
            namespace: claim.metadata.namespace.clone(),
            name: claim.metadata.name.clone(),
            armed: created,
        };
        let out = self.acquire_bound(pool, claim, created, token).await;
        guard.armed = false;
        out
    }

    async fn acquire_bound(
        &self,
        pool: &Pool,
        claim: Claim,
        created: bool,
        token: Option<String>,
    ) -> Result<BoundSandbox> {
        let bound = match self.wait_claim(&claim).await {
            Ok(bound) => {
                if bound.namespace != claim.metadata.namespace || bound.claim != claim.metadata.name
                {
                    return Err(Error::InvalidArgument(
                        "Fleet returned a sandbox bound to a different claim".into(),
                    ));
                }
                bound
            }
            Err(e) => {
                if created
                    && let Err(release) = self
                        .release(&claim.metadata.namespace, &claim.metadata.name)
                        .await
                {
                    tracing::warn!(error = %release, "failed to release claim after bind failure");
                }
                return Err(e);
            }
        };
        if let Some(token) = token.filter(|_| created) {
            let runtime = match self
                .sdk()
                .get_template(
                    pool.metadata.namespace.clone(),
                    pool.spec.sandbox_template_ref.name.clone(),
                )
                .await
            {
                Ok(t) => t.spec.vm_template.runtime.unwrap_or(RuntimeKind::Kubevirt),
                Err(_) => RuntimeKind::Kubevirt,
            };
            if let Err(e) = self.await_claim_secrets(&bound, &token, &runtime).await {
                if let Err(release) = self.release(&bound.namespace, &bound.claim).await {
                    tracing::warn!(error = %release,
                        "failed to release claim whose secrets never arrived");
                }
                return Err(e);
            }
        }
        Ok(bound)
    }

    /// Re-reads a claim by name and waits for it to be bound (reattach).
    pub async fn attach_claim(&self, namespace: &str, name: &str) -> Result<BoundSandbox> {
        let bound = self.wait_claim(&claim_stub(namespace, name)).await?;
        if bound.namespace != namespace || bound.claim != name {
            return Err(Error::InvalidArgument(
                "Fleet returned a sandbox bound to a different claim".into(),
            ));
        }
        Ok(bound)
    }

    /// The image a claim's sandbox runs: its template's `containerDiskImage`
    /// (the reference as the template names it, digest-pinned for a managed
    /// pool) and the runtime. `Ok(None)` when the template names no image.
    pub async fn claim_image(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<(String, RuntimeKind)>> {
        let claim = self.sdk().get_claim(claim_stub(namespace, name)).await?;
        let template = self
            .sdk()
            .get_template(
                namespace.to_string(),
                claim.spec.sandbox_template_ref.name.clone(),
            )
            .await?;
        let image = template.spec.vm_template.container_disk_image.trim();
        Ok((!image.is_empty()).then(|| {
            (
                image.to_string(),
                template
                    .spec
                    .vm_template
                    .runtime
                    .clone()
                    .unwrap_or(RuntimeKind::Kubevirt),
            )
        }))
    }

    /// Releases (deletes) a claim by name. Missing claims are fine. Its
    /// `cua-claim-<name>` Secret, if any, is deleted too (the operator's
    /// owner reference is the backstop).
    pub async fn release(&self, namespace: &str, name: &str) -> Result<()> {
        match self.sdk().delete_claim(claim_stub(namespace, name)).await {
            Ok(()) => {}
            Err(e) if is_status(&e, 404) => {}
            Err(e) => return Err(e.into()),
        }
        if self.claim_secret_written(namespace, name) {
            self.delete_claim_secret(namespace, name).await?;
        }
        Ok(())
    }

    /// Extends the claim's lease to now + `duration` (`Sandbox.keep_alive`).
    /// Returns the RFC 3339 shutdown time sent.
    pub async fn keep_alive(
        &self,
        namespace: &str,
        name: &str,
        duration: Duration,
    ) -> Result<String> {
        if duration.is_zero() {
            return Err(Error::InvalidArgument(
                "keep_alive duration must be positive".into(),
            ));
        }
        let shutdown = humantime::format_rfc3339_seconds(SystemTime::now() + duration).to_string();
        self.sdk()
            .renew_claim(claim_stub(namespace, name), shutdown.clone())
            .await?;
        Ok(shutdown)
    }

    /// Lists claims in a pool namespace.
    pub async fn list_claims(&self, namespace: &str) -> Result<Vec<Claim>> {
        Ok(self.sdk().list_claims(namespace.into()).await?)
    }

    /// Every claim of the account named `name` (a `cloud:<name>` sandbox),
    /// across the namespaces this principal can read. Namespaces it may not
    /// read, or that vanish meanwhile, are skipped.
    pub async fn find_claims(&self, name: &str) -> Result<Vec<Claim>> {
        let namespaces = self.sdk().list_namespaces().await?;
        let mut out = Vec::new();
        for ns in namespaces {
            match self.list_claims(&ns.name).await {
                Ok(claims) => out.extend(claims.into_iter().filter(|c| c.metadata.name == name)),
                Err(Error::Sdk(
                    SdkError::Status { status: 403, .. } | SdkError::PoolAccessDenied { .. },
                )) => continue,
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// [`Self::find_claims`] for the create-time uniqueness check: a Fleet
    /// that cannot list the account's namespaces does not block a create.
    async fn find_claims_best_effort(&self, name: &str) -> Vec<Claim> {
        match tokio::time::timeout(Duration::from_secs(10), self.find_claims(name)).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                tracing::debug!(name, error = %e, "cloud name uniqueness not checked");
                vec![]
            }
            Err(_) => {
                tracing::debug!(name, "cloud name uniqueness check timed out");
                vec![]
            }
        }
    }
}

/// Releases a claim [`FleetClient::acquire`] made when its future is
/// dropped before it returned (see `cua_vmm::cleanup`).
struct ClaimGuard {
    fleet: FleetClient,
    namespace: String,
    name: String,
    armed: bool,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (fleet, namespace, name) = (
            self.fleet.clone(),
            std::mem::take(&mut self.namespace),
            std::mem::take(&mut self.name),
        );
        cua_vmm::cleanup::spawn(async move {
            match fleet.release(&namespace, &name).await {
                Ok(()) => tracing::info!(claim = %name, "released the claim of a cancelled create"),
                Err(e) => {
                    tracing::warn!(claim = %name, error = %e, "could not release the claim of a cancelled create")
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_names() {
        let n = ephemeral_pool_name();
        assert!(n.starts_with("cua-eph-"));
        assert_eq!(n.len(), "cua-eph-".len() + 12);
        assert!(cyclops_sdk::validate_dns_label(&n).is_ok());
    }

    #[test]
    fn ecr_secret_only_for_private_ecr() {
        assert!(needs_ecr_pull_secret(
            "123.dkr.ecr.us-east-1.amazonaws.com/img:1"
        ));
        assert!(!needs_ecr_pull_secret(
            "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:x"
        ));
        assert!(!needs_ecr_pull_secret("ghcr.io/trycua/x"));
    }

    #[test]
    fn template_request_is_daemon_agnostic_by_default() {
        let spec = PoolSpec::new("cua-e2e-x", "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main");
        let t = spec.template_request().unwrap();
        let vm = &t.spec.vm_template;
        assert!(vm.probes.is_none(), "no hard-coded readiness probe");
        assert_eq!(vm.services.as_ref().unwrap()[0].name, "env");
        assert_eq!(vm.services.as_ref().unwrap()[0].target_port, 3211);
        assert!(vm.image_pull_secret.is_none());
        let t = spec
            .clone()
            .readiness_tcp(8000)
            .runtime(RuntimeKind::Gvisor)
            .template_request()
            .unwrap();
        let probes = t.spec.vm_template.probes.unwrap();
        assert_eq!(
            probes.as_value()["readinessProbe"]["tcpSocket"]["port"],
            8000
        );
        assert_eq!(t.spec.vm_template.runtime, Some(RuntimeKind::Gvisor));
    }
}
