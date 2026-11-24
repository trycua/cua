//! The GitHub sign-in the host uses for an approved push or pull request.
//!
//! An agent in a Space holds no GitHub access: when it asks to push, the host
//! asks its owner and, once a person says yes, does the push itself. This
//! module decides *with which sign-in*, the same way for the desktop app, the
//! web app and the CLI. The sources are tried in this order, and the first
//! that has something wins:
//!
//! 1. a token saved on this computer (Settings, Connections, `GITHUB_TOKEN`,
//!    in the host-only key store);
//! 2. `GH_TOKEN` or `GITHUB_TOKEN` in the Cua daemon's environment
//!    (`GH_ENTERPRISE_TOKEN` with `GH_HOST` for GitHub Enterprise);
//! 3. the GitHub CLI, signed in with `gh auth login`: the token is read with
//!    `gh auth token --hostname H [--user U]` at the moment an approved
//!    action runs, held in memory for that call, and never written down by
//!    Cua;
//! 4. git's own sign-in (an SSH key or a credential helper), which the push
//!    itself uses when the repository has an SSH remote. That source gives no
//!    token, so it cannot open a pull request.
//!
//! The first three are [`CredentialSource`]s. Another source (the Cua GitHub
//! App's device flow, for example) is one more entry in [`default_chain`]:
//! nothing else here changes.
//!
//! A token is a [`Token`]: it prints as `(hidden)`, is never part of an
//! error, an audit record or a log line, and never goes into a Space (the
//! names in [`super::HOST_ONLY_KEYS`] are refused as `env_from_host`).
//! Scanning, which the Settings page does, never reads a token from `gh`.
//!
//! What a person chose is not secret: which `gh` account to use on a host
//! and whether a host is switched off. It is kept beside the keys.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::keys::AgentKeys;
use crate::error::{Error, Result};

/// The default host.
pub const DEFAULT_HOST: &str = "github.com";
/// The saved key (host only) that is a pasted token.
pub const SAVED_KEY: &str = "GITHUB_TOKEN";
/// The store name of what a person chose (accounts, switches).
pub const SETTINGS_NAME: &str = "github-connection";
/// The `gh` that has `auth status --json`.
const MIN_GH: (u32, u32) = (2, 81);
/// How long one `gh` call may take.
const GH_WAIT: Duration = Duration::from_secs(15);
/// How long the check of a token against its host may take.
const CHECK_WAIT: Duration = Duration::from_secs(8);
/// The app methods of this module (`SpaceService.CallSpaceTool` names; no MCP
/// tool lists them, so no agent can read or change the connection).
pub const APP_METHODS: &[&str] = &[
    "github.status",
    "github.rescan",
    "github.use",
    "github.enable",
];

/// Whether `name` is one of [`APP_METHODS`].
pub fn is_app_method(name: &str) -> bool {
    APP_METHODS.contains(&name)
}

/// A token. It never prints, serializes or compares as text: only
/// [`Token::expose`] gives it out, for the one call that uses it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(value: impl Into<String>) -> Token {
        Token(value.into().trim().to_string())
    }

    /// The token, for the call that uses it. Do not store, log or format it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `s` with this token (and its base64 form, which an HTTP basic header
    /// carries) replaced by `(hidden)`.
    pub fn scrub(&self, s: &str) -> String {
        use base64::Engine as _;
        if self.0.is_empty() {
            return s.to_string();
        }
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{}", self.0));
        s.replace(&b64, "(hidden)").replace(&self.0, "(hidden)")
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token((hidden))")
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // Overwrite before the memory is freed (best effort; the allocator may have copied).
        let n = self.0.len();
        self.0.clear();
        self.0.push_str(&"\0".repeat(n));
        self.0.clear();
    }
}

/// Which source gave the sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A token saved in Settings, Connections.
    Saved,
    /// A token in the daemon's environment.
    Env,
    /// The GitHub CLI.
    Gh,
    /// Git's own sign-in (SSH key or credential helper): no token.
    Git,
}

/// A sign-in that was found, with its token.
#[derive(Debug)]
pub struct Resolved {
    pub host: String,
    pub source: Source,
    /// The account's login when it is known without asking GitHub (`gh` says it).
    pub login: Option<String>,
    /// The variable, for an environment token.
    pub env_name: Option<String>,
    pub token: Token,
}

/// What the host tells a card about the sign-in that will be used. It holds
/// no token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub host: String,
    pub source: Source,
    pub login: Option<String>,
    pub env_name: Option<String>,
    /// The account in plain words ("ada, signed in with gh").
    pub account: String,
    /// A token source: the host can open the pull request.
    pub can_open_pr: bool,
}

/// Why no sign-in can be used, in the words the person is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The person switched this host off.
    Off(String),
    /// Nothing is signed in.
    Nothing(String),
    /// The `gh` account the person chose is no longer signed in.
    PickedGone { host: String, login: String },
    /// `gh` knows the account but would not give a token.
    GhRefused { host: String },
}

impl Unavailable {
    /// The sentence for the person.
    pub fn message(&self) -> String {
        match self {
            Unavailable::Off(h) => format!(
                "GitHub is turned off for {h} on this computer (Settings, Connections, GitHub); nothing was sent"
            ),
            Unavailable::Nothing(h) => nothing_line(h),
            Unavailable::PickedGone { host, login } => format!(
                "The gh account you chose for {host}, {login}, is no longer signed in: choose another in Settings, Connections, or run gh auth login"
            ),
            Unavailable::GhRefused { host } => format!(
                "gh is signed in on {host} but would not give a token: run gh auth login again"
            ),
        }
    }
}

