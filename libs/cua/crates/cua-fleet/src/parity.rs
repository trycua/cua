//! Sandbox parity with the local runtimes: sidecars on every runtime
//! (trycua/cloud#7893) and tenant registry pull Secrets (#7887).
//!
//! The `libs/fleet` mirror predates these fields, so they are written as
//! JSON next to the typed template (`vmTemplate.sidecars`) and through the
//! gateway's Kubernetes API (`cua-registry-*` Secrets), with the exact shapes
//! Fleet admits.
//!
//! Sidecars are addressed by name on every runtime: the sandbox reaches a
//! sidecar at its name (on its declared ports) and a sidecar reaches the
//! sandbox at [`MAIN_CONTAINER_NAME`]. Pod runtimes (gVisor) run them in the
//! sandbox pod (so `localhost` works too); KubeVirt runs them in a companion
//! gVisor pod and names them in the guest's `/etc/hosts` through cloud-init.

use crate::{Error, FleetClient, Result};
use base64::Engine as _;
use cyclops_sdk::{HttpHeader, HttpRequest, SdkError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub use cua_image::RegistryCredentials;

/// Whether Fleet builds `kind: container` image recipes (remote builds of
/// `Image` layers on a registry base). Off until Fleet's builder runs them.
pub const REMOTE_BUILDS_SUPPORTED: bool = cfg!(feature = "fleet-remote-builds");

/// Name prefix of tenant registry pull Secrets (`REGISTRY_SECRET_NAME_PREFIX`
/// in #7887).
pub const REGISTRY_SECRET_PREFIX: &str = "cua-registry-";

/// Most sidecars a template may carry (Fleet's admission limit).
pub const MAX_SIDECARS: usize = 8;

/// The hostname sidecars reach the sandbox at, and a name no sidecar may
/// take.
pub const MAIN_CONTAINER_NAME: &str = "main";

/// Service names a sandbox with sidecars may not use (Fleet's KubeVirt
/// companion names its own Services with them; reserved on every runtime,
/// locally too, so a spec stays portable).
pub const RESERVED_SERVICE_NAMES: [&str; 3] = ["main", "sidecars", "sc"];

/// The error for a remote image build Fleet cannot run yet.
pub fn remote_builds_unsupported(what: &str) -> Error {
    Error::InvalidArgument(format!(
        "{what} on cloud sandboxes needs Fleet remote builds from a registry base, which are \
         not deployed yet; build the image locally, push it to a registry and reference it"
    ))
}

/// Refuses [`RESERVED_SERVICE_NAMES`] in `services` when `sidecars` is
/// non-empty (Fleet's admission rule; a sandbox without sidecars may use
/// them).
pub fn check_reserved_service_names<'a>(
    services: impl IntoIterator<Item = &'a String>,
    has_sidecars: bool,
) -> Result<()> {
    if !has_sidecars {
        return Ok(());
    }
    if let Some(n) = services
        .into_iter()
        .find(|n| RESERVED_SERVICE_NAMES.contains(&n.as_str()))
    {
        return Err(Error::InvalidArgument(format!(
            "service name {n:?} is reserved in a sandbox with sidecars (reserved: {})",
            RESERVED_SERVICE_NAMES.join(", ")
        )));
    }
    Ok(())
}

/// An extra container next to a sandbox. The sandbox reaches it at its
/// `name` as a hostname (on its declared `ports`), and it reaches the
/// sandbox at [`MAIN_CONTAINER_NAME`]. Local containers and gVisor pods
/// share one network namespace, so `localhost` works there too.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Sidecar {
    /// Name, unique within the sandbox (DNS label, not `main`); also its
    /// hostname.
    pub name: String,
    /// Image reference.
    pub image: String,
    /// argv replacing the image's ENTRYPOINT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Environment (plain values, not secrets).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Ports it listens on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    /// Arguments after `command` (or the image's entrypoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// CPU request as a Kubernetes quantity (`500m`, `1`). Cloud only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory request as a Kubernetes quantity (`256Mi`). Cloud only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

