// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Modal Sandboxes as your cloud (`--on modal`): one Modal sandbox per
//! sandbox, in the environment `cua cloud connect modal` names.
//!
//! - **API**: Modal documents no HTTP API, so every call goes through Modal's
//!   official Go SDK in `cua-modal-helper` (the contrib provider's helper,
//!   `libs/cua/crates/cua-contrib/modal-helper`): one JSON request and
//!   answer per call. `CUA_MODAL_HELPER`, else next to the running
//!   executable, else on `PATH`.
//! - **Credentials**: the Modal CLI's own sign-in: the profile in
//!   `~/.modal.toml` (the SDK reads it), or `MODAL_TOKEN_ID` +
//!   `MODAL_TOKEN_SECRET`. Cua stores only the profile and environment
//!   names.
//! - **Runtimes**: gVisor (the default) or Modal's VM runtime
//!   (`--runtime microvm`; nested virtualization there is Team and
//!   Enterprise only, so VM images still do not run).
//! - **Relay join**: images whose driver predates the join variables
//!   (`CUA_ENV_MACHINE_ID`, ...) read the join from files, so the sandbox
//!   command is a small `sh` wrapper that writes them from the environment
//!   (0600, owned by the image's `cua` user), drops the variables and
//!   `exec`s the image's own entrypoint.
//! - **Lifetime**: Modal's sandbox `timeout` is the connection's ttl (at
//!   most 24 hours). A Modal sandbox cannot stop and start again; delete it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};
use cua_sandbox_core::placement::Runtime;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::api::{CloudApi, Result, Target, Tested, cloud_err};
use crate::model::{self, Connection, ProvisionSpec, Resource, Tier};

/// The helper's file name.
pub const HELPER: &str = "cua-modal-helper";
/// The Modal App Cua's sandboxes belong to in the environment.
pub const APP: &str = "cua-sandboxes";
/// Modal's longest sandbox lifetime.
pub const MAX_TTL: Duration = Duration::from_secs(24 * 3600);
/// The runtime word of Modal's VM runtime.
pub const MICROVM: &str = "microvm";
/// Tag with the unix seconds of the create (Modal reports no creation time).
const TAG_CREATED: &str = "cua-created";

/// Sandbox pricing (Modal, 2026): a physical core (two vCPUs) per second
/// and a GiB of memory per second.
const USD_PER_CORE_SEC: f64 = 0.000_039_42;
const USD_PER_GIB_SEC: f64 = 0.000_006_67;

/// The estimated hourly cost of `vcpus` and `memory_mb`.
pub fn usd_per_hour(vcpus: u32, memory_mb: u64) -> f64 {
    let cores = (f64::from(vcpus) / 2.0).max(0.125);
    let gib = memory_mb as f64 / 1024.0;
    (cores * USD_PER_CORE_SEC + gib * USD_PER_GIB_SEC) * 3600.0
}

/// The sandbox command: writes the relay join files from the environment
/// (as `cua-spacesd join` of every image version reads them), then runs
/// the image's entrypoint (`"$@"`).
pub const JOIN_WRAPPER: &str = r#"set -eu
# Modal's VM runtime mounts an empty /run: restore what the image's
# desktop expects there (images from before their entrypoint does it).
mkdir -p /run/systemd/seats /run/systemd/sessions /run/systemd/users 2>/dev/null || true
d=/run/cua-relay
umask 077
mkdir -p "$d"
printf '%s' "${CUA_RELAY_TOKEN:?}" > "$d/machine-token"
printf '%s\n' "${CUA_ENV_MACHINE_ID:?}" > "$d/machine-id"
printf '%s' "${CUA_RELAY_JWKS_JSON:-}" > "$d/jwks.json"
printf '%s' "${CUA_HOST_POLICY_JSON:?}" > "$d/policy.json"
chown -R 1000:1000 "$d" 2>/dev/null || true
export CUA_RELAY_TOKEN_FILE="$d/machine-token" CUA_ENV_MACHINE_ID_FILE="$d/machine-id" CUA_RELAY_JWKS="$d/jwks.json" CUA_HOST_POLICY="$d/policy.json"
[ -s "$d/jwks.json" ] || unset CUA_RELAY_JWKS
unset CUA_RELAY_TOKEN CUA_ENV_MACHINE_ID CUA_RELAY_JWKS_JSON CUA_HOST_POLICY_JSON
exec "$@"
"#;

