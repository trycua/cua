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
    /// images (and no image) and, on this machine's containers and microVMs,
    /// for any other image (a static cua-spacesd is put into it); false
    /// elsewhere. false runs the image as it is. A Space without it has an
    /// empty capability set: only its services work.
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

/// `list_spaces`: every registered Space and relay machine.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListSpaces {
    /// Only the Spaces that carry every one of these labels (set by
    /// `create_space`).
    #[serde(default)]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

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

/// What kind of computer a Space is.
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
    /// A microVM: the image's container rootfs booted with its own kernel
    /// (Firecracker on Linux with KVM; on a Mac, Virtualization.framework or
    /// Apple's Containerization).
    Microvm,
}

impl SpaceKind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceKind::Auto => "auto",
            SpaceKind::Container => "container",
            SpaceKind::Vm => "vm",
            SpaceKind::Microvm => "microvm",
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
    /// Where it runs: `local` (this computer, free), `auto` (the
    /// least-loaded of the user's computers that can take it: this one and
    /// the ones signed in to the user's account that provide Spaces, Modal
    /// only when connected and no computer has room), `aws`, `gcp` or
    /// `modal` (your own cloud account, billed there), or `host:<computer>`:
    /// one of the user's own computers that
    /// provides Spaces (`cua host setup --share both` on it), named by
    /// id or by words from its name ("host:spare mac mini" matches "Mac
    /// mini (spare)" or `dillons-mac-mini`). Default: the user's default
    /// location (`cua config set default.on`, `CUA_DEFAULT_ON`), else
    /// `local`; never `auto` unless asked. Not `direct:<addr>`: that is one already-running Space at a
    /// fixed address, attached with `add_space`, not a host this can create
    /// on. An existing computer is added with `add_space`, not created.
    #[serde(default)]
    pub on: Option<String>,
    /// What kind of computer. Default `auto` (from the image).
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
    /// Keep it on the user's machines (with `on` `auto`): when its machine
    /// goes away it is created again on another one, with the same name
    /// (its disk starts fresh); `list_spaces` shows it with `durable` (its
    /// name), and `delete_space` deletes it for good. Default false.
    #[serde(default)]
    pub durable: Option<bool>,
    /// With `on` a linked cloud (`aws`, `hetzner`, ...): the machine type its
    /// host must be (`c7g.xlarge`, `cax41`). Default: the elastic pool picks
    /// the cheapest that fits (cloud_plan shows it).
    #[serde(default)]
    pub instance_type: Option<String>,
    /// With `on` a linked cloud: the region (Hetzner location) its host must
    /// be in. Default: any the cloud offers.
    #[serde(default)]
    pub region: Option<String>,
    /// Labels kept with the Space and shown by `list_spaces` (`labels`): up
    /// to 64 pairs; keys and values are not empty; keys up to 63 bytes,
    /// values up to 256. Not with `durable` or `reuse`.
    #[serde(default)]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
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

/// `stop_space`: turn a Space off.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StopSpace {
    /// Space id or name.
    pub space: SpaceArg,
    /// Suspend it instead: keep its memory so it resumes where it was (a
    /// local container or QEMU VM). Default false: stop it, keeping its
    /// disk and giving its memory back to the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend: Option<bool>,
}

