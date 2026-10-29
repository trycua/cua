//! `AgentSetup` (`Cua.agent_setup()`): detect AI coding agents on this
//! machine, install the bundled cua skills and configure the cua MCP server
//! in their configs (cua-agent-setup). Host-local in both topologies: it
//! edits the calling user's own agent configs, never the daemon's.

use super::{Cua, run};
use crate::{CuaError, Result};
use std::{collections::HashMap, sync::Arc};

impl From<cua_agent_setup::Error> for CuaError {
    fn from(e: cua_agent_setup::Error) -> Self {
        use cua_agent_setup::Error as E;
        let m = e.to_string();
        match e {
            E::InvalidArgument(_) | E::Malformed { .. } => CuaError::InvalidArgument(m),
            E::Io { .. } | E::Internal(_) => CuaError::Internal(m),
        }
    }
}

/// One AI coding agent and what cua configured for it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentInfo {
    /// Registry id (`claude-code`, `codex`, `cursor`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether it looks installed.
    pub installed: bool,
    /// What gave it away (`bin:<path>`, `path:<dir>`, `app:<bundle>`).
    pub evidence: Vec<String>,
    /// User skills directory, when the agent supports Agent Skills.
    pub skills_dir: Option<String>,
    /// User MCP config file, when the agent supports MCP.
    pub mcp_config: Option<String>,
    /// `json`, `toml` or `yaml`.
    pub mcp_format: Option<String>,
    /// A `cua` MCP server launching a cua binary is configured.
    pub cua_configured: bool,
    /// That entry is the one cua wrote.
    pub cua_managed: bool,
    /// A `cua-driver` MCP server (background computer-use) is configured.
    pub cua_driver_configured: bool,
    /// Bundled skills present in the skills directory.
    pub skills_installed: Vec<String>,
    /// Installed bundled skills that differ from this SDK's copy.
    pub skills_outdated: Vec<String>,
    /// Problem reading the MCP config (for example a malformed file).
    pub error: Option<String>,
    /// What could not be verified from the agent's official docs.
    pub unverified: Option<String>,
    /// The account the agent signs in with on its own (`ChatGPT` for
    /// Codex), when cua can see that sign-in.
    #[uniffi(default = None)]
    pub sign_in: Option<String>,
    /// That sign-in is on this computer.
    #[uniffi(default = false)]
    pub signed_in: bool,
}

/// A skill bundled with the SDK.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BundledSkill {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Version.
    pub version: String,
    /// Number of files.
    pub files: u32,
}

/// The MCP server to register.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentMcpServer {
    /// Server name (`cua`).
    #[uniffi(default = "cua")]
    pub name: String,
    /// Launch command (an absolute `cua` path is best: GUI agents often
    /// lack the shell PATH).
    pub command: String,
    /// Arguments (`["mcp"]`).
    pub args: Vec<String>,
    /// Extra environment.
    pub env: HashMap<String, String>,
}

/// What an operation touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentSetupTarget {
    /// A skill folder.
    Skill,
    /// An MCP config entry.
    Mcp,
}

/// What happened to one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentSetupChange {
    /// Written where nothing was.
    Created,
    /// Replaced or edited.
    Updated,
    /// Already as wanted.
    Unchanged,
    /// Removed (or the previous entry restored).
    Removed,
    /// Deliberately left alone (see detail).
    Skipped,
    /// Failed (see detail); the target was left untouched.
    Failed,
}

/// One target's outcome.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentSetupOutcome {
    /// Agents served by this target.
    pub agents: Vec<String>,
    /// Kind.
    pub target: AgentSetupTarget,
    /// Skill or server name.
    pub item: String,
    /// Skill folder or config file.
    pub path: String,
    /// Result.
    pub change: AgentSetupChange,
    /// Explanation.
    pub detail: String,
    /// Backup taken before this write.
    pub backup: Option<String>,
}

fn path_s(p: &std::path::Path) -> String {
    p.display().to_string()
}

impl From<cua_agent_setup::AgentStatus> for AgentInfo {
    fn from(a: cua_agent_setup::AgentStatus) -> Self {
        AgentInfo {
            id: a.id,
            name: a.name,
            installed: a.installed,
            evidence: a.evidence,
            skills_dir: a.skills_dir.as_deref().map(path_s),
            mcp_config: a.mcp_config.as_deref().map(path_s),
            mcp_format: a.mcp_format.map(|f| f.name().to_ascii_lowercase()),
            cua_configured: a.cua_configured,
            cua_managed: a.cua_managed,
            cua_driver_configured: a.cua_driver_configured,
            skills_installed: a.skills_installed,
            skills_outdated: a.skills_outdated,
            error: a.error,
            unverified: a.unverified,
            sign_in: a.sign_in,
            signed_in: a.signed_in,
        }
    }
}

