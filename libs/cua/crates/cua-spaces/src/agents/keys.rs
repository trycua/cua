//! Provider keys for agent runs, kept on this machine (Cua Spaces →
//! Settings → Connectors).
//!
//! The keys live in the OS credential vault through cua-auth's secret store
//! ([`cua_auth::Store::save_secret`]): one item, `agent-keys`, next to the
//! session (service `ai.cua.session`, or the channel's own,
//! `CuaChannel::keychain_service`), with the same access list, so the app,
//! the daemon and the CLI read it without a prompt. A build whose store is
//! a file (an unsigned macOS build, Linux) refuses to save them: keys are
//! never written to a plain file. Set the key in the daemon's environment
//! there instead.
//!
//! Nothing here returns a value to a caller: [`AgentKeys::list`] answers the
//! provider, the variable name, the last four characters and when it was
//! added. Values leave only into a run's env ([`super::run_env`]), for the
//! harness that declares the variable or a caller that names it in
//! `env_from_host`, never into logs or telemetry.

use crate::error::{Error, Result};
use cua_agents::claude_login;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The store's secret name.
pub const SECRET_NAME: &str = "agent-keys";

/// The app's settings methods (`SpaceService.CallSpaceTool` names). They are
/// not contract tools: no MCP server lists or calls them, so an agent can
/// never read or change a key.
pub const APP_METHODS: &[&str] = &[
    "agent_keys.list",
    "agent_keys.check",
    "agent_keys.set",
    "agent_keys.remove",
    "agent_keys.shell",
    "agent_keys.from_shell",
];

/// The provider keys Cua may read from your login shell, by name (Settings,
/// Connectors: "Use ANTHROPIC_API_KEY from your shell"). Nothing else is
/// ever read there.
pub const SHELL_NAMES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "COPILOT_GITHUB_TOKEN",
];

/// How long the login shell has to answer.
const SHELL_WAIT: Duration = Duration::from_secs(5);

/// The marker before the value in the shell's answer (anything a profile prints comes before it).
const SHELL_MARK: &str = "__cua_shell_value__";

/// Your login shell: `$SHELL`, else zsh.
pub fn login_shell() -> std::path::PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/bin/zsh".into())
}

/// `name`'s value in a login, interactive `shell` (the profile files your
/// terminal reads), when it is one of [`SHELL_NAMES`] and set. The shell's
/// stdin is closed and it is stopped after [`SHELL_WAIT`].
pub fn shell_value(shell: &std::path::Path, name: &str) -> Option<String> {
    if !SHELL_NAMES.contains(&name) {
        return None;
    }
    let script = format!("printf '\\n{SHELL_MARK}%s' \"${{{name}-}}\"");
    let mut child = std::process::Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < SHELL_WAIT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text = reader.join().ok()?;
    let value = text.rsplit_once(SHELL_MARK)?.1.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Which of [`SHELL_NAMES`] your login shell has set (names only).
pub fn shell_names(shell: &std::path::Path) -> Vec<String> {
    SHELL_NAMES
        .iter()
        .filter(|n| shell_value(shell, n).is_some())
        .map(|n| (*n).to_string())
        .collect()
}

/// Whether `name` is one of [`APP_METHODS`].
pub fn is_app_method(name: &str) -> bool {
    APP_METHODS.contains(&name)
}

/// A provider with its own row in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub env: &'static str,
}

/// Anthropic and OpenAI; anything else is `other` with its own name.
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "anthropic",
        label: "Anthropic",
        env: "ANTHROPIC_API_KEY",
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        env: "OPENAI_API_KEY",
    },
];

/// The provider id for a variable: its row's, else `other`.
pub fn provider_of(env: &str) -> &'static str {
    PROVIDERS
        .iter()
        .find(|p| p.env == env)
        .map(|p| p.id)
        .unwrap_or("other")
}

/// Names a stored key may not take: they would change how a run's shell,
/// loader or tools behave rather than give it a credential.
const REFUSED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "OLDPWD",
    "TMPDIR",
    "TMP",
    "TEMP",
    "IFS",
    "ENV",
    "BASH_ENV",
    "CDPATH",
    "TERM",
    "LANG",
    "LANGUAGE",
    "DISPLAY",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "PS1",
    "PS2",
    "PS4",
    "PROMPT_COMMAND",
    "SSH_AUTH_SOCK",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];
const REFUSED_PREFIXES: &[&str] = &[
    "DYLD_",
    "LD_",
    "CUA_",
    "BASH_FUNC_",
    "LC_",
    "XDG_",
    "GIT_",
    "NPM_CONFIG_",
    "MALLOC",
];

/// Checks a name for an Other key: an env identifier (`A-Z`, `a-z`, `0-9`,
/// `_`, not starting with a digit, at most 128 bytes) that is not one of
/// the variables a shell or loader reads (`PATH`, `HOME`, `DYLD_*`,
/// `LD_*`, ...).
pub fn validate_name(name: &str) -> Result<()> {
    let ident = !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit();
    if !ident {
        return Err(Error::InvalidArgument(format!(
            "{name:?} is not an environment variable name: use letters, digits and _, not starting with a digit"
        )));
    }
    let upper = name.to_ascii_uppercase();
    if REFUSED.contains(&upper.as_str()) || REFUSED_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        return Err(Error::InvalidArgument(format!(
            "{name} can't be used for an agent key: it changes how programs run"
        )));
    }
    Ok(())
}

/// Checks a key value: not empty, one line, at most 8 KiB.
fn validate_value(value: &str) -> Result<&str> {
    let v = value.trim();
    if v.is_empty() {
        return Err(Error::InvalidArgument("the key is empty".into()));
    }
    if v.len() > 8192 || v.chars().any(|c| c.is_control()) {
        return Err(Error::InvalidArgument(
            "the key must be one line of at most 8192 characters".into(),
        ));
    }
    Ok(v)
}

/// What the APIs answer about a stored key. Never its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    /// `anthropic`, `openai` or `other`.
    pub provider: String,
    /// The variable a run gets it as.
    pub env: String,
    /// Its last four characters (empty for a key shorter than 12).
    pub last4: String,
    /// When it was added (Unix ms).
    pub added_ms: u64,
    /// A copied sign-in's name ("ChatGPT sign-in for Codex"); none for a key
    /// typed in Settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The agent a copied sign-in came from (setup id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// When a copied sign-in stops working (Unix ms), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    /// It has stopped working (`expires_ms` has passed): runs no longer get it.
    pub expired: bool,
}

