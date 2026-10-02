//! The declared schema: every event, every property, and the only values
//! each property may take.
//!
//! This table is the contract. [`validate`] runs on every payload before it
//! is queued, and an event with an undeclared property, or a value outside
//! its property's vocabulary, is dropped whole (and fails tests). The docs
//! table ("Telemetry and privacy") and `cua telemetry show-last` are read
//! from the same data.
//!
//! Property kinds never admit free text: a value is a member of a fixed
//! vocabulary, a boolean, a bounded integer, a strict version string, a
//! random id, a per-install salted hash, or a sample rate.

use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Bumped when the envelope or the meaning of a common property changes.
/// Each event carries its own `event_version` too.
pub const SCHEMA_VERSION: u64 = 1;

/// What a property may hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// One of a fixed vocabulary.
    Enum(&'static [&'static str]),
    /// true or false.
    Bool,
    /// Exactly this boolean (PostHog control flags).
    ConstBool(bool),
    /// An integer in `0..=max`.
    Count { max: u64 },
    /// A strict semver (`1.2.3` or `1.2.3-beta.1`), at most 40 chars.
    Version,
    /// A random per-process id (32 lowercase hex).
    RandomId,
    /// A per-install salted SHA-256 prefix (16 lowercase hex).
    SaltedHash,
    /// A sample rate in `(0, 1]`.
    Rate,
    /// An image from the public sandbox image catalog, `custom` or `none`.
    ImageId,
    /// An app from the public teleport catalog, or `other`.
    TeleportApp,
    /// Members of a fixed vocabulary joined with `+`, in vocabulary order
    /// and without repeats (`cua_volume+sharing`), or `none`.
    Set(&'static [&'static str]),
}

/// One property.
#[derive(Debug, Clone, Copy)]
pub struct Prop {
    /// Name, as sent.
    pub name: &'static str,
    /// What it may hold.
    pub kind: Kind,
    /// What it means (docs and `show-last`).
    pub doc: &'static str,
}

/// Which tier an event belongs to. Only [`Tier::Usage`] exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Anonymous product usage analytics (on by default, off by any switch).
    Usage,
}

/// One event.
#[derive(Debug, Clone, Copy)]
pub struct EventSpec {
    /// Name, as sent.
    pub name: &'static str,
    /// Bumped when this event's properties change meaning.
    pub version: u64,
    /// Tier.
    pub tier: Tier,
    /// Fraction of occurrences sent (the rest are dropped on device).
    pub sample_rate: f64,
    /// Which products may send it.
    pub products: &'static [&'static str],
    /// Its own properties (the common ones are added to every event).
    pub props: &'static [Prop],
    /// Why we collect it: the decision it informs.
    pub purpose: &'static str,
}

// ---------------------------------------------------------------------------
// Vocabularies
// ---------------------------------------------------------------------------

/// Who sends the event.
pub const PRODUCTS: &[&str] = &[
    "cli",
    "daemon",
    "sdk_rust",
    "sdk_python",
    "sdk_typescript",
    "sdk_swift",
    "sdk_kotlin",
    "spaces_app",
    "spacesd",
    "cua_bench",
];
/// SDK surfaces: an app that embeds the SDK.
pub const SDK_PRODUCTS: &[&str] = &[
    "sdk_rust",
    "sdk_python",
    "sdk_typescript",
    "sdk_swift",
    "sdk_kotlin",
];
const ALL: &[&str] = PRODUCTS;
const CLIENTS: &[&str] = &[
    "cli",
    "sdk_rust",
    "sdk_python",
    "sdk_typescript",
    "sdk_swift",
    "sdk_kotlin",
    "spaces_app",
];

