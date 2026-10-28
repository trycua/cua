//! The Spaces tool contract, in Rust, as the single source of truth.
//!
//! `libs/cua/spaces-contract/manifest.json` is generated from this file by
//! `cua-spaces-contract-gen`. Nothing edits the JSON by hand: a manifest a
//! human can edit is a manifest that stops describing the code.
//!
//! What the manifest is *for*: a language binding needs to know, without
//! running anything, which tools exist, which kind of Space can serve them,
//! which guest capabilities they need, what a call costs, what arguments it
//! takes, and whether a failed call left damage behind. Those facts are the
//! difference between an SDK and a JSON-RPC wrapper.
//!
//! The MCP server in `cua-spaces` (`cua daemon mcp`) publishes exactly these
//! tools, with exactly these input schemas; its `tools/list` is built from
//! [`tools`], and its argument parsing uses the [`inputs`] types the schemas
//! are derived from.

pub mod inputs;

use serde::Serialize;

/// Bumped when the shape of the manifest changes.
///
/// 0.2.0: `input_schema`, `requires`, `host_requires`, `rust_symbol`,
/// `instructions` and `transports` added; the `direct` provider added;
/// `add_space` / `remove_space` added; `local_rcdp` renamed `stream_endpoint`.
///
/// 0.3.0: providers use the location words (`relay` added) and Space ids
/// the unified sandbox ref scheme (`local:<name>`, `cloud:<name>`,
/// `direct:<host:port>`, `relay:<machine-id>`; the `space://` spellings
/// still parse).
///
/// 0.4.0: `categories` and `error_kinds` added; every tool states its
/// `category`, its `result` and the error `errors` kinds specific to it.
///
/// 0.5.0: one lifecycle verb set. `create_space` (with `on`, `kind`,
/// `runtime` and `reuse`) replaces `claim_space`, `get_or_create_space` and
/// `local_provision_space`; `delete_space` replaces `release_space`;
/// metering `cloud` (metered only when it creates in the cloud) and the
/// `invalid_placement` error kind added.
///
/// 0.6.0: persistent agents (`agent_start` `home`, the
/// `persistent-agents` category: agents, routines, notifications, pause
/// and resume, per-agent computer access) and the Cua Volume error kinds.
/// Also 0.6.0: the `keyvault` category and `request_site_login` (sign in to a
/// site in a Space with a saved password, approved by the user); the
/// `login_refused` error kind.
/// Also 0.6.0: the `drive` category (Cua Volume: `volume_ls`, `volume_read`,
/// `volume_write`, `volume_delete`, `volume_history`, `volume_restore`,
/// `volume_grant`, `volume_revoke`, `volume_grants`, `volume_request_access`,
/// `volume_requests`, `volume_approve`, `volume_deny`, `volume_audit`) and its
/// error kinds. Added later in 0.6.0 (additive): the drive's runtime
/// tools (`volume_storage`, `volume_storage_set`, `volume_mount_status`,
/// `volume_mount`, `volume_unmount`, `volume_sync_status`, `volume_sync_events`,
/// `volume_sync_resolve`, `volume_cache_stats`, `volume_cache_set`,
/// `volume_cache_clear`).
///
/// 0.7.0: Cua Volume is Cua Volume. The `drive` category is `volume` and its
/// tools are `volume_*` (`volume_ls` ... `volume_cache_clear`); the error
/// kind `drive_backend` is `volume_backend`. The old tool names still work
/// for one release as hidden aliases ([`TOOL_ALIASES`]): calls resolve to
/// the new tool, and `tools/list` and the manifest name only the new ones.
/// Added later in 0.7.0 (additive): Spaces in your own cloud (the `cloud`
/// category: `cloud_status`, `cloud_connect`, `cloud_test`,
/// `cloud_disconnect`, `cloud_sweep`; stop_space and start_space stop and
/// start a Space there), the `cloud` error kind and the `cloud_account`
/// host requirement.
///
/// Added later in 0.7.0 (additive): `create_space` `durable` (a Space kept on
/// the user's machines) and `durable` in `list_spaces`.
///
/// Added later in 0.7.0 (additive): the elastic pool (`cloud_plan`,
/// `cloud_hosts`, `cloud_add_host`, `cloud_remove_host`, `cloud_budget`,
/// `cloud_reconcile`) and `create_space` `instance_type` and `region`.
///
/// Added later in 0.7.0 (additive): prewarm (`prewarm`, `prewarm_list`,
/// `prewarm_remove`).
///
/// Added later in 0.7.0 (compatible): agents in a Space. The persistent-agent
/// tools take the agent names: `agent_add`, `agent_send`, `agent_remove` and
/// `agent_schedule_add`, `agent_schedule_list`, `agent_schedule_remove`,
/// `agent_schedule_set_enabled`. The former names (`persistent_agent_create`,
/// `persistent_agent_send`, `persistent_agent_remove`, `routine_*`) still
/// work as hidden aliases ([`TOOL_ALIASES`]), and the manifest lists every
/// former name under `deprecated_tools` with the tool that replaced it.
///
/// 0.8.0 (additive): agents in a Space. Each Space has a thread
/// (`thread_read`, `thread_post`, `thread_inbox`, `thread_claim`,
/// `thread_reply`, `thread_import`) and agent members (`agent_members`, in
/// the `thread` category; `agent_add` and `agent_remove` take member
/// options, `agent_send` is recorded in the thread); `agent_message`,
/// `agent_status`, `agent_events`, `agent_interrupt` and `agent_stop` take an
/// agent member's `name` as well as a `run_id`. Each agent member has
/// rights (`task`, `control`, `approve`; `agent_set_rights`, `agent_add`
/// `rights`, `agent_members` `rights` and `you_can`); guests read only.
///
/// 0.9.0 (additive): agents connected from a computer. Member rows and
/// agent authors carry `identity` (origin, handle, display name, icon,
/// harness, owner, device, how it joined); join codes
/// (`thread_join_code_create`, `thread_join_code_list`,
/// `thread_join_code_cancel`, `thread_join_redeem`), `thread_join` (an
/// agent sets its name and icon), `thread_settings`, `thread_device_revoke`,
/// `thread_inbox` `wait_ms`, and `join_request` entries.
pub const CONTRACT_VERSION: &str = "0.9.0";
/// Bumped when the meaning of a capability string changes.
pub const CAPABILITY_VERSION: &str = "1";
/// The MCP revision the Spaces server speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// The transport the manifest was first published for (kept for readers of
/// contract 0.1).
pub const TRANSPORT: &str = "mcp_stdio";
/// Every transport the server speaks.
pub const TRANSPORTS: [&str; 2] = ["mcp_stdio", "mcp_streamable_http"];
pub const GENERATED_NOTICE: &str = "Generated by cua-spaces-contract-gen; do not edit by hand.";

/// Which kind of Space can serve a tool.
///
/// This is the axis that actually bites callers, and the one the raw tool list
/// does not publish. A cloud-only tool called against a Local Space fails at
/// the far end, late, with a message about the wrong thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Cua cloud (ended): metered, remote, provisioned.
    Cloud,
    /// A Local Space on the operator's own machine: free, and invisible to the
    /// cloud provisioning tools.
    Local,
    /// Any machine added by address (and token): a cua-spacesd, or a plain MCP
    /// endpoint.
    Direct,
    /// A machine of the signed-in account, reached through the cua.ai relay.
    Relay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

/// MCP tool annotations, same four axes as the Cua Driver contract.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Annotations {
    pub read_only: bool,
    /// True when a failed or repeated call can leave the Space different from
    /// how it was found.
    pub destructive: bool,
    pub idempotent: bool,
    /// True when the result depends on state outside this process.
    pub open_world: bool,
}

impl Annotations {
    pub const fn reading() -> Self {
        Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: true,
        }
    }
    pub const fn mutating() -> Self {
        Annotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        }
    }
    pub const fn mutating_idempotent() -> Self {
        Annotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: true,
        }
    }
}

/// What a call costs the account. Spaces meters provisioning and nothing else,
/// and a caller that cannot see which is which will eventually bill someone by
/// accident — FRICTION.md §5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metering {
    /// No charge beyond the Space already running.
    Free,
    /// Allocates or extends a metered cloud Space.
    Metered,
    /// Metered when it creates in the cloud; free locally.
    Cloud,
}

/// What a tool is for. The manifest lists the categories in this order, and
/// the tools of each category together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Lifecycle,
    CommandsAndFiles,
    StreamsAndDisplay,
    Services,
    Agents,
    PersistentAgents,
    Thread,
    Teleport,
    Keyvault,
    Hotspot,
    Volume,
    Sharing,
    Cloud,
}

/// A category's id, title and one-line purpose, as the manifest publishes it.
#[derive(Clone, Debug, Serialize)]
pub struct CategoryInfo {
    pub id: Category,
    pub title: &'static str,
    pub description: &'static str,
}

/// Every category, in manifest order.
pub fn categories() -> Vec<CategoryInfo> {
    use Category::*;
    let c = |id, title, description| CategoryInfo {
        id,
        title,
        description,
    };
    vec![
        c(
            Lifecycle,
            "Space lifecycle",
            "Register, provision, list and release Spaces.",
        ),
        c(
            CommandsAndFiles,
            "Commands and files",
            "Run shell commands and move files in and out of a Space.",
        ),
        c(
            StreamsAndDisplay,
            "Streams and display",
            "Stream a Space's desktop or windows, and show a Space to the operator.",
        ),
        c(
            Services,
            "Services",
            "Discover and call the MCP services inside a Space.",
        ),
        c(Agents, "Agents", "Run coding agents inside a Space."),
        c(
            PersistentAgents,
            "Persistent agents",
            "Named agents whose memory outlives their Space: homes in Cua Volume, routines, notifications, pause and resume, access to the user's computers.",
        ),
        c(
            Thread,
            "Space thread and members",
            "Each Space's conversation: people and agent members post, an @mention wakes an agent, and anyone can check in from any device.",
        ),
        c(
            Teleport,
            "Session teleport",
            "Move a signed-in app session from this computer into a Space.",
        ),
        c(
            Keyvault,
            "Keyvault",
            "Use the user's saved credentials in a Space, only with their approval.",
        ),
        c(
            Hotspot,
            "Hotspot",
            "Share this computer's network with a Space.",
        ),
        c(
            Volume,
            "Cua Volume",
            "Read and write the volume every Space and agent shares, decide who may reach what, and see where it is stored and how it syncs.",
        ),
        c(
            Sharing,
            "Sharing",
            "Let other cua.ai accounts watch or edit a Space, through the relay.",
        ),
        c(
            Cloud,
            "Your cloud",
            "Create Spaces in your own AWS, Google Cloud, Hetzner or Modal account: connect it, check it, and clean up what Cua created there.",
        ),
    ]
}

/// A tool error kind: the `structuredContent.error.kind` of a failed call
/// (`cua_spaces::Error::tag`), what it means and what to do about it.
#[derive(Clone, Debug, Serialize)]
pub struct ErrorKind {
    pub kind: &'static str,
    pub meaning: &'static str,
    pub fix: &'static str,
}

/// Every error kind a tool can return. `cua-spaces` tests that each of its
/// error tags is listed here.
pub const ERROR_KINDS: &[ErrorKind] = &[
    ErrorKind {
        kind: "invalid_argument",
        meaning: "An argument is missing, malformed or out of range.",
        fix: "Check the call against the tool's input schema; the message names the argument.",
    },
    ErrorKind {
        kind: "not_found",
        meaning: "No Space, run, window, service, tool or file with that id.",
        fix: "List what exists first (list_spaces, agent_list, list_space_windows, list_tools).",
    },
    ErrorKind {
        kind: "spacesd_not_available",
        meaning: "The computer answered but runs no cua-spacesd, so the spacesd primitives cannot work.",
        fix: "Use an image that runs cua-spacesd (for example ghcr.io/trycua/linux:24.04), or reach the Space's own services with list_tools and call_tool.",
    },
    ErrorKind {
        kind: "capability_missing",
        meaning: "The Space's cua-spacesd reports a feature this tool needs as unsupported; the message names it and the driver's limitation.",
        fix: "Use a Space whose spacesd supports the feature (see the tool's `requires`).",
    },
    ErrorKind {
        kind: "host_capability_missing",
        meaning: "A prerequisite on this computer is missing: a local runtime, an operator display or app-session providers.",
        fix: "Set up what the message names (see the tool's `host_requires`).",
    },
    ErrorKind {
        kind: "wrong_provider",
        meaning: "The operation does not apply to this kind of Space.",
        fix: "Use a tool that serves the Space's provider (see the tool's `providers`).",
    },
    ErrorKind {
        kind: "unauthenticated",
        meaning: "The Space's spacesd or the cua.ai relay rejected the token.",
        fix: "Re-add the Space with a valid token, or sign in again with `cua auth login`.",
    },
    ErrorKind {
        kind: "permission_denied",
        meaning: "The signed-in account may not reach that relay computer.",
        fix: "Sign in with the account that owns the computer.",
    },
    ErrorKind {
        kind: "ambiguous_host",
        meaning: "The computer name given to on=\"host:<name>\" fits several of your computers equally well; the message lists them.",
        fix: "Ask the user which computer they meant, then pass on=\"host:<computer id>\" from the list.",
    },
    ErrorKind {
        kind: "limit_exceeded",
        meaning: "The host is at a capacity limit: two macOS VMs per Mac (Apple's macOS license, enforced by Apple Virtualization) or the host's own Space limit.",
        fix: "Delete a Space on that host (delete_space), pick another computer, or create a Linux Space instead.",
    },
    ErrorKind {
        kind: "host_capacity",
        meaning: "The machine has no room for another Space now: its VMs are at their memory limit (shared snapshot memory counted once) or CPU limit, or it is low on memory. Nothing was started; the message says which limit and by how much.",
        fix: "Delete a Space (delete_space), create it smaller (cpus, memory_mb), or create it with on=\"auto\" so another machine takes it.",
    },
    ErrorKind {
        kind: "ambiguous_sandbox",
        meaning: "A short name matches more than one sandbox.",
        fix: "Use the full id (`local:<name>` or `cloud:<name>`).",
    },
    ErrorKind {
        kind: "transfer_failed",
        meaning: "A file transfer did not land, or landed with a different sha256.",
        fix: "Retry; check free space and permissions at the destination.",
    },
    ErrorKind {
        kind: "teleport_refused",
        meaning: "The teleport consent gate refused: a sensitive item without acknowledge_sensitive, a declined approval, or a Space that cannot import the app.",
        fix: "Read teleport_manifest, then pass the items to move and acknowledge_sensitive=true for sensitive ones.",
    },
    ErrorKind {
        kind: "login_refused",
        meaning: "The Keyvault refused a site login: the user declined, no saved password matches the page's origin, the tab is on another origin, or the Keyvault is locked or off.",
        fix: "Ask the user to approve the sign-in in Cua, import the site's saved password, or open the site's sign-in page and try again.",
    },
    ErrorKind {
        kind: "agent",
        meaning: "An agent run failed outside the transport: install, configuration or a refused message.",
        fix: "Read agent_status and its output_tail for the reason.",
    },
    ErrorKind {
        kind: "timeout",
        meaning: "A wait ran out: a claim that never bound, a Space that never became ready, or a command past its timeout.",
        fix: "Retry, or raise the tool's timeout.",
    },
    ErrorKind {
        kind: "cancelled",
        meaning: "The create was cancelled (cancel_create, or its caller went away); what it made is gone.",
        fix: "Nothing to clean up; create it again when you want it.",
    },
    ErrorKind {
        kind: "stream",
        meaning: "The media plane failed to open or run the stream.",
        fix: "Check that the Space's spacesd supports desktop_stream or window_stream, then retry.",
    },
    ErrorKind {
        kind: "env",
        meaning: "A cua-spacesd call failed.",
        fix: "Retry; if it persists, check the spacesd log in the Space.",
    },
    ErrorKind {
        kind: "invalid_placement",
        meaning: "The location, kind or runtime does not exist, or the combination (with the image) does not.",
        fix: "Use one of the values the message lists, or leave kind and runtime unset (auto).",
    },
    ErrorKind {
        kind: "sandbox",
        meaning: "Starting, stopping or deleting the Space's sandbox failed.",
        fix: "Check the local runtime (`cua doctor`) or the cloud pool, then retry.",
    },
    ErrorKind {
        kind: "cloud",
        meaning: "Your cloud account refused or failed a call Cua made for a Space there (AWS, Google Cloud, Hetzner, Modal): a missing permission, a quota, a region without the instance type. The message names the cloud's own error.",
        fix: "Fix what the message names in that account (`cua cloud test <provider>` checks without creating anything), or connect another region or project.",
    },
    ErrorKind {
        kind: "relay",
        meaning: "The cua.ai relay directory failed.",
        fix: "Retry; check `cua auth status`.",
    },
    ErrorKind {
        kind: "mcp",
        meaning: "An MCP service inside the Space failed.",
        fix: "Check the service with list_tools; the message carries its error.",
    },
    ErrorKind {
        kind: "io",
        meaning: "Reading or writing a local file failed.",
        fix: "Check the path and its permissions on this computer.",
    },
    ErrorKind {
        kind: "json",
        meaning: "A reply could not be parsed.",
        fix: "Retry; report it if it persists.",
    },
    ErrorKind {
        kind: "forbidden",
        meaning: "The drive's access rules refuse this principal on this path (an agent outside its own home, its Space's folder, read-only public/ and its grants).",
        fix: "Ask for access with volume_request_access; the user approves it in Cua.",
    },
    ErrorKind {
        kind: "precondition_failed",
        meaning: "A drive write or delete with if_etag or create_only found a different version at the path.",
        fix: "Read the file again and retry with its current etag.",
    },
    ErrorKind {
        kind: "secret_detected",
        meaning: "A write under agents/ carries something that looks like a secret (an API key, a token, a private key); it was not stored. The message names the kind and line, never the value.",
        fix: "Remove the secret from the file; keep credentials in the Keyvault.",
    },
    ErrorKind {
        kind: "lease_held",
        meaning: "Another run holds the persistent agent's home (one writer at a time).",
        fix: "Wait for that run to stop, or stop it; the lease expires on its own.",
    },
    ErrorKind {
        kind: "not_signed_in",
        meaning: "Redeeming a join code needs the redeemer's Cua sign-in, and this computer is not signed in.",
        fix: "Sign in (cua auth login), then redeem the code again.",
    },
    ErrorKind {
        kind: "join_code_refused",
        meaning: "The join code was used, cancelled or has expired, or it is for a Space on another computer. The daemon never says which.",
        fix: "Ask for a new link in Add an agent.",
    },
    ErrorKind {
        kind: "rate_limited",
        meaning: "Too many wrong join codes from this account, or too many posts from this agent (30 a minute, 600 an hour).",
        fix: "Wait the time the message gives, then try again.",
    },
    ErrorKind {
        kind: "task_claimed",
        meaning: "Another session of the agent (or a newer drive of the same session) holds the task's claim; the message names the holder (`<member>/<session>@<epoch>`).",
        fix: "Leave the task to its holder; it is queued again on its own if the holder's lease runs out.",
    },
    ErrorKind {
        kind: "not_confirmed",
        meaning: "The user did not confirm with presence (Touch ID, the login password or the vault passphrase), so no access was widened.",
        fix: "Ask the user to approve it in Cua.",
    },
    ErrorKind {
        kind: "volume_backend",
        meaning: "The drive's store failed or is not configured (a bucket that refused, missing keys, or a backend this build cannot serve).",
        fix: "Check `cua volume config show` and the message.",
    },
];

