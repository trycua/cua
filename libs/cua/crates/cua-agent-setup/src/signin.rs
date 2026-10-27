//! Each agent's own sign-in: whether it is on this computer (read-only
//! checks of the agent's own files and commands), and the copy Connect puts
//! in Keyvault on This Mac so the agent's runs in your Spaces use it.
//!
//! What is read, per agent (sources checked 2026-10-24):
//!
//! | Agent | Signed in | Copied to Keyvault |
//! |---|---|---|
//! | Codex | `$CODEX_HOME/auth.json` (`tokens`, `OPENAI_API_KEY`); with keyring storage, `codex login status`'s exit code | a sign-in made for Cua: `codex login --device-auth` in a folder Cua owns ([`DeviceSignIn`]), as `CODEX_AUTH_JSON`; never your own `auth.json` |
//! | Claude Code | `claude auth status` (exit 0, `authMethod`), else `.credentials.json` / `.claude.json` | the token `claude setup-token` prints after one browser approval, as `CLAUDE_CODE_OAUTH_TOKEN`; never the Keychain item `Claude Code-credentials` |
//! | Hermes | `$HERMES_HOME/.env` provider keys, `auth.json` `providers` (whether a key is there; never its value) | nothing: it uses the providers connected in Settings, Connectors |
//! | Cursor | `~/.config/cursor/auth.json`, `~/.cursor/auth.json`, `CURSOR_API_KEY`; else unknown | nothing (no supported export) |
//! | OpenClaw | legacy `auth-profiles.json`; else unknown (profiles are in SQLite) | nothing yet |
//! | T3 Code | uses the agents it controls | nothing (it uses Codex's and Claude's own) |
//!
//! Sources: Codex `codex-rs/login/src/auth/storage.rs` (`AuthDotJson`,
//! rust-v0.164.0) and learn.chatgpt.com/docs/auth; Claude Code
//! code.claude.com/docs/en/authentication and `/cli-reference`; Hermes
//! `hermes_cli/auth.py`, `.env.example`, `cli-config.yaml.example`
//! (v0.22.5); OpenClaw `docs/gateway/authentication.md` (v2026.10.7);
//! Cursor cursor.com/docs/cli/reference/authentication.

use crate::{
    Error, HostEnv, Result,
    connection::{SignInCopy, run_with_timeout},
    registry,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// What a check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedIn {
    Yes,
    No,
    Unknown,
    Expired,
}

/// A sign-in check: the state, and what the sign-in is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub state: SignedIn,
    /// "ChatGPT sign-in", "Claude sign-in".
    pub label: Option<&'static str>,
}

const QUICK: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn non_empty(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

/// A JWT's `exp`, in Unix ms (unverified: only to tell an expired copy).
fn jwt_exp_ms(token: &str) -> Option<u64> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(Value::as_u64).map(|s| s * 1000)
}

// ---------------------------------------------------------------- Codex

pub(crate) fn codex_home(env: &HostEnv) -> PathBuf {
    env.var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".codex"))
}

