//! The control plane's value types.
//!
//! Every one of these is a plain record or a payload-free enum, because
//! UniFFI 0.31 has no sum type with payloads that round-trips cleanly to
//! Swift, TypeScript and Python at once. Where the Swift package used an enum
//! with associated values, the record here carries an explicit discriminator.
//!
//! **The honesty booleans live here**, as plain `bool` fields that are never
//! optional, never inferred from the presence of a value, and never dropped by
//! a binding:
//!
//! | Field | Type |
//! |---|---|
//! | `is_inferred` | [`AgentEvent`] |
//! | `is_server_backed` | [`SchedulerFacts`] |
//! | `is_server_published` | [`TransferLimits`] |
//! | `approvals_are_enforced` | [`AgentRunHandle`] |
//! | `server_backstop` | [`ProviderCapabilities`] |
//! | `is_production_ready` | [`AgentKindInfo`] |

use serde_json::{Map, Value};

use crate::client::error::{Result, SpacesError};

fn string(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

fn any_string(row: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string(row, key))
}

fn any_bool(row: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| row.get(*key).and_then(Value::as_bool))
}

fn any_i64(row: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        row.get(*key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|number| number as i64))
        })
    })
}

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

/// Which kind of Space this is.
///
/// `FRICTION.md` §6: Local and Fleet Spaces disagree on their vocabulary at
/// every level. An app branches on a capability or a normalised value, never
/// on a string it had to learn empirically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpaceProvider {
    Local,
    Fleet,
    /// Any machine running cua-spacesd, added by URL (`add_space`).
    Direct,
    Demo,
    Unknown,
}

impl SpaceProvider {
    pub fn from_raw(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "local" => SpaceProvider::Local,
            // `cloud` is the unified location word; the legacy Spaces
            // server sends `fleet`.
            "fleet" | "cloud" => SpaceProvider::Fleet,
            "direct" => SpaceProvider::Direct,
            "demo" => SpaceProvider::Demo,
            _ => SpaceProvider::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SpaceProvider::Local => "local",
            SpaceProvider::Fleet => "fleet",
            SpaceProvider::Direct => "direct",
            SpaceProvider::Demo => "demo",
            SpaceProvider::Unknown => "unknown",
        }
    }

    /// `$HOME` inside the Space. §6 records that this differs per provider and
    /// leaks into every default upload destination, so it is published rather
    /// than left for each caller to hard-code.
    pub fn home(self) -> &'static str {
        match self {
            SpaceProvider::Local => "/Users/lume",
            SpaceProvider::Fleet => "/root",
            // Whoever the machine's spacesd runs as; the server expands `~`.
            SpaceProvider::Direct => "~",
            SpaceProvider::Demo | SpaceProvider::Unknown => "/tmp",
        }
    }

    pub fn default_upload_directory(self) -> String {
        format!("{}/Downloads", self.home())
    }

    pub fn capabilities(self) -> ProviderCapabilities {
        match self {
            SpaceProvider::Local => ProviderCapabilities {
                provider: "local".into(),
                agents: true,
                window_list: true,
                upload: true,
                download: true,
                rcdp_streaming: true,
                teleport: true,
                provisioning: false,
                server_backstop: false,
                notes: vec![
                    "Local Spaces are invisible to get_or_create_space; attach is the only way \
                     to reach one (FRICTION.md §5)."
                        .into(),
                    "teleport_app takes the Local branch first; this capability used to read \
                     false and was wrong."
                        .into(),
                ],
            },
            SpaceProvider::Fleet => ProviderCapabilities {
                provider: "fleet".into(),
                agents: true,
                window_list: true,
                upload: true,
                download: true,
                rcdp_streaming: true,
                teleport: true,
                provisioning: true,
                server_backstop: false,
                notes: vec![
                    "download answers in prose rather than JSON on this provider \
                     (FRICTION.md §4); the SDK normalises it."
                        .into(),
                    "stream_endpoint works here too: the media ticket comes from the SDK, not \
                     from a token read over ssh."
                        .into(),
                ],
            },
            SpaceProvider::Direct => ProviderCapabilities {
                provider: "direct".into(),
                agents: true,
                window_list: true,
                upload: true,
                download: true,
                rcdp_streaming: true,
                teleport: true,
                provisioning: false,
                server_backstop: false,
                notes: vec![
                    "A Direct Space is any cua-spacesd added by URL; it is registered, \
                     never provisioned, and release only unregisters it."
                        .into(),
                ],
            },
            SpaceProvider::Demo | SpaceProvider::Unknown => ProviderCapabilities {
                provider: self.as_str().into(),
                agents: false,
                window_list: false,
                upload: false,
                download: false,
                rcdp_streaming: false,
                teleport: false,
                provisioning: false,
                server_backstop: false,
                notes: vec!["No Space is attached.".into()],
            },
        }
    }
}

/// What a provider can actually do, declared rather than discovered by calling
/// a tool and reading the prose it fails with (`FRICTION.md` §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub provider: String,
    pub agents: bool,
    pub window_list: bool,
    pub upload: bool,
    pub download: bool,
    pub rcdp_streaming: bool,
    pub teleport: bool,
    pub provisioning: bool,
    /// **Honesty flag.** Whether the *server* will end a run or a Space that
    /// the client stopped watching.
    ///
    /// `false` everywhere, and the most expensive `false` in the SDK. A
    /// `SIGKILL`ed client never runs its cleanup, which is how one demo Space
    /// accumulated roughly 112 orphaned windows. Never optional: an
    /// `Option<bool>` would read to a caller as permission.
    pub server_backstop: bool,
    pub notes: Vec<String>,
}

