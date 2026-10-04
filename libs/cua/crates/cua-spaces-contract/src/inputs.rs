//! The argument object of every Spaces tool, as Rust types.
//!
//! These structs are the single source of truth for each tool's
//! `inputSchema`: the manifest generator derives the JSON Schema from them
//! with `schemars`, and the MCP server in `cua-spaces` deserializes
//! `tools/call` arguments into the very same types. A field documented here
//! is a field the server reads; a field the server reads is documented here.
//!
//! Before this module the schemas lived only in `spaces_mcp.py`, as
//! hand-written dictionaries that nothing checked against the code.
//!
//! Every optional argument is an `Option`, and its default is stated in the
//! field's doc comment, because that doc comment *is* the published
//! description. Unknown arguments are ignored rather than refused, which is
//! what the Python server did and what MCP hosts that decorate calls expect.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A Space id in any accepted spelling.
///
/// Ids are sandbox refs: `local:<name>`, `cloud:<name>`,
/// `direct:<host:port>` and `relay:<machine-id>`. The legacy spellings
/// (`space://fleet/<ns>/<claim>`, `space://local/<name>`,
/// `fleet:<ns>:<claim>`, ...) are still accepted, as is a registered
/// Space's display name or a bare name unique across locations.
pub type SpaceArg = String;

/// `add_space`: register a Space by URL after a capabilities handshake.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddSpace {
    /// The Space's cua-spacesd: `http(s)://host:port` (port defaults to
    /// 3211), `host:port`, or a Space id (`cloud:<name>`, `local:<name>`).
    /// Any other MCP endpoint (for example
    /// `http://host:8765/mcp`) is added as a Space with one MCP service and
    /// no spacesd capabilities.
    pub url: String,
    /// The spacesd token (or the bearer an MCP endpoint needs). Stored in
    /// a separate 0600 credentials file, never in `spaces.json`.
    #[serde(default)]
    pub token: Option<String>,
    /// Display name. Default: the guest's hostname.
    #[serde(default)]
    pub name: Option<String>,
    /// For an MCP endpoint URL: the service name to register it under.
    /// Default `mcp`.
    #[serde(default)]
    pub service: Option<String>,
}

/// What a Space runs besides its image (`create_space`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpaceWorkload {
    /// Entrypoint override (argv), for example
    /// `["python", "-m", "my_mcp", "--port", "8765"]`.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Guest environment variables.
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    /// Named services (name to guest port), for example `{"mcp": 8765}`.
    /// Each is probed for readiness and reachable with `list_tools` /
    /// `call_tool` (`service=<name>`).
    #[serde(default)]
    pub services: Option<BTreeMap<String, u16>>,
    /// Whether the image runs cua-spacesd. Default: true for the canonical
    /// images (and no image), false for any other image. A Space without it
    /// has an empty capability set: only its services work.
    #[serde(default)]
    pub spacesd: Option<bool>,
}

/// `remove_space`: forget a registered Space. Nothing in the Space is
/// touched; `delete_space` deletes a Space that `create_space` made.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RemoveSpace {
    /// Space id or name.
    pub space: SpaceArg,
}

/// `list_spaces`: takes no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListSpaces {}

/// The cloud runtime a cloud Space's pool is named after (internal; the
/// tools take `runtime` as a string).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FleetRuntime {
    /// A full KubeVirt VM booted from a KubeVirt containerDisk image (one
    /// with a `/disk/disk.img` layer).
    Kubevirt,
    /// A gVisor (runsc) pod running a container rootfs image. About six
    /// times faster to warm-start, with a far smaller memory footprint.
    Gvisor,
}

impl FleetRuntime {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            FleetRuntime::Kubevirt => "kubevirt",
            FleetRuntime::Gvisor => "gvisor",
        }
    }
}

/// What kind of machine a Space is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpaceKind {
    /// From the image: macOS and Windows images are VMs, an image with a
    /// container rootfs is a container, a disk-only image is a VM.
    Auto,
    /// A container (gVisor, or runc locally).
    Container,
    /// A virtual machine (QEMU or Lume locally, KubeVirt in the cloud).
    Vm,
}

impl SpaceKind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceKind::Auto => "auto",
            SpaceKind::Container => "container",
            SpaceKind::Vm => "vm",
        }
    }
}

