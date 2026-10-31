//! Finding the sign-in Claude Code already has on this computer.
//!
//! Connect for Claude Code imports the login the person made with Claude
//! Code itself (the way cswap, ccswap and T3 Code work with it), instead of
//! asking for a new one. This module only knows where Claude Code keeps it
//! and how to read it; it never changes Claude Code's files or Keychain.
//!
//! Where the login lives (sources: Claude Code's own storage code as
//! mirrored by ccswap `internal/claudecode/creds.go` and cswap
//! `paths.py`/`credentials.py`; Hermes `agent/anthropic_credentials.py`):
//!
//! | What | Where |
//! |---|---|
//! | The tokens (`claudeAiOauth`: `accessToken`, `refreshToken`, `expiresAt`, `scopes`, `subscriptionType`) | macOS: Keychain item `Claude Code-credentials` (account = the login name; with `CLAUDE_CONFIG_DIR` the service is `Claude Code-credentials-<first 8 hex of sha256(dir)>`; `CLAUDE_SECURESTORAGE_CONFIG_DIR` overrides which dir is hashed). Linux and Windows, and the fallback on macOS: `<config dir>/.credentials.json` (config dir = `CLAUDE_CONFIG_DIR` or `~/.claude`). |
//! | Who it is (`oauthAccount`: `emailAddress`, `organizationName`) | the global config: `<config dir>/.config.json` when Claude Code (2.1.299 rule) still uses that legacy file, else `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`. |
//!
//! The global config holds who is signed in, never the tokens. When both a
//! Keychain item and a credentials file exist, the one with the later expiry
//! wins (Claude Code refreshes one source and not always the other).
//!
//! What Cua keeps: the access token only (never the refresh token), its
//! expiry, the account's email and the plan name. A refresh token works once
//! (each refresh returns a new pair and spends the old one), so a second
//! holder that refreshed would sign Claude Code out. Cua therefore never
//! refreshes: it re-reads this login when its copy runs out
//! ([`crate::signin`], `cua-spaces` agent keys).

use crate::{HostEnv, Os};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The Keychain service Claude Code keeps its login under (no config dir).
pub const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// An access token's shortest and longest believable length. Real ones are
/// about 108 characters (`sk-ant-oat01-` and 95 more).
const TOKEN_MIN: usize = 40;
const TOKEN_MAX: usize = 400;

/// What a Keychain read answered.
#[derive(Clone, PartialEq, Eq)]
pub enum KeychainRead {
    /// The item's secret.
    Found(String),
    /// No such item.
    NotFound,
    /// The person (or the system) refused access.
    Denied,
    /// The Keychain can't be used here (not macOS, a test process, an error).
    Unavailable,
}

impl std::fmt::Debug for KeychainRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeychainRead::Found(_) => f.write_str("Found(<redacted>)"),
            KeychainRead::NotFound => f.write_str("NotFound"),
            KeychainRead::Denied => f.write_str("Denied"),
            KeychainRead::Unavailable => f.write_str("Unavailable"),
        }
    }
}

/// The macOS Keychain as this module uses it, so tests use a fake and never
/// the real one.
pub trait Keychain: Send + Sync {
    /// Whether an item exists, asked without reading its secret (no prompt).
    fn exists(&self, service: &str, account: &str) -> bool;
    /// The item's secret. The system asks the person to allow this the first
    /// time (that prompt is the consent). `account` `None`: any account.
    fn read(&self, service: &str, account: Option<&str>) -> KeychainRead;
}

/// No Keychain (Linux, Windows, hermetic environments).
pub struct NoKeychain;

impl Keychain for NoKeychain {
    fn exists(&self, _: &str, _: &str) -> bool {
        false
    }
    fn read(&self, _: &str, _: Option<&str>) -> KeychainRead {
        KeychainRead::Unavailable
    }
}

/// The real macOS Keychain, through `/usr/bin/security` (the tool Claude
/// Code itself and cswap use). Refuses in a test process.
pub struct MacSecurity;