/// Host-side prerequisites a tool can have. The guest-side ones are
/// spacesd feature names (see [`Tool::requires`]).
pub mod host {
    /// A local runtime (Lume, QEMU, or a container engine) is available.
    pub const LOCAL_RUNTIME: &str = "local_runtime";
    /// Something draws on the operator's desktop (the Cua Spaces app).
    pub const OPERATOR_DISPLAY: &str = "operator_display";
    /// The host can read app sessions (teleport sender side).
    pub const APP_SESSIONS: &str = "app_sessions";
    /// Signed in to a cua.ai account on a relay (`cua auth login`).
    pub const RELAY_ACCOUNT: &str = "relay_account";
    /// A cloud account connected with `cua cloud connect`.
    pub const CLOUD_ACCOUNT: &str = "cloud_account";
}

#[derive(Clone, Debug, Serialize)]
pub struct Tool {
    pub name: &'static str,
    /// The category it is listed under ([`categories`]).
    pub category: Category,
    /// One line.
    pub description: &'static str,
    /// Everything a model needs to call the tool well. The MCP description is
    /// `description`, a blank line, then this.
    pub instructions: &'static str,
    pub platforms: Vec<Platform>,
    pub providers: Vec<Provider>,
    pub metering: Metering,
    pub capabilities: Vec<&'static str>,
    /// cua-spacesd features (`GetCapabilities.features[].name`) the Space
    /// must report as supported. Checked before the call; a missing one is a
    /// `capability_missing` error naming it, never a late failure. A name
    /// ending in `.*` means "the feature for the requested item" (for
    /// example `teleport.firefox`).
    pub requires: Vec<&'static str>,
    /// Host-side prerequisites ([`host`]).
    pub host_requires: Vec<&'static str>,
    pub annotations: Annotations,
    /// The typed Swift path that covers this tool. The coverage table in
    /// `libs/spaces-sdk-swift/README.md` is generated-adjacent prose; this is
    /// the machine-readable form of the same claim, and a follow-on binding
    /// should be able to diff its own surface against it.
    pub sdk_symbol: &'static str,
    /// The Rust API in `cua-spaces` that implements the tool.
    pub rust_symbol: &'static str,
    /// Stated where the tool's own documentation is silent or was wrong.
    pub notes: Vec<&'static str>,
    /// What a successful call returns: the JSON (or text) of its content.
    pub result: &'static str,
    /// Error kinds ([`ERROR_KINDS`]) specific to this tool, beyond the ones
    /// every tool, every Space-scoped tool and every `requires` or
    /// `host_requires` entry implies.
    pub errors: Vec<&'static str>,
    /// JSON Schema of the arguments object, derived from [`inputs`].
    pub input_schema: serde_json::Value,
}

impl Tool {
    /// The description MCP `tools/list` publishes.
    pub fn mcp_description(&self) -> String {
        if self.instructions.is_empty() {
            self.description.to_string()
        } else {
            format!("{}\n\n{}", self.description, self.instructions)
        }
    }
}

#[derive(Serialize)]
pub struct Manifest {
    pub generated_notice: &'static str,
    pub experimental: bool,
    pub contract_version: &'static str,
    pub capability_version: &'static str,
    pub mcp_protocol_version: &'static str,
    pub transport: &'static str,
    pub transports: Vec<&'static str>,
    pub tool_count: usize,
    pub categories: Vec<CategoryInfo>,
    pub error_kinds: &'static [ErrorKind],
    pub tools: Vec<Tool>,
    /// Former tool names ([`TOOL_ALIASES`]): still accepted by `tools/call`,
    /// never in `tools/list`.
    pub deprecated_tools: Vec<DeprecatedTool>,
}

/// A former tool name and the tool that replaced it.
#[derive(Clone, Debug, Serialize)]
pub struct DeprecatedTool {
    pub name: &'static str,
    /// Always true: the name still works but is not advertised.
    pub deprecated: bool,
    pub replaced_by: &'static str,
}

/// [`TOOL_ALIASES`] as manifest entries.
pub fn deprecated_tools() -> Vec<DeprecatedTool> {
    TOOL_ALIASES
        .iter()
        .map(|(name, replaced_by)| DeprecatedTool {
            name,
            deprecated: true,
            replaced_by,
        })
        .collect()
}

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];
/// The operator-display tools draw on the *operator's* desktop, not in the
/// Space. Today that compositor path exists on macOS only.
const OPERATOR_PLATFORMS: [Platform; 1] = [Platform::Macos];
const ALL_PROVIDERS: [Provider; 4] = [
    Provider::Cloud,
    Provider::Local,
    Provider::Direct,
    Provider::Relay,
];
/// Where `create_space` creates: the locations with a sandbox backend.
const CREATE_PROVIDERS: [Provider; 3] = [Provider::Cloud, Provider::Local, Provider::Relay];

/// Everything about one tool except its schema, which comes from a type.
struct Spec {
    name: &'static str,
    category: Category,
    result: &'static str,
    errors: &'static [&'static str],
    description: &'static str,
    instructions: &'static str,
    platforms: &'static [Platform],
    providers: &'static [Provider],
    metering: Metering,
    capabilities: &'static [&'static str],
    requires: &'static [&'static str],
    host_requires: &'static [&'static str],
    annotations: Annotations,
    sdk_symbol: &'static str,
    rust_symbol: &'static str,
    notes: &'static [&'static str],
}

impl Spec {
    const fn new(name: &'static str, description: &'static str) -> Self {
        Spec {
            name,
            category: Category::Lifecycle,
            result: "",
            errors: &[],
            description,
            instructions: "",
            platforms: &ALL_PLATFORMS,
            providers: &ALL_PROVIDERS,
            metering: Metering::Free,
            capabilities: &[],
            requires: &[],
            host_requires: &[],
            annotations: Annotations::reading(),
            sdk_symbol: "",
            rust_symbol: "",
            notes: &[],
        }
    }

    fn with<T: schemars::JsonSchema>(self) -> Tool {
        Tool {
            name: self.name,
            category: self.category,
            result: self.result,
            errors: self.errors.to_vec(),
            description: self.description,
            instructions: self.instructions,
            platforms: self.platforms.to_vec(),
            providers: self.providers.to_vec(),
            metering: self.metering,
            capabilities: self.capabilities.to_vec(),
            requires: self.requires.to_vec(),
            host_requires: self.host_requires.to_vec(),
            annotations: self.annotations,
            sdk_symbol: self.sdk_symbol,
            rust_symbol: self.rust_symbol,
            notes: self.notes.to_vec(),
            input_schema: inputs::input_schema::<T>(),
        }
    }
}

const SEND_FILE_INSTRUCTIONS: &str = "The same transfer the Cua Spaces pop-out's Teleport drop zone performs: files land in the Space user's ~/Downloads through the spacesd's TeleportService (chunked, resumable, every file verified by sha256 before it is published). A transfer that does not land, or lands corrupted, is an error and not a reported success.\n\nrespect_ignorefiles (default true) applies ONLY when `path` is a folder. It honors `.gitignore` and `.ignore` at the folder root and in every subdirectory (each scoped to its own subtree, so a nested ignore file overrides its parent), plus `.dockerignore` at the root only. All three use gitignore semantics: negation (`!`), directory-only patterns (`foo/`), anchoring, `*`/`?`/`[...]`, `**`, comments, last-matching-rule-wins. As in git, an excluded directory is pruned and `!` cannot re-include anything beneath it. `.git/` is always skipped and symlinks are never followed. For a single named file it has NO effect.";

const LIST_TOOLS_INSTRUCTIONS: &str = "Lists the tools of an MCP service INSIDE the Space. `service` is any service the Space declares (an MCP server the image runs, reached over streamable HTTP; no spacesd needed) or `driver`, the spacesd's own cua-driver registry (screenshot, click, type, launch apps, accessibility, ...). The default is `driver`, or the only declared service of a Space without spacesd. With no `name`, returns a scannable overview; pass name=<substring> for full input schemas. Then invoke them with call_tool. The reply names every service the Space exposes.";

const AGENT_START_INSTRUCTIONS: &str = "Installs the harness if needed (pinned, checksum-verified), gives it the Space's own MCP tools plus any mcp_servers, and runs it over the Agent Client Protocol as a detached spacesd process with auto-approved permissions (the Space is the sandbox). The run survives this server restarting or the connection dropping. Returns a run_id immediately: follow it with agent_events (cursor) or agent_status, continue it with agent_message, cancel a turn with agent_interrupt. Keys: provider keys saved in Cua Spaces (Settings → Connectors) reach the runs whose harness reads them; env_from_host names provider key variables from this server's environment; host credential files are never copied.";

const AGENT_EVENTS_INSTRUCTIONS: &str = "Normalized events (install, session, turn_started, message, thought, tool_call, tool_update, plan, usage, permission, turn_ended, error, exited, ...) after `cursor`. Pass the returned cursor back to continue; caught_up=true means nothing more is written yet. The same kinds for every harness.";

/// What an agent that joined a Space does next, in plain words. One text for
/// the redeem result, the tool descriptions, the pasted line and the link's
/// page, so every way in teaches the same steps.
macro_rules! join_next_steps {
    () => {
        "Next steps, in this order: 1. Set your display name and icon now with thread_join. 2. Keep watching the thread with thread_inbox (wait_ms 25000) in a loop, so you receive what people in the Space send you; answer each task with thread_post and reply_to its seq. 3. Reply with thread_post as yourself (pass the member credential from thread_join_redeem as `_member` on each call, so it posts as the agent and not as the person who signed in) and never @mention yourself, because a message that @mentions you gives you a task. 4. If you cannot loop (a CLI session that only runs when someone prompts it), say so in your first post and stay reachable the supported way: a scheduled poll that runs thread_inbox, or a wake hook."
    };
}

/// [`join_next_steps!`] as a constant.
pub const JOIN_NEXT_STEPS: &str = join_next_steps!();

const THREAD_READ_INSTRUCTIONS: &str = "Every Space has one thread: what people and its agent members said, the tasks @mentions created and how they went. Without `after` it returns the newest entries (`before` pages back, `has_more_before` says whether there are older ones); pass the returned `cursor` as `after` to read on; `follow_ms` waits for the next entry (a check-in from any device). `mark_read` moves your read cursor, which `unread` counts from. Entries are never edited: a task's state is the last `task_update` entry for it, folded into `tasks`.";

const THREAD_POST_INSTRUCTIONS: &str = "Posts as you (the daemon stamps the author; it is never an argument). `@name` of an agent member of this Space makes the message a task for it: the daemon wakes the agent (restoring its home if it was asleep), delivers the message as its next turn, and posts its answer as a reply when the turn ends. An agent with several sessions gives it to its lowest free session (`to_session` picks one); a busy agent gets it after its current turn; a paused one keeps it queued. Text and attachment names that look like a secret are refused: secrets belong in the Keyvault. `view` says what the person is looking at in the Space as they post (its desktop, or a window by app, title and window_id); it is stored with the entry, so an agent reads which window \"this\" means.";

const AGENT_ADD_INSTRUCTIONS: &str = "A persistent agent is an agent member of the Space it works in, with its memory in its home in the Cua Volume (agents/<name>/), restored before each run and saved after each turn. People @mention it in the Space's thread (thread_post) and it answers there; it sleeps until a message or a schedule wakes it. Its harness keeps its memory in the home (Claude Code auto memory, Codex memories, Hermes MEMORY.md, the OpenClaw workspace). With every_minutes, daily_at or weekly_on and a prompt it also gets a schedule. With sessions (1-8) it works on up to that many tasks at once, each in its own run; session 1 alone writes its home, the others start from the home as last saved. durable, on other than local and once are not available yet and are refused. Inside the Space the agent gets the `cua` bridge: the thread, notify_user, the drive, and the user's computers it was granted.";

const THREAD_JOIN_INSTRUCTIONS: &str = concat!(
    "For an agent member: sets the name people see (1-32 characters; not a person's name in the Space, another member's handle or \"Cua\"; names may repeat, and the app then shows your owner after it), your icon (`harness` or an https:// image) and the harness and version you run (shown as you reported them). Your owner, the computer you post from and how you joined come from your membership and can't be set. An agent connected with `cua join` gets its @handle from its first name (`codex`, `codex-2`). A refused name is noted in the thread. ",
    join_next_steps!()
);

const JOIN_CODE_INSTRUCTIONS: &str = "Connect your own agent: returns one line with a link (https://cua.ai/join/<code>) to paste into an agent on a computer you are signed in on. The link serves a static doc for agents that tells them to run `cua join <code>`. The code works once, only for this Space, expires (1 hour by default, 15 minutes to 1 day) and is stored only as a hash; redeeming it needs the redeemer's Cua sign-in. `can_task` says who can give the agent tasks; `ask_before_join` makes the owner or an admin let it in from the thread first.";

const JOIN_REDEEM_INSTRUCTIONS: &str = concat!(
    "What `cua join <code>` calls: redeems a join code made on this machine for the signed-in account, and makes the agent a member of the code's Space, bound to `device`. The member key in the result goes to the keychain (an agent that calls this tool itself holds it for this session only: never show or store it elsewhere); each later call presents it (signed by the device key when there is one, or as `_member` through the MCP tools) and the daemon stamps the agent's posts from it. Codes for another account's Space, or for a Space on another machine, can't be redeemed yet. Wrong codes are limited per account and per machine. The result repeats the next steps and carries `member_credential`. ",
    join_next_steps!()
);

const AGENT_SET_RIGHTS_INSTRUCTIONS: &str = "Each agent member has three rights: `task` (whose @mention or agent_send gives it a task; default `members`), `control` (interrupt, stop, pause, resume or take control; default `admins`, and whoever asked for its current task can stop it) and `approve` (answer its approval requests; default `admins`). Each is `owner`, `admins`, `members` or a list of account ids. The owner always holds every right; guests (people a Space is shared with to watch) only read the thread. Only the owner and admins change them; the thread gets a note saying who did.";