impl Sidecar {
    /// A sidecar running `image`, named after the image's repository.
    pub fn new(image: impl Into<String>) -> Self {
        let image = image.into();
        Self {
            name: default_sidecar_name(&image),
            image,
            command: None,
            env: BTreeMap::new(),
            ports: vec![],
            args: None,
            cpu: None,
            memory: None,
        }
    }
}

/// `redis` for `docker.io/library/redis:7-alpine`: the last path segment of
/// the repository, lowercased to a DNS label.
pub fn default_sidecar_name(image: &str) -> String {
    let repo = image.split('@').next().unwrap_or(image);
    let last = repo.rsplit('/').next().unwrap_or(repo);
    let last = last.split(':').next().unwrap_or(last);
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches('-').to_string();
    name.truncate(50);
    let name = name.trim_end_matches('-').to_string();
    if name.is_empty() || name == "main" {
        "sidecar".into()
    } else {
        name
    }
}

/// Checks sidecars the way Fleet's admission does (so a bad spec fails
/// before any pool exists): at most [`MAX_SIDECARS`], unique DNS-label names
/// other than `main`, an image each, valid env names, ports in range and
/// not claimed by two sidecars. `reserved` ports belong to the sandbox
/// itself (its spacesd); services may target sidecar ports.
pub fn validate_sidecars(sidecars: &[Sidecar], reserved: &[u16]) -> Result<()> {
    if sidecars.len() > MAX_SIDECARS {
        return Err(Error::InvalidArgument(format!(
            "at most {MAX_SIDECARS} sidecars (got {})",
            sidecars.len()
        )));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut ports: BTreeMap<u16, String> = BTreeMap::new();
    for s in sidecars {
        if cyclops_sdk::validate_dns_label(&s.name).is_err() || s.name == MAIN_CONTAINER_NAME {
            return Err(Error::InvalidArgument(format!(
                "sidecar name {:?} must be a DNS label other than \"main\"",
                s.name
            )));
        }
        if !names.insert(s.name.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "sidecar name {:?} is used twice",
                s.name
            )));
        }
        if s.image.trim().is_empty() {
            return Err(Error::InvalidArgument(format!(
                "sidecar {:?} needs an image",
                s.name
            )));
        }
        if let Some(k) = s.env.keys().find(|k| !cua_image::spec::is_env_name(k)) {
            return Err(Error::InvalidArgument(format!(
                "sidecar {:?}: bad environment variable name {k:?}",
                s.name
            )));
        }
        for p in &s.ports {
            if *p == 0 {
                return Err(Error::InvalidArgument(format!(
                    "sidecar {:?}: port 0",
                    s.name
                )));
            }
            if reserved.contains(p) {
                return Err(Error::InvalidArgument(format!(
                    "sidecar {:?} port {p} is also a sandbox port (containers share one \
                     network namespace)",
                    s.name
                )));
            }
            if let Some(other) = ports.insert(*p, s.name.clone()) {
                return Err(Error::InvalidArgument(format!(
                    "port {p} is claimed by sidecars {other:?} and {:?}",
                    s.name
                )));
            }
        }
    }
    Ok(())
}

/// `vmTemplate.sidecars[]` entries (`SandboxSidecar`). CPU and memory are
/// left to Fleet's defaults (500m / 512Mi) unless set; local sidecars use
/// the same defaults.
pub fn sidecars_json(sidecars: &[Sidecar]) -> Value {
    Value::Array(
        sidecars
            .iter()
            .map(|s| {
                let mut v = json!({"name": s.name, "image": s.image});
                if let Some(c) = s.command.as_ref().filter(|c| !c.is_empty()) {
                    v["command"] = json!(c);
                }
                if !s.env.is_empty() {
                    v["env"] = json!(s.env);
                }
                if !s.ports.is_empty() {
                    v["ports"] = json!(s.ports);
                }
                if let Some(a) = s.args.as_ref().filter(|a| !a.is_empty()) {
                    v["args"] = json!(a);
                }
                if let Some(c) = s.cpu.as_ref().filter(|c| !c.is_empty()) {
                    v["cpu"] = json!(c);
                }
                if let Some(m) = s.memory.as_ref().filter(|m| !m.is_empty()) {
                    v["memory"] = json!(m);
                }
                v
            })
            .collect(),
    )
}