impl From<cua_agent_setup::Outcome> for AgentSetupOutcome {
    fn from(o: cua_agent_setup::Outcome) -> Self {
        use cua_agent_setup::{Change as C, Target as T};
        AgentSetupOutcome {
            agents: o.agents,
            target: match o.target {
                T::Skill => AgentSetupTarget::Skill,
                T::Mcp => AgentSetupTarget::Mcp,
            },
            item: o.item,
            path: path_s(&o.path),
            change: match o.change {
                C::Created => AgentSetupChange::Created,
                C::Updated => AgentSetupChange::Updated,
                C::Unchanged => AgentSetupChange::Unchanged,
                C::Removed => AgentSetupChange::Removed,
                C::Skipped => AgentSetupChange::Skipped,
                C::Failed => AgentSetupChange::Failed,
            },
            detail: o.detail,
            backup: o.backup.as_deref().map(path_s),
        }
    }
}

fn outcomes(v: Vec<cua_agent_setup::Outcome>) -> Vec<AgentSetupOutcome> {
    v.into_iter().map(Into::into).collect()
}

/// Agent onboarding for this machine's user.
#[derive(uniffi::Object)]
pub struct AgentSetup {
    inner: cua_agent_setup::AgentSetup,
}

impl AgentSetup {
    /// On an explicit environment (Rust hosts and tests: a temporary home).
    pub fn with_env(env: cua_agent_setup::HostEnv) -> Arc<Self> {
        Arc::new(AgentSetup {
            inner: cua_agent_setup::AgentSetup::new(env),
        })
    }

    fn server(&self, s: Option<AgentMcpServer>) -> cua_agent_setup::McpServer {
        match s {
            Some(s) => cua_agent_setup::McpServer {
                // No name: the channel's (`cua`, or `cua-local` for the
                // Local app), so the Local app never writes Stable's entry.
                name: if s.name.trim().is_empty() {
                    self.inner.env().channel.mcp_server_name().into()
                } else {
                    s.name
                },
                command: s.command,
                args: s.args,
                env: s.env.into_iter().collect(),
            },
            None => cua_agent_setup::McpServer::default_for(self.inner.env()),
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(cua_agent_setup::AgentSetup) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let inner = self.inner.clone();
        run(async move {
            tokio::task::spawn_blocking(move || f(inner))
                .await
                .map_err(|e| CuaError::Internal(format!("agent setup task failed: {e}")))?
        })
        .await
    }
}

#[uniffi::export]
impl Cua {
    /// Agent onboarding: detect AI coding agents, install the cua skills and
    /// configure the cua MCP server for the current user.
    pub fn agent_setup(&self) -> Arc<AgentSetup> {
        AgentSetup::with_env(cua_agent_setup::HostEnv::current())
    }
}

#[uniffi::export]
impl AgentSetup {
    /// Every supported agent, installed or not (read-only).
    pub fn detect(&self) -> Vec<AgentInfo> {
        self.inner.detect().into_iter().map(Into::into).collect()
    }

    /// Same as `detect`, named for status displays.
    pub fn status(&self) -> Vec<AgentInfo> {
        self.detect()
    }

    /// Starts `agent`'s own sign-in on this computer (`codex login`: the
    /// browser opens). Returns once it is running; `detect` reports
    /// `signed_in` when it is done. cua never reads or copies the sign-in.
    pub fn sign_in(&self, agent: String) -> Result<()> {
        Ok(self.inner.sign_in(&agent)?)
    }

    /// The skills bundled with this SDK.
    pub fn skills(&self) -> Vec<BundledSkill> {
        self.inner
            .bundled_skills()
            .into_iter()
            .map(|s| BundledSkill {
                name: s.name,
                description: s.description,
                version: s.version,
                files: s.files as u32,
            })
            .collect()
    }

    /// Resolves `all` (installed agents), `none` or `claude,codex` to ids.
    pub fn select(&self, selection: String) -> Result<Vec<String>> {
        Ok(self.inner.select(&selection)?)
    }