const POWER_INSTRUCTIONS: &str = "Turns a Space off the way its provider can. By default it stops: a local VM (QEMU or Lume) shuts down with its disk kept and gives its memory back to the machine, a local container stops or is suspended as its runtime can, a Space one of your computers provides is stopped by that computer, and a Space in your own cloud (on=\"aws\", \"gcp\") is stopped there: compute billing stops, its disk stays (storage is still billed); a Hetzner server (on=\"hetzner\") is stopped too, but Hetzner keeps billing a stopped server, so delete it to stop the charge. suspend=true suspends a local container or QEMU VM instead: its memory is kept (so it still uses it) and it resumes where it was. Anything streaming from the Space and any hotspot on it end first. A Modal sandbox, a Space added by address and your own computers on the relay cannot be turned off (wrong_provider); delete_space deletes a Space instead. start_space turns it on again.";

const PAUSE_INSTRUCTIONS: &str = "One call: stops the run, saves the home, releases the home's lease, keeps the agent's routines from firing, and suspends its Space when it is local. A cloud Space cannot be suspended, so pausing releases it after the home is saved; agent_resume creates it again from the same image and restores the home. Apps open in a released cloud Space do not come back. The user's own computers are never suspended.";

const SITE_LOGIN_INSTRUCTIONS: &str = "Signs in to a website in the Space's browser with a password the user saved in the Cua Keyvault (imported from their browser). The first call files a Keyvault request and returns a request_id; the user approves it in Cua (Touch ID or the login password), once per sign-in by default. Call again with the same space, url and request_id: the Keyvault checks the page is on the saved login's exact origin, types the username and password through the Space's cua-driver, submits the form and returns what it did, never the password. Pass the browser tab you opened (session, target_id, tab_id from get_browser_state on the same Space) to sign in there; without one the Keyvault opens its own browser in the Space on `url` and returns its target_id and tab_id. `agent` names the persistent agent asking, shown to the user.";

const DRIVE_INSTRUCTIONS: &str = "Cua Volume is one versioned drive per account, shared by every Space and agent: public/ (shared reference, read by everyone), agents/<agent>/ (a persistent agent's home: memory, outputs, inbox) and spaces/<space>/ (per-Space scratch). As the user (the default) you see everything. With as_agent (and in_space) you see exactly what that agent sees: it reads public/, writes its own home and its Space's folder, and anything wider needs a grant the user approves.";

const SHARES_RESULT: &str = "JSON: `space`, `machine` (the relay computer it is shared as; empty when not shared), `invitee_space` (what the people it is shared with open: `relay:<computer>`), `url`, `online` and `shares` (each `who`, `role`: viewer or editor, and `connected`).";

const SHARE_INSTRUCTIONS: &str = "Shares the Space through the cua.ai relay with one person in your own team, by verified email or account id, or with the whole team as `team:<id>` (as a viewer or an editor). Sharing is for teams: an account in no team gets `sharing_needs_team` (\"Sharing is for teams. Start a team to share this Space.\"), and someone outside your team gets `share_outside_team`. The same allowlist and device enrollment `cua host share` uses. A viewer joins presence with their own named cursor and watches a view-only stream: the Space's driver refuses their input, shell, files and MCP. An editor gets full use, and their input is a human session. Sharing again changes the role. A host (`relay:<id>`) is shared directly; any other Space (local, cloud, or added by address) is attached to the relay on its first share: its own driver dials out, so it needs no inbound port. Invitees open `invitee_space`.";

