//! Typed event builders.
//!
//! Every builder takes enums, booleans, numbers, or strings it immediately
//! classifies into a fixed vocabulary (an image reference becomes its
//! catalog id or `custom`; a command path becomes a known command or
//! `other`). No builder copies caller text into a payload: the PII fuzz
//! test feeds paths, emails, hostnames and titles through each one.

use serde_json::{Map, Value};
use std::time::Duration;

use crate::schema::{self, event};

/// One event: a declared name and its own properties (the common ones are
/// added when it is sent).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Declared event name.
    pub name: &'static str,
    /// Its properties.
    pub props: Map<String, Value>,
}

impl Event {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            props: Map::new(),
        }
    }

    fn s(mut self, k: &str, v: &'static str) -> Self {
        self.props.insert(k.into(), Value::String(v.into()));
        self
    }

    fn b(mut self, k: &str, v: bool) -> Self {
        self.props.insert(k.into(), Value::Bool(v));
        self
    }

    fn n(mut self, k: &str, v: u64) -> Self {
        self.props.insert(k.into(), Value::from(v));
        self
    }

    fn owned(mut self, k: &str, v: String) -> Self {
        self.props.insert(k.into(), Value::String(v));
        self
    }
}

/// Returns the vocabulary entry equal to `value`, else `fallback`.
pub fn pick(vocab: &'static [&'static str], value: &str, fallback: &'static str) -> &'static str {
    let v = value.trim();
    vocab.iter().copied().find(|x| *x == v).unwrap_or(fallback)
}

// ---------------------------------------------------------------------------
// Classifiers
// ---------------------------------------------------------------------------

/// Outcome of an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Cancelled => "cancelled",
        }
    }

    /// `ok`, `cancelled`, anything else `error`.
    pub fn from_word(word: &str) -> Self {
        match word.trim() {
            "ok" => Outcome::Ok,
            "cancelled" => Outcome::Cancelled,
            _ => Outcome::Error,
        }
    }
}

/// A `CuaError` variant name (`InvalidArgument`) or snake name as an
/// error kind; anything else is `other`. Never pass the message.
pub fn error_kind(variant: &str) -> &'static str {
    let snake: String = {
        let mut out = String::new();
        for (i, c) in variant.trim().chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    };
    pick(schema::ERROR_KINDS, &snake, "other")
}

/// Duration bucket.
pub fn duration_bucket(d: Duration) -> &'static str {
    match d.as_millis() {
        0..=99 => "lt_100ms",
        100..=999 => "100_999ms",
        1_000..=4_999 => "1_4s",
        5_000..=29_999 => "5_29s",
        30_000..=119_999 => "30_119s",
        120_000..=599_999 => "2_9m",
        _ => "gte_10m",
    }
}

/// Uptime bucket.
pub fn uptime_bucket(d: Duration) -> &'static str {
    match d.as_secs() / 3600 {
        0 => "lt_1h",
        1..=5 => "1_5h",
        6..=23 => "6_23h",
        24..=167 => "1_6d",
        _ => "gte_7d",
    }
}

/// Count bucket.
pub fn count_bucket(n: u64) -> &'static str {
    match n {
        0 => "0",
        1 => "1",
        2..=4 => "2_4",
        5..=9 => "5_9",
        10..=49 => "10_49",
        50..=99 => "50_99",
        _ => "gte_100",
    }
}

/// Location word (`local`, `cloud`, `direct:...`, a provider) as a
/// location; unknown providers are `other`.
pub fn location(on: &str) -> &'static str {
    let word = on
        .trim()
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    pick(schema::LOCATIONS, &word, "other")
}

/// Sandbox kind.
pub fn sandbox_kind(kind: &str) -> &'static str {
    pick(schema::SANDBOX_KINDS, &kind.to_ascii_lowercase(), "unknown")
}

