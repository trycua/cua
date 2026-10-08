//! Provider keys for agent runs, kept on this machine (Cua Spaces →
//! Settings → Agents).
//!
//! The keys live in the OS credential vault through cua-auth's secret store
//! ([`cua_auth::Store::save_secret`]): one item, `agent-keys`, next to the
//! session (service `run.cua.ai`), with the same access list, so the app,
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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The store's secret name.
pub const SECRET_NAME: &str = "agent-keys";

/// The app's settings methods (`SpaceService.CallSpaceTool` names). They are
/// not contract tools: no MCP server lists or calls them, so an agent can
/// never read or change a key.
pub const APP_METHODS: &[&str] = &["agent_keys.list", "agent_keys.set", "agent_keys.remove"];

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
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Blob {
    #[serde(default)]
    keys: Vec<Stored>,
}

/// How long a read of the vault item is reused (the app and readiness
/// checks read often; the daemon is the one writer).
const CACHE_FOR: Duration = Duration::from_secs(30);

/// The stored agent keys of one credential store.
pub struct AgentKeys {
    store: Option<cua_auth::Store>,
    unavailable: Option<String>,
    // Serializes read-modify-write; holds the last read.
    cache: Mutex<Option<(Instant, Vec<Stored>)>>,
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
        Ok(self.read(&mut g, false)?.iter().map(Stored::info).collect())
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

    /// The names of the stored keys (readiness). A store that can't be read
    /// has none.
    pub fn names(&self) -> Vec<String> {
        let mut g = self.lock();
        match self.read(&mut g, false) {
            Ok(keys) => keys.into_iter().map(|k| k.env).collect(),
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
                .filter(|k| names.contains(&k.env.as_str()))
                .map(|k| (k.env, k.value))
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "agent keys: the store can't be read");
                BTreeMap::new()
            }
        }
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

/// Runs one of [`APP_METHODS`] against `keys`; every answer is the
/// [`KeysReport`] after the change. `None` for any other name.
pub fn app_call(
    keys: &AgentKeys,
    name: &str,
    args: serde_json::Value,
) -> Option<Result<KeysReport>> {
    let args = if args.is_null() {
        serde_json::json!({})
    } else {
        args
    };
    Some(match name {
        "agent_keys.list" => keys.report(),
        "agent_keys.set" => serde_json::from_value::<SetArgs>(args)
            .map_err(|e| Error::InvalidArgument(format!("agent_keys.set: {}", redact_serde(&e))))
            .and_then(|a| keys.set(&a.provider, a.env.as_deref(), &a.value))
            .and_then(|_| keys.report()),
        "agent_keys.remove" => serde_json::from_value::<RemoveArgs>(args)
            .map_err(|e| Error::InvalidArgument(format!("agent_keys.remove: {e}")))
            .and_then(|a| keys.remove(&a.env))
            .and_then(|_| keys.report()),
        _ => return None,
    })
}

/// [`app_call`] on this process's keys as a tool answer (the daemon's
/// `CallSpaceTool` and the SDK's embedded runtime). `None` for any name
/// that is not one of [`APP_METHODS`].
#[cfg(feature = "mcp")]
pub fn app_tool(name: &str, args: serde_json::Value) -> Option<crate::mcp::ToolOutcome> {
    app_call(global(), name, args).map(|r| match r {
        Ok(report) => crate::mcp::ToolOutcome::json(&report),
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

    #[test]
    fn app_methods_answer_the_report_without_values() {
        let (keys, dir) = temp_keys();
        let r = app_call(
            &keys,
            "agent_keys.set",
            serde_json::json!({"provider": "openai", "value": "sk-test-0000-1234"}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.keys.len(), 1);
        assert!(!serde_json::to_string(&r).unwrap().contains("sk-test-0000"));
        // A malformed call does not echo what it was given.
        let e = app_call(
            &keys,
            "agent_keys.set",
            serde_json::json!({"provider": "openai", "value": 12345678}),
        )
        .unwrap()
        .unwrap_err();
        assert!(!e.to_string().contains("12345678"), "{e}");
        let r = app_call(
            &keys,
            "agent_keys.remove",
            serde_json::json!({"env": "OPENAI_API_KEY"}),
        )
        .unwrap()
        .unwrap();
        assert!(r.keys.is_empty());
        assert!(app_call(&keys, "agent_start", serde_json::json!({})).is_none());
        for m in APP_METHODS {
            assert!(
                cua_spaces_contract::tool(m).is_none(),
                "{m} is a contract tool"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
