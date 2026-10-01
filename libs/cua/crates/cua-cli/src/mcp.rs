//! `cua mcp` (alias `serve-mcp`) and `cua daemon mcp`: one stdio Model
//! Context Protocol server, `cua_spaces::mcp::McpServer`.
//!
//! - The **Spaces contract tools** (`add_space`, `space_bash`, `send_file`,
//!   `teleport_app`, ...; `libs/cua/spaces-contract/manifest.json`) are the
//!   daemon's own implementation: `cua_sdk::Spaces::call_tool_json` runs
//!   them in the embedded runtime, or in `cua daemon` through
//!   `SpaceService.CallSpaceTool`.
//! - The **sandbox, computer and skills tools** below are an extension of
//!   that server ([`Server`] is a `ToolExtension`). The two that overlap
//!   the contract, `computer_shell` and `computer_file_write`, run the
//!   contract's `space_bash` / `space_write` handlers on the sandbox's
//!   spacesd (`cua_spaces::mcp::call_on`): one implementation.
//!
//! Tools are gated by permissions (`--permissions` or `CUA_MCP_PERMISSIONS`;
//! default: all). Computer coordinates are in the pixel space of the last
//! `computer_screenshot` of that sandbox (identity until one is taken).

use crate::{
    computer::{self, Computer, MAX_LENGTH, split_keys},
    sandbox, skills,
};
use cua_sdk::{Cua, CuaError};
use cua_spaces::mcp::{McpServer, ToolOutcome};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

/// Spaces contract permissions are `spaces:<tool>`; groups `spaces:all` and
/// `spaces:readonly` (tools annotated read-only).
fn spaces_permissions(readonly: bool) -> Vec<String> {
    cua_spaces::contract::tools()
        .into_iter()
        .filter(|t| !readonly || t.annotations.read_only)
        .map(|t| format!("spaces:{}", t.name))
        .collect()
}

const SANDBOX_ALL: &[&str] = &[
    "sandbox:list",
    "sandbox:create",
    "sandbox:delete",
    "sandbox:start",
    "sandbox:stop",
    "sandbox:restart",
    "sandbox:suspend",
    "sandbox:get",
    "sandbox:view",
    "sandbox:browser",
];
const IMAGES_ALL: &[&str] = &["images:list"];
const TELEPORT_ALL: &[&str] = &["teleport:browser"];
const COMPUTER_ALL: &[&str] = &[
    "computer:screenshot",
    "computer:click",
    "computer:type",
    "computer:key",
    "computer:scroll",
    "computer:drag",
    "computer:hotkey",
    "computer:clipboard",
    "computer:file",
    "computer:shell",
    "computer:window",
    "computer:accessibility",
];
const SKILLS_ALL: &[&str] = &[
    "skills:list",
    "skills:read",
    "skills:record",
    "skills:delete",
];

/// Expands a comma-separated permission list (groups `sandbox:all`,
/// `sandbox:readonly`, `computer:all`, `computer:readonly`, `skills:all`,
/// `skills:readonly`, `images:all`, `teleport:all`, `spaces:all`,
/// `spaces:readonly`, `all`; single `spaces:<tool>`). Empty means all.
pub fn parse_permissions(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let spaces_all = spaces_permissions(false);
    let spaces_ro = spaces_permissions(true);
    let mut all: Vec<&str> = SANDBOX_ALL
        .iter()
        .chain(COMPUTER_ALL)
        .chain(SKILLS_ALL)
        .chain(IMAGES_ALL)
        .chain(TELEPORT_ALL)
        .copied()
        .collect();
    all.extend(spaces_all.iter().map(String::as_str));
    for p in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let spaces_group: Vec<&str>;
        let group: &[&str] = match p {
            "spaces:all" => {
                spaces_group = spaces_all.iter().map(String::as_str).collect();
                &spaces_group
            }
            "spaces:readonly" => {
                spaces_group = spaces_ro.iter().map(String::as_str).collect();
                &spaces_group
            }
            "all" => &all,
            "sandbox:all" => SANDBOX_ALL,
            "sandbox:readonly" => &["sandbox:list", "sandbox:get"],
            "computer:all" => COMPUTER_ALL,
            "computer:readonly" => &["computer:screenshot"],
            "skills:all" => SKILLS_ALL,
            "skills:readonly" => &["skills:list", "skills:read"],
            // Deprecated name of `sandbox:view`.
            "sandbox:vnc" => &["sandbox:view"],
            "images:all" | "images:readonly" => IMAGES_ALL,
            "teleport:all" => TELEPORT_ALL,
            one if all.contains(&one) => {
                out.insert(one.to_string());
                continue;
            }
            unknown => {
                eprintln!("cua mcp: unknown permission {unknown:?}");
                continue;
            }
        };
        out.extend(group.iter().map(|g| g.to_string()));
    }
    if out.is_empty() && s.trim().is_empty() {
        out.extend(all.iter().map(|g| g.to_string()));
    }
    out
}

struct Tool {
    name: &'static str,
    permission: &'static str,
    description: &'static str,
    /// (name, JSON type, required, description)
    params: &'static [(&'static str, &'static str, bool, &'static str)],
}

const SB: (&str, &str, bool, &str) = (
    "sandbox",
    "string",
    false,
    "Sandbox ref (local:<name>, cloud:<name>, direct:<host:port>) or a name unique across locations (default: --sandbox / CUA_SANDBOX)",
);
const NAME: (&str, &str, bool, &str) = (
    "name",
    "string",
    true,
    "Sandbox ref (local:<name>, cloud:<name>, direct:<host:port>) or a name unique across locations",
);
const X: (&str, &str, bool, &str) = ("x", "integer", true, "X in screenshot pixels");
const Y: (&str, &str, bool, &str) = ("y", "integer", true, "Y in screenshot pixels");
const WID: (&str, &str, bool, &str) = ("window_id", "string", true, "Window id");