impl MacSecurity {
    const BIN: &'static str = "/usr/bin/security";
}

impl Keychain for MacSecurity {
    fn exists(&self, service: &str, account: &str) -> bool {
        if cua_home::is_test_process() || !cfg!(target_os = "macos") {
            return false;
        }
        // Without `-w` or `-g` only the attributes print: no secret, no prompt.
        std::process::Command::new(Self::BIN)
            .args(["find-generic-password", "-s", service, "-a", account])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn read(&self, service: &str, account: Option<&str>) -> KeychainRead {
        if cua_home::is_test_process() || !cfg!(target_os = "macos") {
            return KeychainRead::Unavailable;
        }
        let mut cmd = std::process::Command::new(Self::BIN);
        cmd.args(["find-generic-password", "-s", service]);
        if let Some(a) = account {
            cmd.args(["-a", a]);
        }
        cmd.arg("-w")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let Ok(out) = cmd.output() else {
            return KeychainRead::Unavailable;
        };
        match out.status.code() {
            Some(0) => {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() {
                    KeychainRead::NotFound
                } else {
                    KeychainRead::Found(s)
                }
            }
            // 44: errSecItemNotFound.
            Some(44) => KeychainRead::NotFound,
            // 36: errSecAuthFailed / 51: user canceled / 128: errSecUserCanceled.
            Some(36) | Some(51) | Some(128) => KeychainRead::Denied,
            _ => {
                let err = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
                if err.contains("could not be found") {
                    KeychainRead::NotFound
                } else if err.contains("denied") || err.contains("cancel") {
                    KeychainRead::Denied
                } else {
                    KeychainRead::Unavailable
                }
            }
        }
    }
}

/// The Keychain this environment may use: the real one on a real Mac (agent
/// CLIs allowed), none in a hermetic environment.
pub fn default_keychain(env: &HostEnv) -> Box<dyn Keychain> {
    if env.os == Os::MacOs && env.run_agent_clis {
        Box::new(MacSecurity)
    } else {
        Box::new(NoKeychain)
    }
}

/// Where a login was found, in words a person recognises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Keychain { service: String },
    File(PathBuf),
}

impl Source {
    /// "the macOS Keychain item Claude Code-credentials", "~/.claude/.credentials.json".
    pub fn describe(&self, env: &HostEnv) -> String {
        match self {
            Source::Keychain { service } => {
                format!("the macOS Keychain item \u{201c}{service}\u{201d}")
            }
            Source::File(p) => match p.strip_prefix(&env.home) {
                Ok(rel) => format!("~/{}", rel.display()),
                Err(_) => p.display().to_string(),
            },
        }
    }

    /// A fixed word for telemetry: `keychain` or `file`.
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Keychain { .. } => "keychain",
            Source::File(_) => "file",
        }
    }
}

/// The login's tokens, as far as Cua keeps them. `Debug` never prints the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Oauth {
    /// The access token (`sk-ant-oat01-...`).
    pub access_token: String,
    /// There is a refresh token (Cua never keeps it).
    pub has_refresh_token: bool,
    /// When the access token stops working (Unix ms).
    pub expires_ms: Option<u64>,
    pub scopes: Vec<String>,
    /// `max`, `pro`, `team`, `enterprise`.
    pub subscription_type: Option<String>,
}

impl std::fmt::Debug for Oauth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oauth")
            .field("access_token", &"<redacted>")
            .field("has_refresh_token", &self.has_refresh_token)
            .field("expires_ms", &self.expires_ms)
            .field("subscription_type", &self.subscription_type)
            .finish()
    }
}

impl Oauth {
    /// Not usable any more.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.expires_ms.is_some_and(|e| e <= now_ms)
    }

    /// Less than `within_ms` left (or already gone).
    pub fn near_expiry(&self, now_ms: u64, within_ms: u64) -> bool {
        self.expires_ms.is_some_and(|e| e <= now_ms + within_ms)
    }
}