/// `resize_space`: give a Space more or fewer vCPUs, more or less memory,
/// or a bigger disk.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResizeSpace {
    /// Space id or name.
    pub space: SpaceArg,
    /// vCPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    /// Grow the disk to this many GiB (a disk never shrinks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u32>,
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
    /// Provider key variables to forward from this server's environment
    /// (or from the keys saved in Cua Spaces, Settings → Connections), for
    /// example ["ANTHROPIC_API_KEY"]. Names only; values never pass through
    /// the conversation. Saved keys the harness reads are added without
    /// being named.
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
    /// Extra MCP servers. The Space's own tools are included unless
    /// `sandbox_mcp` is false.
    #[serde(default)]
    pub mcp_servers: Vec<AgentMcpServer>,
    /// Stop (resumably) once the prompt is answered. Default false.
    #[serde(default)]
    pub exit_when_idle: Option<bool>,
    /// Open a terminal on the Space's desktop following the run. Default
    /// false.
    #[serde(default)]
    pub show: Option<bool>,
    /// The wire format of `base_url`: `anthropic`, `openai-responses`,
    /// `openai-chat` or `gemini`. Default: the harness's own.
    #[serde(default)]
    pub wire: Option<String>,
    /// Host files attached to the first prompt (absolute paths on this
    /// server's computer, at most 8 MiB each).
    #[serde(default)]
    pub files: Vec<String>,
    /// Give the agent the Space's own MCP tools. Default true.
    #[serde(default)]
    pub sandbox_mcp: Option<bool>,
    /// Copy the cua skills into the harness. Default true.
    #[serde(default)]
    pub skills: Option<bool>,
    /// A label for listings.
    #[serde(default)]
    pub label: Option<String>,
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
    #[serde(default)]
    pub run_id: String,
    /// Or an agent member of the Space by name (its current run). Give
    /// exactly one of `run_id` and `name`.
    #[serde(default)]
    pub name: Option<String>,
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
    #[serde(default)]
    pub run_id: String,
    /// Or an agent member of the Space by name (its current run). Give
    /// exactly one of `run_id` and `name`.
    #[serde(default)]
    pub name: Option<String>,
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
    #[serde(default)]
    pub run_id: String,
    /// Or an agent member of the Space by name (its current run). Give
    /// exactly one of `run_id` and `name`.
    #[serde(default)]
    pub name: Option<String>,
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
    #[serde(default)]
    pub run_id: String,
    /// Or an agent member of the Space by name (its current run). Give
    /// exactly one of `run_id` and `name`.
    #[serde(default)]
    pub name: Option<String>,
}

/// `agent_stop`: stop a run and verify it died.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentStop {
    /// Space id or name.
    pub space: SpaceArg,
    /// `run_id` from `agent_start`.
    #[serde(default)]
    pub run_id: String,
    /// Or an agent member of the Space by name (its current run). Give
    /// exactly one of `run_id` and `name`.
    #[serde(default)]
    pub name: Option<String>,
}

/// `agent_capabilities`: takes no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentCapabilities {}

/// `agent_add` (formerly `persistent_agent_create`): a named agent whose memory outlives its runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentCreate {
    /// The agent's name: 1-63 of a-z, 0-9, `.`, `_`, `-`. Its home is
    /// `agents/<name>/` in the Cua Volume.
    pub name: String,
    /// Harness id. `agent_capabilities` lists what each needs.
    #[schemars(schema_with = "agent_id_schema")]
    #[serde(alias = "harness")]
    pub agent: String,
    /// The Space it works in (id or name).
    pub space: SpaceArg,
    /// Model id.
    #[serde(default)]
    pub model: Option<String>,
    /// A custom model endpoint base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider key variables forwarded from this server's environment (or
    /// the keys saved in Cua Spaces) at every start, for example
    /// ["ANTHROPIC_API_KEY"]. Names only.
    #[serde(default)]
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets; see `env_from_host`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Re-drive the agent's runs after a crash. Default false.
    #[serde(default)]
    pub durable: Option<bool>,
    /// Where it runs: `local` (default), `auto` or a machine.
    #[serde(default)]
    pub on: Option<String>,
    /// Remove the member when its first task ends. Default false.
    #[serde(default)]
    pub once: Option<bool>,
    /// Parallel sessions. Default 1.
    #[serde(default)]
    pub sessions: Option<u32>,
    /// A schedule: every N minutes (with `prompt`).
    #[serde(default)]
    pub every_minutes: Option<i64>,
    /// A schedule: every day at `HH:MM` local time (with `prompt`).
    #[serde(default)]
    pub daily_at: Option<String>,
    /// A schedule: every week at `<weekday> HH:MM` local time (with `prompt`).
    #[serde(default)]
    pub weekly_on: Option<String>,
    /// What the schedule asks each time.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The schedule's title. Default: the first words of `prompt`.
    #[serde(default)]
    pub title: Option<String>,
    /// Who can give it tasks, control it and answer its approvals. Default:
    /// `task` members, `control` admins, `approve` admins (the owner always
    /// can).
    #[serde(default)]
    pub rights: Option<AgentRights>,
}