const TOOLS: &[Tool] = &[
    Tool {
        name: "images_list",
        permission: "images:list",
        description: "List the sandbox image catalog (the images the docs list): ref, OS, container or VM, local and cloud runtime, whether it ships cua-spacesd and which browsers, its other variants, and the command that creates it. browser_tools=true means the in-sandbox cua-driver browser tools can drive its browser. Call this first to choose an image.",
        params: &[
            (
                "os",
                "string",
                false,
                "Only this OS: linux, windows or macos",
            ),
            (
                "browser",
                "boolean",
                false,
                "Only images whose browser the cua-driver browser tools drive",
            ),
            (
                "all",
                "boolean",
                false,
                "Include unpublished entries (benchmark images)",
            ),
        ],
    },
    Tool {
        name: "sandbox_list",
        permission: "sandbox:list",
        description: "List sandboxes: local, direct and cloud (Fleet) ones by default, each row with location (local, cloud or direct). Pass location to list only one kind. If the cloud cannot be listed, the local rows come back with a warning.",
        params: &[(
            "location",
            "string",
            false,
            "Only sandboxes in this location: local, cloud or direct (default: all)",
        )],
    },
    Tool {
        name: "sandbox_create",
        permission: "sandbox:create",
        description: "Create a sandbox from any image. `on` says where (local, the default unless the user configured another with `cua config set default.on`; or cloud), `kind` what (auto, container, vm) and `runtime` which engine (auto, or gvisor, runc, qemu, lume locally; gvisor, kubevirt in the cloud); an impossible combination fails and lists the valid values. To browse the web, pass browser=true (and optionally url): it starts the canonical Linux image, registers it as a Space, launches Chromium with a throwaway profile inside it and returns the space, session, target_id and tab_id to pass to call_tool with the in-sandbox browser tools (browser_navigate, get_browser_state, browser_click, browser_type). Pass command, env and services to run a server in it, for example an MCP server: services {\"mcp\": 8765}. Without browser=true it does not register a Space: to reach its MCP server with list_tools/call_tool (service=\"mcp\"), use create_space instead (same arguments), or add this one with add_space (address local:<name>).",
        params: &[
            (
                "browser",
                "boolean",
                false,
                "Start a browser in it, ready for the cua-driver browser tools (default image: the canonical Linux image)",
            ),
            (
                "url",
                "string",
                false,
                "With browser=true: open this URL first",
            ),
            ("os_type", "string", false, "linux (default) or macos"),
            (
                "image",
                "string",
                false,
                "Image reference, any registry (overrides os_type)",
            ),
            ("name", "string", false, "Sandbox name"),
            (
                "command",
                "array",
                false,
                "Entrypoint override (argv), for example [\"python\", \"-m\", \"my_mcp\"]",
            ),
            (
                "env",
                "object",
                false,
                "Environment variables (string values)",
            ),
            (
                "services",
                "object",
                false,
                "Named services: name to guest port, for example {\"mcp\": 8765}; each is probed for readiness",
            ),
            (
                "on",
                "string",
                false,
                "Where it runs: local or cloud (default: the user's default location, else local)",
            ),
            (
                "kind",
                "string",
                false,
                "What kind of machine: auto (default; from the image), container or vm",
            ),
            (
                "runtime",
                "string",
                false,
                "Which engine: auto (default) or one the location offers for the kind (local: gvisor, runc, qemu, lume; cloud: gvisor, kubevirt)",
            ),
            (
                "pool",
                "string",
                false,
                "Use this dedicated cloud pool (Fleets advanced; implies on=cloud)",
            ),
        ],
    },
    Tool {
        name: "sandbox_get",
        permission: "sandbox:get",
        description: "Get details for a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_start",
        permission: "sandbox:start",
        description: "Start (resume) a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_stop",
        permission: "sandbox:stop",
        description: "Stop (suspend) a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_restart",
        permission: "sandbox:restart",
        description: "Restart a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_suspend",
        permission: "sandbox:suspend",
        description: "Suspend a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_delete",
        permission: "sandbox:delete",
        description: "Delete a sandbox.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_view",
        permission: "sandbox:view",
        description: "Get a browser link to the sandbox desktop in the cua-spacesd HTML5 viewer (video, audio, input, clipboard, files). The link expires after an hour.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_vnc",
        permission: "sandbox:view",
        description: "Deprecated: use sandbox_view. Returns the same viewer link.",
        params: &[NAME],
    },
    Tool {
        name: "sandbox_open_browser",
        permission: "sandbox:browser",
        description: "Launch Chromium with a fresh throwaway profile inside an existing sandbox (an image with cua-spacesd and Chromium, see images_list), bind it and optionally open a URL. Returns the space, session, target_id and tab_id for call_tool with the in-sandbox browser tools. sandbox_create with browser=true already does this.",
        params: &[
            NAME,
            ("url", "string", false, "Open this URL first"),
            (
                "session",
                "string",
                false,
                "cua-driver session label to use on every later browser call (default: browse)",
            ),
        ],
    },
    Tool {
        name: "teleport_browser_session",
        permission: "teleport:browser",
        description: "Teleport the user's signed-in session for specific sites (for example github.com) from their Firefox or Chrome into a sandbox, through the Cua Keyvault. Name each site: nothing is selected by default and there is no all-sessions option. The first call moves nothing: it files a Keyvault request and returns consent_required, a summary and a request_id. Show the summary to the user; they approve it in Cua (Touch ID or password). Then call again with the request_id to move exactly those sites. Use it only when the user asks to browse as themselves.",
        params: &[
            NAME,
            (
                "browser",
                "string",
                true,
                "firefox or chrome (chromium, brave and edge profiles use chrome)",
            ),
            (
                "sites",
                "array",
                true,
                "The exact sites to teleport, for example [\"github.com\"]",
            ),
            (
                "duration_minutes",
                "integer",
                false,
                "How long the sandbox may keep them (default 15, at most 1440)",
            ),
            (
                "reason",
                "string",
                false,
                "Why, in a few words; shown to the user as unverified",
            ),
            (
                "request_id",
                "string",
                false,
                "The request_id from the first call, after the user approved it in Cua",
            ),
        ],
    },
    Tool {
        name: "computer_screenshot",
        permission: "computer:screenshot",
        description: "Take a screenshot (long edge at most 1200 px). Later coordinates are in its pixel space.",
        params: &[SB],
    },
    Tool {
        name: "computer_click",
        permission: "computer:click",
        description: "Click at screenshot coordinates.",
        params: &[
            X,
            Y,
            ("button", "string", false, "left (default), right or middle"),
            SB,
        ],
    },
    Tool {
        name: "computer_double_click",
        permission: "computer:click",
        description: "Double-click at screenshot coordinates.",
        params: &[X, Y, SB],
    },
    Tool {
        name: "computer_move_cursor",
        permission: "computer:click",
        description: "Move the cursor.",
        params: &[X, Y, SB],
    },
    Tool {
        name: "computer_mouse_down",
        permission: "computer:click",
        description: "Press a mouse button.",
        params: &[
            X,
            Y,
            ("button", "string", false, "left, right or middle"),
            SB,
        ],
    },
    Tool {
        name: "computer_mouse_up",
        permission: "computer:click",
        description: "Release a mouse button.",
        params: &[
            X,
            Y,
            ("button", "string", false, "left, right or middle"),
            SB,
        ],
    },
    Tool {
        name: "computer_type",
        permission: "computer:type",
        description: "Type text.",
        params: &[("text", "string", true, "Text"), SB],
    },
    Tool {
        name: "computer_key",
        permission: "computer:key",
        description: "Press a key (enter, escape, tab, a, ...).",
        params: &[("key", "string", true, "Key"), SB],
    },
    Tool {
        name: "computer_key_down",
        permission: "computer:key",
        description: "Hold a key.",
        params: &[("key", "string", true, "Key"), SB],
    },
    Tool {
        name: "computer_key_up",
        permission: "computer:key",
        description: "Release a key.",
        params: &[("key", "string", true, "Key"), SB],
    },
    Tool {
        name: "computer_hotkey",
        permission: "computer:hotkey",
        description: "Press a shortcut such as ctrl+c or cmd+shift+s.",
        params: &[("keys", "string", true, "Keys joined by +"), SB],
    },
    Tool {
        name: "computer_scroll",
        permission: "computer:scroll",
        description: "Scroll up, down, left or right.",
        params: &[
            (
                "direction",
                "string",
                false,
                "down (default), up, left, right",
            ),
            ("amount", "integer", false, "Lines (default 3)"),
            SB,
        ],
    },
    Tool {
        name: "computer_drag",
        permission: "computer:drag",
        description: "Drag between two points.",
        params: &[
            ("start_x", "integer", true, "Start X"),
            ("start_y", "integer", true, "Start Y"),
            ("end_x", "integer", true, "End X"),
            ("end_y", "integer", true, "End Y"),
            SB,
        ],
    },
    Tool {
        name: "computer_clipboard_get",
        permission: "computer:clipboard",
        description: "Read clipboard text.",
        params: &[SB],
    },
    Tool {
        name: "computer_clipboard_set",
        permission: "computer:clipboard",
        description: "Set clipboard text.",
        params: &[("text", "string", true, "Text"), SB],
    },
    Tool {
        name: "computer_file_read",
        permission: "computer:file",
        description: "Read a text file.",
        params: &[("path", "string", true, "Path"), SB],
    },
    Tool {
        name: "computer_file_write",
        permission: "computer:file",
        description: "Write a text file (the Spaces `space_write` implementation, on this sandbox).",
        params: &[
            ("path", "string", true, "Path"),
            ("content", "string", true, "Content"),
            SB,
        ],
    },
    Tool {
        name: "computer_file_list",
        permission: "computer:file",
        description: "List a directory.",
        params: &[("path", "string", false, "Path (default .)"), SB],
    },
    Tool {
        name: "computer_shell",
        permission: "computer:shell",
        description: "Run a shell command (sh -c, 120 s timeout; the Spaces `space_bash` implementation, on this sandbox).",
        params: &[("command", "string", true, "Command line"), SB],
    },
    Tool {
        name: "computer_window_list",
        permission: "computer:window",
        description: "List windows.",
        params: &[("app", "string", false, "Filter by app or title"), SB],
    },
    Tool {
        name: "computer_window_open",
        permission: "computer:window",
        description: "Open a file or URL.",
        params: &[("path", "string", true, "Path or URL"), SB],
    },
    Tool {
        name: "computer_window_focus",
        permission: "computer:window",
        description: "Focus a window.",
        params: &[WID, SB],
    },
    Tool {
        name: "computer_window_unfocus",
        permission: "computer:window",
        description: "Remove focus from the current window (Escape).",
        params: &[SB],
    },
    Tool {
        name: "computer_window_minimize",
        permission: "computer:window",
        description: "Minimize a window.",
        params: &[WID, SB],
    },
    Tool {
        name: "computer_window_maximize",
        permission: "computer:window",
        description: "Maximize a window.",
        params: &[WID, SB],
    },
    Tool {
        name: "computer_window_close",
        permission: "computer:window",
        description: "Close a window.",
        params: &[WID, SB],
    },
    Tool {
        name: "computer_window_resize",
        permission: "computer:window",
        description: "Resize a window.",
        params: &[
            WID,
            ("width", "integer", true, "Width"),
            ("height", "integer", true, "Height"),
            SB,
        ],
    },
    Tool {
        name: "computer_window_move",
        permission: "computer:window",
        description: "Move a window.",
        params: &[
            WID,
            ("x", "integer", true, "X"),
            ("y", "integer", true, "Y"),
            SB,
        ],
    },
    Tool {
        name: "computer_window_get_info",
        permission: "computer:window",
        description: "Get a window's title, position and size.",
        params: &[WID, SB],
    },
    Tool {
        name: "computer_launch",
        permission: "computer:window",
        description: "Launch an application.",
        params: &[
            ("app", "string", true, "App name"),
            ("args", "array", false, "Arguments"),
            SB,
        ],
    },
    Tool {
        name: "computer_get_screen_size",
        permission: "computer:screenshot",
        description: "Primary display size in points.",
        params: &[SB],
    },
    Tool {
        name: "computer_get_cursor_position",
        permission: "computer:screenshot",
        description: "Cursor position in points.",
        params: &[SB],
    },
    Tool {
        name: "computer_get_current_window",
        permission: "computer:screenshot",
        description: "The focused window's id and title.",
        params: &[SB],
    },
    Tool {
        name: "computer_get_accessibility_tree",
        permission: "computer:screenshot",
        description: "Accessibility tree of the focused (or given) window.",
        params: &[
            ("window_id", "string", false, "Window id"),
            ("max_depth", "integer", false, "Depth limit"),
            SB,
        ],
    },
    Tool {
        name: "computer_accessibility_act",
        permission: "computer:accessibility",
        description: "Act on an accessibility element (press, focus, set_value, ...).",
        params: &[
            ("element_id", "string", true, "Element id"),
            (
                "action",
                "string",
                false,
                "press (default), focus, set_value, ...",
            ),
            ("value", "string", false, "Value for set_value"),
            ("snapshot_id", "string", false, "Snapshot id from the tree"),
            SB,
        ],
    },
    Tool {
        name: "skills_list",
        permission: "skills:list",
        description: "List recorded skills.",
        params: &[],
    },
    Tool {
        name: "skills_read",
        permission: "skills:read",
        description: "Read a skill and its steps.",
        params: &[("name", "string", true, "Skill name")],
    },
    Tool {
        name: "skills_delete",
        permission: "skills:delete",
        description: "Delete a skill.",
        params: &[("name", "string", true, "Skill name")],
    },
    Tool {
        name: "skills_record",
        permission: "skills:record",
        description: "How to record a skill (recording is interactive: cua skills record).",
        params: &[("name", "string", true, "Skill name")],
    },
];