    /// Installs `skills` (empty: all bundled) for `agents`. Folders cua did
    /// not write are skipped unless `force`.
    pub async fn install_skills(
        &self,
        agents: Vec<String>,
        skills: Vec<String>,
        force: bool,
    ) -> Result<Vec<AgentSetupOutcome>> {
        self.blocking(move |s| Ok(outcomes(s.install_skills(&agents, &skills, force)?)))
            .await
    }

    /// Configures the MCP server (default: `cua mcp`, `cua` resolved on
    /// PATH; `cua-local mcp` named `cua-local` for the Local app) for
    /// `agents`. A server without a name gets the channel's.
    pub async fn configure_mcp(
        &self,
        agents: Vec<String>,
        server: Option<AgentMcpServer>,
    ) -> Result<Vec<AgentSetupOutcome>> {
        let server = self.server(server);
        self.blocking(move |s| Ok(outcomes(s.configure_mcp(&agents, &server)?)))
            .await
    }

    /// Background computer-use for `agents`: the bundled `cua-driver` skill
    /// (`skills`) and the `cua-driver mcp` server (`mcp`), launching
    /// `command` (default: the installed cua-driver). The same step as
    /// `cua agents setup --cua-driver` and the installers' `cua-driver` item.
    pub async fn setup_cua_driver(
        &self,
        agents: Vec<String>,
        command: Option<String>,
        skills: bool,
        mcp: bool,
    ) -> Result<Vec<AgentSetupOutcome>> {
        let server = match command {
            Some(c) if !c.trim().is_empty() => cua_agent_setup::McpServer::driver(c),
            _ => cua_agent_setup::McpServer::driver_for(self.inner.env()),
        };
        self.blocking(move |s| {
            Ok(outcomes(s.setup_cua_driver(
                &agents,
                &server,
                cua_agent_setup::Parts { skills, mcp },
                false,
            )?))
        })
        .await
    }

    /// Removes what cua added for `agents` (skills and/or MCP).
    pub async fn remove(
        &self,
        agents: Vec<String>,
        skills: bool,
        mcp: bool,
    ) -> Result<Vec<AgentSetupOutcome>> {
        self.blocking(move |s| {
            Ok(outcomes(
                s.remove(&agents, cua_agent_setup::Parts { skills, mcp })?,
            ))
        })
        .await
    }

    /// Refreshes installed cua skills and re-points managed MCP entries.
    pub async fn update(&self, server: Option<AgentMcpServer>) -> Result<Vec<AgentSetupOutcome>> {
        let server = self.server(server);
        self.blocking(move |s| Ok(outcomes(s.update(Some(&server))?)))
            .await
    }

    /// Whether the user said "never" to agent onboarding.
    pub fn onboarding_declined(&self) -> bool {
        cua_agent_setup::config::onboarding_declined(self.inner.env())
    }