/// `create_space`: create a Space (a new sandbox) where `on` says.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CreateSpace {
    /// Guest image. Default: the canonical Linux image
    /// (`ghcr.io/trycua/linux:24.04`), whose variant follows the kind.
    #[serde(default)]
    pub image: Option<String>,
    /// Where it runs: `local` (this machine, free), `cloud` (Cua cloud,
    /// metered), or `host:<machine>`: one of the user's own machines that
    /// provides Spaces (`cua host setup --provide-spaces` on it), named by
    /// id or by words from its name ("host:spare mac mini" matches "Mac
    /// mini (spare)" or `dillons-mac-mini`). Default: the user's default
    /// location (`cua config set default.on`, `CUA_DEFAULT_ON`), else
    /// `local`. Not `direct:<addr>`: that is one already-running Space at a
    /// fixed address, attached with `add_space`, not a host this can create
    /// on. An existing machine is added with `add_space`, not created.
    #[serde(default)]
    pub on: Option<String>,
    /// What kind of machine. Default `auto` (from the image).
    #[serde(default)]
    pub kind: Option<SpaceKind>,
    /// Which engine. Default `auto`: the safest one for the location and
    /// kind. Local: `gvisor` or `runc` (containers), `qemu` or `lume`
    /// (VMs). Cloud: `gvisor` (containers), `kubevirt` (VMs). Anything
    /// else fails with `invalid_placement`, listing the valid values.
    #[serde(default)]
    pub runtime: Option<String>,
    /// Name. Default: a generated `space-<hex>`.
    #[serde(default)]
    pub name: Option<String>,
    /// Return a reachable registered Space in the same location that has
    /// the requested services instead of creating one (get-or-create).
    /// Default false.
    #[serde(default)]
    pub reuse: Option<bool>,
    /// Wait until the Space is ready (its spacesd, or its declared
    /// services). Default true.
    #[serde(default)]
    pub wait: Option<bool>,
    /// Seconds to wait for readiness (local). Default 600.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// How many Spaces to create, 1 to 8 (default 1), all with the same
    /// settings (a name gets `-2`, `-3`, ... suffixes). With more than one
    /// the result is an array.
    #[serde(default)]
    pub count: Option<u32>,
    /// Command, environment and services (see [`SpaceWorkload`]).
    #[serde(flatten)]
    pub workload: SpaceWorkload,
}

/// A tool whose only argument is the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpaceOnly {
    /// Space id or name.
    pub space: SpaceArg,
}

/// `space_bash`: run a shell command inside the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpaceBash {
    /// Space id or name.
    pub space: SpaceArg,
    /// Command line, run with `/bin/sh -c` (`cmd.exe /C` on Windows guests).
    pub command: String,
    /// Seconds before the command is killed. Default 60.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// `space_write`: write literal text to a file in the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpaceWrite {
    /// Space id or name.
    pub space: SpaceArg,
    /// Absolute path in the Space. Parent directories are created.
    pub path: String,
    /// The exact file contents.
    pub content: String,
}

/// `upload`: copy a host file or folder into the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Upload {
    /// Space id or name.
    pub space: SpaceArg,
    /// Host path (file or folder).
    pub path: String,
    /// Absolute destination path in the Space; an existing file there is
    /// replaced. Default: `<guest home>/<name>`, or `<name> (1).ext` when
    /// that is taken (nothing is replaced).
    #[serde(default)]
    pub dest: Option<String>,
}

/// `send_file`: the Teleport drop zone transfer into the Space's Downloads.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SendFile {
    /// Space id or name.
    pub space: SpaceArg,
    /// Host path (file or folder) to send.
    pub path: String,
    /// Subdirectory of the Space user's `~/Downloads` to land in. `~/Downloads`
    /// and `~/Downloads/<sub>` are accepted too. Default: `~/Downloads` itself.
    #[serde(default)]
    pub target_directory: Option<String>,
    /// Honor `.gitignore` / `.ignore` (nested) and `.dockerignore` (root only)
    /// when sending a folder. Default true. No effect on a single named file.
    #[serde(default)]
    pub respect_ignorefiles: Option<bool>,
}