/// Runtime; a third-party provider's own runtime is `provider`.
pub fn runtime(runtime: &str) -> &'static str {
    let r = runtime.trim().to_ascii_lowercase();
    if r.is_empty() {
        return "auto";
    }
    match pick(schema::RUNTIMES, &r, "") {
        "" => "provider",
        v => v,
    }
}

/// Guest OS family.
pub fn guest_os(os: &str) -> &'static str {
    let o = os.trim().to_ascii_lowercase();
    let o = match o.as_str() {
        "darwin" | "mac" | "osx" => "macos",
        "win" | "win32" | "win64" => "windows",
        other => other,
    };
    pick(schema::GUEST_OS, o, "unknown")
}

/// An image reference as its catalog id (`linux:24.04`), `none` when
/// empty, else `custom`. Aliases (`linux`, `macos:tahoe`) resolve through
/// `canonical` first if the caller has it.
pub fn image_id(reference: &str) -> String {
    let r = reference.trim();
    if r.is_empty() {
        return "none".into();
    }
    // Drop a digest; keep the tag.
    let r = r.split('@').next().unwrap_or(r);
    let short = r
        .trim_start_matches("docker://")
        .trim_start_matches("ghcr.io/trycua/");
    if schema::image_ids().contains(short) {
        short.to_string()
    } else {
        "custom".into()
    }
}

/// A teleport catalog id (`vscode`, `chrome`), else `other`. Host bundle
/// ids and paths of apps outside the public catalog are never sent.
pub fn teleport_app(id: &str) -> String {
    let i = id.trim();
    if schema::teleport_app_ids().contains(i) {
        i.to_string()
    } else {
        "other".into()
    }
}

/// A cua-agents harness id, else `other`.
pub fn harness(id: &str) -> &'static str {
    pick(schema::HARNESSES, id, "other")
}

/// A cua-bench taskset name, else `custom`.
pub fn taskset(name: &str) -> &'static str {
    pick(schema::TASKSETS, name, "custom")
}

/// Success rate in `0.0..=1.0` as a score bucket.
pub fn score_bucket(rate: Option<f64>) -> &'static str {
    let Some(r) = rate.filter(|r| r.is_finite()) else {
        return "none";
    };
    let pct = (r.clamp(0.0, 1.0) * 100.0).round() as u64;
    match pct {
        0 => "0",
        1..=24 => "1_24",
        25..=49 => "25_49",
        50..=74 => "50_74",
        75..=99 => "75_99",
        _ => "100",
    }
}

/// Average fps as a bucket.
pub fn fps_bucket(fps: f64) -> &'static str {
    match fps {
        f if !f.is_finite() || f < 10.0 => "lt_10",
        f if f < 24.0 => "10_23",
        f if f < 30.0 => "24_29",
        f if f < 50.0 => "30_49",
        f if f < 60.0 => "50_59",
        _ => "gte_60",
    }
}

/// Latency in ms as a bucket (`unknown` when not measured).
pub fn latency_bucket(ms: Option<f64>) -> &'static str {
    match ms.filter(|m| m.is_finite() && *m >= 0.0) {
        None => "unknown",
        Some(m) if m < 16.0 => "lt_16ms",
        Some(m) if m < 33.0 => "16_32ms",
        Some(m) if m < 66.0 => "33_65ms",
        Some(m) if m < 133.0 => "66_132ms",
        Some(m) if m < 266.0 => "133_265ms",
        Some(_) => "gte_266ms",
    }
}

/// Stream height in pixels as a resolution class.
pub fn resolution(height: u32) -> &'static str {
    match height {
        0 => "unknown",
        1..=719 => "lt_720p",
        720..=1079 => "720p",
        1080..=1439 => "1080p",
        1440..=2159 => "1440p",
        _ => "gte_4k",
    }
}

// ---------------------------------------------------------------------------
// Caller kind (teleport)
// ---------------------------------------------------------------------------