/// Who holds one of an agent member's rights.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum RightHolders {
    /// `owner`, `admins` (the owner and team admins) or `members` (also the
    /// people the Space is shared with to edit, and its agent members).
    Group(String),
    /// These accounts (relay account ids), besides the owner.
    Accounts(Vec<String>),
}

/// An agent member's rights. Guests (view-only shares) never hold any.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentRights {
    /// Who can @mention it to give it a task (also `agent_send` and
    /// `agent_message`). Default `members`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<RightHolders>,
    /// Who can interrupt, stop, pause, resume or take control of it.
    /// Default `admins`; whoever asked for its current task can also stop
    /// that task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<RightHolders>,
    /// Who can answer its approval requests. Default `admins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<RightHolders>,
}

/// `agent_set_rights`: change who can task, control and approve an agent
/// member. Omitted rights stay as they are.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentSetRights {
    /// The agent member's name.
    pub name: String,
    /// Who can give it tasks: `owner`, `admins`, `members` or account ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<RightHolders>,
    /// Who can interrupt, stop, pause, resume or take control of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<RightHolders>,
    /// Who can answer its approval requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<RightHolders>,
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

/// `agent_send` (formerly `persistent_agent_send`): give a persistent agent a turn.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PersistentAgentSend {
    /// The persistent agent's name.
    pub name: String,
    /// The message (the prompt of a new run, or a follow-up).
    pub text: String,
    /// The agent's Space (id or name); when given, the agent must be a
    /// member of it. Needed for an agent in a Space another machine
    /// provides (a teammate's): the message goes to that machine. Default:
    /// the agent is found by its name here.
    #[serde(default)]
    pub space: Option<SpaceArg>,
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

/// `agent_schedule_add` (formerly `routine_add`): a recurring turn of a persistent agent. Give exactly one
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

/// `agent_schedule_list` (formerly `routine_list`).
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

/// `agent_schedule_set_enabled` (formerly `routine_set_enabled`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutineSetEnabled {
    /// The routine's id.
    pub id: String,
    /// On or off.
    pub enabled: bool,
}

/// `thread_read`: a Space's thread after a cursor.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadRead {
    /// Space id or name.
    pub space: SpaceArg,
    /// Entries after this `seq` (the `cursor` of the previous call), oldest
    /// first. Without it: the newest `limit` entries (before `before`).
    #[serde(default)]
    pub after: Option<u64>,
    /// With no `after`: the newest entries before this `seq` (paging back).
    /// Default: the end of the thread.
    #[serde(default)]
    pub before: Option<u64>,
    /// Most entries to return (1-500). Default 100.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Advance your read cursor to the last entry returned. Default false.
    #[serde(default)]
    pub mark_read: Option<bool>,
    /// With `after`: wait up to this many milliseconds (at most 25000) for
    /// a new entry when there is none after it. Default 0 (answer at once).
    #[serde(default)]
    pub follow_ms: Option<u64>,
    /// With `follow_ms`: the `live.key` of the previous call. The wait also
    /// ends when an agent's live stage (`live.agents`) is no longer the one
    /// that key names, so a reader sees "thinking", "using Bash" as it changes.
    #[serde(default)]
    pub live_after: Option<String>,
    /// Only your DM with this agent member (its handle): its entries, yours
    /// that task or @mention it, and their replies, task updates, answers
    /// and notes. Paging, `unread` and `mark_read` then apply to the DM,
    /// with its own read cursor.
    #[serde(default)]
    pub with: Option<String>,
}

/// An approval request in a thread entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadApprovalArg {
    /// What would happen, in a few words ("buy", "deploy").
    pub action: String,
    /// The details the person reads before answering.
    #[serde(default)]
    pub details: Option<String>,
}