/// One normalised readiness vocabulary. A ready Space reports `running` on
/// Local and `Bound` on Fleet; both become `Ready`, and the string that
/// produced it is kept on `raw_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpaceState {
    Ready,
    Starting,
    Stopped,
    Failed,
    Unknown,
}

impl SpaceState {
    pub fn from_phase(phase: &str) -> Self {
        match phase.to_ascii_lowercase().as_str() {
            "running" | "bound" | "ready" => SpaceState::Ready,
            "pending" | "starting" | "provisioning" | "creating" => SpaceState::Starting,
            "stopped" | "released" | "terminated" => SpaceState::Stopped,
            "failed" | "error" => SpaceState::Failed,
            _ => SpaceState::Unknown,
        }
    }

    pub fn is_ready(self) -> bool {
        self == SpaceState::Ready
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SpaceState::Ready => "ready",
            SpaceState::Starting => "starting",
            SpaceState::Stopped => "stopped",
            SpaceState::Failed => "failed",
            SpaceState::Unknown => "unknown",
        }
    }
}

/// A Space as the account sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceInfo {
    /// A `SpaceID` in Rust and in Swift; a plain `String` on the wire.
    pub id: String,
    pub provider: SpaceProvider,
    pub operating_system: String,
    pub state: SpaceState,
    /// The phase string the server actually sent. Normalising never discards
    /// the original.
    pub raw_phase: String,
    pub ip_address: Option<String>,
}

impl SpaceInfo {
    pub fn from_row(row: &Map<String, Value>) -> Self {
        let id = string(row, "id").unwrap_or_default();
        let phase = string(row, "phase").unwrap_or_default();
        let declared = string(row, "provider")
            .or_else(|| provider_prefix(&id).map(str::to_string))
            .unwrap_or_default();
        SpaceInfo {
            provider: SpaceProvider::from_raw(&declared),
            operating_system: string(row, "os").unwrap_or_default(),
            state: SpaceState::from_phase(&phase),
            raw_phase: phase,
            ip_address: string(row, "ip"),
            id,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state.is_ready()
    }

    pub fn capabilities(&self) -> ProviderCapabilities {
        self.provider.capabilities()
    }
}

/// The provider half of a qualified Space id, when there is one.
pub fn provider_prefix(space_id: &str) -> Option<&str> {
    if let Some(rest) = space_id.strip_prefix("space://") {
        return rest.split_once('/').map(|(provider, _)| provider);
    }
    space_id.split_once(':').map(|(prefix, _)| prefix)
}

/// A window inside a Space, and the rcdp target that streams it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceWindow {
    pub id: String,
    pub app: String,
    pub title: String,
    pub width_px: u32,
    pub height_px: u32,
    /// Scale factor in hundredths, so no float crosses the boundary or the
    /// conformance document.
    pub scale_factor_hundredths: u32,
    pub visible: bool,
    /// The owning process, when the server reports it. The join key
    /// `FRICTION.md` §37 asks for.
    pub process_id: Option<i32>,
}

impl SpaceWindow {
    pub fn from_row(row: &Map<String, Value>) -> Self {
        // §7: the list tool names this `window`, the stream tool takes it as
        // `window_id`. Reading only one spelling yields an empty id for every
        // window — silently, because the JSON parse succeeded.
        let id = any_string(row, &["window", "window_id", "id"]).unwrap_or_default();
        let geometry = row
            .get("geometry")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let scale = geometry
            .get("scale_factor")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        SpaceWindow {
            id,
            app: any_string(row, &["app_name", "app"]).unwrap_or_default(),
            title: string(row, "title").unwrap_or_default(),
            width_px: any_i64(&geometry, &["width_px"]).unwrap_or(0).max(0) as u32,
            height_px: any_i64(&geometry, &["height_px"]).unwrap_or(0).max(0) as u32,
            scale_factor_hundredths: (scale * 100.0).round().max(0.0) as u32,
            visible: row.get("visible").and_then(Value::as_bool).unwrap_or(true),
            process_id: any_i64(row, &["pid", "owner_pid"]).map(|pid| pid as i32),
        }
    }

    /// rcdp targets are `target-` prefixed. An id that is not is the silent
    /// failure §7 describes — a JSON parse that succeeded against the wrong key.
    pub fn looks_like_rcdp_target(&self) -> bool {
        self.id.starts_with("target-")
    }
}

/// Where an in-product stream connects. Distinct from the operator-facing
/// display tools — `FRICTION.md` §10.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEndpoint {
    pub host: String,
    pub port: u16,
    /// The cua-driver port on the same Space, behind the same per-boot token.
    pub driver_port: u16,
    /// Minted fresh on every boot of the Space, so a cached value is valid
    /// only until it restarts.
    pub token: String,
}