/// `agent_keys.list`: the stored keys and whether this machine can store
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeysReport {
    pub keys: Vec<KeyInfo>,
    pub providers: Vec<Provider>,
    /// Keys can be saved here.
    pub available: bool,
    /// Why not, when they can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    env: String,
    provider: String,
    value: String,
    added_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_ms: Option<u64>,
    /// When the provider (or an agent's run) said this credential is rejected (Unix ms). Saving
    /// the credential again replaces the entry, so a reconnect clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rejected_ms: Option<u64>,
    /// How a copied sign-in was made (`claude-login-keychain`, `claude-login-file`,
    /// `device-code`, `token`). A `claude-login-*` copy is re-read from Claude
    /// Code's own sign-in when it runs out ([`AgentKeys::sync_claude_login`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("env", &self.env)
            .field("provider", &self.provider)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl Stored {
    /// Past its `expires_ms` (a copied sign-in that stopped working).
    fn expired(&self) -> bool {
        self.expires_ms.is_some_and(|e| e <= now_ms())
    }

    /// Whether runs may be given it: not past its expiry and not rejected.
    fn usable(&self) -> bool {
        !self.expired() && self.rejected_ms.is_none()
    }

    fn info(&self) -> KeyInfo {
        let n = self.value.chars().count();
        let last4 = if n >= 12 {
            self.value.chars().skip(n - 4).collect()
        } else {
            String::new()
        };
        KeyInfo {
            provider: self.provider.clone(),
            env: self.env.clone(),
            last4,
            added_ms: self.added_ms,
            label: self.label.clone(),
            source: self.source.clone(),
            expires_ms: self.expires_ms,
            // A rejected credential reads like an expired one: it no longer works, Reconnect.
            expired: !self.usable(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Blob {
    #[serde(default)]
    keys: Vec<Stored>,
}

/// How long a read of the vault item is reused by readiness checks and a
/// run's env (they read in bursts). Short, because the daemon is not the one
/// writer: the app saves the sign-ins Connect makes into the same item, and
/// the next run must see them.
const CACHE_FOR: Duration = Duration::from_secs(2);

/// The variable Claude Code's sign-in is delivered as.
const CLAUDE_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// How long before its expiry a copied Claude sign-in is looked at again.
const REFRESH_WITHIN_MS: u64 = 30 * 60 * 1000;

/// The least time between two re-reads of Claude Code's own sign-in.
const RESYNC_EVERY: Duration = Duration::from_secs(20);

/// How long Claude Code may take to refresh its own sign-in when asked.
const ASK_WAIT: Duration = Duration::from_secs(25);

/// Where the freshness check reads Claude Code's own sign-in (a fake in tests).
pub trait ClaudeLogin: Send + Sync {
    /// Reads Claude Code's sign-in now.
    fn lookup(&self) -> claude_login::Lookup;
    /// Asks Claude Code to refresh its own sign-in; whether that ran well.
    fn ask_to_refresh(&self) -> bool;
}

/// This computer's Claude Code. Reads nothing in a test process.
pub struct HostClaudeLogin;

impl ClaudeLogin for HostClaudeLogin {
    fn lookup(&self) -> claude_login::Lookup {
        if cua_home::is_test_process() {
            return claude_login::Lookup::NotSignedIn;
        }
        let env = cua_agents::HostEnv::current();
        claude_login::lookup(&env, &*claude_login::default_keychain(&env), now_ms())
    }

    fn ask_to_refresh(&self) -> bool {
        let env = cua_agents::HostEnv::current();
        // Called from async code: tell the runtime this thread blocks.
        let ask = || claude_login::ask_claude_to_refresh(&env, ASK_WAIT);
        match tokio::runtime::Handle::try_current() {
            Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(ask)
            }
            _ => ask(),
        }
    }
}

/// What [`AgentKeys::sync_claude_login`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginSync {
    /// There is no copy of Claude Code's sign-in.
    NotApplicable,
    /// The copy works and is not about to run out.
    Fresh,
    /// A newer sign-in was read from Claude Code and saved.
    Updated,
    /// The copy has run out and Claude Code has nothing newer: Reconnect.
    Stale,
}

/// The stored agent keys of one credential store.
pub struct AgentKeys {
    store: Option<cua_auth::Store>,
    unavailable: Option<String>,
    // Serializes read-modify-write; holds the last read.
    cache: Mutex<Option<(Instant, Vec<Stored>)>>,
    // When Claude Code's own sign-in was last re-read ([`AgentKeys::sync_claude_login`]).
    claude_sync: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for AgentKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentKeys")
            .field("store", &self.store.as_ref().map(|s| s.kind()))
            .finish()
    }
}

/// Why a file store can't hold agent keys, in the words of this system (the
/// app shows it as "Keys can't be saved here: <this>."). A Mac's unsigned
/// build and Windows with `CUA_CREDENTIAL_STORE=file` have a vault they
/// could use; Linux has none.
const fn file_store_reason() -> &'static str {
    if cfg!(target_os = "macos") {
        "this build of cua keeps credentials in a file, and agent keys are only kept in the Keychain (a Cua-signed build of Cua Spaces does). Set the key for the Cua daemon instead"
    } else if cfg!(target_os = "windows") {
        "cua is set to keep credentials in a file (CUA_CREDENTIAL_STORE=file), and agent keys are only kept in Windows Credential Manager. Set the key for the Cua daemon instead"
    } else {
        "cua has no secure place to keep agent keys on this computer yet, and they are never saved to a file. Set the key for the Cua daemon instead"
    }
}

impl AgentKeys {
    /// Keys in `store`. A file store can't hold them (see the module docs).
    pub fn new(store: cua_auth::Store) -> Self {
        if store.kind() == "keychain" {
            Self {
                store: Some(store),
                unavailable: None,
                cache: Mutex::new(None),
                claude_sync: Mutex::new(None),
            }
        } else {
            Self::unavailable(file_store_reason())
        }
    }

    /// Nowhere to keep keys, for `reason`.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            store: None,
            unavailable: Some(reason.into()),
            cache: Mutex::new(None),
            claude_sync: Mutex::new(None),
        }
    }

    /// Whether keys can be saved.
    pub fn available(&self) -> bool {
        self.store.is_some()
    }

    fn read(&self, guard: &mut Option<(Instant, Vec<Stored>)>, fresh: bool) -> Result<Vec<Stored>> {
        let Some(store) = &self.store else {
            return Ok(vec![]);
        };
        if !fresh
            && let Some((at, keys)) = guard.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return Ok(keys.clone());
        }
        let keys = match store.load_secret(SECRET_NAME).map_err(store_err)? {
            None => vec![],
            Some(bytes) => {
                serde_json::from_slice::<Blob>(&bytes)
                    .map_err(|_| Error::Agent("the stored agent keys can't be read".into()))?
                    .keys
            }
        };
        *guard = Some((Instant::now(), keys.clone()));
        Ok(keys)
    }

    fn write(&self, guard: &mut Option<(Instant, Vec<Stored>)>, keys: Vec<Stored>) -> Result<()> {
        let store = self.store.as_ref().ok_or_else(|| self.refusal())?;
        if keys.is_empty() {
            store.clear_secret(SECRET_NAME).map_err(store_err)?;
        } else {
            let blob = serde_json::to_vec(&Blob { keys: keys.clone() })?;
            store.save_secret(SECRET_NAME, &blob).map_err(store_err)?;
        }
        *guard = Some((Instant::now(), keys));
        Ok(())
    }

    fn refusal(&self) -> Error {
        Error::HostCapabilityMissing {
            what: "Keeping agent keys".into(),
            why: self.unavailable.clone().unwrap_or_default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Instant, Vec<Stored>)>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The stored keys, without their values.
    pub fn list(&self) -> Result<Vec<KeyInfo>> {
        let mut g = self.lock();
        Ok(self.read(&mut g, true)?.iter().map(Stored::info).collect())
    }

    /// [`AgentKeys::list`] with the providers and availability.
    pub fn report(&self) -> Result<KeysReport> {
        Ok(KeysReport {
            keys: self.list()?,
            providers: PROVIDERS.to_vec(),
            available: self.available(),
            unavailable: self.unavailable.clone(),
        })
    }

    /// Adds or replaces a key. `provider` is `anthropic`, `openai` or
    /// `other`; `other` needs `env` (checked by [`validate_name`]).
    pub fn set(&self, provider: &str, env: Option<&str>, value: &str) -> Result<KeyInfo> {
        let env = match (provider, env.map(str::trim).filter(|e| !e.is_empty())) {
            ("other", Some(e)) => {
                validate_name(e)?;
                e.to_string()
            }
            ("other", None) => {
                return Err(Error::InvalidArgument(
                    "an Other key needs its environment variable name".into(),
                ));
            }
            (p, _) => PROVIDERS
                .iter()
                .find(|x| x.id == p)
                .ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "unknown provider {p:?}: use anthropic, openai or other"
                    ))
                })?
                .env
                .to_string(),
        };
        let value = validate_value(value)?;
        if self.store.is_none() {
            return Err(self.refusal());
        }
        let mut g = self.lock();
        let mut keys = self.read(&mut g, true)?;
        keys.retain(|k| k.env != env);
        let stored = Stored {
            provider: provider_of(&env).into(),
            env,
            value: value.into(),
            added_ms: now_ms(),
            label: None,
            source: None,
            expires_ms: None,
            rejected_ms: None,
            origin: None,
        };
        let info = stored.info();
        keys.push(stored);
        keys.sort_by(|a, b| a.env.cmp(&b.env));
        self.write(&mut g, keys)?;
        Ok(info)
    }

    /// Removes the key named `env`; whether there was one.
    pub fn remove(&self, env: &str) -> Result<bool> {
        if self.store.is_none() {
            return Ok(false);
        }
        let mut g = self.lock();
        let mut keys = self.read(&mut g, true)?;
        let before = keys.len();
        keys.retain(|k| k.env != env);
        if keys.len() == before {
            return Ok(false);
        }
        self.write(&mut g, keys)?;
        Ok(true)
    }

    /// Saves a sign-in Connect copied from `source` (an agent's setup id)
    /// under `env`, named `label`. It replaces an earlier copy from the
    /// same agent, but never a key the person typed in Settings: that one
    /// is kept and the copy refused.
    pub fn import(
        &self,
        source: &str,
        env: &str,
        label: &str,
        value: &str,
        expires_ms: Option<u64>,
        origin: Option<&str>,
    ) -> Result<KeyInfo> {
        validate_name(env)?;
        let value = validate_value(value)?;
        if self.store.is_none() {
            return Err(self.refusal());
        }
        let mut g = self.lock();
        let mut keys = self.read(&mut g, true)?;
        if keys
            .iter()
            .any(|k| k.env == env && k.source.as_deref() != Some(source))
        {
            return Err(Error::InvalidArgument(format!(
                "{env} is already in Keyvault; remove it in Settings → Connectors to copy this one"
            )));
        }
        keys.retain(|k| k.source.as_deref() != Some(source));
        let stored = Stored {
            provider: provider_of(env).into(),
            env: env.into(),
            value: value.into(),
            added_ms: now_ms(),
            label: Some(label.into()),
            source: Some(source.into()),
            expires_ms,
            rejected_ms: None,
            origin: origin.map(str::to_string),
        };
        let info = stored.info();
        keys.push(stored);
        keys.sort_by(|a, b| a.env.cmp(&b.env));
        self.write(&mut g, keys)?;
        Ok(info)
    }

    /// Deletes every copy from `source`; the variables removed.
    pub fn remove_imported(&self, source: &str) -> Result<Vec<String>> {
        if self.store.is_none() {
            return Ok(vec![]);
        }
        let mut g = self.lock();
        let mut keys = self.read(&mut g, true)?;
        let removed: Vec<String> = keys
            .iter()
            .filter(|k| k.source.as_deref() == Some(source))
            .map(|k| k.env.clone())
            .collect();
        if removed.is_empty() {
            return Ok(removed);
        }
        keys.retain(|k| k.source.as_deref() != Some(source));
        self.write(&mut g, keys)?;
        Ok(removed)
    }

    /// The copied sign-ins (no values), read fresh.
    pub fn imported(&self) -> Result<Vec<KeyInfo>> {
        let mut g = self.lock();
        Ok(self
            .read(&mut g, true)?
            .iter()
            .filter(|k| k.source.is_some())
            .map(Stored::info)
            .collect())
    }

    /// Keeps a copy of Claude Code's own sign-in usable, without ever
    /// refreshing it ourselves.
    ///
    /// Claude Code's access token lasts hours, and its refresh token works
    /// once: each refresh returns a new pair and spends the old one. If Cua
    /// refreshed, Claude Code's next refresh would fail and sign the person
    /// out of Claude Code. So Cua keeps the access token only, and when its
    /// copy is within [`REFRESH_WITHIN_MS`] of running out (or a run said it
    /// was refused) it reads Claude Code's own sign-in again: Claude Code
    /// refreshes it as it is used and writes the new pair under its own lock.
    /// With `ask` and nothing newer there, Claude Code is asked to refresh by
    /// a one-line request ([`ClaudeLogin::ask_to_refresh`], at most every five
    /// minutes), waiting a few seconds. Never a request of ours to Anthropic,
    /// and nothing is checked live before saving.
    ///
    /// Re-read at most every 20 seconds. Only copies made by importing
    /// Claude Code's sign-in are touched.
    pub fn sync_claude_login(&self, src: &dyn ClaudeLogin, ask: bool) -> LoginSync {
        let copy = {
            let mut g = self.lock();
            match self.read(&mut g, false) {
                Ok(keys) => keys.into_iter().find(|k| {
                    k.env == CLAUDE_ENV
                        && k.origin
                            .as_deref()
                            .is_some_and(|o| o.starts_with("claude-login"))
                }),
                Err(_) => None,
            }
        };
        let Some(copy) = copy else {
            return LoginSync::NotApplicable;
        };
        let now = now_ms();
        let rejected = copy.rejected_ms.is_some();
        let running_out = rejected
            || copy
                .expires_ms
                .is_some_and(|e| e <= now + REFRESH_WITHIN_MS);
        if !running_out {
            return LoginSync::Fresh;
        }
        let verdict = |copy: &Stored| {
            if copy.usable() {
                LoginSync::Fresh
            } else {
                LoginSync::Stale
            }
        };
        {
            let mut last = self.claude_sync.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|t| t.elapsed() < RESYNC_EVERY) {
                return verdict(&copy);
            }
            *last = Some(Instant::now());
        }
        let newer = |l: &claude_login::Lookup| -> Option<claude_login::Found> {
            match l {
                claude_login::Lookup::Found(f)
                    if !f.oauth.expired(now_ms())
                        && f.oauth.access_token != copy.value
                        && (rejected
                            || f.oauth.expires_ms.unwrap_or(u64::MAX)
                                > copy.expires_ms.unwrap_or(0)) =>
                {
                    Some(f.clone())
                }
                _ => None,
            }
        };
        let mut found = newer(&src.lookup());
        if found.is_none() && ask && src.ask_to_refresh() {
            found = newer(&src.lookup());
        }
        let Some(f) = found else {
            return verdict(&copy);
        };
        let origin = match f.source {
            claude_login::Source::Keychain { .. } => "claude-login-keychain",
            claude_login::Source::File(_) => "claude-login-file",
        };
        match self.import(
            "claude-code",
            CLAUDE_ENV,
            &f.label(),
            &f.oauth.access_token,
            f.oauth.expires_ms,
            Some(origin),
        ) {
            Ok(_) => LoginSync::Updated,
            Err(_) => verdict(&copy),
        }
    }

    /// The names of the stored keys (readiness). A store that can't be read
    /// has none.
    pub fn names(&self) -> Vec<String> {
        let mut g = self.lock();
        match self.read(&mut g, false) {
            Ok(keys) => keys
                .into_iter()
                .filter(Stored::usable)
                .map(|k| k.env)
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "agent keys: the store can't be read");
                vec![]
            }
        }
    }

    /// The values of the stored keys named in `names` (only those), for a
    /// run's env.
    pub fn values(&self, names: &[&str]) -> BTreeMap<String, String> {
        if names.is_empty() {
            return BTreeMap::new();
        }
        let mut g = self.lock();
        match self.read(&mut g, false) {
            Ok(keys) => keys
                .into_iter()
                // A sign-in past its expiry is not forwarded: the run would
                // only be told 401 by the provider.
                .filter(|k| names.contains(&k.env.as_str()) && k.usable())
                .map(|k| (k.env, k.value))
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "agent keys: the store can't be read");
                BTreeMap::new()
            }
        }
    }
}