/// An answer to an approval entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadAnswerArg {
    /// The `seq` of the approval entry.
    pub approval: u64,
    /// Approve (true) or deny (false).
    pub approve: bool,
    /// A note for the agent.
    #[serde(default)]
    pub note: Option<String>,
}

/// `thread_post`: post to a Space's thread.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadPost {
    /// Space id or name.
    pub space: SpaceArg,
    /// The message (up to 32 KiB). `@name` of an agent member of the Space
    /// makes it a task for that agent.
    pub text: String,
    /// The `seq` of the entry this answers.
    #[serde(default)]
    pub reply_to: Option<u64>,
    /// Up to 16 files: Cua Volume paths, or (for this machine's own user)
    /// local file paths, which are copied into
    /// `spaces/<space>/thread/files/<entry id>/`.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Ask the people in the thread to approve something.
    #[serde(default)]
    pub approval: Option<ThreadApprovalArg>,
    /// Answer an approval entry.
    #[serde(default)]
    pub answer: Option<ThreadAnswerArg>,
    /// With an @mention of an agent member that runs several sessions:
    /// which session (1-based) takes the task. Default: any free session
    /// (the lowest idle one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_session: Option<u32>,
    /// What the person is looking at in the Space as they post: its
    /// desktop, or one of its windows. Stored with the entry (`view` in
    /// `thread_read` and `thread_inbox`), so an agent knows which window a
    /// message is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ThreadViewArg>,
}

/// `thread_post`'s `view`: the Space's desktop or one of its windows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadViewArg {
    /// `desktop` or `window`.
    pub kind: String,
    /// The window's app ("Firefox"), at most 128 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The window's title ("Pull requests"); longer than 256 characters is cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The window's id in the Space (as `computer_window_list` gives it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
}

/// `thread_inbox`: open tasks addressed to an agent member.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInbox {
    /// Space id or name.
    pub space: SpaceArg,
    /// The agent member. Default: every agent member of the Space.
    #[serde(default)]
    pub member: Option<String>,
    /// When there is no open task: wait up to this many milliseconds (at
    /// most 25000) for one. Default 0 (answer at once).
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// `thread_join`: an agent member sets how it looks in its Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadJoin {
    /// Space id or name (the agent's own Space).
    pub space: SpaceArg,
    /// The name people see (1-32 characters). Not a person's name, another
    /// member's handle, or "Cua".
    pub name: String,
    /// The harness you run in: `hermes`, `claude-code`, `codex`, `cursor`,
    /// `openclaw`, `t3code` or `other`. Shown as reported by you.
    #[serde(default)]
    pub harness: Option<String>,
    /// Your harness's version (at most 32 characters).
    #[serde(default)]
    pub version: Option<String>,
    /// `harness` (your harness's mark) or an https:// image URL.
    #[serde(default)]
    pub icon: Option<String>,
}

/// `thread_join_code_create`: a one-time code an agent joins the Space with.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadJoinCodeCreate {
    /// Space id or name.
    pub space: SpaceArg,
    /// Seconds until the code expires: 900 (15 minutes) to 86400 (1 day).
    /// Default 3600.
    #[serde(default)]
    pub ttl_s: Option<u64>,
    /// Who can give the agent tasks: `owner` (whoever made the code) or
    /// `members`. Default `owner`.
    #[serde(default)]
    pub can_task: Option<String>,
    /// Ask the owner or an admin before the agent joins. Default false.
    #[serde(default)]
    pub ask_before_join: Option<bool>,
}

/// A Space's join codes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadJoinCodeList {
    /// Space id or name.
    pub space: SpaceArg,
}

/// `thread_join_code_cancel`: an unused code stops working.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadJoinCodeCancel {
    /// Space id or name.
    pub space: SpaceArg,
    /// The code's `id` (from thread_join_code_create or _list).
    pub id: String,
}

/// The computer redeeming a join code.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct JoinDeviceArg {
    /// This computer's device id.
    pub id: String,
    /// Its name ("Ada's MacBook Pro").
    #[serde(default)]
    pub name: String,
    /// Its device public key (base64url), when it has one: later calls are
    /// signed with it.
    #[serde(default)]
    pub public_key: Option<String>,
}