/// `download`: fetch a file or folder out of the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Download {
    /// Space id or name.
    pub space: SpaceArg,
    /// Absolute path in the Space (file or folder).
    pub path: String,
    /// Host destination directory. Default `~/Downloads/cua-spaces`.
    #[serde(default)]
    pub dest: Option<String>,
}

/// `stream_endpoint`: mint a media ticket for a desktop or window stream.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StreamEndpoint {
    /// Space id or name.
    pub space: SpaceArg,
    /// Window handle from `list_space_windows`. Streams that window.
    #[serde(default)]
    pub window_id: Option<String>,
    /// Stream the largest visible window of this app (case-insensitive
    /// substring). Ignored when `window_id` is set.
    #[serde(default)]
    pub app_name: Option<String>,
    /// Display id for a desktop stream. Default: the primary display.
    #[serde(default)]
    pub display: Option<String>,
    /// Maximum frames per second. Default 30.
    #[serde(default)]
    pub max_fps: Option<u32>,
    /// Ticket lifetime in seconds (at most 600). Default 60.
    #[serde(default)]
    pub ticket_ttl: Option<u64>,
}

/// `list_space_windows`: the Space's streamable windows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListSpaceWindows {
    /// Space id or name.
    pub space: SpaceArg,
    /// Only windows of this app (case-insensitive substring).
    #[serde(default)]
    pub app_name: Option<String>,
}

/// `stream_space_window`: draw one Space window on the operator's desktop.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StreamSpaceWindow {
    /// Space id or name.
    pub space: SpaceArg,
    /// Auto-pick this app's largest visible window, e.g. "Firefox".
    #[serde(default)]
    pub app_name: Option<String>,
    /// Exact window handle from `list_space_windows` (overrides `app_name`).
    #[serde(default)]
    pub window_id: Option<String>,
    /// Label for the operator's window. Default: the window title.
    #[serde(default)]
    pub title: Option<String>,
}

/// `list_tools`: tools of an MCP service inside the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListTools {
    /// Space id or name.
    pub space: SpaceArg,
    /// Substring filter; matching tools come back with full input schemas.
    #[serde(default)]
    pub name: Option<String>,
    /// Service. A service the Space declares (see `list_spaces`), reached
    /// over generic MCP (streamable HTTP), or `driver` (the spacesd's
    /// cua-driver registry; `mcp`, `cua-driver` and `computer-server` are
    /// aliases unless the Space declares a service by that name). Default:
    /// `driver`, or the only declared service of a Space without
    /// spacesd.
    #[serde(default)]
    pub service: Option<String>,
}

/// `call_tool`: invoke a tool of an MCP service inside the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CallTool {
    /// Space id or name.
    pub space: SpaceArg,
    /// Tool name from `list_tools`.
    pub tool: String,
    /// Arguments object for the tool. Default `{}`.
    #[serde(default)]
    pub arguments: Option<serde_json::Map<String, serde_json::Value>>,
    /// Service (see `list_tools`). Default: `driver`, or the only declared
    /// service of a Space without spacesd.
    #[serde(default)]
    pub service: Option<String>,
}

/// The coding-agent harnesses (all driven over the Agent Client Protocol).
pub const AGENT_IDS: &[&str] = &[
    "claude-code",
    "gemini-cli",
    "google-antigravity",
    "goose",
    "hermes",
    "openai-codex",
    "openclaw",
    "opencode",
    "pi",
];

fn agent_id_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "enum": AGENT_IDS,
    })
}

/// An MCP server for the run: `url` (streamable HTTP, reachable from inside
/// the Space) or `command` (stdio, run in the Space).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentMcpServer {
    /// Name the agent sees.
    pub name: String,
    /// `http(s)://` URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Command run inside the Space.
    #[serde(default)]
    pub command: Option<String>,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