impl StreamEndpoint {
    /// `local_rcdp` answers with a single `ws://host:port/ws?token=…` URL on
    /// some backends and with structured fields on others. Both land here, so
    /// no caller parses a string the server happened to format (§4).
    pub fn from_row(row: &Map<String, Value>, fallback_host: Option<&str>) -> Self {
        let mut host = string(row, "host")
            .or_else(|| fallback_host.map(str::to_string))
            .unwrap_or_default();
        let mut port = any_i64(row, &["port"]).unwrap_or(8765) as u16;
        let mut token = string(row, "token")
            .or_else(|| string(row, "ticket"))
            .unwrap_or_default();

        if let Some(url) = any_string(row, &["ws", "url", "ws_url"]) {
            if let Some(rest) = url.split("://").nth(1) {
                let authority = rest.split('/').next().unwrap_or("");
                if let Some((parsed_host, parsed_port)) = authority.rsplit_once(':') {
                    if let Ok(parsed) = parsed_port.parse::<u16>() {
                        host = parsed_host.to_string();
                        port = parsed;
                    }
                } else if !authority.is_empty() {
                    host = authority.to_string();
                }
            }
            if token.is_empty()
                && let Some((_, query)) = url
                    .split_once("token=")
                    .or_else(|| url.split_once("ticket="))
            {
                token = query.split('&').next().unwrap_or("").to_string();
            }
        }
        StreamEndpoint {
            host,
            port,
            driver_port: any_i64(row, &["driver_port"]).unwrap_or(8801) as u16,
            token,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent runs
// ---------------------------------------------------------------------------

/// The status vocabulary the Spaces agent harness publishes, preserved
/// verbatim. `FRICTION.md` §9 records this as a thing the MCP got *right*:
/// `Unknown` means "the probe failed or the signal was ambiguous" and is never
/// a stand-in for a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    Running,
    AwaitingInput,
    Idle,
    Finished,
    Failed,
    Crashed,
    Unknown,
}

impl AgentState {
    pub fn from_wire(wire: Option<&str>) -> Self {
        match wire.unwrap_or("") {
            "running" => AgentState::Running,
            "awaiting_input" => AgentState::AwaitingInput,
            "idle" => AgentState::Idle,
            "finished" => AgentState::Finished,
            "failed" => AgentState::Failed,
            "crashed" => AgentState::Crashed,
            _ => AgentState::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Running => "running",
            AgentState::AwaitingInput => "awaiting_input",
            AgentState::Idle => "idle",
            AgentState::Finished => "finished",
            AgentState::Failed => "failed",
            AgentState::Crashed => "crashed",
            AgentState::Unknown => "unknown",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(
            self,
            AgentState::Running | AgentState::AwaitingInput | AgentState::Idle
        )
    }

    pub fn has_ended(self) -> bool {
        matches!(
            self,
            AgentState::Finished | AgentState::Failed | AgentState::Crashed
        )
    }
}

/// **One** state type, returned in full by every call that reports state.
///
/// `FRICTION.md` §22: the cheap call and the expensive call used to disagree
/// about what a state is. Here the cheap call may omit the output tail —
/// `output_tail` is `Option` and says so — but never the explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSnapshot {
    pub id: String,
    pub space: String,
    pub agent: String,
    pub state: AgentState,
    /// Why it is in that state, in the harness's own words.
    pub reason: String,
    /// Whether a message will be accepted right now. **Published, not
    /// inferred** — `FRICTION.md` §9.
    pub accepts_message: bool,
    pub exit_code: Option<i32>,
    pub summary: String,
    /// `None` — not `""` — when the call does not carry output, so a consumer
    /// can tell "no output" from "not asked for".
    pub output_tail: Option<String>,
    /// `FRICTION.md` §23: truncation used to be silent.
    pub output_truncated: bool,
    pub created_at_ms: Option<i64>,
    /// True when `reason` came from an earlier, richer snapshot. Surfaced
    /// rather than hidden.
    pub reason_is_carried_forward: bool,
    /// The prompt as the harness echoes it, metadata marker and all.
    pub raw_prompt: Option<String>,
}

impl RunSnapshot {
    pub fn decode(
        row: &Map<String, Value>,
        run_id: &str,
        space: &str,
        requested_tail: Option<u32>,
        previous: Option<&RunSnapshot>,
    ) -> Self {
        let tail = string(row, "output_tail");
        let mut reason = string(row, "reason").unwrap_or_default();
        let mut carried = false;
        if reason.is_empty()
            && let Some(previous) = previous
            && !previous.reason.is_empty()
        {
            reason = previous.reason.clone();
            carried = true;
        }
        let truncated = match row.get("truncated").and_then(Value::as_bool) {
            Some(explicit) => explicit,
            None => match (&tail, requested_tail) {
                // The server returns at most `tail` lines and says nothing
                // when it clipped. A full window is the only signal there is.
                (Some(tail), Some(requested)) if requested > 0 => {
                    tail.split('\n').count() as u32 >= requested
                }
                _ => false,
            },
        };
        RunSnapshot {
            id: string(row, "run_id").unwrap_or_else(|| run_id.to_string()),
            space: space.to_string(),
            agent: string(row, "agent")
                .or_else(|| previous.map(|p| p.agent.clone()))
                .unwrap_or_default(),
            state: AgentState::from_wire(row.get("status").and_then(Value::as_str)),
            reason,
            accepts_message: row
                .get("accepts_message")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            exit_code: any_i64(row, &["exit_code"]).map(|code| code as i32),
            summary: string(row, "summary").unwrap_or_default(),
            output_tail: tail,
            output_truncated: truncated,
            created_at_ms: row
                .get("created_at")
                .and_then(Value::as_f64)
                .map(|seconds| (seconds * 1000.0).round() as i64),
            reason_is_carried_forward: carried,
            raw_prompt: any_string(row, &["prompt", "raw_summary"])
                .or_else(|| previous.and_then(|p| p.raw_prompt.clone())),
        }
    }

    /// Anything a consumer would redraw for.
    pub fn differs_visibly(&self, other: &RunSnapshot) -> bool {
        self.state != other.state
            || self.reason != other.reason
            || self.accepts_message != other.accepts_message
            || self.exit_code != other.exit_code
            || self.summary != other.summary
            || self.output_tail != other.output_tail
    }
}

/// What the harness published about how a run takes turns — read from
/// `agent_start`'s `capabilities`, never guessed from behaviour (§9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnModel {
    pub kind: String,
    pub accepts_follow_ups: bool,
}

impl TurnModel {
    pub fn from_row(row: &Map<String, Value>) -> Self {
        TurnModel {
            kind: any_string(row, &["turn_model", "kind"]).unwrap_or_else(|| "unknown".into()),
            accepts_follow_ups: any_bool(row, &["accepts_followups", "accepts_message"])
                .unwrap_or(true),
        }
    }
}

/// The handle `start_agent` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunHandle {
    pub id: String,
    pub space: String,
    pub agent: String,
    pub turn_model: TurnModel,
    pub notes: Vec<String>,
    /// **Honesty flag.** Whether a human decision at an approval seam can
    /// actually stop this agent.
    ///
    /// `false` in every shipping backend: `agent_start` runs auto-approved and
    /// the Space *is* the sandbox. Published so a product renders "not
    /// enforced in this build" from the SDK rather than hard-coding the claim.
    pub approvals_are_enforced: bool,
}