/// `thread_join_redeem`: redeem a join code on this computer (`cua join`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadJoinRedeem {
    /// The code (`7KQ2-9FXA`) or the link that ends in it.
    pub code: String,
    /// The computer the agent runs on.
    pub device: JoinDeviceArg,
}

/// `thread_settings`: a Space's thread settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSettings {
    /// Space id or name.
    pub space: SpaceArg,
    /// "Ask before members' agents join". Omit to read the settings.
    #[serde(default)]
    pub ask_before_join: Option<bool>,
}

/// `thread_device_revoke`: remove the agents connected from a computer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadDeviceRevoke {
    /// The computer's device id.
    pub device: String,
}

/// `thread_claim`: claim a task.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadClaim {
    /// Space id or name.
    pub space: SpaceArg,
    /// The task's `seq`.
    pub task: u64,
    /// The agent member claiming it (for a person acting for an agent).
    /// An agent's own call always claims as itself.
    #[serde(default)]
    pub member: Option<String>,
    /// The member's session that claims it (1-based; for a person acting
    /// for an agent). Default: its lowest session with no task. An agent's
    /// own call always claims for its own session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
}

/// `thread_reply`: reply under a task and update it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadReply {
    /// Space id or name.
    pub space: SpaceArg,
    /// The task's `seq`.
    pub task: u64,
    /// The reply (up to 32 KiB).
    pub text: String,
    /// Files, as in `thread_post`.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// The task's new state: `done`, `failed` or `working` (more to come).
    /// Default `done`.
    #[serde(default)]
    pub state: Option<String>,
}

/// One entry of `thread_import`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadImportEntry {
    /// A stable id from the source; an id already imported is skipped.
    pub import_id: String,
    /// The original time (Unix ms).
    pub ts_ms: u64,
    /// `person` (this machine's user) or `agent`.
    pub author_kind: String,
    /// For `agent`: the agent member who wrote it (a member of the Space).
    #[serde(default)]
    pub agent_name: Option<String>,
    /// The text.
    pub text: String,
    /// `message`, `approval` or `answer`. Default `message`.
    #[serde(default)]
    pub kind: Option<String>,
}

/// `thread_import`: a one-time import of an app's earlier conversation.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ThreadImport {
    /// Space id or name.
    pub space: SpaceArg,
    /// Where they come from (`cua-bots`, `openkoalabots`).
    pub source: String,
    /// The entries, oldest first (at most 5000 per call).
    pub entries: Vec<ThreadImportEntry>,
}

/// `agent_remove` (formerly `persistent_agent_remove`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentRemove {
    /// The agent member's name.
    pub name: String,
    /// Keep its home (`agents/<name>/`) in the Cua Volume. Default true.
    #[serde(default)]
    pub keep_home: Option<bool>,
}

/// `agent_members`: the agent members of a Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentMembers {
    /// Space id or name.
    pub space: SpaceArg,
    /// Also each agent's DM with you: `dm {unread, last}` and `needs_you`
    /// (open approvals you may answer, its latest @mention of you that you
    /// haven't answered). Default false.
    #[serde(default)]
    pub overview: Option<bool>,
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
    /// The computer (a Space id, usually `relay:<computer-id>` from `cua
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
    /// The computer. Default: every computer.
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

/// `hotspot_start`: route the Space's egress through this computer.
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
    /// `fs` (this computer) or `s3` (an S3-compatible bucket: AWS S3, R2,
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

/// `share_space`: let someone in your team watch or edit the Space.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShareSpace {
    /// Space id or name.
    pub space: SpaceArg,
    /// Who: the email address (verified by cua.ai) or account id of someone
    /// in your own team, or `team:<id>` for the whole team (as `viewer` or
    /// `editor`). Sharing is for teams: it needs a team on the Teams plan,
    /// and a whole-team share needs a team owner or admin who owns the Space.
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

/// Which cloud a `cloud_*` tool acts on: `aws`, `azure`, `gcp`, `hetzner` or
/// `modal`.
pub type CloudArg = String;