/// The Spaces tools, in manifest order.
pub fn tools() -> Vec<Tool> {
    use inputs as i;
    vec![
        // --- Spaces lifecycle -------------------------------------------------
        Spec {
            category: Category::Lifecycle,
            result: "JSON: the registered Space (`id`, `name`, `provider`, `spacesd_version`, `features`, `os`, `services`, `added_at`) and `phase: \"ready\"`.",
            errors: &["unauthenticated"],
            instructions: "Tries a GetCapabilities handshake with the Space's cua-spacesd first. A local:<name> or cloud:<name> id without one is still a Space, with an empty capability set (its declared services, tunnels and public URLs work); a direct URL without one is added as an MCP Space when it answers MCP initialize (service `mcp`), and refused with spacesd_not_available when it answers neither. The Space is stored in ~/.cua/spaces.json and its token in ~/.cua/spaces-credentials.json (0600). Re-adding the same address updates it.",
            capabilities: &["space.register"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "SpacesConnection.addSpace(url:token:)",
            rust_symbol: "Spaces::add",
            ..Spec::new("add_space", "Add an existing computer as a Space by address and token.")
        }
        .with::<i::AddSpace>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: `{\"removed\": <Space>}`, the registration that was removed.",
            instructions: "Only the registration and its stored token are removed. The sandbox keeps running, including a Space in your cloud (its cloud resources and relay computer stay); delete_space deletes a Space that create_space made.",
            capabilities: &["space.register"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "SpacesConnection.removeSpace(_:)",
            rust_symbol: "Spaces::remove",
            ..Spec::new("remove_space", "Forget a registered Space without touching it.")
        }
        .with::<i::RemoveSpace>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: an array of Spaces (`id`, `name`, `provider`, `spacesd_version`, `features`, `os`, `services`, `added_at`, for a Space one of your computers provides `host` and `host_name`, for a Space that can be turned off and on `power` (`suspend` or `stop`) and `power_state`, and for a Space in your cloud `cloud`, `cloud_place` and `cloud_delete`, and for a relay computer the relay cannot reach now `status` (`offline` or `not sharing`)), each with `phase`: `ready`, or `offline` for a relay computer the relay cannot reach now (a computer that is off or stopped sharing, or a Space that is gone and left its record; `relay_unregister_space` removes it).",
            errors: &["relay", "unauthenticated"],
            instructions: "Every registered Space (cloud, local, direct and relay) with its id (local:<name>, cloud:<name>, direct:<host:port>, relay:<computer-id>), provider, spacesd version (empty without one) and supported features as of the last handshake. The signed-in account's computers on the relay are listed too: those are the names `create_space` accepts as on=\"host:<computer>\". A Space a computer provides carries `host` (that computer's id) and `host_name`; group them under it when you show them. A Space in your cloud carries `cloud` (the provider: aws, gcp, modal), `cloud_place` (account and region, to show with it) and `cloud_delete` (where delete_space can delete it: here, host:<computer>, or elsewhere). A durable Space (create_space durable=true) carries `durable` (its name, by which it is found wherever it runs now).",
            capabilities: &["space.discover"],
            sdk_symbol: "SpacesConnection.spaces()",
            rust_symbol: "Spaces::list",
            ..Spec::new("list_spaces", "List the Spaces this connection can see.")
        }
        .with::<i::ListSpaces>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: the registered Space, `phase: \"ready\"` and `reused` (true when `reuse` returned an existing Space); with `wait: false`, `{\"id\", \"phase\": \"starting\"}` at once; with `count` above 1, an array of them.",
            errors: &[
                "invalid_placement",
                "sandbox",
                "timeout",
                "ambiguous_host",
                "limit_exceeded",
            ],
            instructions: "Creates a new sandbox where `on` says (local: this computer, free; aws, azure, gcp, hetzner or modal: the user's own cloud account, connected with cloud_connect, billed there; default: the user's default location, else local; with no location and none configured, when a linked cloud has a budget set with cloud_budget, this computer first and the linked clouds when it is full, never past the budget, unless placement.overflow is off; a Space that landed in the user's cloud carries `cloud` and `cloud_place`, the pool host and its price, so say so), waits until it is ready (its spacesd when the image runs one, else its declared services), and registers it as local:<name>. Cua Cloud (on=\"cloud\") has ended and fails with an error that says so. `kind` (auto, container, vm, microvm: the container image with its own kernel, never picked by auto) and `runtime` (auto, or an engine the location offers for the kind) are chosen automatically by default: the canonical Linux image runs as a gVisor container, macOS images as Lume VMs, Windows and disk-only images as VMs (QEMU locally, KubeVirt in the cloud). An impossible combination fails with invalid_placement and lists the valid values. Any image works; without cua-spacesd the capability set is empty. reuse=true returns a reachable registered Space in the same location with the requested services instead of creating one. On the user's own computers: on=\"auto\" creates it on the least-loaded one that can take it (this computer or one signed in to the user's account that provides Spaces; the host refuses when full and the next is asked); on=\"host:<computer>\" creates the Space on a computer set up with `cua host setup --provide-spaces` (for example \"spin up 2 macOS Spaces on my spare Mac mini\" is on=\"host:spare mac mini\", image=\"macos\", count=2). The computer is matched by id, exact name, or the words of its name; when several fit equally the call fails with ambiguous_host listing them (ask the user which, then pass on=\"host:<id>\"). The host runs it with its own runtimes (Lume for macOS VMs on a Mac, Docker for Linux), the Space joins the relay and is returned as relay:<computer> with `host` set. A Mac runs at most two macOS VMs (Apple's macOS license, enforced by Apple Virtualization); a host at a limit fails with limit_exceeded. Only the computer's owner and the accounts it is shared with as editors can create there, from an enrolled device. on=\"direct:<addr>\" is not accepted here: a direct address is one already-running Space, attached with add_space, not a host this call can create on. durable=true (with on=\"auto\") keeps the Space on the user's computers: when its computer goes away it is created again on another one with the same name (its disk starts fresh), list_spaces shows it with `durable`, and delete_space deletes it for good.",
            providers: &CREATE_PROVIDERS,
            metering: Metering::Cloud,
            capabilities: &["space.create"],
            host_requires: &[],
            annotations: Annotations::mutating(),
            sdk_symbol: "Spaces.create(options)",
            rust_symbol: "Spaces::create",
            notes: &["Local needs a container engine, QEMU or Lume (`cua runtime doctor`)."],
            ..Spec::new(
                "create_space",
                "Create a Space: a new sandbox on this computer, on one of your computers, or in the cloud.",
            )
        }
        .with::<i::CreateSpace>(),
        Spec {
            category: Category::Lifecycle,
            result: "Text: what was deleted (`Deleted <id> (...)`), or `Removed <id> (...)` for a Space added by address.",
            errors: &["sandbox"],
            instructions: "Deletes the sandbox of a Space that create_space made (cloud: metering stops; local: the instance is deleted; on one of your computers, on=\"host:...\": that computer deletes it and it leaves the relay) and forgets it. A Space in your cloud (list_spaces: cloud, cloud_place, cloud_delete) is deleted permanently through the records of the device that created it: cloud_delete \"here\" deletes it from this device; \"host:<computer>\" asks that host (it must be online, and only its owner may); \"elsewhere\" is refused with where it was created: delete it there, or remove_space. Another Space added by address (direct:, relay:) is only forgotten: cua did not create it. Any hotspot on the Space is stopped first. To forget a Space without deleting anything, use remove_space.",
            capabilities: &["space.delete"],
            annotations: Annotations::mutating(),
            sdk_symbol: "Spaces.delete(space)",
            rust_symbol: "Spaces::delete",
            ..Spec::new("delete_space", "Delete a Space's sandbox and forget it.")
        }
        .with::<i::SpaceOnly>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: `space`, `state` (`suspended` or `stopped`), `power` (`suspend` or `stop`) and `message`.",
            errors: &["wrong_provider", "sandbox", "relay", "permission_denied", "cloud"],
            instructions: POWER_INSTRUCTIONS,
            capabilities: &["space.power"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "Spaces.stop(space)",
            rust_symbol: "Spaces::stop",
            notes: &["list_spaces says which Spaces turn off (`power`: `suspend` or `stop`; absent when it cannot) and how they were left (`power_state`)."],
            ..Spec::new("stop_space", "Turn a Space off: stop it (disk kept, memory freed) or, with suspend, suspend it (memory kept).")
        }
        .with::<i::StopSpace>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: `space`, `cpus`, `memory_mb`, `disk_gb` (what it has now) and `message`.",
            errors: &["wrong_provider", "sandbox", "invalid_argument", "timeout"],
            instructions: "Gives a local VM Space (QEMU) another shape: `cpus` vCPUs, `memory_mb` MiB of memory, a disk grown to `disk_gb` GiB; what you leave out stays. The VM shuts down and boots again from its own disk, so its files stay but its memory and running processes do not; a stopped one boots. A disk never shrinks. Returns once it is ready again. Containers, Lume VMs and Spaces on other computers are refused (wrong_provider).",
            capabilities: &["space.power"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "Spaces.resize(space, cpus, memory_mb, disk_gb)",
            rust_symbol: "Spaces::resize",
            ..Spec::new("resize_space", "Change a Space's vCPUs, memory or disk size.")
        }
        .with::<i::ResizeSpace>(),
        Spec {
            category: Category::Lifecycle,
            result: "JSON: `space`, `state` (`running`), `power` (`suspend` or `stop`) and `message`.",
            errors: &["wrong_provider", "sandbox", "relay", "permission_denied", "cloud", "timeout"],
            instructions: "Turns a Space stop_space turned off on again: resumes a suspended one, boots a stopped one (a Space one of your computers provides, or one in your cloud, joins the relay again), and leaves a running one as it is. Returns once it answers again (bounded). The same Spaces turn on as turn off.",
            capabilities: &["space.power"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "Spaces.start(space)",
            rust_symbol: "Spaces::start",
            ..Spec::new("start_space", "Turn a Space back on: resume or boot it.")
        }
        .with::<i::SpaceOnly>(),
        // --- In-Space execution ----------------------------------------------
        Spec {
            category: Category::CommandsAndFiles,
            result: "Text: stdout, then `[stderr]` and stderr, then `[exit N]`.",
            errors: &["timeout"],
            instructions: "Runs through the spacesd's ProcessService with `/bin/sh -c`. Output is stdout, then `[stderr]` and stderr, then `[exit N]`; each stream keeps its last 16000 characters when longer.",
            capabilities: &["space.exec", "space.exec.shell"],
            annotations: Annotations::mutating(),
            sdk_symbol: "Space.bash(_:)",
            rust_symbol: "Space::bash",
            ..Spec::new("space_bash", "Run a shell command inside the Space.")
        }
        .with::<i::SpaceBash>(),
        Spec {
            category: Category::CommandsAndFiles,
            result: "Text: `wrote <path> (<n> bytes, sha256 <hex> verified in Space)`.",
            errors: &["transfer_failed"],
            instructions: "Written through FilesystemService, verified by sha256; no shell is involved, so nothing needs quoting.",
            capabilities: &["space.files.write"],
            annotations: Annotations::mutating(),
            sdk_symbol: "Space.write(_:to:)",
            rust_symbol: "Space::write",
            notes: &["Literal text: no local file is read and nothing is shell-quoted, which is the whole reason it exists next to space_bash."],
            ..Spec::new("space_write", "Write literal text to a path inside the Space.")
        }
        .with::<i::SpaceWrite>(),
        Spec {
            category: Category::CommandsAndFiles,
            result: "JSON: `source`, `dest`, `kind` (`file` or `folder`), `files`, `directories`, `bytes`, `sha256` (single file) and `verified`.",
            errors: &["transfer_failed", "io"],
            instructions: "Files stream through FilesystemService in chunks with a sha256 check; a folder is copied file by file (empty directories included, symlinks skipped). No size cap.",
            capabilities: &["space.files.write", "space.files.transfer"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.files.send(_:to:within:)",
            rust_symbol: "Space::upload",
            notes: &["Without `dest` nothing in the Space is replaced: a taken `<home>/<name>` lands as `<name> (1).ext`. An explicit `dest` is written as given and replaces a file already there."],
            ..Spec::new("upload", "Send a local file or folder into the Space.")
        }
        .with::<i::Upload>(),
        Spec {
            category: Category::CommandsAndFiles,
            result: "JSON: `source`, `destination`, `dest`, `kind`, `files` (each `path`, `size`, `sha256`), `bytes`, `respect_ignorefiles`, `skipped_by_ignorefiles`, `skipped_count`, `skipped_by_guest` and `verified`.",
            errors: &["transfer_failed", "io"],
            instructions: SEND_FILE_INSTRUCTIONS,
            capabilities: &["space.files.write", "space.files.transfer"],
            annotations: Annotations::mutating(),
            sdk_symbol: "Space.sendFile(_:into:respectingIgnoreFiles:)",
            rust_symbol: "Space::send_file",
            notes: &[
                "Unlike `upload`, the copy is verified in the Space by sha256 before the call returns: a transfer that does not land, or lands corrupted, is an error and not a reported success.",
                "`respect_ignorefiles` applies only when the path is a folder, with gitignore semantics; a file named explicitly is always sent.",
                "A file of the same name already in the Space is kept: the new copy lands as `name (1).ext` and its path is in `files`.",
            ],
            ..Spec::new("send_file", "Send a host file or folder into the Space's Downloads, verified by sha256 on arrival.")
        }
        .with::<i::SendFile>(),
        Spec {
            category: Category::CommandsAndFiles,
            result: "JSON: `source`, `dest`, `kind`, `files`, `bytes`, `sha256` (single file) and `verified`.",
            errors: &["transfer_failed", "io"],
            instructions: "Files stream through FilesystemService and are verified by sha256; a folder is copied recursively.",
            capabilities: &["space.files.read", "space.files.transfer"],
            sdk_symbol: "Space.download(_:into:)",
            rust_symbol: "Space::download",
            notes: &[
                "Nothing on the host is overwritten: when `dest` already has an entry of that name the copy lands as `name (1).ext` (or `name (1)` for a folder), and `dest` in the result is the path used.",
            ],
            ..Spec::new("download", "Fetch a file or folder out of the Space.")
        }
        .with::<i::Download>(),
        // --- Frames and windows ------------------------------------------------
        Spec {
            category: Category::StreamsAndDisplay,
            result: "JSON: `space`, `media_session_id`, `ws_url` (ticket included), `ticket`, `ticket_expires_at`, `codec` (`h264`, `bgra` or `png`), `wire_version`, `frame_size` and `needs_gateway_headers`.",
            errors: &["stream"],
            instructions: "Opens a media session (StreamService.OpenMedia) for the desktop or one window and returns its ticketed WebSocket URL (rcdp wire v2, see libs/cua/proto/MEDIA.md). The ticket is short-lived, bound to that one session, and safe to put in a URL; it is not the Space's token. Works for every provider because the token comes from the SDK, not from the guest.",
            capabilities: &["space.stream.endpoint"],
            requires: &["desktop_stream"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "Space.streamEndpoint(forceRefresh:)",
            rust_symbol: "Streams::open",
            notes: &["Giving the caller frames is a different act from presenting the Space to an operator. FRICTION.md 10.", "Replaces local_rcdp, which only worked for Local macOS Spaces and read its token over ssh."],
            ..Spec::new("stream_endpoint", "Return a ticketed media endpoint for the Space's desktop or one window.")
        }
        .with::<i::StreamEndpoint>(),
        Spec {
            category: Category::StreamsAndDisplay,
            result: "JSON: an array of windows (`window_id`, `epoch`, `title`, `app_name`, `app_id`, `pid`, `bounds`, `on_screen`, `focused`, `available`, `limitation`).",
            instructions: "The windows a viewer can stream now, from StreamService.ListTargets: on screen, sized, normal app windows (no hidden, minimized or system-UI windows), each with its handle, title, app and bounds.",
            capabilities: &["space.windows.list"],
            requires: &["windows"],
            sdk_symbol: "Space.windows()",
            rust_symbol: "Streams::windows",
            ..Spec::new("list_space_windows", "List the windows open inside the Space.")
        }
        .with::<i::ListSpaceWindows>(),
        Spec {
            category: Category::StreamsAndDisplay,
            result: "Text: `Streaming <window> of <space> to your desktop (bidirectional).`",
            errors: &["stream"],
            instructions: "FOR THE HUMAN. Opens a bidirectional window on the operator's desktop showing one Space window (the Cua Spaces app draws it from a media ticket). Pass app_name to auto-pick that app's largest visible window, or an exact window_id.",
            platforms: &OPERATOR_PLATFORMS,
            capabilities: &["operator.display", "operator.display.window"],
            requires: &["window_stream"],
            host_requires: &[host::OPERATOR_DISPLAY],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.operatorDisplay.streamWindowToOperatorDesktop(_:)",
            rust_symbol: "OperatorDisplay::stream_window",
            notes: &["Operator-side: the pixels land on the human's computer, so the platform axis is the operator's OS and not the Space's."],
            ..Spec::new("stream_space_window", "Draw one Space window on the operator's desktop.")
        }
        .with::<i::StreamSpaceWindow>(),
        Spec {
            category: Category::StreamsAndDisplay,
            result: "Text: `Pinned <space> as picture-in-picture on your desktop.`",
            instructions: "FOR THE HUMAN, NOT FOR YOU. Pins an always-on-top picture-in-picture overlay of the Space onto the user's own desktop. Call it only when the user asks to see a Space; never to observe your own work.",
            platforms: &OPERATOR_PLATFORMS,
            capabilities: &["operator.display", "operator.display.pip"],
            requires: &["desktop_stream"],
            host_requires: &[host::OPERATOR_DISPLAY],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.operatorDisplay.pinPictureInPictureOnOperatorDesktop()",
            rust_symbol: "OperatorDisplay::pin_pip",
            ..Spec::new("show_space_pip", "Pin the Space as picture-in-picture on the operator's desktop.")
        }
        .with::<i::SpaceOnly>(),
        Spec {
            category: Category::StreamsAndDisplay,
            result: "Text: `Unpinned <space> from your desktop.`",
            platforms: &OPERATOR_PLATFORMS,
            capabilities: &["operator.display", "operator.display.pip"],
            host_requires: &[host::OPERATOR_DISPLAY],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.operatorDisplay.unpinPictureInPictureFromOperatorDesktop()",
            rust_symbol: "OperatorDisplay::unpin_pip",
            ..Spec::new("hide_space_pip", "Unpin the picture-in-picture window.")
        }
        .with::<i::SpaceOnly>(),
        Spec {
            category: Category::StreamsAndDisplay,
            result: "Text: `Opened the <space> viewer on your desktop.`",
            instructions: "FOR THE HUMAN, NOT FOR YOU. Opens a live viewer window on the user's own desktop. Never call it to obtain a screenshot or to check on your own work; use the screenshot tool for that.",
            platforms: &OPERATOR_PLATFORMS,
            capabilities: &["operator.display", "operator.display.viewer"],
            requires: &["desktop_stream"],
            host_requires: &[host::OPERATOR_DISPLAY],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.operatorDisplay.openViewerOnOperatorDesktop()",
            rust_symbol: "OperatorDisplay::open_viewer",
            ..Spec::new("open_space_viewer", "Open the full Space viewer on the operator's desktop.")
        }
        .with::<i::SpaceOnly>(),
        // --- Services ----------------------------------------------------------
        Spec {
            category: Category::Services,
            result: "JSON: `service`, `count`, `tools` (`name` and `description`; with `name`, also `inputSchema`), `services`, `contract_version`, and a `hint` when no filter was given.",
            errors: &["mcp"],
            instructions: LIST_TOOLS_INSTRUCTIONS,
            capabilities: &["space.services.discover"],
            sdk_symbol: "space.services.list()",
            rust_symbol: "Services::list_tools",
            ..Spec::new("list_tools", "List the MCP services and tools reachable inside the Space.")
        }
        .with::<i::ListTools>(),
        Spec {
            category: Category::Services,
            result: "The called tool's own result, unchanged: its content parts (images included), `structuredContent` and `isError`.",
            errors: &["mcp"],
            instructions: "Returns the tool's content parts unchanged, images included. A tool-level failure comes back with isError, not as a transport error.",
            capabilities: &["space.services.call"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.services.call(_:on:_:)",
            rust_symbol: "Services::call_tool",
            notes: &["Open world twice over: the callee is a tool the SDK has never seen."],
            ..Spec::new("call_tool", "Call a service tool inside the Space.")
        }
        .with::<i::CallTool>(),
        // --- Agents ------------------------------------------------------------
        Spec {
            category: Category::Agents,
            result: "JSON: `run_id`, `agent`, `space`, `process_tag`, `cwd`, `notes` and `hint`.",
            errors: &["agent"],
            instructions: AGENT_START_INSTRUCTIONS,
            capabilities: &["agent.run.start"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.agents.start(...)",
            rust_symbol: "Agents::start",
            notes: &["Runs auto-approved: the Space is the sandbox, which is why approvalsAreEnforced is false."],
            ..Spec::new("agent_start", "Start a coding agent in the Space.")
        }
        .with::<i::AgentStart>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `delivered`, `run_id`, `status`, `phase` and `reason`.",
            errors: &["agent"],
            instructions: "Continues the same session. While a turn runs the message is queued (or, with force=true, the turn is interrupted first); a run whose runner exited is restarted and resumes its session.",
            capabilities: &["agent.run.message"],
            annotations: Annotations::mutating(),
            sdk_symbol: "AgentRun.send(_:)",
            rust_symbol: "Agents::send",
            ..Spec::new("agent_message", "Send a follow-up to an agent run.")
        }
        .with::<i::AgentMessage>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `run_id`, `status`, `phase`, `events` (each `seq`, `ts_ms`, `turn`, `kind`, and `text`, `tool_id`, `tool_title`, `tool_kind`, `tool_status`, `stop_reason` when present; `category`: `message`, `user`, `activity` or `hidden`, and a one-line `summary` for activity; `raw` on request), `cursor` and `caught_up`.",
            errors: &["agent"],
            instructions: AGENT_EVENTS_INSTRUCTIONS,
            capabilities: &["agent.run.read", "agent.run.output"],
            sdk_symbol: "AgentRun.events(cursor:)",
            rust_symbol: "Agents::events",
            ..Spec::new("agent_events", "Read an agent run's events after a cursor.")
        }
        .with::<i::AgentEvents>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `run_id`, `agent`, `status`, `phase`, `reason`, `turn`, `alive`, `accepts_message`, `summary`, `created_at`, `result` (`text`, `stop_reason`, `usage`, `error`, `tool_calls`), `output_tail` and `space`.",
            errors: &["agent"],
            instructions: "status is running, idle, failed, crashed or unknown ('unknown' is never a guess). Includes the last turn's result (final text, stop reason, usage) and a rendered output tail. Read from files in the Space; never disturbs the run.",
            capabilities: &["agent.run.read", "agent.run.output"],
            sdk_symbol: "AgentRun.status()",
            rust_symbol: "Agents::status",
            ..Spec::new("agent_status", "Read an agent run's status and result.")
        }
        .with::<i::AgentStatus>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `run_id`, `interrupted` and `status`.",
            errors: &["agent"],
            instructions: "Cancels the turn in flight (ACP session/cancel); the session stays open for agent_message.",
            capabilities: &["agent.run.interrupt"],
            annotations: Annotations::mutating(),
            sdk_symbol: "AgentRun.interrupt()",
            rust_symbol: "Agents::interrupt",
            ..Spec::new("agent_interrupt", "Interrupt an agent run's current turn.")
        }
        .with::<i::AgentInterrupt>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `run_id`, `stopped`, `alive` and `reason`.",
            errors: &["agent"],
            instructions: "Ends the run, then verifies its process is gone; reports stopped=false rather than claiming success.",
            capabilities: &["agent.run.stop"],
            annotations: Annotations::mutating(),
            sdk_symbol: "AgentRun.stop()",
            rust_symbol: "Agents::stop",
            ..Spec::new("agent_stop", "Stop an agent run.")
        }
        .with::<i::AgentStop>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `{\"space\", \"runs\"}`, each run with `run_id`, `agent`, `status`, `phase`, `reason`, `turn`, `alive`, `accepts_message`, `summary`, `created_at` and `label`.",
            errors: &["agent"],
            instructions: "Every run in the Space, newest first, including runs another client started.",
            capabilities: &["agent.run.read"],
            sdk_symbol: "space.agents.list()",
            rust_symbol: "Agents::list",
            ..Spec::new("agent_list", "List agent runs in the Space.")
        }
        .with::<i::SpaceOnly>(),
        Spec {
            category: Category::Agents,
            result: "JSON: `protocol`, `statuses`, `event_kinds`, `harnesses` (each `id`, `name`, `ready` (supported and not `auth: missing`), `supported`, `auth` (`ok`: a key saved in Cua Spaces (Settings → Connectors) or one this server can forward with env_from_host; `missing`: none, so a run needs a sign-in inside its Space; `unknown`: the harness may be set up through its own config), `auth_hint`, `installs`, `keys`, `endpoint_wires`, `sandbox_mcp`, `interactive_cli`, `notes`), `forwardable_env` and `runner`.",
            instructions: "Every harness: readiness (including whether a credential is available, with a hint when not), what it installs, which key variables it reads, which endpoint wire formats it takes, and its known limits; plus the event kinds and statuses.",
            capabilities: &["agent.capabilities"],
            sdk_symbol: "space.agents.harnessCapabilities()",
            rust_symbol: "agents::capabilities",
            ..Spec::new("agent_capabilities", "Report what the agent harnesses support.")
        }
        .with::<i::AgentCapabilities>(),
        // --- Persistent agents ----------------------------------------------------
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: the agent record: `name`, `harness`, `space`, `model`, `base_url`, `env_from_host`, `paused`, `space_state` and `created_ms`; as a member of its Space also `sessions`, `policy` {`durable`, `on`, `once`, `guard`}, `added_by`, `member_since_ms`, `status` and, with a schedule, `schedule` (the routine).",
            errors: &["agent"],
            instructions: AGENT_ADD_INSTRUCTIONS,
            capabilities: &["agent.persistent.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.create(...)",
            rust_symbol: "Persistent::create",
            ..Spec::new("agent_add", "Create a named agent whose memory outlives its Space.")
        }
        .with::<i::PersistentAgentCreate>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `{\"agents\"}`, each with `name`, `harness`, `space`, `paused`, `space_state`, `run_id`, `saved_ms` and `last_error`.",
            instructions: "Every persistent agent of this computer's cua home, with its Space, its current run and when its home was last saved.",
            capabilities: &["agent.persistent.read"],
            sdk_symbol: "spaces.persistentAgents.list()",
            rust_symbol: "Persistent::list",
            ..Spec::new("persistent_agent_list", "List persistent agents.")
        }
        .with::<i::PersistentAgentList>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `removed` (the record) and `home_deleted`. By default the home stays in the Cua Volume.",
            errors: &["agent"],
            instructions: "Stops its run, removes its routines, closes its open thread tasks and takes it out of its Space. Its home (agents/<name>/) stays in the Cua Volume unless keep_home is false. The thread keeps what it wrote.",
            capabilities: &["agent.persistent.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.remove(_:)",
            rust_symbol: "Persistent::remove",
            ..Spec::new("agent_remove", "Forget a persistent agent.")
        }
        .with::<i::AgentRemove>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `run_id`, `started` (a new run, with the home restored) and `restored` (`files`, `bytes`, `millis`).",
            errors: &["agent", "lease_held"],
            instructions: "A follow-up to the agent's idle run, or a new run with its home restored when it has none. Refused while a turn runs (never interrupts) and while paused. Follow the run with agent_events on the agent's Space.",
            capabilities: &["agent.persistent.run"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.send(_:_:)",
            rust_symbol: "Persistent::send",
            ..Spec::new("agent_send", "Give a persistent agent a turn.")
        }
        .with::<i::PersistentAgentSend>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `files`, `bytes`, `unchanged`, `removed`, `blocked` (files the secret scanner kept out) and `millis`.",
            errors: &["agent", "secret_detected"],
            instructions: "Saves the agent's home from its Space into the Cua Volume now (the daemon also saves after every turn): only files whose SHA-256 changed move.",
            capabilities: &["agent.persistent.run"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.persistentAgents.save(_:)",
            rust_symbol: "Persistent::save",
            ..Spec::new("persistent_agent_save", "Save a persistent agent's home now.")
        }
        .with::<i::PersistentAgentName>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `stopped_run`, `saved` (the home save), `space_state` (`suspended`, `released` or `running`) and `millis`.",
            errors: &["agent", "sandbox"],
            instructions: PAUSE_INSTRUCTIONS,
            capabilities: &["agent.persistent.run"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.persistentAgents.pause(_:)",
            rust_symbol: "Persistent::pause",
            ..Spec::new("agent_pause", "Pause a persistent agent: its run, its routines and its Space.")
        }
        .with::<i::PersistentAgentName>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `space` (it can change when a cloud Space is created again), `recreated`, `restored`, `run_id` and `ready_ms` (from the call to the home being back in a ready Space).",
            errors: &["agent", "sandbox"],
            metering: Metering::Cloud,
            instructions: "Resumes a suspended local Space, or creates a released cloud Space again from the same image, restores the home and lets routines fire again. With `prompt`, starts a run on it.",
            capabilities: &["agent.persistent.run"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.resume(_:prompt:)",
            rust_symbol: "Persistent::resume",
            ..Spec::new("agent_resume", "Resume a paused persistent agent.")
        }
        .with::<i::AgentResume>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: the routine: `id`, `botID` (the agent), `title`, `prompt`, `schedule`, `isEnabled` and `createdAt`.",
            instructions: "Fired by `cua daemon` whether or not an app is open, as a turn of the agent (`[routine] <title>: <prompt>`). A routine never interrupts a turn: a busy or paused agent skips that slot. A daemon that was asleep fires a missed routine once on waking.",
            capabilities: &["agent.persistent.routines"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.addRoutine(...)",
            rust_symbol: "Persistent::routine_add",
            ..Spec::new("agent_schedule_add", "Schedule a recurring turn for a persistent agent.")
        }
        .with::<i::RoutineAdd>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `{\"routines\"}`, each with `id`, `botID`, `title`, `prompt`, `schedule`, `label`, `isEnabled`, `lastFiredAt`, `lastRunID`, `lastOutcome` and `next_fire`.",
            instructions: "Routines with their next firing and what the last one did.",
            capabilities: &["agent.persistent.routines"],
            sdk_symbol: "spaces.persistentAgents.routines(agent:)",
            rust_symbol: "Persistent::routines",
            ..Spec::new("agent_schedule_list", "List routines.")
        }
        .with::<i::RoutineList>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `removed` (the id).",
            capabilities: &["agent.persistent.routines"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.persistentAgents.removeRoutine(_:)",
            rust_symbol: "Persistent::routine_remove",
            ..Spec::new("agent_schedule_remove", "Delete a routine.")
        }
        .with::<i::RoutineId>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: the routine.",
            capabilities: &["agent.persistent.routines"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.persistentAgents.setRoutineEnabled(_:_:)",
            rust_symbol: "Persistent::routine_set_enabled",
            ..Spec::new("agent_schedule_set_enabled", "Turn a routine on or off.")
        }
        .with::<i::RoutineSetEnabled>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `notified` and `id`.",
            instructions: "Posts a notification the Cua app shows (\"Your research is ready\"). Agents inside a Space get the same tool on their `cua` bridge.",
            capabilities: &["notifications.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.notifications.post(...)",
            rust_symbol: "notify::Feed::post",
            ..Spec::new("notify_user", "Tell the user something in the Cua app.")
        }
        .with::<i::NotifyUser>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `{\"notifications\"}`, newest first, each with `id`, `at_ms`, `agent`, `kind` (`turn_ended`, `message`, `approval`, `error`), `title`, `body`, `run_id`, `space` and `read`.",
            instructions: "The feed the Cua app turns into system notifications: turn ends of persistent agents, notify_user calls, access requests and failures. It persists while no app is open.",
            capabilities: &["notifications.read"],
            sdk_symbol: "spaces.notifications.list(...)",
            rust_symbol: "notify::Feed::list",
            ..Spec::new("notifications_list", "Read the notifications feed.")
        }
        .with::<i::NotificationsList>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `marked` (how many changed).",
            capabilities: &["notifications.read"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.notifications.markRead(_:)",
            rust_symbol: "notify::Feed::ack",
            ..Spec::new("notifications_ack", "Mark notifications read.")
        }
        .with::<i::NotificationsAck>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: the grant: `id`, `agent`, `machine`, `created_ms`, `expires_ms`.",
            errors: &["not_confirmed"],
            instructions: "Lets one persistent agent use one of the user's computers (its computer-use tools, through the agent's bridge). Asks the user for presence (Touch ID or passphrase) first. Other agents of the account get nothing.",
            capabilities: &["computer.access.grant"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.persistentAgents.allowComputer(_:machine:)",
            rust_symbol: "access::Access::grant",
            ..Spec::new("computer_access_grant", "Let a persistent agent use one of the user's computers.")
        }
        .with::<i::ComputerAccessGrant>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `revoked` (how many grants).",
            instructions: "Takes the access back at once: the agent's next call is refused. No presence needed.",
            capabilities: &["computer.access.grant"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.persistentAgents.revokeComputer(_:machine:)",
            rust_symbol: "access::Access::revoke",
            ..Spec::new("computer_access_revoke", "Take back a persistent agent's access to a computer.")
        }
        .with::<i::ComputerAccessRevoke>(),
        Spec {
            category: Category::PersistentAgents,
            result: "JSON: `{\"grants\"}` (each `id`, `agent`, `machine`, `created_ms`, `expires_ms`, `revoked`) and, with `audit`, `audit` (newest first: `ts_ms`, `principal`, `action`, `path` (the computer), `detail`) plus `audit_verified`.",
            instructions: "Which agents may use which of the user's computers, and the audited record of grants, uses and refusals.",
            capabilities: &["computer.access.read"],
            sdk_symbol: "spaces.persistentAgents.computerAccess(agent:)",
            rust_symbol: "access::Access::grants",
            ..Spec::new("computer_access_list", "List per-agent computer access.")
        }
        .with::<i::ComputerAccessList>(),
        // --- Space thread and members ------------------------------------------
        Spec {
            category: Category::Thread,
            result: "JSON: `space`, `me` (your author object), `role` (yours here: `owner`, `admin`, `member`, `guest` (reads only) or `agent`), `entries` (oldest first; each `v`, `seq`, `id`, `ts_ms`, `author` {`kind`, `id`, `name`}, `kind`, `text`, `mentions`, `reply_to`, `task`, `attachments`, `approval`, `answer`, `update`, and when set `imported`, `import`, `task_seq` (the task a system note is about), `view` (what the person was looking at as they posted: {`kind` `desktop` or `window`, `app`, `title`, `window_id`})), `cursor` (pass it as `after`), `caught_up`, `has_more_before`, `read_seq` (your read position), `unread` (entries after it, not yours) and `tasks` (open tasks: `seq`, `to`, `state`, `session`, `lease_until_ms`, `author`, `ts_ms`, `text`, `view`). `live`: {`key`, `agents`: [{`agent`, `stage`, `since_ms`, `text`}]}, what each agent member with a task on its way is doing now (`starting`, `installing`, `thinking`, `replying`, `using <tool>`; `since_ms`: when its oldest open task was given; `text`: what it has said so far this turn, the whole text each time and only while it streams, replaced by its reply when the turn ends); pass `key` back as `live_after` with `follow_ms` to be woken when it changes. With `with`: `with` and only your DM's entries, its own `read_seq` and `unread`, and no `tasks`.",
            instructions: THREAD_READ_INSTRUCTIONS,
            capabilities: &["thread.read"],
            sdk_symbol: "space.thread().read(after:limit:)",
            rust_symbol: "thread::Thread::read",
            ..Spec::new("thread_read", "Read a Space's thread.")
        }
        .with::<i::ThreadRead>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `entry` (the new entry, as in `thread_read`).",
            errors: &["forbidden"],
            instructions: THREAD_POST_INSTRUCTIONS,
            capabilities: &["thread.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.thread().post(_:attachments:)",
            rust_symbol: "thread::Thread::post",
            ..Spec::new("thread_post", "Post to a Space's thread.")
        }
        .with::<i::ThreadPost>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `tasks`, oldest first (each `seq`, `to`, `state`, `text`, `author`, `ts_ms`, `session`, and `view` when the person said what they were looking at: {`kind`, `app`, `title`, `window_id`}).",
            instructions: concat!("Open tasks (queued, claimed or working) addressed to an agent member: the messages that @mentioned it. An agent's own call lists its own. With `wait_ms` (at most 25000) an empty inbox waits for a task before answering, so an agent on its own machine can loop on it: call it again each time it returns, for as long as its user wants it in the Space. ", join_next_steps!()),
            capabilities: &["thread.read"],
            sdk_symbol: "space.thread().inbox()",
            rust_symbol: "thread::Thread::inbox",
            ..Spec::new("thread_inbox", "List an agent member's open tasks.")
        }
        .with::<i::ThreadInbox>(),
        Spec {
            category: Category::Thread,
            result: "JSON: the `task_update` entry (state `claimed`) and `claim` (`holder` `<member>/<session>@<epoch>`, `session`, `sessions`, `lease_until_ms`).",
            errors: &["task_claimed"],
            instructions: "Claims a task for one session of the agent, with a lease the daemon renews while that session works on it. A task another session holds is refused with `task_claimed`, naming the holder; when the lease runs out the task is queued again with a note. The daemon claims tasks itself when it gives one to a session; an agent calls this to take a task from its inbox while it works (for its own session).",
            capabilities: &["thread.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.thread().claim(_:)",
            rust_symbol: "thread::Thread::claim",
            ..Spec::new("thread_claim", "Claim a task.")
        }
        .with::<i::ThreadClaim>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `reply` (the reply entry) and `update` (the `task_update` entry).",
            errors: &["forbidden"],
            instructions: "Replies under a task and sets its state (`done` by default). When an agent ends a turn without replying, the daemon posts the turn's answer as its reply.",
            capabilities: &["thread.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.thread().reply(_:text:)",
            rust_symbol: "thread::Thread::reply",
            ..Spec::new("thread_reply", "Reply to a task and update it.")
        }
        .with::<i::ThreadReply>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `imported` (entries added), `skipped` (import ids already imported) and `note` (the system entry).",
            errors: &["permission_denied"],
            instructions: "For apps moving their own earlier conversation into the Space's thread once (Cua Bots, OpenKoalaBots). Only this machine's own user may import; each entry keeps its original time, is marked `imported`, and is checked for secrets like a post. An entry whose `import_id` was imported before is skipped, so a repeated import adds nothing.",
            capabilities: &["thread.import"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.thread().import(_:source:)",
            rust_symbol: "thread::Thread::import",
            ..Spec::new("thread_import", "Import an app's earlier conversation into a Space's thread.")
        }
        .with::<i::ThreadImport>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `members`, each with `name`, `identity` {`origin` (`space` or `external`), `handle`, `display_name`, `collides`, `icon`, `harness` {`id`, `version`, `reported`}, `owner` {`id`, `name`}, `device` (`id`, `name`; external only), `joined` {`method`, `at_ms`, `by`}, `state` (`active` or `pending`), `last_seen_ms`}, `harness`, `model`, `sessions`, `policy`, `added_by`, `member_since_ms`, `paused`, `run_id`, `status` {`state`: `working`, `idle`, `waiting` (for an answer to an approval it asked), `paused`, `pending` (waiting to be let in), `asleep` (no run) or `failed`, the busiest of its sessions; `activity`: a short line, when there is one}, `open_tasks`, `session_states` (one per session: `k`, `state`, `activity`, `task_seq`, `run_id`), `rights` and `you_can`; with `overview`, also `dm` {`unread`, `last` {`seq`, `ts_ms`, `text`, `author`} or null} and `needs_you` (each `seq`, `kind`: `approval` or `question`, `text`, `ts_ms`).",
            instructions: "The agent members of the Space with what each is doing. People members are the owner, the accounts the Space is shared with (space_shares) and anyone who posted.",
            capabilities: &["thread.read"],
            sdk_symbol: "space.agents().list()",
            rust_symbol: "Persistent::members",
            ..Spec::new("agent_members", "List a Space's agent members.")
        }
        .with::<i::AgentMembers>(),
        Spec {
            category: Category::Thread,
            result: "JSON: the member as in `agent_members` (with `rights`) and `you_can`.",
            errors: &["permission_denied"],
            instructions: AGENT_SET_RIGHTS_INSTRUCTIONS,
            capabilities: &["agent.persistent.manage"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.agents().get(_:).setRights(_:)",
            rust_symbol: "Persistent::set_rights",
            ..Spec::new("agent_set_rights", "Choose who can task, stop and approve an agent.")
        }
        .with::<i::AgentSetRights>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `member` (your row, as in `agent_members`), `handle`, `space`, `space_name` and `line` (one sentence saying who you are now).",
            errors: &["permission_denied", "invalid_argument"],
            instructions: THREAD_JOIN_INSTRUCTIONS,
            capabilities: &["thread.post"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.thread().join(name:harness:version:icon:)",
            rust_symbol: "Persistent::thread_join",
            ..Spec::new("thread_join", "Set your name and icon in a Space's thread.")
        }
        .with::<i::ThreadJoin>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `id`, `code` (`XXXX-XXXX`; shown only here), `url`, `line` (the one line to paste into an agent) and `expires_at_ms`.",
            errors: &["permission_denied", "invalid_argument"],
            instructions: JOIN_CODE_INSTRUCTIONS,
            capabilities: &["thread.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.thread().createJoinCode(ttl:canTask:askBeforeJoin:)",
            rust_symbol: "Persistent::join_code_create",
            ..Spec::new("thread_join_code_create", "Make a one-time code an agent joins the Space with.")
        }
        .with::<i::ThreadJoinCodeCreate>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `codes`, newest first, each `id`, `created_at_ms`, `expires_at_ms`, `state` (`open`, `pending_approval`, `redeemed`, `denied`, `expired` or `cancelled`), `can_task`, `ask_before_join`, `created_by` and, once used, `member_handle`.",
            instructions: "The Space's join codes (never the codes themselves). The owner and admins see every code; others see their own.",
            capabilities: &["thread.read"],
            sdk_symbol: "space.thread().joinCodes()",
            rust_symbol: "Persistent::join_code_list",
            ..Spec::new("thread_join_code_list", "List a Space's join codes.")
        }
        .with::<i::ThreadJoinCodeList>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `id` and `state` (`cancelled`).",
            errors: &["permission_denied", "not_found", "invalid_argument"],
            instructions: "An open code stops working; a code whose agent is waiting to be let in is closed and the agent dropped. Whoever made the code, the owner or an admin.",
            capabilities: &["thread.post"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.thread().cancelJoinCode(_:)",
            rust_symbol: "Persistent::join_code_cancel",
            ..Spec::new("thread_join_code_cancel", "Cancel an unused join code.")
        }
        .with::<i::ThreadJoinCodeCancel>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `space`, `space_name`, `space_owner`, `handle`, `member_id`, `state` (`active`, or `pending` until the owner or an admin lets it in), `owner`, `device` and `key` (the member key: returned once, for the keychain; never show or store it elsewhere).",
            errors: &["not_signed_in", "join_code_refused", "rate_limited", "permission_denied"],
            instructions: JOIN_REDEEM_INSTRUCTIONS,
            capabilities: &["thread.post"],
            annotations: Annotations::mutating(),
            sdk_symbol: "Spaces.call_tool_json(\"thread_join_redeem\")",
            rust_symbol: "Persistent::redeem",
            ..Spec::new("thread_join_redeem", "Redeem a join code on this computer.")
        }
        .with::<i::ThreadJoinRedeem>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `space` and `ask_before_join`.",
            errors: &["permission_denied"],
            instructions: "Reads a Space's thread settings, or (the owner and admins) sets them. `ask_before_join` (\"Ask before members' agents join\", on by default): an agent redeemed by anyone other than this computer's own user waits until the owner or an admin answers its join request.",
            capabilities: &["thread.read"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.thread().settings(askBeforeJoin:)",
            rust_symbol: "Persistent::thread_settings",
            ..Spec::new("thread_settings", "Read or change a Space's thread settings.")
        }
        .with::<i::ThreadSettings>(),
        Spec {
            category: Category::Thread,
            result: "JSON: `removed` (the handles of the agents removed).",
            errors: &["permission_denied"],
            instructions: "Removes every agent connected from that computer (its member keys stop at once), with a note in each thread. `cua logout` and removing this computer from the account call it for this computer.",
            capabilities: &["agent.persistent.manage"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "Spaces.call_tool_json(\"thread_device_revoke\")",
            rust_symbol: "Persistent::revoke_device",
            ..Spec::new("thread_device_revoke", "Remove the agents connected from a computer.")
        }
        .with::<i::ThreadDeviceRevoke>(),
        // --- Session teleport ---------------------------------------------------
        Spec {
            category: Category::Teleport,
            result: "JSON: `app`, `display_name`, `scope`, `items` (each `relative_path`, `label`, `estimated_bytes`, `is_sensitive`, `is_checked_by_default`), `total_estimated_bytes`, `notes`; with `space`, a `receiver` (`supported`, `limitation`, `app_installed`, `supported_apps`).",
            instructions: "Read on this computer from the app's own provider: items, sizes, which are sensitive and which are checked by default. With `space`, also reports whether the Space can import the app.",
            capabilities: &["session.teleport.inspect"],
            host_requires: &[host::APP_SESSIONS],
            sdk_symbol: "space.sessions.manifest(for:scope:)",
            rust_symbol: "Teleport::manifest",
            notes: &["The manifest is the thing an Approval is minted from: the SDK will not send a teleport the caller has not seen a manifest for."],
            ..Spec::new("teleport_manifest", "Describe what a session teleport would move.")
        }
        .with::<i::TeleportManifest>(),
        Spec {
            category: Category::Teleport,
            result: "JSON. First call: `consent_required: true`, a `request_id`, `would_send` and `approve`. Retry with `request_id` once the user approved: `moved: true` with `transferred_paths` (what the Space imported), `import_ids` and `expires_ms`; or still `consent_required` while pending; or an `error` with the Keyvault code (`requires_cua_app`, `denied`, `disabled`) when it cannot proceed.",
            errors: &["teleport_refused", "transfer_failed"],
            instructions: "Moves the app's signed-in session into the Space, but only through the Cua Keyvault and only with the user's consent. This tool never delivers on its own: the first call files a Keyvault access request and returns a request_id; the user approves it in Cua (Touch ID or the login password) or an unattended rule matches; a later call with the same request_id has the Keyvault broker perform the delivery over the daemon's authenticated channel. Without `include` the manifest's default-checked items move, never everything.",
            capabilities: &["session.teleport.send"],
            requires: &["teleport.*"],
            host_requires: &[host::APP_SESSIONS],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.sessions.send(_ approval:)",
            rust_symbol: "Teleport::send",
            notes: &[
                "Works on every provider. A comment in the SDK claiming teleport was cloud-only was stale and was a real bug: callers were being told to provision a metered Space for work a free Local Space does.",
                "Consent is owned by the Keyvault broker, not this tool: the automation caller is treated as an unverified third party, so a delivery needs a live consent or an audited unattended rule and can never be self-approved (red-team E2).",
            ],
            ..Spec::new("teleport_app", "Move an app session into the Space.")
        }
        .with::<i::TeleportApp>(),
        // --- Keyvault ------------------------------------------------------------
        Spec {
            category: Category::Keyvault,
            result: "JSON. First call: `status: \"pending\"`, a `request_id`, `site` and `space`. Retry with `request_id`: `status: \"filled\"` with `site`, `origin`, `username_hint` (masked), `submitted`, `page_url` and the browser `target_id`/`tab_id` it signed in; `status: \"pending\"` while the user decides; `status: \"denied\"` when they declined. Never the password.",
            errors: &["login_refused"],
            instructions: SITE_LOGIN_INSTRUCTIONS,
            capabilities: &["keyvault.login"],
            requires: &["driver"],
            annotations: Annotations::mutating(),
            sdk_symbol: "space.requestSiteLogin(url:)",
            rust_symbol: "site_login::request_site_login",
            notes: &[
                "The password never reaches the caller: the Keyvault fills the form through the Space's cua-driver and returns only a masked username.",
                "Approval is required by default for every sign-in (one use per approval); only an unattended rule the user wrote skips it.",
            ],
            ..Spec::new("request_site_login", "Sign in to a site in the Space with the user's saved password.")
        }
        .with::<i::RequestSiteLogin>(),
        // --- Hotspot -------------------------------------------------------------
        Spec {
            category: Category::Hotspot,
            result: "JSON: the hotspot (`space`, `state`, `hotspot_id`, `socks_address`, `active_connections`, `bytes_out`, `bytes_in`, `served_here`).",
            instructions: "Starts the Space's reverse-SOCKS hotspot (TunnelService.StartHotspot) and serves its egress from this computer, so the Space browses as this computer. Runs until hotspot_stop, delete_space, or this server exiting.",
            capabilities: &["hotspot.control"],
            requires: &["hotspot"],
            annotations: Annotations::mutating(),
            sdk_symbol: "connection.hotspot.start(sharingWith:)",
            rust_symbol: "Spaces::hotspot_start",
            notes: &["Capability-gated rather than cloud-only: any Space whose spacesd supports `hotspot` can borrow this computer's network."],
            ..Spec::new("hotspot_start", "Start sharing this computer's network with a Space.")
        }
        .with::<i::HotspotStart>(),
        Spec {
            category: Category::Hotspot,
            result: "JSON: `{\"stopped\": [...]}`, the hotspots that were stopped.",
            capabilities: &["hotspot.control"],
            requires: &["hotspot"],
            annotations: Annotations::mutating(),
            sdk_symbol: "connection.hotspot.stop()",
            rust_symbol: "Spaces::hotspot_stop",
            ..Spec::new("hotspot_stop", "Stop sharing this computer's network.")
        }
        .with::<i::HotspotTarget>(),
        Spec {
            category: Category::Hotspot,
            result: "JSON: `{\"hotspots\": [...]}`, each shaped like hotspot_start's result.",
            capabilities: &["hotspot.read"],
            requires: &["hotspot"],
            sdk_symbol: "connection.hotspot.status()",
            rust_symbol: "Spaces::hotspot_statuses",
            ..Spec::new("hotspot_status", "Report which Spaces are sharing this computer's network, with counters.")
        }
        .with::<i::HotspotTarget>(),
        // --- Cua Volume -------------------------------------------------------------
        Spec {
            category: Category::Volume,
            result: "JSON: `path`, `principal` and `entries` (each `path`, `name`, `folder`, `size`, `modified_ms`, `etag`, `mode`, and `sync` when a file has something to say: `state` `pending_upload`, `conflict` (with `conflict_path`) or `conflict_copy`, and `written_by` when another device wrote it last).",
            errors: &["forbidden", "volume_backend"],
            instructions: DRIVE_INSTRUCTIONS,
            capabilities: &["drive.read"],
            sdk_symbol: "spaces.volumeLs(path:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::ls",
            ..Spec::new("volume_ls", "List a folder of the Cua Volume.")
        }
        .with::<i::DriveLs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `path`, `size`, `etag`, `version`, `modified_ms`, `encoding` (`utf8` or `base64`), `content`, and `sync` while the drive's services run (`state` `synced`, `pending_upload`, `conflict` or `conflict_copy`; `written_by`, `conflict_path`).",
            errors: &["forbidden", "volume_backend"],
            instructions: "Reads at most 8 MiB. Text comes back as utf8, anything else as base64.",
            capabilities: &["drive.read"],
            sdk_symbol: "spaces.volumeRead(path:version:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::read",
            ..Spec::new("volume_read", "Read a file from the Cua Volume.")
        }
        .with::<i::DriveRead>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the new version's `key`, `size`, `etag`, `version` and `modified_ms`.",
            errors: &["forbidden", "precondition_failed", "secret_detected", "volume_backend"],
            instructions: "Every write is a new version (history is kept). if_etag makes it a compare-and-swap, create_only makes it create-only. Writes under agents/ are scanned for secrets and refused by kind and line; keep credentials in the Keyvault.",
            capabilities: &["drive.write"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeWrite(path:content:encoding:ifEtag:createOnly:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::write",
            ..Spec::new("volume_write", "Write a file to the Cua Volume.")
        }
        .with::<i::DriveWrite>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `{\"deleted\": <path>}`.",
            errors: &["forbidden", "precondition_failed", "volume_backend"],
            instructions: "Adds a delete marker; volume_history and volume_restore still reach the old versions.",
            capabilities: &["drive.write"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeDelete(path:ifEtag:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::delete",
            ..Spec::new("volume_delete", "Delete a file from the Cua Volume.")
        }
        .with::<i::DriveDelete>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `path` and `versions` (each `version`, `size`, `modified_ms`, `deleted`, `latest`), newest first.",
            errors: &["forbidden", "volume_backend"],
            capabilities: &["drive.read"],
            sdk_symbol: "spaces.volumeHistory(path:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::history",
            ..Spec::new("volume_history", "List a Cua Volume file's versions.")
        }
        .with::<i::DriveHistory>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the new current version's `key`, `size`, `etag`, `version` and `modified_ms`.",
            errors: &["forbidden", "secret_detected", "volume_backend"],
            instructions: "Writes the old version's content as a new version, so the restore itself can be undone.",
            capabilities: &["drive.write"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeRestore(path:version:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Session::restore",
            ..Spec::new("volume_restore", "Restore an old version of a Cua Volume file.")
        }
        .with::<i::DriveRestore>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the grant (`id`, `principal`, `prefix`, `mode`, `created_ms`, `expires_ms`, `revoked`, `note`).",
            errors: &["not_confirmed", "forbidden"],
            instructions: "Only for the user: widens an agent's or a Space's access past the defaults (an agent reads public/ and writes its own home and its Space's folder). The user confirms with presence (Touch ID, the login password or the vault passphrase) before anything widens; a grant that widens nothing is refused.",
            capabilities: &["drive.grant"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeGrant(principal:prefix:mode:expiresInSecs:note:)",
            rust_symbol: "cua_volume::Drive::grant",
            ..Spec::new("volume_grant", "Grant an agent or Space more Cua Volume access.")
        }
        .with::<i::DriveGrant>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the revoked grant.",
            errors: &["forbidden"],
            capabilities: &["drive.grant"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeRevoke(grantId:)",
            rust_symbol: "cua_volume::Drive::revoke",
            ..Spec::new("volume_revoke", "Revoke a Cua Volume grant.")
        }
        .with::<i::DriveRevoke>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `{\"grants\": [...]}`, live grants only unless `all`.",
            errors: &["forbidden"],
            capabilities: &["drive.grant"],
            sdk_symbol: "spaces.volumeGrants(all:)",
            rust_symbol: "cua_volume::Drive::grants",
            ..Spec::new("volume_grants", "List Cua Volume grants.")
        }
        .with::<i::DriveGrants>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the request (`id`, `principal`, `prefix`, `mode`, `reason`, `created_ms`).",
            errors: &["forbidden"],
            instructions: "Files a request the user sees in Cua; it grants nothing by itself. Poll volume_ls or volume_read to see when access arrives.",
            capabilities: &["drive.request"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeRequestAccess(prefix:mode:reason:asAgent:inSpace:)",
            rust_symbol: "cua_volume::Drive::request_access",
            ..Spec::new("volume_request_access", "Ask the user for more Cua Volume access.")
        }
        .with::<i::DriveRequestAccess>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `{\"requests\": [...]}`.",
            errors: &["forbidden"],
            capabilities: &["drive.grant"],
            sdk_symbol: "spaces.volumeRequests()",
            rust_symbol: "cua_volume::Drive::requests",
            ..Spec::new("volume_requests", "List Cua Volume access requests waiting for the user.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the grant the request became.",
            errors: &["not_confirmed", "forbidden"],
            instructions: "Only for the user, with presence.",
            capabilities: &["drive.grant"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeApprove(requestId:expiresInSecs:)",
            rust_symbol: "cua_volume::Drive::approve",
            ..Spec::new("volume_approve", "Approve a Cua Volume access request.")
        }
        .with::<i::DriveApprove>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `{\"denied\": <request id>}`.",
            errors: &["forbidden"],
            capabilities: &["drive.grant"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.volumeDeny(requestId:)",
            rust_symbol: "cua_volume::Drive::deny",
            ..Spec::new("volume_deny", "Decline a Cua Volume access request.")
        }
        .with::<i::DriveDeny>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `events` (each `seq`, `ts_ms`, `principal`, `action`, `path`, `detail`), newest first, and `verified` (with `error` when the hash chain does not verify).",
            errors: &["forbidden"],
            instructions: "Every grant, revocation, request, refusal, blocked secret, sync, lease and agent access, from a hash-chained log; an edited or truncated log reports verified=false.",
            capabilities: &["drive.audit"],
            sdk_symbol: "spaces.volumeAudit(limit:)",
            rust_symbol: "cua_volume::audit::AuditLog::tail",
            ..Spec::new("volume_audit", "Read the Cua Volume audit log.")
        }
        .with::<i::DriveAudit>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `backend` (`fs` or `s3`), `fs_path`, `s3` (`endpoint`, `region`, `bucket`, `root`, `path_style`), `has_keys`, `cloud_available`.",
            errors: &["forbidden"],
            instructions: "Only for the user. Keys are never returned; `has_keys` says whether the credential store holds them.",
            capabilities: &["drive.admin"],
            sdk_symbol: "spaces.volumeStorage()",
            rust_symbol: "cua_volume::service::DriveService::storage",
            ..Spec::new("volume_storage", "Show where the Cua Volume keeps its bytes.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `ok`, `reachable`, `authorized`, `versioning`, `detail` (a sentence when not ok saying how to fix it), `problem` (`unreachable`, `bad_keys`, `forbidden`, `bucket_missing`, `versioning_off`, `path_style_needed`) and `applied` (saved and switched live).",
            errors: &["forbidden", "invalid_argument"],
            instructions: "Only for the user. Tests the storage first (bucket versioning, list, then write, read back and delete a probe object; retries path-style addressing to tell when a self-hosted store needs it); with dry_run nothing else happens. Otherwise, when the test passes, saves the setting and the keys (credential store) and switches the running drive to it without a restart. The bucket must have versioning on. cloud is refused.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeStorageSet(update:)",
            rust_symbol: "cua_volume::service::DriveService::set_storage",
            ..Spec::new("volume_storage_set", "Test or change where the Cua Volume keeps its bytes.")
        }
        .with::<i::DriveStorageSet>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `enabled`, `state` (`off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`), `method` (`nfs`, `fuse`, `fskit`, `none`), `path`, `volume_name`, `detail`, `settings_url`; plus the Spaces' own: `volumes` (mounted in a Space: `space`, `mount_path`, `backend`, `principal`) and `volume_errors` (each `space` and `error`: why a Space has no Cua Volume, for example no /dev/fuse in a container on runc).",
            errors: &["forbidden"],
            instructions: "Only for the user. The drive can appear as a volume (Finder on macOS, a FUSE mount on Linux); it is off until the user turns it on with volume_mount.",
            capabilities: &["drive.admin"],
            sdk_symbol: "spaces.volumeMountStatus()",
            rust_symbol: "cua_volume::service::DriveService::mount_status",
            ..Spec::new("volume_mount_status", "Show whether the Cua Volume is mounted as a volume.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the mount status after the attempt (see volume_mount_status).",
            errors: &["forbidden"],
            instructions: "Only for the user. Turns the mount on (kept across restarts) and mounts. Idempotent.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeMount()",
            rust_symbol: "cua_volume::service::DriveService::mount",
            ..Spec::new("volume_mount", "Mount the Cua Volume as a volume.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the mount status after unmounting.",
            errors: &["forbidden", "volume_backend"],
            instructions: "Only for the user. Turns the mount off; files still uploading land first.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeUnmount()",
            rust_symbol: "cua_volume::service::DriveService::unmount",
            ..Spec::new("volume_unmount", "Unmount the Cua Volume volume.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `device_id`, `device_name`, `feed` (`live`, `off` for a store on this computer, `offline` when the bucket stops answering, with `last_error`), `poll_interval_ms`, `last_poll_ms`, `last_remote_change_ms`, `pending_uploads`, `pending_bytes`, `conflicts` (each `path`, `conflict_path`, `winner_device`, `loser_device`, `winner_version`, `loser_version`, `ts_ms`), `devices` (each `id`, `name`, `this_device`, `last_seen_ms`, `last_change_ms`, `changes`), `last_error`, `pending` (each `path`, `bytes`: files still uploading), `backend` (`fs` or `s3`), `mount` (this computer's mount: `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`), `cache` (a bucket's block cache, when there is one) and `volumes` (the volume mounted in Spaces: each `space`, `mount_path`, `backend`, `principal`).",
            errors: &["forbidden"],
            instructions: "Devices sharing one bucket learn each other's changes through a change log in the bucket itself. A write that lost to a later one is never dropped: it stays in the file's history and as the visible conflict copy. A persistent agent may call it too (through its bridge): it sees the pending uploads and conflicts of files it can read and its own Space's volume. In a Space it reports this computer's view.",
            capabilities: &["drive.read"],
            sdk_symbol: "spaces.volumeSyncStatus()",
            rust_symbol: "cua_volume::service::DriveService::sync_status",
            ..Spec::new("volume_sync_status", "Show the Cua Volume's sync state across devices.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `events` (each `seq`, `ts_ms`, `kind`, `path`, `device`, `size`, `version`, `detail`) and `next_seq`.",
            errors: &["forbidden"],
            instructions: "Only for the user. A long poll: answers at once when events newer than since_seq exist, else waits up to wait_ms for one. Kinds: remote_change, remote_delete, upload_started, upload_done, upload_failed, conflict, error.",
            capabilities: &["drive.admin"],
            sdk_symbol: "spaces.volumeSyncEvents(sinceSeq:waitMs:)",
            rust_symbol: "cua_volume::service::DriveService::sync_events",
            ..Spec::new("volume_sync_events", "Wait for Cua Volume sync events.")
        }
        .with::<i::DriveSyncEvents>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `{\"resolved\": <path>}`.",
            errors: &["forbidden", "not_found"],
            instructions: "Only for the user. Clears the conflict from the list; both files stay.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeSyncResolve(path:)",
            rust_symbol: "cua_volume::service::DriveService::sync_resolve",
            ..Spec::new("volume_sync_resolve", "Mark a Cua Volume sync conflict as seen.")
        }
        .with::<i::DriveSyncResolve>(),
        Spec {
            category: Category::Volume,
            result: "JSON: `dir`, `size_bytes`, `capacity_bytes`, `block_bytes`, `blocks`, `hits`, `misses`, `hit_rate`, `prefetched_bytes`, `evictions`.",
            errors: &["forbidden"],
            instructions: "Only for the user. Reads of a remote store go through a local block cache; counters are since the runtime started.",
            capabilities: &["drive.admin"],
            sdk_symbol: "spaces.volumeCacheStats()",
            rust_symbol: "cua_volume::service::DriveService::cache_stats",
            ..Spec::new("volume_cache_stats", "Show the Cua Volume block cache.")
        }
        .with::<i::NoArgs>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the cache stats after the change.",
            errors: &["forbidden", "invalid_argument"],
            instructions: "Only for the user. Evicts down to the new cap at once; kept across restarts.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeCacheSet(capacityBytes:)",
            rust_symbol: "cua_volume::service::DriveService::cache_set",
            ..Spec::new("volume_cache_set", "Set the Cua Volume block cache's size cap.")
        }
        .with::<i::DriveCacheSet>(),
        Spec {
            category: Category::Volume,
            result: "JSON: the cache stats after clearing.",
            errors: &["forbidden"],
            instructions: "Only for the user. Drops every cached block; files stay in the drive.",
            capabilities: &["drive.admin"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.volumeCacheClear()",
            rust_symbol: "cua_volume::service::DriveService::cache_clear",
            ..Spec::new("volume_cache_clear", "Clear the Cua Volume block cache.")
        }
        .with::<i::NoArgs>(),
        // --- Sharing -------------------------------------------------------------
        Spec {
            category: Category::Sharing,
            result: SHARES_RESULT,
            errors: &["relay", "permission_denied", "unauthenticated"],
            instructions: SHARE_INSTRUCTIONS,
            capabilities: &["space.share"],
            host_requires: &[host::RELAY_ACCOUNT],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.sharing.share(with:role:)",
            rust_symbol: "Spaces::share_space",
            notes: &["Viewers and editors are enforced by the Space's own cua-spacesd, not by the caller; a Space whose driver predates relay_attach reports capability_missing."],
            ..Spec::new("share_space", "Let someone in your team watch or edit a Space.")
        }
        .with::<i::ShareSpace>(),
        Spec {
            category: Category::Sharing,
            result: SHARES_RESULT,
            errors: &["relay", "permission_denied", "unauthenticated"],
            instructions: "Removes one account (or everyone) at once: the relay cuts their open streams and refuses new calls. With no `who`, the Space stops being shared and a Space that is not a host leaves the relay.",
            capabilities: &["space.share"],
            host_requires: &[host::RELAY_ACCOUNT],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "space.sharing.unshare(_:)",
            rust_symbol: "Spaces::unshare_space",
            ..Spec::new("unshare_space", "Stop sharing a Space with an account, or with everyone.")
        }
        .with::<i::UnshareSpace>(),
        Spec {
            category: Category::Sharing,
            result: "JSON: the shares (see share_space) and, with `audit`, `audit`: the newest owner-side lines (`ts_ms`, `action`: share, unshare, attach or detach, `space`, `detail`).",
            errors: &["relay", "unauthenticated"],
            instructions: "Who the Space is shared with, in which role, and who of them is connected through the relay now.",
            capabilities: &["space.share"],
            host_requires: &[host::RELAY_ACCOUNT],
            sdk_symbol: "space.sharing.shares()",
            rust_symbol: "Spaces::space_shares",
            ..Spec::new("space_shares", "List who a Space is shared with.")
        }
        .with::<i::SpaceShares>(),
        Spec {
            category: Category::Sharing,
            result: "JSON: `space`, `relay_space` (`relay:<computer>`: how the account's other devices reach it), `machine`, `url` and `online`.",
            errors: &["relay", "unauthenticated"],
            instructions: "Publishes the Space as a computer of the signed-in account on the cua.ai relay, the way `cua host setup` publishes this computer: the Space's own driver joins the relay over outbound WSS with a computer token the account registered, so no inbound port is opened. The account's other devices (a phone off this network, the Cua app on another computer) then open `relay_space`; nobody else can until share_space adds them. Idempotent, and it re-attaches a Space that restarted. Needs a driver with relay_attach (a host is already registered).",
            capabilities: &["space.relay"],
            host_requires: &[host::RELAY_ACCOUNT],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.relay.register(_:)",
            rust_symbol: "Spaces::relay_register",
            ..Spec::new("relay_register_space", "Publish a Space on the relay for the account's other devices.")
        }
        .with::<i::SpaceOnly>(),
        Spec {
            category: Category::Sharing,
            result: "JSON: `space` and `unregistered` (false when it was not on the relay).",
            errors: &["relay", "unauthenticated", "permission_denied", "invalid_argument"],
            instructions: "Takes the Space off the relay: its driver leaves and its computer is removed, with every share. A `relay:<computer>` id removes that computer's record from the relay directory, which only its owner may do: a Space that registered itself (also after it was deleted), or a computer that is no longer connected. A host that is connected now is taken off with `cua host remove` on that computer instead.",
            capabilities: &["space.relay"],
            host_requires: &[host::RELAY_ACCOUNT],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.relay.unregister(_:)",
            rust_symbol: "Spaces::relay_unregister",
            ..Spec::new("relay_unregister_space", "Take a Space off the relay.")
        }
        .with::<i::SpaceOnly>(),
        // --- your cloud ------------------------------------------------------
        Spec {
            category: Category::Cloud,
            result: CLOUD_STATUS_RESULT,
            instructions: CLOUD_INSTRUCTIONS,
            capabilities: &["cloud.read"],
            sdk_symbol: "spaces.cloud.status(provider:)",
            rust_symbol: "cua_cloud::Clouds::status",
            ..Spec::new(
                "cloud_status",
                "List your clouds (AWS, Google Cloud, Hetzner, Modal): found credentials, the connected account and region, what each can run at what hourly cost, and what Cua created there.",
            )
        }
        .with::<i::CloudStatus>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: the cloud as `cloud_status` lists it, whose `status` is the outcome (`connected`, `needs-trust`, `checking`, `waiting-for-approval` or `failed`), plus `checks` (each `name`, `ok`, `detail`), `next` (absent when connected; `run`: the exact command for the agent, or `open`: a page for a person, with `paste` when there is a command to paste there, and `why`) and `verify` (the command that confirms it).",
            errors: &["cloud", "invalid_argument"],
            instructions: "Without auth it keeps how the cloud is linked already (an OIDC or sealed link stays so, so a run that only changes budget_usd_month or max_size never falls back to a CLI sign-in); a cloud not linked yet, or auth=\"cli\", runs cloud_test first and connects only when every check passes, storing the profile, region, project or environment names under the cua home (never a key: the cloud's own CLI sign-in stays where it is). auth=\"oidc\" links AWS, Google Cloud or Azure with no stored key (Azure is always oidc; a Google Cloud project_number implies it): it never prompts and is idempotent, so call it again with the same arguments until `status` is `connected`, following `next` each time. It needs budget_usd_month first. Until the account is named it returns needs-trust and the trust step: a page for the person (AWS: the trust stack, then ask for the 12-digit account_id; Google Cloud: Cloud Shell and the trust script, then ask for the project_number it prints; Azure: Deploy to Azure, then ask for the outputs subscription_id, tenant_id and client_id), or, when an agent asks on a machine signed in to gcloud or az, the exact command to run. Once named, the link is recorded on Cua's issuer and check=true proves it (a check token, the exchange with the cloud, a dry run, permission check or validate; nothing is created) and saves it. Hetzner and Modal with a token and budget_usd_month seal the token to the account's devices (auth=\"sealed\", the default on a device with the account's pool keys): it is checked first (nothing is created), Cua's servers keep only ciphertext, any device of the account opens it in memory when it calls the cloud, and no tool ever returns it; on another device, auth=\"sealed\" without a token links the account's sealed token. A link, token or limit raise made by an agent returns waiting-for-approval until a person approves it in Cua Spaces. budget_usd_month and max_size (`4x16`) also apply to CLI links: machines past them are refused before any cloud call. With make_default, create_space without `on` creates there (`default.on`).",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.cloud.connect(_:makeDefault:ttlHours:)",
            rust_symbol: "cua_cloud::Clouds::connect",
            ..Spec::new("cloud_connect", "Connect a cloud account Spaces can be created in.")
        }
        .with::<i::CloudConnect>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `provider`, `ok`, `account` (the account, project or workspace the credentials reach) and `checks` (each `name`, `ok`, `detail`: credentials, permissions by dry run, quota, the region and machine types).",
            errors: &["cloud"],
            instructions: "Creates nothing: it signs in with the cloud's own CLI credentials (or, for a link without a stored key, a token from Cua's issuer exchanged with the cloud), asks the cloud to validate a create without doing it (AWS DryRun, Google Cloud permission checks, an Azure deployment validate), and reads quotas. A failed check says what to fix.",
            capabilities: &["cloud.read"],
            sdk_symbol: "spaces.cloud.test(_:)",
            rust_symbol: "cua_cloud::Clouds::test",
            ..Spec::new("cloud_test", "Check a cloud account without creating anything.")
        }
        .with::<i::CloudTest>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `provider`, `disconnected`, `deleted` (with delete=true: the receipt, each resource deleted first) and `left` (resources Cua still records there).",
            errors: &["cloud"],
            instructions: "Refuses (a `cloud` error listing them) while Cua-created instances or sandboxes still run in that cloud, because a disconnected cloud is no longer swept and they would bill unseen. With delete=true it first deletes everything Cua created there (the same tag and record checks as cloud_sweep with all=true), then asks the cloud again and forgets the connection (and `default.on` when it named this cloud) only when nothing is left; if something remains it keeps the connection and says what. Nothing that Cua did not create is ever touched.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.disconnect(_:delete:)",
            rust_symbol: "cua_cloud::Clouds::disconnect",
            ..Spec::new("cloud_disconnect", "Forget a connected cloud account (deleting what Cua created there first, with delete=true).")
        }
        .with::<i::CloudDisconnect>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: the cloud as `cloud_status` lists it once approved or denied (`status`, `approval` empty, the limits in force).",
            errors: &["cloud", "invalid_argument"],
            instructions: "Only for a person, never for an agent: no MCP server lists or runs it, the daemon refuses it when it runs for an agent, and Cua's issuer refuses an agent's proof. It approves what an agent asked for on that cloud (its link, or a raise of its budget or largest machine; see `approval`, `approval_agent`, `pending_budget_usd_month` and `pending_max_size` in cloud_status), or with deny=true declines it: a raise is dropped and the limits in force stay; a link never approved is disconnected.",
            capabilities: &["cloud.approve"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.approve(_:deny:)",
            rust_symbol: "cua_cloud::Clouds::approve",
            ..Spec::new("cloud_approve", "Approve (or deny) what an agent asked for on a cloud: its link or its raised limits. For a person only.")
        }
        .with::<i::CloudApprove>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `dry_run` and `resources`, each `provider`, `id`, `type`, `space`, `expires`, `action` (`delete`, `deleted`, `keep` or `failed`) and `reason`.",
            errors: &["cloud"],
            instructions: "Acts only on resources that carry Cua's tags (cua-managed=true and this device's cua-owner) and that this cua home recorded, or that carry this device's owner id: never on anything else in the account. Default dry_run=true lists what it would delete: expired Spaces (past their ttl), resources whose Space is gone, and with all=true every Cua resource there.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.sweep(provider:dryRun:all:)",
            rust_symbol: "cua_cloud::Clouds::sweep",
            ..Spec::new("cloud_sweep", "Find and delete what Cua left in your clouds.")
        }
        .with::<i::CloudSweep>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `plan` (`assignments`: `group`, `target` (`on`: `host` with `name`, or `new` with `index`), `count`; `new_hosts`: `provider`, `type_name`, `region`, `usd_per_hour`, `boot_secs`, `billed_secs`; `unplaced` with `reason`; `added_usd_per_hour`, `projected_usd`, `lower_bound_usd`, `solver`, `ready_in_secs`, `explain`), `groups` (the demand it detected) and `pool` (`hosts`, `cloud`, `offers`, `constraints`, `month_usd`); with apply, `added` (the hosts added).",
            errors: &["cloud", "limit_exceeded"],
            instructions: ELASTIC_INSTRUCTIONS,
            capabilities: &["cloud.read"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.plan(_:)",
            rust_symbol: "Spaces::elastic_plan",
            ..Spec::new("cloud_plan", "Show (or apply) the elastic pool's plan for some Spaces: where they go, which machines it adds, and the cost.")
        }
        .with::<i::CloudPlan>(),
        Spec {
            category: Category::Cloud,
            result: "JSON array: each host `name`, `provider`, `type_name`, `region`, `id`, `machine`, `state` (`creating`, `booting`, `joined`, `cordoned`, `offline`, `gone`), `started`, `expires`, `usd_per_hour`, `billing_step_secs`, `billing_min_secs`, `pinned`.",
            errors: &["cloud"],
            instructions: "The hosts the elastic pool (or cloud_add_host) added in the user's linked clouds: machines that provide Spaces to the pool, with their live cost.",
            capabilities: &["cloud.read"],
            sdk_symbol: "spaces.cloud.hosts()",
            rust_symbol: "cua_cloud::Clouds::hosts",
            ..Spec::new("cloud_hosts", "List the hosts the elastic pool added in your clouds.")
        }
        .with::<i::CloudHosts>(),
        Spec {
            category: Category::Cloud,
            result: "JSON array: the hosts added (see cloud_hosts).",
            errors: &["cloud", "invalid_argument"],
            instructions: "Adds hosts by hand (pinned: the scaler never removes them); it costs money in the user's account. Each host is tagged and recorded first, installs cua, joins the pool and ends at its TTL. cloud_remove_host removes it.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.addHost(_:)",
            rust_symbol: "cua_cloud::Clouds::add_host",
            ..Spec::new("cloud_add_host", "Add hosts to your pool in a linked cloud.")
        }
        .with::<i::CloudAddHost>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `removed` (names) and `failed` (each `name`, `error`).",
            errors: &["cloud", "not_found"],
            instructions: "Deletes the hosts' machines (only ones this cua home added and tagged) and their relay entries. The Spaces on them end.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.removeHost(_:)",
            rust_symbol: "cua_cloud::Clouds::remove_host",
            ..Spec::new("cloud_remove_host", "Remove hosts this home added to your pool.")
        }
        .with::<i::CloudRemoveHost>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `policy` (`autoscale`, `max_usd_per_hour`, `monthly_usd`, `max_new_hosts`, `warm`, `interactive_slo_secs`, `routed`, `allow_metal`), `month_usd` (spent this month on elastic hosts), `running_usd_per_hour` and `hosts` (how many run).",
            errors: &["invalid_argument"],
            instructions: "The caps the elastic pool keeps to: it never adds a machine that would take the clouds' cost over max_usd_per_hour or this month's spend over monthly_usd, and with autoscale false it adds none. Fields given are set; the rest stay.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.cloud.budget(_:)",
            rust_symbol: "Spaces::set_elastic_policy",
            ..Spec::new("cloud_budget", "Read or set the elastic pool's budget and autoscale switch.")
        }
        .with::<i::CloudBudget>(),
        Spec {
            category: Category::Cloud,
            result: "JSON array: each `host` and `action` (`action`: `keep` with `until` and `reason`, `cordon` with `reason`, or `remove` with `reason` and `saves_usd`).",
            errors: &["cloud"],
            instructions: "One scale-in pass now (the daemon runs one every 30 s): idle hosts go at the end of the time already paid, the least-used one is cordoned when the others have room.",
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.cloud.reconcile()",
            rust_symbol: "Spaces::elastic_reconcile",
            ..Spec::new("cloud_reconcile", "Run the elastic pool's scale-in pass now.")
        }
        .with::<i::CloudReconcile>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `id`, `image`, `kind`, `cpus`, `memory_mb`, `count`, `ready`, `placements` (`host`, `machine`, `count`, `ready`), `expires_at` (unix seconds) and `new_hosts` (hosts added for it).",
            errors: &["invalid_argument", "limit_exceeded", "cloud"],
            instructions: PREWARM_INSTRUCTIONS,
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating(),
            sdk_symbol: "spaces.prewarm(_:)",
            rust_symbol: "Spaces::prewarm",
            ..Spec::new("prewarm", "Keep Spaces of one shape ready where the elastic pool puts them, so the next creates start in a fraction of a second.")
        }
        .with::<i::Prewarm>(),
        Spec {
            category: Category::Cloud,
            result: "JSON array of prewarms, as `prewarm` returns them, with `ready` counted now.",
            errors: &["cloud"],
            instructions: PREWARM_INSTRUCTIONS,
            capabilities: &["cloud.read"],
            annotations: Annotations::reading(),
            sdk_symbol: "spaces.prewarms()",
            rust_symbol: "Spaces::prewarms",
            ..Spec::new("prewarm_list", "List the prewarms and how many Spaces each keeps ready.")
        }
        .with::<i::PrewarmList>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `removed` (false when there was no such prewarm).",
            errors: &["cloud"],
            instructions: PREWARM_INSTRUCTIONS,
            capabilities: &["cloud.manage"],
            annotations: Annotations::mutating_idempotent(),
            sdk_symbol: "spaces.removePrewarm(_:)",
            rust_symbol: "Spaces::remove_prewarm",
            ..Spec::new("prewarm_remove", "End a prewarm: its ready Spaces go.")
        }
        .with::<i::PrewarmRemove>(),
        Spec {
            category: Category::Cloud,
            result: "JSON array, oldest first, newest last: each `ts` (RFC 3339), `action` (`connect`, `disconnect`, `resource_recorded`, `resource_created`, `resource_confirmed`, `resource_deleted`), `provider`, `kind`, `id`, `space`, `actor` (`user`, `sweep`, `auto-sweep`, `disconnect`, `rollback`), `outcome` (`ok`, `failed`, `refused`) and `detail`.",
            instructions: "Reads the append-only audit log under the cua home (cloud/audit.jsonl, rotated at 5 MB): every connect, disconnect, resource recorded before its create, created, confirmed and deleted (by you, by a sweep, by the automatic sweep, by disconnect with delete), and each budget alert (`budget_alert`). These entries have `source` `device`. For clouds linked to the account (OIDC or a sealed token) it adds what Cua's issuer recorded, with `source` `account`: tokens issued or refused, links made, approved, changed and revoked. It never holds a secret or a token. provider narrows it to one cloud. Default limit 50.",
            capabilities: &["cloud.read"],
            sdk_symbol: "spaces.cloud.audit(limit:)",
            rust_symbol: "cua_cloud::Clouds::audit",
            ..Spec::new("cloud_audit", "Read the log of what Cua did in your clouds.")
        }
        .with::<i::CloudAudit>(),
        Spec {
            category: Category::Cloud,
            result: "JSON: `by`, `since` (`YYYY-MM-DD`), `until` (RFC 3339), `rows` (each `key`: the Space, agent, day or cloud; `usd`: estimated US dollars; `hours`: machine hours; `machines`: how many ran), by day oldest first, otherwise costliest first, and `total_usd`.",
            instructions: "Estimates what the machines Cua created cost: each one's list price times how long it ran inside the window, from the machines still recorded and the usage log of the ones gone (cloud/usage.jsonl under the cua home). by=`space`, `agent` (who started it: an agent's name, `user`, or `unknown` for machines recorded before that was kept), `day` (UTC) or `cloud`. since defaults to the first of this month. It is an estimate: the cloud's own bill is the final word. Pool hosts count under `pool hosts`.",
            capabilities: &["cloud.read"],
            sdk_symbol: "spaces.cloud.cost(by:since:provider:)",
            rust_symbol: "cua_cloud::Clouds::cost",
            ..Spec::new("cloud_cost", "Estimate what Cua's machines in your clouds cost, by Space, agent, day or cloud.")
        }
        .with::<i::CloudCost>(),
    ]
}