/// Who asked for a teleport. See [`schema::CALLER_KINDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    /// The Cua app or CLI, verified by its code signature.
    CuaAppSigned,
    /// A third-party app that embeds the cua SDK (signed by someone else).
    EmbeddedSdk,
    /// Ad hoc signed or unsigned code.
    Unsigned,
    /// Not verifiable on this platform.
    Unknown,
}

impl CallerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CallerKind::CuaAppSigned => "cua_app_signed",
            CallerKind::EmbeddedSdk => "embedded_sdk",
            CallerKind::Unsigned => "unsigned",
            CallerKind::Unknown => "unknown",
        }
    }

    /// What a process knows about itself when no broker verified it: SDK
    /// surfaces are `embedded_sdk`, anything else `unknown` (a process
    /// cannot vouch for its own signature here).
    pub fn self_reported(product: &str) -> Self {
        if schema::SDK_PRODUCTS.contains(&product) {
            CallerKind::EmbeddedSdk
        } else {
            CallerKind::Unknown
        }
    }
}

/// How the caller kind was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKindSource {
    BrokerVerified,
    SelfReported,
}

impl CallerKindSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CallerKindSource::BrokerVerified => "broker_verified",
            CallerKindSource::SelfReported => "self_reported",
        }
    }
}

/// Teleport outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleportOutcome {
    Ok,
    RequiresCuaApp,
    ConsentDenied,
    Forbidden,
    Locked,
    Disabled,
    RateLimited,
    NotFound,
    Invalid,
    Error,
    Cancelled,
}

impl TeleportOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            TeleportOutcome::Ok => "ok",
            TeleportOutcome::RequiresCuaApp => "requires_cua_app",
            TeleportOutcome::ConsentDenied => "consent_denied",
            TeleportOutcome::Forbidden => "forbidden",
            TeleportOutcome::Locked => "locked",
            TeleportOutcome::Disabled => "disabled",
            TeleportOutcome::RateLimited => "rate_limited",
            TeleportOutcome::NotFound => "not_found",
            TeleportOutcome::Invalid => "invalid",
            TeleportOutcome::Error => "error",
            TeleportOutcome::Cancelled => "cancelled",
        }
    }
}

/// What a teleport is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeleportInfo {
    /// Catalog id (classified with [`teleport_app`]).
    pub app: String,
    /// `full`, `install_only`, `unsupported` or `unknown`.
    pub capability: &'static str,
    /// `app_only`, `app_with_files`, `app_with_state` or `session`.
    pub move_kind: &'static str,
    pub caller_kind: CallerKind,
    pub caller_kind_source: CallerKindSource,
    /// `broker` or `sdk`.
    pub path: &'static str,
}

impl TeleportInfo {
    /// Classifies raw inputs.
    pub fn new(
        app: &str,
        capability: &str,
        move_kind: &str,
        caller_kind: CallerKind,
        caller_kind_source: CallerKindSource,
        path: &str,
    ) -> Self {
        Self {
            app: teleport_app(app),
            capability: pick(schema::TELEPORT_CAPABILITIES, capability, "unknown"),
            move_kind: pick(schema::TELEPORT_MOVES, move_kind, "session"),
            caller_kind,
            caller_kind_source,
            path: pick(schema::TELEPORT_PATHS, path, "sdk"),
        }
    }