fn codex_uses_keyring(env: &HostEnv) -> bool {
    std::fs::read_to_string(codex_home(env).join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|d| {
            d.get("cli_auth_credentials_store")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .is_some_and(|s| s == "keyring")
}

fn codex_check(env: &HostEnv) -> Check {
    let label = Some("ChatGPT sign-in");
    let file = codex_home(env).join("auth.json");
    match std::fs::read_to_string(&file) {
        Ok(text) if text.trim().is_empty() => Check {
            state: SignedIn::No,
            label,
        },
        Ok(text) => {
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                return Check {
                    state: SignedIn::Unknown,
                    label,
                };
            };
            let tokens = v.get("tokens");
            if tokens.is_some_and(|t| non_empty(t.get("refresh_token"))) {
                return Check {
                    state: SignedIn::Yes,
                    label,
                };
            }
            if let Some(exp) = tokens
                .and_then(|t| t.get("access_token"))
                .and_then(Value::as_str)
                .and_then(jwt_exp_ms)
            {
                let state = if exp <= now_ms() {
                    SignedIn::Expired
                } else {
                    SignedIn::Yes
                };
                return Check { state, label };
            }
            if non_empty(v.get("OPENAI_API_KEY")) {
                return Check {
                    state: SignedIn::Yes,
                    label: Some("OpenAI key"),
                };
            }
            Check {
                state: SignedIn::No,
                label,
            }
        }
        Err(_) if codex_uses_keyring(env) => {
            // The sign-in is in the system keyring: ask Codex (it prints to
            // stderr and exits 0 signed in, 1 not).
            let state = match crate::connection::binary(env, registry::find("codex").unwrap()) {
                Some(bin) => match run_with_timeout(env, &bin, &["login", "status"], QUICK) {
                    Some(_) => SignedIn::Yes,
                    None => SignedIn::No,
                },
                None => SignedIn::Unknown,
            };
            Check { state, label }
        }
        Err(_) => Check {
            state: SignedIn::No,
            label,
        },
    }
}

/// Codex's sign-in for a run in a Space: one made for Cua with Codex's own
/// device-code login ([`DeviceSignIn`]); your own `auth.json` is never read.
fn codex_copy() -> Result<SignInCopy, CopyRefusal> {
    Err(CopyRefusal::NotYet(
        "Connected. Codex signs in with a code: run `cua connect setup --agents codex --sign-in`, or use Connect in Cua Spaces."
            .into(),
    ))
}

/// The variable a run gets Codex's copied `auth.json` as; the Codex
/// harness writes it to `~/.codex/auth.json` in the Space.
pub const CODEX_AUTH_ENV: &str = "CODEX_AUTH_JSON";

// ---------------------------------------------------------- Claude Code

fn claude_dir(env: &HostEnv) -> PathBuf {
    env.var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude"))
}

fn claude_json(env: &HostEnv) -> PathBuf {
    match env.var("CLAUDE_CONFIG_DIR") {
        Some(d) => PathBuf::from(d).join(".claude.json"),
        None => env.home.join(".claude.json"),
    }
}

fn claude_check(env: &HostEnv) -> Check {
    let label = Some("Claude sign-in");
    // Claude's own answer first (`claude auth status`: JSON, exit 0 when
    // signed in, 1 when not).
    if let Some(bin) = crate::connection::binary(env, registry::find("claude-code").unwrap()) {
        if let Some(out) = run_with_timeout(env, &bin, &["auth", "status"], QUICK) {
            let method = serde_json::from_str::<Value>(out.trim())
                .ok()
                .and_then(|v| {
                    v.get("authMethod")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            if method.as_deref() != Some("none") {
                return Check {
                    state: SignedIn::Yes,
                    label,
                };
            }
            return Check {
                state: SignedIn::No,
                label,
            };
        }
        // A failed command is "not signed in" only if Claude said so; an
        // older Claude without `auth status` falls through to the files.
        if claude_says_signed_out(env, &bin) {
            return Check {
                state: SignedIn::No,
                label,
            };
        }
    }
    if let Some(v) = read_json(&claude_dir(env).join(".credentials.json")) {
        let o = v.get("claudeAiOauth");
        if o.is_some_and(|o| non_empty(o.get("refreshToken"))) {
            return Check {
                state: SignedIn::Yes,
                label,
            };
        }
        if let Some(exp) = o.and_then(|o| o.get("expiresAt")).and_then(Value::as_u64) {
            let state = if exp <= now_ms() {
                SignedIn::Expired
            } else {
                SignedIn::Yes
            };
            return Check { state, label };
        }
    }
    if let Some(v) = read_json(&claude_json(env))
        && (v.get("oauthAccount").is_some_and(Value::is_object)
            || non_empty(v.get("primaryApiKey")))
    {
        return Check {
            state: SignedIn::Yes,
            label,
        };
    }
    Check {
        state: SignedIn::Unknown,
        label,
    }
}

/// `claude auth status` exited 1 with JSON saying `none`.
fn claude_says_signed_out(env: &HostEnv, bin: &Path) -> bool {
    let out = crate::connection::run_capture(env, bin, &["auth", "status"], QUICK);
    out.is_some_and(|(code, stdout)| {
        code == Some(1)
            && serde_json::from_str::<Value>(stdout.trim())
                .ok()
                .and_then(|v| {
                    v.get("authMethod")
                        .and_then(Value::as_str)
                        .map(|m| m == "none")
                })
                .unwrap_or(false)
    })
}

/// How long `claude setup-token` waits for the browser approval.
pub const SETUP_TOKEN_WAIT: Duration = Duration::from_secs(600);

/// A `claude setup-token` token (`sk-ant-oat01-…`) in its output.
pub fn find_setup_token(out: &str) -> Option<String> {
    let start = out.find("sk-ant-oat")?;
    let tok: String = out[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (tok.len() > 20).then_some(tok)
}

fn claude_copy(env: &HostEnv, wait: Duration) -> Result<SignInCopy, CopyRefusal> {
    let bin = crate::connection::binary(env, registry::find("claude-code").unwrap())
        .ok_or_else(|| CopyRefusal::Failed("Claude Code isn't installed on This Mac.".into()))?;
    if !env.run_agent_clis {
        return Err(CopyRefusal::Failed(
            "agent commands are off in this environment".into(),
        ));
    }
    // `setup-token` draws in a terminal; `script` gives it one where the
    // system has it. The browser opens for the one approval.
    let script = Path::new("/usr/bin/script");
    let bin_s = bin.display().to_string();
    let out = match env.os {
        crate::Os::MacOs if script.exists() => crate::connection::run_capture(
            env,
            script,
            &["-q", "/dev/null", &bin_s, "setup-token"],
            wait,
        ),
        crate::Os::Linux if script.exists() => {
            let cmd = format!("{} setup-token", shell_quote(&bin_s));
            crate::connection::run_capture(env, script, &["-qec", &cmd, "/dev/null"], wait)
        }
        _ => crate::connection::run_capture(env, &bin, &["setup-token"], wait),
    };
    let Some((code, stdout)) = out else {
        return Err(CopyRefusal::Cancelled(
            "Connected. Sign-in not copied: the browser approval didn't finish.".into(),
        ));
    };
    match find_setup_token(&stdout) {
        Some(token) => Ok(SignInCopy {
            source: "claude-code".into(),
            env: "CLAUDE_CODE_OAUTH_TOKEN".into(),
            label: "Claude Code · Claude sign-in".into(),
            value: token,
            // A setup-token token lasts a year.
            expires_ms: Some(now_ms() + 365 * 24 * 3600 * 1000),
        }),
        None if code == Some(0) => Err(CopyRefusal::Failed(
            "Connected. Sign-in not copied: claude setup-token printed no token.".into(),
        )),
        None => Err(CopyRefusal::Cancelled(
            "Connected. Sign-in not copied: the browser approval was cancelled.".into(),
        )),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// --------------------------------------------------------------- Hermes

fn hermes_home(env: &HostEnv) -> PathBuf {
    if let Some(h) = env.var("HERMES_HOME") {
        return PathBuf::from(h);
    }
    if env.os == crate::Os::Windows {
        let local = env
            .var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.join("AppData/Local"));
        return local.join("hermes");
    }
    env.home.join(".hermes")
}

/// The `.env` lines Hermes reads (`KEY=value`, `export KEY=value`, quotes).
fn dotenv(p: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(p) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                return None;
            }
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_string(), v.to_string()))
        })
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// Provider keys Hermes reads from `.env` (`hermes_cli/auth.py` registry).
const HERMES_KEYS: &[&str] = &[
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "OPENAI_API_KEY",
    "NOUS_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "GLM_API_KEY",
    "ZAI_API_KEY",
    "Z_AI_API_KEY",
    "KIMI_API_KEY",
    "MINIMAX_API_KEY",
    "COPILOT_GITHUB_TOKEN",
    "DEEPSEEK_API_KEY",
    "XAI_API_KEY",
    "HF_TOKEN",
    "NVIDIA_API_KEY",
    "DASHSCOPE_API_KEY",
    "AI_GATEWAY_API_KEY",
];

/// Hermes' providers Cua names, with the key each reads and what the row
/// calls it.
const HERMES_NAMED: &[(&str, &str, &str)] = &[
    ("openrouter", "OPENROUTER_API_KEY", "OpenRouter key"),
    ("anthropic", "ANTHROPIC_API_KEY", "Anthropic key"),
    ("openai", "OPENAI_API_KEY", "OpenAI key"),
    ("openai-api", "OPENAI_API_KEY", "OpenAI key"),
];

/// `model.provider` in `config.yaml` (`auto` when unset).
fn hermes_provider(env: &HostEnv) -> String {
    std::fs::read_to_string(hermes_home(env).join("config.yaml"))
        .ok()
        .and_then(|t| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&t).ok())
        .and_then(|v| {
            v.get("model")
                .and_then(|m| m.get("provider"))
                .and_then(|p| p.as_str())
                .map(|s| s.trim().to_ascii_lowercase())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "auto".into())
}

fn hermes_oauth_providers(env: &HostEnv) -> Vec<String> {
    read_json(&hermes_home(env).join("auth.json"))
        .and_then(|v| v.get("providers").and_then(Value::as_object).cloned())
        .map(|o| {
            o.into_iter()
                .filter(|(_, s)| !s.is_null() && s.as_object().is_none_or(|m| !m.is_empty()))
                .map(|(k, _)| k)
                .collect()
        })
        .unwrap_or_default()
}

/// Which of Hermes' `.env` keys its provider uses: `(env, label)`. Only
/// whether the name is there; the value is never kept.
fn hermes_key(env: &HostEnv) -> Option<(&'static str, &'static str)> {
    let names: Vec<String> = dotenv(&hermes_home(env).join(".env"))
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let has = |k: &str| names.iter().any(|n| n == k);
    let provider = hermes_provider(env);
    HERMES_NAMED
        .iter()
        .filter(|(p, _, _)| provider == "auto" || *p == provider)
        .find(|(_, k, _)| has(k))
        .map(|(_, k, l)| (*k, *l))
}

fn hermes_label(env: &HostEnv) -> Option<&'static str> {
    let provider = hermes_provider(env);
    if let Some((_, l)) = hermes_key(env) {
        return Some(l);
    }
    HERMES_NAMED
        .iter()
        .find(|(p, _, _)| *p == provider)
        .map(|(_, _, l)| *l)
        .or(Some("provider key"))
}

