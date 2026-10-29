//! `cua connections`: the providers Cua Spaces connects once for every app
//! that can use them (what Settings, Connections shows).
//!
//! ```text
//! cua connections list                         # every provider, its state, the apps it works with
//! pbpaste | cua connections add deepseek       # the key comes from stdin, never argv
//! cua connections add custom --base-url http://host:1234/v1 --model qwen < /dev/null
//! cua connections add zai --region coding-global < key.txt
//! cua connections test                         # ask each provider whether the saved keys still work
//! cua connections remove deepseek
//! ```
//!
//! These are the daemon's app methods (`agent_keys.*`), not MCP tools: no
//! agent can read or change a key, and nothing here prints one. `cua connect`
//! is a different command: it connects your coding apps to Cua.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use cua_agents::connections as reg;
use cua_sdk::{Cua, CuaError};

use crate::util::{hint, line};

/// How long `cua connections test` waits for the providers in all.
const TEST_WAIT: Duration = Duration::from_secs(20);

#[derive(Subcommand, Debug, Clone)]
pub enum ConnectionsCmd {
    /// List the providers with their state (Connected, Needs reconnect, Not
    /// connected) and the apps each works with. Never prints a key.
    #[command(after_help = "Examples:
  cua connections list
  cua connections list --all
  cua --json connections list")]
    List {
        /// Also list every provider that is not connected (by default: the
        /// ones up front and the ones with something saved).
        #[arg(long)]
        all: bool,
    },
    /// Connect a provider. The key is read from stdin (pipe it in; a
    /// terminal is refused so it isn't echoed). Saving never waits on the
    /// provider: `cua connections test` asks it afterwards.
    ///
    /// The provider is one of the ids `cua connections list --all` prints.
    /// Pipe the key in: `pbpaste | cua connections add deepseek`. Sign-ins
    /// another app made on this machine are imported with
    /// `cua connect setup <app> --sign-in`.
    #[command(after_help = "Examples:
  cua connections add deepseek
  cua connections add zai --region coding-global
  cua connections add deepinfra --model meta-llama/Meta-Llama-3.1-70B-Instruct
  cua connections add custom --base-url http://host:1234/v1 --model qwen")]
    Add {
        /// The provider id (`cua connections list --all`).
        provider: String,
        /// How: `key` (an API key from stdin; the default) or `endpoint` (a
        /// custom endpoint: its base URL, model and API style; a key is optional).
        #[arg(long)]
        method: Option<String>,
        /// The plan or region, for a provider that has them (Z.AI: global, cn,
        /// coding-global, coding-cn).
        #[arg(long)]
        region: Option<String>,
        /// The model to use, for a provider with no default model or to choose another.
        #[arg(long)]
        model: Option<String>,
        /// A custom endpoint's base URL.
        #[arg(long = "base-url")]
        base_url: Option<String>,
        /// A custom endpoint's API style: openai-chat (default), openai-responses or anthropic.
        #[arg(long)]
        wire: Option<String>,
    },
    /// Ask each provider whether its saved key still works (a read-only
    /// request on the provider's own host, bounded in time; offline is not a
    /// failure). A key the provider refuses is listed as Needs reconnect.
    #[command(after_help = "Examples:
  cua connections test
  cua connections test openrouter")]
    Test {
        /// Only this provider.
        provider: Option<String>,
    },
    /// Disconnect a provider: remove what is saved for it.
    #[command(after_help = "Examples:
  cua connections remove deepseek")]
    Remove { provider: String },
}

/// One provider as `list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Row {
    pub id: String,
    pub name: String,
    /// `connected`, `needs_reconnect` or `not_connected`.
    pub state: &'static str,
    /// The apps it works with.
    pub works_with: Vec<String>,
    /// `api_key`, `import_login`, ... or the plan chosen, when connected.
    pub detail: Option<String>,
}

