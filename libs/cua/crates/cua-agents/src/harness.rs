//! The harnesses: which installables each needs, how it speaks the Agent
//! Client Protocol, how it authenticates headlessly, and how it is pointed
//! at a custom endpoint. None of this parses a harness's output: every
//! harness is driven over ACP, natively or through its maintained adapter.

use serde::Serialize;
use serde_json::json;

/// A model endpoint's wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Wire {
    /// Anthropic Messages.
    Anthropic,
    /// OpenAI Responses.
    OpenaiResponses,
    /// OpenAI Chat Completions (and compatible servers).
    OpenaiChat,
    /// Gemini API.
    Gemini,
}

impl Wire {
    /// Parses `anthropic`, `openai-responses`, `openai-chat` or `gemini`.
    pub fn parse(s: &str) -> Option<Wire> {
        match s {
            "anthropic" => Some(Wire::Anthropic),
            "openai-responses" | "responses" => Some(Wire::OpenaiResponses),
            "openai-chat" | "openai" | "chat" => Some(Wire::OpenaiChat),
            "gemini" | "google" => Some(Wire::Gemini),
            _ => None,
        }
    }
}

/// One harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harness {
    /// Id (`claude-code`, `openai-codex`, ...).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// What to install (see `installables.json`), dependencies implied.
    pub installs: &'static [&'static str],
    /// The ACP agent: a binary in `~/.cua/bin` and its arguments.
    pub acp: &'static [&'static str],
    /// The interactive CLI a human uses in a terminal, when there is one.
    pub cli: Option<&'static str>,
    /// Credential env vars the harness reads, in preference order.
    pub keys: &'static [&'static str],
    /// ACP `authenticate` method used with an env key, when the agent
    /// requires one.
    pub auth_method: Option<&'static str>,
    /// Wire formats it can be pointed at with a base URL.
    pub wires: &'static [Wire],
    /// The agent-side mode to select (by ACP `_meta.kind` or id): the
    /// sandbox is the boundary, so a harness's own sandbox is switched off.
    pub mode_kinds: &'static [&'static str],
    /// The `cua-agent-setup` registry id whose skills directory it reads.
    pub skills_agent: Option<&'static str>,
    /// Whether runs get the sandbox's own MCP (cua-driver) by default.
    pub sandbox_mcp: bool,
    /// Supported by the SDK: installed, driven and proven live.
    pub ready: bool,
    /// What is known not to work, published with its capabilities.
    pub notes: &'static [&'static str],
}

const FULL: &[&str] = &["full_access", "bypassPermissions", "yolo"];