fn schema(t: &Tool) -> Value {
    let mut props = serde_json::Map::new();
    let mut req = vec![];
    for (n, ty, required, d) in t.params {
        let mut p = json!({"type": ty, "description": d});
        if *ty == "array" {
            p["items"] = json!({"type": "string"});
        }
        if t.name == "sandbox_create" && *n == "kind" {
            p["enum"] = json!(["auto", "container", "vm"]);
        }
        if t.name == "teleport_browser_session" && *n == "browser" {
            p["enum"] = json!(["firefox", "chrome"]);
        }
        if t.name == "sandbox_list" && *n == "location" {
            p["enum"] = json!(["local", "cloud", "direct"]);
        }
        props.insert(n.to_string(), p);
        if *required {
            req.push(*n);
        }
    }
    json!({"type": "object", "properties": props, "required": req})
}

/// The `cua mcp` tool list for `cua dump-docs --type mcp`: every tool the
/// server can expose (all permissions), in `tools/list` order, with its
/// permission and, for the Spaces contract tools, the manifest facts.
pub fn docs() -> Value {
    let groups = |ro: &[&str], all: &[&str]| json!({"all": all, "readonly": ro});
    let mut tools: Vec<Value> = cua_spaces::contract::tools()
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "group": "spaces",
                "permission": format!("spaces:{}", t.name),
                "description": t.description,
                "instructions": t.instructions,
                "input_schema": t.input_schema,
                "providers": t.providers,
                "platforms": t.platforms,
                "metering": t.metering,
                "capabilities": t.capabilities,
                "annotations": t.annotations,
                "sdk_symbol": t.sdk_symbol,
                "rust_symbol": t.rust_symbol,
                "notes": t.notes,
            })
        })
        .collect();
    tools.extend(TOOLS.iter().map(|t| {
        json!({
            "name": t.name,
            "group": t.permission.split(':').next().unwrap_or_default(),
            "permission": t.permission,
            "description": t.description,
            "input_schema": schema(t),
        })
    }));
    json!({
        "version": cua_sdk::VERSION,
        "contract_version": cua_spaces::contract::CONTRACT_VERSION,
        "mcp_protocol_version": cua_spaces::contract::MCP_PROTOCOL_VERSION,
        "permission_groups": {
            "sandbox": groups(&["sandbox:list", "sandbox:get"], SANDBOX_ALL),
            "computer": groups(&["computer:screenshot"], COMPUTER_ALL),
            "skills": groups(&["skills:list", "skills:read"], SKILLS_ALL),
            "images": groups(IMAGES_ALL, IMAGES_ALL),
            "teleport": groups(&[], TELEPORT_ALL),
            "spaces": {"all": spaces_permissions(false), "readonly": spaces_permissions(true)},
        },
        "tools": tools,
    })
}