/// The key from stdin: piped, never typed (a terminal would echo it).
pub fn read_key(stdin: &mut dyn Read, is_terminal: bool, required: bool) -> Result<String, CuaError> {
    if is_terminal {
        if !required {
            return Ok(String::new());
        }
        return Err(CuaError::InvalidArgument(
            "pipe the key in, so it isn't shown or kept in your shell history: \
             pbpaste | cua connections add <provider>"
                .into(),
        ));
    }
    let mut s = String::new();
    stdin
        .take(16 * 1024)
        .read_to_string(&mut s)
        .map_err(|e| CuaError::InvalidArgument(format!("reading the key from stdin: {e}")))?;
    let s = s.trim().to_string();
    if s.is_empty() && required {
        return Err(CuaError::InvalidArgument("no key on stdin".into()));
    }
    Ok(s)
}

fn harness_name(id: &str) -> String {
    cua_agents::harness::harness(id).map_or_else(|| id.to_string(), |h| h.name.to_string())
}

/// The rows for `report` (the daemon's `agent_keys.list`): every visible
/// provider, up front first.
pub fn rows(report: &serde_json::Value, all: bool) -> Vec<Row> {
    let keys = report["keys"].as_array().cloned().unwrap_or_default();
    let by_env = |env: &str| keys.iter().find(|k| k["env"] == env);
    let mut out = vec![];
    let ordered = reg::visible()
        .filter(|p| p.tier == reg::Tier::Top)
        .chain(reg::visible().filter(|p| p.tier != reg::Tier::Top));
    for p in ordered {
        let saved: Vec<&serde_json::Value> = p.envs().filter_map(by_env).collect();
        let working = saved.iter().find(|k| k["expired"] != true);
        let state = if working.is_some() {
            "connected"
        } else if !saved.is_empty() {
            "needs_reconnect"
        } else {
            "not_connected"
        };
        if !all && state == "not_connected" && p.tier != reg::Tier::Top {
            continue;
        }
        let detail = working.or(saved.first()).map(|k| {
            let region = k["region"]
                .as_str()
                .and_then(|r| p.regions.iter().find(|x| x.id == r))
                .map(|r| format!(", {}", r.name))
                .unwrap_or_default();
            format!("{}{region}", method_of(p, k["env"].as_str().unwrap_or_default()))
        });
        let mut works: Vec<String> = reg::works_with(p).into_iter().map(harness_name).collect();
        works.extend(p.apps.iter().map(|a| match *a {
            "copilot-cli" => "GitHub Copilot CLI".to_string(),
            "t3code" => "T3 Code".to_string(),
            other => other.to_string(),
        }));
        out.push(Row {
            id: p.id.into(),
            name: p.name.into(),
            state,
            works_with: works,
            detail,
        });
    }
    out
}

/// How a saved name connects `p`, in words.
fn method_of(p: &reg::Provider, env: &str) -> &'static str {
    match p
        .methods
        .iter()
        .find(|m| m.env == env || m.aliases.contains(&env))
        .map(|m| m.kind)
    {
        Some(reg::MethodKind::ImportLogin) => "sign-in found on this machine",
        Some(reg::MethodKind::SubscriptionLogin) => "plan sign-in",
        Some(reg::MethodKind::CustomEndpoint) => "custom endpoint",
        _ => "API key",
    }
}

fn state_words(state: &str) -> &'static str {
    match state {
        "connected" => "Connected",
        "needs_reconnect" => "Needs reconnect",
        _ => "Not connected",
    }
}

/// `list`'s text lines for `rows`.
pub fn list_lines(rows: &[Row], report: &serde_json::Value) -> Vec<String> {
    let mut v = vec![];
    if report["available"] == false {
        v.push(format!(
            "Keys can't be saved here: {}.",
            report["unavailable"].as_str().unwrap_or("no Keychain")
        ));
    }
    let w = rows.iter().map(|r| r.id.len()).max().unwrap_or(0);
    for r in rows {
        let mut l = format!("{:<w$}  {:<15}", r.id, state_words(r.state));
        if let Some(d) = &r.detail {
            l.push_str(&format!("  {d}"));
        }
        if !r.works_with.is_empty() {
            l.push_str(&format!("  works with {}", r.works_with.join(", ")));
        }
        v.push(l.trim_end().to_string());
    }
    if rows.iter().all(|r| r.state == "not_connected") {
        v.push(hint(
            "Nothing connected yet. Add a key with: pbpaste | cua connections add anthropic",
        ));
    }
    v
}

