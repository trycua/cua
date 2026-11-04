//! Presence: who is looking at a Space and where their cursors are.
//!
//! [`Space::join_presence`] opens `PresenceService.Join` (a server stream)
//! and returns a [`PresenceSession`]. The first event is always the caller's
//! own participant plus the current roster; cursor updates go out with the
//! unary `UpdateCursor` so the same flow works from gRPC-Web, or over the
//! QUIC presence datagram channel when the Space offers one
//! (`libs/cua/proto/PRESENCE.md`).
//!
//! Drawing is the app's; [`view::PresenceView`] is the shared model of what
//! to draw (interpolated, faded, shaped) and [`art`] the shared cursor art.

use crate::error::{Error, Result};
use crate::space::Space;
use cua_spacesd_client::pb;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub mod art;
pub mod view;

pub use view::{CursorShape, Drawable, PresenceView, ShapeSource};

/// A participant as seen in the roster.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Participant {
    /// Server-assigned id (unique per join).
    pub participant_id: String,
    /// Principal id.
    #[serde(default)]
    pub principal_id: String,
    /// Display name.
    #[serde(default)]
    pub display_name: String,
    /// Server-assigned color.
    #[serde(default)]
    pub color: String,
    /// `human` or `agent`.
    #[serde(default)]
    pub kind: String,
}

impl From<pb::Participant> for Participant {
    fn from(p: pb::Participant) -> Self {
        let principal = p.principal.unwrap_or_default();
        Participant {
            participant_id: p.participant_id,
            principal_id: principal.id,
            display_name: principal.display_name,
            color: principal.color,
            kind: match pb::PrincipalKind::try_from(principal.kind).unwrap_or_default() {
                pb::PrincipalKind::Agent => "agent",
                pb::PrincipalKind::Human => "human",
                pb::PrincipalKind::Unspecified => "unspecified",
            }
            .into(),
        }
    }
}

/// A cursor position, normalized to `[0, 1]` within the display or window.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cursor {
    /// Display id (empty = primary).
    #[serde(default)]
    pub display_id: String,
    /// Window handle, when over a window stream.
    #[serde(default)]
    pub window_id: Option<String>,
    /// X in `[0, 1]`.
    pub x: f64,
    /// Y in `[0, 1]`.
    pub y: f64,
    /// Visible.
    #[serde(default = "yes")]
    pub visible: bool,
    /// A button is down (datagram channel only; always false on the stream).
    #[serde(default)]
    pub pressed: bool,
    /// The shape the guest shows here (server-computed; the arrow when the
    /// Space does not report shapes). Ignored when publishing.
    #[serde(default)]
    pub shape: CursorShape,
    /// How `shape` was determined.
    #[serde(default)]
    pub shape_source: ShapeSource,
    /// When the server received it, in milliseconds on the server clock; 0
    /// when unknown. Ignored when publishing.
    #[serde(default)]
    pub at_ms: f64,
    /// When this client received it, in local Unix milliseconds; 0 when
    /// unknown. Ignored when publishing.
    #[serde(default)]
    pub received_ms: f64,
}

fn yes() -> bool {
    true
}

impl From<pb::CursorPosition> for Cursor {
    fn from(c: pb::CursorPosition) -> Self {
        let p = c.position.unwrap_or_default();
        Cursor {
            display_id: c.display_id,
            window_id: c.window.map(|w| w.id),
            x: p.x,
            y: p.y,
            visible: c.visible,
            pressed: false,
            shape: CursorShape::from_wire(c.shape),
            shape_source: ShapeSource::from_wire(c.shape_source),
            at_ms: 0.0,
            received_ms: now_ms(),
        }
    }
}

/// Local Unix time in milliseconds, the clock [`PresenceView`] expects.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1_000.0)
        .unwrap_or(0.0)
}

fn timestamp_ms(t: Option<pbjson_types::Timestamp>) -> f64 {
    t.map(|t| t.seconds as f64 * 1_000.0 + f64::from(t.nanos) / 1e6)
        .unwrap_or(0.0)
}