impl From<Unavailable> for Error {
    fn from(u: Unavailable) -> Error {
        Error::InvalidArgument(u.message())
    }
}

/// "No GitHub sign-in found: run gh auth login or paste a token".
pub fn nothing_line(host: &str) -> String {
    if host == DEFAULT_HOST {
        "No GitHub sign-in found: run gh auth login or paste a token".into()
    } else {
        format!("No sign-in found for {host}: run gh auth login --hostname {host}")
    }
}

/* ---- The environment and the GitHub CLI, behind small traits so tests use fakes ----- */

/// Environment variables.
pub trait Env: Send + Sync {
    fn var(&self, name: &str) -> Option<String>;
}

/// This process's environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }
}

/// What a `gh` call came to.
#[derive(Debug, Default)]
pub struct GhOut {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The GitHub CLI.
pub trait Gh: Send + Sync {
    /// Whether a `gh` can be found.
    fn found(&self) -> bool;
    /// Runs `gh args` with no sign-in variables in its environment (so it
    /// answers for its own login) and its stdin closed. `None` when it cannot run.
    fn run(&self, args: &[&str]) -> Option<GhOut>;
}

/// The `gh` on this computer: `PATH`, then the places a service launched
/// outside a terminal does not have on its `PATH`.
pub struct SystemGh {
    path: Option<std::ffi::OsString>,
}

impl SystemGh {
    pub fn new() -> SystemGh {
        SystemGh { path: None }
    }

    /// A `gh` found only in `dirs` (a test's fake).
    pub fn in_dirs(dirs: &[&Path]) -> SystemGh {
        SystemGh {
            path: std::env::join_paths(dirs).ok(),
        }
    }

    fn binary(&self) -> Option<PathBuf> {
        let mut dirs: Vec<PathBuf> = match &self.path {
            Some(p) => std::env::split_paths(p).collect(),
            None => {
                let mut v: Vec<PathBuf> = std::env::var_os("PATH")
                    .map(|p| std::env::split_paths(&p).collect())
                    .unwrap_or_default();
                for extra in [
                    "/opt/homebrew/bin",
                    "/usr/local/bin",
                    "/usr/bin",
                    "/home/linuxbrew/.linuxbrew/bin",
                ] {
                    v.push(extra.into());
                }
                if let Some(h) = std::env::var_os("HOME") {
                    v.push(Path::new(&h).join(".local/bin"));
                }
                v
            }
        };
        dirs.dedup();
        let names: &[&str] = if cfg!(windows) {
            &["gh.exe", "gh.cmd"]
        } else {
            &["gh"]
        };
        dirs.iter()
            .flat_map(|d| names.iter().map(move |n| d.join(n)))
            .find(|p| p.is_file())
    }
}

impl Default for SystemGh {
    fn default() -> Self {
        Self::new()
    }
}

impl Gh for SystemGh {
    fn found(&self) -> bool {
        self.binary().is_some()
    }

    fn run(&self, args: &[&str]) -> Option<GhOut> {
        let bin = self.binary()?;
        let spawn = || {
            std::process::Command::new(&bin)
                .args(args)
                // gh answers for its own login, not for a token in this environment.
                .env_remove("GH_TOKEN")
                .env_remove("GITHUB_TOKEN")
                .env_remove("GH_ENTERPRISE_TOKEN")
                .env_remove("GITHUB_ENTERPRISE_TOKEN")
                .env("GH_PROMPT_DISABLED", "1")
                .env("GH_NO_UPDATE_NOTIFIER", "1")
                .env("GH_SPINNER_DISABLED", "1")
                .env("NO_COLOR", "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        };
        // A binary another thread of this process has only just written is briefly "text file
        // busy" (ETXTBSY, 26) while a concurrent fork still holds its write fd. Try again; it
        // clears in microseconds. Any other error means gh cannot run.
        let mut child = None;
        for attempt in 0..20 {
            match spawn() {
                Ok(c) => {
                    child = Some(c);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(26) && attempt < 19 => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => return None,
            }
        }
        let mut child = child?;
        let mut out = child.stdout.take()?;
        let mut err = child.stderr.take()?;
        let o = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = std::io::Read::read_to_end(&mut out, &mut s);
            String::from_utf8_lossy(&s).into_owned()
        });
        let e = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = std::io::Read::read_to_end(&mut err, &mut s);
            String::from_utf8_lossy(&s).into_owned()
        });
        let start = Instant::now();
        let ok = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st.success(),
                Ok(None) if start.elapsed() < GH_WAIT => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        Some(GhOut {
            ok,
            stdout: o.join().unwrap_or_default(),
            stderr: e.join().unwrap_or_default(),
        })
    }
}

/* ---- What `gh` knows -------------------------------------------------------------- */

/// One account `gh` is signed in with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GhAccount {
    pub login: String,
    /// The account `gh` uses for the host unless told otherwise.
    pub active: bool,
    /// GitHub accepts the sign-in (false: it needs `gh auth login` again).
    pub valid: bool,
    pub scopes: Vec<String>,
}