/// Carries one helper request and its answer (JSON). The process one runs
/// `cua-modal-helper`; tests answer in process with the same protocol.
#[async_trait]
pub trait HelperTransport: Send + Sync {
    /// Sends `request`; `profile_only` keeps a `MODAL_TOKEN_*` of this
    /// process from overriding the connection's profile.
    async fn call(
        &self,
        request: serde_json::Value,
        profile_only: bool,
        deadline: Duration,
    ) -> Result<serde_json::Value>;
}

/// The `cua-modal-helper` process.
pub struct ProcessHelper {
    helper: Option<PathBuf>,
}

/// Modal as a [`CloudApi`].
pub struct ModalApi {
    transport: Arc<dyn HelperTransport>,
    app: String,
    images: Arc<dyn cua_contrib::image_config::ImageConfigSource>,
    /// `~/.modal.toml` (tests point it elsewhere).
    config_file: Option<PathBuf>,
}

impl Default for ModalApi {
    fn default() -> Self {
        Self::from_env()
    }
}

impl ModalApi {
    /// `CUA_MODAL_HELPER`, `CUA_MODAL_APP` (default `cua-sandboxes`), image
    /// configs from the registry.
    pub fn from_env() -> Self {
        ModalApi {
            transport: Arc::new(ProcessHelper {
                helper: std::env::var_os("CUA_MODAL_HELPER")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from),
            }),
            app: std::env::var("CUA_MODAL_APP")
                .ok()
                .filter(|a| !a.trim().is_empty())
                .unwrap_or_else(|| APP.into()),
            images: cua_contrib::image_config::registry(),
            config_file: std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".modal.toml")),
        }
    }

    /// With this transport, image configs and `~/.modal.toml` (tests).
    pub fn with(
        transport: Arc<dyn HelperTransport>,
        images: Arc<dyn cua_contrib::image_config::ImageConfigSource>,
        config_file: Option<PathBuf>,
    ) -> Self {
        ModalApi {
            transport,
            app: APP.into(),
            images,
            config_file,
        }
    }

    /// The profiles in `~/.modal.toml`: (name, active, environment). Only
    /// names are read; tokens are left to the SDK.
    fn profiles(&self) -> Vec<(String, bool, String)> {
        let Some(text) = self
            .config_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return vec![];
        };
        parse_profiles(&text)
    }

    async fn call(&self, conn: &Connection, req: Request<'_>) -> Result<Reply> {
        let op = req.op;
        let deadline = Duration::from_secs(req.deadline_secs + 30);
        let req = Request {
            profile: &conn.profile,
            environment: &conn.environment,
            app: &self.app,
            ..req
        };
        let answer = self
            .transport
            .call(
                serde_json::to_value(&req)?,
                !conn.profile.is_empty(),
                deadline,
            )
            .await?;
        let reply: Reply = serde_json::from_value(answer).map_err(|e| {
            cloud_err(
                "modal",
                op,
                format!("an answer the helper should not give: {e}"),
            )
        })?;
        if let Some(e) = reply.error {
            return Err(match e.kind.as_str() {
                "not_found" => Error::NotFound(format!("modal: {}", e.message)),
                "invalid" => Error::InvalidArgument(format!("modal {op}: {}", e.message)),
                "timeout" => Error::Timeout(format!("modal {op}: {}", e.message)),
                "auth" => cloud_err(
                    "modal",
                    "sign in",
                    format!(
                        "{} (sign in with `modal token new`, or pick a profile with \
                         `cua cloud connect modal --profile <name>`)",
                        e.message
                    ),
                ),
                _ => cloud_err("modal", op, e.message),
            });
        }
        Ok(reply)
    }
}