impl Cursor {
    /// A visible cursor on the primary display.
    pub fn at(x: f64, y: f64) -> Self {
        Cursor {
            display_id: String::new(),
            window_id: None,
            x,
            y,
            visible: true,
            pressed: false,
            shape: CursorShape::Arrow,
            shape_source: ShapeSource::Unspecified,
            at_ms: 0.0,
            received_ms: 0.0,
        }
    }

    /// This cursor hidden (for example the pointer left the viewer).
    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }

    fn to_pb(&self) -> pb::CursorPosition {
        pb::CursorPosition {
            display_id: self.display_id.clone(),
            window: self
                .window_id
                .clone()
                .map(|id| pb::WindowRef { id, epoch: 0 }),
            position: Some(pb::Point {
                x: self.x.clamp(0.0, 1.0),
                y: self.y.clamp(0.0, 1.0),
            }),
            visible: self.visible,
            shape: 0,
            shape_source: 0,
        }
    }

    fn target(&self) -> (String, Option<String>) {
        (self.display_id.clone(), self.window_id.clone())
    }
}

/// One presence event.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresenceEvent {
    /// Someone joined.
    Joined {
        /// Who.
        participant: Participant,
    },
    /// Someone left.
    Left {
        /// Their participant id.
        participant_id: String,
        /// Why: `left`, `disconnected`, `timeout`, `run_ended`, or empty
        /// when the server did not say.
        #[serde(default)]
        reason: String,
    },
    /// Someone's cursor moved. Cursor batches arrive as one of these per
    /// cursor.
    CursorMoved {
        /// Whose.
        participant_id: String,
        /// Where.
        cursor: Cursor,
    },
    /// Someone's cursor shape changed (the caller's own included).
    ShapeChanged {
        /// Whose.
        participant_id: String,
        /// New shape.
        shape: CursorShape,
        /// How it was determined.
        #[serde(default)]
        source: ShapeSource,
    },
    /// Everyone present; anyone not listed has left.
    Heartbeat {
        /// Participant ids present.
        participant_ids: Vec<String>,
    },
    /// Heartbeat.
    KeepAlive,
}

fn leave_reason(v: i32) -> String {
    match pb::LeaveReason::try_from(v).unwrap_or_default() {
        pb::LeaveReason::Left => "left",
        pb::LeaveReason::Disconnected => "disconnected",
        pb::LeaveReason::Timeout => "timeout",
        pb::LeaveReason::RunEnded => "run_ended",
        pb::LeaveReason::Unspecified => "",
    }
    .into()
}

/// The cursor palette, in the server's assignment order (cua-spacesd's
/// `PresenceService` keeps a requested palette color unless another
/// participant already has it).
pub const PALETTE: &[&str] = &[
    "#e6194b", "#3cb44b", "#4363d8", "#f58231", "#911eb4", "#46f0f0", "#f032e6", "#bcf60c",
    "#008080", "#9a6324", "#800000", "#000075",
];

/// The stable presence color of an agent (or any principal): a palette
/// color picked by a hash (FNV-1a, 32-bit) of its id. The same id gets the
/// same color in every SDK, so an app can color a Bot's avatar and the
/// cursor it joins presence with (see [`Identity::agent`]) from one
/// source.
pub fn color_for(id: &str) -> &'static str {
    let mut h: u32 = 0x811c_9dc5;
    for b in id.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

/// Black or white, whichever reads better on `background` (`#rrggbb`), by
/// WCAG contrast. Unparseable input gets black.
pub fn text_color_on(background: &str) -> &'static str {
    let hex = background.trim_start_matches('#');
    let Ok(v) = u32::from_str_radix(hex, 16) else {
        return "#000000";
    };
    if hex.len() != 6 {
        return "#000000";
    }
    let lin = |c: u32| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin((v >> 16) & 0xff) + 0.7152 * lin((v >> 8) & 0xff) + 0.0722 * lin(v & 0xff);
    // Contrast against white is 1.05 / (l + 0.05); against black (l + 0.05) / 0.05.
    if 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 {
        "#ffffff"
    } else {
        "#000000"
    }
}

