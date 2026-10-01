//! One sandbox model for pools and `Sandbox.create`.
//!
//! [`SandboxSpec`] is what a sandbox runs (image, process, services,
//! probes, resources, sidecars, pull secret, claim secrets) and
//! [`PoolOptions`] is how a pool keeps capacity for it (warm floor, size,
//! idle TTL, TTL policy, claim TTL, runtime). [`FleetClient::apply`] is the
//! one pool writer: managed pools, `Sandbox.create`, the language bindings
//! and the CLI all build their templates through
//! [`SandboxSpec::template_json`], and `cua fleet pool export --terraform`
//! prints the same fields as a `fleets_pool` block
//! ([`terraform_pool_block`]).
//!
//! Field names are Fleet's (`vmTemplate.*` from trycua/cloud#7887 and
//! #7893, `idleTtlSeconds` / `ttlPolicy` from #7886). Fields the
//! `libs/fleet` mirror does not carry yet are written as JSON next to the
//! typed ones; when the mirror syncs they move into the typed
//! [`VmTemplate`] literal.

use crate::{Error, FleetClient, PoolHandle, Result, RuntimeKind, Sidecar};
use cyclops_sdk_schema::{
    Firmware, OSGymSandboxTemplateSpec, SandboxService, ServiceProtocol, VmTemplate,
    WarmPoolAutoscaling,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, time::Duration};

/// Default autoscaling ceiling of a pool that sets a warm floor or a
/// minimum but no maximum.
pub const DEFAULT_MAX_POOL_SIZE: u32 = 10;

/// How a sandbox runs `command` / `args` / `env` (`vmTemplate.processMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProcessMode {
    /// Today's behaviour: pods run them; KubeVirt ignores `command` and
    /// refuses `args` / `env`.
    Legacy,
    /// Every runtime runs them (KubeVirt through cloud-init).
    Run,
}

impl ProcessMode {
    /// The Fleet spelling (`Legacy` / `Run`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ProcessMode::Legacy => "Legacy",
            ProcessMode::Run => "Run",
        }
    }

    /// Parses `legacy` / `run` (any case).
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "legacy" => Ok(ProcessMode::Legacy),
            "run" => Ok(ProcessMode::Run),
            other => Err(Error::InvalidArgument(format!(
                "process mode {other:?} must be Legacy or Run"
            ))),
        }
    }
}

/// What expiry of the pool TTL or idle TTL deletes (`spec.ttlPolicy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TtlPolicy {
    /// Only the pool (Fleet's behaviour when absent).
    Retain,
    /// The pool and its dead unbound claims (never Bound claims).
    Cascade,
}

impl TtlPolicy {
    /// The Fleet spelling (`Retain` / `Cascade`).
    pub fn as_str(&self) -> &'static str {
        match self {
            TtlPolicy::Retain => "Retain",
            TtlPolicy::Cascade => "Cascade",
        }
    }

    /// Parses `retain` / `cascade` (any case).
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "retain" => Ok(TtlPolicy::Retain),
            "cascade" => Ok(TtlPolicy::Cascade),
            other => Err(Error::InvalidArgument(format!(
                "ttl policy {other:?} must be Retain or Cascade"
            ))),
        }
    }
}

/// A readiness probe on a guest port (`vmTemplate.probes.readinessProbe`):
/// a replica is ready, and a claim binds, only once it passes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ReadinessProbe {
    /// A TCP connect succeeds.
    Tcp {
        /// Guest port.
        port: u16,
    },
    /// `GET path` answers 2xx/3xx.
    Http {
        /// Guest port.
        port: u16,
        /// Path (starts with `/`).
        path: String,
    },
}

impl ReadinessProbe {
    /// The probed port.
    pub fn port(&self) -> u16 {
        match self {
            ReadinessProbe::Tcp { port } | ReadinessProbe::Http { port, .. } => *port,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            ReadinessProbe::Tcp { port } => {
                json!({"readinessProbe": {"tcpSocket": {"port": port}}})
            }
            ReadinessProbe::Http { port, path } => {
                json!({"readinessProbe": {"httpGet": {"port": port, "path": path}}})
            }
        }
    }

    fn from_json(probes: &Value) -> Option<Self> {
        let r = &probes["readinessProbe"];
        let port = |v: &Value| v["port"].as_u64().and_then(|p| u16::try_from(p).ok());
        if let Some(port) = port(&r["tcpSocket"]) {
            return Some(ReadinessProbe::Tcp { port });
        }
        let h = &r["httpGet"];
        port(h).map(|port| ReadinessProbe::Http {
            port,
            path: h["path"].as_str().unwrap_or("/").to_string(),
        })
    }
}

/// What a sandbox runs. The same record drives `Sandbox.create`, managed
/// pools and [`FleetClient::apply`]; [`SandboxSpec::template_json`] is its
/// one mapping to a Fleet template.
///
/// Empty collections and `None` mean "not set": the template leaves the
/// field to Fleet's default, and [`FleetClient::check_pool_spec`] does not
/// compare it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    /// Image reference (a container image for gVisor, a containerDisk for
    /// KubeVirt; `vmTemplate.containerDiskImage`).
    pub image: String,
    /// argv replacing the image ENTRYPOINT (`vmTemplate.command`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Arguments replacing the image CMD (`vmTemplate.args`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Plain environment variables, not secrets (`vmTemplate.env`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Service name → guest port (`vmTemplate.services`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub services: BTreeMap<String, u16>,
    /// Readiness probe (`vmTemplate.probes.readinessProbe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ReadinessProbe>,
    /// vCPUs (`vmTemplate.cpuCores`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    /// Memory in MiB (`vmTemplate.memory`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// UEFI firmware (Windows images; `vmTemplate.firmware: efi`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub efi: bool,
    /// Extra containers next to the sandbox, on every runtime, addressed by
    /// name (`vmTemplate.sidecars`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<Sidecar>,
    /// A `cua-registry-*` pull Secret in the pool namespace
    /// (`vmTemplate.imagePullSecret`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_secret: Option<String>,
    /// `vmTemplate.processMode`. `None`: `Run` when a process field
    /// (`command`, `args`, `env`) is set, else Fleet's default (`Legacy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_mode: Option<ProcessMode>,
    /// Per-claim Secrets (`vmTemplate.claimSecrets`): claims may carry a
    /// token delivered at `/run/cua/env-token`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claim_secrets: bool,
}