const PREWARM_INSTRUCTIONS: &str = "A prewarm keeps count Spaces of one shape (image, kind, cpus, memory_mb) ready on the machines the elastic pool picks (the same solve as on=\"auto\": devices first, then hosts with room, then new cloud machines, under the budget and pins). Any later create of that shape on those machines claims one in a fraction of a second, through every surface, with no extra argument; the pool keeps count ready until ttl_secs. A prewarm the budget, quotas or pins cannot fit is refused and buys nothing. End it with prewarm_remove when the burst is over.";

const ELASTIC_INSTRUCTIONS: &str = "The elastic pool places Spaces with on=\"auto\" (or on a linked cloud) by one solve: the user's devices first (free), then cloud hosts already running, then new cloud hosts (provider x type x region) at the least cost that fits the image's needs (kind, KVM, arch, size learned from earlier Spaces), the start-time goal, the relay's machine limit and the budget (cloud_budget). Idle cloud hosts go at the end of their paid time. cloud_plan shows the decision without doing it; pins (provider, region, instance_type, hosts, no_scale, devices_only, max_usd_per_hour) are constraints of the same solve, so an agent can inspect, adjust and apply.";

const CLOUD_STATUS_RESULT: &str = "JSON: `default_on`, `providers` (each `name`, `title`, `tier` (`vm`: a VM that provides Spaces, `sandbox`: one sandbox per Space), `connected`, `default`, `credentials` (`found`, `source`), `account`, `profile`, `region`, `zone`, `project`, `environment`, `label` (\"AWS · us-west-2\"), `ttl_hours`, `kinds` (each `image`, `kind`, `supported`, `reason`, `machine_type`, `usd_per_hour`), `auth` (`cli`, `oidc` or `sealed` once connected), `status` (`connected`, `waiting-for-approval`, `not-connected`, or `failed` when the link was revoked), `budget_usd_month` and `max_size` when set, `spent_month` (estimated US dollars this calendar month), `running` (machines Cua runs there now), `approval` (`connection` or `limits` while a person's approval is pending, else absent) and, for `sealed`, `holders` (the devices that can open the API token now; at least one must be online for machines to be made there unattended)) and `resources` (what Cua created: `provider`, `id`, `type`, `space`, `region`, `state`, `created`, `expires`, `expired`).";