/// What `gh` is, on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GhInfo {
    pub found: bool,
    pub version: Option<String>,
    /// `gh` is older than the version that lists accounts as JSON (2.81).
    pub old: bool,
}

fn parse_version(out: &str) -> Option<(String, (u32, u32))> {
    let v = out.lines().next()?.split_whitespace().nth(2)?;
    let mut p = v.split('.');
    let major = p.next()?.parse().ok()?;
    let minor = p.next()?.parse().ok()?;
    Some((v.to_string(), (major, minor)))
}

fn split_scopes(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().trim_matches(['\'', '"']).trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// The accounts in `gh auth status --json hosts`, by host.
fn parse_status_json(text: &str) -> Option<BTreeMap<String, Vec<GhAccount>>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let hosts = v.get("hosts")?.as_object()?;
    let mut out = BTreeMap::new();
    for (host, list) in hosts {
        let accounts = list
            .as_array()?
            .iter()
            .filter_map(|a| {
                let login = a.get("login")?.as_str()?.to_string();
                if login.is_empty() {
                    return None;
                }
                Some(GhAccount {
                    login,
                    active: a.get("active").and_then(Value::as_bool).unwrap_or(false),
                    valid: a.get("state").and_then(Value::as_str) == Some("success"),
                    scopes: a
                        .get("scopes")
                        .and_then(Value::as_str)
                        .map(split_scopes)
                        .unwrap_or_default(),
                })
            })
            .collect();
        out.insert(host.clone(), accounts);
    }
    Some(out)
}

/// The accounts in the text `gh auth status` prints (a `gh` older than 2.81).
fn parse_status_text(text: &str) -> BTreeMap<String, Vec<GhAccount>> {
    let mut out: BTreeMap<String, Vec<GhAccount>> = BTreeMap::new();
    let mut current: Option<(String, usize)> = None;
    for raw in text.lines() {
        let l = raw
            .trim()
            .trim_start_matches(['\u{2713}', '-', 'X', '!', ' ']);
        if let Some(rest) = l.strip_prefix("Logged in to ") {
            let mut w = rest.split_whitespace();
            let host = w.next().unwrap_or_default().to_string();
            let login = match w.next() {
                Some("account") | Some("as") => w.next().unwrap_or_default().to_string(),
                _ => String::new(),
            };
            if host.is_empty() || login.is_empty() {
                current = None;
                continue;
            }
            let list = out.entry(host.clone()).or_default();
            list.push(GhAccount {
                login,
                active: true,
                valid: true,
                scopes: vec![],
            });
            current = Some((host, list.len() - 1));
        } else if let Some((host, i)) = &current {
            if let Some(v) = l.strip_prefix("Active account:") {
                out.get_mut(host).unwrap()[*i].active = v.trim() == "true";
            } else if let Some(v) = l.strip_prefix("Token scopes:") {
                out.get_mut(host).unwrap()[*i].scopes = split_scopes(v);
            }
        }
    }
    out
}

/* ---- What a person chose ---------------------------------------------------------- */

/// One host's switches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostChoice {
    /// Cua does not use any GitHub sign-in on this host.
    #[serde(default)]
    pub off: bool,
    /// The `gh` account to use; none: the one `gh` uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Choices {
    #[serde(default)]
    hosts: BTreeMap<String, HostChoice>,
}

/* ---- The chain -------------------------------------------------------------------- */

/// What one source says about a host.
pub enum Offer {
    /// Nothing from this source.
    None,
    /// A sign-in, without its token (a plan or a scan).
    Found {
        source: Source,
        login: Option<String>,
        env_name: Option<String>,
    },
    /// This source is the one to use but cannot be used.
    Blocked(Unavailable),
}

/// The pieces a source reads.
pub struct Ctx<'a> {
    pub keys: &'a AgentKeys,
    pub gh: &'a dyn Gh,
    pub env: &'a dyn Env,
    pub choice: HostChoice,
    pub host: &'a str,
    /// What `gh` knows about `host` (read once per resolution, token-free).
    pub gh_accounts: &'a [GhAccount],
}

/// One place a GitHub sign-in can come from. The chain tries them in order.
pub trait CredentialSource: Send + Sync {
    fn source(&self) -> Source;
    /// Whether this source has a sign-in for the host, without reading a token.
    fn offer(&self, cx: &Ctx<'_>) -> Offer;
    /// The token, read now (only for an approved action).
    fn token(&self, cx: &Ctx<'_>) -> std::result::Result<Resolved, Unavailable>;
}

/// The sources in the order they are tried. A future source (the Cua GitHub
/// App's device flow) is added here.
pub fn default_chain() -> Vec<Box<dyn CredentialSource>> {
    vec![Box::new(SavedToken), Box::new(EnvToken), Box::new(GhCli)]
}

struct SavedToken;

impl CredentialSource for SavedToken {
    fn source(&self) -> Source {
        Source::Saved
    }
    fn offer(&self, cx: &Ctx<'_>) -> Offer {
        // A pasted token is for github.com; Enterprise hosts sign in with gh or GH_ENTERPRISE_TOKEN.
        if cx.host == DEFAULT_HOST && cx.keys.names().iter().any(|n| n == SAVED_KEY) {
            Offer::Found {
                source: Source::Saved,
                login: None,
                env_name: None,
            }
        } else {
            Offer::None
        }
    }
    fn token(&self, cx: &Ctx<'_>) -> std::result::Result<Resolved, Unavailable> {
        let t = cx
            .keys
            .values(&[SAVED_KEY])
            .remove(SAVED_KEY)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| Unavailable::Nothing(cx.host.into()))?;
        Ok(Resolved {
            host: cx.host.into(),
            source: Source::Saved,
            login: None,
            env_name: None,
            token: Token::new(t),
        })
    }
}

/// The variable that holds a token for `host` in the daemon's environment.
fn env_token_name(env: &dyn Env, host: &str) -> Option<&'static str> {
    if host == DEFAULT_HOST {
        ["GH_TOKEN", "GITHUB_TOKEN"]
            .into_iter()
            .find(|n| env.var(n).is_some())
    } else if env.var("GH_HOST").as_deref() == Some(host) {
        ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
            .into_iter()
            .find(|n| env.var(n).is_some())
    } else {
        None
    }
}

