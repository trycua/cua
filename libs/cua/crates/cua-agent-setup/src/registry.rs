//! The agent registry: where each AI coding agent keeps its user-scope
//! skills and MCP servers. Paths and key names come from each agent's
//! official docs (sources in `README.md`, checked 2026-09-22); fields marked
//! `unverified` are best effort.

use crate::{HostEnv, McpServer, edit::Fields, edit::Format};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The root a relative path hangs off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// `$HOME`.
    Home,
    /// `$XDG_CONFIG_HOME`, else `~/.config` (every OS: the agents below use
    /// `~/.config` on macOS too).
    Config,
    /// The OS app-data dir: `~/Library/Application Support` (macOS),
    /// `%APPDATA%` (Windows), `$XDG_CONFIG_HOME` or `~/.config` (Linux).
    AppData,
}

/// A path, optionally overridden by an environment variable that names a
/// directory (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, ...).
#[derive(Clone, Copy, Debug)]
pub struct Loc {
    /// `(VAR, path under $VAR)`, used when `VAR` is set and non-empty.
    pub env: Option<(&'static str, &'static str)>,
    /// Default root.
    pub base: Base,
    /// Default path under `base` (`/`-separated).
    pub rel: &'static str,
    /// On Windows, the default is this path under `%LOCALAPPDATA%` (else
    /// `~/AppData/Local`) instead (Hermes keeps its home there).
    pub windows_local: Option<&'static str>,
}

impl Loc {
    const fn home(rel: &'static str) -> Self {
        Loc {
            env: None,
            base: Base::Home,
            rel,
            windows_local: None,
        }
    }
    const fn config(rel: &'static str) -> Self {
        Loc {
            env: None,
            base: Base::Config,
            rel,
            windows_local: None,
        }
    }
    const fn app_data(rel: &'static str) -> Self {
        Loc {
            env: None,
            base: Base::AppData,
            rel,
            windows_local: None,
        }
    }
    const fn env(self, var: &'static str, under: &'static str) -> Self {
        Loc {
            env: Some((var, under)),
            ..self
        }
    }

    const fn windows_local(self, rel: &'static str) -> Self {
        Loc {
            windows_local: Some(rel),
            ..self
        }
    }

    /// The concrete path on `env`.
    pub fn resolve(&self, env: &HostEnv) -> PathBuf {
        if let Some((var, under)) = self.env
            && let Some(v) = env.var(var)
        {
            return join(PathBuf::from(v), under);
        }
        if let (Some(rel), crate::Os::Windows) = (self.windows_local, env.os) {
            let local = env
                .var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| env.home.join("AppData/Local"));
            return join(local, rel);
        }
        let root = match self.base {
            Base::Home => env.home.clone(),
            Base::Config => env
                .var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| env.home.join(".config")),
            Base::AppData => match env.os {
                crate::Os::MacOs => env.home.join("Library/Application Support"),
                crate::Os::Windows => env
                    .var("APPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| env.home.join("AppData/Roaming")),
                crate::Os::Linux => env
                    .var("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| env.home.join(".config")),
            },
        };
        join(root, self.rel)
    }
}