    fn apply(&self, e: Event) -> Event {
        e.owned("app", self.app.clone())
            .s("capability", self.capability)
            .s("move", self.move_kind)
            .s("caller_kind", self.caller_kind.as_str())
            .s("caller_kind_source", self.caller_kind_source.as_str())
            .s("path", self.path)
    }
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// `cua_first_run`.
pub fn first_run(install_channel: &str) -> Event {
    Event::new(event::FIRST_RUN).s(
        "install_channel",
        pick(schema::INSTALL_CHANNELS, install_channel, "unknown"),
    )
}

/// `cua_cli_command`.
pub fn cli_command(
    command: &str,
    outcome: Outcome,
    error_variant: Option<&str>,
    elapsed: Duration,
    json_output: bool,
    topology: &str,
) -> Event {
    Event::new(event::CLI_COMMAND)
        .s("command", pick(schema::CLI_COMMANDS, command, "other"))
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
        .s("duration_bucket", duration_bucket(elapsed))
        .b("json_output", json_output)
        .s("topology", pick(schema::TOPOLOGIES, topology, "none"))
}

/// What a sandbox create was.
#[derive(Debug, Clone, Default)]
pub struct SandboxCreate<'a> {
    /// `on` / location word.
    pub on: &'a str,
    pub kind: &'a str,
    pub runtime: &'a str,
    /// Image reference (classified to a catalog id or `custom`).
    pub image: &'a str,
    pub guest_os: &'a str,
    pub with_overlays: bool,
    pub with_build: bool,
}

/// `cua_sandbox_created`.
pub fn sandbox_created(
    s: &SandboxCreate<'_>,
    outcome: Outcome,
    error_variant: Option<&str>,
    elapsed: Duration,
) -> Event {
    Event::new(event::SANDBOX_CREATED)
        .s("location", location(s.on))
        .s("kind", sandbox_kind(s.kind))
        .s("runtime", runtime(s.runtime))
        .owned("image", image_id(s.image))
        .s("guest_os", guest_os(s.guest_os))
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
        .s("duration_bucket", duration_bucket(elapsed))
        .b("with_overlays", s.with_overlays)
        .b("with_build", s.with_build)
}

/// `cua_sandbox_deleted`.
pub fn sandbox_deleted(on: &str, outcome: Outcome, error_variant: Option<&str>) -> Event {
    Event::new(event::SANDBOX_DELETED)
        .s("location", location(on))
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
}

/// `cua_api_used` (`api` outside the vocabulary is dropped: returns None).
pub fn api_used(
    api: &str,
    outcome: Outcome,
    error_variant: Option<&str>,
    elapsed: Duration,
) -> Option<Event> {
    let api = pick(schema::APIS, api, "");
    (!api.is_empty()).then(|| {
        Event::new(event::API_USED)
            .s("api", api)
            .s("outcome", outcome.as_str())
            .s(
                "error_kind",
                error_variant.map(error_kind).unwrap_or("none"),
            )
            .s("duration_bucket", duration_bucket(elapsed))
    })
}

/// `cua_teleport_attempted`.
pub fn teleport_attempted(info: &TeleportInfo) -> Event {
    info.apply(Event::new(event::TELEPORT_ATTEMPTED))
}

/// `cua_teleport_completed`.
pub fn teleport_completed(
    info: &TeleportInfo,
    outcome: TeleportOutcome,
    elapsed: Duration,
    items: u64,
) -> Event {
    info.apply(Event::new(event::TELEPORT_COMPLETED))
        .s("outcome", outcome.as_str())
        .s("duration_bucket", duration_bucket(elapsed))
        .s("item_count", count_bucket(items))
}

/// A Keyvault consent decision (counts only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    Requested,
    Approved,
    Denied,
    Expired,
}

/// `cua_keyvault_consent`.
pub fn keyvault_consent(d: ConsentDecision) -> Event {
    Event::new(event::KEYVAULT_CONSENT).s(
        "decision",
        match d {
            ConsentDecision::Requested => "requested",
            ConsentDecision::Approved => "approved",
            ConsentDecision::Denied => "denied",
            ConsentDecision::Expired => "expired",
        },
    )
}

/// `cua_spaces_feature_used`; None for a feature outside the vocabulary.
pub fn spaces_feature_used(feature: &str) -> Option<Event> {
    let f = pick(schema::FEATURES, feature, "");
    (!f.is_empty()).then(|| Event::new(event::SPACES_FEATURE_USED).s("feature", f))
}