/// Who is signed in (from the global config's `oauthAccount`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
    pub organization: Option<String>,
}

/// A login found on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub source: Source,
    pub oauth: Oauth,
    pub account: Account,
}

impl Found {
    /// "Signed in as ada@example.com (Claude Max)": what the rows show.
    pub fn label(&self) -> String {
        label_for(
            self.account.email.as_deref(),
            self.oauth.subscription_type.as_deref(),
        )
    }
}

/// "Signed in as ada@example.com (Claude Max)", or without the parts not known.
pub fn label_for(email: Option<&str>, subscription_type: Option<&str>) -> String {
    let plan = subscription_type.and_then(plan_name);
    match (email.filter(|e| !e.is_empty()), plan) {
        (Some(e), Some(p)) => format!("Signed in as {e} ({p})"),
        (Some(e), None) => format!("Signed in as {e}"),
        (None, Some(p)) => format!("Signed in to {p}"),
        (None, None) => "Signed in to Claude".to_string(),
    }
}

/// "Claude Max" for `max`; None for a type this does not know to name.
pub fn plan_name(subscription_type: &str) -> Option<String> {
    let t = subscription_type.trim().to_ascii_lowercase();
    if t.is_empty()
        || t.len() > 32
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(match t.as_str() {
        "max" => "Claude Max".into(),
        "pro" => "Claude Pro".into(),
        "team" => "Claude Team".into(),
        "enterprise" => "Claude Enterprise".into(),
        "free" => "Claude Free".into(),
        other => {
            let mut c = other.chars();
            let first = c.next()?.to_ascii_uppercase();
            format!("Claude {first}{}", c.as_str())
        }
    })
}

/// What a lookup found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(Found),
    /// Claude Code isn't signed in here (nothing at any location).
    NotSignedIn,
    /// A login exists but the Keychain refused to hand it over.
    Denied,
    /// There was something, but it isn't a login Cua can use (the line says why).
    Unusable(String),
}

/// Why a credentials document wasn't taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Not JSON.
    NotJson,
    /// No `claudeAiOauth` (an API-key login, or signed out).
    NoOauth,
    /// No access token, or one that isn't shaped like Claude's.
    BadToken,
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Whether `t` is shaped like a Claude access or setup token
/// (`sk-ant-oat01-...`): the prefix, a believable length, token characters only.
pub fn token_shaped(t: &str) -> bool {
    t.starts_with("sk-ant-oat")
        && (TOKEN_MIN..=TOKEN_MAX).contains(&t.len())
        && t.chars().all(token_char)
}

fn ms(v: Option<&Value>) -> Option<u64> {
    let n = v?.as_u64().or_else(|| v?.as_f64().map(|f| f as u64))?;
    // Seconds, not milliseconds (before the year 5138).
    Some(if n < 100_000_000_000 { n * 1000 } else { n })
}