/// `agent_start`: run a coding agent inside the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentStart {
    /// Space id or name.
    pub space: SpaceArg,
    /// Harness id. `agent_capabilities` lists what each needs.
    #[schemars(schema_with = "agent_id_schema")]
    pub agent: String,
    /// The task.
    pub prompt: String,
    /// Provider key variables to forward from this server's environment,
    /// for example ["ANTHROPIC_API_KEY"]. Names only; values never pass
    /// through the conversation.
    #[serde(default)]
    pub env_from_host: Vec<String>,
    /// More environment for the agent, for example
    /// `{"HERMES_HOME": "/home/cua/bot"}`. Stored 0600 in the run and
    /// redacted from its output; name provider keys in `env_from_host`
    /// instead, so their values never pass through the conversation.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Git URL cloned into the working directory first.
    #[serde(default)]
    pub repo: Option<String>,
    /// Branch or tag for `repo`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Working directory in the Space. Default: the run's own.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Model id.
    #[serde(default)]
    pub model: Option<String>,
    /// A custom model endpoint base URL (a proxy or compatible server).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Extra MCP servers. The Space's own tools are always included.
    #[serde(default)]
    pub mcp_servers: Vec<AgentMcpServer>,
    /// Stop (resumably) once the prompt is answered. Default false.
    #[serde(default)]
    pub exit_when_idle: Option<bool>,
    /// Open a terminal on the Space's desktop following the run. Default
    /// false.
    #[serde(default)]
    pub show: Option<bool>,
    /// Run as this persistent agent: its home in the Cua Volume
    /// (`agents/<home>/`) is restored into the Space first and saved after
    /// every turn, the harness keeps its memory there, and the agent gets
    /// the `cua` bridge (notify_user, the drive). Creates the persistent
    /// agent (this harness, this Space) when it does not exist yet. Default:
    /// a one-off run whose state lives only in the Space.
    #[serde(default)]
    pub home: Option<String>,
}

/// `agent_message`: a follow-up in the same session.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessage {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    pub run_id: String,
    /// The message.
    pub text: String,
    /// Interrupt a running turn first instead of queueing. Default false.
    #[serde(default)]
    pub force: Option<bool>,
}

/// `agent_status`: one run's status, result and recent output.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentStatus {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    pub run_id: String,
    /// Rendered output lines to return. Default 40.
    #[serde(default)]
    pub tail: Option<u32>,
}

/// `agent_events`: normalized events after a cursor.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentEvents {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    pub run_id: String,
    /// `cursor` from the previous call. Default 0 (the start).
    #[serde(default)]
    pub cursor: Option<u64>,
    /// Most events to return. Default 100.
    #[serde(default)]
    pub max: Option<u32>,
    /// Include each event's raw ACP payload. Default false.
    #[serde(default)]
    pub raw: Option<bool>,
}

/// `agent_interrupt`: cancel the turn in flight.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentInterrupt {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    pub run_id: String,
}

/// `agent_stop`: stop a run and verify it died.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentStop {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    pub run_id: String,
}

/// `agent_capabilities`: takes no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentCapabilities {}

/// `persistent_agent_create`: a named agent whose memory outlives its runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentCreate {
    /// The agent's name: 1-63 of a-z, 0-9, `.`, `_`, `-`. Its home is
    /// `agents/<name>/` in the Cua Volume.
    pub name: String,
    /// Harness id. `agent_capabilities` lists what each needs.
    #[schemars(schema_with = "agent_id_schema")]
    pub agent: String,
    /// The Space it works in (id or name).
    pub space: SpaceArg,
    /// Model id.
    #[serde(default)]
    pub model: Option<String>,
    /// A custom model endpoint base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider key variables forwarded from this server's environment at
    /// every start, for example ["ANTHROPIC_API_KEY"]. Names only.
    #[serde(default)]
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets; see `env_from_host`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A persistent agent by name.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentName {
    /// The persistent agent's name.
    pub name: String,
}

/// `persistent_agent_list`: takes no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentList {}

/// `persistent_agent_send`: give a persistent agent a turn.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentSend {
    /// The persistent agent's name.
    pub name: String,
    /// The message (the prompt of a new run, or a follow-up).
    pub text: String,
}

/// `agent_resume`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentResume {
    /// The persistent agent's name.
    pub name: String,
    /// Start a run on this prompt once the agent is back. Default: none
    /// (the next message or routine starts one).
    #[serde(default)]
    pub prompt: Option<String>,
}