impl AgentRunHandle {
    /// The run's private directory inside the Space. Published because
    /// `FRICTION.md` §8 records that cleanup otherwise forces every caller to
    /// hard-code this path.
    pub fn directory(&self) -> String {
        format!("~/.spaces-agents/{}", self.id)
    }
}

/// `false` in every shipping backend. See [`AgentRunHandle::approvals_are_enforced`].
pub const APPROVALS_ARE_ENFORCED: bool = false;

/// How a message should be delivered to a run that may be mid-turn. A
/// payload-free enum; `queue`'s timeout is a separate argument, because
/// UniFFI 0.31 sum types with payloads do not round-trip cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Refuse if a turn is in flight, with the harness's explanation. The
    /// default, and it stays the default: refusal beats silent damage (§9).
    RefuseIfBusy,
    /// Abandon the turn in flight and deliver into the wreckage. Named for
    /// what the backend does: `force: true` reaches `runner.kill`.
    AbandonCurrentTurn,
    /// Hold the message until the run accepts it, then deliver. **There is no
    /// backend queue**: the message is held in this process and dies with it.
    QueueUntilIdle,
}

/// The result of a send, with **one** shape and a populated `reason` on both
/// branches. `accepted` is the only discriminator and it is always present, so
/// a refusal cannot be mistaken for a delivery. `FRICTION.md` §4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub run_id: String,
    pub accepted: bool,
    pub reason: String,
    /// The run's state at the moment of refusal, when the harness reported it.
    pub state_at_refusal: Option<AgentState>,
    /// How long the core waited before delivering, for `QueueUntilIdle`.
    pub queued_for_ms: Option<u64>,
}

impl Delivery {
    pub fn decode(row: &Map<String, Value>, run_id: &str, queued_for_ms: Option<u64>) -> Self {
        let delivered = row
            .get("delivered")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // §4: "why" is `note` on one branch and `reason` on the other, so it
        // cannot be read uniformly. It is read here, once.
        let why = if delivered {
            string(row, "note")
        } else {
            string(row, "reason")
        };
        Delivery {
            run_id: string(row, "run_id").unwrap_or_else(|| run_id.to_string()),
            accepted: delivered,
            reason: why
                .unwrap_or_else(|| if delivered { "delivered" } else { "refused" }.to_string()),
            state_at_refusal: if delivered {
                None
            } else {
                Some(AgentState::from_wire(
                    row.get("status").and_then(Value::as_str),
                ))
            },
            queued_for_ms,
        }
    }

    /// For callers who want a refusal on the error channel instead.
    pub fn required(self) -> Result<Delivery> {
        if self.accepted {
            Ok(self)
        } else {
            Err(SpacesError::ToolFailed {
                tool: "agent_message".into(),
                message: self.reason,
            })
        }
    }
}

/// The result of a stop, which **verifies** rather than assuming (§9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopOutcome {
    pub stopped: bool,
    /// `None` when the liveness probe itself could not run — which is not the
    /// same as "alive" and is not flattened into it.
    pub alive: Option<bool>,
    pub reason: String,
}

impl StopOutcome {
    pub fn decode(row: &Map<String, Value>) -> Self {
        StopOutcome {
            stopped: row.get("stopped").and_then(Value::as_bool).unwrap_or(false),
            alive: row.get("alive").and_then(Value::as_bool),
            reason: string(row, "reason").unwrap_or_default(),
        }
    }
}

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStartRequest {
    pub agent: String,
    pub prompt: String,
    /// The field `FRICTION.md` §21 and §41 ask for. The harness has nowhere to
    /// put an application's own key, so the core does the smuggling in one
    /// place, out of the prompt body, and strips it back out of every
    /// `summary` it publishes.
    pub metadata: Vec<MetadataEntry>,
    /// Maps to `agent_start`'s `show`. **A test suite should set it false**:
    /// surviving terminal windows are what trashed a demo machine twice
    /// (§54).
    pub shows_window: bool,
    /// Reserved. No backend reads this key today — `server_backstop` is
    /// `false` everywhere — and an older server ignores an unknown key, so
    /// sending it is free and gaining the behaviour is not a breaking change.
    pub timeout_seconds: Option<u32>,
}

impl Default for AgentStartRequest {
    fn default() -> Self {
        AgentStartRequest {
            agent: "claude-code".into(),
            prompt: String::new(),
            metadata: Vec::new(),
            shows_window: true,
            timeout_seconds: None,
        }
    }
}

