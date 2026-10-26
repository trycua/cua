//! Each agent's own sign-in: whether it is on this computer (read-only
//! checks of the agent's own files and commands), and the copy Connect puts
//! in Keyvault on This Mac so the agent's runs in your Spaces use it.
//!
//! What is read, per agent (sources checked 2026-10-24):
//!
//! | Agent | Signed in | Copied to Keyvault |
//! |---|---|---|
//! | Codex | `$CODEX_HOME/auth.json` (`tokens`, `OPENAI_API_KEY`); with keyring storage, `codex login status`'s exit code | the whole `auth.json` (OpenAI's documented headless route), as `CODEX_AUTH_JSON` |
//! | Claude Code | `claude auth status` (exit 0, `authMethod`), else `.credentials.json` / `.claude.json` | the token `claude setup-token` prints after one browser approval, as `CLAUDE_CODE_OAUTH_TOKEN`; never the Keychain item `Claude Code-credentials` |
//! | Hermes | `$HERMES_HOME/.env` provider keys, `auth.json` `providers` | the key of the provider `config.yaml` uses (OpenRouter, Anthropic, OpenAI); OAuth providers: not yet |
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

fn codex_copy(env: &HostEnv) -> Result<SignInCopy, CopyRefusal> {
    let file = codex_home(env).join("auth.json");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) if !t.trim().is_empty() => t,
        _ if codex_uses_keyring(env) => {
            return Err(CopyRefusal::NotYet(
                "Codex keeps its sign-in in the Keychain here; Cua can't copy it yet.".into(),
            ));
        }
        _ => {
            return Err(CopyRefusal::NotSignedIn(
                "No ChatGPT sign-in on This Mac yet.".into(),
            ));
        }
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        CopyRefusal::Failed(format!(
            "{} can't be read.",
            crate::connection::tilde(env, &file)
        ))
    })?;
    let tokens = v.get("tokens");
    if tokens.is_some_and(|t| non_empty(t.get("refresh_token")) || non_empty(t.get("access_token")))
    {
        // One line: the store keeps single-line values.
        let value = serde_json::to_string(&v).map_err(|e| CopyRefusal::Failed(e.to_string()))?;
        let expires_ms = if tokens.is_some_and(|t| non_empty(t.get("refresh_token"))) {
            None
        } else {
            tokens
                .and_then(|t| t.get("access_token"))
                .and_then(Value::as_str)
                .and_then(jwt_exp_ms)
        };
        return Ok(SignInCopy {
            source: "codex".into(),
            env: CODEX_AUTH_ENV.into(),
            label: "Codex · ChatGPT sign-in".into(),
            value,
            expires_ms,
        });
    }
    if let Some(key) = v
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .filter(|k| !k.trim().is_empty())
    {
        return Ok(SignInCopy {
            source: "codex".into(),
            env: "OPENAI_API_KEY".into(),
            label: "Codex · OpenAI key".into(),
            value: key.trim().into(),
            expires_ms: None,
        });
    }
    Err(CopyRefusal::NotSignedIn(
        "No ChatGPT sign-in on This Mac yet.".into(),
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

/// Hermes providers that sign in with OAuth (`hermes auth`): not copied yet.
const HERMES_OAUTH: &[&str] = &[
    "nous",
    "openai-codex",
    "xai-oauth",
    "qwen-oauth",
    "minimax-oauth",
];

/// The providers a Hermes run in a Space can use, with the key each reads
/// and what the row calls it.
const HERMES_COPYABLE: &[(&str, &str, &str)] = &[
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

/// What Hermes' `.env` key for `provider` is: `(env, label, value)`.
fn hermes_key(env: &HostEnv) -> Option<(&'static str, &'static str, String)> {
    let keys = dotenv(&hermes_home(env).join(".env"));
    let get = |k: &str| keys.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let provider = hermes_provider(env);
    if provider == "auto" {
        return HERMES_COPYABLE
            .iter()
            .find_map(|(_, k, l)| get(k).map(|v| (*k, *l, v)));
    }
    HERMES_COPYABLE
        .iter()
        .find(|(p, _, _)| *p == provider)
        .and_then(|(_, k, l)| get(k).map(|v| (*k, *l, v)))
}

fn hermes_label(env: &HostEnv) -> Option<&'static str> {
    let provider = hermes_provider(env);
    if let Some((_, l, _)) = hermes_key(env) {
        return Some(l);
    }
    HERMES_COPYABLE
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

fn hermes_copy(env: &HostEnv) -> Result<SignInCopy, CopyRefusal> {
    let provider = hermes_provider(env);
    if HERMES_OAUTH.contains(&provider.as_str()) {
        return Err(CopyRefusal::NotYet(format!(
            "Hermes signs in to {provider} with OAuth; Cua can't copy that sign-in yet."
        )));
    }
    match hermes_key(env) {
        Some((k, l, v)) => Ok(SignInCopy {
            source: "hermes".into(),
            env: k.into(),
            label: format!("Hermes · {l}"),
            value: v,
            expires_ms: None,
        }),
        None if provider == "auto" && !hermes_oauth_providers(env).is_empty() => {
            Err(CopyRefusal::NotYet(
                "Hermes signs in with OAuth; Cua can't copy that sign-in yet.".into(),
            ))
        }
        None if provider != "auto" && !HERMES_COPYABLE.iter().any(|(p, _, _)| *p == provider) => {
            Err(CopyRefusal::NotYet(format!(
                "Runs in a Space can't use Hermes' {provider} key yet."
            )))
        }
        None => Err(CopyRefusal::NotSignedIn(
            "No provider key in Hermes' .env on This Mac yet.".into(),
        )),
    }
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

/// Whether Connect copies this agent's sign-in at all.
pub fn importable(agent: &str) -> bool {
    matches!(agent, "codex" | "claude-code" | "hermes")
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

/// Where the sign-in is read from, as Advanced shows it.
pub fn copy_source(env: &HostEnv, agent: &str) -> Option<String> {
    match agent {
        "codex" => Some(crate::connection::tilde(
            env,
            &codex_home(env).join("auth.json"),
        )),
        "claude-code" => Some("claude setup-token".into()),
        "hermes" => Some(crate::connection::tilde(
            env,
            &hermes_home(env).join(".env"),
        )),
        _ => None,
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
        "codex" => codex_copy(env),
        "claude-code" => claude_copy(env, wait),
        "hermes" => hermes_copy(env),
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
