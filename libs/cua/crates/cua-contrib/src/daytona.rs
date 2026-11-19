//! Daytona (<https://daytona.io>): sandboxes from snapshots.
//!
//! - **Image**: Daytona runs snapshots, built once from a registry image
//!   (`POST /snapshots {name, imageName}`; the image must be pinned by tag
//!   or digest, never `latest`). The snapshot is named by [`template_key`]
//!   over the image digest and the shape, so later creates reuse it and a
//!   moved tag builds a new one. A snapshot in `error` is rebuilt.
//! - **Entrypoint**: Daytona runs the image's entrypoint (the create's
//!   `command` replaces it through the snapshot's `entrypoint`), with the
//!   sandbox environment, so the spacesd token is delivered at create.
//! - **Ports**: `GET /sandbox/{id}/ports/{port}/preview-url` returns each
//!   port's URL; sandboxes are created `public` so cua-spacesd (which
//!   authenticates with its own token) is reachable without Daytona's
//!   preview-token header.
//! - **Auth**: `DAYTONA_API_KEY` (bearer), `DAYTONA_API_URL` (default
//!   `https://app.daytona.io/api`), optional `DAYTONA_TARGET` (region).
//!
//! API: <https://github.com/daytona/clients/blob/main/openapi-specs/api.json>
//! (Apache-2.0).

use crate::{
    DAYTONA_ENV,
    common::{self, Api, CredentialStore, Secret, template_key},
};
use cua_sandbox_core::{
    Error, ImageMode, PortExposure, Provider, ProviderCapabilities, ProviderCreate,
    ProviderInstance, Result, RunKind, ServiceEndpoint, Status,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};

/// Default wall-clock lifetime (minutes) when the create sets none.
pub const DEFAULT_TTL_MINUTES: u64 = 60;

/// Daytona configuration.
#[derive(Clone)]
pub struct DaytonaConfig {
    /// API key.
    pub api_key: Option<Secret>,
    /// API base URL.
    pub api_url: String,
    /// Region (`us`, `eu`).
    pub target: Option<String>,
    /// Poll interval for snapshot builds and sandbox starts.
    pub poll: Duration,
}

impl std::fmt::Debug for DaytonaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaytonaConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("api_url", &self.api_url)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl DaytonaConfig {
    /// From `DAYTONA_API_KEY` (or `cua auth provider set daytona`),
    /// `DAYTONA_API_URL` and `DAYTONA_TARGET`.
    pub fn from_env() -> Self {
        Self {
            api_key: common::credential(&CredentialStore::default(), "daytona", DAYTONA_ENV)
                .map(|(s, _, _)| s),
            api_url: common::base_url("DAYTONA_API_URL", "https://app.daytona.io/api"),
            target: std::env::var("DAYTONA_TARGET")
                .ok()
                .filter(|t| !t.trim().is_empty()),
            poll: Duration::from_secs(3),
        }
    }
}