/// A metadata key/value. A record rather than a map, because UniFFI maps do
/// not preserve order and the metadata marker must be byte-stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    pub key: String,
    pub value: String,
}

/// Which agent harness to start, and whether it is real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentKindInfo {
    pub id: String,
    /// **Honesty flag.** Whether this backend is wired end to end, reported
    /// rather than discovered by watching a run fail. Two harnesses are
    /// production-ready; everything else `agent_start` accepts is a stub that
    /// starts and exits, and a picker should grey those out.
    pub is_production_ready: bool,
}

/// The two harnesses that are wired end to end.
pub const PRODUCTION_READY_AGENTS: [&str; 2] = ["claude-code", "codex"];

pub fn agent_kind(id: &str) -> AgentKindInfo {
    AgentKindInfo {
        id: id.to_string(),
        is_production_ready: PRODUCTION_READY_AGENTS.contains(&id),
    }
}

/// What the server says about its agent harnesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCapabilities {
    pub statuses: Vec<String>,
    pub harnesses: Vec<AgentKindInfo>,
    /// Which classifier produced a status, or empty when none is configured.
    /// An unconfigured classifier reports nothing rather than a guess dressed
    /// as a state.
    pub status_classifier: String,
}

impl HarnessCapabilities {
    pub fn decode(row: &Map<String, Value>) -> Self {
        HarnessCapabilities {
            statuses: row
                .get("statuses")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            harnesses: row
                .get("harnesses")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_object)
                        .filter_map(|harness| any_string(harness, &["name", "agent"]))
                        .map(|name| agent_kind(&name))
                        .collect()
                })
                .unwrap_or_default(),
            status_classifier: string(row, "status_classifier").unwrap_or_default(),
        }
    }
}

/// Agent output as typed events.
///
/// **The honesty rule, and it is the point of this type.** The Spaces harness
/// publishes one thing about a run's output: `output_tail`, a fixed window of
/// terminal scrollback. From that, the only kinds this core will ever produce
/// are `Text`, `StateChanged` and `Finished`, and they carry
/// `is_inferred == false`. Everything richer — a tool use, a question, an
/// artifact — is a guess made by reading scrollback, is produced only by the
/// opt-in transcript classifier, and carries `is_inferred == true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub run_id: String,
    /// The line this event was read from, so a caller can resume.
    pub line_index: u32,
    pub kind: AgentEventKind,
    /// The plain-text projection, always populated, so a caller that only
    /// wants a boring transcript never switches on `kind`.
    pub text: String,
    pub state: Option<AgentState>,
    pub exit_code: Option<i32>,
    /// **Honesty flag.** `true` when a classifier guessed this event out of
    /// terminal scrollback rather than the harness having published it.
    /// Defaults to `false`; nothing in this crate sets it `true`.
    pub is_inferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEventKind {
    /// A line of output.
    Text,
    /// Published by the harness, never inferred.
    StateChanged,
    Finished,
    /// A line the core could not say anything more about than that it arrived.
    Raw,
    // --- inferred-only below: never produced without a classifier ---
    ToolUse,
    Artifact,
    Question,
}

impl AgentEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentEventKind::Text => "text",
            AgentEventKind::StateChanged => "state_changed",
            AgentEventKind::Finished => "finished",
            AgentEventKind::Raw => "raw",
            AgentEventKind::ToolUse => "tool_use",
            AgentEventKind::Artifact => "artifact",
            AgentEventKind::Question => "question",
        }
    }

    /// Whether this kind can only ever be a guess. A binding can assert that
    /// no event of one of these kinds ever arrives with `is_inferred == false`.
    pub fn is_inferred_only(self) -> bool {
        matches!(
            self,
            AgentEventKind::ToolUse | AgentEventKind::Artifact | AgentEventKind::Question
        )
    }
}

/// Split a snapshot into events, **without inferring anything**.
///
/// Every event this produces carries `is_inferred == false`, because every one
/// of them is either a literal line of the harness's output or the harness's
/// own published status.
pub fn events_from_snapshot(snapshot: &RunSnapshot) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    if let Some(tail) = &snapshot.output_tail {
        for (index, line) in tail.split('\n').enumerate() {
            if line.is_empty() && index + 1 == tail.split('\n').count() {
                continue;
            }
            events.push(AgentEvent {
                run_id: snapshot.id.clone(),
                line_index: index as u32,
                kind: AgentEventKind::Text,
                text: line.to_string(),
                state: None,
                exit_code: None,
                is_inferred: false,
            });
        }
    }
    let next = events.len() as u32;
    events.push(AgentEvent {
        run_id: snapshot.id.clone(),
        line_index: next,
        kind: AgentEventKind::StateChanged,
        text: format!("[{}] {}", snapshot.state.as_str(), snapshot.reason),
        state: Some(snapshot.state),
        exit_code: None,
        is_inferred: false,
    });
    if snapshot.state.has_ended() {
        events.push(AgentEvent {
            run_id: snapshot.id.clone(),
            line_index: next + 1,
            kind: AgentEventKind::Finished,
            text: match snapshot.exit_code {
                Some(code) => format!("[exited {code}]"),
                None => "[finished]".to_string(),
            },
            state: Some(snapshot.state),
            exit_code: snapshot.exit_code,
            is_inferred: false,
        });
    }
    events
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