fn hermes_check(env: &HostEnv) -> Check {
    let keys = dotenv(&hermes_home(env).join(".env"));
    let has_key = keys.iter().any(|(k, _)| HERMES_KEYS.contains(&k.as_str()));
    let state = if has_key || !hermes_oauth_providers(env).is_empty() {
        SignedIn::Yes
    } else {
        SignedIn::No
    };
    Check {
        state,
        label: hermes_label(env),
    }
}

/// Hermes uses the provider keys connected in Settings, Connectors; its own
/// `.env` is never copied.
fn hermes_copy() -> Result<SignInCopy, CopyRefusal> {
    Err(CopyRefusal::NotYet(
        "Connected. Hermes uses your Anthropic or OpenAI connection: connect one in Settings, Connectors.".into(),
    ))
}

// ------------------------------------------------- Cursor, OpenClaw, T3

fn cursor_check(env: &HostEnv) -> Check {
    let config = env
        .var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".config"));
    let files = [
        config.join("cursor/auth.json"),
        env.home.join(".cursor/auth.json"),
    ];
    let state = if files
        .iter()
        .any(|f| std::fs::metadata(f).is_ok_and(|m| m.is_file() && m.len() > 2))
    {
        SignedIn::Yes
    } else {
        // Cursor's app keeps its sign-in in its own database: Cua can't
        // tell, so it doesn't guess.
        SignedIn::Unknown
    };
    Check { state, label: None }
}