/// `routine_add`: a recurring turn of a persistent agent. Give exactly one
/// of `every_minutes`, `daily_at` or `weekly_on`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutineAdd {
    /// The persistent agent that runs it.
    pub agent: String,
    /// A short title (shown in the app and prefixed to the turn).
    pub title: String,
    /// What the agent is asked each time.
    pub prompt: String,
    /// Every N minutes.
    #[serde(default)]
    pub every_minutes: Option<i64>,
    /// Every day at `HH:MM` (local time).
    #[serde(default)]
    pub daily_at: Option<String>,
    /// Every week at `<weekday> HH:MM` (local time), for example `mon 09:00`.
    #[serde(default)]
    pub weekly_on: Option<String>,
    /// Default true.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `routine_list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutineList {
    /// Only this persistent agent's routines. Default: all.
    #[serde(default)]
    pub agent: Option<String>,
}

/// A routine by id.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutineId {
    /// The routine's id.
    pub id: String,
}

/// `routine_set_enabled`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutineSetEnabled {
    /// The routine's id.
    pub id: String,
    /// On or off.
    pub enabled: bool,
}

/// `notify_user`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NotifyUser {
    /// One short line.
    pub title: String,
    /// The details. Default empty.
    #[serde(default)]
    pub body: Option<String>,
    /// The persistent agent it is about, if any.
    #[serde(default)]
    pub agent: Option<String>,
}

/// `notifications_list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NotificationsList {
    /// Only unread ones. Default false.
    #[serde(default)]
    pub unread_only: Option<bool>,
    /// Only ones newer than this (Unix ms).
    #[serde(default)]
    pub since_ms: Option<u64>,
}

/// `notifications_ack`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NotificationsAck {
    /// Ids to mark read. Default: every notification.
    #[serde(default)]
    pub ids: Vec<String>,
}

/// `computer_access_grant`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ComputerAccessGrant {
    /// The persistent agent.
    pub agent: String,
    /// The machine (a Space id, usually `relay:<machine-id>` from `cua
    /// host setup`).
    pub machine: String,
    /// End the grant after this many seconds. Default: until revoked.
    #[serde(default)]
    pub expires_in_secs: Option<u64>,
}

/// `computer_access_revoke`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ComputerAccessRevoke {
    /// The persistent agent.
    pub agent: String,
    /// The machine. Default: every machine.
    #[serde(default)]
    pub machine: Option<String>,
}

/// `computer_access_list`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ComputerAccessList {
    /// Only this persistent agent's grants. Default: all.
    #[serde(default)]
    pub agent: Option<String>,
    /// Include the audit log's newest entries (this many). Default 0.
    #[serde(default)]
    pub audit: Option<u32>,
}

/// `teleport_manifest`: what a teleport of a host app would move.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TeleportManifest {
    /// App id, e.g. `firefox`, `chrome`, `claude-code`.
    pub app: String,
    /// `full` (the profile) or `tabs`. Default `full`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Space to check the receiving side of (`GetManifest`). Optional.
    #[serde(default)]
    pub space: Option<SpaceArg>,
}

/// `teleport_app`: move a host app session into the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TeleportApp {
    /// Space id or name.
    pub space: SpaceArg,
    /// App id, e.g. `firefox`, `chrome`, `claude-code`.
    pub app: String,
    /// `full` or `tabs`. Default `full`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Manifest `rel_path`s to send. Default: the manifest's default-checked
    /// items, never everything. An empty list is refused as ambiguous.
    #[serde(default)]
    pub include: Option<Vec<String>>,
    /// Must be true when the selection holds a sensitive item (credentials,
    /// cookies). The consent is explicit rather than defaulted.
    #[serde(default)]
    pub acknowledge_sensitive: Option<bool>,
    /// The Keyvault request id returned by a previous call. Present on a retry
    /// after the user approved the teleport in Cua; the broker then performs
    /// the delivery. Absent on the first call.
    #[serde(default)]
    pub request_id: Option<String>,
}