/// The Daytona provider.
pub struct Daytona {
    config: DaytonaConfig,
    builds: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Daytona {
    /// From the environment.
    pub fn from_env() -> Self {
        Self::new(DaytonaConfig::from_env())
    }

    /// With `config`.
    pub fn new(config: DaytonaConfig) -> Self {
        Self {
            config,
            builds: Mutex::new(HashMap::new()),
        }
    }

    fn api(&self) -> Result<Api> {
        let key = self
            .config
            .api_key
            .clone()
            .ok_or_else(|| common::not_configured("daytona", DAYTONA_ENV))?;
        Ok(Api::new(
            "daytona",
            self.config.api_url.clone(),
            vec![(
                "Authorization",
                Secret::new(format!("Bearer {}", key.expose())),
            )],
            "DAYTONA_API_KEY",
        ))
    }

    fn polls(&self, budget: Duration) -> u32 {
        (budget.as_secs() / self.config.poll.as_secs().max(1)).max(1) as u32 + 1
    }

    /// The snapshot for `spec`, built when missing (or failed). Returns its
    /// name.
    async fn ensure_snapshot(&self, api: &Api, spec: &ProviderCreate) -> Result<String> {
        let memory_gb = spec.memory_mb.div_ceil(1024).max(1);
        let entrypoint = spec.command.clone().filter(|c| !c.is_empty());
        let name = template_key(
            &spec.image,
            &[
                ("provider", "daytona".into()),
                (
                    "entrypoint",
                    entrypoint.clone().unwrap_or_default().join("\u{0}"),
                ),
                ("cpu", spec.cpus.to_string()),
                ("memory_gb", memory_gb.to_string()),
            ],
        );
        let lock = self
            .builds
            .lock()
            .unwrap()
            .entry(name.clone())
            .or_default()
            .clone();
        let _held = lock.lock().await;
        let existing: Snapshots = api
            .json(
                Method::GET,
                &format!("/snapshots?name={name}&limit=10"),
                None::<&()>,
            )
            .await?;
        if let Some(s) = existing.items.into_iter().find(|s| s.name == name) {
            match s.state.as_str() {
                "active" => {
                    tracing::info!(snapshot = %name, "reusing Daytona snapshot");
                    return Ok(name);
                }
                "error" | "build_failed" => {
                    tracing::warn!(snapshot = %name, "rebuilding a failed Daytona snapshot");
                    api.call(
                        Method::DELETE,
                        &format!("/snapshots/{}", s.id),
                        None::<&()>,
                        true,
                    )
                    .await?;
                }
                _ => {
                    self.wait_snapshot(api, &s.id, spec.timeout).await?;
                    return Ok(name);
                }
            }
        }
        if spec.image.digest.is_empty() && spec.image.pinned_ref.ends_with(":latest") {
            return Err(Error::UnsupportedImage(format!(
                "{}: Daytona snapshots need a pinned tag or digest, not `latest` (the registry \
                 could not be read to pin it)",
                spec.image.reference
            )));
        }
        tracing::info!(snapshot = %name, image = %spec.image.pinned_ref, "building Daytona snapshot");
        let mut body = json!({
            "name": name,
            "imageName": spec.image.pinned_ref,
            "cpu": spec.cpus,
            "memory": memory_gb,
        });
        if let Some(e) = &entrypoint {
            body["entrypoint"] = json!(e);
        }
        let created: SnapshotDoc = api.json(Method::POST, "/snapshots", Some(&body)).await?;
        self.wait_snapshot(api, &created.id, spec.timeout).await?;
        Ok(name)
    }

    async fn wait_snapshot(&self, api: &Api, id: &str, budget: Duration) -> Result<()> {
        let path = format!("/snapshots/{id}");
        common::poll(
            &format!("Daytona snapshot {id}"),
            budget,
            self.config.poll,
            self.polls(budget),
            || async {
                let s: SnapshotDoc = api.json(Method::GET, &path, None::<&()>).await?;
                match s.state.as_str() {
                    "active" => Ok(Some(())),
                    "error" | "build_failed" => Err(Error::UnsupportedImage(format!(
                        "Daytona snapshot {} failed: {}",
                        s.name,
                        s.error_reason.unwrap_or_else(|| "no reason given".into())
                    ))),
                    _ => Ok(None),
                }
            },
        )
        .await
    }

    async fn wait_started(&self, api: &Api, id: &str, budget: Duration) -> Result<SandboxDoc> {
        let path = format!("/sandbox/{id}");
        common::poll(
            &format!("Daytona sandbox {id} to start"),
            budget,
            self.config.poll,
            self.polls(budget),
            || async {
                let s: SandboxDoc = api.json(Method::GET, &path, None::<&()>).await?;
                match s.state.as_deref() {
                    Some("started") => Ok(Some(s)),
                    Some(st @ ("error" | "build_failed" | "destroyed")) => Err(Error::Runtime(
                        cua_sandbox_core::RuntimeError::Other(format!(
                            "Daytona sandbox {id} is {st}: {}",
                            s.error_reason.clone().unwrap_or_default()
                        )),
                    )),
                    _ => Ok(None),
                }
            },
        )
        .await
    }

    /// Resolves the preview URLs of `ports` into the instance.
    async fn with_endpoints(
        &self,
        api: &Api,
        mut instance: ProviderInstance,
        ports: &[u16],
    ) -> Result<ProviderInstance> {
        for port in ports {
            let p: PreviewUrl = api
                .json(
                    Method::GET,
                    &format!("/sandbox/{}/ports/{port}/preview-url", instance.id),
                    None::<&()>,
                )
                .await?;
            instance.endpoints.insert(
                *port,
                ServiceEndpoint {
                    url: p.url.trim_end_matches('/').to_string(),
                    headers: vec![],
                },
            );
        }
        instance.details.insert(
            PORTS_KEY.into(),
            ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
        Ok(instance)
    }

    fn instance(s: &SandboxDoc) -> ProviderInstance {
        let status = match s.state.as_deref() {
            Some("started") => Status::Running,
            Some("stopped" | "archived") => Status::Stopped,
            Some("paused") => Status::Suspended,
            Some(
                "creating" | "starting" | "restoring" | "pending_build" | "building_snapshot"
                | "pulling_snapshot" | "resuming",
            ) => Status::Provisioning,
            Some(other) => Status::Unknown(other.into()),
            None => Status::Unknown("unknown".into()),
        };
        let name = s
            .labels
            .get("cua.name")
            .cloned()
            .or_else(|| s.name.clone())
            .unwrap_or_else(|| s.id.clone());
        let mut i = ProviderInstance::new(&s.id, name, status);
        if let Some(sn) = &s.snapshot {
            i.details.insert("snapshot".into(), sn.clone());
        }
        if let Some(t) = &s.target {
            i.details.insert("target".into(), t.clone());
        }
        i
    }
}

/// Instance detail listing the ports whose preview URLs were resolved.
const PORTS_KEY: &str = "_ports";

#[derive(Deserialize)]
struct Snapshots {
    #[serde(default)]
    items: Vec<SnapshotDoc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDoc {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    error_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    error_reason: Option<String>,
}

#[derive(Deserialize)]
struct SandboxList {
    #[serde(default)]
    items: Vec<SandboxDoc>,
}

#[derive(Deserialize)]
struct PreviewUrl {
    url: String,
}

#[async_trait::async_trait]
impl Provider for Daytona {
    fn name(&self) -> &'static str {
        "daytona"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            runtime: "daytona",
            arches: vec!["amd64"],
            image_mode: ImageMode::Template,
            ports: PortExposure::Https,
            command: true,
            env_to_entrypoint: true,
            // Private registries are registered in the Daytona dashboard,
            // not per sandbox.
            private_registry: false,
            suspend: true,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: DAYTONA_ENV,
            gpus: vec![],
        }
    }

    fn check_configured(&self) -> Result<()> {
        self.api().map(|_| ())
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        let api = self.api()?;
        let snapshot = self.ensure_snapshot(&api, spec).await?;
        let ttl_minutes = spec
            .ttl
            .map(|t| t.as_secs().div_ceil(60).max(1))
            .unwrap_or(DEFAULT_TTL_MINUTES);
        let mut body = json!({
            "name": spec.name,
            "snapshot": snapshot,
            "env": spec.env,
            "labels": spec.labels,
            "public": true,
            "autoStopInterval": 0,
            "ttlMinutes": ttl_minutes,
        });
        if let Some(t) = &self.config.target {
            body["target"] = json!(t);
        }
        let created: SandboxDoc = api.json(Method::POST, "/sandbox", Some(&body)).await?;
        let started = match self.wait_started(&api, &created.id, spec.timeout).await {
            Ok(s) => s,
            Err(e) => {
                let _ = self.delete(&created.id).await;
                return Err(e);
            }
        };
        let instance = Self::instance(&started);
        match self.with_endpoints(&api, instance, &spec.ports).await {
            Ok(i) => Ok(i),
            Err(e) => {
                let _ = self.delete(&created.id).await;
                Err(e)
            }
        }
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        let api = self.api()?;
        let s: SandboxDoc = api
            .json(Method::GET, &format!("/sandbox/{id}"), None::<&()>)
            .await?;
        let instance = Self::instance(&s);
        if instance.status != Status::Running {
            return Ok(instance);
        }
        // Every guest port's URL is one call; resolve the common ones.
        let ports = [cua_sandbox_core::ENV_PORT];
        self.with_endpoints(&api, instance, &ports).await
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        let labels =
            url::form_urlencoded::byte_serialize(br#"{"cua.managed":"true"}"#).collect::<String>();
        let l: SandboxList = self
            .api()?
            .json(
                Method::GET,
                &format!("/sandbox?labels={labels}&limit=100"),
                None::<&()>,
            )
            .await?;
        Ok(l.items.iter().map(Self::instance).collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.api()?
            .call(Method::DELETE, &format!("/sandbox/{id}"), None::<&()>, true)
            .await
    }

    fn endpoint(&self, instance: &ProviderInstance, port: u16) -> Result<ServiceEndpoint> {
        if let Some(e) = instance.endpoints.get(&port) {
            return Ok(e.clone());
        }
        // Standard preview URLs are `https://{port}-{sandboxId}.{proxyDomain}`:
        // derive any port from one the API returned.
        let derived = instance.endpoints.iter().find_map(|(p, e)| {
            let prefix = format!("{p}-{}.", instance.id);
            let (scheme, rest) = e.url.split_once("://")?;
            rest.strip_prefix(&prefix).map(|domain| ServiceEndpoint {
                url: format!("{scheme}://{port}-{}.{domain}", instance.id),
                headers: e.headers.clone(),
            })
        });
        derived.ok_or_else(|| {
            Error::InvalidArgument(format!(
                "Daytona port {port} of {} has no preview URL (declare it with --port when \
                 creating the sandbox)",
                instance.id
            ))
        })
    }

    async fn suspend(&self, id: &str) -> Result<()> {
        self.api()?
            .call(
                Method::POST,
                &format!("/sandbox/{id}/stop"),
                None::<&()>,
                false,
            )
            .await
    }

    async fn resume(&self, id: &str) -> Result<ProviderInstance> {
        let api = self.api()?;
        api.call(
            Method::POST,
            &format!("/sandbox/{id}/start"),
            None::<&()>,
            false,
        )
        .await?;
        let s = self
            .wait_started(&api, id, Duration::from_secs(600))
            .await?;
        Ok(Self::instance(&s))
    }

    async fn keep_alive(&self, id: &str, duration: Duration) -> Result<()> {
        let minutes = duration.as_secs().div_ceil(60).max(1);
        self.api()?
            .call(
                Method::POST,
                &format!("/sandbox/{id}/ttl/{minutes}"),
                None::<&()>,
                false,
            )
            .await
    }
}