/// Reads a credentials document (`{"claudeAiOauth": {...}}`).
pub fn parse_credentials(text: &str) -> Result<Oauth, ParseError> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|_| ParseError::NotJson)?;
    let o = v
        .get("claudeAiOauth")
        .filter(|o| o.is_object())
        .ok_or(ParseError::NoOauth)?;
    let access = o
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !token_shaped(access) {
        return Err(if access.is_empty() {
            ParseError::NoOauth
        } else {
            ParseError::BadToken
        });
    }
    Ok(Oauth {
        access_token: access.to_string(),
        has_refresh_token: o
            .get("refreshToken")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty()),
        expires_ms: ms(o.get("expiresAt")),
        scopes: o
            .get("scopes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        subscription_type: o
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

// ------------------------------------------------------------ locations

/// Claude Code's config folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn config_dir(env: &HostEnv) -> PathBuf {
    env.var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude"))
}

/// The credentials file in the config folder.
pub fn credentials_file(env: &HostEnv) -> PathBuf {
    config_dir(env).join(".credentials.json")
}

/// The Keychain service for a config folder: the plain name for the default,
/// else it with the first 8 hex digits of the folder path's SHA-256.
pub fn keychain_service_for(dir: &str) -> String {
    let h = Sha256::digest(dir.as_bytes());
    format!("{KEYCHAIN_SERVICE}-{}", &hex::encode(h)[..8])
}

/// The Keychain services this environment's login may be under, in the order
/// to try. `CLAUDE_SECURESTORAGE_CONFIG_DIR`, when defined, names the only
/// one Claude Code reads (empty: the default). Else `CLAUDE_CONFIG_DIR`
/// names a hashed one, plus the default when it names the default folder.
pub fn keychain_services(env: &HostEnv) -> Vec<String> {
    if let Some(secure) = env.vars.get("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        return vec![if secure.is_empty() {
            KEYCHAIN_SERVICE.to_string()
        } else {
            keychain_service_for(secure)
        }];
    }
    let Some(dir) = env.var("CLAUDE_CONFIG_DIR") else {
        return vec![KEYCHAIN_SERVICE.to_string()];
    };
    let mut v = vec![keychain_service_for(dir)];
    if same_dir(Path::new(dir), &env.home.join(".claude")) {
        v.push(KEYCHAIN_SERVICE.to_string());
    }
    v
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// The Keychain account Claude Code uses: the login name, or `claude-code-user`
/// when that has characters outside letters, digits, `.`, `_`, `-`.
pub fn keychain_account(env: &HostEnv) -> String {
    let name = env
        .var("USER")
        .map(str::to_string)
        .or_else(|| {
            env.home
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        name
    } else {
        "claude-code-user".into()
    }
}

/// The global config file Claude Code uses (2.1.299's choice): the legacy
/// `<config dir>/.config.json` when it is a regular file that is non-empty
/// (or empty, writable and `.claude.json` is absent), else `.claude.json`
/// in `CLAUDE_CONFIG_DIR` or the home.
pub fn global_config(env: &HostEnv) -> PathBuf {
    let base = env
        .var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.clone());
    let global = base.join(".claude.json");
    let legacy = config_dir(env).join(".config.json");
    match std::fs::metadata(&legacy) {
        Ok(m)
            if m.is_file()
                && (m.len() > 0 || (!m.permissions().readonly() && !global.exists())) =>
        {
            legacy
        }
        _ => global,
    }
}

/// Who is signed in, from the global config's `oauthAccount`.
pub fn account(env: &HostEnv) -> Account {
    let Some(v) = std::fs::read_to_string(global_config(env))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Account::default();
    };
    let a = v.get("oauthAccount");
    let s = |k: &str| {
        a.and_then(|a| a.get(k))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 254 && !s.chars().any(char::is_control))
            .map(str::to_string)
    };
    Account {
        email: s("emailAddress"),
        organization: s("organizationName"),
    }
}

// -------------------------------------------------------------- refresh

/// The least time between two asks of Claude Code to refresh its sign-in.
const REFRESH_ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// Asks Claude Code itself to refresh its sign-in, by a one-line request
/// (`claude -p`), waiting up to `wait`. Claude Code refreshes when its access
/// token has run out and writes the new pair under its own lock, so its
/// refresh token stays the one Claude Code holds: Cua never spends it. Asked
/// at most once every five minutes. Whether it ended well; the caller reads
/// the sign-in again either way. Never in a hermetic environment.
pub fn ask_claude_to_refresh(env: &HostEnv, wait: std::time::Duration) -> bool {
    use std::sync::{Mutex, OnceLock};
    static LAST: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    if !env.run_agent_clis || cua_home::is_test_process() {
        return false;
    }
    let Some(spec) = crate::registry::find("claude-code") else {
        return false;
    };
    let Some(bin) = crate::connection::binary(env, spec) else {
        return false;
    };
    {
        let mut last = LAST
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < REFRESH_ASK_EVERY) {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    matches!(
        crate::connection::run_capture(
            env,
            &bin,
            &[
                "-p",
                "Reply with the word ok.",
                "--model",
                "haiku",
                "--max-turns",
                "1"
            ],
            wait,
        ),
        Some((Some(0), _))
    )
}

// --------------------------------------------------------------- lookup

/// What exists, read without a secret: whether a Claude Code login looks
/// present (so the consent can say where it would be read from).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    /// Where it would be read from, first choice first.
    pub sources: Vec<Source>,
    pub account: Account,
}