/// One stream session's aggregate stats.
#[derive(Debug, Clone, Default)]
pub struct StreamStats<'a> {
    pub transport: &'a str,
    pub codec: &'a str,
    /// Hardware decode.
    pub hw_decode: Option<bool>,
    pub avg_fps: f64,
    pub p95_latency_ms: Option<f64>,
    pub height: u32,
    pub duration: Duration,
}

/// `cua_stream_stats`.
pub fn stream_stats(s: &StreamStats<'_>) -> Event {
    Event::new(event::STREAM_STATS)
        .s(
            "transport",
            pick(
                schema::STREAM_TRANSPORTS,
                &s.transport.to_ascii_lowercase(),
                "other",
            ),
        )
        .s(
            "codec",
            pick(schema::CODECS, &s.codec.to_ascii_lowercase(), "other"),
        )
        .s(
            "decode",
            match s.hw_decode {
                Some(true) => "hw",
                Some(false) => "sw",
                None => "none",
            },
        )
        .s("fps_bucket", fps_bucket(s.avg_fps))
        .s("latency_bucket", latency_bucket(s.p95_latency_ms))
        .s("resolution", resolution(s.height))
        .s("duration_bucket", duration_bucket(s.duration))
}

/// `cua_stream_stats` from what a viewer counted over a whole session:
/// frames received over the session's duration give the fps bucket, the
/// largest frame height the resolution bucket, `p95_latency_ms` (input to
/// present, or a control round trip) the latency bucket. None when the stream carried
/// no video (nothing to measure). Every value is bucketed; raw counts,
/// sizes and times never leave this function.
pub fn stream_summary(
    transport: &str,
    codec: &str,
    hw_decode: Option<bool>,
    frames: u64,
    height: u32,
    p95_latency_ms: Option<f64>,
    duration: Duration,
) -> Option<Event> {
    if frames == 0 || duration.is_zero() {
        return None;
    }
    Some(stream_stats(&StreamStats {
        transport,
        codec,
        hw_decode,
        avg_fps: frames as f64 / duration.as_secs_f64(),
        p95_latency_ms,
        height,
        duration,
    }))
}

/// `cua_onboarding_step`; None for a step outside the vocabulary.
pub fn onboarding_step(step: &str, outcome: Outcome) -> Option<Event> {
    let s = pick(schema::ONBOARDING_STEPS, step, "");
    (!s.is_empty()).then(|| {
        Event::new(event::ONBOARDING_STEP)
            .s("step", s)
            .s("outcome", outcome.as_str())
    })
}

/// `cua_agent_run_completed`.
pub fn agent_run_completed(
    harness_id: &str,
    on: &str,
    outcome: Outcome,
    error_variant: Option<&str>,
    elapsed: Duration,
) -> Event {
    Event::new(event::AGENT_RUN_COMPLETED)
        .s("harness", harness(harness_id))
        .s("location", location(on))
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
        .s("duration_bucket", duration_bucket(elapsed))
}

/// `cua_bench_run_completed`.
pub fn bench_run_completed(
    taskset_name: &str,
    success_rate: Option<f64>,
    tasks: u64,
    outcome: Outcome,
) -> Event {
    Event::new(event::BENCH_RUN_COMPLETED)
        .s("taskset", taskset(taskset_name))
        .s("score_bucket", score_bucket(success_rate))
        .s("task_count", count_bucket(tasks))
        .s("outcome", outcome.as_str())
}

/// `cua_daemon_started`.
pub fn daemon_started(mode: &str, outcome: Outcome) -> Event {
    Event::new(event::DAEMON_STARTED)
        .s("mode", pick(schema::DAEMON_MODES, mode, "background"))
        .s("outcome", outcome.as_str())
}

/// `cua_daemon_health`.
pub fn daemon_health(uptime: Duration, sandboxes: u64) -> Event {
    Event::new(event::DAEMON_HEALTH)
        .s("uptime_bucket", uptime_bucket(uptime))
        .s("sandboxes", count_bucket(sandboxes))
}