const CLOUD_INSTRUCTIONS: &str = "Sandboxes and Spaces can run in the user's own cloud account: on=\"aws\" (EC2), \"azure\" (Azure Virtual Machines), \"gcp\" (Compute Engine), \"hetzner\" (Hetzner Cloud) or \"modal\" (Modal Sandboxes). EC2, Azure, Compute Engine and Hetzner Cloud start one small VM or server per sandbox that runs the image with Docker; Modal runs one Modal sandbox. Hetzner Cloud servers have no /dev/kvm, so Hetzner runs container images only (Windows and other VM images are refused). Either way the guest's cua-spacesd dials out to the cua.ai relay as a computer of the signed-in account (no inbound port), and the Space is that computer, relay:<computer>. AWS and Google Cloud use their own CLI sign-in or, with auth=\"oidc\", no stored key at all (Cua's issuer signs short-lived tokens the account trusts); Azure always links that way; Modal uses its CLI sign-in; Hetzner uses a project API token (Read & Write), and Modal can take its token too: cloud_connect seals it to the account's devices (Cua's servers keep only ciphertext; no tool returns it), or keeps Hetzner's in this device's Cua credential store (auth=\"cli\"). Connect it first with cloud_connect (the user decides; it costs money in their account). Everything Cua creates is tagged cua-managed=true with this cua home's cua-owner and an expiry, recorded under the cua home, deleted with its sandbox, and ends itself at its expiry (a Hetzner server, which cannot delete itself, shuts down and is billed until cloud_sweep or delete_space removes it); cloud_sweep removes leftovers, and the daemon runs that sweep by itself every 15 minutes (CUA_CLOUD_SWEEP_SECS; 0 turns it off). cloud_disconnect refuses while Cua resources still run there unless delete=true; cloud_audit lists what Cua did.";