/// The deterministic Secret name for credentials of `username` on
/// `registry`: `cua-registry-<16 hex>`. The password is not part of it, so
/// rotating a password updates the same Secret (and keeps the pool key).
pub fn registry_secret_name(registry: &str, username: &str) -> String {
    let mut h = Sha256::new();
    h.update(cua_image::normalize_registry(registry).as_bytes());
    h.update([0u8]);
    h.update(username.as_bytes());
    format!(
        "{REGISTRY_SECRET_PREFIX}{}",
        &hex::encode(h.finalize())[..16]
    )
}

/// The Secret body #7887's admission accepts: exactly a
/// `kubernetes.io/dockerconfigjson` Secret holding only `.dockerconfigjson`.
pub fn registry_secret_body(
    namespace: &str,
    name: &str,
    registry: &str,
    creds: &RegistryCredentials,
) -> Result<Value> {
    if !name.starts_with(REGISTRY_SECRET_PREFIX)
        || cyclops_sdk::validate_dns_label(&name[REGISTRY_SECRET_PREFIX.len()..]).is_err()
    {
        return Err(Error::InvalidArgument(format!(
            "registry secret name {name:?} must be {REGISTRY_SECRET_PREFIX}<dns-label>"
        )));
    }
    if registry.is_empty() || registry.contains('/') || registry.contains("://") {
        return Err(Error::InvalidArgument(
            "registry must be a bare host[:port] such as ghcr.io".into(),
        ));
    }
    if creds.username.is_empty() || creds.password.is_empty() {
        return Err(Error::InvalidArgument(
            "registry username and password must not be empty".into(),
        ));
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    // Docker Hub credentials are looked up under the v1 index key.
    let key = if cua_image::normalize_registry(registry) == "docker.io" {
        "https://index.docker.io/v1/".to_string()
    } else {
        registry.to_string()
    };
    let auth = b64.encode(format!("{}:{}", creds.username, creds.password));
    let config = json!({"auths": {key: {
        "username": creds.username, "password": creds.password, "auth": auth}}});
    Ok(json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": "kubernetes.io/dockerconfigjson",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {"cua.ai/registry-secret": "true"},
        },
        "data": {".dockerconfigjson": b64.encode(config.to_string())},
    }))
}

impl FleetClient {
    fn core_url(&self, ns: &str, name: Option<&str>) -> String {
        let base = self.config().base_url.trim_end_matches('/');
        match name {
            Some(n) => format!("{base}/api/k8s/api/v1/namespaces/{ns}/secrets/{n}"),
            None => format!("{base}/api/k8s/api/v1/namespaces/{ns}/secrets"),
        }
    }