fn openclaw_check(env: &HostEnv) -> Check {
    let state_dir = env
        .var("OPENCLAW_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".openclaw"));
    let agents = state_dir.join("agents");
    let legacy = std::fs::read_dir(&agents)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("agent/auth-profiles.json"))
        .filter_map(|p| read_json(&p))
        .any(|v| {
            v.get("profiles")
                .and_then(Value::as_object)
                .is_some_and(|o| !o.is_empty())
        });
    Check {
        state: if legacy {
            SignedIn::Yes
        } else {
            SignedIn::Unknown
        },
        label: None,
    }
}

// ------------------------------------------------------------- Public

/// The agent's own sign-in, checked now.
pub fn check(env: &HostEnv, agent: &str) -> Check {
    match agent {
        "codex" => codex_check(env),
        "claude-code" => claude_check(env),
        "hermes" => hermes_check(env),
        "cursor" => cursor_check(env),
        "openclaw" => openclaw_check(env),
        _ => Check {
            state: SignedIn::Unknown,
            label: None,
        },
    }
}

/// Whether Connect makes a sign-in for this agent at all (Claude Code's
/// `setup-token`, Codex's device code).
pub fn importable(agent: &str) -> bool {
    matches!(agent, "codex" | "claude-code")
}

/// Whether Cua can start the agent's own sign-in (`codex login`).
pub fn can_start_sign_in(agent: &str) -> bool {
    registry::find(agent).is_some_and(|a| a.login.is_some())
}