impl AgentKeys {
    /// Notes that the credentials named in `envs` were rejected by their provider (a run of an
    /// agent got an authentication failure with them): they read as needing a reconnect and no run
    /// gets them until they are saved again. Returns the names that changed.
    pub fn mark_rejected(&self, envs: &[&str]) -> Result<Vec<String>> {
        self.set_rejected(envs.iter().map(|e| (*e, None)), Some(now_ms()))
    }

    /// Sets (`Some`) or clears (`None`) the rejection of each `(env, only if added at)`; a
    /// credential saved again since (a different `added_ms`) is left alone.
    fn set_rejected<'a>(
        &self,
        which: impl Iterator<Item = (&'a str, Option<u64>)>,
        at: Option<u64>,
    ) -> Result<Vec<String>> {
        if self.store.is_none() {
            return Ok(vec![]);
        }
        let which: Vec<(&str, Option<u64>)> = which.collect();
        let mut g = self.lock();
        let mut keys = self.read(&mut g, true)?;
        let mut changed = vec![];
        for k in keys.iter_mut() {
            let Some((_, added)) = which.iter().find(|(e, _)| *e == k.env) else {
                continue;
            };
            if added.is_some_and(|a| a != k.added_ms) || k.rejected_ms.is_some() == at.is_some() {
                continue;
            }
            k.rejected_ms = at;
            changed.push(k.env.clone());
        }
        if !changed.is_empty() {
            self.write(&mut g, keys)?;
        }
        Ok(changed)
    }