/// `request_site_login`: sign in to a site in the Space with a saved password.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RequestSiteLogin {
    /// Space id or name.
    pub space: SpaceArg,
    /// The site's sign-in page (http or https), for example
    /// `https://github.com/login`.
    pub url: String,
    /// Which saved username, when the site has several.
    #[serde(default)]
    pub username: Option<String>,
    /// The persistent agent asking (shown to the user in the approval).
    #[serde(default)]
    pub agent: Option<String>,
    /// cua-driver lifecycle session label of the browser tab to sign in.
    #[serde(default)]
    pub session: Option<String>,
    /// cua-driver browser target id (from `get_browser_state`).
    #[serde(default)]
    pub target_id: Option<String>,
    /// cua-driver tab id (from `get_browser_state`).
    #[serde(default)]
    pub tab_id: Option<String>,
    /// The request id a previous call returned. Present on the retry after
    /// the user approved; absent on the first call.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Seconds to wait for the user's decision on a retry. Default 20, at
    /// most 120.
    #[serde(default)]
    pub wait_secs: Option<u32>,
    /// Sends the password over a relay connection that predates end-to-end
    /// sealing even though the relay could read it in transit. Default
    /// `false`: such a Space refuses the fill instead (S1).
    #[serde(default)]
    pub relay_plaintext_ack: bool,
}

/// `hotspot_start`: route the Space's egress through this machine.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct HotspotStart {
    /// Space id or name.
    pub space: SpaceArg,
    /// Destinations (CIDRs or host suffixes) that keep using the Space's own
    /// network.
    #[serde(default)]
    pub bypass: Option<Vec<String>>,
    /// Point the Space's system proxy (and `HTTP(S)_PROXY` for new processes)
    /// at the hotspot. Default true.
    #[serde(default)]
    pub set_system_proxy: Option<bool>,
}

/// `hotspot_stop` / `hotspot_status`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct HotspotTarget {
    /// Space id or name. Default: every Space with an active hotspot.
    #[serde(default)]
    pub space: Option<SpaceArg>,
}

// --- Cua Volume ----------------------------------------------------------------

/// `volume_ls`: the immediate children of a drive folder.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveLs {
    /// Folder, for example `agents/ada/` or `public/`. Default: the root.
    #[serde(default)]
    pub path: Option<String>,
    /// See the drive as this persistent agent does (`ada`). Only narrows:
    /// the agent's defaults and grants apply. Default: the user (the whole
    /// drive).
    #[serde(default)]
    pub as_agent: Option<String>,
    /// With `as_agent`: the Space id the agent is in, whose `spaces/<space>/`
    /// folder it may write.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_read`: read a file.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveRead {
    /// File path, for example `agents/ada/memory/MEMORY.md`.
    pub path: String,
    /// A version id from `volume_history`. Default: the current version.
    #[serde(default)]
    pub version: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_write`: write a file (a new version).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveWrite {
    /// File path.
    pub path: String,
    /// The content, as text or base64 (see `encoding`). At most 8 MiB.
    pub content: String,
    /// `utf8` or `base64`. Default `utf8`.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Write only if the current version has this etag (from `volume_read`
    /// or `volume_ls`): a compare-and-swap.
    #[serde(default)]
    pub if_etag: Option<String>,
    /// Write only if nothing is at `path` yet. Default false.
    #[serde(default)]
    pub create_only: Option<bool>,
    /// See `volume_ls`.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_delete`: delete a file (its history stays).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveDelete {
    /// File path.
    pub path: String,
    /// Delete only if the current version has this etag.
    #[serde(default)]
    pub if_etag: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_history`: a file's versions, newest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveHistory {
    /// File path.
    pub path: String,
    /// See `volume_ls`.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_restore`: make an old version current again.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveRestore {
    /// File path.
    pub path: String,
    /// The version id to restore (from `volume_history`).
    pub version: String,
    /// See `volume_ls`.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_grant`: widen an agent's or a Space's access.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveGrant {
    /// `agent:<name>` or `space:<id>`.
    pub principal: String,
    /// A folder (`agents/writer/outputs/`) or one file.
    pub prefix: String,
    /// `r` (read) or `rw` (read and write).
    pub mode: String,
    /// Lifetime in seconds. Default: until revoked.
    #[serde(default)]
    pub expires_in_secs: Option<u64>,
    /// A note shown next to the grant.
    #[serde(default)]
    pub note: Option<String>,
}

/// `volume_revoke`: revoke a grant.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveRevoke {
    /// The grant id (from `volume_grants`).
    pub grant_id: String,
}

/// `volume_grants`: the grants.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveGrants {
    /// Include expired and revoked grants. Default false.
    #[serde(default)]
    pub all: Option<bool>,
}