struct EnvToken;

impl CredentialSource for EnvToken {
    fn source(&self) -> Source {
        Source::Env
    }
    fn offer(&self, cx: &Ctx<'_>) -> Offer {
        match env_token_name(cx.env, cx.host) {
            Some(n) => Offer::Found {
                source: Source::Env,
                login: None,
                env_name: Some(n.into()),
            },
            None => Offer::None,
        }
    }
    fn token(&self, cx: &Ctx<'_>) -> std::result::Result<Resolved, Unavailable> {
        let n =
            env_token_name(cx.env, cx.host).ok_or_else(|| Unavailable::Nothing(cx.host.into()))?;
        Ok(Resolved {
            host: cx.host.into(),
            source: Source::Env,
            login: None,
            env_name: Some(n.into()),
            token: Token::new(cx.env.var(n).unwrap_or_default()),
        })
    }
}

/// The `gh` account to use on a host: the one the person chose, else the one
/// `gh` uses. `Err` carries the chosen login that is gone.
fn pick<'a>(cx: &Ctx<'a>) -> std::result::Result<Option<&'a GhAccount>, String> {
    if let Some(want) = &cx.choice.account {
        return cx
            .gh_accounts
            .iter()
            .find(|a| &a.login == want)
            .map(Some)
            .ok_or_else(|| want.clone());
    }
    Ok(cx
        .gh_accounts
        .iter()
        .find(|a| a.active)
        .or_else(|| cx.gh_accounts.first()))
}

struct GhCli;

impl CredentialSource for GhCli {
    fn source(&self) -> Source {
        Source::Gh
    }
    fn offer(&self, cx: &Ctx<'_>) -> Offer {
        match pick(cx) {
            Err(login) => Offer::Blocked(Unavailable::PickedGone {
                host: cx.host.into(),
                login,
            }),
            Ok(None) => Offer::None,
            Ok(Some(a)) if !a.valid => Offer::Blocked(Unavailable::GhRefused {
                host: cx.host.into(),
            }),
            Ok(Some(a)) => Offer::Found {
                source: Source::Gh,
                login: Some(a.login.clone()),
                env_name: None,
            },
        }
    }
    fn token(&self, cx: &Ctx<'_>) -> std::result::Result<Resolved, Unavailable> {
        let a = match pick(cx) {
            Err(login) => {
                return Err(Unavailable::PickedGone {
                    host: cx.host.into(),
                    login,
                });
            }
            Ok(None) => return Err(Unavailable::Nothing(cx.host.into())),
            Ok(Some(a)) => a,
        };
        let refused = || Unavailable::GhRefused {
            host: cx.host.into(),
        };
        let mut args = vec!["auth", "token", "--hostname", cx.host];
        // The account is named whenever it is not the one gh uses itself.
        if !a.active || cx.choice.account.is_some() {
            args.extend(["--user", a.login.as_str()]);
        }
        let out = cx.gh.run(&args).ok_or_else(refused)?;
        let t = out.stdout.trim().to_string();
        if !out.ok || t.is_empty() {
            return Err(refused());
        }
        Ok(Resolved {
            host: cx.host.into(),
            source: Source::Gh,
            login: Some(a.login.clone()),
            env_name: None,
            token: Token::new(t),
        })
    }
}

/* ---- The report the apps draw ------------------------------------------------------ */

/// What one source holds for a host, for the page's list (in chain order).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceRow {
    /// `saved`, `env`, `gh` or `git`.
    pub id: Source,
    /// This source is the one that will be used.
    pub used: bool,
    /// It has something for the host.
    pub present: bool,
    /// Plain words: "A saved token", "GH_TOKEN in the Cua daemon's environment", ...
    pub line: String,
}

/// An account in the picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccountRow {
    pub login: String,
    /// `gh`'s own active account for the host.
    pub active: bool,
    /// The one that will be used.
    pub selected: bool,
    /// The person chose it (not just `gh`'s active one).
    pub chosen: bool,
    pub valid: bool,
    pub scopes: Vec<String>,
}