/// `cua_spacesd_health`.
pub fn spacesd_health(uptime: Duration, imports_failed: u64) -> Event {
    Event::new(event::SPACESD_HEALTH)
        .s("uptime_bucket", uptime_bucket(uptime))
        .s("imports_failed", count_bucket(imports_failed))
}

/// `cua_spacesd_session_deliveries`.
pub fn spacesd_session_deliveries(grant_present: bool, count: u64) -> Event {
    Event::new(event::SPACESD_SESSION_DELIVERIES)
        .s(
            "delivery_grant",
            if grant_present { "present" } else { "absent" },
        )
        .n("count", count.min(100_000))
}

/// Create time bucket ([`schema::CREATE_TIMES`]).
pub fn create_time_bucket(d: Duration) -> &'static str {
    match d.as_secs() {
        0..=4 => "lt_5s",
        5..=9 => "5_9s",
        10..=19 => "10_19s",
        20..=39 => "20_39s",
        40..=59 => "40_59s",
        60..=119 => "60_119s",
        120..=299 => "2_4m",
        300..=599 => "5_9m",
        600..=1_799 => "10_29m",
        _ => "gte_30m",
    }
}

/// `cua_app_active` (send it with `Telemetry::capture_active_day`, which
/// keeps it to one per install per UTC day), with no experiment on.
pub fn app_active() -> Event {
    app_active_with(&[])
}

/// `cua_app_active` with the Spaces app's experiments that are on
/// (`experiments_on`, [`experiments_set`]).
pub fn app_active_with(experiments: &[&str]) -> Event {
    Event::new(event::APP_ACTIVE).owned("experiments_on", experiments_set(experiments))
}

/// The experiments in `ids` that are in [`schema::EXPERIMENTS`], in its
/// order and once each, joined with `+`; `none` when there are none.
/// Anything else is dropped.
pub fn experiments_set(ids: &[&str]) -> String {
    let on: Vec<&str> = schema::EXPERIMENTS
        .iter()
        .copied()
        .filter(|e| ids.iter().any(|i| i.trim() == *e))
        .collect();
    if on.is_empty() {
        "none".into()
    } else {
        on.join("+")
    }
}

/// `cua_experiment`: an experiment's switch in the Spaces app's Settings
/// (`experiment_on` or `experiment_off`); None for an action or experiment
/// outside the vocabulary.
pub fn experiment(action: &str, experiment: &str) -> Option<Event> {
    let a = pick(schema::EXPERIMENT_ACTIONS, action, "");
    let e = pick(schema::EXPERIMENTS, experiment, "");
    (!a.is_empty() && !e.is_empty()).then(|| {
        Event::new(event::EXPERIMENT)
            .s("action", a)
            .s("experiment", e)
    })
}

/// `cua_onboarding_page`; None for a page or action outside the
/// vocabulary. An unknown choice is sent as `none`.
pub fn onboarding_page(page: &str, action: &str, choice: &str) -> Option<Event> {
    let page = pick(schema::ONBOARDING_PAGES, page, "");
    let action = pick(schema::PAGE_ACTIONS, action, "");
    (!page.is_empty() && !action.is_empty()).then(|| {
        Event::new(event::ONBOARDING_PAGE)
            .s("page", page)
            .s("action", action)
            .s("choice", pick(schema::PAGE_CHOICES, choice, "none"))
    })
}

/// `cua_space_wizard`; None for an action outside the vocabulary.
pub fn space_wizard(action: &str) -> Option<Event> {
    let a = pick(schema::WIZARD_ACTIONS, action, "");
    (!a.is_empty()).then(|| Event::new(event::SPACE_WIZARD).s("action", a))
}

/// What a finished Space create was.
#[derive(Debug, Clone, Default)]
pub struct SpaceCreate<'a> {
    /// Location word (`local`, `cloud`, `direct:...`).
    pub on: &'a str,
    pub guest_os: &'a str,
    /// `container` or `vm`.
    pub kind: &'a str,
    /// The last progress phase (sent only on failure, classified).
    pub last_phase: &'a str,
    /// It failed because a phase stopped moving.
    pub stalled: bool,
    /// GPU acceleration was turned on.
    pub gpu: bool,
}