pub const OS_FAMILIES: &[&str] = &["macos", "linux", "windows", "other"];
pub const ARCHES: &[&str] = &["arm64", "x86_64", "other"];
pub const OUTCOMES: &[&str] = &["ok", "error", "cancelled"];
/// `CuaError` variants in snake_case, plus `none` and `other`. Never the
/// message.
pub const ERROR_KINDS: &[&str] = &[
    "none",
    "invalid_argument",
    "invalid_placement",
    "not_found",
    "provider_not_configured",
    "unsupported",
    "spacesd_not_available",
    "timeout",
    "fleet",
    "fleet_admission_denied",
    "runtime",
    "env",
    "http",
    "unauthenticated",
    "permission_denied",
    "transport",
    "closed",
    "capability_missing",
    "host_capability_missing",
    "teleport_refused",
    "pool_spec_mismatch",
    "claim_secrets_not_delivered",
    "internal",
    "ambiguous_sandbox",
    "insufficient_disk",
    "requires_cua_app",
    "cancelled",
    "other",
];
pub const DURATIONS: &[&str] = &[
    "lt_100ms",
    "100_999ms",
    "1_4s",
    "5_29s",
    "30_119s",
    "2_9m",
    "gte_10m",
];
pub const UPTIMES: &[&str] = &["lt_1h", "1_5h", "6_23h", "1_6d", "gte_7d"];
pub const COUNTS: &[&str] = &["0", "1", "2_4", "5_9", "10_49", "50_99", "gte_100"];
pub const LOCATIONS: &[&str] = &[
    "local", "cloud", "direct", "relay", "e2b", "daytona", "modal", "other",
];
pub const SANDBOX_KINDS: &[&str] = &["auto", "container", "vm", "unknown"];
pub const RUNTIMES: &[&str] = &[
    "auto", "gvisor", "runc", "qemu", "lume", "kubevirt", "provider", "unknown",
];
pub const GUEST_OS: &[&str] = &["linux", "windows", "macos", "android", "unknown"];
pub const TOPOLOGIES: &[&str] = &["embedded", "daemon", "none"];
pub const INSTALL_CHANNELS: &[&str] = &[
    "install_script",
    "homebrew",
    "pip",
    "npm",
    "cargo",
    "spaces_app",
    "manual",
    "unknown",
];
pub const DAEMON_MODES: &[&str] = &["foreground", "background", "app"];
/// Which SDK entry points were used (feature adoption). Fixed names.
pub const APIS: &[&str] = &[
    "sandbox.connect",
    "sandbox.connect_url",
    "sandbox.viewer_url",
    "sandbox.public_url",
    "sandbox.forward",
    "sandbox.mcp",
    "sandbox.agents",
    "sandbox.spacesd",
    "spaces.create",
    "spaces.connect",
    "media.open",
    "teleport.push",
    "agent_setup.setup",
    "auth.login",
    "local.image_pull",
    "local.image_build",
    "host.setup",
];
pub const TELEPORT_CAPABILITIES: &[&str] = &["full", "install_only", "unsupported", "unknown"];
pub const TELEPORT_MOVES: &[&str] = &["app_only", "app_with_files", "app_with_state", "session"];
/// Who asked for a teleport. With `caller_kind_source=broker_verified` the
/// value comes from the Keyvault broker's OS-verified caller identity
/// (audit token plus code signature); with `self_reported` it is what the
/// sending process knows about itself (no broker was reached).
pub const CALLER_KINDS: &[&str] = &["cua_app_signed", "embedded_sdk", "unsigned", "unknown"];
pub const CALLER_KIND_SOURCES: &[&str] = &["broker_verified", "self_reported"];
pub const TELEPORT_PATHS: &[&str] = &["broker", "sdk"];
pub const TELEPORT_OUTCOMES: &[&str] = &[
    "ok",
    "requires_cua_app",
    "consent_denied",
    "forbidden",
    "locked",
    "disabled",
    "rate_limited",
    "not_found",
    "invalid",
    "error",
    "cancelled",
];
pub const CONSENT_DECISIONS: &[&str] = &["requested", "approved", "denied", "expired"];
pub const DELIVERY_GRANTS: &[&str] = &["present", "absent"];
/// Spaces app features (feature adoption). Fixed names.
pub const FEATURES: &[&str] = &[
    "space_create_local",
    "space_create_cloud",
    "space_open_viewer",
    "space_delete",
    "space_switch",
    "teleport_app_picker",
    "teleport_drop",
    "window_drag",
    "file_send",
    "hotspot",
    "agent_run",
    "host_setup",
    "keyvault_open",
    "relay_machine_open",
    "settings_open",
    "sign_in",
    "update_install",
    "volume_open_finder",
    "agents_page_open",
    "share_open",
    // Settings, General: Launch Cua Spaces at login turned on or off.
    "launch_at_login_on",
    "launch_at_login_off",
];
pub const ONBOARDING_STEPS: &[&str] = &[
    "installer_started",
    "installer_completed",
    "app_launched",
    "onboarding_shown",
    "onboarding_completed",
    "onboarding_skipped",
    "signed_in",
    "agents_setup_completed",
    "first_sandbox_created",
    "first_space_created",
    "first_teleport",
    "first_stream",
    "first_space_ready",
    "first_agent_run",
];
/// The Spaces app's first-run pages (`cua-spaces-app-core` `onboarding`).
pub const ONBOARDING_PAGES: &[&str] = &[
    "welcome",
    "signin",
    "agents",
    "presentation",
    "volume",
    "this_machine",
    "done",
];
/// What happened on a first-run page.
pub const PAGE_ACTIONS: &[&str] = &["shown", "completed", "skipped", "back"];
/// The answer a first-run page was left with (fixed words; `none` for a
/// page without a choice).
pub const PAGE_CHOICES: &[&str] = &[
    "none",
    "signed_in",
    "not_signed_in",
    "agents_set_up",
    "no_agents",
    "notch_and_menu_bar",
    "menu_bar_only",
    "this_mac",
    "s3",
    "later",
    "access_others",
    "host",
];
/// Where a Space create was when it failed (`cua_vmm::progress::Phase`
/// words, the app core's `creating::PHASES`), `none` when it succeeded,
/// `other` when no known phase was reported.
pub const CREATE_PHASES: &[&str] = &[
    "none",
    "preparing",
    "pulling",
    "creating",
    "booting",
    "waiting_for_services",
    "connecting",
    "other",
];
/// Time from create to ready (or to the failure), bucketed finely enough
/// for median and p90 estimates.
pub const CREATE_TIMES: &[&str] = &[
    "lt_5s", "5_9s", "10_19s", "20_39s", "40_59s", "60_119s", "2_4m", "5_9m", "10_29m", "gte_30m",
];
/// The New Space panel: opened, closed without creating, or submitted.
pub const WIZARD_ACTIONS: &[&str] = &["opened", "cancelled", "submitted"];
/// Where a Cua Volume setting was changed.
pub const VOLUME_SURFACES: &[&str] = &["onboarding", "settings"];
/// Where the Cua Volume's files live.
pub const VOLUME_STORAGE: &[&str] = &["this_mac", "s3", "later"];
/// How the Cua Volume shows up on this machine (`volume_mount_status`).
pub const MOUNT_METHODS: &[&str] = &["fskit", "nfs", "fuse", "none", "unknown"];
/// Persistent agent actions (the daemon's persistent-agent tools).
pub const PERSISTENT_ACTIONS: &[&str] = &[
    "create",
    "remove",
    "pause",
    "resume",
    "send",
    "routine_add",
    "routine_toggle",
    "routine_remove",
    "computer_allow",
    "computer_revoke",
];
/// Host setup: relay or a direct address.
pub const HOST_MODES: &[&str] = &["relay", "direct", "unknown"];
/// What a host machine is for: share its desktop, or only run Spaces for
/// the account's other devices.
pub const HOST_PROFILES: &[&str] = &["desktop", "spare", "both", "unknown"];
/// Keyvault actions (the Keyvault broker in `cua daemon`).
pub const KEYVAULT_ACTIONS: &[&str] = &["setup", "unlock", "lock", "import", "site_login"];
/// How the Keyvault was set up or unlocked.
pub const KEYVAULT_METHODS: &[&str] = &["os_key_store", "passphrase", "recovery_key", "none"];
/// Sharing a Space.
pub const SHARE_ACTIONS: &[&str] = &["share", "unshare", "change_role"];
/// The role a Space is shared with.
pub const SHARE_ROLES: &[&str] = &["viewer", "editor", "none"];
/// Updater steps.
pub const UPDATE_ACTIONS: &[&str] = &["checked", "found", "not_found", "installed", "failed"];
/// Update channels (`cua-spaces-app-core` `about::UpdateChannel`).
pub const UPDATE_CHANNELS: &[&str] = &["stable", "beta"];
/// Who started an update check.
pub const UPDATE_TRIGGERS: &[&str] = &["user", "background"];
/// How a device enrolled with the relay.
pub const ENROLL_METHODS: &[&str] = &["sign_in", "approval", "rekey"];
/// The Spaces app's experiments (Settings, Experiments;
/// `cua-spaces-app-core` `experiments::Experiment::id`).
pub const EXPERIMENTS: &[&str] = &["cua_volume", "your_cloud", "sharing"];
/// An experiment's switch turned on or off.
pub const EXPERIMENT_ACTIONS: &[&str] = &["experiment_on", "experiment_off"];
pub const STREAM_TRANSPORTS: &[&str] = &["quic", "webrtc", "websocket", "grpc", "other"];
pub const CODECS: &[&str] = &["h264", "hevc", "av1", "vp8", "vp9", "jpeg", "png", "other"];
pub const DECODERS: &[&str] = &["hw", "sw", "none"];
pub const FPS_BUCKETS: &[&str] = &["lt_10", "10_23", "24_29", "30_49", "50_59", "gte_60"];
pub const LATENCY_BUCKETS: &[&str] = &[
    "lt_16ms",
    "16_32ms",
    "33_65ms",
    "66_132ms",
    "133_265ms",
    "gte_266ms",
    "unknown",
];
pub const RESOLUTIONS: &[&str] = &["lt_720p", "720p", "1080p", "1440p", "gte_4k", "unknown"];
/// cua-agents harness ids (`cua_agents::harness::HARNESSES`) or `other`.
pub const HARNESSES: &[&str] = &[
    "claude-code",
    "openai-codex",
    "gemini-cli",
    "google-antigravity",
    "opencode",
    "goose",
    "pi",
    "hermes",
    "openclaw",
    "other",
];
/// cua-bench dataset names (`cua_bench/registry.json`) or `custom`.
pub const TASKSETS: &[&str] = &[
    "cua-bench-basic",
    "cua-bench-kicad",
    "cua-bench-workflows",
    "custom",
];
pub const SCORES: &[&str] = &["0", "1_24", "25_49", "50_74", "75_99", "100", "none"];
/// `cua` commands, as `group.sub` from the clap tree. A cua-cli test walks
/// the real command tree and fails if one is missing here.
pub const CLI_COMMANDS: &[&str] = include!("cli_commands.in");