/// One host in the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostReport {
    pub host: String,
    /// Switched on (false: the person turned this host off).
    pub enabled: bool,
    /// The source in use (none when nothing is signed in, or the host is off).
    pub source: Option<Source>,
    pub login: Option<String>,
    pub env_name: Option<String>,
    /// The status in plain words ("Signed in as ada with gh").
    pub line: String,
    /// Something is wrong that the person can fix (a chosen account gone, gh refused).
    pub problem: bool,
    /// Why the token is broader than push and pull requests need, in plain words.
    pub warning: Option<String>,
    pub accounts: Vec<AccountRow>,
    pub sources: Vec<SourceRow>,
}

/// The whole report (`github.status`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub gh: GhInfo,
    pub hosts: Vec<HostReport>,
    /// What a person chose can be kept on this computer.
    pub can_save: bool,
    /// An older answer is shown until the page asks again (`github.rescan`).
    pub scanned_ms: u64,
}

/// A token's account and scopes, asked of its host on a rescan.
#[derive(Clone, Debug, Default)]
struct Identity {
    login: Option<String>,
    scopes: Option<Vec<String>>,
    /// The host refused the token.
    refused: bool,
}

/// The scopes a token has beyond what a push and a pull request need, in
/// plain words ("change GitHub Actions workflows"). Nothing for a token that
/// lists no scopes (a fine-grained token or an app token).
pub fn broader_scopes(scopes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in scopes {
        let what = match s.as_str() {
            "workflow" => "change GitHub Actions workflows",
            "delete_repo" => "delete repositories",
            "site_admin" => "administer the GitHub site",
            x if x.starts_with("admin:") => "administer organizations and settings",
            x if x.starts_with("write:") && x != "write:discussion" => {
                "write to organizations and packages"
            }
            x if x.starts_with("delete:") => "delete packages",
            _ => continue,
        };
        if !out.iter().any(|o| o == what) {
            out.push(what.to_string());
        }
    }
    out
}

fn warning_for(scopes: &[String]) -> Option<String> {
    let b = broader_scopes(scopes);
    if b.is_empty() {
        return None;
    }
    Some(format!(
        "This sign-in can also {}. Pushing and opening pull requests only needs the repo scope: use a narrower token, or sign in to gh again with only that.",
        join_words(&b)
    ))
}

fn join_words(v: &[String]) -> String {
    match v {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The REST API of a host.
pub fn api_base(host: &str) -> String {
    if host == DEFAULT_HOST {
        "https://api.github.com".into()
    } else {
        format!("https://{host}/api/v3")
    }
}

/// A host name that is safe to put in a URL and a command line.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.starts_with(['.', '-'])
        && !h.ends_with(['.', '-'])
        && !h.contains("..")
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
}

fn valid_login(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 100
        && l.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_[]".contains(c))
}

/// The GitHub connection of this computer.
pub struct GithubAuth<'k> {
    keys: &'k AgentKeys,
    gh: Box<dyn Gh>,
    env: Box<dyn Env>,
    chain: Vec<Box<dyn CredentialSource>>,
    /// Where the host's API is, when it is not GitHub's (a test's fake).
    api_override: Option<String>,
    /// What a person chose, when it cannot be kept in the store.
    mem: Mutex<Option<Choices>>,
    /// Answers of the last rescan (in memory only).
    identity: Mutex<BTreeMap<String, Identity>>,
    scanned_ms: Mutex<u64>,
}

impl<'k> GithubAuth<'k> {
    pub fn new(keys: &'k AgentKeys, gh: Box<dyn Gh>, env: Box<dyn Env>) -> Self {
        GithubAuth {
            keys,
            gh,
            env,
            chain: default_chain(),
            api_override: None,
            mem: Mutex::new(None),
            identity: Mutex::new(BTreeMap::new()),
            scanned_ms: Mutex::new(0),
        }
    }

    /// Asks `base` instead of the host's API (a test's fake).
    pub fn with_api(mut self, base: &str) -> Self {
        self.api_override = Some(base.trim_end_matches('/').into());
        self
    }

    fn choices(&self) -> Choices {
        if let Some(c) = self.mem.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return c;
        }
        self.keys
            .aux_load(SETTINGS_NAME)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_choices(&self, c: Choices) -> Result<()> {
        let empty = c.hosts.values().all(|h| *h == HostChoice::default());
        if self.keys.available() {
            if empty {
                self.keys.aux_clear(SETTINGS_NAME)?;
            } else {
                self.keys
                    .aux_save(SETTINGS_NAME, &serde_json::to_vec(&c)?)?;
            }
            *self.mem.lock().unwrap_or_else(|p| p.into_inner()) = None;
        } else {
            *self.mem.lock().unwrap_or_else(|p| p.into_inner()) = Some(c);
        }
        Ok(())
    }

    fn gh_info(&self) -> GhInfo {
        if !self.gh.found() {
            return GhInfo::default();
        }
        let v = self
            .gh
            .run(&["--version"])
            .and_then(|o| parse_version(&o.stdout));
        GhInfo {
            found: true,
            old: v.as_ref().is_some_and(|(_, v)| *v < MIN_GH),
            version: v.map(|(s, _)| s),
        }
    }