impl Identity {
    /// An agent's identity, requesting its stable [`color_for`] color.
    pub fn agent(id: &str, display_name: &str) -> Self {
        Identity {
            id: id.into(),
            display_name: display_name.into(),
            color: color_for(id).into(),
            agent: true,
        }
    }
}

/// Who is present and where their cursors are, folded from
/// [`PresenceEvent`]s: the shared presence cursor as data. Drawing is the
/// app's. The same model as `@trycua/cua/spaces/presence`'s
/// `PresenceRoster` and the Swift SDK's `PresenceRoster`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Roster {
    /// The caller's participant id.
    pub me: String,
    /// Everyone, the caller first, then in join order.
    pub entries: Vec<(Participant, Option<Cursor>)>,
}

impl Roster {
    /// A roster seeded from a fresh session: the caller and everyone present
    /// at join.
    pub fn from_session(session: &PresenceSession) -> Self {
        let mut r = Roster {
            me: session.me().participant_id.clone(),
            entries: vec![(session.me().clone(), None)],
        };
        for (p, c) in session.roster() {
            if !r.contains(&p.participant_id) {
                r.entries.push((p.clone(), c.clone()));
            }
        }
        r
    }

    fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|(p, _)| p.participant_id == id)
    }

    /// One participant and their last cursor.
    pub fn get(&self, participant_id: &str) -> Option<&(Participant, Option<Cursor>)> {
        self.entries
            .iter()
            .find(|(p, _)| p.participant_id == participant_id)
    }

    /// The color an agent shows as, for its avatar and its cursor alike: the
    /// color the server assigned once it is present (a requested color is
    /// kept unless another participant already holds it), else its stable
    /// [`color_for`]. Keyed by the id it joined with ([`Identity::id`]).
    pub fn color_of(&self, principal_id: &str) -> String {
        self.entries
            .iter()
            .find(|(p, _)| p.principal_id == principal_id)
            .map(|(p, _)| p.color.clone())
            .unwrap_or_else(|| color_for(principal_id).to_string())
    }

    /// Everyone but the caller: the cursors to draw.
    pub fn others(&self) -> impl Iterator<Item = &(Participant, Option<Cursor>)> {
        self.entries
            .iter()
            .filter(|(p, _)| p.participant_id != self.me)
    }

    /// Folds one event in. Returns whether anything changed.
    pub fn apply(&mut self, e: &PresenceEvent) -> bool {
        match e {
            PresenceEvent::Joined { participant } => {
                if self.contains(&participant.participant_id) {
                    return false;
                }
                self.entries.push((participant.clone(), None));
                true
            }
            PresenceEvent::Left { participant_id, .. } => {
                let before = self.entries.len();
                self.entries
                    .retain(|(p, _)| p.participant_id != *participant_id);
                before != self.entries.len()
            }
            PresenceEvent::CursorMoved {
                participant_id,
                cursor,
            } => match self
                .entries
                .iter_mut()
                .find(|(p, _)| p.participant_id == *participant_id)
            {
                Some((_, c)) => {
                    *c = Some(cursor.clone());
                    true
                }
                None => false,
            },
            PresenceEvent::ShapeChanged {
                participant_id,
                shape,
                source,
            } => match self
                .entries
                .iter_mut()
                .find(|(p, _)| p.participant_id == *participant_id)
            {
                Some((_, Some(c))) => {
                    c.shape = *shape;
                    c.shape_source = *source;
                    true
                }
                _ => false,
            },
            PresenceEvent::Heartbeat { participant_ids } => {
                let before = self.entries.len();
                let me = self.me.clone();
                self.entries.retain(|(p, _)| {
                    p.participant_id == me || participant_ids.contains(&p.participant_id)
                });
                before != self.entries.len()
            }
            PresenceEvent::KeepAlive => false,
        }
    }
}

/// Shortest interval between two cursor sends (30 Hz, newest wins).
pub const SEND_INTERVAL: Duration = Duration::from_millis(33);
/// Roster heartbeat interval the SDK asks for.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// A joined presence session.
pub struct PresenceSession {
    space: Space,
    me: Participant,
    /// Keeps this participant registered as the Space's viewer identity for
    /// media sessions (see [`Space::presence_participant`]) while joined.
    _viewer: JoinedParticipant,
    roster: Vec<(Participant, Option<Cursor>)>,
    stream: tonic::Streaming<pb::JoinResponse>,
    queue: VecDeque<PresenceEvent>,
    sender: CursorSender,
    datagram_events: Option<tokio::sync::mpsc::Receiver<PresenceEvent>>,
    stream_ended: bool,
    left: bool,
}