    /// One control-plane request whose body may hold a credential: errors
    /// carry the status only, never the request or response body.
    async fn secret_request(&self, method: &str, url: String, body: Option<&Value>) -> Result<u16> {
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
                    value: "application/json".into(),
                },
            ],
            body: body.map(|b| b.to_string().into_bytes()),
            timeout_secs: Some(30),
            max_response_bytes: Some(1 << 20),
        };
        match self.sdk().execute_authenticated(request).await {
            Ok(r) => Ok(r.status),
            Err(SdkError::Status { status, .. }) => Ok(status),
            Err(SdkError::Transport { .. }) => Err(Error::InvalidArgument(format!(
                "registry secret {method}: the Fleet gateway did not answer"
            ))),
            Err(e) => Err(e.into()),
        }
    }

    /// Creates or replaces the `cua-registry-*` pull Secret `name` in
    /// `namespace` holding `creds` for `registry`. Secrets are write-only
    /// through the gateway, so replacing is delete + create. The credential
    /// never appears in an error or log line.
    pub async fn put_registry_secret(
        &self,
        namespace: &str,
        name: &str,
        registry: &str,
        creds: &RegistryCredentials,
    ) -> Result<()> {
        cyclops_sdk::validate_dns_label(namespace)?;
        let body = registry_secret_body(namespace, name, registry, creds)?;
        let status = self
            .secret_request("POST", self.core_url(namespace, None), Some(&body))
            .await?;
        let status = match status {
            200..=202 => return Ok(()),
            409 => {
                let d = self
                    .secret_request("DELETE", self.core_url(namespace, Some(name)), None)
                    .await?;
                if !matches!(d, 200..=204 | 404) {
                    return Err(secret_status("replace", name, d));
                }
                self.secret_request("POST", self.core_url(namespace, None), Some(&body))
                    .await?
            }
            s => s,
        };
        if (200..=202).contains(&status) {
            tracing::debug!(
                namespace,
                secret = name,
                registry,
                "registry pull secret written"
            );
            Ok(())
        } else {
            Err(secret_status("create", name, status))
        }
    }

    /// Deletes a `cua-registry-*` Secret (missing is fine).
    pub async fn delete_registry_secret(&self, namespace: &str, name: &str) -> Result<()> {
        if !name.starts_with(REGISTRY_SECRET_PREFIX) {
            return Err(Error::InvalidArgument(format!(
                "only {REGISTRY_SECRET_PREFIX}* secrets can be deleted"
            )));
        }
        match self
            .secret_request("DELETE", self.core_url(namespace, Some(name)), None)
            .await?
        {
            200..=204 | 404 => Ok(()),
            s => Err(secret_status("delete", name, s)),
        }
    }

    /// Creates a template from JSON (see [`crate::PoolSpec::template_json`]);
    /// an existing one is left as it is.
    pub(crate) async fn create_template_json(&self, template: Value) -> Result<()> {
        let ns = template["metadata"]["namespace"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let url = self.k8s_url(&ns, "osgymsandboxtemplates", None);
        match self.raw("POST", url, Some(template)).await? {
            (200..=202 | 409, _) => Ok(()),
            (s, v) => Err(SdkError::status("create template", s, v.to_string().as_bytes()).into()),
        }
    }

    /// Reconciles a template whose spec carries parity fields: the typed
    /// template (`spec.template_request()`) as JSON plus
    /// `vmTemplate.sidecars`. Create, or merge-patch an existing one with
    /// explicit nulls for what the desired spec omits.
    pub(crate) async fn reconcile_template_json(&self, template: Value) -> Result<()> {
        let ns = template["metadata"]["namespace"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let name = template["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let item = self.k8s_url(&ns, "osgymsandboxtemplates", Some(&name));
        let (status, _) = self.raw("GET", item.clone(), None).await?;
        match status {
            200 => {
                let mut patch = template.clone();
                let vm = &mut patch["spec"]["vmTemplate"];
                // Declarative: what the desired spec omits is removed.
                for k in [
                    "imagePullSecret",
                    "sidecars",
                    "command",
                    "args",
                    "env",
                    "processMode",
                    "claimSecrets",
                    "probes",
                    "services",
                    "cpuCores",
                    "memory",
                    "firmware",
                ] {
                    if vm.get(k).is_none() {
                        vm[k] = Value::Null;
                    }
                }
                match self.raw("PATCH", item, Some(patch)).await? {
                    (200, _) => Ok(()),
                    (s, v) => {
                        Err(SdkError::status("update template", s, v.to_string().as_bytes()).into())
                    }
                }
            }
            403 | 404 => {
                let url = self.k8s_url(&ns, "osgymsandboxtemplates", None);
                match self.raw("POST", url, Some(template)).await? {
                    (200..=202 | 409, _) => Ok(()),
                    (s, v) => {
                        Err(SdkError::status("create template", s, v.to_string().as_bytes()).into())
                    }
                }
            }
            s => Err(SdkError::status("get template", s, b"").into()),
        }
    }
}

fn secret_status(op: &str, name: &str, status: u16) -> Error {
    let hint = if status == 403 {
        " (the Fleet gateway refused it: check the account may write registry secrets in that \
         namespace)"
    } else {
        ""
    };
    Error::InvalidArgument(format!(
        "could not {op} registry secret {name}: HTTP {status}{hint}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_are_dns_labels() {
        assert_eq!(default_sidecar_name("redis:7-alpine"), "redis");
        assert_eq!(
            default_sidecar_name("docker.io/library/postgres@sha256:ab"),
            "postgres"
        );
        assert_eq!(default_sidecar_name("ghcr.io/me/My_Svc:1"), "my-svc");
        assert_eq!(default_sidecar_name("reg:5000/main"), "sidecar");
    }

    #[test]
    fn sidecars_validate_like_admission() {
        let db = Sidecar {
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        };
        validate_sidecars(std::slice::from_ref(&db), &[8000]).unwrap();
        assert!(
            validate_sidecars(&[db.clone(), db.clone()], &[]).is_err(),
            "dup name"
        );
        assert!(
            validate_sidecars(std::slice::from_ref(&db), &[6379]).is_err(),
            "port clash"
        );
        let twin = Sidecar {
            name: "db2".into(),
            ..db.clone()
        };
        assert!(
            validate_sidecars(&[db.clone(), twin], &[]).is_err(),
            "shared port"
        );
        let main = Sidecar {
            name: "main".into(),
            ..db.clone()
        };
        assert!(validate_sidecars(&[main], &[]).is_err());
        let many: Vec<Sidecar> = (0..9)
            .map(|i| Sidecar {
                name: format!("s{i}"),
                ports: vec![],
                ..db.clone()
            })
            .collect();
        assert!(validate_sidecars(&many, &[]).is_err());
        let bad_env = Sidecar {
            env: [("1X".to_string(), "v".to_string())].into(),
            ..db
        };
        assert!(validate_sidecars(&[bad_env], &[]).is_err());
    }

    #[test]
    fn sidecar_json_matches_the_7887_shape() {
        let s = Sidecar {
            command: Some(vec!["redis-server".into()]),
            env: [("A".to_string(), "1".to_string())].into(),
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        };
        assert_eq!(
            sidecars_json(&[s, Sidecar::new("busybox")]),
            json!([
                {"name": "redis", "image": "redis:7-alpine", "command": ["redis-server"],
                 "env": {"A": "1"}, "ports": [6379]},
                {"name": "busybox", "image": "busybox"}
            ])
        );
        let sized = Sidecar {
            args: Some(vec!["--appendonly".into(), "yes".into()]),
            cpu: Some("500m".into()),
            memory: Some("256Mi".into()),
            ..Sidecar::new("redis:7")
        };
        assert_eq!(
            sidecars_json(std::slice::from_ref(&sized)),
            json!([{"name": "redis", "image": "redis:7", "args": ["--appendonly", "yes"],
                    "cpu": "500m", "memory": "256Mi"}])
        );
        // Templates read back carry them (the Terraform export uses them).
        let spec = crate::SandboxSpec::from_template_json(&json!({"spec": {"vmTemplate": {
            "containerDiskImage": "img",
            "sidecars": sidecars_json(std::slice::from_ref(&sized)),
        }}}));
        assert_eq!(spec.sidecars, vec![sized]);
    }

    #[test]
    fn secret_body_is_exactly_a_dockerconfigjson() {
        let creds = RegistryCredentials::new("me", "hunter2");
        let name = registry_secret_name("ghcr.io", "me");
        assert!(name.starts_with("cua-registry-") && name.len() == 13 + 16);
        assert_eq!(name, registry_secret_name("ghcr.io", "me"), "stable");
        assert_ne!(name, registry_secret_name("ghcr.io", "you"));
        assert_eq!(
            registry_secret_name("docker.io", "me"),
            registry_secret_name("index.docker.io", "me")
        );
        let b = registry_secret_body("ns", &name, "ghcr.io", &creds).unwrap();
        assert_eq!(b["type"], "kubernetes.io/dockerconfigjson");
        let data = b["data"].as_object().unwrap();
        assert_eq!(data.keys().collect::<Vec<_>>(), vec![".dockerconfigjson"]);
        assert!(b["metadata"].get("annotations").is_none());
        let raw = base64::engine::general_purpose::STANDARD
            .decode(data[".dockerconfigjson"].as_str().unwrap())
            .unwrap();
        let cfg: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(cfg["auths"]["ghcr.io"]["username"], "me");
        let hub = registry_secret_body("ns", &name, "docker.io", &creds).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(hub["data"][".dockerconfigjson"].as_str().unwrap())
            .unwrap();
        assert!(
            String::from_utf8(raw)
                .unwrap()
                .contains("https://index.docker.io/v1/")
        );
        assert!(registry_secret_body("ns", "ecr-credentials", "ghcr.io", &creds).is_err());
        assert!(registry_secret_body("ns", &name, "https://ghcr.io", &creds).is_err());
    }
}