    /// The accounts `gh` is signed in with, by host (no token is read).
    fn gh_accounts(&self, info: &GhInfo, only: Option<&str>) -> BTreeMap<String, Vec<GhAccount>> {
        if !info.found {
            return BTreeMap::new();
        }
        let mut args = vec!["auth", "status"];
        if !info.old {
            args.extend(["--json", "hosts"]);
        }
        if let Some(h) = only {
            args.extend(["--hostname", h]);
        }
        let Some(out) = self.gh.run(&args) else {
            return BTreeMap::new();
        };
        // An old gh prints the status on stderr and exits 1 when any account has a problem.
        let text = if out.stdout.trim().is_empty() {
            out.stderr
        } else {
            out.stdout
        };
        if info.old {
            parse_status_text(&text)
        } else {
            parse_status_json(&text).unwrap_or_default()
        }
    }

    fn walk(
        &self,
        host: &str,
        choice: &HostChoice,
        accounts: &[GhAccount],
    ) -> (Option<Offer>, Vec<SourceRow>) {
        let cx = Ctx {
            keys: self.keys,
            gh: self.gh.as_ref(),
            env: self.env.as_ref(),
            choice: choice.clone(),
            host,
            gh_accounts: accounts,
        };
        let mut winner: Option<Offer> = None;
        let mut rows = Vec::new();
        for s in &self.chain {
            let o = s.offer(&cx);
            let present = !matches!(o, Offer::None);
            let used = present && winner.is_none();
            rows.push(SourceRow {
                id: s.source(),
                used,
                present,
                line: source_line(s.source(), &o, host),
            });
            if used {
                winner = Some(o);
            }
        }
        (winner, rows)
    }

    /// The sign-in that would be used for `host`, without reading a token (for a card).
    pub fn plan(&self, host: &str) -> std::result::Result<Plan, Unavailable> {
        let choice = self.choices().hosts.remove(host).unwrap_or_default();
        if choice.off {
            return Err(Unavailable::Off(host.into()));
        }
        let info = self.gh_info();
        let accounts = self
            .gh_accounts(&info, Some(host))
            .remove(host)
            .unwrap_or_default();
        let (winner, _) = self.walk(host, &choice, &accounts);
        match winner {
            Some(Offer::Found {
                source,
                login,
                env_name,
            }) => Ok(Plan {
                host: host.into(),
                source,
                account: account_words(source, login.as_deref(), env_name.as_deref(), host),
                login,
                env_name,
                can_open_pr: true,
            }),
            Some(Offer::Blocked(u)) => Err(u),
            _ => Err(Unavailable::Nothing(host.into())),
        }
    }

    /// The token for `host`, read now. Call it when an approved action runs, use the
    /// token for that call and drop it: nothing here keeps it.
    pub fn token(&self, host: &str) -> std::result::Result<Resolved, Unavailable> {
        let choice = self.choices().hosts.remove(host).unwrap_or_default();
        if choice.off {
            return Err(Unavailable::Off(host.into()));
        }
        let info = self.gh_info();
        let accounts = self
            .gh_accounts(&info, Some(host))
            .remove(host)
            .unwrap_or_default();
        let cx = Ctx {
            keys: self.keys,
            gh: self.gh.as_ref(),
            env: self.env.as_ref(),
            choice,
            host,
            gh_accounts: &accounts,
        };
        for s in &self.chain {
            match s.offer(&cx) {
                Offer::None => continue,
                Offer::Blocked(u) => return Err(u),
                Offer::Found { .. } => return s.token(&cx),
            }
        }
        Err(Unavailable::Nothing(host.into()))
    }

    /// Whether the person switched `host` off.
    pub fn is_off(&self, host: &str) -> bool {
        self.choices().hosts.get(host).is_some_and(|h| h.off)
    }

    /// The report of what is signed in on this computer (no network, no token).
    pub fn status(&self) -> Report {
        let info = self.gh_info();
        let by_host = self.gh_accounts(&info, None);
        self.report(info, by_host)
    }