    /// Saves or clears the "never" answer (`~/.cua/config`).
    pub fn set_onboarding_declined(&self, declined: bool) -> Result<()> {
        Ok(cua_agent_setup::config::set_onboarding_declined(
            self.inner.env(),
            declined,
        )?)
    }
}

/// Keyvault on This Mac as the connection status and Connect's sign-in
/// copy see it: the Keychain-backed agent keys (`cua_spaces::agents::keys`),
/// which runs in your Spaces read at run time. Values never leave this
/// process except into a run's env.
#[cfg(feature = "spaces")]
pub struct KeyvaultSignIns(&'static cua_spaces::agents::keys::AgentKeys);

#[cfg(feature = "spaces")]
impl KeyvaultSignIns {
    /// This process's store.
    pub fn global() -> Self {
        KeyvaultSignIns(cua_spaces::agents::keys::global())
    }
}

#[cfg(feature = "spaces")]
impl cua_agent_setup::SignInVault for KeyvaultSignIns {
    fn imported(&self) -> std::result::Result<Vec<cua_agent_setup::ImportedSignIn>, String> {
        Ok(self
            .0
            .imported()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|k| {
                Some(cua_agent_setup::ImportedSignIn {
                    source: k.source?,
                    label: k.label.unwrap_or_default(),
                    env: k.env,
                    expires_ms: k.expires_ms,
                    added_ms: k.added_ms,
                })
            })
            .collect())
    }

    fn save(
        &self,
        c: &cua_agent_setup::SignInCopy,
    ) -> std::result::Result<cua_agent_setup::ImportedSignIn, String> {
        let k = self
            .0
            .import(
                &c.source,
                &c.env,
                &c.label,
                &c.value,
                c.expires_ms,
                c.origin.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        cua_telemetry::global().capture(cua_telemetry::events::agent_signin_imported(
            &c.source,
            c.origin.as_deref(),
        ));
        Ok(cua_agent_setup::ImportedSignIn {
            source: c.source.clone(),
            env: k.env,
            label: c.label.clone(),
            expires_ms: c.expires_ms,
            added_ms: k.added_ms,
        })
    }

    fn remove(&self, source: &str) -> std::result::Result<Vec<String>, String> {
        self.0.remove_imported(source).map_err(|e| e.to_string())
    }
}

/// Counts the sign-ins Connect could not copy (fixed words only: the agent and
/// the kind, never the line or the account).
fn note_refusals(results: &[cua_agent_setup::connection::ConnectResult]) {
    #[cfg(feature = "spaces")]
    for r in results {
        if let Some(kind) = r.sign_in.as_ref().and_then(|s| s.refused.as_deref()) {
            cua_telemetry::global()
                .capture(cua_telemetry::events::agent_signin_refused(&r.agent, kind));
        }
    }
    #[cfg(not(feature = "spaces"))]
    let _ = results;
}

/// The vault this build uses, when it has one.
pub fn sign_in_vault() -> Option<Box<dyn cua_agent_setup::SignInVault>> {
    #[cfg(feature = "spaces")]
    {
        Some(Box::new(KeyvaultSignIns::global()))
    }
    #[cfg(not(feature = "spaces"))]
    {
        None
    }
}

/// Device sign-ins waiting to be approved, by id (started and waited in two calls).
fn device_logins() -> &'static std::sync::Mutex<
    std::collections::HashMap<String, cua_agent_setup::signin::DeviceSignIn>,
> {
    static L: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, cua_agent_setup::signin::DeviceSignIn>>,
    > = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| CuaError::Internal(e.to_string()))
}

#[uniffi::export]
impl AgentSetup {
    /// Whether each agent is really connected (CONTRACT §4: `{agents,
    /// checked_at_ms}`), as JSON, the same JSON `cua connect status --json`
    /// prints and the `connect_status` MCP tool answers. `expected` is the
    /// MCP server this app writes (an entry launching another cua is
    /// `stale`); `refresh` skips the few-second cache (window focus).
    pub fn connection_status_json(
        &self,
        expected: Option<AgentMcpServer>,
        refresh: bool,
    ) -> String {
        let opts = cua_agent_setup::StatusOptions {
            expected: expected.map(|e| self.server(Some(e))),
            refresh,
            ..Default::default()
        };
        let vault = sign_in_vault();
        let r = self.inner.connection_status(&opts, vault.as_deref());
        serde_json::to_string(&r).unwrap_or_default()
    }

    /// Connect (and Reconnect) for `agents`: Cua's skills and MCP entry
    /// (`server`, default this channel's cua), and with `sign_in` the
    /// agent's sign-in copied into Keyvault on This Mac (Claude Code's is the
    /// sign-in it already has, read from where Claude Code keeps it). JSON:
    /// one result per agent.
    pub async fn connect_json(
        &self,
        agents: Vec<String>,
        server: Option<AgentMcpServer>,
        skills: bool,
        mcp: bool,
        sign_in: bool,
    ) -> Result<String> {
        let server = self.server(server);
        self.blocking(move |s| {
            let vault = sign_in_vault();
            let r = s.connect(
                &agents,
                &server,
                cua_agent_setup::connection::ConnectParts {
                    skills,
                    mcp,
                    sign_in,
                },
                vault.as_deref(),
                cua_agent_setup::signin::CLAUDE_IMPORT_WAIT,
            )?;
            note_refusals(&r);
            to_json(&r)
        })
        .await
    }

    /// Disconnect of some parts only (an app's page's switches): Cua's skills
    /// and/or its MCP entry, and with `sign_in` the copy in Keyvault. JSON
    /// `{outcomes, removed_sign_ins}`.
    pub async fn disconnect_parts_json(
        &self,
        agents: Vec<String>,
        skills: bool,
        mcp: bool,
        sign_in: bool,
    ) -> Result<String> {
        self.blocking(move |s| {
            let vault = sign_in_vault();
            let (outcomes, removed) = s.disconnect(
                &agents,
                cua_agent_setup::Parts { skills, mcp },
                cua_agent_setup::RemoveOptions::default(),
                sign_in,
                vault.as_deref(),
            )?;
            to_json(&serde_json::json!({"outcomes": outcomes, "removed_sign_ins": removed}))
        })
        .await
    }