/// Every harness.
pub const HARNESSES: &[Harness] = &[
    Harness {
        id: "claude-code",
        name: "Claude Code",
        installs: &["claude-agent-acp", "claude-code"],
        acp: &["claude-agent-acp"],
        cli: Some("claude"),
        keys: &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_AUTH_TOKEN",
        ],
        auth_method: None,
        wires: &[Wire::Anthropic],
        mode_kinds: FULL,
        skills_agent: Some("claude-code"),
        sandbox_mcp: true,
        ready: true,
        notes: &[],
    },
    Harness {
        id: "openai-codex",
        name: "OpenAI Codex",
        installs: &["codex-acp"],
        acp: &["codex-acp"],
        cli: Some("codex"),
        keys: &["OPENAI_API_KEY", "CODEX_API_KEY"],
        auth_method: None,
        wires: &[Wire::OpenaiResponses],
        mode_kinds: FULL,
        skills_agent: Some("codex"),
        sandbox_mcp: true,
        ready: true,
        notes: &[],
    },
    Harness {
        id: "gemini-cli",
        name: "Gemini CLI",
        installs: &["gemini-cli"],
        acp: &["gemini", "--acp"],
        cli: Some("gemini"),
        keys: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        auth_method: Some("gemini-api-key"),
        wires: &[Wire::Gemini],
        mode_kinds: FULL,
        skills_agent: Some("gemini-cli"),
        sandbox_mcp: true,
        ready: true,
        notes: &[
            "Google stopped serving free Gemini CLI accounts on 2026-06-18; use a paid API key",
        ],
    },
    Harness {
        id: "google-antigravity",
        name: "Google Antigravity",
        installs: &["antigravity-acp"],
        acp: &["agy-acp-server", "--uid="],
        cli: None,
        keys: &["GEMINI_API_KEY"],
        auth_method: Some("gemini-api-key"),
        wires: &[Wire::Gemini],
        mode_kinds: FULL,
        skills_agent: Some("antigravity"),
        sandbox_mcp: true,
        ready: true,
        notes: &[
            "runs Google's ACP server (the IDE's agent) headless; there is no interactive terminal CLI",
            "the server is proprietary and about 1 GB; its sha256 is pinned by cua (Google publishes none)",
        ],
    },
    Harness {
        id: "opencode",
        name: "OpenCode",
        installs: &["opencode"],
        acp: &["opencode", "acp"],
        cli: Some("opencode"),
        keys: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        auth_method: None,
        wires: &[Wire::OpenaiChat],
        mode_kinds: FULL,
        skills_agent: Some("opencode"),
        sandbox_mcp: true,
        ready: true,
        notes: &[],
    },
    Harness {
        id: "goose",
        name: "Goose",
        installs: &["goose"],
        acp: &["goose", "acp"],
        cli: Some("goose"),
        keys: &[
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GOOGLE_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        auth_method: None,
        wires: &[Wire::OpenaiChat],
        mode_kinds: FULL,
        skills_agent: Some("goose"),
        sandbox_mcp: true,
        ready: true,
        notes: &[],
    },
    Harness {
        id: "pi",
        name: "Pi",
        installs: &["pi"],
        acp: &["pi-acp"],
        cli: Some("pi"),
        keys: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        auth_method: None,
        wires: &[Wire::OpenaiChat],
        mode_kinds: FULL,
        skills_agent: Some("pi"),
        sandbox_mcp: false,
        ready: true,
        notes: &[
            "Pi has no MCP client (by design); MCP servers passed to a run are ignored",
            "driven through the community pi-acp adapter (MIT), not a first-party ACP server",
        ],
    },
    Harness {
        id: "hermes",
        name: "Hermes",
        installs: &["hermes"],
        acp: &["hermes-acp"],
        cli: Some("hermes"),
        keys: &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        auth_method: None,
        wires: &[Wire::OpenaiChat],
        mode_kinds: FULL,
        skills_agent: None,
        sandbox_mcp: false,
        ready: true,
        notes: &[
            "hermes-acp does not take ACP session MCP servers; configure MCP in HERMES_HOME's config.yaml",
            "installed from a pinned commit with uv sync --frozen (Hermes ships no wheel); needs git and Python 3.11 to 3.13 or uv's managed Python",
            "cua skills are not copied into ~/.hermes/skills",
        ],
    },
    Harness {
        id: "openclaw",
        name: "OpenClaw",
        installs: &["openclaw"],
        acp: &["openclaw", "acp"],
        cli: Some("openclaw"),
        keys: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"],
        auth_method: None,
        wires: &[Wire::OpenaiChat],
        mode_kinds: FULL,
        skills_agent: Some("openclaw"),
        sandbox_mcp: false,
        ready: true,
        notes: &[
            "openclaw acp (bridge mode) refuses per-session MCP servers; configure MCP on the OpenClaw gateway",
            "openclaw acp bridges to a Gateway; the run starts a loopback Gateway for itself",
        ],
    },
];

/// A harness by id.
pub fn harness(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.id == id)
}

/// The env var an endpoint's key is read from, for `h` and `wire`.
pub fn endpoint_key_var(h: &Harness, wire: Option<Wire>) -> &'static str {
    match wire.unwrap_or(h.wires[0]) {
        Wire::Anthropic => "ANTHROPIC_API_KEY",
        Wire::Gemini => "GEMINI_API_KEY",
        Wire::OpenaiChat | Wire::OpenaiResponses => "OPENAI_API_KEY",
    }
}

/// Ids of every harness, sorted.
pub fn ids() -> Vec<&'static str> {
    let mut v: Vec<_> = HARNESSES.iter().map(|h| h.id).collect();
    v.sort_unstable();
    v
}