/// Former tool names still accepted (for one release) and the tool each is
/// now: `(old, new)`. Hidden: never in `tools/list` or the manifest's
/// `tools`; the manifest lists them under `deprecated_tools`.
pub const TOOL_ALIASES: &[(&str, &str)] = &[
    ("drive_ls", "volume_ls"),
    ("drive_read", "volume_read"),
    ("drive_write", "volume_write"),
    ("drive_delete", "volume_delete"),
    ("drive_history", "volume_history"),
    ("drive_restore", "volume_restore"),
    ("drive_grant", "volume_grant"),
    ("drive_revoke", "volume_revoke"),
    ("drive_grants", "volume_grants"),
    ("drive_request_access", "volume_request_access"),
    ("drive_requests", "volume_requests"),
    ("drive_approve", "volume_approve"),
    ("drive_deny", "volume_deny"),
    ("drive_audit", "volume_audit"),
    ("drive_storage", "volume_storage"),
    ("drive_storage_set", "volume_storage_set"),
    ("drive_mount_status", "volume_mount_status"),
    ("drive_mount", "volume_mount"),
    ("drive_unmount", "volume_unmount"),
    ("drive_sync_status", "volume_sync_status"),
    ("drive_sync_events", "volume_sync_events"),
    ("drive_sync_resolve", "volume_sync_resolve"),
    ("drive_cache_stats", "volume_cache_stats"),
    ("drive_cache_set", "volume_cache_set"),
    ("drive_cache_clear", "volume_cache_clear"),
    ("persistent_agent_create", "agent_add"),
    ("persistent_agent_send", "agent_send"),
    ("persistent_agent_remove", "agent_remove"),
    ("routine_add", "agent_schedule_add"),
    ("routine_list", "agent_schedule_list"),
    ("routine_remove", "agent_schedule_remove"),
    ("routine_set_enabled", "agent_schedule_set_enabled"),
];