/// Where a login would be read from, without reading any secret (a Keychain
/// item's attributes and a file's existence only).
pub fn presence(env: &HostEnv, keychain: &dyn Keychain) -> Presence {
    let mut sources = vec![];
    let account_name = keychain_account(env);
    for service in keychain_services(env) {
        if keychain.exists(&service, &account_name) {
            sources.push(Source::Keychain { service });
        }
    }
    let file = credentials_file(env);
    if std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() > 2) {
        sources.push(Source::File(file));
    }
    Presence {
        sources,
        account: account(env),
    }
}

/// Reads the login: the Keychain item (macOS), then the credentials file; the
/// one that works longest wins. Reading the Keychain item is what makes the
/// system ask the person to allow it.
pub fn lookup(env: &HostEnv, keychain: &dyn Keychain, now_ms: u64) -> Lookup {
    let mut found: Vec<(Source, Oauth)> = vec![];
    let mut denied = false;
    let mut unusable: Option<String> = None;
    let account_name = keychain_account(env);

    for service in keychain_services(env) {
        // Claude Code writes the item under the login name; fall back to any
        // account for an item another tool wrote.
        let mut r = keychain.read(&service, Some(&account_name));
        if r == KeychainRead::NotFound {
            r = keychain.read(&service, None);
        }
        match r {
            KeychainRead::Found(text) => match parse_credentials(&text) {
                Ok(o) => found.push((Source::Keychain { service }, o)),
                Err(ParseError::NoOauth) => {}
                Err(_) => {
                    unusable =
                        Some("Claude Code's sign-in in the Keychain isn't one Cua can read.".into())
                }
            },
            KeychainRead::Denied => denied = true,
            KeychainRead::NotFound | KeychainRead::Unavailable => {}
        }
    }

    let file = credentials_file(env);
    if let Ok(text) = std::fs::read_to_string(&file) {
        match parse_credentials(&text) {
            Ok(o) => found.push((Source::File(file), o)),
            Err(ParseError::NoOauth) => {}
            Err(_) => {
                unusable = Some("Claude Code's credentials file isn't one Cua can read.".into())
            }
        }
    }

    // One that has not run out beats one that has; then the later expiry.
    found.sort_by_key(|(_, o)| (!o.expired(now_ms), o.expires_ms.unwrap_or(u64::MAX)));
    match found.pop() {
        Some((source, oauth)) => Lookup::Found(Found {
            source,
            oauth,
            account: account(env),
        }),
        None if denied => Lookup::Denied,
        None => match unusable {
            Some(line) => Lookup::Unusable(line),
            None => Lookup::NotSignedIn,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex};

    const NOW: u64 = 1_800_000_000_000;

    fn tok(n: u8) -> String {
        format!("sk-ant-oat01-{}", format!("{n:02x}").repeat(45))
    }

    fn creds(access: &str, expires: u64, sub: &str) -> String {
        serde_json::json!({"claudeAiOauth": {
            "accessToken": access, "refreshToken": "sk-ant-ort01-refresh-not-kept",
            "expiresAt": expires, "scopes": ["user:inference", "user:profile"],
            "subscriptionType": sub, "rateLimitTier": "default_claude_max_20x"
        }, "mcpOAuth": {"x": {}}})
        .to_string()
    }

    /// A fake Keychain: `(service, account)` to secret.
    #[derive(Default)]
    struct Fake {
        items: Mutex<BTreeMap<(String, String), String>>,
        deny: bool,
        reads: Mutex<Vec<String>>,
    }
    impl Fake {
        fn with(service: &str, account: &str, secret: &str) -> Self {
            let f = Fake::default();
            f.items
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), secret.into());
            f
        }
    }
    impl Keychain for Fake {
        fn exists(&self, s: &str, a: &str) -> bool {
            self.items
                .lock()
                .unwrap()
                .contains_key(&(s.into(), a.into()))
        }
        fn read(&self, s: &str, a: Option<&str>) -> KeychainRead {
            self.reads.lock().unwrap().push(s.into());
            if self.deny {
                return KeychainRead::Denied;
            }
            let items = self.items.lock().unwrap();
            items
                .iter()
                .find(|((svc, acct), _)| svc == s && a.is_none_or(|a| a == acct))
                .map(|(_, v)| KeychainRead::Found(v.clone()))
                .unwrap_or(KeychainRead::NotFound)
        }
    }

    fn env(dir: &tempfile::TempDir) -> HostEnv {
        let mut e = HostEnv::isolated(dir.path());
        e.vars.insert("USER".into(), "ada".into());
        e
    }

    #[test]
    fn parses_the_shape_claude_code_writes() {
        let o = parse_credentials(&creds(&tok(1), NOW + 3_600_000, "max")).unwrap();
        assert_eq!(o.access_token, tok(1));
        assert!(o.has_refresh_token);
        assert_eq!(o.expires_ms, Some(NOW + 3_600_000));
        assert_eq!(o.subscription_type.as_deref(), Some("max"));
        assert_eq!(o.scopes, ["user:inference", "user:profile"]);
        // Never prints the token.
        assert!(!format!("{o:?}").contains("sk-ant"));
    }

    #[test]
    fn refuses_what_is_not_a_login() {
        assert_eq!(parse_credentials("nope"), Err(ParseError::NotJson));
        assert_eq!(parse_credentials("{}"), Err(ParseError::NoOauth));
        assert_eq!(
            parse_credentials(r#"{"claudeAiOauth": null}"#),
            Err(ParseError::NoOauth)
        );
        assert_eq!(
            parse_credentials(&creds("sk-ant-oat01-short", NOW, "pro")),
            Err(ParseError::BadToken)
        );
        // An API key is not a login, and neither is a cut-off or joined token.
        assert_eq!(
            parse_credentials(&creds(
                &format!("sk-ant-api03-{}", "a".repeat(80)),
                NOW,
                "pro"
            )),
            Err(ParseError::BadToken)
        );
        assert!(!token_shaped(&format!("{}\n", tok(2)).replace('\n', " x")));
        assert!(!token_shaped(&"sk-ant-oat01-".repeat(40)[..450]));
        assert!(token_shaped(&tok(3)));
    }

    #[test]
    fn expiry_in_seconds_is_read_as_milliseconds() {
        let o = parse_credentials(&creds(&tok(1), 1_800_000_000, "pro")).unwrap();
        assert_eq!(o.expires_ms, Some(1_800_000_000_000));
        assert!(o.expired(NOW + 1));
        assert!(!o.expired(NOW - 1));
        assert!(o.near_expiry(NOW - 1000, 5000));
        assert!(!o.near_expiry(NOW - 10_000, 5000));
    }

    #[test]
    fn labels_say_who_and_which_plan() {
        assert_eq!(
            label_for(Some("ada@example.com"), Some("max")),
            "Signed in as ada@example.com (Claude Max)"
        );
        assert_eq!(
            label_for(Some("ada@example.com"), None),
            "Signed in as ada@example.com"
        );
        assert_eq!(label_for(None, Some("pro")), "Signed in to Claude Pro");
        assert_eq!(label_for(None, None), "Signed in to Claude");
        assert_eq!(
            plan_name("enterprise").as_deref(),
            Some("Claude Enterprise")
        );
        assert_eq!(plan_name("weird plan!"), None);
        for l in [
            label_for(Some("a@b.c"), Some("team")),
            label_for(None, None),
        ] {
            assert!(!l.contains('\u{b7}'), "no middle dot: {l}");
        }
    }

    #[test]
    fn keychain_service_names_follow_claude_codes_rules() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(&d);
        assert_eq!(keychain_services(&e), [KEYCHAIN_SERVICE]);
        // A custom folder is hashed (8 hex of sha256 of the path).
        e.vars
            .insert("CLAUDE_CONFIG_DIR".into(), "/work/claude-a".into());
        let s = keychain_services(&e);
        assert_eq!(s.len(), 1);
        assert!(
            s[0].starts_with("Claude Code-credentials-")
                && s[0].len() == KEYCHAIN_SERVICE.len() + 9
        );
        // The folder named the default one: the plain item is tried too.
        let def = d.path().join(".claude");
        std::fs::create_dir_all(&def).unwrap();
        e.vars
            .insert("CLAUDE_CONFIG_DIR".into(), def.display().to_string());
        let s = keychain_services(&e);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], KEYCHAIN_SERVICE);
        // The secure-storage override is the only store, and empty is the default.
        e.vars
            .insert("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), "".into());
        assert_eq!(keychain_services(&e), [KEYCHAIN_SERVICE]);
        e.vars
            .insert("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), "/x".into());
        assert_eq!(keychain_services(&e), [keychain_service_for("/x")]);
    }

    #[test]
    fn keychain_account_is_the_login_name_or_the_fallback() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(&d);
        assert_eq!(keychain_account(&e), "ada");
        e.vars.insert("USER".into(), "ada lovelace".into());
        assert_eq!(keychain_account(&e), "claude-code-user");
    }

    #[test]
    fn finds_the_login_in_the_keychain_with_the_account_from_config() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        std::fs::write(
            d.path().join(".claude.json"),
            r#"{"oauthAccount": {"emailAddress": "ada@example.com", "organizationName": "Analytical"}}"#,
        )
        .unwrap();
        let kc = Fake::with(
            KEYCHAIN_SERVICE,
            "ada",
            &creds(&tok(1), NOW + 7_200_000, "max"),
        );
        let Lookup::Found(f) = lookup(&e, &kc, NOW) else {
            panic!("found")
        };
        assert_eq!(
            f.source,
            Source::Keychain {
                service: KEYCHAIN_SERVICE.into()
            }
        );
        assert_eq!(f.account.email.as_deref(), Some("ada@example.com"));
        assert_eq!(f.label(), "Signed in as ada@example.com (Claude Max)");
        assert_eq!(f.source.kind(), "keychain");
        assert!(f.source.describe(&e).contains("Claude Code-credentials"));
    }

    #[test]
    fn finds_the_login_in_the_credentials_file_where_there_is_no_keychain() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        std::fs::create_dir_all(d.path().join(".claude")).unwrap();
        std::fs::write(
            d.path().join(".claude/.credentials.json"),
            creds(&tok(4), NOW + 1_000_000, "pro"),
        )
        .unwrap();
        let Lookup::Found(f) = lookup(&e, &NoKeychain, NOW) else {
            panic!("found")
        };
        assert_eq!(f.source.kind(), "file");
        assert_eq!(f.source.describe(&e), "~/.claude/.credentials.json");
        assert_eq!(f.label(), "Signed in to Claude Pro");
    }

    #[test]
    fn config_dir_override_moves_every_location() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(&d);
        let dir = d.path().join("work-claude");
        std::fs::create_dir_all(&dir).unwrap();
        e.vars
            .insert("CLAUDE_CONFIG_DIR".into(), dir.display().to_string());
        std::fs::write(
            dir.join(".credentials.json"),
            creds(&tok(5), NOW + 10, "team"),
        )
        .unwrap();
        std::fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"w@example.com"}}"#,
        )
        .unwrap();
        // The real home's files are never consulted.
        std::fs::write(
            d.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"home@example.com"}}"#,
        )
        .unwrap();
        let Lookup::Found(f) = lookup(&e, &NoKeychain, NOW) else {
            panic!("found")
        };
        assert_eq!(f.account.email.as_deref(), Some("w@example.com"));
        assert_eq!(f.source, Source::File(dir.join(".credentials.json")));
    }

    #[test]
    fn the_legacy_config_json_wins_when_claude_code_would_use_it() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        std::fs::create_dir_all(d.path().join(".claude")).unwrap();
        std::fs::write(
            d.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"new@example.com"}}"#,
        )
        .unwrap();
        assert_eq!(global_config(&e), d.path().join(".claude.json"));
        std::fs::write(
            d.path().join(".claude/.config.json"),
            r#"{"oauthAccount":{"emailAddress":"legacy@example.com"}}"#,
        )
        .unwrap();
        assert_eq!(global_config(&e), d.path().join(".claude/.config.json"));
        assert_eq!(account(&e).email.as_deref(), Some("legacy@example.com"));
    }

    #[test]
    fn the_later_expiry_wins_when_both_stores_have_a_login() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        std::fs::create_dir_all(d.path().join(".claude")).unwrap();
        std::fs::write(
            d.path().join(".claude/.credentials.json"),
            creds(&tok(6), NOW - 5000, "max"),
        )
        .unwrap();
        let kc = Fake::with(KEYCHAIN_SERVICE, "ada", &creds(&tok(7), NOW + 5000, "max"));
        let Lookup::Found(f) = lookup(&e, &kc, NOW) else {
            panic!("found")
        };
        assert_eq!(f.oauth.access_token, tok(7));
        // And the other way round.
        let kc = Fake::with(KEYCHAIN_SERVICE, "ada", &creds(&tok(7), NOW - 9000, "max"));
        std::fs::write(
            d.path().join(".claude/.credentials.json"),
            creds(&tok(6), NOW + 9000, "max"),
        )
        .unwrap();
        let Lookup::Found(f) = lookup(&e, &kc, NOW) else {
            panic!("found")
        };
        assert_eq!(f.oauth.access_token, tok(6));
    }

    #[test]
    fn nothing_and_denied_and_unreadable_are_told_apart() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        assert_eq!(lookup(&e, &NoKeychain, NOW), Lookup::NotSignedIn);
        let denied = Fake {
            deny: true,
            ..Default::default()
        };
        assert_eq!(lookup(&e, &denied, NOW), Lookup::Denied);
        // A file that is not credentials is reported, never read as a login.
        std::fs::create_dir_all(d.path().join(".claude")).unwrap();
        std::fs::write(d.path().join(".claude/.credentials.json"), "garbage").unwrap();
        assert!(matches!(lookup(&e, &NoKeychain, NOW), Lookup::Unusable(_)));
        // An API-key login (no claudeAiOauth) is not a sign-in to import.
        std::fs::write(d.path().join(".claude/.credentials.json"), "{}").unwrap();
        assert_eq!(lookup(&e, &NoKeychain, NOW), Lookup::NotSignedIn);
    }

    #[test]
    fn presence_reads_no_secret() {
        let d = tempfile::tempdir().unwrap();
        let e = env(&d);
        let kc = Fake::with(KEYCHAIN_SERVICE, "ada", &creds(&tok(1), NOW, "max"));
        let p = presence(&e, &kc);
        assert_eq!(
            p.sources,
            [Source::Keychain {
                service: KEYCHAIN_SERVICE.into()
            }]
        );
        assert!(
            kc.reads.lock().unwrap().is_empty(),
            "presence never reads a secret"
        );
        assert!(presence(&e, &NoKeychain).sources.is_empty());
    }

    #[test]
    fn the_real_keychain_is_never_used_in_a_test_process() {
        // `MacSecurity` refuses here whatever the host has.
        assert!(!MacSecurity.exists(KEYCHAIN_SERVICE, "anyone"));
        assert_eq!(
            MacSecurity.read(KEYCHAIN_SERVICE, None),
            KeychainRead::Unavailable
        );
    }
}