/// Ids of the ready harnesses, sorted.
pub fn ready() -> Vec<&'static str> {
    let mut v: Vec<_> = HARNESSES.iter().filter(|h| h.ready).map(|h| h.id).collect();
    v.sort_unstable();
    v
}

/// A custom model endpoint (a proxy, a gateway, a compatible server).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Endpoint {
    /// Base URL, for example `https://proxy.example/v1`.
    pub base_url: String,
    /// Wire format; default: the harness's first.
    #[serde(default)]
    pub wire: Option<Wire>,
    /// Model id; default: the harness's own default.
    #[serde(default)]
    pub model: Option<String>,
}

/// A file the SDK writes into the run directory before launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFile {
    /// Path relative to the run directory, or an absolute guest path (a
    /// persistent agent home).
    pub rel: String,
    pub body: String,
}

/// Everything a harness needs at launch besides the prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// Env for the agent (never secrets: those go to `secrets.json`).
    pub env: Vec<(String, String)>,
    /// Extra ACP-agent arguments.
    pub args: Vec<String>,
    /// Config files, relative to the run directory.
    pub files: Vec<ConfigFile>,
    /// A wrapper that starts the agent (OpenClaw's Gateway).
    pub wrapper: Option<String>,
    /// Shell lines the launcher runs before it starts the agent (linking a
    /// signed-in harness's credentials into a relocated config dir).
    pub prelude: Vec<String>,
}

fn trim_v1(url: &str) -> &str {
    url.trim_end_matches('/').trim_end_matches("/v1")
}

fn v1(url: &str) -> String {
    format!("{}/v1", trim_v1(url))
}

/// The env, arguments and files that point `h` at `endpoint` (when given)
/// and select a model, for a run whose directory is `run_dir`.
pub fn launch(
    h: &Harness,
    endpoint: Option<&Endpoint>,
    model: Option<&str>,
    run_dir: &str,
    cwd: &str,
) -> crate::Result<Launch> {
    launch_with_home(h, endpoint, model, run_dir, cwd, None)
}

/// Where each harness keeps its memory inside a persistent agent home, so
/// what the agent learns outlives the run and the Space. Paths are relative
/// to the home.
///
/// | Harness | Memory | How it gets there |
/// |---|---|---|
/// | Claude Code | `claude-memory/` | `autoMemoryDirectory` in a per-run `CLAUDE_CONFIG_DIR` |
/// | Codex | `codex/memories/` | `CODEX_HOME=<home>/codex` with `[features] memories = true` |
/// | Hermes | `hermes/memories/` | `HERMES_HOME=<home>/hermes` |
/// | OpenClaw | `workspace/` (`MEMORY.md`, `memory/`) | `OPENCLAW_WORKSPACE_DIR=<home>/workspace` |
///
/// Every harness also works in `<home>/work` unless the caller names a
/// working directory.
pub fn memory_dir(h: &Harness) -> Option<&'static str> {
    match h.id {
        "claude-code" => Some("claude-memory"),
        "openai-codex" => Some("codex/memories"),
        "hermes" => Some("hermes/memories"),
        "openclaw" => Some("workspace"),
        _ => None,
    }
}