    /// The API keys to check against their provider: `(env, added_ms, value)`. A sign-in token
    /// (`CLAUDE_CODE_OAUTH_TOKEN`, `CODEX_AUTH_JSON`) has no check that is known to accept it,
    /// so it is not asked about.
    fn checkable(&self) -> Vec<(String, u64, String)> {
        let mut g = self.lock();
        match self.read(&mut g, true) {
            Ok(keys) => keys
                .into_iter()
                .filter(|k| endpoint_for(&k.env).is_some() && !k.expired())
                .map(|k| (k.env, k.added_ms, k.value))
                .collect(),
            Err(_) => vec![],
        }
    }
}

/// What the provider said of a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// The provider accepted it.
    Valid,
    /// The provider refused it (HTTP 401 or 403): it does not work.
    Rejected,
    /// No answer worth acting on (offline, rate limited, a server error): leave it as it was.
    Unknown,
}

/// The providers' read-only "list models" requests: free, about 100 ms, and they answer 401 for a
/// key that does not work. Only API keys are asked about (see [`AgentKeys::checkable`]).
#[derive(Clone, Debug)]
pub struct CheckEndpoints {
    pub anthropic: String,
    pub openai: String,
}

impl Default for CheckEndpoints {
    fn default() -> Self {
        Self {
            anthropic: "https://api.anthropic.com/v1/models?limit=1".into(),
            openai: "https://api.openai.com/v1/models".into(),
        }
    }
}

/// Which provider's request checks `env`, if it is an API key we can check.
fn endpoint_for(env: &str) -> Option<&'static str> {
    match env {
        "ANTHROPIC_API_KEY" => Some("anthropic"),
        "OPENAI_API_KEY" => Some("openai"),
        _ => None,
    }
}

/// How long an answer is reused (Settings opens and the Agents page opens ask often).
const CHECK_CACHE: Duration = Duration::from_secs(5 * 60);

type CheckCache = Mutex<BTreeMap<(String, u64), (Instant, KeyCheck)>>;

fn check_cache() -> &'static CheckCache {
    static CACHE: OnceLock<CheckCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Asks the provider about one key. The value only goes into the request's header; nothing here
/// logs it, and an error is reported without its text.
async fn ask_provider(
    client: &reqwest::Client,
    ep: &CheckEndpoints,
    env: &str,
    value: &str,
) -> KeyCheck {
    let request = match endpoint_for(env) {
        Some("anthropic") => client
            .get(&ep.anthropic)
            .header("x-api-key", value)
            .header("anthropic-version", "2023-06-01"),
        Some(_) => client.get(&ep.openai).bearer_auth(value),
        None => return KeyCheck::Unknown,
    };
    match request.send().await {
        Ok(r) if r.status().is_success() => KeyCheck::Valid,
        Ok(r) if matches!(r.status().as_u16(), 401 | 403) => KeyCheck::Rejected,
        _ => KeyCheck::Unknown,
    }
}

impl AgentKeys {
    /// Asks each provider whether the saved API keys still work (a cheap read-only request,
    /// answers reused for five minutes, never logged), notes a refused key as needing a reconnect
    /// and clears the note on one that is accepted again. Sign-in tokens are not asked about.
    /// Never fails the caller: the list that follows says what is known.
    pub async fn recheck(&self, ep: &CheckEndpoints) -> Result<KeysReport> {
        let todo = self.checkable();
        if !todo.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .build()
                .map_err(|e| Error::Agent(e.to_string()))?;
            let mut rejected: Vec<(String, u64)> = vec![];
            let mut accepted: Vec<(String, u64)> = vec![];
            for (env, added, value) in todo {
                let cached = check_cache()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&(env.clone(), added))
                    .filter(|(at, _)| at.elapsed() < CHECK_CACHE)
                    .map(|(_, c)| *c);
                let answer = match cached {
                    Some(c) => c,
                    None => {
                        let c = ask_provider(&client, ep, &env, &value).await;
                        if c != KeyCheck::Unknown {
                            check_cache()
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .insert((env.clone(), added), (Instant::now(), c));
                        }
                        c
                    }
                };
                match answer {
                    KeyCheck::Rejected => rejected.push((env, added)),
                    KeyCheck::Valid => accepted.push((env, added)),
                    KeyCheck::Unknown => {}
                }
            }
            self.set_rejected(
                rejected.iter().map(|(e, a)| (e.as_str(), Some(*a))),
                Some(now_ms()),
            )?;
            self.set_rejected(accepted.iter().map(|(e, a)| (e.as_str(), Some(*a))), None)?;
        }
        self.report()
    }
}