/// The current name of `name`: itself, or the tool a former name
/// ([`TOOL_ALIASES`]) now is.
pub fn canonical(name: &str) -> &str {
    TOOL_ALIASES
        .iter()
        .find(|(old, _)| *old == name)
        .map(|(_, new)| *new)
        .unwrap_or(name)
}

/// Tools only a person may call: no MCP server lists or runs them for an
/// agent (the apps, the CLI and the SDK call them through the daemon).
/// Their capabilities are never granted to an agent.
pub const PERSON_ONLY_TOOLS: &[&str] = &["cloud_approve"];

/// Whether `name` is a tool only a person may call ([`PERSON_ONLY_TOOLS`]).
pub fn person_only(name: &str) -> bool {
    PERSON_ONLY_TOOLS.contains(&canonical(name))
}

/// The tool named `name` (a former name resolves to its tool).
pub fn tool(name: &str) -> Option<Tool> {
    let name = canonical(name);
    tools().into_iter().find(|tool| tool.name == name)
}

pub fn manifest() -> Manifest {
    let tools = tools();
    Manifest {
        generated_notice: GENERATED_NOTICE,
        experimental: true,
        contract_version: CONTRACT_VERSION,
        capability_version: CAPABILITY_VERSION,
        mcp_protocol_version: MCP_PROTOCOL_VERSION,
        transport: TRANSPORT,
        transports: TRANSPORTS.to_vec(),
        tool_count: tools.len(),
        categories: categories(),
        error_kinds: ERROR_KINDS,
        tools,
        deprecated_tools: deprecated_tools(),
    }
}

/// The manifest as it is written to disk: two-space indented, trailing
/// newline, so a diff in review is a diff of meaning.
pub fn manifest_json() -> String {
    let mut text = serde_json::to_string_pretty(&manifest()).expect("serialize Spaces manifest");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn former_tool_names_resolve_but_are_not_published() {
        for (old, new) in TOOL_ALIASES {
            assert_eq!(tool(old).map(|t| t.name), Some(*new), "{old}");
            assert!(tools().iter().all(|t| t.name != *old), "{old} is listed");
        }
        assert_eq!(canonical("volume_ls"), "volume_ls");
        assert_eq!(canonical("space_list"), "space_list");
        let m = manifest();
        assert!(m.tools.iter().all(|t| !t.name.starts_with("drive_")));
        // Every former name is in the manifest as deprecated, pointing at a
        // tool the manifest advertises.
        assert_eq!(m.deprecated_tools.len(), TOOL_ALIASES.len());
        for d in &m.deprecated_tools {
            assert!(d.deprecated);
            assert!(
                m.tools.iter().any(|t| t.name == d.replaced_by),
                "{}",
                d.name
            );
        }
    }

    /// The agent names (agents in a Space) are the persistent-agent tools
    /// under new names: the former name resolves to the very same tool
    /// (schema, annotations, implementation).
    #[test]
    fn agent_names_alias_the_persistent_agent_tools() {
        for (old, new) in [
            ("persistent_agent_create", "agent_add"),
            ("persistent_agent_send", "agent_send"),
            ("persistent_agent_remove", "agent_remove"),
            ("routine_add", "agent_schedule_add"),
            ("routine_list", "agent_schedule_list"),
            ("routine_remove", "agent_schedule_remove"),
            ("routine_set_enabled", "agent_schedule_set_enabled"),
        ] {
            assert_eq!(canonical(old), new);
            let (a, b) = (tool(old).unwrap(), tool(new).unwrap());
            assert_eq!(a.name, new);
            assert_eq!(a.rust_symbol, b.rust_symbol);
            assert_eq!(a.input_schema, b.input_schema);
            assert!(tools().iter().all(|t| t.name != old), "{old} is listed");
        }
    }

    #[test]
    fn advertises_every_tool() {
        assert_eq!(tools().len(), 114);
    }

    #[test]
    fn person_only_tools_exist_and_are_never_for_agents() {
        for name in PERSON_ONLY_TOOLS {
            let t = tool(name).unwrap_or_else(|| panic!("{name} is not a contract tool"));
            assert!(person_only(name));
            assert!(t.instructions.contains("Only for a person"), "{name}");
            assert!(!t.annotations.read_only, "{name}");
        }
        assert!(!person_only("cloud_connect"));
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<&str> = tools().iter().map(|tool| tool.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    /// Provisioning is the only thing tied to a provider. Everything else,
    /// hotspot included, is gated on what the Space's spacesd reports.
    #[test]
    fn only_provisioning_is_provider_specific() {
        let specific: Vec<&str> = tools()
            .iter()
            .filter(|tool| tool.providers.len() != ALL_PROVIDERS.len())
            .map(|tool| tool.name)
            .collect();
        assert_eq!(specific, vec!["create_space"]);
    }

    /// Providers use the location vocabulary of every other surface.
    #[test]
    fn providers_are_the_location_words() {
        assert_eq!(
            serde_json::to_value(ALL_PROVIDERS).unwrap(),
            serde_json::json!(["cloud", "local", "direct", "relay"])
        );
        // create_space makes local and cloud Spaces, and relay Spaces on
        // your machines (`on="host:<machine>"`).
        let create = tool("create_space").unwrap();
        assert_eq!(
            create.providers,
            vec![Provider::Cloud, Provider::Local, Provider::Relay]
        );
    }

    #[test]
    fn hotspot_is_capability_gated_not_cloud_only() {
        for name in ["hotspot_start", "hotspot_stop", "hotspot_status"] {
            let tool = tool(name).unwrap();
            assert_eq!(tool.requires, vec!["hotspot"], "{name}");
            assert!(tool.providers.contains(&Provider::Local), "{name}");
        }
    }

    /// The stale-comment bug, pinned so it cannot come back.
    #[test]
    fn teleport_app_has_a_local_path() {
        let teleport = tool("teleport_app").expect("teleport_app is advertised");
        assert!(teleport.providers.contains(&Provider::Local));
    }

    #[test]
    fn only_creating_in_the_cloud_is_metered() {
        let metered: Vec<(&str, Metering)> = tools()
            .iter()
            .filter(|tool| tool.metering != Metering::Free)
            .map(|tool| (tool.name, tool.metering))
            .collect();
        // Resuming a paused agent creates its released cloud Space again.
        assert_eq!(
            metered,
            vec![
                ("create_space", Metering::Cloud),
                ("agent_resume", Metering::Cloud)
            ]
        );
    }

    /// One verb set: create/delete make and destroy, add/remove register
    /// and forget; no claim, provision or release in tool names. (A thread
    /// task is claimed, not a Space: `thread_claim` is not a lifecycle verb.)
    #[test]
    fn lifecycle_verbs_are_orthogonal() {
        for tool in tools()
            .into_iter()
            .filter(|t| t.category != Category::Thread)
        {
            for word in ["claim", "provision", "release", "acquire", "get_or_create"] {
                assert!(!tool.name.contains(word), "{} uses {word}", tool.name);
            }
        }
        for name in [
            "create_space",
            "delete_space",
            "stop_space",
            "start_space",
            "resize_space",
            "add_space",
            "remove_space",
            "list_spaces",
        ] {
            assert!(tool(name).is_some(), "{name}");
        }
    }

    #[test]
    fn every_tool_has_an_object_schema_and_a_rust_symbol() {
        for tool in tools() {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            assert!(tool.input_schema.get("$schema").is_none());
            assert!(tool.input_schema.get("$defs").is_none(), "{}", tool.name);
            assert!(!tool.rust_symbol.is_empty(), "{}", tool.name);
        }
    }

    #[test]
    fn space_scoped_tools_require_space() {
        for tool in tools() {
            let props = tool.input_schema["properties"].as_object().unwrap();
            let required: Vec<&str> = tool.input_schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if props.contains_key("space")
                && !matches!(
                    tool.name,
                    "hotspot_stop" | "hotspot_status" | "teleport_manifest"
                    // Optional: an agent is found by its name; `space` names
                    // a Space another machine provides.
                    | "agent_send"
                )
            {
                assert!(
                    required.contains(&"space"),
                    "{} must require space",
                    tool.name
                );
            }
        }
    }

    /// Tools are grouped by category, in category order, and each states
    /// its result and only known error kinds.
    #[test]
    fn tools_are_categorized_and_documented() {
        let order: Vec<Category> = categories().iter().map(|c| c.id).collect();
        let mut last = 0;
        for tool in tools() {
            let at = order.iter().position(|c| *c == tool.category).unwrap();
            assert!(at >= last, "{} is out of category order", tool.name);
            last = at;
            assert!(!tool.result.is_empty(), "{} has no result", tool.name);
            for kind in &tool.errors {
                assert!(
                    ERROR_KINDS.iter().any(|k| k.kind == *kind),
                    "{}: unknown error kind {kind}",
                    tool.name
                );
            }
        }
        assert_eq!(last, order.len() - 1, "a category has no tools");
    }

    #[test]
    fn schemas_carry_documented_defaults_and_enums() {
        let create = tool("create_space").unwrap();
        let kinds: Vec<&str> = create.input_schema["properties"]["kind"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["const"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["auto", "container", "vm", "microvm"]);
        for axis in ["on", "runtime", "reuse"] {
            assert!(
                create.input_schema["properties"][axis]["description"]
                    .as_str()
                    .is_some_and(|d| d.contains("Default")),
                "{axis}"
            );
        }
        let start = tool("agent_start").unwrap();
        assert_eq!(
            start.input_schema["properties"]["agent"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            inputs::AGENT_IDS.len()
        );
        let bash = tool("space_bash").unwrap();
        assert!(
            bash.input_schema["properties"]["timeout"]["description"]
                .as_str()
                .unwrap()
                .contains("Default 60")
        );
    }
}