/// [`launch`] for a persistent agent whose home is the guest directory
/// `home` (see [`memory_dir`]).
pub fn launch_with_home(
    h: &Harness,
    endpoint: Option<&Endpoint>,
    model: Option<&str>,
    run_dir: &str,
    cwd: &str,
    home: Option<&str>,
) -> crate::Result<Launch> {
    let mut l = Launch::default();
    let model = endpoint
        .and_then(|e| e.model.clone())
        .or_else(|| model.map(str::to_string));
    if let Some(e) = endpoint {
        let wire = e.wire.unwrap_or(h.wires[0]);
        if !h.wires.contains(&wire) {
            return Err(crate::Error::Invalid(format!(
                "{} cannot use a {wire:?} endpoint (it speaks {:?})",
                h.name, h.wires
            )));
        }
    }
    let base = endpoint.map(|e| e.base_url.trim_end_matches('/').to_string());
    let env = |l: &mut Launch, k: &str, v: String| l.env.push((k.into(), v));
    match h.id {
        "claude-code" => {
            if let Some(b) = &base {
                env(&mut l, "ANTHROPIC_BASE_URL", trim_v1(b).to_string());
            }
            if let Some(m) = &model {
                env(&mut l, "ANTHROPIC_MODEL", m.clone());
            }
            if let Some(home) = home {
                // Only memory moves: a per-run config dir points auto memory
                // at the home, and borrows the guest's sign-in and skills.
                let cfg = format!("{run_dir}/claude-config");
                l.files.push(ConfigFile {
                    rel: "claude-config/settings.json".into(),
                    body: serde_json::to_string_pretty(
                        &json!({"autoMemoryDirectory": format!("{home}/claude-memory")}),
                    )?,
                });
                let q = crate::quote;
                l.prelude.extend([
                    format!("mkdir -p {}", q(&format!("{home}/claude-memory"))),
                    format!(
                        "[ -e \"$HOME/.claude/.credentials.json\" ] && ln -sf \"$HOME/.claude/.credentials.json\" {}",
                        q(&format!("{cfg}/.credentials.json"))
                    ),
                    format!(
                        "[ -e \"$HOME/.claude.json\" ] && [ ! -e {c} ] && cp \"$HOME/.claude.json\" {c}",
                        c = q(&format!("{cfg}/.claude.json"))
                    ),
                    format!(
                        "[ -d \"$HOME/.claude/skills\" ] && ln -sfn \"$HOME/.claude/skills\" {}",
                        q(&format!("{cfg}/skills"))
                    ),
                    "true".into(),
                ]);
                env(&mut l, "CLAUDE_CONFIG_DIR", cfg);
            }
        }
        "openai-codex" => {
            // With a home, CODEX_HOME is the home's `codex/` with memories
            // on; without one, a run-local CODEX_HOME holds only the provider
            // override.
            let codex_home = match home {
                Some(h) => Some(format!("{h}/codex")),
                None => base.as_ref().map(|_| format!("{run_dir}/codex-home")),
            };
            let mut toml = String::new();
            if let Some(b) = &base {
                let m = model.clone().unwrap_or_else(|| "gpt-5-codex".into());
                toml.push_str(&format!(
                    "model = {m:?}\nmodel_provider = \"cua\"\n\n[model_providers.cua]\n\
                     name = \"custom endpoint\"\nbase_url = {:?}\nenv_key = \"OPENAI_API_KEY\"\n\
                     wire_api = \"responses\"\n",
                    v1(b)
                ));
            } else if let Some(m) = &model {
                l.args.extend(["-c".into(), format!("model={m:?}")]);
            }
            if home.is_some() {
                if !toml.is_empty() {
                    toml.push('\n');
                }
                toml.push_str("[features]\nmemories = true\n");
            }
            if let Some(dir) = codex_home {
                let rel = match home {
                    Some(_) => format!("{dir}/config.toml"),
                    None => "codex-home/config.toml".into(),
                };
                l.files.push(ConfigFile { rel, body: toml });
                if home.is_some() {
                    // A signed-in guest keeps its login; auth.json is never
                    // synced to the drive.
                    l.prelude.push(format!(
                        "[ -e \"$HOME/.codex/auth.json\" ] && [ ! -e {a} ] && ln -s \"$HOME/.codex/auth.json\" {a}; true",
                        a = crate::quote(&format!("{dir}/auth.json"))
                    ));
                }
                env(&mut l, "CODEX_HOME", dir);
            }
        }
        "gemini-cli" | "google-antigravity" => {
            if h.id == "gemini-cli" {
                // The sandbox is the boundary; an untrusted folder would
                // disable the run's MCP servers and full-access mode.
                env(&mut l, "GEMINI_CLI_TRUST_WORKSPACE", "true".into());
            }
            if let Some(b) = &base {
                env(&mut l, "GOOGLE_GEMINI_BASE_URL", trim_v1(b).to_string());
            }
            if h.id == "gemini-cli"
                && let Some(m) = &model
            {
                l.args.extend(["--model".into(), m.clone()]);
            }
        }
        "opencode" => {
            if let Some(b) = &base {
                let m = model.clone().unwrap_or_else(|| "default".into());
                let cfg = json!({
                    "$schema": "https://opencode.ai/config.json",
                    "provider": {"cua": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "custom endpoint",
                        "options": {"baseURL": v1(b), "apiKey": "{env:OPENAI_API_KEY}"},
                        "models": {m.clone(): {"name": m, "tool_call": true}}}},
                    "model": format!("cua/{m}"),
                    "autoupdate": false,
                    "share": "disabled",
                });
                l.files.push(ConfigFile {
                    rel: "xdg/opencode/opencode.json".into(),
                    body: serde_json::to_string_pretty(&cfg)?,
                });
                env(&mut l, "XDG_CONFIG_HOME", format!("{run_dir}/xdg"));
            }
        }
        "goose" => {
            env(&mut l, "GOOSE_MODE", "auto".into());
            env(&mut l, "GOOSE_DISABLE_KEYRING", "1".into());
            if let Some(b) = &base {
                env(&mut l, "GOOSE_PROVIDER", "openai".into());
                env(&mut l, "OPENAI_HOST", trim_v1(b).to_string());
            }
            if let Some(m) = &model {
                env(&mut l, "GOOSE_MODEL", m.clone());
            }
        }
        "pi" => {
            if let Some(b) = &base {
                let m = model.clone().unwrap_or_else(|| "default".into());
                let models = json!({"providers": {"cua": {"baseUrl": v1(b),
                    "api": "openai-completions", "apiKey": "${OPENAI_API_KEY}",
                    "models": [{"id": m}]}}});
                let settings =
                    json!({"defaultProvider": "cua", "defaultModel": m, "quietStartup": true});
                l.files.push(ConfigFile {
                    rel: "pi-agent/models.json".into(),
                    body: models.to_string(),
                });
                l.files.push(ConfigFile {
                    rel: "pi-agent/settings.json".into(),
                    body: settings.to_string(),
                });
                env(&mut l, "PI_CODING_AGENT_DIR", format!("{run_dir}/pi-agent"));
                env(&mut l, "PI_OFFLINE", "1".into());
            }
        }
        "hermes" => {
            let hermes_home = match home {
                Some(h) => Some(format!("{h}/hermes")),
                None => base.as_ref().map(|_| format!("{run_dir}/hermes-home")),
            };
            if let Some(b) = &base {
                let m = model.clone().unwrap_or_else(|| "default".into());
                let dir = hermes_home.clone().expect("set with an endpoint");
                l.files.push(ConfigFile {
                    rel: match home {
                        Some(_) => format!("{dir}/config.yaml"),
                        None => "hermes-home/config.yaml".into(),
                    },
                    body: format!(
                        "model:\n  default: {m:?}\n  provider: custom\n  base_url: {:?}\n  \
                         api_key: ${{OPENAI_API_KEY}}\n  context_length: 131072\n",
                        v1(b)
                    ),
                });
            } else if let Some(dir) = &hermes_home {
                // No endpoint: start from the guest's own Hermes setup.
                let q = crate::quote;
                l.prelude.extend([
                    format!("mkdir -p {}", q(dir)),
                    format!(
                        "[ -e \"$HOME/.hermes/config.yaml\" ] && [ ! -e {c} ] && cp \"$HOME/.hermes/config.yaml\" {c}; true",
                        c = q(&format!("{dir}/config.yaml"))
                    ),
                    format!(
                        "[ -e \"$HOME/.hermes/.env\" ] && [ ! -e {c} ] && ln -s \"$HOME/.hermes/.env\" {c}; true",
                        c = q(&format!("{dir}/.env"))
                    ),
                ]);
            }
            if let Some(dir) = hermes_home {
                env(&mut l, "HERMES_HOME", dir);
            }
        }
        "openclaw" => {
            let workspace = home.map_or_else(|| cwd.to_string(), |h| format!("{h}/workspace"));
            let mut cfg = json!({"gateway": {"mode": "local", "port": 18789,
                "bind": "loopback", "auth": {"mode": "none"}},
                "agents": {"defaults": {"workspace": workspace}}});
            if home.is_some() {
                env(&mut l, "OPENCLAW_WORKSPACE_DIR", workspace.clone());
            }
            if let Some(b) = &base {
                let m = model.clone().unwrap_or_else(|| "default".into());
                cfg["models"] = json!({"providers": {"cua": {"baseUrl": v1(b),
                    "apiKey": "${OPENAI_API_KEY}", "api": "openai-completions",
                    "models": [{"id": m, "name": m, "reasoning": false, "input": ["text"],
                                "contextWindow": 128000, "maxTokens": 4096}]}}});
                cfg["agents"]["defaults"]["model"] = json!({"primary": format!("cua/{m}")});
            }
            l.files.push(ConfigFile {
                rel: "openclaw/openclaw.json".into(),
                body: serde_json::to_string_pretty(&cfg)?,
            });
            env(&mut l, "OPENCLAW_STATE_DIR", format!("{run_dir}/openclaw"));
            env(
                &mut l,
                "OPENCLAW_CONFIG_PATH",
                format!("{run_dir}/openclaw/openclaw.json"),
            );
            l.wrapper = Some(OPENCLAW_WRAPPER.into());
        }
        _ => {}
    }
    Ok(l)
}