/// Notes that an agent run got an authentication failure: the saved credentials of `provider`
/// that `harness` reads (and only those) read as needing a reconnect.
pub fn mark_rejected_for_run(harness: &str, provider: &str) {
    let Some(h) = super::harness::harness(harness) else {
        return;
    };
    let envs: Vec<&str> = h
        .keys
        .iter()
        .copied()
        .filter(|k| provider_envs(provider).contains(k))
        .collect();
    if envs.is_empty() {
        return;
    }
    if let Err(e) = global().mark_rejected(&envs) {
        tracing::warn!(error = %e, "agent keys: could not note a rejected credential");
    }
}

/// The variables that hold a provider's credentials (a key, a sign-in token).
fn provider_envs(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        "openai" => &["OPENAI_API_KEY", "CODEX_AUTH_JSON"],
        _ => &[],
    }
}

fn store_err(e: cua_auth::Error) -> Error {
    Error::Agent(format!("the Keychain refused the agent keys: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// This process's agent keys: the default credential store
/// ([`cua_auth::Store::from_env`]). A test process never uses the real OS
/// vault here (set `CUA_CREDENTIAL_STORE=test-keychain:<dir>` instead).
pub fn global() -> &'static AgentKeys {
    static KEYS: OnceLock<AgentKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        let store = cua_auth::Store::from_env();
        if cua_home::is_test_process() && !matches!(store, cua_auth::Store::TestKeychain(_)) {
            return AgentKeys::unavailable("a test process keeps no agent keys");
        }
        AgentKeys::new(store)
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    provider: String,
    #[serde(default)]
    env: Option<String>,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveArgs {
    env: String,
}

#[derive(Serialize)]
struct ShellReport {
    /// The provider keys your login shell has set (names; never a value).
    names: Vec<String>,
}

/// Runs one of [`APP_METHODS`] against `keys`; every answer is the
/// [`KeysReport`] after the change, except `agent_keys.shell` (which names
/// your login shell has set). `None` for any other name. `shell` is the
/// login shell `agent_keys.shell` and `agent_keys.from_shell` read.
pub fn app_call(
    keys: &AgentKeys,
    name: &str,
    args: serde_json::Value,
    shell: &std::path::Path,
) -> Option<Result<serde_json::Value>> {
    let args = if args.is_null() {
        serde_json::json!({})
    } else {
        args
    };
    let report = |r: Result<KeysReport>| {
        r.and_then(|r| serde_json::to_value(r).map_err(|e| Error::Agent(e.to_string())))
    };
    Some(match name {
        // The check needs the network: `app_tool_async` does it; a synchronous caller gets the list.
        "agent_keys.list" | "agent_keys.check" => {
            // A copy of Claude Code's sign-in that ran out is read again from
            // Claude Code before Settings calls it "Needs reconnect".
            keys.sync_claude_login(&HostClaudeLogin, false);
            report(keys.report())
        }
        "agent_keys.set" => report(
            serde_json::from_value::<SetArgs>(args)
                .map_err(|e| {
                    Error::InvalidArgument(format!("agent_keys.set: {}", redact_serde(&e)))
                })
                .and_then(|a| keys.set(&a.provider, a.env.as_deref(), &a.value))
                .and_then(|_| keys.report()),
        ),
        "agent_keys.remove" => report(
            serde_json::from_value::<RemoveArgs>(args)
                .map_err(|e| Error::InvalidArgument(format!("agent_keys.remove: {e}")))
                .and_then(|a| keys.remove(&a.env))
                .and_then(|_| keys.report()),
        ),
        "agent_keys.shell" => serde_json::to_value(ShellReport {
            names: shell_names(shell),
        })
        .map_err(|e| Error::Agent(e.to_string())),
        // The value goes straight from your shell into the store; it is never answered.
        "agent_keys.from_shell" => report(
            serde_json::from_value::<RemoveArgs>(args)
                .map_err(|e| Error::InvalidArgument(format!("agent_keys.from_shell: {e}")))
                .and_then(|a| {
                    let value = shell_value(shell, &a.env).ok_or_else(|| {
                        Error::InvalidArgument(format!("{} isn't set in your shell", a.env))
                    })?;
                    let provider = provider_of(&a.env);
                    keys.set(
                        provider,
                        (provider == "other").then_some(a.env.as_str()),
                        &value,
                    )
                })
                .and_then(|_| keys.report()),
        ),
        _ => return None,
    })
}

/// [`app_call`] on this process's keys as a tool answer (the daemon's
/// `CallSpaceTool` and the SDK's embedded runtime). `None` for any name
/// that is not one of [`APP_METHODS`].
#[cfg(feature = "mcp")]
pub fn app_tool(name: &str, args: serde_json::Value) -> Option<crate::mcp::ToolOutcome> {
    app_call(global(), name, args, &login_shell()).map(|r| match r {
        Ok(report) => crate::mcp::ToolOutcome::json(&report),
        Err(e) => crate::mcp::ToolOutcome::error(&e),
    })
}

/// [`app_tool`] for a caller that can wait: `agent_keys.check` asks the providers whether the saved
/// API keys still work, then answers like `agent_keys.list`.
#[cfg(feature = "mcp")]
pub async fn app_tool_async(
    name: &str,
    args: serde_json::Value,
) -> Option<crate::mcp::ToolOutcome> {
    if name != "agent_keys.check" {
        return app_tool(name, args);
    }
    Some(match global().recheck(&CheckEndpoints::default()).await {
        Ok(report) => match serde_json::to_value(report) {
            Ok(v) => crate::mcp::ToolOutcome::json(&v),
            Err(e) => crate::mcp::ToolOutcome::error(&Error::Agent(e.to_string())),
        },
        Err(e) => crate::mcp::ToolOutcome::error(&e),
    })
}

/// A serde error without the input it quotes (it could be the key).
fn redact_serde(e: &serde_json::Error) -> String {
    let s = e.to_string();
    if s.contains("missing field") || s.contains("unknown field") {
        s
    } else {
        "expected {provider, env?, value} as strings".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_keys() -> (AgentKeys, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "cua-agent-keys-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (
            AgentKeys::new(cua_auth::Store::TestKeychain(dir.clone())),
            dir,
        )
    }

    #[test]
    fn keys_are_stored_and_never_answered() {
        let (keys, dir) = temp_keys();
        assert!(keys.list().unwrap().is_empty());
        let info = keys
            .set("anthropic", None, "  sk-ant-test-0000-abcd  ")
            .unwrap();
        assert_eq!(info.env, "ANTHROPIC_API_KEY");
        assert_eq!(info.provider, "anthropic");
        assert_eq!(info.last4, "abcd");
        keys.set("other", Some("MISTRAL_API_KEY"), "mk-123456789012")
            .unwrap();
        // Replacing keeps one entry.
        keys.set("anthropic", None, "sk-ant-test-0000-wxyz")
            .unwrap();
        let listed = keys.list().unwrap();
        assert_eq!(listed.len(), 2);
        let report = serde_json::to_string(&keys.report().unwrap()).unwrap();
        assert!(!report.contains("sk-ant-test-0000"), "{report}");
        assert!(report.contains("wxyz") && report.contains("MISTRAL_API_KEY"));
        // A fresh handle reads the same store (no cache).
        let again = AgentKeys::new(cua_auth::Store::TestKeychain(dir.clone()));
        assert_eq!(
            again.values(&["ANTHROPIC_API_KEY"]),
            BTreeMap::from([(
                "ANTHROPIC_API_KEY".to_string(),
                "sk-ant-test-0000-wxyz".to_string()
            )])
        );
        // Only the names asked for.
        assert!(again.values(&["OPENAI_API_KEY"]).is_empty());
        assert!(!format!("{:?}", again.lock()).contains("sk-ant"));
        assert!(keys.remove("ANTHROPIC_API_KEY").unwrap());
        assert!(!keys.remove("ANTHROPIC_API_KEY").unwrap());
        assert_eq!(keys.names(), vec!["MISTRAL_API_KEY".to_string()]);
        assert!(keys.remove("MISTRAL_API_KEY").unwrap());
        // The last key gone: the item is gone too.
        assert!(!dir.join(SECRET_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sign_in_the_app_saves_shows_in_the_daemons_next_list_and_run() {
        // The daemon reads the list (and warms its cache) before Connect.
        let (daemon, dir) = temp_keys();
        assert!(daemon.list().unwrap().is_empty());
        assert!(daemon.names().is_empty());
        // The app saves the sign-in Connect made into the same item.
        let app = AgentKeys::new(cua_auth::Store::TestKeychain(dir.clone()));
        app.import(
            "codex",
            "CODEX_AUTH_JSON",
            "ChatGPT sign-in for Codex",
            "{\"tokens\":{}}",
            None,
            None,
        )
        .unwrap();
        // Settings' next read sees it right away.
        let listed = daemon.list().unwrap();
        assert_eq!(
            listed.iter().map(|k| k.env.as_str()).collect::<Vec<_>>(),
            ["CODEX_AUTH_JSON"]
        );
        // And a run started a moment later gets it.
        std::thread::sleep(CACHE_FOR + Duration::from_millis(50));
        assert!(
            daemon
                .values(&["CODEX_AUTH_JSON"])
                .contains_key("CODEX_AUTH_JSON")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An expired sign-in is listed as expired, and no run gets it.
    #[test]
    fn an_expired_sign_in_reaches_no_run() {
        let (keys, dir) = temp_keys();
        let long_ago = now_ms() - 1_000;
        keys.import(
            "claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "Signed in as ada@example.com (Claude Max)",
            "sk-ant-oat01-test",
            Some(long_ago),
            None,
        )
        .unwrap();
        let listed = keys.list().unwrap();
        assert!(listed[0].expired && listed[0].expires_ms == Some(long_ago));
        assert!(keys.names().is_empty());
        assert!(keys.values(&["CLAUDE_CODE_OAUTH_TOKEN"]).is_empty());
        // One that has time left still goes.
        keys.import(
            "claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "Signed in as ada@example.com (Claude Max)",
            "sk-ant-oat01-test",
            Some(now_ms() + 3_600_000),
            None,
        )
        .unwrap();
        assert!(!keys.list().unwrap()[0].expired);
        assert_eq!(keys.names(), ["CLAUDE_CODE_OAUTH_TOKEN"]);
        assert_eq!(keys.values(&["CLAUDE_CODE_OAUTH_TOKEN"]).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tiny local provider: answers 200 to the key `good`, 401 to anything else, 500 to `flaky`;
    /// counts the requests it got.
    async fn provider() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/models", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let status = if req.contains("good") {
                    "200 OK"
                } else if req.contains("flaky") {
                    "500 Internal Server Error"
                } else {
                    "401 Unauthorized"
                };
                let _ = sock
                    .write_all(
                        format!("HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}")
                            .as_bytes(),
                    )
                    .await;
            }
        });
        (url, hits)
    }

    /// A key the provider refuses reads as needing a reconnect and reaches no run; one it accepts
    /// stays; a key saved again clears the note; sign-in tokens are not asked about; the answer is
    /// reused, never logged and never returned.
    #[tokio::test]
    async fn a_saved_key_the_provider_refuses_needs_a_reconnect() {
        let (keys, dir) = temp_keys();
        let (url, hits) = provider().await;
        let ep = CheckEndpoints {
            anthropic: url.clone(),
            openai: url,
        };
        keys.set("anthropic", None, "sk-ant-stale-0000-abcd")
            .unwrap();
        keys.set("openai", None, "sk-good-1111-wxyz").unwrap();
        keys.import(
            "claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "Claude sign-in",
            "sk-ant-oat01-whatever",
            None,
            None,
        )
        .unwrap();
        let report = keys.recheck(&ep).await.unwrap();
        let by = |env: &str| report.keys.iter().find(|k| k.env == env).unwrap().expired;
        assert!(by("ANTHROPIC_API_KEY"), "the refused key needs a reconnect");
        assert!(!by("OPENAI_API_KEY"));
        // The sign-in token is not asked about: no request for it, and it stays as it was.
        assert!(!by("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        // The refused key reaches no run; the others do.
        assert_eq!(keys.names(), ["CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"]);
        assert!(keys.values(&["ANTHROPIC_API_KEY"]).is_empty());
        // Opened again a moment later: the answers are reused.
        keys.recheck(&ep).await.unwrap();
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        // The report never carries a value.
        let text = serde_json::to_string(&report).unwrap();
        assert!(
            !text.contains("sk-ant-stale") && !text.contains("sk-good"),
            "{text}"
        );
        // Reconnecting (saving it again) clears the note.
        keys.set("anthropic", None, "sk-good-2222-abcd").unwrap();
        assert!(!keys.list().unwrap().iter().any(|k| k.expired));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A provider that cannot answer is no reason to flag a key.
    #[tokio::test]
    async fn an_unreachable_provider_flags_nothing() {
        let (keys, dir) = temp_keys();
        let (url, _) = provider().await;
        let ep = CheckEndpoints {
            anthropic: url.clone(),
            openai: url,
        };
        keys.set("openai", None, "sk-flaky-3333-wxyz").unwrap();
        assert!(!keys.recheck(&ep).await.unwrap().keys[0].expired);
        let offline = CheckEndpoints {
            anthropic: "http://127.0.0.1:1/x".into(),
            openai: "http://127.0.0.1:1/x".into(),
        };
        keys.set("anthropic", None, "sk-ant-offline-4444").unwrap();
        assert!(
            !keys
                .recheck(&offline)
                .await
                .unwrap()
                .keys
                .iter()
                .any(|k| k.expired)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A run's authentication failure marks only the credentials its harness reads, for its provider.
    #[test]
    fn a_run_that_fails_to_authenticate_marks_what_it_was_given() {
        let (keys, dir) = temp_keys();
        keys.set("anthropic", None, "sk-ant-api-0000-abcd").unwrap();
        keys.import(
            "claude-code",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "Claude sign-in",
            "sk-ant-oat01-whatever",
            None,
            None,
        )
        .unwrap();
        keys.set("openai", None, "sk-openai-1111-wxyz").unwrap();
        // Hermes read the API key (not the sign-in token), and said Anthropic refused it.
        let h = super::super::harness::harness("hermes").unwrap();
        let envs: Vec<&str> = h
            .keys
            .iter()
            .copied()
            .filter(|k| provider_envs("anthropic").contains(k))
            .collect();
        assert!(envs.contains(&"ANTHROPIC_API_KEY"));
        keys.mark_rejected(&envs).unwrap();
        let expired = |env: &str| {
            keys.list()
                .unwrap()
                .iter()
                .find(|k| k.env == env)
                .unwrap()
                .expired
        };
        assert!(expired("ANTHROPIC_API_KEY"));
        assert!(!expired("OPENAI_API_KEY"));
        // Noting it twice changes nothing more.
        assert!(keys.mark_rejected(&envs).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_key_shows_no_characters() {
        let (keys, dir) = temp_keys();
        assert_eq!(keys.set("openai", None, "short").unwrap().last4, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_store_keeps_no_keys() {
        let dir = std::env::temp_dir().join(format!("cua-agent-keys-file-{}", std::process::id()));
        let keys = AgentKeys::new(cua_auth::Store::File(dir.join("credentials.json")));
        assert!(!keys.available());
        let e = keys.set("anthropic", None, "sk-ant-test-0000").unwrap_err();
        // The refusal names this system's vault, or says there is none (Linux).
        let named = if cfg!(target_os = "macos") {
            "Keychain"
        } else if cfg!(target_os = "windows") {
            "Windows Credential Manager"
        } else {
            "no secure place"
        };
        assert!(e.to_string().contains(named), "{e}");
        if !cfg!(target_os = "macos") {
            assert!(!e.to_string().contains("Keychain"), "{e}");
        }
        assert!(!dir.exists());
        let r = keys.report().unwrap();
        assert!(!r.available && r.unavailable.is_some() && r.keys.is_empty());
    }

    #[test]
    fn other_names_must_be_safe_env_names() {
        for ok in ["MISTRAL_API_KEY", "groq_key", "_X", "A1"] {
            validate_name(ok).unwrap();
        }
        for bad in [
            "",
            "1KEY",
            "MY-KEY",
            "MY KEY",
            "KEY=1",
            "PATH",
            "path",
            "HOME",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
            "CUA_HOME",
            "NODE_OPTIONS",
            "BASH_ENV",
            "BASH_FUNC_x%%",
            "SHELL",
        ] {
            assert!(validate_name(bad).is_err(), "{bad} accepted");
        }
        assert!(validate_name(&"A".repeat(129)).is_err());
        let (keys, dir) = temp_keys();
        assert!(keys.set("other", Some("LD_PRELOAD"), "x").is_err());
        assert!(keys.set("other", None, "x").is_err());
        assert!(keys.set("gemini", None, "x").is_err());
        assert!(keys.set("openai", None, "   ").is_err());
        assert!(keys.set("openai", None, "a\nb").is_err());
        assert!(keys.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The fake login shell is a /bin/sh script: unix only.
    #[cfg(unix)]
    #[test]
    fn a_key_from_your_shell_is_saved_and_only_its_name_is_answered() {
        let (keys, dir) = temp_keys();
        // A login shell whose profile prints noise and exports one key; it runs the command it is given.
        let shell = dir.join("fake-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\necho 'welcome to your shell'\nANTHROPIC_API_KEY=sk-ant-from-shell-0000-wxyz; export ANTHROPIC_API_KEY\nshift 3\nexec /bin/sh -c \"$1\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let names = app_call(&keys, "agent_keys.shell", serde_json::json!({}), &shell)
            .unwrap()
            .unwrap();
        assert_eq!(names, serde_json::json!({"names": ["ANTHROPIC_API_KEY"]}));
        let r = app_call(
            &keys,
            "agent_keys.from_shell",
            serde_json::json!({"env": "ANTHROPIC_API_KEY"}),
            &shell,
        )
        .unwrap()
        .unwrap();
        let text = serde_json::to_string(&r).unwrap();
        assert!(
            text.contains("wxyz") && !text.contains("sk-ant-from-shell"),
            "{text}"
        );
        assert_eq!(
            keys.values(&["ANTHROPIC_API_KEY"])
                .get("ANTHROPIC_API_KEY")
                .map(String::as_str),
            Some("sk-ant-from-shell-0000-wxyz")
        );
        // Not set there, or not a provider key Cua reads: refused.
        assert!(
            app_call(
                &keys,
                "agent_keys.from_shell",
                serde_json::json!({"env": "OPENAI_API_KEY"}),
                &shell
            )
            .unwrap()
            .is_err()
        );
        assert!(
            app_call(
                &keys,
                "agent_keys.from_shell",
                serde_json::json!({"env": "HOME"}),
                &shell
            )
            .unwrap()
            .is_err()
        );
        assert_eq!(shell_value(&shell, "PATH"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_methods_answer_the_report_without_values() {
        let (keys, dir) = temp_keys();
        let r = app_call(
            &keys,
            "agent_keys.set",
            serde_json::json!({"provider": "openai", "value": "sk-test-0000-1234"}),
            std::path::Path::new("/nonexistent-shell"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(r["keys"].as_array().unwrap().len(), 1);
        assert!(!serde_json::to_string(&r).unwrap().contains("sk-test-0000"));
        // A malformed call does not echo what it was given.
        let e = app_call(
            &keys,
            "agent_keys.set",
            serde_json::json!({"provider": "openai", "value": 12345678}),
            std::path::Path::new("/nonexistent-shell"),
        )
        .unwrap()
        .unwrap_err();
        assert!(!e.to_string().contains("12345678"), "{e}");
        let r = app_call(
            &keys,
            "agent_keys.remove",
            serde_json::json!({"env": "OPENAI_API_KEY"}),
            std::path::Path::new("/nonexistent-shell"),
        )
        .unwrap()
        .unwrap();
        assert!(r["keys"].as_array().unwrap().is_empty());
        assert!(
            app_call(
                &keys,
                "agent_start",
                serde_json::json!({}),
                std::path::Path::new("/nonexistent-shell")
            )
            .is_none()
        );
        for m in APP_METHODS {
            assert!(
                cua_spaces_contract::tool(m).is_none(),
                "{m} is a contract tool"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the freshness check on a copy of Claude Code's own sign-in

    const HOUR: u64 = 3_600_000;

    fn login(access: &str, expires_ms: u64, email: &str) -> claude_login::Lookup {
        claude_login::Lookup::Found(claude_login::Found {
            source: claude_login::Source::File(std::path::PathBuf::from(
                "/fixture/.credentials.json",
            )),
            oauth: claude_login::Oauth {
                access_token: access.into(),
                has_refresh_token: true,
                expires_ms: Some(expires_ms),
                scopes: vec![],
                subscription_type: Some("max".into()),
            },
            account: claude_login::Account {
                email: Some(email.into()),
                organization: None,
            },
        })
    }

    /// A fake Claude Code: what its sign-in reads as now, what it reads as after it
    /// is asked to refresh, and how often it was read and asked.
    struct FakeClaude {
        now: Mutex<claude_login::Lookup>,
        after_ask: Option<claude_login::Lookup>,
        reads: Mutex<u32>,
        asks: Mutex<u32>,
    }
    impl FakeClaude {
        fn new(now: claude_login::Lookup, after_ask: Option<claude_login::Lookup>) -> Self {
            Self {
                now: Mutex::new(now),
                after_ask,
                reads: Mutex::new(0),
                asks: Mutex::new(0),
            }
        }
    }
    impl ClaudeLogin for FakeClaude {
        fn lookup(&self) -> claude_login::Lookup {
            *self.reads.lock().unwrap() += 1;
            self.now.lock().unwrap().clone()
        }
        fn ask_to_refresh(&self) -> bool {
            *self.asks.lock().unwrap() += 1;
            match &self.after_ask {
                Some(l) => {
                    *self.now.lock().unwrap() = l.clone();
                    true
                }
                None => false,
            }
        }
    }

    fn import_login(keys: &AgentKeys, token: &str, expires_ms: u64) {
        keys.import(
            "claude-code",
            CLAUDE_ENV,
            "Signed in as ada@example.com (Claude Max)",
            token,
            Some(expires_ms),
            Some("claude-login-file"),
        )
        .unwrap();
    }

    fn token_of(keys: &AgentKeys) -> String {
        keys.values(&[CLAUDE_ENV])
            .remove(CLAUDE_ENV)
            .unwrap_or_default()
    }

    #[test]
    fn a_copy_far_from_expiry_is_delivered_without_reading_claude_code() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-one", now_ms() + 5 * HOUR);
        let claude = FakeClaude::new(claude_login::Lookup::NotSignedIn, None);
        assert_eq!(keys.sync_claude_login(&claude, true), LoginSync::Fresh);
        assert_eq!(
            *claude.reads.lock().unwrap(),
            0,
            "no Keychain read when nothing is needed"
        );
        assert_eq!(*claude.asks.lock().unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_copies_made_from_claude_codes_own_sign_in_are_synced() {
        let (keys, dir) = temp_keys();
        let claude = FakeClaude::new(
            login("sk-ant-oat01-new", now_ms() + 8 * HOUR, "a@b.c"),
            None,
        );
        assert_eq!(
            keys.sync_claude_login(&claude, true),
            LoginSync::NotApplicable
        );
        // A pasted token (a year, no refresh) and an API key are never replaced.
        keys.import(
            "claude-code",
            CLAUDE_ENV,
            "Claude token you pasted",
            "sk-ant-oat01-pasted",
            Some(now_ms() - 1),
            Some("token"),
        )
        .unwrap();
        assert_eq!(
            keys.sync_claude_login(&claude, true),
            LoginSync::NotApplicable
        );
        assert_eq!(*claude.reads.lock().unwrap(), 0);
        assert_eq!(token_of(&keys), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_about_to_run_out_is_replaced_by_what_claude_code_has_refreshed() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-old", now_ms() + HOUR / 4);
        let claude = FakeClaude::new(
            login("sk-ant-oat01-new", now_ms() + 8 * HOUR, "ada@example.com"),
            None,
        );
        assert_eq!(keys.sync_claude_login(&claude, true), LoginSync::Updated);
        assert_eq!(token_of(&keys), "sk-ant-oat01-new");
        let listed = keys.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].expired);
        assert_eq!(
            listed[0].label.as_deref(),
            Some("Signed in as ada@example.com (Claude Max)")
        );
        assert_eq!(
            *claude.asks.lock().unwrap(),
            0,
            "no need to ask Claude Code: it had refreshed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_out_copy_asks_claude_code_to_refresh_once_and_reads_again() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-old", now_ms() - 1000);
        assert!(keys.list().unwrap()[0].expired);
        // Claude Code has the same run-out token until asked; then it has a new one.
        let claude = FakeClaude::new(
            login("sk-ant-oat01-old", now_ms() - 1000, "ada@example.com"),
            Some(login(
                "sk-ant-oat01-fresh",
                now_ms() + 8 * HOUR,
                "ada@example.com",
            )),
        );
        assert_eq!(keys.sync_claude_login(&claude, true), LoginSync::Updated);
        assert_eq!(*claude.asks.lock().unwrap(), 1);
        assert_eq!(token_of(&keys), "sk-ant-oat01-fresh");
        assert!(!keys.list().unwrap()[0].expired);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_reads_again_but_never_asks_claude_code_to_refresh() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-old", now_ms() - 1000);
        let claude = FakeClaude::new(
            login("sk-ant-oat01-old", now_ms() - 1000, "a@b.c"),
            Some(login("sk-ant-oat01-fresh", now_ms() + HOUR, "a@b.c")),
        );
        assert_eq!(keys.sync_claude_login(&claude, false), LoginSync::Stale);
        assert_eq!(*claude.asks.lock().unwrap(), 0);
        assert!(
            keys.list().unwrap()[0].expired,
            "honest: Needs reconnect until Claude Code has a new one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_newer_anywhere_is_needs_reconnect_and_signed_out_is_not_guessed_at() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-old", now_ms() - 1000);
        for gone in [
            claude_login::Lookup::NotSignedIn,
            claude_login::Lookup::Denied,
        ] {
            let (k2, d2) = temp_keys();
            import_login(&k2, "sk-ant-oat01-old", now_ms() - 1000);
            let claude = FakeClaude::new(gone, None);
            assert_eq!(k2.sync_claude_login(&claude, true), LoginSync::Stale);
            assert_eq!(token_of(&k2), "", "an expired copy is never delivered");
            let _ = std::fs::remove_dir_all(&d2);
        }
        // An older or equal token from Claude Code is not "newer".
        let claude = FakeClaude::new(login("sk-ant-oat01-old", now_ms() - 1000, "a@b.c"), None);
        assert_eq!(keys.sync_claude_login(&claude, true), LoginSync::Stale);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_code_is_read_at_most_every_twenty_seconds() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-old", now_ms() - 1000);
        let claude = FakeClaude::new(claude_login::Lookup::NotSignedIn, None);
        for _ in 0..5 {
            assert_eq!(keys.sync_claude_login(&claude, true), LoginSync::Stale);
        }
        assert_eq!(*claude.reads.lock().unwrap(), 1);
        assert_eq!(*claude.asks.lock().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_copy_recovers_when_claude_code_has_a_different_sign_in() {
        let (keys, dir) = temp_keys();
        import_login(&keys, "sk-ant-oat01-revoked", now_ms() + 5 * HOUR);
        keys.mark_rejected(&[CLAUDE_ENV]).unwrap();
        assert!(
            keys.list().unwrap()[0].expired,
            "refused reads as Needs reconnect"
        );
        // Claude Code still holds the same refused token: stays refused.
        let same = FakeClaude::new(
            login("sk-ant-oat01-revoked", now_ms() + 5 * HOUR, "a@b.c"),
            None,
        );
        assert_eq!(keys.sync_claude_login(&same, false), LoginSync::Stale);
        assert!(keys.list().unwrap()[0].expired);
        // The person signed in again in Claude Code: the new sign-in replaces it.
        let (keys2, dir2) = temp_keys();
        import_login(&keys2, "sk-ant-oat01-revoked", now_ms() + 5 * HOUR);
        keys2.mark_rejected(&[CLAUDE_ENV]).unwrap();
        let again = FakeClaude::new(
            login("sk-ant-oat01-again", now_ms() + 8 * HOUR, "a@b.c"),
            None,
        );
        assert_eq!(keys2.sync_claude_login(&again, false), LoginSync::Updated);
        assert!(!keys2.list().unwrap()[0].expired);
        assert_eq!(token_of(&keys2), "sk-ant-oat01-again");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn the_host_login_reads_nothing_in_a_test_process() {
        assert_eq!(HostClaudeLogin.lookup(), claude_login::Lookup::NotSignedIn);
        assert!(!HostClaudeLogin.ask_to_refresh());
    }
}
