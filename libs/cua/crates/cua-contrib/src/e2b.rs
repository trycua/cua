//! E2B (<https://e2b.dev>): Firecracker microVMs from templates.
//!
//! - **Image**: E2B runs templates, not registry images, so the first create
//!   of an image builds one (`POST /v3/templates`, then `POST
//!   /v2/templates/{id}/builds/{build}` with `fromImage` set to the pinned
//!   reference). The template is named by [`template_key`] over the image
//!   digest, the start command and the shape, so later creates (from any
//!   process) reuse it, and a moved tag builds a new one.
//! - **Entrypoint**: E2B does not run the image's `ENTRYPOINT`/`CMD`; the
//!   template's start command does, once, at build time, and every sandbox
//!   resumes from that memory snapshot. The start command is the image's
//!   entrypoint (or the create's `command`), and the ready command waits
//!   for cua-spacesd on 3211. The guest environment therefore does not
//!   reach the entrypoint: cua-spacesd starts in bootstrap mode and the SDK
//!   installs its token with `Init`.
//! - **Ports**: every guest port is `https://{port}-{sandboxID}.{domain}`
//!   (public traffic, the E2B default; cua-spacesd authenticates with its
//!   own token).
//! - **Auth**: `E2B_API_KEY` (header `X-API-Key`); `E2B_DOMAIN` /
//!   `E2B_API_URL` as in the official SDKs. `CUA_E2B_PORT_URL`
//!   (`http://host:{port}/{id}`-style template with `{port}`, `{id}` and
//!   `{domain}`) overrides the port URL for self-hosted deployments and tests.
//!
//! API: <https://github.com/e2b-dev/E2B/blob/main/spec/openapi.yml>.

use crate::{
    E2B_ENV,
    common::{self, Api, CredentialStore, Secret, template_key},
    image_config::{self, ImageConfigSource},
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

/// Default sandbox lifetime (the create's TTL wins): one hour, the E2B
/// Hobby maximum.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// E2B configuration.
#[derive(Clone)]
pub struct E2bConfig {
    /// API key.
    pub api_key: Option<Secret>,
    /// Control-plane base URL.
    pub api_url: String,
    /// Sandbox domain (`e2b.app`).
    pub domain: String,
    /// Port URL template (`{port}`, `{id}`, `{domain}`); `None`: the E2B
    /// proxy, `https://{port}-{id}.{domain}`.
    pub port_url: Option<String>,
    /// Template build budget and poll interval.
    pub build_poll: Duration,
}

impl std::fmt::Debug for E2bConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2bConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("api_url", &self.api_url)
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

impl E2bConfig {
    /// From `E2B_API_KEY` (or `cua auth provider set e2b`), `E2B_DOMAIN`,
    /// `E2B_API_URL` and `CUA_E2B_PORT_URL`.
    pub fn from_env() -> Self {
        let domain = std::env::var("E2B_DOMAIN")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "e2b.app".into());
        Self {
            api_key: common::credential(&CredentialStore::default(), "e2b", E2B_ENV)
                .map(|(s, _, _)| s),
            api_url: common::base_url("E2B_API_URL", &format!("https://api.{domain}")),
            domain,
            port_url: std::env::var("CUA_E2B_PORT_URL")
                .ok()
                .filter(|u| !u.trim().is_empty()),
            build_poll: Duration::from_secs(3),
        }
    }
}