impl ProcessHelper {
    fn helper(&self) -> Result<PathBuf> {
        if let Some(h) = &self.helper {
            return if h.is_file() {
                Ok(h.clone())
            } else {
                Err(helper_missing(Some(h)))
            };
        }
        let exe = if cfg!(windows) {
            format!("{HELPER}.exe")
        } else {
            HELPER.to_string()
        };
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            && dir.join(&exe).is_file()
        {
            return Ok(dir.join(&exe));
        }
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
            .ok_or_else(|| helper_missing(None))
    }
}

#[async_trait]
impl HelperTransport for ProcessHelper {
    async fn call(
        &self,
        request: serde_json::Value,
        profile_only: bool,
        budget: Duration,
    ) -> Result<serde_json::Value> {
        let op = request["op"].as_str().unwrap_or_default().to_string();
        let helper = self.helper()?;
        let body = serde_json::to_vec(&request)?;
        let mut cmd = tokio::process::Command::new(&helper);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if profile_only {
            cmd.env_remove("MODAL_TOKEN_ID")
                .env_remove("MODAL_TOKEN_SECRET")
                .env_remove("MODAL_PROFILE");
        }
        let mut child = cmd.spawn().map_err(|e| {
            cloud_err(
                "modal",
                "run the helper",
                format!("{}: {e}", helper.display()),
            )
        })?;
        let mut stdin = child.stdin.take().expect("piped");
        stdin.write_all(&body).await?;
        drop(stdin);
        let mut stdout = child.stdout.take().expect("piped").take(4 << 20);
        let mut stderr = child.stderr.take().expect("piped").take(64 << 10);
        let run = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
            a?;
            b?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, out, err))
        };
        let (status, out, err) = tokio::time::timeout(budget, run)
            .await
            .map_err(|_| Error::Timeout(format!("modal {op} within {budget:?}")))??;
        serde_json::from_slice(&out).map_err(|e| {
            cloud_err(
                "modal",
                &op,
                format!(
                    "the helper exited with {status} and no answer ({e}): {}",
                    String::from_utf8_lossy(&err)
                        .chars()
                        .take(300)
                        .collect::<String>()
                ),
            )
        })
    }
}

impl ModalApi {
    fn resource(&self, conn: &Connection, s: &SandboxDoc) -> Resource {
        let created = s
            .tags
            .get(TAG_CREATED)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Resource {
            provider: "modal".into(),
            id: s.id.clone(),
            resource_type: "sandbox".into(),
            name: s.name.clone(),
            project: conn.environment.clone(),
            created,
            expires: model::tag_expires(&s.tags),
            tags: s.tags.clone(),
            state: state_of(&s.status).into(),
            ..Default::default()
        }
    }
}

/// A finished Modal sandbox never runs again: it is terminated.
fn state_of(status: &str) -> &'static str {
    match status {
        "running" => "running",
        _ => "terminated",
    }
}

fn helper_missing(at: Option<&Path>) -> Error {
    cloud_err(
        "modal",
        "find the helper",
        format!(
            "Modal runs through its official Go SDK in {HELPER}{}; build it with `go build` in \
             libs/cua/crates/cua-contrib/modal-helper and put it next to cua, on PATH, or in \
             CUA_MODAL_HELPER",
            at.map(|p| format!(" (not at {})", p.display()))
                .unwrap_or_default()
        ),
    )
}

/// `(name, active, environment)` of each `[profile]` in a `.modal.toml`.
pub fn parse_profiles(text: &str) -> Vec<(String, bool, String)> {
    let mut out: Vec<(String, bool, String)> = vec![];
    for line in text.lines() {
        let l = line.trim();
        if let Some(name) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            out.push((
                name.trim().trim_matches('"').to_string(),
                false,
                String::new(),
            ));
            continue;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let Some(cur) = out.last_mut() else { continue };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "active" => cur.1 = v == "true",
            "environment" => cur.2 = v.to_string(),
            _ => {}
        }
    }
    out
}