/// Starts a loopback Gateway for `openclaw acp` and stops it with the agent.
const OPENCLAW_WRAPPER: &str = r#"#!/bin/sh
"$HOME/.cua/bin/openclaw" gateway --port 18789 --auth none > "$OPENCLAW_STATE_DIR/gateway.log" 2>&1 &
gw=$!
trap 'kill $gw 2>/dev/null' EXIT INT TERM
i=0
while [ $i -lt 240 ]; do
  if command -v nc >/dev/null 2>&1; then nc -z 127.0.0.1 18789 2>/dev/null && break
  else (echo > /dev/tcp/127.0.0.1/18789) 2>/dev/null && break; fi
  i=$((i+1)); sleep 0.5
done
"$HOME/.cua/bin/openclaw" acp --url ws://127.0.0.1:18789
"#;

/// Published capabilities of one harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub ready: bool,
    pub protocol: &'static str,
    pub sandbox_mcp: bool,
    pub installs: Vec<String>,
    pub keys: &'static [&'static str],
    pub endpoint_wires: &'static [Wire],
    pub interactive_cli: Option<&'static str>,
    pub can_interrupt: bool,
    pub followups: bool,
    pub notes: &'static [&'static str],
}

impl Harness {
    /// Its published capabilities.
    pub fn info(&self) -> HarnessInfo {
        HarnessInfo {
            id: self.id,
            name: self.name,
            ready: self.ready,
            protocol: "acp",
            sandbox_mcp: self.sandbox_mcp,
            installs: crate::installables::resolve(self.installs).unwrap_or_default(),
            keys: self.keys,
            endpoint_wires: self.wires,
            interactive_cli: self.cli,
            can_interrupt: true,
            followups: true,
            notes: self.notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_installs_known_items_and_links_its_acp_binary() {
        for h in HARNESSES {
            let order = crate::installables::resolve(h.installs).unwrap();
            let bins: Vec<String> = order
                .iter()
                .flat_map(|id| crate::installables::get(id).unwrap().bins.keys().cloned())
                .collect();
            assert!(bins.iter().any(|b| b == h.acp[0]), "{}: {bins:?}", h.id);
            if let Some(cli) = h.cli {
                assert!(bins.iter().any(|b| b == cli), "{}: cli {cli}", h.id);
            }
            if let Some(a) = h.skills_agent {
                assert!(cua_agent_setup::registry::find(a).is_some(), "{a}");
            }
        }
    }

    #[test]
    fn endpoints_are_applied_per_wire_and_refused_on_the_wrong_one() {
        let e = Endpoint {
            base_url: "http://127.0.0.1:8787/v1".into(),
            wire: None,
            model: Some("m1".into()),
        };
        let c = launch(harness("claude-code").unwrap(), Some(&e), None, "/r", "/w").unwrap();
        assert!(
            c.env
                .contains(&("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:8787".into()))
        );
        let x = launch(harness("openai-codex").unwrap(), Some(&e), None, "/r", "/w").unwrap();
        assert!(
            x.files[0]
                .body
                .contains("base_url = \"http://127.0.0.1:8787/v1\"")
        );
        assert!(
            x.env
                .contains(&("CODEX_HOME".into(), "/r/codex-home".into()))
        );
        let bad = Endpoint {
            wire: Some(Wire::Gemini),
            ..e
        };
        assert!(
            launch(
                harness("claude-code").unwrap(),
                Some(&bad),
                None,
                "/r",
                "/w"
            )
            .is_err()
        );
    }
    #[test]
    fn a_home_holds_each_harness_memory() {
        let home = "/home/cua/cua-volume/agents/ada";
        let has = |l: &Launch, k: &str, v: &str| l.env.contains(&(k.to_string(), v.to_string()));
        let c = launch_with_home(
            harness("claude-code").unwrap(),
            None,
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        assert!(has(&c, "CLAUDE_CONFIG_DIR", "/r/claude-config"));
        assert_eq!(c.files[0].rel, "claude-config/settings.json");
        assert!(c.files[0].body.contains(
            "\"autoMemoryDirectory\": \"/home/cua/cua-volume/agents/ada/claude-memory\""
        ));
        let x = launch_with_home(
            harness("openai-codex").unwrap(),
            None,
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        assert!(has(
            &x,
            "CODEX_HOME",
            "/home/cua/cua-volume/agents/ada/codex"
        ));
        assert_eq!(
            x.files[0].rel,
            "/home/cua/cua-volume/agents/ada/codex/config.toml"
        );
        assert_eq!(x.files[0].body, "[features]\nmemories = true\n");
        let e = Endpoint {
            base_url: "http://h:1/v1".into(),
            wire: None,
            model: Some("m".into()),
        };
        let x = launch_with_home(
            harness("openai-codex").unwrap(),
            Some(&e),
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        let body = &x.files[0].body;
        assert!(
            body.starts_with("model = \"m\""),
            "top-level keys come before tables: {body}"
        );
        assert!(body.ends_with("[features]\nmemories = true\n"), "{body}");
        let hm = launch_with_home(
            harness("hermes").unwrap(),
            Some(&e),
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        assert!(has(
            &hm,
            "HERMES_HOME",
            "/home/cua/cua-volume/agents/ada/hermes"
        ));
        assert_eq!(
            hm.files[0].rel,
            "/home/cua/cua-volume/agents/ada/hermes/config.yaml"
        );
        let hm = launch_with_home(
            harness("hermes").unwrap(),
            None,
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        assert!(has(
            &hm,
            "HERMES_HOME",
            "/home/cua/cua-volume/agents/ada/hermes"
        ));
        assert!(hm.files.is_empty());
        let oc = launch_with_home(
            harness("openclaw").unwrap(),
            None,
            None,
            "/r",
            "/w",
            Some(home),
        )
        .unwrap();
        assert!(has(
            &oc,
            "OPENCLAW_WORKSPACE_DIR",
            "/home/cua/cua-volume/agents/ada/workspace"
        ));
        assert!(
            oc.files[0]
                .body
                .contains("/home/cua/cua-volume/agents/ada/workspace")
        );
        // Without a home nothing moves.
        let c = launch(harness("claude-code").unwrap(), None, None, "/r", "/w").unwrap();
        assert!(c.env.is_empty() && c.files.is_empty() && c.prelude.is_empty());
        let x = launch(harness("openai-codex").unwrap(), None, None, "/r", "/w").unwrap();
        assert!(x.env.is_empty() && x.files.is_empty());
        // Every prelude is valid POSIX sh.
        for id in ["claude-code", "openai-codex", "hermes", "openclaw"] {
            let l =
                launch_with_home(harness(id).unwrap(), None, None, "/r", "/w", Some(home)).unwrap();
            let out = std::process::Command::new("/bin/sh")
                .arg("-n")
                .arg("-c")
                .arg(l.prelude.join("\n"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{id}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(memory_dir(harness(id).unwrap()).is_some());
        }
    }
}