/// The E2B provider.
pub struct E2b {
    config: E2bConfig,
    images: Arc<dyn ImageConfigSource>,
    /// One template build per key at a time in this process.
    builds: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl E2b {
    /// From the environment.
    pub fn from_env() -> Self {
        Self::new(E2bConfig::from_env())
    }

    /// With `config`, reading image configs from the registry.
    pub fn new(config: E2bConfig) -> Self {
        Self {
            config,
            images: image_config::registry(),
            builds: Mutex::new(HashMap::new()),
        }
    }

    /// Replaces where image configs come from (tests).
    pub fn with_image_configs(mut self, images: Arc<dyn ImageConfigSource>) -> Self {
        self.images = images;
        self
    }

    fn api(&self) -> Result<Api> {
        let key = self
            .config
            .api_key
            .clone()
            .ok_or_else(|| common::not_configured("e2b", E2B_ENV))?;
        Ok(Api::new(
            "e2b",
            self.config.api_url.clone(),
            vec![("X-API-Key", key)],
            "E2B_API_KEY",
        ))
    }

    fn build_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.builds
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// The template for `spec`: an existing ready one named by its key, or a
    /// new build. Returns the template id.
    async fn ensure_template(&self, api: &Api, spec: &ProviderCreate) -> Result<String> {
        let cfg = self
            .images
            .config(&spec.image, spec.registry_credentials.as_ref())
            .await?;
        let start = cfg.start_line(spec.command.as_deref()).ok_or_else(|| {
            Error::UnsupportedImage(format!(
                "{} has no ENTRYPOINT or CMD, and E2B needs a start command: pass a command",
                spec.image.reference
            ))
        })?;
        let ready =
            (spec.image.spacesd != Some(false)).then(|| ready_cmd(cua_sandbox_core::ENV_PORT));
        let user = if cfg.user.is_empty() {
            "root".to_string()
        } else {
            cfg.user.clone()
        };
        let name = template_key(
            &spec.image,
            &[
                ("provider", "e2b".into()),
                ("start", start.clone()),
                ("ready", ready.clone().unwrap_or_default()),
                ("user", user.clone()),
                ("cpu", spec.cpus.to_string()),
                ("memory", spec.memory_mb.to_string()),
            ],
        );
        let lock = self.build_lock(&name);
        let _held = lock.lock().await;
        if let Some(id) = self.ready_template(api, &name, spec.timeout).await? {
            tracing::info!(template = %name, "reusing E2B template");
            return Ok(id);
        }
        tracing::info!(template = %name, image = %spec.image.pinned_ref, "building E2B template");
        let created: TemplateCreated = api
            .json(
                Method::POST,
                "/v3/templates",
                Some(&json!({
                    "name": name,
                    "cpuCount": spec.cpus,
                    "memoryMB": spec.memory_mb,
                })),
            )
            .await?;
        let mut start_body = json!({
            "fromImage": spec.image.pinned_ref,
            "steps": [{"type": "USER", "args": [user]}],
            "startCmd": start,
        });
        if let Some(r) = &ready {
            start_body["readyCmd"] = json!(r);
        }
        if let Some(c) = &spec.registry_credentials {
            start_body["fromImageRegistry"] = json!({
                "type": "registry",
                "username": c.username,
                "password": c.password,
            });
        }
        api.call(
            Method::POST,
            &format!(
                "/v2/templates/{}/builds/{}",
                created.template_id, created.build_id
            ),
            Some(&start_body),
            false,
        )
        .await?;
        self.wait_build(api, &created.template_id, &created.build_id, spec.timeout)
            .await?;
        Ok(created.template_id)
    }

    /// The id of the template `name` when it has a ready build; waits for a
    /// build another process started. `None`: build it.
    async fn ready_template(
        &self,
        api: &Api,
        name: &str,
        budget: Duration,
    ) -> Result<Option<String>> {
        let alias: TemplateAlias = match api
            .json(
                Method::GET,
                &format!("/templates/aliases/{name}"),
                None::<&()>,
            )
            .await
        {
            Ok(a) => a,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let t: TemplateWithBuilds = api
            .json(
                Method::GET,
                &format!("/templates/{}", alias.template_id),
                None::<&()>,
            )
            .await?;
        if t.builds.iter().any(|b| b.status == "ready") {
            return Ok(Some(alias.template_id));
        }
        if let Some(b) = t
            .builds
            .iter()
            .find(|b| b.status == "building" || b.status == "waiting")
        {
            self.wait_build(api, &alias.template_id, &b.build_id, budget)
                .await?;
            return Ok(Some(alias.template_id));
        }
        // Only failed builds: build again under the same name.
        Ok(None)
    }

    async fn wait_build(
        &self,
        api: &Api,
        template: &str,
        build: &str,
        budget: Duration,
    ) -> Result<()> {
        let path = format!("/templates/{template}/builds/{build}/status");
        let polls = (budget.as_secs() / self.config.build_poll.as_secs().max(1)).max(1) as u32 + 1;
        common::poll(
            &format!("E2B template {template} build"),
            budget,
            self.config.build_poll,
            polls,
            || async {
                let info: BuildInfo = api.json(Method::GET, &path, None::<&()>).await?;
                match info.status.as_str() {
                    "ready" => Ok(Some(())),
                    "error" => Err(Error::UnsupportedImage(format!(
                        "E2B template build {build} failed: {}",
                        info.reason
                            .map(|r| r.message)
                            .unwrap_or_else(|| "no reason given".into())
                    ))),
                    _ => Ok(None),
                }
            },
        )
        .await
    }

    fn instance(&self, s: &SandboxDoc, name: Option<&str>) -> ProviderInstance {
        let status = match s.state.as_deref() {
            Some("paused") => Status::Suspended,
            Some("running") | None => Status::Running,
            Some(other) => Status::Unknown(other.into()),
        };
        let name = name
            .map(str::to_string)
            .or_else(|| s.metadata.get("cua.name").cloned())
            .unwrap_or_else(|| s.sandbox_id.clone());
        let mut i = ProviderInstance::new(&s.sandbox_id, name, status);
        i.details.insert("template".into(), s.template_id.clone());
        let domain = s
            .domain
            .clone()
            .unwrap_or_else(|| self.config.domain.clone());
        i.details.insert("domain".into(), domain);
        if let Some(t) = &s.traffic_access_token {
            // Kept for `endpoint`; `provider_details` never shows it.
            i.details.insert(TRAFFIC_TOKEN_KEY.into(), t.clone());
        }
        i
    }
}

/// Instance detail holding the traffic token of a sandbox created with
/// restricted public traffic (stripped from `provider_details`).
const TRAFFIC_TOKEN_KEY: &str = "_traffic_token";

/// Succeeds once something listens on `port` (E2B retries it).
fn ready_cmd(port: u16) -> String {
    format!("bash -c 'exec 3<>/dev/tcp/127.0.0.1/{port}'")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TemplateCreated {
    #[serde(rename = "templateID")]
    template_id: String,
    #[serde(rename = "buildID")]
    build_id: String,
}

#[derive(Deserialize)]
struct TemplateAlias {
    #[serde(rename = "templateID")]
    template_id: String,
}

#[derive(Deserialize)]
struct TemplateWithBuilds {
    #[serde(default)]
    builds: Vec<TemplateBuild>,
}

#[derive(Deserialize)]
struct TemplateBuild {
    #[serde(rename = "buildID")]
    build_id: String,
    status: String,
}

#[derive(Deserialize)]
struct BuildInfo {
    status: String,
    #[serde(default)]
    reason: Option<BuildReason>,
}

#[derive(Deserialize)]
struct BuildReason {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxDoc {
    #[serde(rename = "sandboxID")]
    sandbox_id: String,
    #[serde(rename = "templateID", default)]
    template_id: String,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    traffic_access_token: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[async_trait::async_trait]
impl Provider for E2b {
    fn name(&self) -> &'static str {
        "e2b"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            runtime: "firecracker",
            arches: vec!["amd64"],
            image_mode: ImageMode::Template,
            ports: PortExposure::Https,
            command: true,
            env_to_entrypoint: false,
            private_registry: true,
            suspend: true,
            max_cpus: None,
            max_memory_mb: None,
            credential_env: E2B_ENV,
            // E2B sandboxes are CPU-only.
            gpus: vec![],
        }
    }

    fn check_configured(&self) -> Result<()> {
        self.api().map(|_| ())
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        let api = self.api()?;
        let template = self.ensure_template(&api, spec).await?;
        let ttl = spec.ttl.unwrap_or(DEFAULT_TTL).as_secs().max(1);
        let created: SandboxDoc = api
            .json(
                Method::POST,
                "/v2/sandboxes",
                Some(&json!({
                    "templateID": template,
                    "timeout": ttl,
                    "metadata": spec.labels,
                    "envVars": spec.env,
                    "network": {"allowPublicTraffic": true},
                })),
            )
            .await?;
        Ok(self.instance(&created, Some(&spec.name)))
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        let s: SandboxDoc = self
            .api()?
            .json(Method::GET, &format!("/sandboxes/{id}"), None::<&()>)
            .await?;
        Ok(self.instance(&s, None))
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        let all: Vec<SandboxDoc> = self
            .api()?
            .json(
                Method::GET,
                "/v2/sandboxes?metadata=cua.managed%3Dtrue&state=running,paused",
                None::<&()>,
            )
            .await?;
        Ok(all.iter().map(|s| self.instance(s, None)).collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.api()?
            .call(
                Method::DELETE,
                &format!("/sandboxes/{id}"),
                None::<&()>,
                true,
            )
            .await
    }

    fn endpoint(&self, instance: &ProviderInstance, port: u16) -> Result<ServiceEndpoint> {
        let domain = instance
            .details
            .get("domain")
            .cloned()
            .unwrap_or_else(|| self.config.domain.clone());
        let url = match &self.config.port_url {
            Some(t) => t
                .replace("{port}", &port.to_string())
                .replace("{id}", &instance.id)
                .replace("{domain}", &domain),
            None => format!("https://{port}-{}.{domain}", instance.id),
        };
        let headers = instance
            .details
            .get(TRAFFIC_TOKEN_KEY)
            .map(|t| vec![("e2b-traffic-access-token".to_string(), t.clone())])
            .unwrap_or_default();
        Ok(ServiceEndpoint {
            url: url.trim_end_matches('/').to_string(),
            headers,
        })
    }

    async fn suspend(&self, id: &str) -> Result<()> {
        self.api()?
            .call(
                Method::POST,
                &format!("/sandboxes/{id}/pause"),
                None::<&()>,
                false,
            )
            .await
    }

    async fn resume(&self, id: &str) -> Result<ProviderInstance> {
        let s: SandboxDoc = self
            .api()?
            .json(
                Method::POST,
                &format!("/v2/sandboxes/{id}/connect"),
                Some(&json!({"timeout": DEFAULT_TTL.as_secs()})),
            )
            .await?;
        Ok(self.instance(&s, None))
    }

    async fn keep_alive(&self, id: &str, duration: Duration) -> Result<()> {
        self.api()?
            .call(
                Method::POST,
                &format!("/sandboxes/{id}/timeout"),
                Some(&json!({"timeout": duration.as_secs().max(1)})),
                false,
            )
            .await
    }
}