impl std::fmt::Debug for PresenceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresenceSession")
            .field("me", &self.me)
            .field("roster", &self.roster.len())
            .field("datagrams", &self.sender.uses_datagrams())
            .finish()
    }
}

/// Presence participants this process joined, per Space id, newest last.
fn joined_participants() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static JOINED: std::sync::OnceLock<Mutex<HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    JOINED.get_or_init(Default::default)
}

/// Registers a joined participant for its Space until dropped.
struct JoinedParticipant {
    space: String,
    participant_id: String,
}

impl JoinedParticipant {
    fn register(space: String, participant_id: &str) -> Self {
        if !participant_id.is_empty() {
            joined_participants()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(space.clone())
                .or_default()
                .push(participant_id.to_owned());
        }
        Self {
            space,
            participant_id: participant_id.to_owned(),
        }
    }
}

impl Drop for JoinedParticipant {
    fn drop(&mut self) {
        let mut joined = joined_participants()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ids) = joined.get_mut(&self.space) {
            if let Some(at) = ids.iter().rposition(|id| *id == self.participant_id) {
                ids.remove(at);
            }
            if ids.is_empty() {
                joined.remove(&self.space);
            }
        }
    }
}

impl Space {
    /// The presence participant this process most recently joined this
    /// Space as, while that session is live. Media sessions opened here send
    /// it (`OpenMediaRequest.presence_participant_id`), so the viewer's input
    /// is attributed to its own presence identity: presence shows its own
    /// cursor, never an agent's, and the guest draws no agent cursor for it.
    pub fn presence_participant(&self) -> Option<String> {
        joined_participants()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.id().to_string())
            .and_then(|ids| ids.last().cloned())
    }
}

/// Who is joining.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// Stable principal id (for example a user id or `agent:<name>`).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Requested color (the server may assign another).
    pub color: String,
    /// Agent rather than human.
    pub agent: bool,
}

impl Space {
    /// Joins presence as `identity`. Waits (up to `timeout`) for the first
    /// event, which carries the caller's participant and the roster.
    ///
    /// Asks for server-computed cursor shapes, 5 s roster heartbeats, cursor
    /// batches and (native builds, non-gateway Spaces) the QUIC datagram
    /// channel; a daemon without them ignores the request.
    pub async fn join_presence(
        &self,
        identity: Identity,
        timeout: Duration,
    ) -> Result<PresenceSession> {
        self.join_presence_with(identity, timeout, true).await
    }