/// `cua_space_create`.
pub fn space_create(s: &SpaceCreate<'_>, outcome: Outcome, elapsed: Duration) -> Event {
    let failed_phase = match outcome {
        Outcome::Ok => "none",
        _ => match pick(
            schema::CREATE_PHASES,
            &s.last_phase.to_ascii_lowercase(),
            "other",
        ) {
            // `none` means success; a failure never reports it.
            "none" => "other",
            p => p,
        },
    };
    Event::new(event::SPACE_CREATE)
        .s("location", location(s.on))
        .s("guest_os", guest_os(s.guest_os))
        .s("kind", sandbox_kind(s.kind))
        .s("outcome", outcome.as_str())
        .s("failed_phase", failed_phase)
        .b("stalled", s.stalled && outcome == Outcome::Error)
        .s("time_bucket", create_time_bucket(elapsed))
        .b("gpu", s.gpu)
}

/// `cua_space_create_started`.
pub fn space_create_started(on: &str, guest: &str, kind: &str, gpu: bool) -> Event {
    Event::new(event::SPACE_CREATE_STARTED)
        .s("location", location(on))
        .s("guest_os", guest_os(guest))
        .s("kind", sandbox_kind(kind))
        .b("gpu", gpu)
}

/// A Cua Volume setting that was saved.
#[derive(Debug, Clone, Default)]
pub struct VolumeSetup<'a> {
    /// `onboarding` or `settings`.
    pub surface: &'a str,
    /// `this_mac`, `s3` or `later`.
    pub storage: &'a str,
    pub add_to_finder: bool,
    /// `volume_mount_status`'s method (`fskit`, `nfs`, `fuse`, `none`).
    pub mount_method: &'a str,
}

/// `cua_volume_setup`.
pub fn volume_setup(v: &VolumeSetup<'_>, outcome: Outcome) -> Event {
    let method = v.mount_method.trim().to_ascii_lowercase();
    Event::new(event::VOLUME_SETUP)
        .s(
            "surface",
            pick(schema::VOLUME_SURFACES, v.surface, "settings"),
        )
        .s("storage", pick(schema::VOLUME_STORAGE, v.storage, "later"))
        .b("add_to_finder", v.add_to_finder)
        .s(
            "mount_method",
            if method.is_empty() {
                "unknown"
            } else {
                pick(schema::MOUNT_METHODS, &method, "unknown")
            },
        )
        .s("outcome", outcome.as_str())
}

/// `cua_persistent_agent`; None for an action outside the vocabulary.
/// `harness` is classified with [`harness`] (only `create` names one).
pub fn persistent_agent(
    action: &str,
    harness_id: Option<&str>,
    outcome: Outcome,
    error_variant: Option<&str>,
) -> Option<Event> {
    let a = pick(schema::PERSISTENT_ACTIONS, action, "");
    (!a.is_empty()).then(|| {
        Event::new(event::PERSISTENT_AGENT)
            .s("action", a)
            .s("harness", harness_id.map(harness).unwrap_or("other"))
            .s("outcome", outcome.as_str())
            .s(
                "error_kind",
                error_variant.map(error_kind).unwrap_or("none"),
            )
    })
}

/// `cua_host_setup`. `profile` is the request's (`desktop`, `spare`);
/// `share_desktop` and `provide_spaces` both on is `both`.
pub fn host_setup(
    mode: &str,
    share_desktop: bool,
    provide_spaces: bool,
    outcome: Outcome,
    error_variant: Option<&str>,
) -> Event {
    let profile = match (share_desktop, provide_spaces) {
        (true, true) => "both",
        (false, true) => "spare",
        (true, false) => "desktop",
        (false, false) => "unknown",
    };
    Event::new(event::HOST_SETUP)
        .s(
            "mode",
            pick(schema::HOST_MODES, &mode.to_ascii_lowercase(), "unknown"),
        )
        .s("profile", profile)
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
}