// ---------------------------------------------------------------------------
// Common properties
// ---------------------------------------------------------------------------

/// Added to every event.
pub const COMMON: &[Prop] = &[
    p(
        "telemetry_schema_version",
        Kind::Count { max: 1000 },
        "Envelope version.",
    ),
    p(
        "event_version",
        Kind::Count { max: 1000 },
        "This event's version.",
    ),
    p(
        "product",
        Kind::Enum(PRODUCTS),
        "Which Cua program sent it.",
    ),
    p("product_version", Kind::Version, "Its version."),
    p("os_family", Kind::Enum(OS_FAMILIES), "Host OS family."),
    p(
        "os_major",
        Kind::Count { max: 999 },
        "Host OS major version.",
    ),
    p("arch", Kind::Enum(ARCHES), "CPU architecture."),
    p("is_ci", Kind::Bool, "Sent from a CI environment."),
    p(
        "is_synthetic",
        Kind::Bool,
        "Sent by Cua's own tests (CUA_TELEMETRY_SYNTHETIC).",
    ),
    p(
        "process_session_id",
        Kind::RandomId,
        "Random per process, never stored.",
    ),
    p(
        "sample_rate",
        Kind::Rate,
        "Fraction of these events that are sent.",
    ),
    p(
        "$process_person_profile",
        Kind::ConstBool(false),
        "No person profile is created.",
    ),
    p(
        "$geoip_disable",
        Kind::ConstBool(true),
        "No location lookup from the IP.",
    ),
    p("$lib", Kind::Enum(&["cua-telemetry"]), "Sender library."),
    p("$lib_version", Kind::Version, "Sender library version."),
];

const fn p(name: &'static str, kind: Kind, doc: &'static str) -> Prop {
    Prop { name, kind, doc }
}