/// `cloud_status`: the clouds Spaces can be created in, and what Cua
/// created in them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudStatus {
    /// Only this cloud (`aws`, `azure`, `gcp`, `hetzner`, `modal`). Default:
    /// every one.
    #[serde(default)]
    pub provider: Option<CloudArg>,
}

/// `cloud_plan`: what the elastic pool would do for some Spaces (where
/// each goes, which machines it adds, what they cost), without doing it;
/// with `apply`, it adds the machines.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudPlan {
    /// How many Spaces. Default 1.
    #[serde(default)]
    pub count: Option<u32>,
    /// Image (`linux`, `windows`, an OCI reference). Default: linux.
    #[serde(default)]
    pub image: Option<String>,
    /// `container`, `vm` or `microvm`. Default: from the image.
    #[serde(default)]
    pub kind: Option<String>,
    /// vCPUs each. Default: what Spaces of this image used before, else
    /// the image's size.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// Memory each, MiB.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// Keep them on one machine (forks of one checkpoint).
    #[serde(default)]
    pub together: Option<bool>,
    /// Who asks: `interactive` (one Space someone waits on), `batch`,
    /// `compat` or `persistent_agent`. Default: interactive for one Space,
    /// batch for more.
    #[serde(default)]
    pub caller: Option<String>,
    /// Expected lifetime, seconds.
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Only this cloud.
    #[serde(default)]
    pub provider: Option<CloudArg>,
    /// Only this region.
    #[serde(default)]
    pub region: Option<String>,
    /// Only this machine type.
    #[serde(default)]
    pub instance_type: Option<String>,
    /// Exactly this many new machines.
    #[serde(default)]
    pub hosts: Option<u32>,
    /// Add no machines (place on what runs).
    #[serde(default)]
    pub no_scale: Option<bool>,
    /// Only the user's own devices.
    #[serde(default)]
    pub devices_only: Option<bool>,
    /// A $/hour cap for this plan (default: the budget's).
    #[serde(default)]
    pub max_usd_per_hour: Option<f64>,
    /// Add the machines the plan needs (they join the pool; create the
    /// Spaces with on="auto"). Default false: plan only.
    #[serde(default)]
    pub apply: Option<bool>,
}

/// `cloud_hosts`: the hosts the elastic pool added in your clouds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudHosts {}

/// `cloud_add_host`: adds hosts to your pool in a linked cloud by hand
/// (the scaler never removes them).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudAddHost {
    /// `aws`, `azure`, `gcp` or `hetzner`.
    pub provider: CloudArg,
    /// Machine type (`cloud_plan` and `cloud_status` list them).
    pub instance_type: String,
    /// Region. Default: the connection's.
    #[serde(default)]
    pub region: Option<String>,
    /// How many, 1 to 10. Default 1.
    #[serde(default)]
    pub count: Option<u32>,
    /// Hours until each ends on its own, 1 to 168. Default 24.
    #[serde(default)]
    pub ttl_hours: Option<u32>,
}

/// `cloud_remove_host`: removes hosts this home added.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudRemoveHost {
    /// Their names (`hetzner-1`).
    pub names: Vec<String>,
}

/// `cloud_budget`: the elastic pool's caps and switches; reads them, and
/// sets those given.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudBudget {
    /// Most USD per hour your clouds may cost at once.
    #[serde(default)]
    pub max_usd_per_hour: Option<f64>,
    /// Most USD per calendar month on elastic hosts.
    #[serde(default)]
    pub monthly_usd: Option<f64>,
    /// Add and remove cloud hosts by itself.
    #[serde(default)]
    pub autoscale: Option<bool>,
    /// Most hosts one placement adds.
    #[serde(default)]
    pub max_new_hosts: Option<u32>,
    /// Keep an idle host warm when demand predicts a Space soon.
    #[serde(default)]
    pub warm: Option<bool>,
    /// Let one small VM Space buy a KVM metal host (dollars an hour). Off:
    /// such a request is refused with the reason; batches of forks may
    /// still buy one.
    #[serde(default)]
    pub allow_metal: Option<bool>,
}