/// `cua_host_space_provided`.
pub fn host_space_provided(
    kind: &str,
    guest: &str,
    outcome: Outcome,
    error_variant: Option<&str>,
) -> Event {
    Event::new(event::HOST_SPACE_PROVIDED)
        .s("kind", sandbox_kind(kind))
        .s("guest_os", guest_os(guest))
        .s("outcome", outcome.as_str())
        .s(
            "error_kind",
            error_variant.map(error_kind).unwrap_or("none"),
        )
}

/// A Keyvault action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyvaultAction {
    Setup,
    Unlock,
    Lock,
    Import,
    /// A site's sign-in was used in a Space.
    SiteLogin,
}

/// How the Keyvault was set up or unlocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyvaultMethod {
    /// The OS key store (Touch ID on a Mac).
    OsKeyStore,
    Passphrase,
    RecoveryKey,
    None,
}

/// `cua_keyvault_action`.
pub fn keyvault_action(a: KeyvaultAction, m: KeyvaultMethod, outcome: Outcome) -> Event {
    Event::new(event::KEYVAULT_ACTION)
        .s(
            "action",
            match a {
                KeyvaultAction::Setup => "setup",
                KeyvaultAction::Unlock => "unlock",
                KeyvaultAction::Lock => "lock",
                KeyvaultAction::Import => "import",
                KeyvaultAction::SiteLogin => "site_login",
            },
        )
        .s(
            "method",
            match m {
                KeyvaultMethod::OsKeyStore => "os_key_store",
                KeyvaultMethod::Passphrase => "passphrase",
                KeyvaultMethod::RecoveryKey => "recovery_key",
                KeyvaultMethod::None => "none",
            },
        )
        .s("outcome", outcome.as_str())
}

/// `cua_share`; None for an action outside the vocabulary.
pub fn share(action: &str, role: &str, outcome: Outcome) -> Option<Event> {
    let a = pick(schema::SHARE_ACTIONS, action, "");
    (!a.is_empty()).then(|| {
        Event::new(event::SHARE)
            .s("action", a)
            .s(
                "role",
                if a == "unshare" {
                    "none"
                } else {
                    pick(schema::SHARE_ROLES, &role.to_ascii_lowercase(), "none")
                },
            )
            .s("outcome", outcome.as_str())
    })
}

/// `cua_presence_session`: `participants` is the most people seen at once.
pub fn presence_session(outcome: Outcome, participants: u64, elapsed: Duration) -> Event {
    Event::new(event::PRESENCE_SESSION)
        .s("outcome", outcome.as_str())
        .s("participants", count_bucket(participants))
        .s("duration_bucket", duration_bucket(elapsed))
}

/// `cua_app_update`; None for an action outside the vocabulary. An
/// unknown channel is `stable`, an unknown trigger `background`.
pub fn app_update(action: &str, channel: &str, trigger: &str) -> Option<Event> {
    let a = pick(schema::UPDATE_ACTIONS, action, "");
    (!a.is_empty()).then(|| {
        Event::new(event::APP_UPDATE)
            .s("action", a)
            .s("channel", pick(schema::UPDATE_CHANNELS, channel, "stable"))
            .s(
                "trigger",
                pick(schema::UPDATE_TRIGGERS, trigger, "background"),
            )
    })
}

/// `cua_device_enroll`; None for a method outside the vocabulary.
pub fn device_enroll(method: &str, outcome: Outcome) -> Option<Event> {
    let m = pick(schema::ENROLL_METHODS, method, "");
    (!m.is_empty()).then(|| {
        Event::new(event::DEVICE_ENROLL)
            .s("method", m)
            .s("outcome", outcome.as_str())
    })
}