const OUTCOME: Prop = p("outcome", Kind::Enum(OUTCOMES), "ok, error or cancelled.");
const ERROR_KIND: Prop = p(
    "error_kind",
    Kind::Enum(ERROR_KINDS),
    "Typed error category (the error enum variant); never the message.",
);
const DURATION: Prop = p(
    "duration_bucket",
    Kind::Enum(DURATIONS),
    "Duration, bucketed.",
);
const LOCATION: Prop = p("location", Kind::Enum(LOCATIONS), "Where the sandbox runs.");
const CALLER_KIND: Prop = p(
    "caller_kind",
    Kind::Enum(CALLER_KINDS),
    "Who asked for the teleport: the signed Cua app, an app embedding the SDK, an unsigned program, or unknown.",
);
const CALLER_KIND_SOURCE: Prop = p(
    "caller_kind_source",
    Kind::Enum(CALLER_KIND_SOURCES),
    "broker_verified: from the Keyvault broker's OS-verified caller identity; self_reported: the sending process's own view.",
);
const TELEPORT_APP: Prop = p(
    "app",
    Kind::TeleportApp,
    "Teleport catalog id of the app (public catalog), else other.",
);
const TELEPORT_CAPABILITY: Prop = p(
    "capability",
    Kind::Enum(TELEPORT_CAPABILITIES),
    "What teleport can do with the app.",
);
const TELEPORT_MOVE: Prop = p("move", Kind::Enum(TELEPORT_MOVES), "What moves.");
const TELEPORT_PATH: Prop = p(
    "path",
    Kind::Enum(TELEPORT_PATHS),
    "broker: recorded by the Keyvault broker; sdk: recorded by the calling SDK.",
);

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

pub mod event {
    pub const FIRST_RUN: &str = "cua_first_run";
    pub const CLI_COMMAND: &str = "cua_cli_command";
    pub const SANDBOX_CREATED: &str = "cua_sandbox_created";
    pub const SANDBOX_DELETED: &str = "cua_sandbox_deleted";
    pub const API_USED: &str = "cua_api_used";
    pub const TELEPORT_ATTEMPTED: &str = "cua_teleport_attempted";
    pub const TELEPORT_COMPLETED: &str = "cua_teleport_completed";
    pub const KEYVAULT_CONSENT: &str = "cua_keyvault_consent";
    pub const SPACES_FEATURE_USED: &str = "cua_spaces_feature_used";
    pub const STREAM_STATS: &str = "cua_stream_stats";
    pub const ONBOARDING_STEP: &str = "cua_onboarding_step";
    pub const AGENT_RUN_COMPLETED: &str = "cua_agent_run_completed";
    pub const BENCH_RUN_COMPLETED: &str = "cua_bench_run_completed";
    pub const DAEMON_STARTED: &str = "cua_daemon_started";
    pub const DAEMON_HEALTH: &str = "cua_daemon_health";
    pub const SPACESD_HEALTH: &str = "cua_spacesd_health";
    pub const SPACESD_SESSION_DELIVERIES: &str = "cua_spacesd_session_deliveries";
    pub const APP_ACTIVE: &str = "cua_app_active";
    pub const ONBOARDING_PAGE: &str = "cua_onboarding_page";
    pub const SPACE_WIZARD: &str = "cua_space_wizard";
    pub const SPACE_CREATE: &str = "cua_space_create";
    pub const SPACE_CREATE_STARTED: &str = "cua_space_create_started";
    pub const VOLUME_SETUP: &str = "cua_volume_setup";
    pub const PERSISTENT_AGENT: &str = "cua_persistent_agent";
    pub const HOST_SETUP: &str = "cua_host_setup";
    pub const HOST_SPACE_PROVIDED: &str = "cua_host_space_provided";
    pub const KEYVAULT_ACTION: &str = "cua_keyvault_action";
    pub const SHARE: &str = "cua_share";
    pub const PRESENCE_SESSION: &str = "cua_presence_session";
    pub const APP_UPDATE: &str = "cua_app_update";
    pub const DEVICE_ENROLL: &str = "cua_device_enroll";
    pub const EXPERIMENT: &str = "cua_experiment";
}