#[derive(Serialize, Default)]
struct Request<'a> {
    op: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    app: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    profile: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    environment: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    image: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    argv: Vec<String>,
    #[serde(skip_serializing_if = "is_zero_f")]
    cpus: f64,
    #[serde(skip_serializing_if = "is_zero_u")]
    memory_mib: u64,
    #[serde(skip_serializing_if = "is_zero_u")]
    timeout_secs: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tags: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "str::is_empty")]
    runtime: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    id: &'a str,
    deadline_secs: u64,
}

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero_u(v: &u64) -> bool {
    *v == 0
}

#[derive(Deserialize, Default)]
struct Reply {
    #[serde(default)]
    account: Option<AccountDoc>,
    #[serde(default)]
    sandbox: Option<SandboxDoc>,
    #[serde(default)]
    sandboxes: Vec<SandboxDoc>,
    #[serde(default)]
    error: Option<HelperError>,
}

#[derive(Deserialize)]
struct AccountDoc {
    #[serde(default)]
    sandboxes: usize,
}

#[derive(Deserialize)]
struct SandboxDoc {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct HelperError {
    kind: String,
    message: String,
}

fn ours(tags: &BTreeMap<String, String>, owner: &str, what: &str, id: &str) -> Result<()> {
    if model::is_ours(tags, owner) {
        Ok(())
    } else {
        Err(cloud_err(
            "modal",
            what,
            format!("refusing: sandbox {id} is not tagged as this Cua home's"),
        ))
    }
}

#[async_trait]
impl CloudApi for ModalApi {
    fn name(&self) -> &'static str {
        "modal"
    }