/// What the core can honestly say about recurring work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerFacts {
    /// **Honesty flag.** `false` in every shipping backend: `spaces_mcp.py`
    /// contains zero occurrences of `schedule`, `cron` or `recurr`. A schedule
    /// fires while your process runs, and not after. **Render it** — a product
    /// that draws a routine as though it were durable is lying on the SDK's
    /// behalf.
    pub is_server_backed: bool,
    /// The documented policy, so every client does not invent a different one.
    /// Twelve overnight routines landing at once is worse than a skipped run.
    pub missed_slot_policy: String,
}

pub const SCHEDULER_IS_SERVER_BACKED: bool = false;

pub fn scheduler_facts() -> SchedulerFacts {
    SchedulerFacts {
        is_server_backed: SCHEDULER_IS_SERVER_BACKED,
        missed_slot_policy: "collapse_to_one_firing".into(),
    }
}

// ---------------------------------------------------------------------------
// Transfers
// ---------------------------------------------------------------------------

/// Where an upload should land. A payload-free discriminator; the directory or
/// path travels alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadPlacementKind {
    /// Exactly this path. May overwrite — chosen deliberately, spelled out.
    ExactPath,
    /// Inside a directory under the user's own filename, in a freshly minted
    /// uniquely-named subdirectory, so two files called `notes.txt` stay two
    /// files and the agent still sees `notes.txt`. `FRICTION.md` §13.
    CollisionSafe,
    /// Inside a directory under the user's filename, overwriting anything
    /// already there. The old client's behaviour, available but named.
    Clobbering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadPlacement {
    pub kind: UploadPlacementKind,
    /// The exact path, or the directory. Empty means the provider's default
    /// upload directory.
    pub path: String,
}

impl UploadPlacement {
    pub fn collision_safe_default() -> Self {
        UploadPlacement {
            kind: UploadPlacementKind::CollisionSafe,
            path: String::new(),
        }
    }
}

/// A file that now exists inside the Space, at the path actually written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub path: String,
    pub name: String,
    pub byte_count: Option<u64>,
}

/// Caller-declared transfer limits, checked before any I/O.
///
/// `FRICTION.md` §14: the documented caps are a product contract with nothing
/// behind them in the API. The core cannot invent a server-side cap; what it
/// can do is carry the caps as data, check all three before any byte moves,
/// and name the file that broke the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferLimits {
    pub max_file_count: u32,
    pub max_bytes_per_file: u64,
    pub max_bytes_per_batch: u64,
    /// **Honesty flag.** Whether the *server* published these numbers.
    ///
    /// `false` everywhere, and not a placeholder: the only cap the backend
    /// enforces is `MAX_TRANSFER_BYTES`, a hard-coded 25 MB constant at
    /// `spaces_mcp.py:62`. There is no limits tool, so the count cap and the
    /// batch cap have no server counterpart at all and two clients will drift.
    pub is_server_published: bool,
}

impl TransferLimits {
    /// The one cap the backend actually enforces, plus SDK-side defaults for
    /// the two it does not.
    pub fn conservative_default() -> Self {
        TransferLimits {
            max_file_count: 6,
            max_bytes_per_file: 25 * 1024 * 1024,
            max_bytes_per_batch: 200 * 1_000_000,
            is_server_published: false,
        }
    }

    /// No limits. The core's default, because it does not know the product's
    /// policy and will not invent one.
    pub fn unlimited() -> Self {
        TransferLimits {
            max_file_count: u32::MAX,
            max_bytes_per_file: u64::MAX,
            max_bytes_per_batch: u64::MAX,
            is_server_published: false,
        }
    }

    /// Admit what fits and say precisely why the rest does not.
    ///
    /// The order of the checks is load-bearing: per-file size is tested before
    /// the running total, so one oversized file is reported as oversized
    /// rather than as "the batch is too big" — which would point the user at
    /// the wrong file to remove.
    pub fn admit(&self, candidates: &[TransferCandidate]) -> Admission {
        let mut admission = Admission::default();
        let mut count: u64 = 0;
        let mut total: u64 = 0;
        for candidate in candidates {
            if count >= u64::from(self.max_file_count) {
                admission.rejected.push(Rejection {
                    candidate: candidate.clone(),
                    violation: format!(
                        "{} files exceeds the limit of {}",
                        count + 1,
                        self.max_file_count
                    ),
                });
                continue;
            }
            if candidate.byte_count > self.max_bytes_per_file {
                admission.rejected.push(Rejection {
                    candidate: candidate.clone(),
                    violation: format!(
                        "{} is {} bytes, over the {} byte limit",
                        candidate.name, candidate.byte_count, self.max_bytes_per_file
                    ),
                });
                continue;
            }
            if total.saturating_add(candidate.byte_count) > self.max_bytes_per_batch {
                admission.rejected.push(Rejection {
                    candidate: candidate.clone(),
                    violation: format!(
                        "{} bytes total, over the {} byte limit",
                        total + candidate.byte_count,
                        self.max_bytes_per_batch
                    ),
                });
                continue;
            }
            admission.accepted.push(candidate.clone());
            count += 1;
            total += candidate.byte_count;
        }
        admission
    }