/// Every event.
pub const EVENTS: &[EventSpec] = &[
    EventSpec {
        name: event::FIRST_RUN,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: ALL,
        props: &[p(
            "install_channel",
            Kind::Enum(INSTALL_CHANNELS),
            "How Cua was installed.",
        )],
        purpose: "Activation funnel: installs by channel.",
    },
    EventSpec {
        name: event::CLI_COMMAND,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["cli"],
        props: &[
            p(
                "command",
                Kind::Enum(CLI_COMMANDS),
                "The command (group.sub), never its arguments.",
            ),
            OUTCOME,
            ERROR_KIND,
            DURATION,
            p("json_output", Kind::Bool, "--json was given."),
            p(
                "topology",
                Kind::Enum(TOPOLOGIES),
                "Embedded runtime or cua daemon.",
            ),
        ],
        purpose: "Feature adoption and reliability per command.",
    },
    EventSpec {
        name: event::SANDBOX_CREATED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[
            LOCATION,
            p("kind", Kind::Enum(SANDBOX_KINDS), "container or vm."),
            p("runtime", Kind::Enum(RUNTIMES), "The engine."),
            p(
                "image",
                Kind::ImageId,
                "Catalog image id; any other reference is sent as custom.",
            ),
            p("guest_os", Kind::Enum(GUEST_OS), "Guest OS family."),
            OUTCOME,
            ERROR_KIND,
            DURATION,
            p("with_overlays", Kind::Bool, "Binaries were injected."),
            p("with_build", Kind::Bool, "Image layers were built."),
        ],
        purpose: "Local vs cloud usage (the OSS-to-paid path), reliability by runtime, image adoption, time to sandbox.",
    },
    EventSpec {
        name: event::SANDBOX_DELETED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[LOCATION, OUTCOME, ERROR_KIND],
        purpose: "Lifecycle completion and cleanup reliability.",
    },
    EventSpec {
        name: event::API_USED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 0.25,
        products: CLIENTS,
        props: &[
            p("api", Kind::Enum(APIS), "The SDK entry point."),
            OUTCOME,
            ERROR_KIND,
            DURATION,
        ],
        purpose: "SDK feature adoption and error rates per entry point.",
    },
    EventSpec {
        name: event::TELEPORT_ATTEMPTED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &[
            "cli",
            "daemon",
            "sdk_rust",
            "sdk_python",
            "sdk_typescript",
            "sdk_swift",
            "sdk_kotlin",
            "spaces_app",
        ],
        props: &[
            TELEPORT_APP,
            TELEPORT_CAPABILITY,
            TELEPORT_MOVE,
            CALLER_KIND,
            CALLER_KIND_SOURCE,
            TELEPORT_PATH,
        ],
        purpose: "Teleport demand per app and how it is invoked (signed Cua app vs embedded or unsigned SDK callers).",
    },
    EventSpec {
        name: event::TELEPORT_COMPLETED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &[
            "cli",
            "daemon",
            "sdk_rust",
            "sdk_python",
            "sdk_typescript",
            "sdk_swift",
            "sdk_kotlin",
            "spaces_app",
        ],
        props: &[
            TELEPORT_APP,
            TELEPORT_CAPABILITY,
            TELEPORT_MOVE,
            CALLER_KIND,
            CALLER_KIND_SOURCE,
            TELEPORT_PATH,
            p(
                "outcome",
                Kind::Enum(TELEPORT_OUTCOMES),
                "Typed outcome, including requires_cua_app (refused because the Cua app is needed).",
            ),
            DURATION,
            p("item_count", Kind::Enum(COUNTS), "Items moved, bucketed."),
        ],
        purpose: "Teleport success rate per app and caller kind; how often RequiresCuaApp refuses.",
    },
    EventSpec {
        name: event::KEYVAULT_CONSENT,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon", "spaces_app", "cli"],
        props: &[p(
            "decision",
            Kind::Enum(CONSENT_DECISIONS),
            "A consent request was made, approved, denied or expired. Nothing else about it.",
        )],
        purpose: "Consent UX health: approval vs denial counts only.",
    },
    EventSpec {
        name: event::SPACES_FEATURE_USED,
        version: 2,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[p(
            "feature",
            Kind::Enum(FEATURES),
            "The Spaces app feature.",
        )],
        purpose: "Spaces app feature adoption.",
    },
    EventSpec {
        name: event::STREAM_STATS,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 0.5,
        products: CLIENTS,
        props: &[
            p(
                "transport",
                Kind::Enum(STREAM_TRANSPORTS),
                "Media transport.",
            ),
            p("codec", Kind::Enum(CODECS), "Video codec."),
            p(
                "decode",
                Kind::Enum(DECODERS),
                "Hardware or software decode.",
            ),
            p(
                "fps_bucket",
                Kind::Enum(FPS_BUCKETS),
                "Average presented fps, bucketed.",
            ),
            p(
                "latency_bucket",
                Kind::Enum(LATENCY_BUCKETS),
                "p95 input-to-present or control RTT, bucketed.",
            ),
            p(
                "resolution",
                Kind::Enum(RESOLUTIONS),
                "Stream resolution class.",
            ),
            DURATION,
        ],
        purpose: "Streaming performance by codec and decoder.",
    },
    EventSpec {
        name: event::ONBOARDING_STEP,
        version: 2,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &[
            "cli",
            "spaces_app",
            "sdk_rust",
            "sdk_python",
            "sdk_typescript",
            "sdk_swift",
            "sdk_kotlin",
        ],
        props: &[
            p(
                "step",
                Kind::Enum(ONBOARDING_STEPS),
                "The funnel step (first_* steps fire once per install).",
            ),
            OUTCOME,
        ],
        purpose: "Install and onboarding funnel: where people drop off.",
    },
    EventSpec {
        name: event::AGENT_RUN_COMPLETED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[
            p("harness", Kind::Enum(HARNESSES), "cua-agents harness id."),
            LOCATION,
            OUTCOME,
            ERROR_KIND,
            DURATION,
        ],
        purpose: "Which coding-agent harnesses run in sandboxes and how reliably.",
    },
    EventSpec {
        name: event::BENCH_RUN_COMPLETED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["cua_bench", "sdk_python", "cli"],
        props: &[
            p(
                "taskset",
                Kind::Enum(TASKSETS),
                "cua-bench dataset name; any other task set is custom.",
            ),
            p(
                "score_bucket",
                Kind::Enum(SCORES),
                "Aggregate success rate (percent), bucketed.",
            ),
            p("task_count", Kind::Enum(COUNTS), "Tasks run, bucketed."),
            OUTCOME,
        ],
        purpose: "Which benchmarks are run and coarse aggregate scores.",
    },
    EventSpec {
        name: event::DAEMON_STARTED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon"],
        props: &[
            p("mode", Kind::Enum(DAEMON_MODES), "How the daemon runs."),
            OUTCOME,
        ],
        purpose: "Daemon adoption and startup reliability.",
    },
    EventSpec {
        name: event::DAEMON_HEALTH,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon"],
        props: &[
            p(
                "uptime_bucket",
                Kind::Enum(UPTIMES),
                "Daemon uptime, bucketed.",
            ),
            p(
                "sandboxes",
                Kind::Enum(COUNTS),
                "Sandboxes it knows, bucketed.",
            ),
        ],
        purpose: "Aggregate daemon health (hourly at most).",
    },
    EventSpec {
        name: event::SPACESD_HEALTH,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spacesd"],
        props: &[
            p(
                "uptime_bucket",
                Kind::Enum(UPTIMES),
                "cua-spacesd uptime, bucketed.",
            ),
            p(
                "imports_failed",
                Kind::Enum(COUNTS),
                "Failed session imports in the window, bucketed.",
            ),
        ],
        purpose: "Aggregate in-sandbox health. Sent only when the host enabled telemetry for that sandbox.",
    },
    EventSpec {
        name: event::SPACESD_SESSION_DELIVERIES,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spacesd"],
        props: &[
            p(
                "delivery_grant",
                Kind::Enum(DELIVERY_GRANTS),
                "Whether session deliveries arrived with a Keyvault broker-issued grant.",
            ),
            p(
                "count",
                Kind::Count { max: 100_000 },
                "How many in the window.",
            ),
        ],
        purpose: "Receive-side count of session deliveries with and without a broker grant. Reliable even when a modified client strips its own telemetry. Sent only when the host enabled telemetry for that sandbox.",
    },
    EventSpec {
        name: event::APP_ACTIVE,
        version: 2,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[p(
            "experiments_on",
            Kind::Set(EXPERIMENTS),
            "The Spaces app's experiments turned on in Settings, Experiments on this install (cua_volume, your_cloud, sharing, joined with +), or none.",
        )],
        purpose: "Retention (day 1, day 7, day 30) and daily and weekly active installs: at most one per install per UTC day, on a day the product is used. Also how many installs use each experiment.",
    },
    EventSpec {
        name: event::ONBOARDING_PAGE,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            p("page", Kind::Enum(ONBOARDING_PAGES), "The first-run page."),
            p(
                "action",
                Kind::Enum(PAGE_ACTIONS),
                "shown, completed (Continue with an answer), skipped (Continue or Skip without one) or back.",
            ),
            p(
                "choice",
                Kind::Enum(PAGE_CHOICES),
                "The page's answer as a fixed word (signed_in, menu_bar_only, s3, host, ...); none for pages without one.",
            ),
        ],
        purpose: "Onboarding funnel: which page people leave on, and what they pick on each.",
    },
    EventSpec {
        name: event::SPACE_WIZARD,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[p(
            "action",
            Kind::Enum(WIZARD_ACTIONS),
            "The New Space panel was opened, closed without creating, or submitted.",
        )],
        purpose: "How often a New Space is started and abandoned.",
    },
    EventSpec {
        name: event::SPACE_CREATE_STARTED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            LOCATION,
            p("guest_os", Kind::Enum(GUEST_OS), "Guest OS family."),
            p("kind", Kind::Enum(SANDBOX_KINDS), "container or vm."),
            p("gpu", Kind::Bool, "GPU acceleration was turned on."),
        ],
        purpose: "Which Spaces are started, where, and how often with GPU acceleration.",
    },
    EventSpec {
        name: event::SPACE_CREATE,
        version: 2,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            LOCATION,
            p("guest_os", Kind::Enum(GUEST_OS), "Guest OS family."),
            p("kind", Kind::Enum(SANDBOX_KINDS), "container or vm."),
            OUTCOME,
            p(
                "failed_phase",
                Kind::Enum(CREATE_PHASES),
                "The create phase it failed or was cancelled in (pulling, booting, ...); none when it succeeded.",
            ),
            p(
                "stalled",
                Kind::Bool,
                "It failed because a phase stopped moving (the app's stall timeout).",
            ),
            p(
                "time_bucket",
                Kind::Enum(CREATE_TIMES),
                "Time from pressing Create to ready, to the failure, or to the cancel finishing, bucketed.",
            ),
            p("gpu", Kind::Bool, "GPU acceleration was turned on."),
        ],
        purpose: "Activation and reliability: Space create success, failure and cancel rates by kind, location and GPU, time to ready or to cancel, which phase fails or is cancelled, stall timeouts.",
    },
    EventSpec {
        name: event::VOLUME_SETUP,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            p(
                "surface",
                Kind::Enum(VOLUME_SURFACES),
                "The first run's Cua Volume page or Settings, Storage.",
            ),
            p(
                "storage",
                Kind::Enum(VOLUME_STORAGE),
                "Where the files live: this machine, an S3-compatible bucket, or decide later.",
            ),
            p(
                "add_to_finder",
                Kind::Bool,
                "The volume was added to Finder (mounted on Linux).",
            ),
            p(
                "mount_method",
                Kind::Enum(MOUNT_METHODS),
                "How it mounts here (fskit, nfs, fuse), none or unknown.",
            ),
            OUTCOME,
        ],
        purpose: "Cua Volume adoption: storage choice, Finder volume, mount method.",
    },
    EventSpec {
        name: event::PERSISTENT_AGENT,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon"],
        props: &[
            p(
                "action",
                Kind::Enum(PERSISTENT_ACTIONS),
                "What was done to a persistent agent or its routines.",
            ),
            p(
                "harness",
                Kind::Enum(HARNESSES),
                "cua-agents harness id of the agent it created; other otherwise.",
            ),
            OUTCOME,
            ERROR_KIND,
        ],
        purpose: "Persistent agents adoption: create, pause and resume, routines.",
    },
    EventSpec {
        name: event::HOST_SETUP,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[
            p("mode", Kind::Enum(HOST_MODES), "Relay or a direct address."),
            p(
                "profile",
                Kind::Enum(HOST_PROFILES),
                "Share this desktop, only run Spaces for other devices, or both.",
            ),
            OUTCOME,
            ERROR_KIND,
        ],
        purpose: "Host Spaces adoption: machines set up for access, and by which profile.",
    },
    EventSpec {
        name: event::HOST_SPACE_PROVIDED,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon"],
        props: &[
            p("kind", Kind::Enum(SANDBOX_KINDS), "container or vm."),
            p("guest_os", Kind::Enum(GUEST_OS), "Guest OS family."),
            OUTCOME,
            ERROR_KIND,
        ],
        purpose: "Host Spaces use: Spaces a host machine created for the account's other devices.",
    },
    EventSpec {
        name: event::KEYVAULT_ACTION,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["daemon"],
        props: &[
            p(
                "action",
                Kind::Enum(KEYVAULT_ACTIONS),
                "Set up, unlock, lock, import, or a site sign-in used in a Space.",
            ),
            p(
                "method",
                Kind::Enum(KEYVAULT_METHODS),
                "The OS key store (Touch ID on a Mac), a passphrase or the recovery key; none when not asked.",
            ),
            OUTCOME,
        ],
        purpose: "Keyvault adoption: set up, how it is unlocked, and site sign-ins used.",
    },
    EventSpec {
        name: event::SHARE,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            p(
                "action",
                Kind::Enum(SHARE_ACTIONS),
                "Share, unshare or change a role.",
            ),
            p(
                "role",
                Kind::Enum(SHARE_ROLES),
                "viewer (view-only) or editor; none for unshare.",
            ),
            OUTCOME,
        ],
        purpose: "Sharing adoption, and how often Spaces are shared view-only.",
    },
    EventSpec {
        name: event::PRESENCE_SESSION,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[
            OUTCOME,
            p(
                "participants",
                Kind::Enum(COUNTS),
                "Most people in the Space at once, bucketed.",
            ),
            DURATION,
        ],
        purpose: "Multiplayer: presence sessions, their size and length.",
    },
    EventSpec {
        name: event::APP_UPDATE,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            p(
                "action",
                Kind::Enum(UPDATE_ACTIONS),
                "A check, an update found or not, installed, or failed.",
            ),
            p(
                "channel",
                Kind::Enum(UPDATE_CHANNELS),
                "The update channel.",
            ),
            p(
                "trigger",
                Kind::Enum(UPDATE_TRIGGERS),
                "Check Now or the scheduled check.",
            ),
        ],
        purpose: "Do people update: checks, installs and failures by channel.",
    },
    EventSpec {
        name: event::DEVICE_ENROLL,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: CLIENTS,
        props: &[
            p(
                "method",
                Kind::Enum(ENROLL_METHODS),
                "A fresh sign-in, an approval from an enrolled device, or a same-machine re-key.",
            ),
            OUTCOME,
        ],
        purpose: "Device enrollment: how devices join the relay and how often it fails.",
    },
    EventSpec {
        name: event::EXPERIMENT,
        version: 1,
        tier: Tier::Usage,
        sample_rate: 1.0,
        products: &["spaces_app"],
        props: &[
            p(
                "action",
                Kind::Enum(EXPERIMENT_ACTIONS),
                "The switch turned on or off.",
            ),
            p(
                "experiment",
                Kind::Enum(EXPERIMENTS),
                "Which experiment: cua_volume, your_cloud or sharing.",
            ),
        ],
        purpose: "Which experiments people try, and which they turn off again.",
    },
];