    fn title(&self) -> &'static str {
        "Modal"
    }

    fn tier(&self) -> Tier {
        Tier::Sandbox
    }

    fn arches(&self) -> Vec<&'static str> {
        vec!["amd64"]
    }

    fn runtimes(&self) -> Vec<Runtime> {
        vec![Runtime::Gvisor, Runtime::Other(MICROVM.into())]
    }

    fn detect(&self) -> CloudCredentials {
        if std::env::var_os("MODAL_TOKEN_ID").is_some() {
            return CloudCredentials {
                found: true,
                source: "MODAL_TOKEN_ID".into(),
            };
        }
        let profiles = self.profiles();
        match profiles.iter().find(|p| p.1).or(profiles.first()) {
            Some((name, ..)) => CloudCredentials {
                found: true,
                source: format!("~/.modal.toml profile {name}"),
            },
            None => CloudCredentials::default(),
        }
    }

    fn resolve(&self, t: &Target) -> Result<Connection> {
        let profiles = self.profiles();
        let profile = t
            .profile
            .clone()
            .or_else(|| profiles.iter().find(|p| p.1).map(|p| p.0.clone()))
            .unwrap_or_default();
        if !profile.is_empty() && !profiles.is_empty() && !profiles.iter().any(|p| p.0 == profile) {
            return Err(Error::InvalidArgument(format!(
                "modal: no profile {profile:?} in ~/.modal.toml (profiles: {})",
                profiles
                    .iter()
                    .map(|p| p.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let environment = t
            .environment
            .clone()
            .or_else(|| {
                profiles
                    .iter()
                    .find(|p| p.0 == profile)
                    .map(|p| p.2.clone())
                    .filter(|e| !e.is_empty())
            })
            .unwrap_or_else(|| "main".into());
        Ok(Connection {
            provider: "modal".into(),
            profile,
            environment,
            ..Default::default()
        })
    }

    fn kinds(&self, _: &Connection) -> Vec<CloudKind> {
        let sandbox = |image: &str, mb: u64| CloudKind {
            image: image.into(),
            kind: "container".into(),
            supported: true,
            reason: "one Modal sandbox (gVisor; --runtime microvm for Modal's VM runtime)".into(),
            machine_type: format!("1 core (2 vCPU) / {} GiB", mb / 1024),
            usd_per_hour: (usd_per_hour(2, mb) * 1000.0).round() / 1000.0,
        };
        let no = |image: &str, reason: &str| CloudKind {
            image: image.into(),
            kind: "vm".into(),
            supported: false,
            reason: reason.into(),
            ..Default::default()
        };
        vec![
            sandbox("linux", 4096),
            sandbox("linux-slim", 4096),
            no(
                "windows",
                "Windows is a VM image; Modal sandboxes run container images",
            ),
            no(
                "omarchy",
                "Omarchy is a VM image; Modal sandboxes run container images",
            ),
            no("macos", "macOS runs on Apple hardware only"),
        ]
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = Tested::default();
        let who = if conn.profile.is_empty() {
            "MODAL_TOKEN_ID".to_string()
        } else {
            format!("profile {}", conn.profile)
        };
        match self
            .call(
                conn,
                Request {
                    op: "check",
                    tags: [(model::tag::MANAGED.to_string(), "true".to_string())].into(),
                    deadline_secs: 60,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(r) => {
                t.account = who.clone();
                t.check("credentials", true, who.clone());
                t.check(
                    "environment",
                    true,
                    format!(
                        "{} ({} Cua sandboxes running)",
                        conn.environment,
                        r.account.map(|a| a.sandboxes).unwrap_or(0)
                    ),
                );
            }
            Err(Error::NotFound(m)) => {
                t.check("credentials", true, who);
                t.check(
                    "environment",
                    false,
                    format!("{m} (create it with `modal environment create`)"),
                );
            }
            Err(e) => t.check("credentials", false, e.to_string()),
        }
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        let image = cua_sandbox_core::ProviderImage {
            reference: spec.image.clone(),
            pinned_ref: spec.image.clone(),
            digest: String::new(),
            kind: cua_sandbox_core::provider::RunKind::Container,
            arch: "amd64".into(),
            spacesd: None,
        };
        let cfg = self.images.config(&image, None).await?;
        let inner = cfg.argv(spec.command.as_deref());
        if inner.is_empty() {
            return Err(Error::UnsupportedImage(format!(
                "{} has no ENTRYPOINT or CMD: pass a command for Modal",
                spec.image
            )));
        }
        let mut argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            JOIN_WRAPPER.to_string(),
            "cua-join".to_string(),
        ];
        argv.extend(inner);
        let ttl = Duration::from_secs(spec.ttl_secs)
            .min(MAX_TTL)
            .max(Duration::from_secs(600));
        let mut tags = spec.tags.clone();
        tags.insert(TAG_CREATED.into(), model::now().to_string());
        let runtime = match spec.runtime.as_str() {
            "gvisor" => "gvisor",
            MICROVM => "vm",
            "" => "",
            other => {
                return Err(Error::InvalidArgument(format!(
                    "modal: unknown runtime {other:?} (gvisor or {MICROVM})"
                )));
            }
        };
        let mut env = spec.env.clone();
        if runtime == "vm" {
            // Modal's VM runtime boots the rootfs as a microVM's root with
            // its container agent as PID 1: a container, which drivers from
            // before they detect that report as a bare host.
            env.insert("CUA_ENV_RUNTIME".into(), "container".into());
        }
        let r = self
            .call(
                conn,
                Request {
                    op: "create",
                    image: &spec.image,
                    argv,
                    cpus: f64::from(spec.cpus.unwrap_or(2)),
                    memory_mib: spec.memory_mb.unwrap_or(4096),
                    timeout_secs: ttl.as_secs(),
                    env,
                    tags,
                    runtime,
                    name: &spec.name,
                    deadline_secs: 15 * 60,
                    ..Default::default()
                },
            )
            .await?;
        let doc = r
            .sandbox
            .ok_or_else(|| cloud_err("modal", "create", "no sandbox in the answer"))?;
        let mut res = self.resource(conn, &doc);
        res.name = spec.name.clone();
        // The engine in the placement model's words (`gvisor`, `microvm`).
        res.extra.insert(
            "runtime".into(),
            if spec.runtime.is_empty() {
                "gvisor".into()
            } else {
                spec.runtime.clone()
            },
        );
        Ok(vec![res])
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        match self
            .call(
                conn,
                Request {
                    op: "get",
                    id: &r.id,
                    deadline_secs: 60,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(rep) => Ok(rep
                .sandbox
                .map(|s| (state_of(&s.status).to_string(), s.tags))),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn stop(&self, _: &Connection, _: &Resource, _: &str) -> Result<()> {
        Err(Error::InvalidArgument(
            "modal: a Modal sandbox cannot stop and start again (Modal ends a sandbox for good \
             when it stops); delete it, and create a new one from the image"
                .into(),
        ))
    }

    async fn start(&self, c: &Connection, r: &Resource, o: &str) -> Result<()> {
        self.stop(c, r, o).await
    }

    async fn delete(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        match self.describe(conn, r).await? {
            None => return Ok(()),
            Some((state, tags)) => {
                ours(&tags, owner, "delete", &r.id)?;
                if state == "terminated" && tags.is_empty() {
                    return Ok(());
                }
            }
        }
        match self
            .call(
                conn,
                Request {
                    op: "delete",
                    id: &r.id,
                    deadline_secs: 60,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(_) | Err(Error::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        let r = self
            .call(
                conn,
                Request {
                    op: "list",
                    tags: [
                        (model::tag::MANAGED.to_string(), "true".to_string()),
                        (model::tag::OWNER.to_string(), model::label_value(owner)),
                    ]
                    .into(),
                    deadline_secs: 120,
                    ..Default::default()
                },
            )
            .await?;
        Ok(r.sandboxes
            .iter()
            .filter(|s| model::is_ours(&s.tags, owner))
            .map(|s| self.resource(conn, s))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_and_prices() {
        let p = parse_profiles(
            "[cuaai]\ntoken_id = \"ak-x\"\ntoken_secret = \"as-y\"\nactive = true\n\n[other]\nenvironment = \"dev\"\n",
        );
        assert_eq!(
            p,
            vec![
                ("cuaai".to_string(), true, String::new()),
                ("other".to_string(), false, "dev".to_string())
            ]
        );
        // 1 core + 4 GiB at sandbox rates.
        let h = usd_per_hour(2, 4096);
        assert!((0.23..0.25).contains(&h), "{h}");
    }

    #[test]
    fn the_wrapper_writes_the_join_files_and_execs_the_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        let script = JOIN_WRAPPER.replace("/run/cua-relay", &dir.path().display().to_string());
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script, "cua-join", "sh", "-c", "env"])
            .env("CUA_RELAY_TOKEN", "cmt_x")
            .env("CUA_ENV_MACHINE_ID", "cloud-0123456789abcdef")
            .env("CUA_RELAY_JWKS_JSON", "{\"keys\":[]}")
            .env("CUA_HOST_POLICY_JSON", "{\"owner\":\"a\"}")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let env = String::from_utf8(out.stdout).unwrap();
        assert!(env.contains("CUA_RELAY_TOKEN_FILE="), "{env}");
        assert!(
            !env.contains("CUA_RELAY_TOKEN=cmt_x"),
            "the token is not left in the env"
        );
        assert!(!env.contains("CUA_RELAY_JWKS_JSON="));
        let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap();
        assert_eq!(read("machine-token"), "cmt_x");
        assert_eq!(read("machine-id").trim(), "cloud-0123456789abcdef");
        assert_eq!(read("policy.json"), "{\"owner\":\"a\"}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join("machine-token"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
    }
}