    fn report(&self, gh: GhInfo, by_host: BTreeMap<String, Vec<GhAccount>>) -> Report {
        let choices = self.choices();
        let ident = self
            .identity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut hosts: Vec<String> = vec![DEFAULT_HOST.into()];
        for h in by_host.keys().chain(choices.hosts.keys()) {
            if !hosts.contains(h) && valid_host(h) {
                hosts.push(h.clone());
            }
        }
        if let Some(h) = self.env.var("GH_HOST").filter(|h| valid_host(h))
            && env_token_name(self.env.as_ref(), &h).is_some()
            && !hosts.contains(&h)
        {
            hosts.push(h);
        }
        hosts[1..].sort();
        let hosts = hosts
            .into_iter()
            .map(|host| {
                let choice = choices.hosts.get(&host).cloned().unwrap_or_default();
                let accounts = by_host.get(&host).cloned().unwrap_or_default();
                let (winner, sources) = self.walk(&host, &choice, &accounts);
                let (source, login, env_name, problem) = match &winner {
                    Some(Offer::Found {
                        source,
                        login,
                        env_name,
                    }) => (Some(*source), login.clone(), env_name.clone(), false),
                    Some(Offer::Blocked(_)) => (None, None, None, true),
                    _ => (None, None, None, false),
                };
                let id = ident.get(&host);
                // A token's account is known after a rescan asked its host.
                let login = login.or_else(|| {
                    matches!(source, Some(Source::Saved | Source::Env))
                        .then(|| id.and_then(|i| i.login.clone()))
                        .flatten()
                });
                let enabled = !choice.off;
                let line = if !enabled {
                    "Turned off: Cua will not use GitHub on this host".to_string()
                } else {
                    match &winner {
                        Some(Offer::Blocked(u)) => u.message(),
                        Some(Offer::Found {
                            source, env_name, ..
                        }) => {
                            let refused = matches!(source, Source::Saved | Source::Env)
                                && id.is_some_and(|i| i.refused);
                            if refused {
                                format!(
                                    "{} was refused by {host}: replace it or sign in with gh",
                                    match source {
                                        Source::Saved => "The saved token".to_string(),
                                        _ => env_name.clone().unwrap_or_default(),
                                    }
                                )
                            } else {
                                signed_in_line(*source, login.as_deref(), env_name.as_deref())
                            }
                        }
                        _ => nothing_line(&host),
                    }
                };
                let warning = if !enabled {
                    None
                } else {
                    match source {
                        Some(Source::Gh) => accounts
                            .iter()
                            .find(|a| Some(&a.login) == login.as_ref())
                            .and_then(|a| warning_for(&a.scopes)),
                        Some(Source::Saved | Source::Env) => {
                            id.and_then(|i| i.scopes.as_deref()).and_then(warning_for)
                        }
                        _ => None,
                    }
                };
                let selected = login.clone();
                HostReport {
                    enabled,
                    source: if enabled { source } else { None },
                    login: if enabled { login } else { None },
                    env_name,
                    line,
                    problem: enabled && (problem || id.is_some_and(|i| i.refused)),
                    warning,
                    accounts: accounts
                        .iter()
                        .map(|a| AccountRow {
                            login: a.login.clone(),
                            active: a.active,
                            selected: Some(&a.login) == selected.as_ref()
                                && source == Some(Source::Gh),
                            chosen: choice.account.as_ref() == Some(&a.login),
                            valid: a.valid,
                            scopes: a.scopes.clone(),
                        })
                        .collect(),
                    sources,
                    host,
                }
            })
            .collect();
        Report {
            gh,
            hosts,
            can_save: self.keys.available(),
            scanned_ms: *self.scanned_ms.lock().unwrap_or_else(|p| p.into_inner()),
        }
    }

    /// Looks again: what `gh` is signed in with, and (the one place a token
    /// is read outside an approved action) asks each host that a saved or
    /// environment token is for whose account it is and what it may do, so
    /// the page can say it and warn about scopes. Nothing is kept on disk.
    pub async fn rescan(&self) -> Report {
        let pending = self.rescan_prepare();
        self.rescan_finish(pending).await
    }

    /// The blocking half of [`GithubAuth::rescan`] (it runs `gh`): the tokens in
    /// use that are to be checked against their hosts.
    pub fn rescan_prepare(&self) -> Vec<(String, Token)> {
        let r = self.status();
        r.hosts
            .iter()
            .filter(|h| h.enabled && matches!(h.source, Some(Source::Saved | Source::Env)))
            // Only the token that is in use: read it, ask once, drop it.
            .filter_map(|h| self.token(&h.host).ok().map(|t| (h.host.clone(), t.token)))
            .collect()
    }

    /// The network half of [`GithubAuth::rescan`].
    pub async fn rescan_finish(&self, pending: Vec<(String, Token)>) -> Report {
        let mut found = BTreeMap::new();
        for (host, token) in &pending {
            found.insert(host.clone(), self.identity_of(host, token).await);
        }
        drop(pending);
        *self.identity.lock().unwrap_or_else(|p| p.into_inner()) = found;
        *self.scanned_ms.lock().unwrap_or_else(|p| p.into_inner()) = now_ms();
        self.status()
    }

    async fn identity_of(&self, host: &str, token: &Token) -> Identity {
        let base = self.api_override.clone().unwrap_or_else(|| api_base(host));
        let Ok(http) = reqwest::Client::builder()
            .timeout(CHECK_WAIT)
            .user_agent("cua-spaces")
            .build()
        else {
            return Identity::default();
        };
        let Ok(resp) = http
            .get(format!("{base}/user"))
            .bearer_auth(token.expose())
            .send()
            .await
        else {
            // Offline is not a verdict on the token.
            return Identity::default();
        };
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Identity {
                refused: true,
                ..Default::default()
            };
        }
        if !status.is_success() {
            return Identity::default();
        }
        let scopes = resp
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(split_scopes);
        let login = resp
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["login"].as_str().map(String::from))
            .filter(|l| valid_login(l));
        Identity {
            login,
            scopes,
            refused: false,
        }
    }

    /// Chooses the `gh` account for `host` (`None`: the one `gh` uses).
    pub fn set_account(&self, host: &str, login: Option<&str>) -> Result<Report> {
        if !valid_host(host) {
            return Err(Error::InvalidArgument("that is not a host name".into()));
        }
        if let Some(l) = login {
            if !valid_login(l) {
                return Err(Error::InvalidArgument("that is not an account name".into()));
            }
            let info = self.gh_info();
            let known = self
                .gh_accounts(&info, Some(host))
                .remove(host)
                .unwrap_or_default();
            if !known.iter().any(|a| a.login == l) {
                return Err(Error::InvalidArgument(format!(
                    "gh is not signed in as {l} on {host}: run gh auth login --hostname {host}"
                )));
            }
        }
        let mut c = self.choices();
        let e = c.hosts.entry(host.into()).or_default();
        e.account = login.map(String::from);
        self.save_choices(c)?;
        Ok(self.status())
    }

    /// Switches `host` on or off.
    pub fn set_enabled(&self, host: &str, on: bool) -> Result<Report> {
        if !valid_host(host) {
            return Err(Error::InvalidArgument("that is not a host name".into()));
        }
        let mut c = self.choices();
        c.hosts.entry(host.into()).or_default().off = !on;
        self.save_choices(c)?;
        Ok(self.status())
    }
}