/// `cloud_reconcile`: one scale-in pass now.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudReconcile {}

/// Where in a cloud account Spaces go (`cloud_connect`, `cloud_test`).
/// AWS, Google Cloud and Modal use their own CLI's sign-in (the AWS
/// profile, the gcloud account, the Modal profile) and Cua stores only
/// these names. Hetzner and Modal can take an API token instead: `token`
/// is checked, then sealed to the account's devices (`auth: sealed`) or,
/// for Hetzner with `auth: cli`, saved in this device's Cua credential
/// store.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudTarget {
    /// `aws`, `azure`, `gcp`, `hetzner` or `modal`.
    pub provider: CloudArg,
    /// AWS: the profile in `~/.aws/config` (default: `default`, or
    /// `AWS_PROFILE`). Modal: the profile in `~/.modal.toml` (default: the
    /// active one).
    #[serde(default)]
    pub profile: Option<String>,
    /// AWS region (default: the profile's, else `us-west-2`), Hetzner location
    /// (default `fsn1`), GCP region (default `us-central1`) or Azure region
    /// (default `westus2`).
    #[serde(default)]
    pub region: Option<String>,
    /// GCP zone in the region (default: its `-a` zone).
    #[serde(default)]
    pub zone: Option<String>,
    /// GCP project id (default: `gcloud config get project`). Hetzner and
    /// Modal with a sealed token: a name for the project or workspace
    /// (default `default`), to tell several apart.
    #[serde(default)]
    pub project: Option<String>,
    /// Modal environment (default: the profile's).
    #[serde(default)]
    pub environment: Option<String>,
    /// Hetzner: the project's Read & Write API token (default:
    /// `HCLOUD_TOKEN`, the saved token, or the `hcloud` CLI's context).
    /// Modal: `<token id>:<token secret>`. Secret: never logged or
    /// returned; sealed to the account's devices on connect (or, Hetzner
    /// with `auth: cli`, saved in this device's Cua credential store).
    #[serde(default)]
    pub token: Option<String>,
}

impl std::fmt::Debug for CloudTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudTarget")
            .field("provider", &self.provider)
            .field("profile", &self.profile)
            .field("region", &self.region)
            .field("zone", &self.zone)
            .field("project", &self.project)
            .field("environment", &self.environment)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
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
    /// How Cua reaches the cloud: `cli` (the cloud CLI's sign-in, or for
    /// Hetzner a token kept on this device), `oidc` (AWS, Google Cloud,
    /// Azure: Cua's issuer signs short-lived tokens the account trusts; no
    /// key is stored anywhere; Azure is always `oidc`) or `sealed` (Hetzner,
    /// Modal: the API token is sealed to the account's devices and Cua's
    /// servers keep only ciphertext; with no `token` it seals the one this
    /// device has, or links the account's sealed token). Default: `sealed`
    /// for Hetzner and Modal when this device has the account's pool keys
    /// and a budget is given (or the link is sealed already), else `cli`.
    #[serde(default)]
    pub auth: Option<String>,
    /// The most Cua may spend there per calendar month, US dollars. New
    /// machines are refused once it is reached; stopping and deleting
    /// always works. Needed before the first machine of an `oidc` link.
    #[serde(default)]
    pub budget_usd_month: Option<f64>,
    /// The largest machine Cua may create there: `<vCPUs>x<GB>`, for
    /// example `4x16`.
    #[serde(default)]
    pub max_size: Option<String>,
    /// AWS `oidc`: the 12-digit account id the trust stack is in (the
    /// person who created it knows it).
    #[serde(default)]
    pub account_id: Option<String>,
    /// AWS `oidc`: the role ARN, only when it is not named `cua-spaces`.
    #[serde(default)]
    pub role_arn: Option<String>,
    /// Google Cloud `oidc`: the project number (the trust script prints
    /// it; giving it implies `oidc`). The project id is `target.project`.
    #[serde(default)]
    pub project_number: Option<String>,
    /// Google Cloud `oidc`: a service account to act as. Default: none,
    /// the federated identity calls Google directly.
    #[serde(default)]
    pub service_account: Option<String>,
    /// Azure: the subscription id (the trust template's
    /// `subscriptionId` output).
    #[serde(default)]
    pub subscription_id: Option<String>,
    /// Azure: the Microsoft Entra tenant id (its `tenantId` output).
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Azure: the client id of the `cua-spaces` managed identity (its
    /// `clientId` output).
    #[serde(default)]
    pub client_id: Option<String>,
    /// Azure: the resource group Cua works in, and the only one its roles
    /// reach (default `cua-spaces`).
    #[serde(default)]
    pub resource_group: Option<String>,
    /// `oidc`: run the check that proves the link works (a check token,
    /// the exchange with the cloud, a dry run; nothing is created). The
    /// link is saved only once it passes.
    #[serde(default)]
    pub check: Option<bool>,
    /// The agent making the change (Cua's agent runtime and the cua CLI
    /// set it). An agent's link, or a limit it raises, waits for a
    /// person's approval in Cua Spaces.
    #[serde(default)]
    pub agent: Option<String>,
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
    /// `aws`, `azure`, `gcp`, `hetzner` or `modal`.
    pub provider: CloudArg,
    /// Delete everything Cua created in that cloud first, and disconnect
    /// only once the cloud confirms nothing is left. Default false: the
    /// call is refused (listing them) while Cua resources still run there.
    #[serde(default)]
    pub delete: Option<bool>,
}