/// How a pool keeps capacity for a [`SandboxSpec`]. `None` fields keep
/// Fleet's defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoolOptions {
    /// Backend runtime. `None`: from the image (a containerDisk boots on
    /// KubeVirt, a container rootfs runs on gVisor).
    pub runtime: Option<RuntimeKind>,
    /// Static replicas of a pool without autoscaling (default 1). With
    /// autoscaling, the initial replicas.
    pub replicas: Option<u32>,
    /// Keep one sandbox warm: `minPoolSize` (and `initialPoolSize`) 1.
    pub warm: Option<bool>,
    /// Autoscaling floor (`autoscaling.minPoolSize`).
    pub min_pool_size: Option<u32>,
    /// Autoscaling ceiling (`autoscaling.maxPoolSize`).
    pub max_pool_size: Option<u32>,
    /// Delete the pool after this long without claims
    /// (`spec.idleTtlSeconds`).
    pub idle_ttl: Option<Duration>,
    /// What TTL expiry deletes (`spec.ttlPolicy`).
    pub ttl_policy: Option<TtlPolicy>,
    /// Pool creation-age TTL (`spec.ttlSecondsAfterCreated`).
    pub pool_ttl: Option<Duration>,
    /// Default TTL of claims made through the returned handle
    /// (`ttlSecondsAfterCreated` on each claim). Client side: Fleet pools
    /// have no claim TTL field.
    pub claim_ttl: Option<Duration>,
}

fn secs_u32(d: Duration) -> u32 {
    d.as_secs().min(u64::from(u32::MAX)) as u32
}

impl PoolOptions {
    /// Whether the pool is autoscaled (a warm floor, a minimum or a
    /// maximum is set).
    pub fn autoscaled(&self) -> bool {
        self.warm.is_some() || self.min_pool_size.is_some() || self.max_pool_size.is_some()
    }

    /// `spec.autoscaling`: `warm` means an explicit floor of one
    /// (`minPoolSize: 1`), which is the only warm floor Fleet has.
    pub fn autoscaling(&self) -> Option<WarmPoolAutoscaling> {
        if !self.autoscaled() {
            return None;
        }
        let warm = u32::from(self.warm == Some(true));
        let min = self.min_pool_size.unwrap_or(warm);
        let initial = self.replicas.unwrap_or(0).max(min).max(warm);
        let max = self
            .max_pool_size
            .unwrap_or(DEFAULT_MAX_POOL_SIZE)
            .max(initial)
            .max(1);
        Some(WarmPoolAutoscaling {
            min_pool_size: Some(min),
            initial_pool_size: Some(initial),
            max_pool_size: Some(max),
        })
    }

    /// Checks the pool sizes the options write (`spec.replicas`,
    /// `autoscaling.minPoolSize` / `initialPoolSize` / `maxPoolSize`)
    /// against [`crate::FLEET_ABSOLUTE_MAX_POOL_SIZE`].
    pub fn validate(&self) -> Result<()> {
        crate::check_pool_size("replicas", self.spec_replicas())?;
        if let Some(a) = self.autoscaling() {
            for (what, n) in [
                ("min_pool_size", a.min_pool_size),
                ("initial_pool_size", a.initial_pool_size),
                ("max_pool_size", a.max_pool_size),
            ] {
                if let Some(n) = n {
                    crate::check_pool_size(what, n)?;
                }
            }
        }
        Ok(())
    }

    /// `spec.replicas`: the initial size with autoscaling, else
    /// `replicas` (default 1).
    pub fn spec_replicas(&self) -> u32 {
        match self.autoscaling() {
            Some(a) => a.initial_pool_size.unwrap_or(0),
            None => self.replicas.unwrap_or(1),
        }
    }

    /// The pool fields the `libs/fleet` mirror lacks (#7886), as a merge
    /// patch of `spec`; empty when neither is set.
    pub fn lifecycle_patch(&self) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        if let Some(t) = self.idle_ttl {
            m.insert("idleTtlSeconds".into(), json!(secs_u32(t)));
        }
        if let Some(p) = self.ttl_policy {
            m.insert("ttlPolicy".into(), json!(p.as_str()));
        }
        m
    }

    /// Reads the options back from a pool resource (JSON).
    pub fn from_pool_json(pool: &Value, runtime: Option<RuntimeKind>) -> Self {
        let spec = &pool["spec"];
        let a = &spec["autoscaling"];
        let u = |v: &Value| v.as_u64().map(|n| n.min(u64::from(u32::MAX)) as u32);
        let min = u(&a["minPoolSize"]);
        Self {
            runtime,
            replicas: u(&spec["replicas"]),
            warm: a.is_object().then(|| min.unwrap_or(0) >= 1),
            min_pool_size: min,
            max_pool_size: u(&a["maxPoolSize"]),
            idle_ttl: u(&spec["idleTtlSeconds"]).map(|s| Duration::from_secs(s.into())),
            ttl_policy: spec["ttlPolicy"]
                .as_str()
                .and_then(|p| TtlPolicy::parse(p).ok()),
            pool_ttl: u(&spec["ttlSecondsAfterCreated"]).map(|s| Duration::from_secs(s.into())),
            claim_ttl: None,
        }
    }
}

/// `4096Mi`, `4Gi`, `512M`, `4G` → MiB.
pub fn parse_memory_mib(q: &str) -> Option<u32> {
    let q = q.trim();
    let split = q.find(|c: char| !c.is_ascii_digit()).unwrap_or(q.len());
    let (n, unit) = q.split_at(split);
    let n: u64 = n.parse().ok()?;
    let mib = match unit {
        "Mi" => n,
        "Gi" => n * 1024,
        "Ti" => n * 1024 * 1024,
        "Ki" => n / 1024,
        "M" => n * 1_000_000 / (1 << 20),
        "G" => n * 1_000_000_000 / (1 << 20),
        "" => n / (1 << 20),
        _ => return None,
    };
    u32::try_from(mib).ok()
}

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported(what.into())
}