    /// Check every rule **before any I/O**, which is the only honest order:
    /// half a batch uploaded and then refused is worse than refused.
    pub fn check(&self, candidates: &[TransferCandidate]) -> Result<()> {
        let admission = self.admit(candidates);
        match admission.rejected.first() {
            Some(rejection) => Err(SpacesError::LimitExceeded(rejection.violation.clone())),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferCandidate {
    pub name: String,
    pub byte_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub candidate: TransferCandidate,
    pub violation: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Admission {
    pub accepted: Vec<TransferCandidate>,
    pub rejected: Vec<Rejection>,
}

// ---------------------------------------------------------------------------
// In-space MCP services
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceTool {
    pub service: String,
    pub name: String,
    pub summary: String,
    /// The tool's input schema as JSON text, or empty when the server did not
    /// publish one. Text rather than a tree, because a JSON schema is data a
    /// caller hands onwards rather than a shape the SDK models.
    pub input_schema_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCatalog {
    pub service: String,
    pub tools: Vec<SpaceTool>,
    pub other_services: Vec<String>,
    pub instructions: Option<String>,
    /// **A reachable service that advertises zero tools exists; it is not
    /// ready.** The server says so out loud rather than letting an empty array
    /// read as "no such service", and the core carries it rather than
    /// flattening it.
    pub not_ready_warning: Option<String>,
}

impl ServiceCatalog {
    pub fn is_empty_but_reachable(&self) -> bool {
        self.tools.is_empty() && self.not_ready_warning.is_some()
    }
}

/// A content part returned by an in-Space tool. cua-driver answers with an
/// image *and* text, so this is a list of parts and not a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContentPart {
    /// `text`, `image`, or `other`.
    pub kind: String,
    pub text: String,
    /// Base64 for an image part, empty otherwise. Base64 rather than bytes so
    /// the part is one record in every language.
    pub data_base64: String,
    pub mime_type: String,
}

pub fn tool_content_parts(payload: &Value) -> Vec<ToolContentPart> {
    let rows = match payload.as_array() {
        Some(rows) => rows.clone(),
        None => {
            return vec![match payload.as_str() {
                Some(text) => ToolContentPart {
                    kind: "text".into(),
                    text: text.to_string(),
                    data_base64: String::new(),
                    mime_type: String::new(),
                },
                None => ToolContentPart {
                    kind: "other".into(),
                    text: payload.to_string(),
                    data_base64: String::new(),
                    mime_type: String::new(),
                },
            }];
        }
    };
    rows.iter()
        .map(|row| {
            let object = row.as_object().cloned().unwrap_or_default();
            match object.get("type").and_then(Value::as_str) {
                Some("text") => ToolContentPart {
                    kind: "text".into(),
                    text: string(&object, "text").unwrap_or_default(),
                    data_base64: String::new(),
                    mime_type: String::new(),
                },
                Some("image") => ToolContentPart {
                    kind: "image".into(),
                    text: String::new(),
                    data_base64: string(&object, "data").unwrap_or_default(),
                    mime_type: string(&object, "mimeType").unwrap_or_default(),
                },
                _ => ToolContentPart {
                    kind: "other".into(),
                    text: row.to_string(),
                    data_base64: String::new(),
                    mime_type: String::new(),
                },
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Hotspot
// ---------------------------------------------------------------------------

/// Whether this Mac is sharing its network, and with which Space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotspotStatus {
    pub is_sharing: bool,
    pub space: Option<String>,
    /// Whatever the control server said, kept whole.
    pub detail: String,
}

impl HotspotStatus {
    pub fn decode(row: &Map<String, Value>) -> Self {
        let space = any_string(row, &["space_id", "space"]).filter(|id| !id.is_empty());
        HotspotStatus {
            is_sharing: any_bool(row, &["sharing", "active", "running"]).unwrap_or(false),
            space,
            detail: any_string(row, &["status", "note"])
                .unwrap_or_else(|| Value::Object(row.clone()).to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn both_readiness_vocabularies_normalise_without_discarding_the_original() {
        let local = SpaceInfo::from_row(&object(
            json!({ "id": "local:cua-space-1", "phase": "running", "os": "macos" }),
        ));
        let fleet = SpaceInfo::from_row(&object(json!({ "id": "fleet:abc", "phase": "Bound" })));
        assert!(local.is_ready() && fleet.is_ready());
        assert_eq!(local.raw_phase, "running");
        assert_eq!(fleet.raw_phase, "Bound");
        assert_eq!(local.provider, SpaceProvider::Local);
        assert_eq!(fleet.provider, SpaceProvider::Fleet);
        assert_eq!(local.provider.home(), "/Users/lume");
        assert_eq!(fleet.provider.home(), "/root");
    }

    /// §7: two spellings for the same id. Reading only one yields an empty id
    /// for every window, silently.
    #[test]
    fn every_window_id_spelling_is_read_and_one_is_published() {
        for key in ["window", "window_id", "id"] {
            let window =
                SpaceWindow::from_row(&object(json!({ key: "target-9", "app_name": "Blender" })));
            assert_eq!(window.id, "target-9");
            assert!(window.looks_like_rcdp_target());
        }
    }

    /// §22: the cheap call may omit the tail but never the explanation.
    #[test]
    fn a_missing_reason_is_carried_forward_and_says_so() {
        let rich = RunSnapshot::decode(
            &object(json!({
                "run_id": "run-1", "status": "running", "reason": "compiling",
                "accepts_message": false, "output_tail": "a\nb"
            })),
            "run-1",
            "local:s",
            Some(200),
            None,
        );
        assert_eq!(rich.reason, "compiling");
        assert!(!rich.reason_is_carried_forward);

        let cheap = RunSnapshot::decode(
            &object(json!({ "run_id": "run-1", "status": "running" })),
            "run-1",
            "local:s",
            None,
            Some(&rich),
        );
        assert_eq!(cheap.reason, "compiling");
        assert!(cheap.reason_is_carried_forward);
        // "no output" and "not asked for" stay distinguishable.
        assert_eq!(cheap.output_tail, None);
    }

    /// §23: a full window is the only truncation signal the server gives.
    #[test]
    fn a_full_output_window_is_reported_as_truncated() {
        let tail = (0..10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let snapshot = RunSnapshot::decode(
            &object(json!({ "run_id": "r", "status": "running", "output_tail": tail })),
            "r",
            "s",
            Some(10),
            None,
        );
        assert!(snapshot.output_truncated);
    }

    /// §4: "why" is `note` on one branch and `reason` on the other.
    #[test]
    fn a_delivery_has_one_shape_and_a_reason_on_both_branches() {
        let accepted = Delivery::decode(
            &object(json!({ "delivered": true, "run_id": "r", "note": "queued to the pty" })),
            "r",
            None,
        );
        assert!(accepted.accepted && accepted.reason == "queued to the pty");
        assert_eq!(accepted.state_at_refusal, None);

        let refused = Delivery::decode(
            &object(
                json!({ "delivered": false, "run_id": "r", "status": "running", "reason": "busy" }),
            ),
            "r",
            None,
        );
        assert!(!refused.accepted && refused.reason == "busy");
        assert_eq!(refused.state_at_refusal, Some(AgentState::Running));
        assert!(refused.required().is_err());
    }

    #[test]
    fn a_stop_that_could_not_probe_is_not_flattened_into_alive() {
        let probed = StopOutcome::decode(&object(json!({ "stopped": true, "alive": false })));
        let unprobed = StopOutcome::decode(&object(json!({ "stopped": true })));
        assert_eq!(probed.alive, Some(false));
        assert_eq!(unprobed.alive, None);
    }

    #[test]
    fn a_single_url_and_structured_fields_both_yield_an_endpoint() {
        let from_url = StreamEndpoint::from_row(
            &object(json!({ "ws": "ws://192.168.64.7:8765/ws?token=abc123&x=1" })),
            None,
        );
        assert_eq!(from_url.host, "192.168.64.7");
        assert_eq!(from_url.port, 8765);
        assert_eq!(from_url.token, "abc123");

        let structured = StreamEndpoint::from_row(
            &object(json!({ "host": "10.0.0.1", "port": 9000, "token": "t", "driver_port": 8801 })),
            None,
        );
        assert_eq!(
            (structured.host.as_str(), structured.port),
            ("10.0.0.1", 9000)
        );
    }

    /// The honesty booleans, as one test, so a refactor cannot quietly flip
    /// one to `true` or drop it.
    #[test]
    fn the_honesty_flags_read_what_the_backends_actually_do() {
        assert!(!scheduler_facts().is_server_backed);
        assert!(!TransferLimits::conservative_default().is_server_published);
        const { assert!(!APPROVALS_ARE_ENFORCED) };
        for provider in [
            SpaceProvider::Local,
            SpaceProvider::Fleet,
            SpaceProvider::Demo,
            SpaceProvider::Unknown,
        ] {
            assert!(
                !provider.capabilities().server_backstop,
                "{provider:?} claims a server backstop that does not exist"
            );
        }
        assert!(agent_kind("claude-code").is_production_ready);
        assert!(agent_kind("codex").is_production_ready);
        assert!(!agent_kind("cursor").is_production_ready);
    }

    #[test]
    fn nothing_the_core_produces_is_inferred() {
        let snapshot = RunSnapshot::decode(
            &object(json!({
                "run_id": "r", "status": "finished", "exit_code": 0,
                "reason": "done", "output_tail": "one\ntwo\n"
            })),
            "r",
            "s",
            None,
            None,
        );
        let events = events_from_snapshot(&snapshot);
        assert!(events.iter().all(|event| !event.is_inferred));
        assert!(events.iter().all(|event| !event.kind.is_inferred_only()));
        assert_eq!(events.last().unwrap().kind, AgentEventKind::Finished);
        assert_eq!(events.last().unwrap().exit_code, Some(0));
    }

    /// §14: per-file size is tested before the running total, so the user is
    /// pointed at the right file.
    #[test]
    fn an_oversized_file_is_reported_as_oversized_and_not_as_a_full_batch() {
        let limits = TransferLimits {
            max_file_count: 6,
            max_bytes_per_file: 100,
            max_bytes_per_batch: 150,
            is_server_published: false,
        };
        let admission = limits.admit(&[
            TransferCandidate {
                name: "a".into(),
                byte_count: 80,
            },
            TransferCandidate {
                name: "big".into(),
                byte_count: 500,
            },
            TransferCandidate {
                name: "c".into(),
                byte_count: 90,
            },
        ]);
        assert_eq!(admission.accepted.len(), 1);
        assert_eq!(admission.rejected.len(), 2);
        assert!(
            admission.rejected[0]
                .violation
                .contains("over the 100 byte limit")
        );
        assert!(admission.rejected[1].violation.contains("bytes total"));
    }
}

/// Options for [`crate::client::Connection::create_space`]: the
/// `create_space` tool's arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateSpaceOptions {
    /// Image (default: the canonical Linux image).
    pub image: Option<String>,
    /// Where: `local` (free) or `cloud` (metered). Unset: the user default.
    pub on: Option<String>,
    /// `auto` (default), `container` or `vm`.
    pub kind: Option<String>,
    /// `auto` (default) or an engine the location offers for the kind.
    pub runtime: Option<String>,
    /// Name (default `space-<hex>`).
    pub name: Option<String>,
    /// Return a reachable registered Space in that location instead.
    pub reuse: bool,
    /// Wait until ready (default true).
    pub wait: Option<bool>,
}