fn join(mut p: PathBuf, rel: &str) -> PathBuf {
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

/// How the cua entry is shaped in an agent's MCP config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `{command, args, env?}`.
    Plain,
    /// `{type, command, args, env?}` (Claude Code, Cursor, VS Code).
    Typed(&'static str),
    /// `{command, args, env?, disabled: false}` (Cline, Kiro).
    WithDisabled,
    /// Copilot CLI: `{type: "local", command, args, env?, tools: ["*"]}`.
    Copilot,
    /// OpenCode: `{type: "local", command: [cmd, ...args], environment?, enabled: true}`.
    OpenCode,
    /// Goose: `{type: "stdio", name, cmd, args, enabled, envs, timeout}`.
    Goose,
}

impl Shape {
    /// The entry fields for `server`, in the order they are written.
    pub fn fields(self, s: &McpServer) -> Fields {
        let mut f: Fields = Vec::new();
        let mut push = |k: &str, v: Value| f.push((k.to_string(), v));
        let env = || json!(s.env);
        let has_env = !s.env.is_empty();
        match self {
            Shape::Plain | Shape::WithDisabled | Shape::Typed(_) | Shape::Copilot => {
                match self {
                    Shape::Typed(t) => push("type", json!(t)),
                    Shape::Copilot => push("type", json!("local")),
                    _ => {}
                }
                push("command", json!(s.command));
                push("args", json!(s.args));
                if has_env {
                    push("env", env());
                }
                match self {
                    Shape::WithDisabled => push("disabled", json!(false)),
                    Shape::Copilot => push("tools", json!(["*"])),
                    _ => {}
                }
            }
            Shape::OpenCode => {
                push("type", json!("local"));
                let mut cmd = vec![s.command.clone()];
                cmd.extend(s.args.iter().cloned());
                push("command", json!(cmd));
                if has_env {
                    push("environment", env());
                }
                push("enabled", json!(true));
            }
            Shape::Goose => {
                push("type", json!("stdio"));
                push("name", json!(s.name));
                push("cmd", json!(s.command));
                push("args", json!(s.args));
                push("enabled", json!(true));
                push("envs", env());
                push("timeout", json!(300));
            }
        }
        f
    }

    /// The launch command and args of an existing entry of this shape.
    pub fn command_of(self, v: &Value) -> Option<(String, Vec<String>)> {
        let strs = |a: &Value| -> Vec<String> {
            a.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        match self {
            Shape::OpenCode => {
                let mut c = strs(&v["command"]);
                if c.is_empty() {
                    return None;
                }
                let head = c.remove(0);
                Some((head, c))
            }
            Shape::Goose => Some((v["cmd"].as_str()?.to_string(), strs(&v["args"]))),
            _ => Some((v["command"].as_str()?.to_string(), strs(&v["args"]))),
        }
    }
}

/// An agent CLI that owns its config file and should write it instead of
/// cua (the file also holds the agent's live state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerCli {
    /// `claude mcp add-json --scope user <name> <json>` /
    /// `claude mcp remove --scope user <name>` (writes `~/.claude.json`).
    Claude,
}

impl OwnerCli {
    /// The binary.
    pub fn bin(self) -> &'static str {
        match self {
            OwnerCli::Claude => "claude",
        }
    }
    /// Argv that adds `name` with entry `json`.
    pub fn add_args(self, name: &str, json: &str) -> Vec<String> {
        match self {
            OwnerCli::Claude => ["mcp", "add-json", "--scope", "user", name, json]
                .map(str::to_string)
                .to_vec(),
        }
    }
    /// Argv that removes `name`.
    pub fn remove_args(self, name: &str) -> Vec<String> {
        match self {
            OwnerCli::Claude => ["mcp", "remove", "--scope", "user", name]
                .map(str::to_string)
                .to_vec(),
        }
    }
}

/// Where an agent keeps its MCP servers.
#[derive(Clone, Copy, Debug)]
pub struct McpSpec {
    /// Config file.
    pub file: Loc,
    /// Alternatives used instead of `file` when they exist (first match),
    /// for example `opencode.jsonc`.
    pub alt_files: &'static [Loc],
    /// Syntax.
    pub format: Format,
    /// Key path of the servers object.
    pub key_path: &'static [&'static str],
    /// Entry shape.
    pub shape: Shape,
    /// Preferred writer, when its binary is on PATH.
    pub owner_cli: Option<OwnerCli>,
}

/// One agent.
#[derive(Clone, Copy, Debug)]
pub struct AgentSpec {
    /// Stable id (`claude-code`, `codex`, ...).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Other accepted ids (`claude`, `openai-codex`, ...).
    pub aliases: &'static [&'static str],
    /// Binaries whose presence on PATH means installed.
    pub bins: &'static [&'static str],
    /// Files or directories whose presence means installed.
    pub markers: &'static [Loc],
    /// macOS app bundles (`Cursor.app`) in /Applications or ~/Applications.
    pub apps: &'static [&'static str],
    /// User-scope skills directory, when the agent supports Agent Skills.
    pub skills: Option<Loc>,
    /// Other user-scope skills directories the agent also loads (from its
    /// docs). A cua skill already served from one of these (for example
    /// `~/.claude/skills` when Claude Code is set up too) is not copied
    /// into `skills` again, so the agent does not list it twice.
    pub also_reads: &'static [Loc],
    /// User-scope MCP config, when the agent supports MCP.
    pub mcp: Option<McpSpec>,
    /// What could not be verified from official docs (empty when all was).
    pub unverified: &'static str,
}