/// The spec of `name`.
pub fn spec(name: &str) -> Option<&'static EventSpec> {
    EVENTS.iter().find(|e| e.name == name)
}

// ---------------------------------------------------------------------------
// Catalogs
// ---------------------------------------------------------------------------

const IMAGE_CATALOG_JSON: &str = include_str!("../../../../images/sandbox-images.json");
const TELEPORT_TARGETS_JSON: &str = include_str!("../../cua-teleport/src/ux/targets.json");
/// Teleport provider ids (`cua_teleport` providers); a cua-teleport test
/// checks the list against the registry.
pub const TELEPORT_PROVIDERS: &[&str] = &[
    "chrome",
    "firefox",
    "slack",
    "discord",
    "claude-code",
    "whatsapp",
    "steam",
];

/// Catalog image ids (`linux:24.04`, `bench-web:1.0-disk`, ...): the public
/// refs of `libs/images/sandbox-images.json` without the registry prefix.
pub fn image_ids() -> &'static BTreeSet<String> {
    static IDS: OnceLock<BTreeSet<String>> = OnceLock::new();
    IDS.get_or_init(|| {
        let v: Value = serde_json::from_str(IMAGE_CATALOG_JSON).expect("image catalog is JSON");
        v["images"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["ref"].as_str())
            .map(short_image)
            .collect()
    })
}