/// The words for what a source holds.
fn source_line(s: Source, o: &Offer, host: &str) -> String {
    match (s, o) {
        (Source::Saved, Offer::Found { .. }) => "A token saved in Settings, Connections".into(),
        (Source::Saved, _) => "No saved token".into(),
        (Source::Env, Offer::Found { env_name, .. }) => format!(
            "{} in the Cua daemon's environment",
            env_name.clone().unwrap_or_default()
        ),
        (Source::Env, _) => "No GH_TOKEN in the Cua daemon's environment".into(),
        (Source::Gh, Offer::Found { login, .. }) => {
            format!("gh, signed in as {}", login.clone().unwrap_or_default())
        }
        (Source::Gh, Offer::Blocked(u)) => u.message(),
        (Source::Gh, _) => format!("gh is not signed in on {host}"),
        (Source::Git, _) => "Git's own sign-in (SSH key or credential helper)".into(),
    }
}

/// "Signed in as ada with gh".
pub fn signed_in_line(source: Source, login: Option<&str>, env_name: Option<&str>) -> String {
    let how = match source {
        Source::Gh => "gh".to_string(),
        Source::Saved => "a saved token".to_string(),
        Source::Env => format!("{} from the Cua daemon", env_name.unwrap_or("a token")),
        Source::Git => "git".to_string(),
    };
    match login {
        Some(l) => format!("Signed in as {l} with {how}"),
        None => format!("Signed in with {how}"),
    }
}

/// The account in the words a card uses.
pub fn account_words(
    source: Source,
    login: Option<&str>,
    env_name: Option<&str>,
    host: &str,
) -> String {
    let at = if host == DEFAULT_HOST {
        String::new()
    } else {
        format!(" on {host}")
    };
    match (source, login) {
        (Source::Gh, Some(l)) => format!("{l}, signed in with gh{at}"),
        (Source::Gh, None) => format!("the account gh is signed in with{at}"),
        (Source::Saved, _) => format!("the GitHub token saved in Cua{at}"),
        (Source::Env, _) => format!(
            "{} from the Cua daemon's environment{at}",
            env_name.unwrap_or("a token")
        ),
        (Source::Git, _) => format!("this computer's own git sign-in{at}"),
    }
}

/* ---- The app methods ---------------------------------------------------------------- */

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UseArgs {
    host: String,
    /// Absent: the account `gh` uses.
    #[serde(default)]
    login: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnableArgs {
    host: String,
    enabled: bool,
}

/// This process's connection: the daemon's keys, the real `gh`, the real environment.
pub fn global() -> &'static GithubAuth<'static> {
    static AUTH: OnceLock<GithubAuth<'static>> = OnceLock::new();
    AUTH.get_or_init(|| {
        GithubAuth::new(
            super::keys::global(),
            Box::new(SystemGh::new()),
            Box::new(ProcessEnv),
        )
    })
}

fn json(r: Report) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| Error::Agent(e.to_string()))
}

/// Runs `name` (one of [`APP_METHODS`]) on `auth`. `github.rescan` needs the
/// network: [`app_call_async`] does it; here it answers like `github.status`.
pub fn app_call(auth: &GithubAuth<'_>, name: &str, args: Value) -> Option<Result<Value>> {
    let args = if args.is_null() {
        serde_json::json!({})
    } else {
        args
    };
    Some(match name {
        "github.status" | "github.rescan" => json(auth.status()),
        "github.use" => serde_json::from_value::<UseArgs>(args)
            .map_err(|e| Error::InvalidArgument(format!("github.use: {e}")))
            .and_then(|a| auth.set_account(&a.host, a.login.as_deref()))
            .and_then(json),
        "github.enable" => serde_json::from_value::<EnableArgs>(args)
            .map_err(|e| Error::InvalidArgument(format!("github.enable: {e}")))
            .and_then(|a| auth.set_enabled(&a.host, a.enabled))
            .and_then(json),
        _ => return None,
    })
}

/// [`app_call`] for a caller that can wait: `gh` runs off the async threads,
/// and `github.rescan` asks the hosts about the tokens it uses.
pub async fn app_call_async(name: &str, args: Value) -> Option<Result<Value>> {
    if !is_app_method(name) {
        return None;
    }
    if name == "github.rescan" {
        let pending = tokio::task::spawn_blocking(|| global().rescan_prepare())
            .await
            .unwrap_or_default();
        return Some(json(global().rescan_finish(pending).await));
    }
    let name = name.to_string();
    Some(
        tokio::task::spawn_blocking(move || app_call(global(), &name, args))
            .await
            .unwrap_or_else(|e| Some(Err(Error::Agent(e.to_string()))))
            .unwrap_or_else(|| Err(Error::Agent("unknown method".into()))),
    )
}

#[cfg(test)]
mod tests;