/// `cloud_approve`: a person approves (or denies) what an agent asked for
/// on a cloud: its link, or a raise of its limits.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudApprove {
    /// `aws`, `gcp`, `hetzner` or `modal`.
    pub provider: CloudArg,
    /// Decline instead: an agent's raise is dropped (the limits in force
    /// stay), an agent's link that was never approved is disconnected.
    /// Default false.
    #[serde(default)]
    pub deny: Option<bool>,
}

/// `cloud_audit`: what Cua did in your clouds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudAudit {
    /// How many of the newest entries (default 50, at most 1000).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Only this cloud (`aws`, `gcp`, `hetzner`, `modal`). Default: every one.
    #[serde(default)]
    pub provider: Option<CloudArg>,
}

/// `cloud_cost`: what the machines Cua created in your clouds cost,
/// estimated from list price times runtime.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CloudCost {
    /// Only this cloud (`aws`, `gcp`, `hetzner`, `modal`). Default: every one.
    #[serde(default)]
    pub provider: Option<CloudArg>,
    /// One row per `space`, `agent`, `day` (UTC) or `cloud`. Default `space`.
    #[serde(default)]
    pub by: Option<String>,
    /// The first day counted, `YYYY-MM-DD` (UTC). Default: the first of this
    /// month.
    #[serde(default)]
    pub since: Option<String>,
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

/// `prewarm`: keep Spaces of one shape ready where the elastic pool puts
/// them, so the next creates of that shape start in a fraction of a second.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Prewarm {
    /// Image (`linux`, an OCI reference).
    pub image: String,
    /// How many to keep ready, 1 to 256. Default 1.
    #[serde(default)]
    pub count: Option<u32>,
    /// Where: `auto` (default: your devices first, then hosts with room,
    /// then new cloud machines), `local`, a linked cloud or `host:<machine>`.
    #[serde(default)]
    pub on: Option<String>,
    /// `container`, `vm` or `microvm`. Default: the image's.
    #[serde(default)]
    pub kind: Option<String>,
    /// vCPUs each. Default: the size Spaces of this image use.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// Memory each, MiB.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// Seconds to keep them (default 3600, at most 604800).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// Only this cloud machine type.
    #[serde(default)]
    pub instance_type: Option<String>,
    /// Only this cloud region.
    #[serde(default)]
    pub region: Option<String>,
}

/// `prewarm_list`: the prewarms of this home, with how many are ready.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PrewarmList {}

/// `prewarm_remove`: end a prewarm (its ready Spaces go).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PrewarmRemove {
    /// The prewarm (`pw-xxxxxx`).
    pub id: String,
}