    /// [`Space::join_presence`], choosing whether to ask for the QUIC
    /// datagram channel (`datagrams: false` keeps cursors on the `Join`
    /// stream and `UpdateCursor`, as browsers do).
    pub async fn join_presence_with(
        &self,
        identity: Identity,
        timeout: Duration,
        datagrams: bool,
    ) -> Result<PresenceSession> {
        self.require("presence")?;
        let principal = pb::Principal {
            id: identity.id,
            display_name: identity.display_name,
            color: identity.color,
            kind: if identity.agent {
                pb::PrincipalKind::Agent
            } else {
                pb::PrincipalKind::Human
            } as i32,
        };
        // The relay carries streams, not UDP: a relay Space (like a cloud
        // one behind its gateway) keeps cursors on the stream instead of
        // waiting out a datagram channel that cannot open.
        let channel = PRESENCE_DATAGRAMS.get().cloned();
        let want_datagrams = datagrams
            && channel.is_some()
            && !matches!(
                self.provider(),
                crate::Provider::Cloud | crate::Provider::Relay
            );
        let mut stream = self
            .spacesd()?
            .presence()
            .join(pb::JoinRequest {
                principal: Some(principal),
                keepalive_interval: None,
                cursor_shapes: true,
                roster_interval: Some(pbjson_types::Duration {
                    seconds: HEARTBEAT_INTERVAL.as_secs() as i64,
                    nanos: 0,
                }),
                cursor_batches: true,
                cursor_datagrams: want_datagrams,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let first = tokio::time::timeout(timeout, stream.message())
            .await
            .map_err(|_| Error::Timeout("presence join".into()))?
            .map_err(cua_spacesd_client::Error::from)?
            .ok_or_else(|| Error::Stream("presence stream ended before joining".into()))?;
        let Some(pb::join_response::Event::Joined(joined)) = first.event else {
            return Err(Error::Stream(
                "the first presence event was not `joined`".into(),
            ));
        };
        let me: Participant = joined.participant.unwrap_or_default().into();
        let sender = CursorSender::new(self.clone(), me.participant_id.clone());
        let mut datagram_events = None;
        if let (Some(channel), Some(info)) = (
            channel.as_ref(),
            joined.datagrams.as_ref().filter(|_| want_datagrams),
        ) {
            // Best effort: the stream and UpdateCursor carry cursors when the
            // channel cannot be opened (UDP blocked, NAT, older daemon).
            if let Ok((link, events)) = channel.connect(self, info, timeout).await {
                sender.attach(link);
                datagram_events = Some(events);
            }
        }
        let viewer = JoinedParticipant::register(self.id().to_string(), &me.participant_id);
        Ok(PresenceSession {
            space: self.clone(),
            me,
            _viewer: viewer,
            roster: joined
                .roster
                .into_iter()
                .map(|r| {
                    (
                        r.participant.unwrap_or_default().into(),
                        r.cursor.map(Cursor::from),
                    )
                })
                .collect(),
            stream,
            queue: VecDeque::new(),
            sender,
            datagram_events,
            stream_ended: false,
            left: false,
        })
    }

    /// Sets this Space's presence settings (`SystemService.Init`).
    /// `cursor_probe`: whether cua-spacesd may read the real cursor shape by
    /// briefly moving the idle guest pointer to a participant's position
    /// (on by default); `None` leaves it unchanged.
    pub async fn set_presence_settings(&self, cursor_probe: Option<bool>) -> Result<()> {
        self.spacesd()?
            .system()
            .init(pb::InitRequest {
                presence: Some(pb::PresenceSettings { cursor_probe }),
                ..Default::default()
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }
}

/// The presence datagram channel (`cua-presence/1` over QUIC,
/// `libs/cua/proto/PRESENCE.md` section 6): cursors as latest-wins
/// datagrams. It ships with Cua Spaces (source-available, FSL-1.1-MIT, in
/// `cua-spaces-ext`) and is registered with [`register_presence_datagrams`];
/// without it cursors ride the Join stream and `UpdateCursor`.
pub trait PresenceDatagrams: Send + Sync + 'static {
    /// Opens the channel `info` describes on `space` and waits (up to
    /// `timeout`) for it. Returns the uplink and the stream of cursor
    /// events.
    fn connect<'a>(
        &'a self,
        space: &'a Space,
        info: &'a pb::PresenceDatagrams,
        timeout: Duration,
    ) -> crate::extension::BoxFuture<'a, Result<DatagramChannel>>;
}

/// An open presence datagram channel: the uplink and the cursor events.
pub type DatagramChannel = (
    Arc<dyn CursorUplink>,
    tokio::sync::mpsc::Receiver<PresenceEvent>,
);

static PRESENCE_DATAGRAMS: std::sync::OnceLock<Arc<dyn PresenceDatagrams>> =
    std::sync::OnceLock::new();

/// Registers the presence datagram channel for this process (once; later
/// calls are ignored). Cua Spaces registers its own.
pub fn register_presence_datagrams(channel: Arc<dyn PresenceDatagrams>) {
    let _ = PRESENCE_DATAGRAMS.set(channel);
}

/// Where outgoing cursors go: the datagram channel when attached, else
/// `PresenceService.UpdateCursor`.
#[async_trait::async_trait]
pub trait CursorUplink: Send + Sync {
    /// Sends one cursor; `Ok(false)` when this uplink cannot (fall back).
    async fn send(&self, cursor: &Cursor) -> Result<bool>;
}

#[derive(Default)]
struct SendState {
    last_sent: Option<Instant>,
    last_visible: Option<bool>,
    last_target: Option<(String, Option<String>)>,
    pending: Option<Cursor>,
    flush_scheduled: bool,
    error: Option<String>,
}

/// Publishes the caller's cursor at most every [`SEND_INTERVAL`], newest
/// wins: a move inside the interval is held and sent at its end (so the
/// final position of a movement always goes out), and show, hide and
/// target changes go out at once. Cloneable and independent of the event
/// stream, so publishing never waits for `next_event`.
#[derive(Clone)]
pub struct CursorSender {
    space: Space,
    participant_id: String,
    state: Arc<Mutex<SendState>>,
    uplink: Arc<Mutex<Option<Arc<dyn CursorUplink>>>>,
    interval: Duration,
}

impl CursorSender {
    fn new(space: Space, participant_id: String) -> Self {
        CursorSender {
            space,
            participant_id,
            state: Arc::default(),
            uplink: Arc::default(),
            interval: SEND_INTERVAL,
        }
    }

    pub(crate) fn attach(&self, uplink: Arc<dyn CursorUplink>) {
        *self.uplink.lock().unwrap() = Some(uplink);
    }

    /// Whether cursors go over the datagram channel.
    pub fn uses_datagrams(&self) -> bool {
        self.uplink.lock().unwrap().is_some()
    }

    /// Publishes `cursor` (newest wins, see the type docs). Returns the error
    /// of a send that failed in the background since the last call, if any.
    pub async fn update(&self, cursor: &Cursor) -> Result<()> {
        let now = Instant::now();
        let send_now = {
            let mut st = self.state.lock().unwrap();
            if let Some(e) = st.error.take() {
                return Err(Error::Stream(e));
            }
            let edge = st.last_visible != Some(cursor.visible)
                || st.last_target.as_ref() != Some(&cursor.target());
            let due = st
                .last_sent
                .is_none_or(|t| now.duration_since(t) >= self.interval);
            if edge || due {
                st.pending = None;
                st.last_sent = Some(now);
                st.last_visible = Some(cursor.visible);
                st.last_target = Some(cursor.target());
                true
            } else {
                st.pending = Some(cursor.clone());
                if !st.flush_scheduled {
                    st.flush_scheduled = true;
                    let wait = self.interval - now.duration_since(st.last_sent.unwrap_or(now));
                    let me = self.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(wait).await;
                        me.flush().await;
                    });
                }
                false
            }
        };
        if send_now {
            self.send(cursor).await?;
        }
        Ok(())
    }