/// The daemon call for `cmd`: method and arguments, after the input checks.
pub fn plan(cmd: &ConnectionsCmd, key: impl FnOnce(bool) -> Result<String, CuaError>) -> Result<(String, serde_json::Value), CuaError> {
    match cmd {
        ConnectionsCmd::Add { provider, method, region, model, base_url, wire } => {
            let def = reg::provider(provider)
                .filter(|p| p.visible())
                .ok_or_else(|| {
                    CuaError::InvalidArgument(format!(
                        "unknown provider {provider:?}: one of {} (see `cua connections list --all`)",
                        reg::PROVIDER_IDS.join(", ")
                    ))
                })?;
            let want = method.as_deref().unwrap_or(if def.kind == reg::Kind::Custom { "endpoint" } else { "key" });
            let offered: Vec<&str> = def
                .offered()
                .filter_map(|m| match m.kind {
                    reg::MethodKind::ApiKey => Some("key"),
                    reg::MethodKind::CustomEndpoint => Some("endpoint"),
                    _ => None,
                })
                .collect();
            if !offered.contains(&want) {
                let other = def
                    .offered()
                    .any(|m| matches!(m.kind, reg::MethodKind::ImportLogin | reg::MethodKind::SubscriptionLogin));
                return Err(CuaError::InvalidArgument(if other && matches!(want, "import" | "login") {
                    format!(
                        "{} signs in through its app: run `cua connect setup <app> --sign-in`, or use --method key with an API key",
                        def.name
                    )
                } else {
                    format!("{} can be connected with: {}", def.name, offered.join(", "))
                }));
            }
            let optional = def.key_method().is_some_and(|m| m.secret_optional);
            let value = key(!optional)?;
            let mut a = serde_json::json!({ "provider": def.id, "value": value });
            if let Some(r) = region {
                a["region"] = r.clone().into();
            }
            let mut fields = serde_json::Map::new();
            if let Some(m) = model {
                fields.insert("model".into(), m.clone().into());
            }
            if def.kind == reg::Kind::Custom {
                if let Some(b) = base_url {
                    fields.insert("base_url".into(), b.clone().into());
                }
                fields.insert("wire".into(), wire.clone().unwrap_or_else(|| "openai-chat".into()).into());
            } else if base_url.is_some() || wire.is_some() {
                return Err(CuaError::InvalidArgument(
                    "--base-url and --wire are for a custom endpoint (cua connections add custom)".into(),
                ));
            }
            if !fields.is_empty() {
                a["fields"] = fields.into();
            }
            Ok(("agent_keys.set".into(), a))
        }
        ConnectionsCmd::Test { provider } => {
            if let Some(p) = provider
                && reg::provider(p).filter(|d| d.visible()).is_none()
            {
                return Err(CuaError::InvalidArgument(format!("unknown provider {p:?}")));
            }
            Ok(("agent_keys.check".into(), serde_json::json!({})))
        }
        ConnectionsCmd::List { .. } => Ok(("agent_keys.list".into(), serde_json::json!({}))),
        ConnectionsCmd::Remove { provider } => {
            // The names saved for it are known only from the list: `run` resolves them.
            reg::provider(provider)
                .filter(|p| p.visible())
                .ok_or_else(|| CuaError::InvalidArgument(format!("unknown provider {provider:?}")))?;
            Ok(("agent_keys.list".into(), serde_json::json!({})))
        }
    }
}

async fn call(cua: &Arc<Cua>, method: &str, args: serde_json::Value) -> Result<serde_json::Value, CuaError> {
    let r = cua.spaces().call_tool_json(method.into(), Some(args.to_string())).await?;
    if r.is_error {
        let msg = r.text.strip_prefix("error: ").unwrap_or(&r.text).to_string();
        return Err(CuaError::InvalidArgument(msg));
    }
    Ok(serde_json::from_str(&r.text).unwrap_or_default())
}