fn short_image(r: &str) -> String {
    r.trim_start_matches("ghcr.io/trycua/").to_string()
}

/// Teleport catalog ids: the public targets and provider ids.
pub fn teleport_app_ids() -> &'static BTreeSet<String> {
    static IDS: OnceLock<BTreeSet<String>> = OnceLock::new();
    IDS.get_or_init(|| {
        let v: Value =
            serde_json::from_str(TELEPORT_TARGETS_JSON).expect("teleport targets are JSON");
        let mut ids: BTreeSet<String> = v["targets"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .collect();
        ids.extend(TELEPORT_PROVIDERS.iter().map(|s| s.to_string()));
        ids
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Why a payload was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// Not a declared event.
    UnknownEvent(String),
    /// This product may not send this event.
    WrongProduct(String),
    /// A property that is not declared for this event.
    UndeclaredProperty(String),
    /// A declared property is missing.
    MissingProperty(String),
    /// A value outside the property's vocabulary.
    BadValue(String),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEvent(e) => write!(f, "unknown event {e}"),
            Self::WrongProduct(e) => write!(f, "product may not send {e}"),
            Self::UndeclaredProperty(p) => write!(f, "undeclared property {p}"),
            Self::MissingProperty(p) => write!(f, "missing property {p}"),
            Self::BadValue(p) => write!(f, "value of {p} is outside its vocabulary"),
        }
    }
}