    /// Starts `agent`'s device-code sign-in made for Cua (Codex: `codex login
    /// --device-auth` in a folder Cua owns). JSON `{id, url, code}`: show the
    /// link and the code, then wait with [`Self::device_sign_in_wait_json`].
    pub async fn device_sign_in_start_json(&self, agent: String) -> Result<String> {
        self.blocking(move |s| {
            let login = s
                .start_device_sign_in(&agent)
                .map_err(|r| CuaError::Internal(r.line().to_string()))?;
            let id = format!(
                "{agent}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );
            let out = serde_json::json!({"id": id, "url": login.url, "code": login.code});
            device_logins()
                .lock()
                .map_err(|e| CuaError::Internal(e.to_string()))?
                .insert(id, login);
            to_json(&out)
        })
        .await
    }

    /// Waits for a device sign-in to be approved and saves it into Keyvault on
    /// This Mac. JSON: `{imported, refused, line}` (as Connect's `sign_in`).
    pub async fn device_sign_in_wait_json(&self, id: String) -> Result<String> {
        self.blocking(move |s| {
            let login = device_logins()
                .lock()
                .map_err(|e| CuaError::Internal(e.to_string()))?
                .remove(&id)
                .ok_or_else(|| {
                    CuaError::InvalidArgument(format!("no sign-in {id:?} is waiting"))
                })?;
            let vault = sign_in_vault();
            let r = match vault.as_deref() {
                None => cua_agent_setup::connection::SignInResult {
                    imported: None,
                    refused: Some("failed".into()),
                    line: Some("Keyvault isn't available here.".into()),
                },
                Some(v) => match s.finish_device_sign_in(
                    login,
                    v,
                    cua_agent_setup::signin::DEVICE_SIGN_IN_WAIT,
                ) {
                    Ok(i) => cua_agent_setup::connection::SignInResult {
                        imported: Some(i),
                        refused: None,
                        line: None,
                    },
                    Err(r) => cua_agent_setup::connection::SignInResult {
                        imported: None,
                        refused: Some(r.kind().into()),
                        line: Some(r.line().into()),
                    },
                },
            };
            to_json(&r)
        })
        .await
    }

    /// Disconnect: what Connect did, undone (the previous config entry back
    /// from Cua's record, the skills, and with `sign_in` the copy in
    /// Keyvault). JSON `{outcomes, removed_sign_ins}`.
    pub async fn disconnect_json(&self, agents: Vec<String>, sign_in: bool) -> Result<String> {
        self.blocking(move |s| {
            let vault = sign_in_vault();
            let (outcomes, removed) = s.disconnect(
                &agents,
                cua_agent_setup::Parts::ALL,
                cua_agent_setup::RemoveOptions::default(),
                sign_in,
                vault.as_deref(),
            )?;
            to_json(&serde_json::json!({"outcomes": outcomes, "removed_sign_ins": removed}))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sdk_agent_setup_round_trip_in_a_temp_home() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        let s = AgentSetup::with_env(cua_agent_setup::HostEnv::isolated(d.path()));
        let codex = s.detect().into_iter().find(|a| a.id == "codex").unwrap();
        assert!(codex.installed);
        assert_eq!(codex.mcp_format.as_deref(), Some("toml"));
        assert_eq!(s.select("all".into()).unwrap(), ["codex"]);
        assert!(s.skills().len() >= 4);

        let out = s
            .install_skills(vec!["codex".into()], vec![], false)
            .await
            .unwrap();
        assert!(out.iter().all(|o| o.change == AgentSetupChange::Created));
        let server = AgentMcpServer {
            name: "cua".into(),
            command: "/opt/cua/bin/cua".into(),
            args: vec!["mcp".into()],
            env: HashMap::new(),
        };
        let out = s
            .configure_mcp(vec!["codex".into()], Some(server.clone()))
            .await
            .unwrap();
        assert_eq!(out[0].change, AgentSetupChange::Created);
        assert_eq!(out[0].target, AgentSetupTarget::Mcp);
        let st = s.status().into_iter().find(|a| a.id == "codex").unwrap();
        assert!(st.cua_configured && st.cua_managed);
        let out = s.update(Some(server)).await.unwrap();
        assert!(out.iter().all(|o| o.change == AgentSetupChange::Unchanged));
        // Cua Spaces' entries go; the cua-driver skill (one of the bundled
        // ones) is Cua Driver's and stays.
        let out = s.remove(vec!["codex".into()], true, true).await.unwrap();
        assert!(
            out.iter()
                .filter(|o| o.item != "cua-driver")
                .all(|o| o.change == AgentSetupChange::Removed),
            "{out:?}"
        );
        assert!(
            out.iter()
                .filter(|o| o.item == "cua-driver")
                .all(|o| o.change == AgentSetupChange::Skipped),
            "{out:?}"
        );
        assert!(
            s.configure_mcp(vec!["nope".into()], None)
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown agent")
        );
        assert!(!s.onboarding_declined());
        s.set_onboarding_declined(true).unwrap();
        assert!(s.onboarding_declined());
    }

    #[tokio::test]
    async fn sdk_local_agent_setup_registers_cua_local_beside_stables_cua() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        let env = cua_agent_setup::HostEnv::isolated(d.path());
        let stable = AgentSetup::with_env(env.clone());
        let local = AgentSetup::with_env(env.with_channel(cua_agent_setup::CuaChannel::Local));
        let server = |command: &str| AgentMcpServer {
            // No name: each channel registers its own.
            name: String::new(),
            command: command.into(),
            args: vec!["mcp".into()],
            env: HashMap::new(),
        };
        let out = stable
            .configure_mcp(
                vec!["codex".into()],
                Some(server("/Applications/Cua Spaces.app/Contents/MacOS/cua")),
            )
            .await
            .unwrap();
        assert_eq!(
            (out[0].item.as_str(), out[0].change),
            ("cua", AgentSetupChange::Created)
        );
        let local_cua = "/Users/a/Applications/Cua Spaces Local.app/Contents/MacOS/cua";
        let out = local
            .configure_mcp(vec!["codex".into()], Some(server(local_cua)))
            .await
            .unwrap();
        assert_eq!(
            (out[0].item.as_str(), out[0].change),
            ("cua-local", AgentSetupChange::Created)
        );
        let toml = std::fs::read_to_string(d.path().join(".codex/config.toml")).unwrap();
        assert!(toml.contains("[mcp_servers.cua]"), "{toml}");
        assert!(toml.contains("[mcp_servers.cua-local]"), "{toml}");
        assert!(toml.contains("/Applications/Cua Spaces.app/Contents/MacOS/cua"));
        assert!(toml.contains(local_cua));
        // Each reads its own entry as its own.
        let st = |s: &Arc<AgentSetup>| s.status().into_iter().find(|a| a.id == "codex").unwrap();
        assert!(st(&stable).cua_configured && st(&local).cua_configured);
        // Local's skills are `-local` copies beside Stable's.
        let skills: Vec<String> = local.skills().into_iter().map(|s| s.name).collect();
        assert!(skills.iter().all(|n| n.ends_with("-local")), "{skills:?}");
        local
            .install_skills(vec!["codex".into()], vec![], false)
            .await
            .unwrap();
        assert!(
            d.path()
                .join(".agents/skills/cua-spaces-local/SKILL.md")
                .is_file()
        );
        assert!(!d.path().join(".agents/skills/cua-spaces").exists());
    }

    #[tokio::test]
    async fn sdk_cua_driver_setup_in_a_temp_home() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        let s = AgentSetup::with_env(cua_agent_setup::HostEnv::isolated(d.path()));
        let out = s
            .setup_cua_driver(
                vec!["codex".into()],
                Some("/opt/drv/cua-driver".into()),
                true,
                true,
            )
            .await
            .unwrap();
        assert!(
            out.iter()
                .any(|o| o.target == AgentSetupTarget::Skill && o.item == "cua-driver"),
            "{out:?}"
        );
        assert!(
            out.iter().any(|o| o.target == AgentSetupTarget::Mcp
                && o.item == "cua-driver"
                && o.change == AgentSetupChange::Created),
            "{out:?}"
        );
        let codex = s.status().into_iter().find(|a| a.id == "codex").unwrap();
        assert!(codex.cua_driver_configured && !codex.cua_configured);
        let toml = std::fs::read_to_string(d.path().join(".codex/config.toml")).unwrap();
        assert!(toml.contains("/opt/drv/cua-driver"), "{toml}");
    }
}