    async fn flush(&self) {
        let pending = {
            let mut st = self.state.lock().unwrap();
            st.flush_scheduled = false;
            let p = st.pending.take();
            if let Some(c) = &p {
                st.last_sent = Some(Instant::now());
                st.last_visible = Some(c.visible);
                st.last_target = Some(c.target());
            }
            p
        };
        if let Some(c) = pending
            && let Err(e) = self.send(&c).await
        {
            self.state.lock().unwrap().error = Some(e.to_string());
        }
    }

    async fn send(&self, cursor: &Cursor) -> Result<()> {
        let uplink = self.uplink.lock().unwrap().clone();
        if let Some(uplink) = uplink
            && uplink.send(cursor).await?
        {
            return Ok(());
        }
        self.space
            .spacesd()?
            .presence()
            .update_cursor(pb::UpdateCursorRequest {
                participant_id: self.participant_id.clone(),
                cursor: Some(cursor.to_pb()),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }
}

fn map_moved(c: pb::CursorMoved) -> PresenceEvent {
    let mut cursor: Cursor = c.cursor.unwrap_or_default().into();
    cursor.at_ms = timestamp_ms(c.at);
    PresenceEvent::CursorMoved {
        participant_id: c.participant_id,
        cursor,
    }
}

impl PresenceSession {
    /// The caller.
    pub fn me(&self) -> &Participant {
        &self.me
    }

    /// Everyone else present when this session joined.
    pub fn roster(&self) -> &[(Participant, Option<Cursor>)] {
        &self.roster
    }

    /// Whether cursors travel over the QUIC datagram channel.
    pub fn uses_datagrams(&self) -> bool {
        self.sender.uses_datagrams()
    }

    /// A [`PresenceView`] seeded with the caller and the roster at join,
    /// with the render delay matching this session's transport.
    pub fn view(&self) -> PresenceView {
        let delay = if self.uses_datagrams() {
            view::DATAGRAM_DELAY_MS
        } else {
            view::STREAM_DELAY_MS
        };
        let mut v = PresenceView::new(&self.me.participant_id, delay);
        v.upsert(self.me.clone());
        let now = now_ms();
        for (p, c) in &self.roster {
            v.upsert(p.clone());
            if let Some(c) = c {
                v.apply(
                    &PresenceEvent::CursorMoved {
                        participant_id: p.participant_id.clone(),
                        cursor: c.clone(),
                    },
                    now,
                );
            }
        }
        v
    }

    /// A handle that publishes the caller's cursor without borrowing the
    /// session (so a render loop can publish while another task reads
    /// events).
    pub fn sender(&self) -> CursorSender {
        self.sender.clone()
    }

    /// Publishes the caller's cursor (throttled, newest wins; see
    /// [`CursorSender`]).
    pub async fn update_cursor(&self, cursor: &Cursor) -> Result<()> {
        self.sender.update(cursor).await
    }

    fn enqueue(&mut self, event: pb::join_response::Event) {
        use pb::join_response::Event as E;
        match event {
            E::Joined(j) => self.queue.push_back(PresenceEvent::Joined {
                participant: j.participant.unwrap_or_default().into(),
            }),
            E::ParticipantJoined(p) => self.queue.push_back(PresenceEvent::Joined {
                participant: p.into(),
            }),
            E::ParticipantLeft(l) => self.queue.push_back(PresenceEvent::Left {
                participant_id: l.participant_id,
                reason: leave_reason(l.reason),
            }),
            E::CursorMoved(c) => self.queue.push_back(map_moved(c)),
            E::CursorBatch(b) => {
                for c in b.moves {
                    self.queue.push_back(map_moved(c));
                }
            }
            E::CursorShapeChanged(c) => self.queue.push_back(PresenceEvent::ShapeChanged {
                participant_id: c.participant_id,
                shape: CursorShape::from_wire(c.shape),
                source: ShapeSource::from_wire(c.source),
            }),
            E::RosterHeartbeat(h) => self.queue.push_back(PresenceEvent::Heartbeat {
                participant_ids: h.participant_ids,
            }),
            E::Keepalive(_) => self.queue.push_back(PresenceEvent::KeepAlive),
        }
    }

    /// The next event, or `None` when the stream ended. Fails with
    /// [`Error::Timeout`] after `timeout` without one.
    pub async fn next_event(&mut self, timeout: Duration) -> Result<Option<PresenceEvent>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(e) = self.queue.pop_front() {
                return Ok(Some(e));
            }
            if self.stream_ended {
                return Ok(None);
            }
            let datagram = async {
                match self.datagram_events.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                msg = self.stream.message() => {
                    match msg.map_err(cua_spacesd_client::Error::from)? {
                        Some(m) => {
                            if let Some(e) = m.event {
                                self.enqueue(e);
                            }
                        }
                        None => self.stream_ended = true,
                    }
                }
                e = datagram => {
                    match e {
                        Some(e) => return Ok(Some(e)),
                        // The channel closed: cursors fall back to the stream.
                        None => {
                            self.datagram_events = None;
                            *self.sender.uplink.lock().unwrap() = None;
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(Error::Timeout("waiting for a presence event".into()));
                }
            }
        }
    }

    /// Waits for the first event matching `pred`, skipping others, for at
    /// most `max_events` events or `timeout` in total.
    pub async fn wait_for(
        &mut self,
        timeout: Duration,
        max_events: usize,
        mut pred: impl FnMut(&PresenceEvent) -> bool,
    ) -> Result<PresenceEvent> {
        let deadline = tokio::time::Instant::now() + timeout;
        for _ in 0..max_events {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_event(left).await? {
                Some(e) if pred(&e) => return Ok(e),
                Some(_) => continue,
                None => return Err(Error::Stream("presence stream ended".into())),
            }
        }
        Err(Error::Timeout(format!(
            "no matching presence event in {max_events} events"
        )))
    }

    /// Leaves presence.
    pub async fn leave(mut self) -> Result<()> {
        self.left = true;
        self.space
            .spacesd()?
            .presence()
            .leave(pb::LeaveRequest {
                participant_id: self.me.participant_id.clone(),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }
}

#[cfg(test)]
mod roster_tests {
    use super::*;

    fn p(id: &str, kind: &str) -> Participant {
        Participant {
            participant_id: id.into(),
            principal_id: format!("u-{id}"),
            display_name: id.to_uppercase(),
            color: "#123456".into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn roster_follows_join_cursor_and_leave() {
        let mut r = Roster {
            me: "me".into(),
            entries: vec![(p("me", "human"), None)],
        };
        assert!(r.apply(&PresenceEvent::Joined {
            participant: p("koala", "agent")
        }));
        assert!(!r.apply(&PresenceEvent::Joined {
            participant: p("koala", "agent")
        }));
        assert!(r.apply(&PresenceEvent::CursorMoved {
            participant_id: "koala".into(),
            cursor: Cursor::at(0.25, 0.75),
        }));
        assert!(!r.apply(&PresenceEvent::CursorMoved {
            participant_id: "ghost".into(),
            cursor: Cursor::at(0.0, 0.0),
        }));
        let others: Vec<_> = r.others().collect();
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].1, Some(Cursor::at(0.25, 0.75)));
        assert!(!r.apply(&PresenceEvent::KeepAlive));
        assert!(r.apply(&PresenceEvent::Left {
            participant_id: "koala".into(),
            reason: String::new(),
        }));
        assert!(r.get("koala").is_none());
        assert_eq!(r.entries.len(), 1);
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn colors_are_stable_palette_colors() {
        // Pinned: the TypeScript and Swift SDKs must agree.
        assert_eq!(
            color_for(""),
            PALETTE[(0x811c_9dc5_u32 as usize) % PALETTE.len()]
        );
        assert_eq!(color_for("koala"), color_for("koala"));
        for id in ["ada", "bo", "koala", "openkoalabots-routine-1"] {
            assert!(PALETTE.contains(&color_for(id)));
        }
        assert_eq!(
            ["ada", "bo", "koala", "inbox", "sales"].map(color_for),
            ["#bcf60c", "#4363d8", "#46f0f0", "#bcf60c", "#f58231"]
        );
        let i = Identity::agent("ada", "Ada");
        assert!(i.agent);
        assert_eq!(i.color, color_for("ada"));
    }

    #[test]
    fn text_contrast() {
        assert_eq!(text_color_on("#000075"), "#ffffff");
        assert_eq!(text_color_on("#bcf60c"), "#000000");
        assert_eq!(text_color_on("#46f0f0"), "#000000");
        assert_eq!(text_color_on("#800000"), "#ffffff");
        assert_eq!(text_color_on("nope"), "#000000");
    }
}

#[cfg(test)]
mod color_of_tests {
    use super::*;

    #[test]
    fn the_assigned_color_wins_while_present() {
        let stable = color_for("koala");
        let mut r = Roster {
            me: "me".into(),
            entries: vec![(
                Participant {
                    participant_id: "me".into(),
                    principal_id: "operator".into(),
                    display_name: "Op".into(),
                    color: stable.into(),
                    kind: "human".into(),
                },
                None,
            )],
        };
        assert_eq!(r.color_of("koala"), stable);
        r.apply(&PresenceEvent::Joined {
            participant: Participant {
                participant_id: "k".into(),
                principal_id: "koala".into(),
                display_name: "Koala".into(),
                color: "#123456".into(),
                kind: "agent".into(),
            },
        });
        assert_eq!(r.color_of("koala"), "#123456");
        r.apply(&PresenceEvent::Left {
            participant_id: "k".into(),
            reason: String::new(),
        });
        assert_eq!(r.color_of("koala"), stable);
    }
}