/// Whether `value` is allowed for `kind`.
pub fn value_ok(kind: Kind, value: &Value) -> bool {
    match kind {
        Kind::Enum(vocab) => value.as_str().is_some_and(|s| vocab.contains(&s)),
        Kind::Bool => value.is_boolean(),
        Kind::ConstBool(b) => value.as_bool() == Some(b),
        Kind::Count { max } => value.as_u64().is_some_and(|n| n <= max),
        Kind::Version => value.as_str().is_some_and(is_strict_version),
        Kind::RandomId => value.as_str().is_some_and(|s| is_hex(s, 32)),
        Kind::SaltedHash => value.as_str().is_some_and(|s| is_hex(s, 16)),
        Kind::Rate => value.as_f64().is_some_and(|r| r > 0.0 && r <= 1.0),
        Kind::ImageId => value
            .as_str()
            .is_some_and(|s| s == "custom" || s == "none" || image_ids().contains(s)),
        Kind::TeleportApp => value
            .as_str()
            .is_some_and(|s| s == "other" || teleport_app_ids().contains(s)),
        Kind::Set(vocab) => value.as_str().is_some_and(|s| set_ok(vocab, s)),
    }
}

/// `none`, or members of `vocab` joined with `+` in vocabulary order, each
/// once.
fn set_ok(vocab: &[&str], s: &str) -> bool {
    if s == "none" {
        return true;
    }
    let mut last: Option<usize> = None;
    for word in s.split('+') {
        let Some(i) = vocab.iter().position(|v| *v == word) else {
            return false;
        };
        if last.is_some_and(|l| i <= l) {
            return false;
        }
        last = Some(i);
    }
    last.is_some()
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `1.2.3` or `1.2.3-pre.1`; digits capped; no build metadata.
pub fn is_strict_version(s: &str) -> bool {
    if s.is_empty() || s.len() > 40 {
        return false;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    pre.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 20
            && p.split('.').all(|x| !x.is_empty())
            && p.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'.')
    })
}

/// Checks a complete property map (common plus event properties) for
/// `event`: every property declared, every declared property present, every
/// value in its vocabulary, and the product allowed to send it.
pub fn validate(event: &str, props: &Map<String, Value>) -> Result<(), SchemaError> {
    let spec = spec(event).ok_or_else(|| SchemaError::UnknownEvent(event.into()))?;
    for (k, v) in props {
        let decl = COMMON
            .iter()
            .chain(spec.props.iter())
            .find(|p| p.name == k)
            .ok_or_else(|| SchemaError::UndeclaredProperty(k.clone()))?;
        if !value_ok(decl.kind, v) {
            return Err(SchemaError::BadValue(k.clone()));
        }
    }
    for decl in COMMON.iter().chain(spec.props.iter()) {
        if !props.contains_key(decl.name) {
            return Err(SchemaError::MissingProperty(decl.name.into()));
        }
    }
    let product = props["product"].as_str().unwrap_or_default();
    if !spec.products.contains(&product) {
        return Err(SchemaError::WrongProduct(event.into()));
    }
    Ok(())
}

/// The schema as JSON (docs generation, `cua telemetry schema`).
pub fn to_json() -> Value {
    fn kind_json(k: Kind) -> Value {
        match k {
            Kind::Enum(v) => serde_json::json!({"enum": v}),
            Kind::Bool => serde_json::json!("bool"),
            Kind::ConstBool(b) => serde_json::json!({"const": b}),
            Kind::Count { max } => serde_json::json!({"integer_max": max}),
            Kind::Version => serde_json::json!("version"),
            Kind::RandomId => serde_json::json!("random_id"),
            Kind::SaltedHash => serde_json::json!("salted_hash"),
            Kind::Rate => serde_json::json!("sample_rate"),
            Kind::ImageId => {
                serde_json::json!({"catalog": "sandbox-images", "else": ["custom", "none"]})
            }
            Kind::TeleportApp => serde_json::json!({"catalog": "teleport-apps", "else": ["other"]}),
            Kind::Set(v) => serde_json::json!({"set_of": v, "joined_with": "+", "else": ["none"]}),
        }
    }
    let prop =
        |p: &Prop| serde_json::json!({"name": p.name, "kind": kind_json(p.kind), "doc": p.doc});
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "common": COMMON.iter().map(prop).collect::<Vec<_>>(),
        "events": EVENTS.iter().map(|e| serde_json::json!({
            "name": e.name,
            "version": e.version,
            "sample_rate": e.sample_rate,
            "products": e.products,
            "purpose": e.purpose,
            "properties": e.props.iter().map(prop).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