/// `volume_request_access`: ask the user for more access (as an agent).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveRequestAccess {
    /// The folder or file wanted.
    pub prefix: String,
    /// `r` or `rw`.
    pub mode: String,
    /// Why (shown to the user as the agent's words).
    #[serde(default)]
    pub reason: Option<String>,
    /// The agent asking.
    #[serde(default)]
    pub as_agent: Option<String>,
    /// See `volume_ls`.
    #[serde(default)]
    pub in_space: Option<String>,
}

/// `volume_approve`: turn an access request into a grant.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveApprove {
    /// The request id (from `volume_requests`).
    pub request_id: String,
    /// Lifetime in seconds. Default: until revoked.
    #[serde(default)]
    pub expires_in_secs: Option<u64>,
}

/// `volume_deny`: decline an access request.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveDeny {
    /// The request id.
    pub request_id: String,
}

/// `volume_audit`: the newest audit events.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveAudit {
    /// How many. Default 50, at most 1000.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `volume_storage_set`: test or change where the drive keeps its bytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveStorageSet {
    /// `fs` (this machine) or `s3` (an S3-compatible bucket: AWS S3, R2,
    /// MinIO). `cloud` is refused.
    pub backend: String,
    /// The bucket, for `s3`.
    #[serde(default)]
    pub s3: Option<DriveS3Settings>,
    /// The access key id (with the secret; both or neither). Saved in the
    /// credential store, never in a file.
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// The secret access key.
    #[serde(default)]
    pub secret_access_key: Option<String>,
    /// Only test the connection; change nothing. Default false.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

/// Where an S3-compatible bucket is.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveS3Settings {
    /// `http://127.0.0.1:9000`, `https://<account>.r2.cloudflarestorage.com`;
    /// absent for AWS.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// `us-east-1`; `auto` for R2. Default `us-east-1`.
    #[serde(default)]
    pub region: Option<String>,
    pub bucket: String,
    /// A key prefix inside the bucket. Default none.
    #[serde(default)]
    pub root: Option<String>,
    /// Path-style addressing (MinIO and most self-hosted stores).
    #[serde(default)]
    pub path_style: Option<bool>,
}

/// `volume_sync_events`: sync events after a sequence number (a long poll).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveSyncEvents {
    /// Return events after this `seq` (default 0: all kept).
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Wait up to this long for one when none is newer (default 0, at most
    /// 30000).
    #[serde(default)]
    pub wait_ms: Option<u32>,
}

/// `volume_sync_resolve`: clear a conflict from the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveSyncResolve {
    /// The file (or its conflict copy).
    pub path: String,
}

/// `volume_cache_set`: the block cache's size cap.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DriveCacheSet {
    /// Bytes (at least 268435456, 256 MiB).
    pub capacity_bytes: u64,
}

/// A tool with no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// `share_space`: let an account watch or edit the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShareSpace {
    /// Space id or name.
    pub space: SpaceArg,
    /// Who: an email address (verified by cua.ai) or an account id. Teams
    /// are not accepted: share with each member.
    pub who: String,
    /// `viewer` (presence with their own cursor and a view-only stream) or
    /// `editor` (full use; their input is a human session). Default
    /// `viewer`.
    #[serde(default)]
    pub role: Option<String>,
}

/// `unshare_space`: stop sharing the Space with one account, or with
/// everyone.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct UnshareSpace {
    /// Space id or name.
    pub space: SpaceArg,
    /// The email or account id to remove. Default: everyone (the Space
    /// stops being shared; a Space that is not a host leaves the relay).
    #[serde(default)]
    pub who: Option<String>,
}

/// `space_shares`: who the Space is shared with.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpaceShares {
    /// Space id or name.
    pub space: SpaceArg,
    /// Also return this many of the newest owner-side audit lines. Default
    /// 0.
    #[serde(default)]
    pub audit: Option<u32>,
}

/// The input schema of `T` as MCP publishes it: a self-contained JSON Schema
/// object (no `$schema`, no `$defs`, no `title`), with optional arguments
/// simply left out of `required`.
pub fn input_schema<T: JsonSchema>() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    });
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).expect("schema serializes");
    if let Some(object) = value.as_object_mut() {
        object.remove("title");
        object.remove("$schema");
        object.remove("$defs");
        // The struct's own doc comment names the tool; the tool description
        // says it better, so the schema does not repeat it.
        object.remove("description");
        object
            .entry("properties")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(serde_json::Value::Object(properties)) = object.get_mut("properties") {
            for property in properties.values_mut() {
                drop_null_option(property);
            }
        }
    }
    value
}