/// The server.
pub struct Server {
    cua: Arc<Cua>,
    permissions: BTreeSet<String>,
    default_sandbox: String,
    /// sandbox → (scale, origin) of its last screenshot.
    mapping: Mutex<HashMap<String, Mapping>>,
    /// The Cua Keyvault, which owns every teleport and its consent.
    keyvault: Arc<dyn crate::teleport_session::Broker>,
}

/// Scale and origin of a sandbox's last screenshot.
type Mapping = (f64, (f64, f64));

type ToolResult = Result<Vec<Value>, CuaError>;

fn text(s: impl Into<String>) -> ToolResult {
    Ok(vec![json!({"type": "text", "text": s.into()})])
}

fn jtext(v: Value) -> ToolResult {
    text(serde_json::to_string_pretty(&v).unwrap_or_default())
}

impl Server {
    /// A server over `cua`.
    pub fn new(cua: Arc<Cua>, permissions: BTreeSet<String>, default_sandbox: String) -> Self {
        Self {
            cua,
            permissions,
            default_sandbox,
            mapping: Mutex::new(HashMap::new()),
            keyvault: crate::teleport_session::default_broker(),
        }
    }

    fn permitted(&self) -> impl Iterator<Item = &'static Tool> + '_ {
        TOOLS
            .iter()
            .filter(|t| self.permissions.contains(t.permission))
    }

    /// Runs a Spaces contract handler on this sandbox's spacesd.
    async fn via_space(&self, sb: &str, tool: &str, args: Value) -> ToolResult {
        let env = sandbox::env_of(&self.cua, sb).await?;
        let caps = env
            .inner()
            .capabilities()
            .await
            .map_err(|e| CuaError::Env(e.to_string()))?;
        let space = cua_spaces::Space::attach(
            cua_spaces::SpaceId::Local {
                name: sb.to_string(),
            },
            sb,
            env.inner().clone(),
            caps,
            env.ws_headers(),
        );
        let out = cua_spaces::mcp::call_on(&space, tool, args).await;
        if out.is_error {
            return Err(CuaError::Env(
                out.first_text()
                    .unwrap_or("tool failed")
                    .trim_start_matches("error: ")
                    .to_string(),
            ));
        }
        Ok(out.content)
    }