/// What the row's indented tick copies ("ChatGPT sign-in"), when any.
pub fn copy_label(env: &HostEnv, agent: &str) -> Option<String> {
    match agent {
        "codex" => Some(codex_check(env).label.unwrap_or("ChatGPT sign-in").into()),
        "claude-code" => Some("Claude sign-in".into()),
        "hermes" => hermes_label(env).map(Into::into),
        _ => None,
    }
}

/// How the sign-in is made, as Advanced shows it.
pub fn copy_source(_env: &HostEnv, agent: &str) -> Option<String> {
    match agent {
        "codex" => Some("codex login --device-auth".into()),
        "claude-code" => Some("claude setup-token".into()),
        _ => None,
    }
}

// ------------------------------------------------------- device sign-in

/// How long the device code waits for the person to enter it (Codex's own is 15 minutes).
pub const DEVICE_SIGN_IN_WAIT: Duration = Duration::from_secs(15 * 60);

/// How long Codex has to print its link and code.
const DEVICE_CODE_WAIT: Duration = Duration::from_secs(30);

/// Codex's ChatGPT sign-in made for Cua: `codex login --device-auth` run with
/// `CODEX_HOME` in a folder Cua owns, so your own Codex sign-in is never read
/// or changed. [`start_device_sign_in`] answers the link and the one-time
/// code to show; [`DeviceSignIn::wait`] waits for the approval, copies the
/// new sign-in and deletes the folder. Dropped unfinished, the login is
/// stopped and the folder deleted.
pub struct DeviceSignIn {
    pub url: String,
    pub code: String,
    child: Option<std::process::Child>,
    home: PathBuf,
}

impl std::fmt::Debug for DeviceSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSignIn")
            .field("url", &self.url)
            .field("code", &self.code)
            .finish()
    }
}

/// Text without terminal colors.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ ... final byte in @..~
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The link and the one-time code in what Codex printed.
pub fn find_device_code(out: &str) -> Option<(String, String)> {
    let text = plain(out);
    let url = text
        .split_whitespace()
        .find(|w| w.starts_with("https://"))?
        .trim_end_matches(['.', ','])
        .to_string();
    let code = text.split_whitespace().find(|w| {
        let parts: Vec<&str> = w.split('-').collect();
        parts.len() == 2
            && parts.iter().all(|p| {
                (4..=5).contains(&p.len())
                    && p.chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            })
    })?;
    Some((url, code.to_string()))
}

/// Whether `agent` signs in for Cua with a code entered in the browser
/// (its device sign-in) rather than a token command or a copy.
pub fn signs_in_with_a_code(agent: &str) -> bool {
    agent == "codex"
}

/// Starts Codex's device-code sign-in in `dir` (a folder Cua owns, emptied first).
pub fn start_device_sign_in(
    env: &HostEnv,
    dir: &Path,
    agent: &str,
) -> Result<DeviceSignIn, CopyRefusal> {
    if !signs_in_with_a_code(agent) {
        return Err(CopyRefusal::NotYet(format!(
            "{} has no device sign-in.",
            registry::find(agent)
                .map(crate::connection::display_name)
                .unwrap_or(agent)
        )));
    }
    let bin = crate::connection::binary(env, registry::find("codex").unwrap())
        .ok_or_else(|| CopyRefusal::Failed("Codex isn't installed on This Mac.".into()))?;
    if !env.run_agent_clis {
        return Err(CopyRefusal::Failed(
            "agent commands are off in this environment".into(),
        ));
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)
        .map_err(|e| CopyRefusal::Failed(format!("Couldn't make {}: {e}", dir.display())))?;
    let mut child = crate::connection::base_command(env, &bin)
        .args(["login", "--device-auth"])
        .env("CODEX_HOME", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CopyRefusal::Failed(format!("Couldn't start Codex's sign-in: {e}")))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut pipe = pipe;
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
    }
    drop(tx);
    let start = std::time::Instant::now();
    let mut seen = String::new();
    let mut login = DeviceSignIn {
        url: String::new(),
        code: String::new(),
        child: Some(child),
        home: dir.to_path_buf(),
    };
    while start.elapsed() < DEVICE_CODE_WAIT {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                seen.push_str(&chunk);
                if let Some((url, code)) = find_device_code(&seen) {
                    login.url = url;
                    login.code = code;
                    return Ok(login);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Err(CopyRefusal::Failed(
        "Codex didn't show a sign-in code. Try again.".into(),
    ))
}

use std::io::Read as _;

impl DeviceSignIn {
    /// Waits up to `wait` for the code to be entered and approved, then
    /// copies the sign-in Codex made in the folder and deletes the folder.
    pub fn wait(mut self, wait: Duration) -> Result<SignInCopy, CopyRefusal> {
        let mut child = self.child.take().expect("waited once");
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if start.elapsed() < wait => {
                    std::thread::sleep(Duration::from_millis(250))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let file = self.home.join("auth.json");
        let text = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_dir_all(&self.home);
        let Some(status) = status else {
            return Err(CopyRefusal::Cancelled(
                "The sign-in code expired before it was entered.".into(),
            ));
        };
        let text = match (status.success(), text) {
            (true, Some(t)) if !t.trim().is_empty() => t,
            _ => {
                return Err(CopyRefusal::Cancelled(
                    "The ChatGPT sign-in didn't finish.".into(),
                ));
            }
        };
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| CopyRefusal::Failed("Codex's sign-in couldn't be read.".into()))?;
        let value = serde_json::to_string(&v).map_err(|e| CopyRefusal::Failed(e.to_string()))?;
        Ok(SignInCopy {
            source: "codex".into(),
            env: CODEX_AUTH_ENV.into(),
            label: "Codex · ChatGPT sign-in".into(),
            value,
            expires_ms: None,
        })
    }
}

impl Drop for DeviceSignIn {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }
}