impl SandboxSpec {
    /// A spec running `image` with Fleet's defaults.
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            ..Default::default()
        }
    }

    fn has_process_fields(&self) -> bool {
        self.command.as_ref().is_some_and(|c| !c.is_empty())
            || self.args.as_ref().is_some_and(|a| !a.is_empty())
            || !self.env.is_empty()
    }

    /// The `processMode` the template gets: the explicit one, else `Run`
    /// when a process field is set (so `command` / `args` / `env` run the
    /// same way on every runtime).
    pub fn effective_process_mode(&self) -> Option<ProcessMode> {
        self.process_mode
            .or_else(|| self.has_process_fields().then_some(ProcessMode::Run))
    }

    /// Checks the spec against `runtime` and this build before anything is
    /// written: nothing a runtime cannot run is ever silently dropped.
    pub fn validate(&self, runtime: &RuntimeKind) -> Result<()> {
        crate::runtime::ensure_runtime_offered(runtime)?;
        crate::limits::check_cloud_size(self.cpu, self.memory_mb)?;
        if self.image.trim().is_empty() {
            return Err(Error::InvalidArgument("image must not be empty".into()));
        }
        if let Some((k, p)) = self.services.iter().find(|(k, p)| k.is_empty() || **p == 0) {
            return Err(Error::InvalidArgument(format!(
                "service {k:?} → {p}: services map non-empty names to TCP ports"
            )));
        }
        if let Some(k) = self.env.keys().find(|k| !cua_image::spec::is_env_name(k)) {
            return Err(Error::InvalidArgument(format!(
                "bad environment variable name {k:?}"
            )));
        }
        let vm = matches!(runtime, RuntimeKind::Kubevirt);
        let run = self.effective_process_mode() == Some(ProcessMode::Run);
        if vm && !run && self.has_process_fields() {
            // Only an explicit Legacy gets here: KubeVirt runs process
            // fields only with processMode Run (cloud-init); with Legacy
            // Fleet would ignore `command` and refuse `args` / `env`.
            return Err(unsupported(
                "command, args and env on a cloud VM image (KubeVirt) need processMode Run; \
                 drop process_mode=Legacy (Run is the default when they are set)",
            ));
        }
        if vm && run {
            // KubeVirt Run (trycua/cloud#7887): a VM image has no entrypoint
            // to pass args to, and /etc/cua/env holds one line per value.
            if self.args.as_ref().is_some_and(|a| !a.is_empty())
                && !self.command.as_ref().is_some_and(|c| !c.is_empty())
            {
                return Err(Error::InvalidArgument(
                    "args on a cloud VM image (KubeVirt) need a command: a VM image has no \
                     entrypoint to pass them to"
                        .into(),
                ));
            }
            // Fleet's admission: no C0 control character or DEL (newlines
            // included) in env values, command or args on KubeVirt Run.
            let control = |v: &str| v.chars().any(|c| c.is_ascii_control());
            if let Some(k) = self.env.iter().find(|(_, v)| control(v)).map(|(k, _)| k) {
                return Err(Error::InvalidArgument(format!(
                    "environment variable {k} has a multi-line value or a control character; \
                     a cloud VM image (KubeVirt) takes single-line values"
                )));
            }
            if let Some(field) = [("command", &self.command), ("args", &self.args)]
                .into_iter()
                .find(|(_, v)| v.iter().flatten().any(|a| control(a)))
                .map(|(f, _)| f)
            {
                return Err(Error::InvalidArgument(format!(
                    "{field} has a multi-line argument or a control character; a cloud VM image \
                     (KubeVirt) takes single-line arguments (pass a script file or encode it)"
                )));
            }
        }
        if !self.sidecars.is_empty() || self.registry_secret.is_some() {
            // A pod sidecar shares the sandbox's network namespace, so it
            // may not take spacesd's port; a KubeVirt companion pod has its
            // own.
            let reserved: Vec<u16> = if vm {
                vec![]
            } else {
                self.services.get("env").copied().into_iter().collect()
            };
            crate::validate_sidecars(&self.sidecars, &reserved)?;
            crate::check_reserved_service_names(self.services.keys(), !self.sidecars.is_empty())?;
            if let Some(secret) = &self.registry_secret {
                if !secret.starts_with(crate::REGISTRY_SECRET_PREFIX)
                    || cyclops_sdk::validate_dns_label(secret).is_err()
                {
                    return Err(Error::InvalidArgument(format!(
                        "registry secret {secret:?} must be a {}<name> Secret",
                        crate::REGISTRY_SECRET_PREFIX
                    )));
                }
                if let Some(img) = std::iter::once(&self.image)
                    .chain(self.sidecars.iter().map(|s| &s.image))
                    .find(|i| crate::pool::needs_ecr_pull_secret(i))
                {
                    return Err(Error::InvalidArgument(format!(
                        "{img} needs the account's ECR pull secret, and a sandbox has one pull \
                         secret; use one private registry per sandbox"
                    )));
                }
            }
        }
        Ok(())
    }

    /// The typed template spec (the fields the `libs/fleet` mirror has).
    /// Use [`Self::template_json`] to write a template: it adds the rest.
    pub fn typed_template(&self, runtime: &RuntimeKind) -> Result<OSGymSandboxTemplateSpec> {
        let services = (!self.services.is_empty()).then(|| {
            self.services
                .iter()
                .map(|(name, port)| SandboxService {
                    name: name.clone(),
                    target_port: *port,
                    protocol: Some(ServiceProtocol::TCP),
                })
                .collect()
        });
        let probes = self
            .readiness
            .as_ref()
            .map(|p| {
                cyclops_sdk_schema::PreservedJson::from_json(p.to_json().to_string())
                    .map_err(|e| Error::InvalidArgument(e.to_string()))
            })
            .transpose()?;
        let command = self.command.clone().filter(|c| !c.is_empty());
        Ok(OSGymSandboxTemplateSpec {
            vm_template: VmTemplate {
                container_disk_image: self.image.clone(),
                command,
                runtime: Some(runtime.clone()),
                runtime_class_name: None,
                node_selector: None,
                tolerations: None,
                image_pull_policy: None,
                // The account's ECR pull secret authenticates the private
                // ECR only; attaching it to a public image makes the
                // gateway enforce its private-registry allowlist.
                image_pull_secret: self.registry_secret.clone().or_else(|| {
                    (crate::pool::needs_ecr_pull_secret(&self.image)
                        || self
                            .sidecars
                            .iter()
                            .any(|s| crate::pool::needs_ecr_pull_secret(&s.image)))
                    .then(|| "ecr-credentials".to_string())
                }),
                cpu_cores: self.cpu,
                memory: self.memory_mb.map(|m| format!("{m}Mi")),
                firmware: self.efi.then_some(Firmware::Efi),
                nested_virtualization: None,
                probes,
                services,
                oidc: None,
                // Written as JSON next to the typed template (template_json).
                claim_secrets: None,
                args: None,
                env: None,
                process_mode: None,
            },
        })
    }

    /// The template resource for pool `name` on `runtime`: the typed
    /// fields plus `args`, `env`, `sidecars`, `processMode` and
    /// `claimSecrets` as JSON (see the module docs). The one mapping from a
    /// spec to Fleet.
    pub fn template_json(&self, name: &str, runtime: &RuntimeKind) -> Result<Value> {
        self.validate(runtime)?;
        let spec = self.typed_template(runtime)?;
        let mut v = json!({
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxTemplate",
            "metadata": {"namespace": name, "name": name},
            "spec": serde_json::to_value(&spec).map_err(|e| Error::InvalidArgument(e.to_string()))?,
        });
        let vm = &mut v["spec"]["vmTemplate"];
        // Enabling these as typed fields after the libs/fleet re-mirror is
        // one line each in `typed_template`.
        if let Some(args) = self.args.as_ref().filter(|a| !a.is_empty()) {
            vm["args"] = json!(args);
        }
        if !self.env.is_empty() {
            vm["env"] = json!(self.env);
        }
        if !self.sidecars.is_empty() {
            vm["sidecars"] = crate::sidecars_json(&self.sidecars);
        }
        if let Some(mode) = self.effective_process_mode() {
            vm["processMode"] = json!(mode.as_str());
        }
        if self.claim_secrets {
            vm["claimSecrets"] = json!(true);
        }
        Ok(v)
    }

    /// Reads a spec back from a template resource (or its `spec`, or its
    /// `vmTemplate`). Unknown fields are ignored.
    pub fn from_template_json(template: &Value) -> Self {
        let vm = if template["spec"]["vmTemplate"].is_object() {
            &template["spec"]["vmTemplate"]
        } else if template["vmTemplate"].is_object() {
            &template["vmTemplate"]
        } else {
            template
        };
        let strings = |v: &Value| -> Option<Vec<String>> {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
        };
        let map = |v: &Value| -> BTreeMap<String, String> {
            v.as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let services = vm["services"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        Some((
                            s["name"].as_str()?.to_string(),
                            u16::try_from(s["targetPort"].as_u64()?).ok()?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sidecars = vm["sidecars"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        Some(Sidecar {
                            name: s["name"].as_str()?.to_string(),
                            image: s["image"].as_str()?.to_string(),
                            command: strings(&s["command"]),
                            env: map(&s["env"]),
                            ports: s["ports"]
                                .as_array()
                                .map(|p| {
                                    p.iter()
                                        .filter_map(|n| {
                                            n.as_u64().and_then(|n| u16::try_from(n).ok())
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            args: strings(&s["args"]).filter(|a| !a.is_empty()),
                            cpu: s["cpu"].as_str().map(str::to_string),
                            memory: s["memory"].as_str().map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pull = vm["imagePullSecret"].as_str().map(str::to_string);
        Self {
            image: vm["containerDiskImage"].as_str().unwrap_or_default().into(),
            command: strings(&vm["command"]).filter(|c| !c.is_empty()),
            args: strings(&vm["args"]).filter(|a| !a.is_empty()),
            env: map(&vm["env"]),
            services,
            readiness: ReadinessProbe::from_json(&vm["probes"]),
            cpu: vm["cpuCores"]
                .as_u64()
                .map(|c| c.min(u64::from(u32::MAX)) as u32),
            memory_mb: vm["memory"].as_str().and_then(parse_memory_mib),
            efi: vm["firmware"].as_str() == Some("efi"),
            sidecars,
            // The shared ECR secret is implied by the image, not chosen.
            registry_secret: pull.filter(|p| p.starts_with(crate::REGISTRY_SECRET_PREFIX)),
            process_mode: vm["processMode"]
                .as_str()
                .and_then(|m| ProcessMode::parse(m).ok()),
            claim_secrets: vm["claimSecrets"].as_bool().unwrap_or(false),
        }
    }

    /// The fields of `self` that are set and differ from `current` (a
    /// template read back with [`Self::from_template_json`]). Unset fields
    /// are not compared; `env` and `services` match when every requested
    /// entry is the pool's. `image_matches` decides image equality (a pool's
    /// template holds the digest-pinned variant of the requested tag).
    pub fn diff(&self, current: &SandboxSpec, image_matches: bool) -> Vec<SpecDiff> {
        let mut out = vec![];
        let mut cmp = |field: &str, requested: Option<String>, pool: String| {
            if let Some(requested) = requested
                && requested != pool
            {
                out.push(SpecDiff {
                    field: field.into(),
                    pool,
                    requested,
                });
            }
        };
        let show = |v: &dyn fmt::Debug| format!("{v:?}");
        if !self.image.is_empty() && !image_matches {
            cmp("image", Some(self.image.clone()), current.image.clone());
        }
        let or_none = |v: &Option<Vec<String>>| match v {
            Some(v) => show(v),
            None => "none".to_string(),
        };
        cmp(
            "command",
            self.command.as_ref().map(|c| show(c)),
            or_none(&current.command),
        );
        cmp(
            "args",
            self.args.as_ref().map(|a| show(a)),
            or_none(&current.args),
        );
        // Env and services match when every requested entry is in the
        // pool's (a pool may declare more).
        let env_ok = self.env.iter().all(|(k, v)| current.env.get(k) == Some(v));
        cmp(
            "env",
            (!env_ok).then(|| show(&self.env)),
            show(&current.env),
        );
        let svc_ok = self
            .services
            .iter()
            .all(|(k, p)| current.services.get(k) == Some(p));
        cmp(
            "services",
            (!svc_ok).then(|| show(&self.services)),
            show(&current.services),
        );
        cmp(
            "readiness",
            self.readiness.as_ref().map(|r| show(r)),
            current
                .readiness
                .as_ref()
                .map(|r| show(r))
                .unwrap_or_else(|| "none".into()),
        );
        cmp(
            "cpu",
            self.cpu.map(|c| c.to_string()),
            current
                .cpu
                .map(|c| c.to_string())
                .unwrap_or_else(|| "default".into()),
        );
        cmp(
            "memory_mb",
            self.memory_mb.map(|m| m.to_string()),
            current
                .memory_mb
                .map(|m| m.to_string())
                .unwrap_or_else(|| "default".into()),
        );
        cmp(
            "efi",
            self.efi.then(|| "true".to_string()),
            current.efi.to_string(),
        );
        let mut sidecars = self.sidecars.clone();
        let mut pool_sidecars = current.sidecars.clone();
        sidecars.sort();
        pool_sidecars.sort();
        cmp(
            "sidecars",
            (!sidecars.is_empty()).then(|| show(&sidecars)),
            show(&pool_sidecars),
        );
        cmp(
            "registry_secret",
            self.registry_secret.clone(),
            current
                .registry_secret
                .clone()
                .unwrap_or_else(|| "none".into()),
        );
        cmp(
            "process_mode",
            self.process_mode.map(|m| m.as_str().to_string()),
            current
                .process_mode
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "Legacy".into()),
        );
        cmp(
            "claim_secrets",
            self.claim_secrets.then(|| "true".to_string()),
            current.claim_secrets.to_string(),
        );
        out
    }

    /// `current` with every set field of `self` laid over it: what
    /// `CloudOptions(pool=..., apply=True)` writes.
    pub fn overlay(&self, current: &SandboxSpec) -> SandboxSpec {
        let mut s = current.clone();
        if !self.image.is_empty() {
            s.image = self.image.clone();
        }
        if self.command.is_some() {
            s.command = self.command.clone().filter(|c| !c.is_empty());
        }
        if self.args.is_some() {
            s.args = self.args.clone().filter(|a| !a.is_empty());
        }
        if !self.env.is_empty() {
            s.env = self.env.clone();
        }
        if !self.services.is_empty() {
            s.services = self.services.clone();
        }
        if self.readiness.is_some() {
            s.readiness = self.readiness.clone();
        }
        if self.cpu.is_some() {
            s.cpu = self.cpu;
        }
        if self.memory_mb.is_some() {
            s.memory_mb = self.memory_mb;
        }
        s.efi |= self.efi;
        if !self.sidecars.is_empty() {
            s.sidecars = self.sidecars.clone();
        }
        if self.registry_secret.is_some() {
            s.registry_secret = self.registry_secret.clone();
        }
        if self.process_mode.is_some() {
            s.process_mode = self.process_mode;
        }
        s.claim_secrets |= self.claim_secrets;
        s
    }
}

/// One field where a pool's template differs from the requested spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecDiff {
    /// Spec field (`command`, `env`, `image`, ...).
    pub field: String,
    /// The pool template's value.
    pub pool: String,
    /// The requested value.
    pub requested: String,
}

/// The readable diff of a [`Error::PoolSpecMismatch`].
pub fn format_diffs(diffs: &[SpecDiff]) -> String {
    diffs
        .iter()
        .map(|d| {
            format!(
                "  {}: pool has {}, requested {}",
                d.field, d.pool, d.requested
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------------------------- terraform

fn hcl_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn hcl_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| hcl_str(s))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn hcl_map(m: &BTreeMap<String, String>, indent: &str) -> String {
    let mut s = "{\n".to_string();
    for (k, v) in m {
        s.push_str(&format!("{indent}  {} = {}\n", hcl_str(k), hcl_str(v)));
    }
    s.push_str(&format!("{indent}}}"));
    s
}

fn hcl_ident(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if label.starts_with(|c: char| c.is_ascii_digit()) {
        format!("p_{label}")
    } else {
        label
    }
}

/// A `fleets_pool` resource block (the cyclops-cs Terraform provider) for
/// pool `name` holding `spec` with `options` on `runtime`.
///
/// The provider's attributes (trycua/cloud#7889: `command`,
/// `claim_secrets`, `idle_ttl_seconds`, `ttl_policy`,
/// `ttl_seconds_after_created`; #7890: `args`, `env`, `process_mode`;
/// #7896: repeatable `sidecar` blocks with `name`, `image`, `command`,
/// `args`, `env`, `ports`, `cpu`, `memory`) are written as is; they need a
/// provider release that includes those PRs (newer than 0.3.0). A spec with a
/// `cua-registry-*` pull secret also
/// gets a `fleets_registry_secret` resource (#7890) whose username and
/// password are Terraform variables, never the credentials themselves.
pub fn terraform_pool_block(
    name: &str,
    spec: &SandboxSpec,
    options: &PoolOptions,
    runtime: &RuntimeKind,
) -> String {
    let label = hcl_ident(name);
    let mut out = format!("resource \"fleets_pool\" {} {{\n", hcl_str(&label));
    let mut attr = |k: &str, v: String| out.push_str(&format!("  {k} = {v}\n"));
    attr("name", hcl_str(name));
    if !options.autoscaled() {
        attr("replicas", options.spec_replicas().to_string());
    }
    // Required by the provider: Fleet's defaults when the spec leaves them.
    attr("cpu_cores", spec.cpu.unwrap_or(4).to_string());
    attr(
        "memory",
        hcl_str(
            &spec
                .memory_mb
                .map(|m| format!("{m}Mi"))
                .unwrap_or_else(|| "4Gi".into()),
        ),
    );
    attr("container_disk_image", hcl_str(&spec.image));
    attr("runtime", hcl_str(crate::runtime_name(runtime)));
    if spec.efi {
        attr("firmware", hcl_str("efi"));
    }
    let pull = spec
        .typed_template(runtime)
        .ok()
        .and_then(|t| t.vm_template.image_pull_secret);
    if let Some(p) = &pull {
        attr("image_pull_secret", hcl_str(p));
    }
    if let Some(r) = &spec.readiness {
        let probe = r.to_json()["readinessProbe"].to_string();
        attr("readiness_probe_json", format!("jsonencode({probe})"));
    }
    // Released in the provider (trycua/cloud#7889).
    if let Some(c) = spec.command.as_ref().filter(|c| !c.is_empty()) {
        attr("command", hcl_list(c));
    }
    if spec.claim_secrets {
        attr("claim_secrets", "true".into());
    }
    if let Some(t) = options.idle_ttl {
        attr("idle_ttl_seconds", secs_u32(t).to_string());
    }
    if let Some(p) = options.ttl_policy {
        attr("ttl_policy", hcl_str(p.as_str()));
    }
    if let Some(t) = options.pool_ttl {
        attr("ttl_seconds_after_created", secs_u32(t).to_string());
    }
    // trycua/cloud#7890.
    if let Some(a) = spec.args.as_ref().filter(|a| !a.is_empty()) {
        attr("args", hcl_list(a));
    }
    if !spec.env.is_empty() {
        attr("env", hcl_map(&spec.env, "  "));
    }
    if let Some(m) = spec.effective_process_mode() {
        attr("process_mode", hcl_str(m.as_str()));
    }
    // trycua/cloud#7896.
    for s in &spec.sidecars {
        out.push('\n');
        for line in sidecar_block(s).lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    for (svc, port) in &spec.services {
        out.push_str(&format!(
            "\n  service {{\n    name        = {}\n    target_port = {port}\n  }}\n",
            hcl_str(svc)
        ));
    }
    if let Some(a) = options.autoscaling() {
        out.push_str("\n  autoscaling {\n");
        for (k, v) in [
            ("min_pool_size", a.min_pool_size),
            ("initial_pool_size", a.initial_pool_size),
            ("max_pool_size", a.max_pool_size),
        ] {
            if let Some(v) = v {
                out.push_str(&format!("    {k} = {v}\n"));
            }
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n");
    if let Some(secret) = spec
        .registry_secret
        .as_ref()
        .filter(|s| s.starts_with(crate::REGISTRY_SECRET_PREFIX))
    {
        out.push_str(&registry_secret_block(&label, secret, &spec.image));
    }
    out
}

/// One `sidecar { ... }` block.
fn sidecar_block(s: &Sidecar) -> String {
    let mut b = format!(
        "sidecar {{\n  name = {}\n  image = {}\n",
        hcl_str(&s.name),
        hcl_str(&s.image)
    );
    if let Some(c) = s.command.as_ref().filter(|c| !c.is_empty()) {
        b.push_str(&format!("  command = {}\n", hcl_list(c)));
    }
    if let Some(a) = s.args.as_ref().filter(|a| !a.is_empty()) {
        b.push_str(&format!("  args = {}\n", hcl_list(a)));
    }
    if !s.env.is_empty() {
        b.push_str(&format!("  env = {}\n", hcl_map(&s.env, "  ")));
    }
    if !s.ports.is_empty() {
        b.push_str(&format!(
            "  ports = [{}]\n",
            s.ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(c) = s.cpu.as_ref().filter(|c| !c.is_empty()) {
        b.push_str(&format!("  cpu = {}\n", hcl_str(c)));
    }
    if let Some(m) = s.memory.as_ref().filter(|m| !m.is_empty()) {
        b.push_str(&format!("  memory = {}\n", hcl_str(m)));
    }
    b.push('}');
    b
}

/// The `fleets_registry_secret` (trycua/cloud#7890) for pool resource
/// `label`'s pull secret, with its credentials as variables (Fleet never
/// returns a Secret's contents, and an export never inlines them).
fn registry_secret_block(label: &str, secret: &str, image: &str) -> String {
    let registry = cua_image::registry_of(image);
    let var = |what: &str| format!("{label}_registry_{what}");
    format!(
        "\nvariable {} {{\n  type = string\n}}\n\
         \nvariable {} {{\n  type      = string\n  sensitive = true\n}}\n\
         \nresource \"fleets_registry_secret\" {} {{\n  namespace = fleets_pool.{label}.namespace\n  \
         name      = {}\n  registry  = {}\n  username  = var.{}\n  password  = var.{}\n}}\n",
        hcl_str(&var("username")),
        hcl_str(&var("password")),
        hcl_str(label),
        hcl_str(secret),
        hcl_str(registry),
        var("username"),
        var("password"),
    )
}

// ------------------------------------------------------------------- apply

impl FleetClient {
    /// Reconciles pool `name` (= namespace = template) to run `spec` with
    /// `options`: the one pool writer. The image is resolved to the variant
    /// the runtime runs, digest-pinned; the pool is written, then the
    /// template, and a pool this call created is deleted again if the
    /// template fails.
    pub async fn apply(
        &self,
        name: &str,
        spec: &SandboxSpec,
        options: &PoolOptions,
    ) -> Result<PoolHandle> {
        self.apply_with_credentials(name, spec, options, None).await
    }

    /// [`Self::apply`] that first writes `credentials` to
    /// `spec.registry_secret` in the pool namespace (never logged).
    pub async fn apply_with_credentials(
        &self,
        name: &str,
        spec: &SandboxSpec,
        options: &PoolOptions,
        credentials: Option<&crate::RegistryCredentials>,
    ) -> Result<PoolHandle> {
        cyclops_sdk::validate_dns_label(name)?;
        options.validate()?;
        crate::check_cloud_size(spec.cpu, spec.memory_mb)?;
        let resolved = crate::runtime::resolve_fleet_image_with(
            options.runtime.clone(),
            &spec.image,
            credentials,
        )
        .await?;
        let mut spec = spec.clone();
        spec.image = resolved.image;
        let runtime = resolved.runtime;
        let template = spec.template_json(name, &runtime)?;
        let pool = self
            .sdk()
            .reconcile_pool(crate::pool::pool_request(name, options))
            .await?;
        let result = async {
            let lifecycle = options.lifecycle_patch();
            if !lifecycle.is_empty() {
                self.patch_pool_json(name, json!({"spec": lifecycle}))
                    .await?;
            }
            if let (Some(secret), Some(creds)) = (&spec.registry_secret, credentials) {
                let registry = creds
                    .registry
                    .clone()
                    .unwrap_or_else(|| cua_image::registry_of(&spec.image).to_string());
                self.put_registry_secret(name, secret, &registry, creds)
                    .await?;
            }
            self.reconcile_template_json(template).await?;
            self.sdk()
                .get_template(name.to_string(), name.to_string())
                .await
                .map_err(Error::from)
        }
        .await;
        match result {
            Ok(template) => Ok(PoolHandle {
                pool,
                template: Some(template),
                runtime: Some(runtime),
                claim_ttl: options.claim_ttl,
            }),
            Err(err) => {
                if let Err(rollback) = self.sdk().delete_pool(pool).await {
                    tracing::warn!(pool = %name, error = %rollback,
                        "failed to roll back Fleet pool after template reconcile failure");
                }
                Err(err)
            }
        }
    }

    /// The pool's template as a [`SandboxSpec`] and its runtime.
    pub async fn pool_template(&self, pool: &str) -> Result<(SandboxSpec, RuntimeKind, Value)> {
        let handle = self.get_pool(pool).await?;
        let t = self
            .sdk()
            .get_template(
                handle.pool.metadata.namespace.clone(),
                handle.pool.spec.sandbox_template_ref.name.clone(),
            )
            .await?;
        let runtime = t
            .spec
            .vm_template
            .runtime
            .clone()
            .unwrap_or(RuntimeKind::Kubevirt);
        let raw = self
            .raw(
                "GET",
                self.k8s_url(
                    &handle.pool.metadata.namespace,
                    "osgymsandboxtemplates",
                    Some(&handle.pool.spec.sandbox_template_ref.name),
                ),
                None,
            )
            .await
            .ok()
            .filter(|(s, _)| *s == 200)
            .map(|(_, v)| v)
            .unwrap_or_else(|| serde_json::to_value(&t).unwrap_or(Value::Null));
        Ok((SandboxSpec::from_template_json(&raw), runtime, raw))
    }

    /// The pool resource as [`PoolOptions`] plus its template as a
    /// [`SandboxSpec`] (what `cua fleet pool export` prints).
    pub async fn export_pool(&self, pool: &str) -> Result<(SandboxSpec, PoolOptions, RuntimeKind)> {
        let (spec, runtime, _) = self.pool_template(pool).await?;
        let raw = self.get_pool_json(pool).await?.ok_or_else(|| {
            Error::Sdk(cyclops_sdk::SdkError::status("get pool", 404, b"not found"))
        })?;
        let options = PoolOptions::from_pool_json(&raw, Some(runtime.clone()));
        Ok((spec, options, runtime))
    }

    /// Compares the set fields of `requested` with pool `pool`'s template:
    /// [`Error::PoolSpecMismatch`] with a readable diff when they differ.
    /// The image matches when it is the template's, or resolves to it.
    pub async fn check_pool_spec(&self, pool: &str, requested: &SandboxSpec) -> Result<()> {
        let (current, runtime, _) = self.pool_template(pool).await?;
        let image_matches = self.image_matches(requested, &current, &runtime).await;
        let diffs = requested.diff(&current, image_matches);
        if diffs.is_empty() {
            Ok(())
        } else {
            Err(Error::PoolSpecMismatch {
                pool: pool.to_string(),
                diffs,
            })
        }
    }

    async fn image_matches(
        &self,
        requested: &SandboxSpec,
        current: &SandboxSpec,
        runtime: &RuntimeKind,
    ) -> bool {
        if requested.image.is_empty() || requested.image == current.image {
            return true;
        }
        match crate::runtime::resolve_fleet_image_with(
            Some(runtime.clone()),
            &requested.image,
            None,
        )
        .await
        {
            Ok(r) => r.image == current.image,
            Err(_) => false,
        }
    }

    /// `CloudOptions(pool=..., apply=True)`: lays the set fields of
    /// `requested` over pool `pool`'s template and writes the template
    /// (the pool's capacity settings are kept). A no-op when nothing
    /// differs.
    pub async fn apply_pool_template(
        &self,
        pool: &str,
        requested: &SandboxSpec,
        credentials: Option<&crate::RegistryCredentials>,
    ) -> Result<()> {
        let (current, runtime, _) = self.pool_template(pool).await?;
        let image_matches = self.image_matches(requested, &current, &runtime).await;
        if requested.diff(&current, image_matches).is_empty() {
            return Ok(());
        }
        let mut merged = requested.overlay(&current);
        if !requested.image.is_empty() && !image_matches {
            merged.image = crate::runtime::resolve_fleet_image_with(
                Some(runtime.clone()),
                &requested.image,
                credentials,
            )
            .await?
            .image;
        } else {
            merged.image = current.image.clone();
        }
        let handle = self.get_pool(pool).await?;
        let ns = handle.pool.metadata.namespace.clone();
        let tname = handle.pool.spec.sandbox_template_ref.name.clone();
        if let (Some(secret), Some(creds)) = (&merged.registry_secret, credentials) {
            let registry = creds
                .registry
                .clone()
                .unwrap_or_else(|| cua_image::registry_of(&merged.image).to_string());
            self.put_registry_secret(&ns, secret, &registry, creds)
                .await?;
        }
        let mut template = merged.template_json(&tname, &runtime)?;
        template["metadata"]["namespace"] = json!(ns);
        self.reconcile_template_json(template).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RESERVED_SERVICE_NAMES;

    #[test]
    fn warm_is_an_explicit_floor_of_one() {
        let o = PoolOptions {
            warm: Some(true),
            ..Default::default()
        };
        let a = o.autoscaling().unwrap();
        assert_eq!(a.min_pool_size, Some(1));
        assert_eq!(a.initial_pool_size, Some(1));
        assert_eq!(a.max_pool_size, Some(DEFAULT_MAX_POOL_SIZE));
        assert_eq!(o.spec_replicas(), 1);
        let cold = PoolOptions {
            warm: Some(false),
            max_pool_size: Some(3),
            ..Default::default()
        };
        let a = cold.autoscaling().unwrap();
        assert_eq!(
            (a.min_pool_size, a.initial_pool_size, a.max_pool_size),
            (Some(0), Some(0), Some(3))
        );
        assert_eq!(cold.spec_replicas(), 0);
        let fixed = PoolOptions::default();
        assert!(fixed.autoscaling().is_none());
        assert_eq!(fixed.spec_replicas(), 1);
    }

    #[test]
    fn lifecycle_fields_use_7886_names() {
        let o = PoolOptions {
            idle_ttl: Some(Duration::from_secs(3600)),
            ttl_policy: Some(TtlPolicy::Cascade),
            ..Default::default()
        };
        assert_eq!(
            Value::Object(o.lifecycle_patch()),
            json!({"idleTtlSeconds": 3600, "ttlPolicy": "Cascade"})
        );
        assert!(PoolOptions::default().lifecycle_patch().is_empty());
        let back = PoolOptions::from_pool_json(
            &json!({"spec": {"replicas": 1, "idleTtlSeconds": 3600, "ttlPolicy": "Cascade",
                "autoscaling": {"minPoolSize": 1, "maxPoolSize": 4}}}),
            None,
        );
        assert_eq!(back.idle_ttl, o.idle_ttl);
        assert_eq!(back.ttl_policy, o.ttl_policy);
        assert_eq!(back.warm, Some(true));
        assert_eq!(back.max_pool_size, Some(4));
    }

    #[test]
    fn template_round_trips() {
        let mut s = SandboxSpec::new("ghcr.io/x/y@sha256:ab");
        s.command = Some(vec!["python".into(), "-m".into(), "srv".into()]);
        s.services = [("mcp".to_string(), 8765)].into();
        s.readiness = Some(ReadinessProbe::Http {
            port: 8765,
            path: "/health".into(),
        });
        s.cpu = Some(2);
        s.memory_mb = Some(4096);
        s.claim_secrets = true;
        let t = s.template_json("cua-e2e-x", &RuntimeKind::Gvisor).unwrap();
        let vm = &t["spec"]["vmTemplate"];
        assert_eq!(vm["containerDiskImage"], "ghcr.io/x/y@sha256:ab");
        assert_eq!(vm["memory"], "4096Mi");
        assert_eq!(vm["claimSecrets"], true);
        assert_eq!(vm["probes"]["readinessProbe"]["httpGet"]["path"], "/health");
        assert_eq!(t["metadata"]["name"], "cua-e2e-x");
        let back = SandboxSpec::from_template_json(&t);
        let mut want = s.clone();
        want.process_mode = s.effective_process_mode();
        assert_eq!(back, want);
        assert!(s.diff(&back, true).is_empty());
    }

    #[test]
    fn diff_compares_only_set_fields() {
        let pool = SandboxSpec {
            image: "img@sha256:1".into(),
            command: Some(vec!["a".into()]),
            cpu: Some(4),
            services: [("env".to_string(), 3211)].into(),
            ..Default::default()
        };
        assert!(SandboxSpec::default().diff(&pool, true).is_empty());
        let req = SandboxSpec {
            command: Some(vec!["b".into()]),
            cpu: Some(4),
            ..Default::default()
        };
        let d = req.diff(&pool, true);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "command");
        assert!(format_diffs(&d).contains("command: pool has [\"a\"], requested [\"b\"]"));
        let img = SandboxSpec::new("other:1");
        assert_eq!(img.diff(&pool, false)[0].field, "image");
        let merged = req.overlay(&pool);
        assert_eq!(merged.command, req.command);
        assert_eq!(merged.services, pool.services);
    }

    #[test]
    fn process_fields_run_everywhere_and_legacy_refuses_them_on_kubevirt() {
        let mut s = SandboxSpec::new("img");
        s.command = Some(vec!["x".into()]);
        s.validate(&RuntimeKind::Gvisor).unwrap();
        // Run (the default with a process field) runs it on KubeVirt; an
        // explicit Legacy keeps the refusal.
        assert_eq!(s.effective_process_mode(), Some(ProcessMode::Run));
        s.validate(&RuntimeKind::Kubevirt).unwrap();
        let t = s.template_json("p", &RuntimeKind::Kubevirt).unwrap();
        assert_eq!(t["spec"]["vmTemplate"]["processMode"], "Run");
        s.process_mode = Some(ProcessMode::Legacy);
        let e = s.validate(&RuntimeKind::Kubevirt).unwrap_err();
        assert!(matches!(e, Error::Unsupported(_)), "{e}");
        s.validate(&RuntimeKind::Gvisor).unwrap();
        // No process field: no processMode is written.
        let plain = SandboxSpec::new("img");
        assert_eq!(plain.effective_process_mode(), None);
        let t = plain.template_json("p", &RuntimeKind::Gvisor).unwrap();
        assert!(t["spec"]["vmTemplate"].get("processMode").is_none());
        // Env alone switches to Run and is written on both runtimes.
        let env = SandboxSpec {
            env: [("K".to_string(), "v".to_string())].into(),
            ..SandboxSpec::new("img")
        };
        for rt in [RuntimeKind::Gvisor, RuntimeKind::Kubevirt] {
            let t = env.template_json("p", &rt).unwrap();
            assert_eq!(t["spec"]["vmTemplate"]["env"]["K"], "v");
            assert_eq!(t["spec"]["vmTemplate"]["processMode"], "Run");
        }
        // KubeVirt Run: args need a command, env values one line.
        let args_only = SandboxSpec {
            args: Some(vec!["--port".into(), "1".into()]),
            ..SandboxSpec::new("img")
        };
        assert!(args_only.validate(&RuntimeKind::Kubevirt).is_err());
        args_only.validate(&RuntimeKind::Gvisor).unwrap();
        let multi = SandboxSpec {
            command: Some(vec!["srv".into()]),
            env: [("K".to_string(), "a\nb".to_string())].into(),
            ..SandboxSpec::new("img")
        };
        assert!(multi.validate(&RuntimeKind::Kubevirt).is_err());
        multi.validate(&RuntimeKind::Gvisor).unwrap();
        let script = SandboxSpec {
            command: Some(vec!["python3".into(), "-c".into(), "a=1\nprint(a)".into()]),
            ..SandboxSpec::new("img")
        };
        let e = script
            .validate(&RuntimeKind::Kubevirt)
            .unwrap_err()
            .to_string();
        assert!(e.contains("command"), "{e}");
        script.validate(&RuntimeKind::Gvisor).unwrap();
        let tab = SandboxSpec {
            command: Some(vec!["srv".into()]),
            args: Some(vec!["a\tb".into()]),
            ..SandboxSpec::new("img")
        };
        assert!(tab.validate(&RuntimeKind::Kubevirt).is_err());
    }

    #[test]
    fn sidecars_run_on_both_runtimes_and_reserve_service_names() {
        let db = Sidecar {
            name: "db".into(),
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        };
        let spec = SandboxSpec {
            sidecars: vec![db],
            services: [("db".to_string(), 6379)].into(),
            ..SandboxSpec::new("img")
        };
        for rt in [RuntimeKind::Gvisor, RuntimeKind::Kubevirt] {
            let t = spec.template_json("p", &rt).unwrap();
            assert_eq!(
                t["spec"]["vmTemplate"]["sidecars"],
                json!([{"name": "db", "image": "redis:7-alpine", "ports": [6379]}])
            );
            for reserved in RESERVED_SERVICE_NAMES {
                let mut bad = spec.clone();
                bad.services.insert(reserved.to_string(), 8080);
                let e = bad.validate(&rt).unwrap_err().to_string();
                assert!(e.contains("reserved"), "{e}");
            }
        }
        // Without sidecars the names are free.
        let free = SandboxSpec {
            services: [("main".to_string(), 80), ("sc".to_string(), 81)].into(),
            ..SandboxSpec::new("img")
        };
        free.validate(&RuntimeKind::Gvisor).unwrap();
        // A pod sidecar cannot take spacesd's port; a KubeVirt companion can.
        let clash = SandboxSpec {
            sidecars: vec![Sidecar {
                ports: vec![3211],
                ..Sidecar::new("redis:7")
            }],
            services: [("env".to_string(), 3211)].into(),
            ..SandboxSpec::new("img")
        };
        assert!(clash.validate(&RuntimeKind::Gvisor).is_err());
        clash.validate(&RuntimeKind::Kubevirt).unwrap();
    }

    #[test]
    fn memory_quantities() {
        assert_eq!(parse_memory_mib("4Gi"), Some(4096));
        assert_eq!(parse_memory_mib("512Mi"), Some(512));
        assert_eq!(parse_memory_mib("1x"), None);
    }

    #[test]
    fn terraform_block_uses_provider_names() {
        let mut s = SandboxSpec::new("ghcr.io/trycua/linux:24.04");
        s.cpu = Some(2);
        s.memory_mb = Some(8192);
        s.services = [("env".to_string(), 3211)].into();
        s.command = Some(vec!["srv".into()]);
        s.args = Some(vec!["--port".into(), "8765".into()]);
        s.env = [("LOG".to_string(), "info".to_string())].into();
        s.process_mode = Some(ProcessMode::Run);
        s.claim_secrets = true;
        let o = PoolOptions {
            warm: Some(true),
            idle_ttl: Some(Duration::from_secs(86400)),
            ttl_policy: Some(TtlPolicy::Cascade),
            pool_ttl: Some(Duration::from_secs(604800)),
            ..Default::default()
        };
        let hcl = terraform_pool_block("cua-e2e-x", &s, &o, &RuntimeKind::Gvisor);
        for want in [
            "resource \"fleets_pool\" \"cua_e2e_x\" {",
            "  name = \"cua-e2e-x\"",
            "  cpu_cores = 2",
            "  memory = \"8192Mi\"",
            "  container_disk_image = \"ghcr.io/trycua/linux:24.04\"",
            "  runtime = \"gvisor\"",
            // In the provider (trycua/cloud#7889, #7890): real attributes.
            "\n  command = [\"srv\"]\n",
            "\n  claim_secrets = true\n",
            "\n  idle_ttl_seconds = 86400\n",
            "\n  ttl_policy = \"Cascade\"\n",
            "\n  ttl_seconds_after_created = 604800\n",
            "\n  args = [\"--port\", \"8765\"]\n",
            "\n  env = {\n    \"LOG\" = \"info\"\n  }\n",
            "\n  process_mode = \"Run\"\n",
            "    name        = \"env\"",
            "    target_port = 3211",
            "    min_pool_size = 1",
        ] {
            assert!(hcl.contains(want), "missing {want:?} in\n{hcl}");
        }
        assert!(
            !hcl.contains("  replicas"),
            "autoscaled pools leave replicas to KEDA"
        );
        assert!(!hcl.contains("fleets_registry_secret"), "{hcl}");
        // Without sidecars nothing is pending: the block has no comments.
        assert!(!hcl.contains('#'), "{hcl}");
    }

    #[test]
    fn terraform_sidecars_are_blocks_and_the_registry_secret_is_a_resource() {
        let mut s = SandboxSpec::new("ghcr.io/acme/agent:1");
        s.registry_secret = Some("cua-registry-0123456789abcdef".into());
        s.sidecars = vec![
            Sidecar {
                command: Some(vec!["redis-server".into()]),
                args: Some(vec!["--appendonly".into(), "yes".into()]),
                env: [("MODE".to_string(), "cache".to_string())].into(),
                cpu: Some("500m".into()),
                memory: Some("256Mi".into()),
                ports: vec![6379],
                ..Sidecar::new("redis:7")
            },
            Sidecar::new("busybox"),
        ];
        let hcl = terraform_pool_block("agent", &s, &PoolOptions::default(), &RuntimeKind::Gvisor);
        for want in [
            "  image_pull_secret = \"cua-registry-0123456789abcdef\"",
            "\n  sidecar {\n    name = \"redis\"\n    image = \"redis:7\"\n",
            "    command = [\"redis-server\"]\n",
            "    args = [\"--appendonly\", \"yes\"]\n",
            "    env = {\n      \"MODE\" = \"cache\"\n    }\n",
            "    ports = [6379]\n",
            "    cpu = \"500m\"\n",
            "    memory = \"256Mi\"\n  }\n",
            "\n  sidecar {\n    name = \"busybox\"\n    image = \"busybox\"\n  }\n",
            "\nresource \"fleets_registry_secret\" \"agent\" {",
            "  namespace = fleets_pool.agent.namespace",
            "  name      = \"cua-registry-0123456789abcdef\"",
            "  registry  = \"ghcr.io\"",
            "  username  = var.agent_registry_username",
            "  password  = var.agent_registry_password",
            "variable \"agent_registry_password\" {\n  type      = string\n  sensitive = true\n}",
        ] {
            assert!(hcl.contains(want), "missing {want:?} in\n{hcl}");
        }
        // Nothing is commented out any more, and no credential is inlined.
        assert!(!hcl.contains('#'), "{hcl}");
        assert!(!hcl.contains("password  = \""), "{hcl}");
        // Sidecars sit inside the fleets_pool block, before its closing brace.
        let pool_end = hcl.find("\n}\n").unwrap();
        assert!(hcl.find("sidecar {").unwrap() < pool_end, "{hcl}");
    }
}