/// The cross-agent user skills directory (agentskills.io convention) read
/// by Codex, Cursor, Gemini CLI, OpenClaw, OpenCode, Pi, Windsurf/Devin,
/// Copilot CLI, Amp, Goose, Zed and VS Code. One copy serves them all.
const AGENTS_SKILLS: Loc = Loc::home(".agents/skills");
/// Claude Code's default user skills directory, which Cursor, OpenCode and
/// VS Code also load (their skills docs, checked 2026-09-23).
const CLAUDE_SKILLS: Loc = Loc::home(".claude/skills");

/// Every supported agent.
pub static AGENTS: &[AgentSpec] = &[
    AgentSpec {
        id: "claude-code",
        name: "Claude Code",
        aliases: &["claude"],
        bins: &["claude"],
        markers: &[
            Loc::home(".claude").env("CLAUDE_CONFIG_DIR", ""),
            Loc::home(".claude.json"),
        ],
        apps: &[],
        skills: Some(Loc::home(".claude/skills").env("CLAUDE_CONFIG_DIR", "skills")),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".claude.json").env("CLAUDE_CONFIG_DIR", ".claude.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Typed("stdio"),
            owner_cli: Some(OwnerCli::Claude),
        }),
        unverified: "the .claude.json location under CLAUDE_CONFIG_DIR",
    },
    AgentSpec {
        id: "codex",
        name: "OpenAI Codex",
        aliases: &["openai-codex"],
        bins: &["codex"],
        markers: &[Loc::home(".codex").env("CODEX_HOME", "")],
        apps: &["Codex.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".codex/config.toml").env("CODEX_HOME", "config.toml"),
            alt_files: &[],
            format: Format::Toml,
            key_path: &["mcp_servers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "the Codex.app bundle name",
    },
    AgentSpec {
        id: "cursor",
        name: "Cursor",
        aliases: &[],
        bins: &["cursor", "cursor-agent"],
        markers: &[Loc::home(".cursor")],
        apps: &["Cursor.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[CLAUDE_SKILLS, Loc::home(".codex/skills")],
        mcp: Some(McpSpec {
            file: Loc::home(".cursor/mcp.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Typed("stdio"),
            owner_cli: None,
        }),
        unverified: "the Cursor.app bundle name",
    },
    AgentSpec {
        id: "gemini-cli",
        name: "Gemini CLI",
        aliases: &["gemini"],
        bins: &["gemini"],
        markers: &[
            Loc::home(".gemini/settings.json").env("GEMINI_CLI_HOME", ".gemini/settings.json")
        ],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".gemini/settings.json")
                .env("GEMINI_CLI_HOME", ".gemini/settings.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "",
    },
    AgentSpec {
        id: "cline",
        name: "Cline",
        aliases: &[],
        bins: &["cline"],
        markers: &[Loc::home(".cline")],
        apps: &[],
        skills: Some(Loc::home(".cline/skills")),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".cline/data/settings/cline_mcp_settings.json")
                .env("CLINE_DATA_DIR", "settings/cline_mcp_settings.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::WithDisabled,
            owner_cli: None,
        }),
        unverified: "Cline's docs also name ~/.cline/mcp.json for the CLI; the VS Code extension's globalStorage file is not configured",
    },
    AgentSpec {
        id: "kiro",
        name: "Kiro",
        aliases: &[],
        bins: &["kiro-cli", "kiro"],
        markers: &[Loc::home(".kiro")],
        apps: &["Kiro.app"],
        skills: Some(Loc::home(".kiro/skills")),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".kiro/settings/mcp.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::WithDisabled,
            owner_cli: None,
        }),
        unverified: "the Kiro.app bundle name",
    },
    AgentSpec {
        id: "openclaw",
        name: "OpenClaw",
        aliases: &[],
        bins: &["openclaw"],
        markers: &[Loc::home(".openclaw").env("OPENCLAW_STATE_DIR", "")],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".openclaw/openclaw.json").env("OPENCLAW_STATE_DIR", "openclaw.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcp", "servers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "OPENCLAW_CONFIG_PATH is not honored; a running Gateway may need `openclaw mcp reload`",
    },
    AgentSpec {
        id: "opencode",
        name: "OpenCode",
        aliases: &[],
        bins: &["opencode"],
        markers: &[Loc::config("opencode").env("OPENCODE_CONFIG_DIR", "")],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[CLAUDE_SKILLS],
        mcp: Some(McpSpec {
            file: Loc::config("opencode/opencode.json").env("OPENCODE_CONFIG_DIR", "opencode.json"),
            alt_files: &[
                Loc::config("opencode/opencode.jsonc").env("OPENCODE_CONFIG_DIR", "opencode.jsonc")
            ],
            format: Format::Json,
            key_path: &["mcp"],
            shape: Shape::OpenCode,
            owner_cli: None,
        }),
        unverified: "XDG_CONFIG_HOME handling",
    },
    AgentSpec {
        id: "pi",
        name: "Pi",
        aliases: &["pi-coding-agent"],
        bins: &["pi"],
        markers: &[Loc::home(".pi/agent").env("PI_CODING_AGENT_DIR", "")],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        // Pi has no MCP support by design (pi.dev: "No MCP").
        mcp: None,
        unverified: "",
    },
    AgentSpec {
        id: "windsurf",
        name: "Windsurf (Devin Desktop)",
        aliases: &["devin", "devin-desktop"],
        bins: &["windsurf"],
        markers: &[Loc::home(".codeium/windsurf"), Loc::config("devin")],
        apps: &["Windsurf.app", "Devin.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::config("devin/mcp_config.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "app bundle names; whether the legacy ~/.codeium/windsurf/mcp_config.json is still read",
    },
    AgentSpec {
        id: "copilot-cli",
        name: "GitHub Copilot CLI",
        aliases: &["copilot"],
        bins: &["copilot"],
        markers: &[Loc::home(".copilot").env("COPILOT_HOME", "")],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".copilot/mcp-config.json").env("COPILOT_HOME", "mcp-config.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Copilot,
            owner_cli: None,
        }),
        unverified: "",
    },
    AgentSpec {
        id: "amp",
        name: "Amp",
        aliases: &[],
        bins: &["amp"],
        markers: &[Loc::config("amp")],
        apps: &[],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::config("amp/settings.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["amp.mcpServers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "the Windows settings path",
    },
    AgentSpec {
        id: "goose",
        name: "Goose",
        aliases: &[],
        bins: &["goose"],
        markers: &[Loc::config("goose")],
        apps: &["Goose.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::config("goose/config.yaml"),
            alt_files: &[],
            format: Format::Yaml,
            key_path: &["extensions"],
            shape: Shape::Goose,
            owner_cli: None,
        }),
        unverified: "the Windows path (%APPDATA%\\Block\\goose\\config) is not used; whether `description` is required",
    },
    AgentSpec {
        id: "zed",
        name: "Zed",
        aliases: &[],
        bins: &["zed"],
        markers: &[Loc::config("zed")],
        apps: &["Zed.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::config("zed/settings.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["context_servers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "the Zed.app bundle name",
    },
    AgentSpec {
        id: "vscode",
        name: "VS Code (Copilot agent mode)",
        aliases: &["code", "vs-code"],
        bins: &["code"],
        markers: &[Loc::app_data("Code/User")],
        apps: &["Visual Studio Code.app"],
        skills: Some(AGENTS_SKILLS),
        also_reads: &[CLAUDE_SKILLS],
        mcp: Some(McpSpec {
            file: Loc::app_data("Code/User/mcp.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["servers"],
            shape: Shape::Typed("stdio"),
            owner_cli: None,
        }),
        unverified: "the default-profile mcp.json path (inferred from the profile folder)",
    },
    AgentSpec {
        id: "antigravity",
        name: "Google Antigravity",
        aliases: &["google-antigravity", "agy"],
        bins: &["agy"],
        markers: &[
            Loc::home(".gemini/antigravity"),
            Loc::home(".gemini/config"),
            Loc::home(".gemini/antigravity-cli"),
        ],
        apps: &["Antigravity.app"],
        skills: Some(Loc::home(".gemini/config/skills")),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".gemini/config/mcp_config.json"),
            alt_files: &[],
            format: Format::Json,
            key_path: &["mcpServers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "the Antigravity.app bundle name; the CLI-only skills dir ~/.gemini/antigravity-cli/skills is not written",
    },
    // Hermes Agent (NousResearch/hermes-agent). Same id as the cua-agents
    // harness. Everything lives in the Hermes home: `$HERMES_HOME`, else
    // `~/.hermes`, else `%LOCALAPPDATA%\hermes` on Windows
    // (hermes_constants.py `_get_platform_default_hermes_home`).
    AgentSpec {
        id: "hermes",
        name: "Hermes",
        aliases: &["hermes-agent"],
        bins: &["hermes"],
        markers: &[Loc::home(".hermes")
            .env("HERMES_HOME", "")
            .windows_local("hermes")],
        apps: &["Hermes.app"],
        // The user skills dir; ~/.agents/skills is read only when listed
        // in `skills.external_dirs`, so cua writes here.
        skills: Some(
            Loc::home(".hermes/skills")
                .env("HERMES_HOME", "skills")
                .windows_local("hermes/skills"),
        ),
        also_reads: &[],
        mcp: Some(McpSpec {
            file: Loc::home(".hermes/config.yaml")
                .env("HERMES_HOME", "config.yaml")
                .windows_local("hermes/config.yaml"),
            alt_files: &[],
            format: Format::Yaml,
            key_path: &["mcp_servers"],
            shape: Shape::Plain,
            owner_cli: None,
        }),
        unverified: "named profiles (`hermes profile use`) are not configured, only HERMES_HOME or the default home; HERMES_DATA_DIR_SUFFIX is ignored",
    },
];

/// The agent with id or alias `id`.
pub fn find(id: &str) -> Option<&'static AgentSpec> {
    let id = id.trim().to_ascii_lowercase();
    AGENTS
        .iter()
        .find(|a| a.id == id || a.aliases.contains(&id.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_aliases_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for a in AGENTS {
            assert!(seen.insert(a.id), "{}", a.id);
            for al in a.aliases {
                assert!(seen.insert(al), "{al}");
            }
        }
        assert_eq!(find("claude").unwrap().id, "claude-code");
        assert_eq!(find("OpenAI-Codex").unwrap().id, "codex");
        assert!(find("nope").is_none());
    }

    #[test]
    fn shapes_match_each_agents_documented_entry() {
        let s = McpServer::new("/bin/cua").with_env("K", "v");
        let v = |sh: Shape| crate::edit::to_value(&sh.fields(&s));
        assert_eq!(
            v(Shape::Plain),
            json!({"command": "/bin/cua", "args": ["mcp"], "env": {"K": "v"}})
        );
        assert_eq!(v(Shape::Typed("stdio"))["type"], "stdio");
        assert_eq!(v(Shape::WithDisabled)["disabled"], false);
        assert_eq!(
            v(Shape::Copilot),
            json!({"type": "local", "command": "/bin/cua", "args": ["mcp"], "env": {"K": "v"}, "tools": ["*"]})
        );
        assert_eq!(
            v(Shape::OpenCode),
            json!({"type": "local", "command": ["/bin/cua", "mcp"], "environment": {"K": "v"}, "enabled": true})
        );
        assert_eq!(v(Shape::Goose)["cmd"], "/bin/cua");
        let plain = McpServer::new("cua");
        assert!(
            crate::edit::to_value(&Shape::Plain.fields(&plain))
                .get("env")
                .is_none()
        );
        for sh in [Shape::Plain, Shape::OpenCode, Shape::Goose, Shape::Copilot] {
            assert_eq!(
                sh.command_of(&v(sh)),
                Some(("/bin/cua".into(), vec!["mcp".into()]))
            );
        }
    }
}