/// An `Option<T>` argument is optional because it is absent from
/// `required`, not because it may be `null`: fold `["T", "null"]`,
/// `anyOf: [T, {"type": "null"}]` and `"default": null` back into `T`, which
/// is what every MCP host renders best.
fn drop_null_option(schema: &mut serde_json::Value) {
    use serde_json::Value;
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if object.get("default") == Some(&Value::Null) {
        object.remove("default");
    }
    if let Some(Value::Array(types)) = object.get("type").cloned() {
        let kept: Vec<Value> = types.into_iter().filter(|t| t != "null").collect();
        if kept.len() == 1 {
            object.insert("type".into(), kept[0].clone());
        } else {
            object.insert("type".into(), Value::Array(kept));
        }
    }
    if let Some(Value::Array(branches)) = object.get("anyOf").cloned() {
        let kept: Vec<Value> = branches
            .into_iter()
            .filter(|b| b.get("type") != Some(&Value::String("null".into())))
            .collect();
        if kept.len() == 1 {
            object.remove("anyOf");
            if let Value::Object(inner) = &kept[0] {
                for (key, value) in inner {
                    // The field's own description wins over the type's.
                    object.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
    }
}

/// Which cloud a `cloud_*` tool acts on: `aws`, `gcp` or `modal`.
pub type CloudArg = String;

/// `cloud_status`: the clouds Spaces can be created in, and what Cua
/// created in them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudStatus {
    /// Only this cloud (`aws`, `gcp`, `modal`). Default: every one.
    #[serde(default)]
    pub provider: Option<CloudArg>,
}

/// Where in a cloud account Spaces go (`cloud_connect`, `cloud_test`).
/// Credentials are never arguments: each cloud uses its own CLI's
/// sign-in (the AWS profile, the gcloud account, the Modal profile) and
/// Cua stores only these names.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudTarget {
    /// `aws`, `gcp` or `modal`.
    pub provider: CloudArg,
    /// AWS: the profile in `~/.aws/config` (default: `default`, or
    /// `AWS_PROFILE`). Modal: the profile in `~/.modal.toml` (default: the
    /// active one).
    #[serde(default)]
    pub profile: Option<String>,
    /// AWS region (default: the profile's, else `us-west-2`) or GCP region
    /// (default `us-central1`).
    #[serde(default)]
    pub region: Option<String>,
    /// GCP zone in the region (default: its `-a` zone).
    #[serde(default)]
    pub zone: Option<String>,
    /// GCP project id (default: `gcloud config get project`).
    #[serde(default)]
    pub project: Option<String>,
    /// Modal environment (default: the profile's).
    #[serde(default)]
    pub environment: Option<String>,
}

/// `cloud_connect`: remember where Spaces go in a cloud account, after
/// the same checks `cloud_test` runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudConnect {
    #[serde(flatten)]
    pub target: CloudTarget,
    /// Also make it the default location (`default.on`). Default false.
    #[serde(default)]
    pub make_default: Option<bool>,
    /// Every Space created there deletes itself after this many hours
    /// (the instance or sandbox terminates, the sweeper removes what is
    /// left). Default 8; 0 keeps Spaces until you delete them (Modal caps
    /// a sandbox at 24).
    #[serde(default)]
    pub ttl_hours: Option<u32>,
}

/// `cloud_test`: check a cloud account without creating anything.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudTest {
    #[serde(flatten)]
    pub target: CloudTarget,
}

/// `cloud_disconnect`: forget a connected cloud.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudDisconnect {
    /// `aws`, `gcp` or `modal`.
    pub provider: CloudArg,
}

/// `cloud_sweep`: find (and with `dry_run: false`, delete) what Cua
/// created in your clouds and no longer needs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudSweep {
    /// Only this cloud. Default: every connected one.
    #[serde(default)]
    pub provider: Option<CloudArg>,
    /// List what would be deleted without deleting it. Default true.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Also delete Cua resources that have not expired yet (every Space
    /// in that cloud). Default false: only expired ones, and resources no
    /// Space refers to any more.
    #[serde(default)]
    pub all: Option<bool>,
}