    fn target(&self, a: &Value) -> Result<String, CuaError> {
        let s = a["sandbox"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.default_sandbox);
        if s.is_empty() {
            return Err(CuaError::InvalidArgument(
                "no sandbox specified; pass sandbox, or start with --sandbox / CUA_SANDBOX".into(),
            ));
        }
        Ok(s.to_string())
    }

    async fn computer(&self, a: &Value) -> Result<(String, Computer), CuaError> {
        let name = self.target(a)?;
        let env = sandbox::env_of(&self.cua, &name).await?;
        Ok((name, Computer::new(env)))
    }

    fn map(&self, sb: &str, a: &Value, kx: &str, ky: &str) -> Result<(f64, f64), CuaError> {
        let x = num(a, kx)?;
        let y = num(a, ky)?;
        let (scale, origin) = self
            .mapping
            .lock()
            .unwrap()
            .get(sb)
            .copied()
            .unwrap_or((1.0, (0.0, 0.0)));
        let (sx, sy) = computer::map_point((x, y), scale, origin);
        Ok((sx.round(), sy.round()))
    }

    async fn call(&self, name: &str, a: &Value) -> ToolResult {
        let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
        if let Some(r) = self.call_sandbox(name, a).await {
            return r;
        }
        if let Some(r) = self.call_skills(name, a) {
            return r;
        }
        match name {
            "images_list" => {
                return jtext(crate::catalog::mcp_result(&crate::catalog::Filter {
                    all: a["all"].as_bool().unwrap_or(false),
                    os: a["os"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    browser: a["browser"].as_bool().unwrap_or(false),
                }));
            }
            "sandbox_open_browser" => {
                let sb = self.cua.sandboxes().get(s("name")).await?.id;
                crate::browse::register_space(&self.cua, &sb).await?;
                let session = a["session"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(crate::browse::DEFAULT_SESSION);
                return jtext(
                    crate::browse::start(&self.cua, &sb, a["url"].as_str(), session).await?,
                );
            }
            "teleport_browser_session" => return self.teleport_browser(a).await,
            _ => {}
        }
        let (sb, c) = self.computer(a).await?;
        let ok = |m: String| jtext(json!({"success": true, "message": m}));
        match name {
            "computer_screenshot" => {
                let shot = c.screenshot(None, MAX_LENGTH).await?;
                self.mapping
                    .lock()
                    .unwrap()
                    .insert(sb, (shot.scale, shot.origin));
                use base64::Engine;
                Ok(vec![json!({
                    "type": "image",
                    "data": base64::engine::general_purpose::STANDARD.encode(&shot.png),
                    "mimeType": "image/png",
                })])
            }
            "computer_click" | "computer_double_click" => {
                let (x, y) = self.map(&sb, a, "x", "y")?;
                let button = a["button"].as_str().unwrap_or("left");
                let count = if name == "computer_double_click" {
                    2
                } else {
                    1
                };
                c.click(x, y, button, count).await?;
                ok(format!("clicked ({x}, {y})"))
            }
            "computer_move_cursor" => {
                let (x, y) = self.map(&sb, a, "x", "y")?;
                c.move_to(x, y).await?;
                ok(format!("moved to ({x}, {y})"))
            }
            "computer_mouse_down" | "computer_mouse_up" => {
                let p = self.map(&sb, a, "x", "y")?;
                c.mouse_button(
                    name == "computer_mouse_down",
                    Some(p),
                    a["button"].as_str().unwrap_or("left"),
                )
                .await?;
                ok(name.trim_start_matches("computer_").into())
            }
            "computer_type" => {
                c.type_text(&s("text")).await?;
                ok("typed".into())
            }
            "computer_key" => {
                c.press(&s("key")).await?;
                ok(format!("pressed {}", s("key")))
            }
            "computer_key_down" | "computer_key_up" => {
                c.key_state(&s("key"), name == "computer_key_down").await?;
                ok(name.trim_start_matches("computer_").into())
            }
            "computer_hotkey" => {
                let keys = split_keys(&s("keys"));
                c.hotkey(&keys).await?;
                ok(format!("hotkey {}", keys.join("+")))
            }
            "computer_scroll" => {
                let dir = a["direction"].as_str().unwrap_or("down");
                let amount = a["amount"].as_i64().unwrap_or(3);
                c.scroll(dir, amount, None).await?;
                ok(format!("scrolled {dir} {amount}"))
            }
            "computer_drag" => {
                let from = self.map(&sb, a, "start_x", "start_y")?;
                let to = self.map(&sb, a, "end_x", "end_y")?;
                c.drag(from, to).await?;
                ok("dragged".into())
            }
            "computer_clipboard_get" => {
                jtext(json!({"success": true, "content": c.clipboard().await?}))
            }
            "computer_clipboard_set" => {
                c.set_clipboard(&s("text")).await?;
                ok("clipboard set".into())
            }
            "computer_file_read" | "computer_file_write" | "computer_file_list" => {
                let env = sandbox::env_of(&self.cua, &sb).await?;
                match name {
                    "computer_file_read" => {
                        let b = env.download(s("path")).await?;
                        jtext(json!({"success": true, "content": String::from_utf8_lossy(&b)}))
                    }
                    "computer_file_write" => {
                        self.via_space(
                            &sb,
                            "space_write",
                            json!({"path": s("path"), "content": s("content")}),
                        )
                        .await
                    }
                    _ => {
                        let p = a["path"].as_str().filter(|p| !p.is_empty()).unwrap_or(".");
                        let files: Vec<Value> = env
                            .list_dir(p.to_string(), 1)
                            .await?
                            .into_iter()
                            .map(|e| json!({"name": e.name, "kind": e.kind, "size": e.size}))
                            .collect();
                        jtext(json!({"success": true, "files": files}))
                    }
                }
            }
            "computer_shell" => {
                self.via_space(
                    &sb,
                    "space_bash",
                    json!({"command": s("command"), "timeout": 120}),
                )
                .await
            }
            "computer_window_list" => {
                let ws = c.windows(&s("app")).await?;
                jtext(
                    json!({"success": true, "windows": ws.iter().map(computer::window_json).collect::<Vec<_>>()}),
                )
            }
            "computer_window_open" => {
                c.open(&s("path")).await?;
                ok(format!("opened {}", s("path")))
            }
            "computer_window_focus" => {
                c.window_op(&s("window_id"), "activate").await?;
                ok("focused".into())
            }
            "computer_window_unfocus" => {
                c.press("escape").await?;
                ok("unfocused".into())
            }
            "computer_window_minimize" | "computer_window_maximize" | "computer_window_close" => {
                let op = name.trim_start_matches("computer_window_");
                c.window_op(&s("window_id"), op).await?;
                ok(format!("{op}d"))
            }
            "computer_window_resize" => {
                let w = c
                    .set_bounds(
                        &s("window_id"),
                        None,
                        Some((num(a, "width")?, num(a, "height")?)),
                    )
                    .await?;
                jtext(computer::window_json(&w))
            }
            "computer_window_move" => {
                let w = c
                    .set_bounds(&s("window_id"), Some((num(a, "x")?, num(a, "y")?)), None)
                    .await?;
                jtext(computer::window_json(&w))
            }
            "computer_window_get_info" => {
                jtext(computer::window_json(&c.window(&s("window_id")).await?))
            }
            "computer_launch" => {
                let args: Vec<String> = a["args"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let pid = c.launch(&s("app"), args).await?;
                jtext(json!({"success": true, "pid": pid}))
            }
            "computer_get_screen_size" => {
                let (w, h) = c.screen_size().await?;
                jtext(json!({"success": true, "size": {"width": w, "height": h}}))
            }
            "computer_get_cursor_position" => {
                let (x, y) = c.cursor().await?;
                jtext(json!({"success": true, "position": {"x": x, "y": y}}))
            }
            "computer_get_current_window" => {
                let w = c.focused_window().await?;
                jtext(json!({
                    "window_id": w.as_ref().map(computer::window_id),
                    "title": w.as_ref().map(|w| w.title.clone()).unwrap_or_else(|| "Desktop".into()),
                }))
            }
            "computer_get_accessibility_tree" => {
                let w = a["window_id"].as_str().filter(|w| !w.is_empty());
                let depth = a["max_depth"].as_u64().unwrap_or(0) as u32;
                let t = c.a11y_tree(w, depth).await?;
                jtext(json!({
                    "success": true,
                    "snapshot_id": t.snapshot_id,
                    "truncated": t.truncated,
                    "nodes": t.nodes.iter().map(computer::node_json).collect::<Vec<_>>(),
                }))
            }
            "computer_accessibility_act" => {
                let action = a["action"]
                    .as_str()
                    .filter(|x| !x.is_empty())
                    .unwrap_or("press");
                c.a11y_act(&s("snapshot_id"), &s("element_id"), action, &s("value"))
                    .await?;
                ok(format!("{action} {}", s("element_id")))
            }
            other => Err(CuaError::InvalidArgument(format!("unknown tool {other}"))),
        }
    }

    async fn call_sandbox(&self, name: &str, a: &Value) -> Option<ToolResult> {
        let sbx = self.cua.sandboxes();
        let n = a["name"].as_str().unwrap_or_default().to_string();
        let done = |m: String| jtext(json!({"success": true, "message": m}));
        Some(match name {
            "sandbox_list" => async {
                let location = match a["location"].as_str().unwrap_or("") {
                    "" | "all" => None,
                    l @ ("local" | "cloud" | "direct") => Some(l.to_string()),
                    other => {
                        return Err(CuaError::InvalidArgument(format!(
                            "sandbox_list: location must be local, cloud or direct, not {other:?}"
                        )));
                    }
                };
                let l = sbx.list_with_warnings(location).await?;
                let mut content = jtext(Value::Array(
                    l.sandboxes.iter().map(sandbox::info_json).collect(),
                ))?;
                for w in l.warnings {
                    content.push(json!({"type": "text", "text": format!("warning: {w}")}));
                }
                Ok(content)
            }
            .await,
            "sandbox_get" => async { jtext(sandbox::info_json(&sbx.get(n).await?)) }.await,
            "sandbox_create" => {
                async {
                    let browser = a["browser"].as_bool().unwrap_or(false);
                    let image = match a["image"].as_str().filter(|i| !i.is_empty()) {
                        Some(i) => i.to_string(),
                        None if browser => "linux".to_string(),
                        None => a["os_type"].as_str().unwrap_or("linux").to_string(),
                    };
                    // The user default unless `on` says; a pool implies the
                    // cloud. The SDK resolves the default and validates.
                    let pool = a["pool"].as_str().filter(|s| !s.is_empty());
                    let text = |k: &str| {
                        a[k].as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    };
                    let mut o = sandbox_opts();
                    o.name = a["name"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    o.on = text("on").or_else(|| pool.map(|_| "cloud".to_string()));
                    o.kind = text("kind");
                    o.runtime = text("runtime");
                    let (env, services, command) = workload(a)?;
                    let named: Vec<String> = services.keys().cloned().collect();
                    o.env = env;
                    for port in services.values() {
                        o.wait_for.push(cua_sdk::ReadinessProbe {
                            port: *port,
                            http_path: None,
                            http_status: None,
                            service: None,
                        });
                    }
                    o.services = services;
                    o.command = command;
                    if browser {
                        // The browser preset drives the image's cua-spacesd.
                        o.wait_for.push(cua_sdk::ReadinessProbe {
                            port: cua_proto::SPACESD_DEFAULT_PORT,
                            http_path: None,
                            http_status: None,
                            service: None,
                        });
                    }
                    if let Some(p) = pool {
                        o.pool = Some(p.into());
                    } else {
                        let (img, os) = sandbox::resolve_image(&image, None)?;
                        o.image = img;
                        o.os = os;
                    }
                    let i = sbx.create(o).await?.info();
                    let mut v = json!({
                        "id": i.id,
                        "name": i.name,
                        "status": "ready",
                        "services": named,
                        "message": format!("Created sandbox: {}", i.name),
                    });
                    if browser {
                        let opened = async {
                            crate::browse::register_space(&self.cua, &i.id).await?;
                            crate::browse::start(
                                &self.cua,
                                &i.id,
                                a["url"].as_str(),
                                crate::browse::DEFAULT_SESSION,
                            )
                            .await
                        }
                        .await
                        .map_err(|e| {
                            CuaError::Env(format!(
                                "sandbox {} was created, but its browser did not start: {e} (retry with sandbox_open_browser, or delete it with sandbox_delete)",
                                i.id
                            ))
                        })?;
                        v["message"] = json!(format!(
                            "Created sandbox {} with a browser. Drive it with call_tool (space {}), see next.",
                            i.name, i.id
                        ));
                        if let (Some(m), Some(o)) = (v.as_object_mut(), opened.as_object()) {
                            m.extend(o.clone());
                        }
                    }
                    jtext(v)
                }
                .await
            }
            "sandbox_start" | "sandbox_stop" | "sandbox_suspend" | "sandbox_restart" => {
                async {
                    let sb = sbx.connect(n.clone()).await?;
                    match name {
                        "sandbox_start" => sb.resume().await?,
                        "sandbox_restart" => sb.restart().await?,
                        _ => sb.suspend().await?,
                    }
                    done(format!("{}: {n}", name.trim_start_matches("sandbox_")))
                }
                .await
            }
            "sandbox_delete" => {
                async {
                    let id = sbx.get(n.clone()).await.map(|i| i.id).ok();
                    sbx.delete(n.clone()).await?;
                    // A sandbox registered as a Space (sandbox_create with
                    // browser=true, add_space) leaves no stale registration.
                    if let Some(id) = id {
                        let _ = self
                            .cua
                            .spaces()
                            .call_tool_json(
                                "remove_space".into(),
                                Some(json!({"space": id}).to_string()),
                            )
                            .await;
                    }
                    done(format!("Deleted sandbox: {n}"))
                }
                .await
            }
            "sandbox_view" | "sandbox_vnc" => {
                async {
                    let link = sandbox::viewer_link(&self.cua, &n, None, false, None).await?;
                    let mut v =
                        json!({"viewer_url": link.url, "expires_at_unix": link.expires_at_unix});
                    if name == "sandbox_vnc" {
                        v["vnc_url"] = json!(link.url);
                        v["deprecated"] = json!("use sandbox_view");
                    }
                    jtext(v)
                }
                .await
            }
            _ => return None,
        })
    }

    /// `teleport_browser_session`: the Keyvault request / await / teleport
    /// flow of [`crate::teleport_session`]. This server holds no consent of
    /// its own.
    async fn teleport_browser(&self, a: &Value) -> ToolResult {
        use crate::teleport_session as ts;
        let bad = |e: String| CuaError::InvalidArgument(format!("teleport_browser_session: {e}"));
        let app = ts::browser_app(a["browser"].as_str().unwrap_or_default()).map_err(bad)?;
        let sites = ts::sites(&a["sites"]).map_err(bad)?;
        let duration = ts::duration(&a["duration_minutes"]).map_err(bad)?;
        let sb = self
            .cua
            .sandboxes()
            .get(a["name"].as_str().unwrap_or_default().to_string())
            .await?
            .id;
        let kv: &dyn ts::Broker = self.keyvault.as_ref();
        let started = std::time::Instant::now();
        let first_call = a["request_id"].as_str().is_none_or(str::is_empty);
        let out = match a["request_id"].as_str().filter(|r| !r.is_empty()) {
            None => {
                crate::browse::register_space(&self.cua, &sb).await?;
                ts::request(
                    kv,
                    &sb,
                    app,
                    &sites,
                    duration,
                    a["reason"].as_str().unwrap_or(""),
                )
                .await
            }
            Some(id) => ts::complete(kv, &sb, app, &sites, id).await,
        };
        ts::record_telemetry(app, first_call, &out, started, sites.len());
        match out {
            Ok(v) => jtext(v),
            Err(e) => Err(CuaError::TeleportRefused(format!(
                "{}: {}",
                e.kind(),
                e.message()
            ))),
        }
    }

    fn call_skills(&self, name: &str, a: &Value) -> Option<ToolResult> {
        let n = a["name"].as_str().unwrap_or_default();
        Some(match name {
            "skills_list" => jtext(Value::Array(skills::list())),
            "skills_read" => skills::read_json(n).and_then(jtext),
            "skills_delete" => skills::skill_path(n).and_then(|d| {
                if !d.is_dir() {
                    return Err(CuaError::NotFound(format!("skill not found: {n}")));
                }
                std::fs::remove_dir_all(d).map_err(crate::util::internal)?;
                jtext(json!({"success": true, "message": format!("Deleted skill: {n}")}))
            }),
            "skills_record" => jtext(json!({
                "message": format!("To record skill '{n}', run 'cua skills record --name {n} --sandbox <name>' (or --viewer-url <url>) in a terminal"),
                "instructions": [
                    format!("1. Run: cua skills record --name {n} --sandbox <name> (or --viewer-url <url>)"),
                    "2. Perform the actions you want to record in the viewer",
                    "3. Click 'Stop Recording' when done",
                ],
            })),
            _ => return None,
        })
    }
}

/// `env`, `services` and `command` of a `sandbox_create` call.
#[allow(clippy::type_complexity)]
fn workload(
    a: &Value,
) -> Result<
    (
        HashMap<String, String>,
        HashMap<String, u16>,
        Option<Vec<String>>,
    ),
    CuaError,
> {
    let bad = |what: &str| CuaError::InvalidArgument(format!("sandbox_create: {what}"));
    let mut env = HashMap::new();
    if let Some(m) = a.get("env").filter(|v| !v.is_null()) {
        for (k, v) in m.as_object().ok_or_else(|| bad("env must be an object"))? {
            let v = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            env.insert(k.clone(), v);
        }
    }
    let mut services = HashMap::new();
    if let Some(m) = a.get("services").filter(|v| !v.is_null()) {
        for (k, v) in m
            .as_object()
            .ok_or_else(|| bad("services must be an object"))?
        {
            let port = v
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or_else(|| bad(&format!("service {k}: port must be 1-65535")))?;
            services.insert(k.clone(), port);
        }
    }
    let command = match a.get("command").filter(|v| !v.is_null()) {
        None => None,
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|i| {
                    i.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| bad("command items must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(Value::String(s)) => Some(vec!["sh".into(), "-c".into(), s.clone()]),
        Some(_) => return Err(bad("command must be an array of strings")),
    };
    Ok((env, services, command.filter(|c| !c.is_empty())))
}

fn sandbox_opts() -> cua_sdk::SandboxCreateOptions {
    cua_sdk::SandboxCreateOptions::auto("")
}

fn num(a: &Value, k: &str) -> Result<f64, CuaError> {
    a[k].as_f64()
        .ok_or_else(|| CuaError::InvalidArgument(format!("{k} is required and must be a number")))
}

/// Serves stdio until EOF.
/// Runs the contract tools through the SDK: in process, or in the daemon.
struct SdkTools(Arc<cua_sdk::Spaces>);

#[async_trait::async_trait]
impl cua_spaces::mcp::ToolBackend for SdkTools {
    async fn call(&self, tool: &str, arguments: Value) -> ToolOutcome {
        match self
            .0
            .call_tool_json(tool.to_string(), Some(arguments.to_string()))
            .await
        {
            Ok(r) => ToolOutcome {
                content: serde_json::from_str(&r.content_json).unwrap_or_default(),
                structured: r
                    .structured_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                is_error: r.is_error,
                meta: r
                    .meta_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
            },
            Err(e) => ToolOutcome::error_message(error_kind(&e), e.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl cua_spaces::mcp::ToolExtension for Server {
    fn tools(&self) -> Vec<Value> {
        self.permitted()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": schema(t),
                })
            })
            .collect()
    }

    async fn call(&self, tool: &str, arguments: Value) -> Option<ToolOutcome> {
        self.permitted().find(|t| t.name == tool)?;
        Some(match Server::call(self, tool, &arguments).await {
            Ok(content) => ToolOutcome {
                content,
                structured: None,
                is_error: false,
                meta: None,
            },
            Err(e) => ToolOutcome::error_message(error_kind(&e), e.to_string()),
        })
    }
}

fn error_kind(e: &CuaError) -> &'static str {
    match e {
        // A pool whose template differs is a bad request for that pool.
        CuaError::InvalidArgument(_) | CuaError::PoolSpecMismatch(_) => "invalid_argument",
        CuaError::NotFound(_) | CuaError::ImageNotPublished(_) => "not_found",
        CuaError::AmbiguousSandbox(_) => "ambiguous_sandbox",
        CuaError::SpacesdNotAvailable(_) => "spacesd_not_available",
        CuaError::CapabilityMissing(_) => "capability_missing",
        CuaError::HostCapabilityMissing(_) => "host_capability_missing",
        CuaError::TeleportRefused(_) => "teleport_refused",
        CuaError::Timeout(_) | CuaError::ClaimSecretsNotDelivered(_) => "timeout",
        CuaError::Unauthenticated(_) => "unauthenticated",
        CuaError::Fleet(_) => "fleet",
        CuaError::FleetAdmissionDenied(_) => "fleet_admission_denied",
        CuaError::CloudCreditExhausted(_) => "cloud_credit_exhausted",
        CuaError::Cloud(_) => "cloud",
        CuaError::InsufficientDisk(_) => "insufficient_disk",
        CuaError::Cancelled(_) => "cancelled",
        _ => "env",
    }
}

/// The one MCP server `cua mcp` and `cua daemon mcp` serve: the Spaces
/// contract (through `cua`) plus the sandbox/computer/skills extension.
pub fn server(cua: Arc<Cua>, permissions: BTreeSet<String>, default_sandbox: String) -> McpServer {
    let spaces = cua.spaces();
    let allowed = permissions.clone();
    McpServer::remote(Arc::new(SdkTools(spaces)))
        .with_name("cua")
        .with_filter(Arc::new(move |tool: &str| {
            allowed.contains(&format!("spaces:{tool}"))
        }))
        .with_extension(Arc::new(Server::new(cua, permissions, default_sandbox)))
}

/// Serves `server` on stdin/stdout until EOF.
pub async fn serve_stdio(server: McpServer) -> Result<i32, CuaError> {
    cua_spaces::mcp::stdio::serve_process_stdio(server)
        .await
        .map_err(crate::util::internal)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_create_workload_arguments() {
        let (env, services, command) = workload(&json!({
            "env": {"A": "b", "N": 1},
            "services": {"mcp": 8765},
            "command": ["python", "-m", "srv"],
        }))
        .unwrap();
        assert_eq!(env["A"], "b");
        assert_eq!(env["N"], "1");
        assert_eq!(services["mcp"], 8765);
        assert_eq!(command.unwrap(), ["python", "-m", "srv"]);
        let (_, _, shell) = workload(&json!({"command": "echo hi"})).unwrap();
        assert_eq!(shell.unwrap(), ["sh", "-c", "echo hi"]);
        assert!(workload(&json!({"services": {"mcp": 0}})).is_err());
        assert!(workload(&json!({"services": {"mcp": 70000}})).is_err());
        assert!(workload(&json!({"env": ["A=b"]})).is_err());
        assert!(workload(&json!({"command": [1]})).is_err());
        let schema = schema(TOOLS.iter().find(|t| t.name == "sandbox_create").unwrap());
        for k in [
            "image", "command", "env", "services", "on", "kind", "runtime",
        ] {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        assert!(schema["properties"].get("local").is_none());
        assert_eq!(schema["properties"]["command"]["type"], "array");
        assert_eq!(schema["properties"]["services"]["type"], "object");
        assert_eq!(
            schema["properties"]["kind"]["enum"],
            json!(["auto", "container", "vm"])
        );
        // It creates a sandbox and says so: no Space is registered.
        let d = TOOLS
            .iter()
            .find(|t| t.name == "sandbox_create")
            .unwrap()
            .description;
        assert!(d.contains("local, the default"), "{d}");
        assert!(d.contains("does not register a Space"), "{d}");
    }

    #[test]
    fn permission_groups_expand() {
        let p = parse_permissions("sandbox:readonly,computer:click");
        assert_eq!(
            p.iter().map(String::as_str).collect::<Vec<_>>(),
            ["computer:click", "sandbox:get", "sandbox:list"]
        );
        assert!(parse_permissions("").contains("skills:delete"));
        assert!(parse_permissions("bogus").is_empty());
    }
}