pub async fn run(
    cua: &Arc<Cua>,
    cmd: ConnectionsCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (method, args) = plan(&cmd, |required| {
        read_key(&mut std::io::stdin(), std::io::stdin().is_terminal(), required)
    })?;
    match cmd {
        ConnectionsCmd::List { all } => {
            let report = call(cua, &method, args).await?;
            let rows = rows(&report, all);
            if json {
                line(out, serde_json::json!({ "connections": rows, "available": report["available"] }).to_string());
            } else {
                for l in list_lines(&rows, &report) {
                    line(out, l);
                }
            }
            Ok(0)
        }
        ConnectionsCmd::Add { ref provider, .. } => {
            let report = call(cua, &method, args).await?;
            let row = rows(&report, true).into_iter().find(|r| &r.id == provider);
            if json {
                line(out, serde_json::json!({ "connection": row }).to_string());
            } else if let Some(r) = row {
                line(out, format!("{}: {}. It works with {}.", r.name, state_words(r.state), r.works_with.join(", ")));
                line(out, hint("Saved without asking the provider. Check it with: cua connections test"));
            }
            Ok(0)
        }
        ConnectionsCmd::Test { provider } => {
            // The daemon asks each provider (six seconds at most each); the whole is bounded here.
            let report = tokio::time::timeout(TEST_WAIT, call(cua, &method, args))
                .await
                .map_err(|_| CuaError::InvalidArgument("the providers did not answer in time; nothing was changed".into()))??;
            let mut shown = rows(&report, false);
            if let Some(p) = &provider {
                shown.retain(|r| &r.id == p);
            }
            shown.retain(|r| r.state != "not_connected");
            let bad = shown.iter().any(|r| r.state == "needs_reconnect");
            if json {
                line(out, serde_json::json!({ "connections": shown }).to_string());
            } else {
                if shown.is_empty() {
                    line(out, hint("Nothing to check: no provider is connected."));
                }
                for r in &shown {
                    line(out, format!("{:<22} {}", r.id, state_words(r.state)));
                }
            }
            Ok(if bad { 1 } else { 0 })
        }
        ConnectionsCmd::Remove { provider } => {
            let report = call(cua, &method, args).await?;
            let def = reg::provider(&provider).expect("checked by plan");
            let saved: BTreeSet<String> = report["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k["env"].as_str().map(str::to_string))
                .collect();
            let mine: Vec<&str> = def.envs().filter(|e| saved.contains(*e)).collect();
            if mine.is_empty() {
                line(out, format!("{} is not connected.", def.name));
                return Ok(0);
            }
            for env in mine {
                call(cua, "agent_keys.remove", serde_json::json!({ "env": env })).await?;
            }
            if json {
                line(out, serde_json::json!({ "removed": def.id }).to_string());
            } else {
                line(out, format!("Disconnected {}.", def.name));
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd_add(p: &str) -> ConnectionsCmd {
        ConnectionsCmd::Add { provider: p.into(), method: None, region: None, model: None, base_url: None, wire: None }
    }

    #[test]
    fn the_key_comes_only_from_a_pipe() {
        assert!(read_key(&mut "sk".as_bytes(), true, true).is_err());
        assert!(read_key(&mut "  \n".as_bytes(), false, true).is_err());
        assert_eq!(read_key(&mut "sk-test-0000\n".as_bytes(), false, true).unwrap(), "sk-test-0000");
        // A server that needs no key may be added from a terminal, with none.
        assert_eq!(read_key(&mut "".as_bytes(), true, false).unwrap(), "");
        assert_eq!(read_key(&mut "".as_bytes(), false, false).unwrap(), "");
    }

    #[test]
    fn add_sends_a_provider_a_key_and_only_the_choices_given() {
        let key = |_| Ok("sk-test-0000".to_string());
        let (m, a) = plan(&cmd_add("deepseek"), key).unwrap();
        assert_eq!(m, "agent_keys.set");
        assert_eq!(a, serde_json::json!({"provider": "deepseek", "value": "sk-test-0000"}));
        let mut z = cmd_add("zai");
        if let ConnectionsCmd::Add { region, .. } = &mut z {
            *region = Some("coding-global".into());
        }
        assert_eq!(plan(&z, key).unwrap().1, serde_json::json!({"provider": "zai", "value": "sk-test-0000", "region": "coding-global"}));
        // A custom endpoint: URL, model and wire as fields, and no key needed.
        let c = ConnectionsCmd::Add { provider: "custom".into(), method: None, region: None, model: Some("qwen".into()), base_url: Some("http://h:1234/v1".into()), wire: None };
        let (_, a) = plan(&c, |required| { assert!(!required); Ok(String::new()) }).unwrap();
        assert_eq!(a["fields"], serde_json::json!({"model": "qwen", "base_url": "http://h:1234/v1", "wire": "openai-chat"}));
        assert_eq!(a["value"], "");
        // --base-url is a custom endpoint's.
        let mut d = cmd_add("deepseek");
        if let ConnectionsCmd::Add { base_url, .. } = &mut d {
            *base_url = Some("http://x".into());
        }
        assert!(plan(&d, key).is_err());
    }

    #[test]
    fn unknown_hidden_and_sign_in_only_providers_are_refused_in_words() {
        let key = |_| Ok("k".to_string());
        let e = plan(&cmd_add("nope"), key).unwrap_err().to_string();
        assert!(e.contains("unknown provider") && e.contains("deepseek"), "{e}");
        // An unreviewed login and an unverified provider are not there to add.
        for hidden in ["nous", "xai-oauth", "qwen-oauth", "minimax-oauth", "vertex", "meta-ai"] {
            assert!(plan(&cmd_add(hidden), key).is_err(), "{hidden}");
        }
        let mut c = cmd_add("anthropic");
        if let ConnectionsCmd::Add { method, .. } = &mut c {
            *method = Some("import".into());
        }
        let e = plan(&c, key).unwrap_err().to_string();
        assert!(e.contains("cua connect setup <app> --sign-in"), "{e}");
        let mut c = cmd_add("deepseek");
        if let ConnectionsCmd::Add { method, .. } = &mut c {
            *method = Some("login".into());
        }
        assert!(plan(&c, key).unwrap_err().to_string().contains("can be connected with: key"));
    }

    #[test]
    fn list_says_the_state_in_the_apps_words_and_never_a_key() {
        let report = serde_json::json!({"available": true, "keys": [
            {"provider": "deepseek", "env": "DEEPSEEK_API_KEY", "last4": "9c2e", "added_ms": 1},
            {"provider": "openai", "env": "OPENAI_API_KEY", "last4": "9z9z", "added_ms": 1, "expired": true},
            {"provider": "zai", "env": "GLM_API_KEY", "last4": "7d1f", "added_ms": 1, "region": "coding-global"},
            {"provider": "anthropic", "env": "CLAUDE_CODE_OAUTH_TOKEN", "last4": "", "added_ms": 1}
        ]});
        let r = rows(&report, false);
        let state = |id: &str| r.iter().find(|x| x.id == id).map(|x| x.state);
        assert_eq!(state("deepseek"), Some("connected"));
        assert_eq!(state("openai"), Some("needs_reconnect"));
        assert_eq!(state("anthropic"), Some("connected"));
        assert_eq!(state("kimi"), Some("not_connected"));
        // More providers appear when something is saved, or with --all.
        assert_eq!(state("zai"), Some("connected"));
        assert_eq!(state("fireworks"), None);
        assert!(rows(&report, true).iter().any(|x| x.id == "fireworks"));
        let text = list_lines(&r, &report).join("\n");
        assert!(text.contains("Needs reconnect") && text.contains("Connected") && text.contains("Not connected"), "{text}");
        assert!(text.contains("Global, Coding Plan"), "{text}");
        assert!(text.contains("sign-in found on this machine"), "{text}");
        for leak in ["9c2e", "9z9z", "7d1f", "sk-"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        // Says machines, never devices, and no dot separators.
        assert!(!text.to_lowercase().contains("device") && !text.contains('\u{b7}'));
        // Hermes works with the DeepSeek key; Claude Code does not.
        let ds = r.iter().find(|x| x.id == "deepseek").unwrap();
        assert!(ds.works_with.contains(&"Hermes".to_string()) && !ds.works_with.contains(&"Claude Code".to_string()));
    }
}