/// Why a sign-in wasn't copied (the row's one line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyRefusal {
    /// The agent isn't signed in here (the row offers Sign in).
    NotSignedIn(String),
    /// The person cancelled the browser approval (Try again).
    Cancelled(String),
    /// Cua doesn't copy this kind of sign-in yet.
    NotYet(String),
    /// Something else (the line says what).
    Failed(String),
}

impl CopyRefusal {
    pub fn line(&self) -> &str {
        match self {
            CopyRefusal::NotSignedIn(s)
            | CopyRefusal::Cancelled(s)
            | CopyRefusal::NotYet(s)
            | CopyRefusal::Failed(s) => s,
        }
    }
    /// `not_signed_in`, `cancelled`, `not_yet`, `failed`.
    pub fn kind(&self) -> &'static str {
        match self {
            CopyRefusal::NotSignedIn(_) => "not_signed_in",
            CopyRefusal::Cancelled(_) => "cancelled",
            CopyRefusal::NotYet(_) => "not_yet",
            CopyRefusal::Failed(_) => "failed",
        }
    }
}

/// Reads the sign-in Connect copies. Claude Code's needs one browser
/// approval (`claude setup-token`, waiting up to `wait`).
pub fn read_copy(env: &HostEnv, agent: &str, wait: Duration) -> Result<SignInCopy, CopyRefusal> {
    match agent {
        "codex" => codex_copy(),
        "claude-code" => claude_copy(env, wait),
        "hermes" => hermes_copy(),
        other => Err(CopyRefusal::NotYet(format!(
            "{} has no sign-in Cua can copy.",
            registry::find(other)
                .map(crate::connection::display_name)
                .unwrap_or(other)
        ))),
    }
}

/// An unknown agent id.
pub(crate) fn unknown(agent: &str) -> Error {
    Error::InvalidArgument(format!("unknown agent {agent:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_token_is_found_in_terminal_output() {
        let out = "\u{1b}[2K Your token:\r\n\r\n  sk-ant-oat01-AbC_d3f-xyz0123456789AAAA\r\n\r\nStore it safely.";
        assert_eq!(
            find_setup_token(out).as_deref(),
            Some("sk-ant-oat01-AbC_d3f-xyz0123456789AAAA")
        );
        assert_eq!(find_setup_token("Login cancelled"), None);
        assert_eq!(find_setup_token("sk-ant-oat"), None);
    }

    #[test]
    fn dotenv_reads_hermes_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".env");
        std::fs::write(
            &p,
            "# comment\nOPENROUTER_API_KEY=\"sk-or-1\"\nexport ANTHROPIC_API_KEY='sk-ant-2'\nEMPTY=\n",
        )
        .unwrap();
        assert_eq!(
            dotenv(&p),
            vec![
                ("OPENROUTER_API_KEY".to_string(), "sk-or-1".to_string()),
                ("ANTHROPIC_API_KEY".to_string(), "sk-ant-2".to_string()),
            ]
        );
    }
}
