// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua.env.v1.PresenceService`: who is here and where their cursors are.
//!
//! Wire semantics: `libs/cua/proto/presence.proto` and
//! `libs/cua/proto/PRESENCE.md`.
//!
//! - Colors are assigned by the server: a requested color is kept only when
//!   no other participant uses it, otherwise the next free palette color.
//! - Cursors travel outside the video. Each participant's cursor is coalesced
//!   to the newest position (latest wins) and broadcast at most once per
//!   [`CURSOR_INTERVAL`] tick. Senders never get their own position back.
//! - Participants that join with `cursor_shapes` get the server-computed
//!   shape of every cursor (their own included, via `CursorShapeChanged`);
//!   the [`super::cursor_shape::ShapeProber`] computes it off the hub lock.
//! - `cursor_batches` folds one tick's moves into one `CursorBatch`;
//!   `roster_interval` adds `RosterHeartbeat`s; `cursor_datagrams` moves
//!   cursors to the QUIC presence channel ([`super::presence_quic`]).
//! - Agents appear as participants of kind `AGENT` when they drive input;
//!   they leave when their driver session or run ends (`RUN_ENDED`) or after
//!   [`AGENT_IDLE`] without activity (`TIMEOUT`).
//! - A human cursor that has not moved for [`HUMAN_IDLE`] is broadcast
//!   hidden. A subscriber whose queue stays full for [`STALLED`] is dropped.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1::{
    join_response, presence_service_server::PresenceService, CursorBatch, CursorMoved,
    CursorPosition, CursorShape, CursorShapeChanged, CursorShapeSource, JoinRequest, JoinResponse,
    KeepAlive, LeaveReason, LeaveRequest, LeaveResponse, Participant, ParticipantCursor,
    ParticipantLeft, Point, PresenceDatagrams, PresenceJoined, Principal, PrincipalKind,
    RosterHeartbeat, UpdateCursorRequest, UpdateCursorResponse, WindowRef,
};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use super::caller_principal;
use super::status::{invalid, not_found};
use super::{random_id, DesktopState};

/// Distinct cursor colors, in assignment order.
pub const PALETTE: &[&str] = &[
    "#e6194b", "#3cb44b", "#4363d8", "#f58231", "#911eb4", "#46f0f0", "#f032e6", "#bcf60c",
    "#008080", "#9a6324", "#800000", "#000075",
];
/// The `Join` stream's cursor tick (20 Hz).
pub const CURSOR_INTERVAL: Duration = Duration::from_millis(50);
/// Agents without cursor activity for this long leave (`TIMEOUT`). The same
/// constant fades cua-driver's in-guest overlay cursor, so both hide an idle
/// agent together.
pub const AGENT_IDLE: Duration = cua_driver_core::agent_cursor::AGENT_CURSOR_IDLE_TIMEOUT;
/// A human cursor with no update for this long is broadcast hidden.
pub const HUMAN_IDLE: Duration = Duration::from_secs(60);
/// A subscriber whose queue stays full this long is dropped.
pub const STALLED: Duration = Duration::from_secs(5);
/// Shortest honored `roster_interval`.
pub const MIN_ROSTER_INTERVAL: Duration = Duration::from_secs(1);
/// A datagram ticket must be used within this long.
pub const DATAGRAM_TICKET_TTL: Duration = Duration::from_secs(60);
const SUBSCRIBER_QUEUE: usize = 256;

/// Hub timings, overridable in tests.
#[derive(Debug, Clone, Copy)]
pub struct HubTimings {
    pub cursor_interval: Duration,
    pub agent_idle: Duration,
    pub human_idle: Duration,
    pub stalled: Duration,
}

impl Default for HubTimings {
    fn default() -> Self {
        Self {
            cursor_interval: CURSOR_INTERVAL,
            agent_idle: AGENT_IDLE,
            human_idle: HUMAN_IDLE,
            stalled: STALLED,
        }
    }
}

/// What a participant opted into when it joined.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JoinOptions {
    /// `JoinRequest.cursor_shapes`.
    pub cursor_shapes: bool,
    /// `JoinRequest.cursor_batches`.
    pub cursor_batches: bool,
    /// `JoinRequest.roster_interval`, clamped.
    pub roster_interval: Option<Duration>,
}

impl JoinOptions {
    /// From a request (clamps the heartbeat interval).
    pub fn from_request(request: &JoinRequest) -> Self {
        Self {
            cursor_shapes: request.cursor_shapes,
            cursor_batches: request.cursor_batches,
            roster_interval: request
                .roster_interval
                .and_then(|duration| Duration::try_from(duration).ok())
                .filter(|duration| !duration.is_zero())
                .map(|duration| duration.max(MIN_ROSTER_INTERVAL)),
        }
    }
}

/// A request for the shape at a participant's cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeRequest {
    pub participant_id: String,
    /// The participant's `Principal.id` (for pointer ownership).
    pub principal_id: String,
    pub cursor: CursorPosition,
}

/// Where the hub sends shape requests (the prober).
pub trait ShapeRequests: Send + Sync {
    fn request(&self, request: ShapeRequest);
}

/// One cursor as the datagram channel sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct DatagramCursor {
    pub slot: u8,
    pub seq: u16,
    pub cursor: CursorPosition,
    pub shape: i32,
    pub shape_source: i32,
    /// When the server received this cursor.
    pub received: SystemTime,
}

/// A participant's view for its datagram channel.
#[derive(Debug, Clone, PartialEq)]
pub struct DatagramView {
    /// The recipient's slot.
    pub you: u8,
    /// Changes whenever the slot table does.
    pub slot_generation: u64,
    /// Slot -> participant id, everyone with a slot.
    pub slots: BTreeMap<u8, String>,
    /// Everyone else's cursor (only those that have one).
    pub cursors: Vec<DatagramCursor>,
}

struct Member {
    participant: Participant,
    sender: Option<mpsc::Sender<JoinResponse>>,
    options: JoinOptions,
    /// Last cursor, without shape fields.
    cursor: Option<CursorPosition>,
    cursor_received: SystemTime,
    cursor_at: Instant,
    shape: i32,
    shape_source: i32,
    pending: bool,
    last_broadcast: Option<Instant>,
    last_activity: Instant,
    agent: bool,
    slot: u8,
    seq: u16,
    uplink_seq: Option<u16>,
    datagram_attached: bool,
    next_heartbeat: Option<Instant>,
    full_since: Option<Instant>,
    closed: bool,
}

impl Member {
    fn wants_shapes(&self) -> bool {
        self.options.cursor_shapes
    }

    /// The cursor as `recipient` may see it (shape fields only for
    /// participants that asked for them).
    fn cursor_for(&self, shapes: bool) -> Option<CursorPosition> {
        self.cursor.clone().map(|mut cursor| {
            if shapes {
                cursor.shape = self.shape;
                cursor.shape_source = self.shape_source;
            }
            cursor
        })
    }

    fn send(&mut self, event: join_response::Event, now: Instant) {
        let Some(sender) = &self.sender else {
            return;
        };
        match sender.try_send(JoinResponse { event: Some(event) }) {
            Ok(()) => self.full_since = None,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.full_since.get_or_insert(now);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => self.closed = true,
        }
    }
}

#[derive(Default)]
struct HubState {
    members: HashMap<String, Member>,
    /// Agent key (principal id) -> participant id.
    agents: HashMap<String, String>,
    tick: u64,
    slot_generation: u64,
    /// Datagram ticket -> (participant id, minted at).
    tickets: HashMap<String, (String, Instant)>,
}

pub struct PresenceHub {
    /// Woken when any cursor changes, so datagram channels can send at the
    /// leading edge instead of waiting for their next tick.
    cursor_changed: tokio::sync::Notify,
    state: Mutex<HubState>,
    flusher: Mutex<bool>,
    shapes: Mutex<Option<Arc<dyn ShapeRequests>>>,
    timings: HubTimings,
    me: Weak<PresenceHub>,
}

impl PresenceHub {
    pub fn new() -> Arc<Self> {
        Self::with_timings(HubTimings::default())
    }

    pub fn with_timings(timings: HubTimings) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            cursor_changed: tokio::sync::Notify::new(),
            state: Mutex::new(HubState::default()),
            flusher: Mutex::new(false),
            shapes: Mutex::new(None),
            timings,
            me: me.clone(),
        })
    }

    /// Resolves when a cursor changes after this call (for datagram
    /// channels).
    pub fn cursor_changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.cursor_changed.notified()
    }

    /// Route shape requests to `sink` (the prober).
    pub fn set_shape_requests(&self, sink: Arc<dyn ShapeRequests>) {
        *self.shapes.lock().unwrap() = Some(sink);
    }

    /// Start the cursor flusher (idempotent; needs a tokio runtime).
    pub fn spawn_flusher(&self) {
        let mut started = self.flusher.lock().unwrap();
        if *started {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        *started = true;
        let hub = self.me.clone();
        let interval = self.timings.cursor_interval;
        handle.spawn(async move {
            // Wake five times per cursor interval: a move that was not due
            // when it arrived goes out as soon as its interval has passed,
            // not at the next whole tick (which made 30 Hz senders arrive
            // 50 to 100 ms apart instead of 50).
            const STEPS: u32 = 5;
            let mut interval = tokio::time::interval(interval / STEPS);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            for step in 0u32.. {
                interval.tick().await;
                let Some(hub) = hub.upgrade() else {
                    return;
                };
                if step % STEPS == 0 {
                    hub.tick();
                } else {
                    hub.flush();
                }
            }
        });
    }

    fn assign_color(state: &HubState, requested: &str) -> String {
        let used: Vec<String> = state
            .members
            .values()
            .map(|member| {
                member
                    .participant
                    .principal
                    .as_ref()
                    .map(|p| p.color.to_lowercase())
                    .unwrap_or_default()
            })
            .collect();
        let requested = requested.to_lowercase();
        let valid = requested.len() == 7
            && requested.starts_with('#')
            && requested[1..].chars().all(|c| c.is_ascii_hexdigit());
        if valid && !used.contains(&requested) {
            return requested;
        }
        PALETTE
            .iter()
            .map(|color| color.to_string())
            .find(|color| !used.contains(color))
            .unwrap_or_else(|| PALETTE[state.members.len() % PALETTE.len()].to_string())
    }

    fn free_slot(state: &HubState) -> u8 {
        (1..=u8::MAX)
            .find(|slot| !state.members.values().any(|member| member.slot == *slot))
            .unwrap_or(0)
    }

    fn broadcast(state: &mut HubState, except: Option<&str>, event: join_response::Event) {
        let now = Instant::now();
        for (id, member) in state.members.iter_mut() {
            if Some(id.as_str()) == except {
                continue;
            }
            member.send(event.clone(), now);
        }
    }

    fn new_member(
        participant: Participant,
        sender: Option<mpsc::Sender<JoinResponse>>,
        options: JoinOptions,
        agent: bool,
        slot: u8,
    ) -> Member {
        let now = Instant::now();
        Member {
            participant,
            sender,
            options,
            cursor: None,
            cursor_received: SystemTime::now(),
            cursor_at: now,
            shape: CursorShape::Unspecified as i32,
            shape_source: CursorShapeSource::Unspecified as i32,
            pending: false,
            last_broadcast: None,
            last_activity: now,
            agent,
            slot,
            seq: 0,
            uplink_seq: None,
            datagram_attached: false,
            next_heartbeat: options.roster_interval.map(|every| now + every),
            full_since: None,
            closed: false,
        }
    }

    /// Join with default options (old clients).
    pub fn join(
        &self,
        principal: Principal,
    ) -> (
        Participant,
        Vec<ParticipantCursor>,
        mpsc::Receiver<JoinResponse>,
    ) {
        self.join_with(principal, JoinOptions::default())
    }

    /// Join; returns the participant, the roster of everyone else, and the
    /// event receiver.
    pub fn join_with(
        &self,
        mut principal: Principal,
        options: JoinOptions,
    ) -> (
        Participant,
        Vec<ParticipantCursor>,
        mpsc::Receiver<JoinResponse>,
    ) {
        self.spawn_flusher();
        let (sender, receiver) = mpsc::channel(SUBSCRIBER_QUEUE);
        let mut state = self.state.lock().unwrap();
        principal.color = Self::assign_color(&state, &principal.color);
        if principal.display_name.is_empty() {
            principal.display_name = principal.id.clone();
        }
        let participant = Participant {
            participant_id: random_id("p"),
            principal: Some(principal),
            joined_at: Some(super::timestamp(SystemTime::now())),
        };
        let roster = state
            .members
            .values()
            .map(|member| ParticipantCursor {
                participant: Some(member.participant.clone()),
                cursor: member.cursor_for(options.cursor_shapes),
            })
            .collect();
        Self::broadcast(
            &mut state,
            None,
            join_response::Event::ParticipantJoined(participant.clone()),
        );
        let slot = Self::free_slot(&state);
        state.members.insert(
            participant.participant_id.clone(),
            Self::new_member(participant.clone(), Some(sender), options, false, slot),
        );
        state.slot_generation += 1;
        (participant, roster, receiver)
    }

    /// Remove a participant and tell everyone why.
    pub fn leave(&self, participant_id: &str, reason: LeaveReason) -> bool {
        let mut state = self.state.lock().unwrap();
        Self::leave_locked(&mut state, participant_id, reason)
    }

    fn leave_locked(state: &mut HubState, participant_id: &str, reason: LeaveReason) -> bool {
        let removed = state.members.remove(participant_id).is_some();
        state.agents.retain(|_, id| id != participant_id);
        state.tickets.retain(|_, (id, _)| id != participant_id);
        if removed {
            state.slot_generation += 1;
            Self::broadcast(
                state,
                None,
                join_response::Event::ParticipantLeft(ParticipantLeft {
                    participant_id: participant_id.to_owned(),
                    reason: reason as i32,
                }),
            );
        }
        removed
    }

    fn any_wants_shapes(state: &HubState) -> bool {
        state.members.values().any(Member::wants_shapes)
    }

    fn request_shape(&self, request: Option<ShapeRequest>) {
        let Some(request) = request else {
            return;
        };
        let sink = self.shapes.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink.request(request);
        }
    }

    fn shape_request(state: &HubState, participant_id: &str) -> Option<ShapeRequest> {
        if !Self::any_wants_shapes(state) {
            return None;
        }
        let member = state.members.get(participant_id)?;
        let cursor = member.cursor.clone()?;
        if !cursor.visible {
            return None;
        }
        Some(ShapeRequest {
            participant_id: participant_id.to_owned(),
            principal_id: member
                .participant
                .principal
                .as_ref()
                .map(|p| p.id.clone())
                .unwrap_or_default(),
            cursor,
        })
    }

    fn store_cursor(&self, member: &mut Member, mut cursor: CursorPosition) {
        self.cursor_changed.notify_waiters();
        cursor.shape = CursorShape::Unspecified as i32;
        cursor.shape_source = CursorShapeSource::Unspecified as i32;
        member.cursor = Some(cursor);
        member.cursor_received = SystemTime::now();
        member.cursor_at = Instant::now();
        member.last_activity = member.cursor_at;
        member.pending = true;
        member.seq = member.seq.wrapping_add(1);
    }

    pub fn update_cursor(&self, participant_id: &str, cursor: CursorPosition) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(member) = state.members.get_mut(participant_id) else {
            return false;
        };
        self.store_cursor(member, cursor);
        let request = Self::shape_request(&state, participant_id);
        drop(state);
        self.request_shape(request);
        self.flush();
        true
    }

    /// Publish an agent's cursor, creating the agent participant on first
    /// use. `normalized` is relative to `window` when given, else the display.
    pub fn agent_cursor(
        &self,
        agent_key: &str,
        name: &str,
        display_id: &str,
        window: Option<(String, u64)>,
        normalized: (f64, f64),
    ) {
        self.spawn_flusher();
        let cursor = CursorPosition {
            display_id: display_id.to_owned(),
            window: window.map(|(id, epoch)| WindowRef { id, epoch }),
            position: Some(Point {
                x: normalized.0.clamp(0.0, 1.0),
                y: normalized.1.clamp(0.0, 1.0),
            }),
            visible: true,
            ..CursorPosition::default()
        };
        let mut state = self.state.lock().unwrap();
        let participant_id = match state.agents.get(agent_key) {
            Some(id) if state.members.contains_key(id) => id.clone(),
            _ => {
                let principal = Principal {
                    id: agent_key.to_owned(),
                    display_name: name.to_owned(),
                    color: Self::assign_color(&state, ""),
                    kind: PrincipalKind::Agent as i32,
                };
                let participant = Participant {
                    participant_id: random_id("agent"),
                    principal: Some(principal),
                    joined_at: Some(super::timestamp(SystemTime::now())),
                };
                Self::broadcast(
                    &mut state,
                    None,
                    join_response::Event::ParticipantJoined(participant.clone()),
                );
                let id = participant.participant_id.clone();
                let slot = Self::free_slot(&state);
                state.members.insert(
                    id.clone(),
                    Self::new_member(participant, None, JoinOptions::default(), true, slot),
                );
                state.slot_generation += 1;
                state.agents.insert(agent_key.to_owned(), id.clone());
                id
            }
        };
        if let Some(member) = state.members.get_mut(&participant_id) {
            self.store_cursor(member, cursor);
        }
        let request = Self::shape_request(&state, &participant_id);
        drop(state);
        self.request_shape(request);
        self.flush();
    }

    /// The agent run or driver session behind `agent_key` ended: its cursor
    /// leaves now (`RUN_ENDED`).
    ///
    /// cua-driver scopes cursor ids by runtime generation
    /// (`__cua_runtime_<gen>:<session>`) while its session-end hook reports
    /// the bare session, so `cua-driver:<session>` also matches a scoped key
    /// with that session as its last segment.
    pub fn agent_ended(&self, agent_key: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let scoped = agent_key
            .split_once(':')
            .map(|(prefix, session)| (format!("{prefix}:"), format!(":{session}")));
        let ids: Vec<String> = state
            .agents
            .iter()
            .filter(|(key, _)| {
                key.as_str() == agent_key
                    || scoped.as_ref().is_some_and(|(prefix, suffix)| {
                        key.starts_with(prefix.as_str()) && key.ends_with(suffix.as_str())
                    })
            })
            .map(|(_, id)| id.clone())
            .collect();
        let mut left = false;
        for id in ids {
            left |= Self::leave_locked(&mut state, &id, LeaveReason::RunEnded);
        }
        left
    }

    /// Record a computed shape; tells everyone who asked for shapes (the
    /// owner included) when it changed. Returns whether it changed.
    pub fn set_shape(
        &self,
        participant_id: &str,
        shape: CursorShape,
        source: CursorShapeSource,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(member) = state.members.get_mut(participant_id) else {
            return false;
        };
        if member.shape == shape as i32 && member.shape_source == source as i32 {
            return false;
        }
        member.shape = shape as i32;
        member.shape_source = source as i32;
        member.seq = member.seq.wrapping_add(1);
        let event = join_response::Event::CursorShapeChanged(CursorShapeChanged {
            participant_id: participant_id.to_owned(),
            shape: shape as i32,
            source: source as i32,
            at: Some(super::timestamp(SystemTime::now())),
        });
        let now = Instant::now();
        for member in state.members.values_mut() {
            if member.wants_shapes() {
                member.send(event.clone(), now);
            }
        }
        true
    }

    /// Broadcast due cursor updates (one tick).
    pub fn flush(&self) {
        self.flush_moves(false);
    }

    /// One flusher tick: moves, heartbeats, idle hiding, expiry.
    pub fn tick(&self) {
        self.flush_moves(true);
    }

    fn flush_moves(&self, full_tick: bool) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        let interval = self.timings.cursor_interval;
        if full_tick {
            state.tick += 1;
            // Hide human cursors idle for too long.
            let human_idle = self.timings.human_idle;
            for member in state.members.values_mut() {
                if member.agent || now.duration_since(member.cursor_at) < human_idle {
                    continue;
                }
                if let Some(cursor) = member.cursor.as_mut().filter(|cursor| cursor.visible) {
                    cursor.visible = false;
                    member.pending = true;
                    member.seq = member.seq.wrapping_add(1);
                    member.cursor_received = SystemTime::now();
                }
            }
        }
        // Moves due this tick: (id, member snapshot for per-recipient
        // shaping).
        let due: Vec<(
            String,
            Option<CursorPosition>,
            Option<CursorPosition>,
            SystemTime,
        )> = state
            .members
            .iter_mut()
            .filter(|(_, member)| {
                member.pending
                    && member
                        .last_broadcast
                        .is_none_or(|last| now.duration_since(last) >= interval)
            })
            .map(|(id, member)| {
                member.pending = false;
                member.last_broadcast = Some(now);
                (
                    id.clone(),
                    member.cursor_for(false),
                    member.cursor_for(true),
                    member.cursor_received,
                )
            })
            .collect();
        let tick = state.tick;
        if !due.is_empty() {
            let sent_at = super::timestamp(SystemTime::now());
            for (recipient_id, recipient) in state.members.iter_mut() {
                if recipient.sender.is_none() || recipient.datagram_attached {
                    continue;
                }
                let shapes = recipient.wants_shapes();
                let moves: Vec<CursorMoved> = due
                    .iter()
                    .filter(|(id, ..)| id != recipient_id)
                    .filter_map(|(id, plain, shaped, received)| {
                        let cursor = if shapes { shaped } else { plain };
                        cursor.clone().map(|cursor| CursorMoved {
                            participant_id: id.clone(),
                            cursor: Some(cursor),
                            at: Some(super::timestamp(*received)),
                        })
                    })
                    .collect();
                if moves.is_empty() {
                    continue;
                }
                if recipient.options.cursor_batches {
                    recipient.send(
                        join_response::Event::CursorBatch(CursorBatch {
                            moves,
                            tick,
                            at: Some(sent_at),
                        }),
                        now,
                    );
                } else {
                    for moved in moves {
                        recipient.send(join_response::Event::CursorMoved(moved), now);
                    }
                }
            }
        }
        if !full_tick {
            return;
        }
        // Heartbeats.
        let mut ids: Vec<String> = state.members.keys().cloned().collect();
        ids.sort();
        let at = super::timestamp(SystemTime::now());
        for member in state.members.values_mut() {
            let (Some(every), Some(next)) = (member.options.roster_interval, member.next_heartbeat)
            else {
                continue;
            };
            if now < next {
                continue;
            }
            member.next_heartbeat = Some(now + every);
            member.send(
                join_response::Event::RosterHeartbeat(RosterHeartbeat {
                    participant_ids: ids.clone(),
                    at: Some(at),
                }),
                now,
            );
        }
        // Expiry.
        let agent_idle = self.timings.agent_idle;
        let stalled = self.timings.stalled;
        let mut leaving: Vec<(String, LeaveReason)> = Vec::new();
        for (id, member) in state.members.iter() {
            if member.agent && now.duration_since(member.last_activity) > agent_idle {
                leaving.push((id.clone(), LeaveReason::Timeout));
            } else if member.closed
                || member
                    .full_since
                    .is_some_and(|since| now.duration_since(since) > stalled)
            {
                leaving.push((id.clone(), LeaveReason::Disconnected));
            }
        }
        state
            .tickets
            .retain(|_, (_, minted)| now.duration_since(*minted) < DATAGRAM_TICKET_TTL);
        for (id, reason) in leaving {
            if let Some(member) = state.members.get_mut(&id) {
                // A stalled subscriber's stream ends.
                member.sender = None;
            }
            Self::leave_locked(&mut state, &id, reason);
        }
    }

    pub fn participant_count(&self) -> usize {
        self.state.lock().unwrap().members.len()
    }

    /// Every present participant's principal.
    pub fn principals(&self) -> Vec<Principal> {
        self.state
            .lock()
            .unwrap()
            .members
            .values()
            .filter_map(|member| member.participant.principal.clone())
            .collect()
    }

    /// The principal of a participant.
    pub fn principal(&self, participant_id: &str) -> Option<Principal> {
        self.state
            .lock()
            .unwrap()
            .members
            .get(participant_id)
            .and_then(|member| member.participant.principal.clone())
    }

    /// Ask the prober for every visible cursor's shape (a shape-aware
    /// participant joined).
    pub fn request_all_shapes(&self) {
        let requests: Vec<ShapeRequest> = {
            let state = self.state.lock().unwrap();
            state
                .members
                .keys()
                .filter_map(|id| Self::shape_request(&state, id))
                .collect()
        };
        for request in requests {
            self.request_shape(Some(request));
        }
    }

    /// Whether a participant has at least one shape-aware subscriber to
    /// serve (for the prober).
    pub fn wants_shapes(&self) -> bool {
        Self::any_wants_shapes(&self.state.lock().unwrap())
    }

    // ── Datagram channel ────────────────────────────────────────────────

    /// A single-use ticket binding a QUIC presence connection to
    /// `participant_id`.
    pub fn mint_datagram_ticket(&self, participant_id: &str) -> String {
        let ticket = random_id("pt");
        self.state
            .lock()
            .unwrap()
            .tickets
            .insert(ticket.clone(), (participant_id.to_owned(), Instant::now()));
        ticket
    }

    /// Consume a ticket and attach the datagram channel. Returns the
    /// participant id.
    pub fn attach_datagram(&self, ticket: &str) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        let (id, minted) = state.tickets.remove(ticket)?;
        if minted.elapsed() >= DATAGRAM_TICKET_TTL {
            return None;
        }
        let member = state.members.get_mut(&id)?;
        member.datagram_attached = true;
        Some(id)
    }

    /// The channel closed: cursor moves go back to the `Join` stream.
    pub fn detach_datagram(&self, participant_id: &str) {
        if let Some(member) = self.state.lock().unwrap().members.get_mut(participant_id) {
            member.datagram_attached = false;
        }
    }

    /// What `participant_id`'s datagram channel should show now; `None` once
    /// it left.
    pub fn datagram_view(&self, participant_id: &str) -> Option<DatagramView> {
        let state = self.state.lock().unwrap();
        let me = state.members.get(participant_id)?;
        let slots = state
            .members
            .iter()
            .filter(|(_, member)| member.slot != 0)
            .map(|(id, member)| (member.slot, id.clone()))
            .collect();
        let cursors = state
            .members
            .iter()
            .filter(|(id, member)| id.as_str() != participant_id && member.slot != 0)
            .filter_map(|(_, member)| {
                member.cursor.clone().map(|cursor| DatagramCursor {
                    slot: member.slot,
                    seq: member.seq,
                    cursor,
                    shape: member.shape,
                    shape_source: member.shape_source,
                    received: member.cursor_received,
                })
            })
            .collect();
        Some(DatagramView {
            you: me.slot,
            slot_generation: state.slot_generation,
            slots,
            cursors,
        })
    }

    /// Apply an uplink record from `participant_id`'s own channel. Keeps the
    /// cursor's current target. Returns false for a stale or duplicate seq.
    pub fn datagram_uplink(
        &self,
        participant_id: &str,
        seq: u16,
        position: (f64, f64),
        visible: bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(member) = state.members.get_mut(participant_id) else {
            return false;
        };
        if member
            .uplink_seq
            .is_some_and(|last| !cua_media_protocol::presence::seq_newer(seq, last))
        {
            return false;
        }
        member.uplink_seq = Some(seq);
        let mut cursor = member.cursor.clone().unwrap_or_default();
        cursor.position = Some(Point {
            x: position.0.clamp(0.0, 1.0),
            y: position.1.clamp(0.0, 1.0),
        });
        cursor.visible = visible;
        self.store_cursor(member, cursor);
        let request = Self::shape_request(&state, participant_id);
        drop(state);
        self.request_shape(request);
        true
    }

    /// The channel's owner moved its cursor to another target.
    pub fn datagram_target(
        &self,
        participant_id: &str,
        display_id: String,
        window: Option<WindowRef>,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(member) = state.members.get_mut(participant_id) else {
            return false;
        };
        let mut cursor = member.cursor.clone().unwrap_or_default();
        cursor.display_id = display_id;
        cursor.window = window;
        member.cursor = Some(cursor);
        true
    }
}

pub(crate) struct Presence(pub Arc<DesktopState>);

/// A Join stream that leaves presence when the client goes away.
struct JoinStream {
    receiver: mpsc::Receiver<JoinResponse>,
    hub: Arc<PresenceHub>,
    participant_id: String,
    keepalive: tokio::time::Interval,
}

impl Stream for JoinStream {
    type Item = Result<JoinResponse, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        match self.receiver.poll_recv(context) {
            Poll::Ready(Some(event)) => return Poll::Ready(Some(Ok(event))),
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        if self.keepalive.poll_tick(context).is_ready() {
            return Poll::Ready(Some(Ok(JoinResponse {
                event: Some(join_response::Event::Keepalive(KeepAlive {})),
            })));
        }
        Poll::Pending
    }
}

impl Drop for JoinStream {
    fn drop(&mut self) {
        self.hub
            .leave(&self.participant_id, LeaveReason::Disconnected);
    }
}

type JoinResponseStream = Pin<Box<dyn Stream<Item = Result<JoinResponse, Status>> + Send>>;

#[tonic::async_trait]
impl PresenceService for Presence {
    type JoinStream = JoinResponseStream;

    async fn join(
        &self,
        request: Request<JoinRequest>,
    ) -> Result<Response<Self::JoinStream>, Status> {
        let metadata_principal = caller_principal(&request);
        let asserted = cua_spacesd_server::caller(&request).asserted;
        let request = request.into_inner();
        let options = JoinOptions::from_request(&request);
        let requested = request
            .principal
            .filter(|principal| !principal.id.is_empty());
        // A relay-asserted caller joins as the account user: the request may
        // pick color and kind, never the identity.
        let principal = match (asserted, metadata_principal, requested) {
            (true, Some(mut asserted), requested) => {
                if let Some(requested) = requested {
                    if !requested.color.is_empty() {
                        asserted.color = requested.color;
                    }
                    if requested.kind != 0 {
                        asserted.kind = requested.kind;
                    }
                }
                Some(asserted)
            }
            (_, metadata, requested) => requested.or(metadata),
        };
        let principal = principal.unwrap_or_else(|| Principal {
            id: random_id("anon"),
            display_name: "Anonymous".into(),
            ..Principal::default()
        });
        let hub = self.0.presence.clone();
        let (participant, roster, receiver) = hub.join_with(principal, options);
        // Shapes of everyone already present, so a late joiner sees them
        // before anyone moves.
        if options.cursor_shapes {
            hub.request_all_shapes();
        }
        let datagrams = if request.cursor_datagrams {
            self.0
                .quic
                .lock()
                .unwrap()
                .clone()
                .map(|quic| PresenceDatagrams {
                    endpoint: Some(cua_proto::env::v1::QuicEndpoint {
                        port: u32::from(quic.port),
                        certificate_sha256: quic.certificate_sha256.clone(),
                        alpn: cua_media_protocol::presence::PRESENCE_ALPN.into(),
                    }),
                    ticket: hub.mint_datagram_ticket(&participant.participant_id),
                    tick: Some(cua_proto::wkt::Duration {
                        seconds: 0,
                        nanos: super::presence_quic::DATAGRAM_TICK.as_nanos() as i32,
                    }),
                })
        } else {
            None
        };
        let keepalive_every = request
            .keepalive_interval
            .and_then(|duration| Duration::try_from(duration).ok())
            .filter(|duration| !duration.is_zero())
            .unwrap_or(Duration::from_secs(30));
        let mut keepalive = tokio::time::interval_at(
            tokio::time::Instant::now() + keepalive_every,
            keepalive_every,
        );
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let first = JoinResponse {
            event: Some(join_response::Event::Joined(PresenceJoined {
                participant: Some(participant.clone()),
                roster,
                datagrams,
            })),
        };
        let stream = JoinStream {
            receiver,
            hub,
            participant_id: participant.participant_id,
            keepalive,
        };
        let stream = tokio_stream::StreamExt::chain(tokio_stream::once(Ok(first)), stream);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn update_cursor(
        &self,
        request: Request<UpdateCursorRequest>,
    ) -> Result<Response<UpdateCursorResponse>, Status> {
        let request = request.into_inner();
        let cursor = request
            .cursor
            .ok_or_else(|| invalid("cursor is required"))?;
        if let Some(point) = &cursor.position {
            if !(0.0..=1.0).contains(&point.x) || !(0.0..=1.0).contains(&point.y) {
                return Err(invalid("cursor positions are normalized to [0, 1]"));
            }
        }
        if !self
            .0
            .presence
            .update_cursor(&request.participant_id, cursor)
        {
            return Err(not_found("unknown participant"));
        }
        Ok(Response::new(UpdateCursorResponse {}))
    }

    async fn leave(
        &self,
        request: Request<LeaveRequest>,
    ) -> Result<Response<LeaveResponse>, Status> {
        let id = request.into_inner().participant_id;
        if let Some(principal) = self.0.presence.principal(&id) {
            self.0.leases.release_principal(&principal.id);
        }
        if !self.0.presence.leave(&id, LeaveReason::Left) {
            return Err(not_found("unknown participant"));
        }
        Ok(Response::new(LeaveResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(id: &str, color: &str) -> Principal {
        Principal {
            id: id.into(),
            display_name: id.into(),
            color: color.into(),
            kind: PrincipalKind::Human as i32,
        }
    }

    fn at(x: f64, y: f64) -> CursorPosition {
        CursorPosition {
            display_id: "0".into(),
            position: Some(Point { x, y }),
            visible: true,
            ..CursorPosition::default()
        }
    }

    /// Every event currently queued, bounded.
    fn drain(rx: &mut mpsc::Receiver<JoinResponse>) -> Vec<join_response::Event> {
        let mut out = Vec::new();
        for _ in 0..1_000 {
            match rx.try_recv() {
                Ok(event) => out.extend(event.event),
                Err(_) => break,
            }
        }
        out
    }

    fn fast() -> HubTimings {
        HubTimings {
            cursor_interval: Duration::from_millis(20),
            agent_idle: Duration::from_millis(150),
            human_idle: Duration::from_millis(150),
            stalled: Duration::from_millis(150),
        }
    }

    #[derive(Default)]
    struct Requests(Mutex<Vec<ShapeRequest>>);

    impl ShapeRequests for Requests {
        fn request(&self, request: ShapeRequest) {
            self.0.lock().unwrap().push(request);
        }
    }

    #[tokio::test]
    async fn colors_are_unique_and_cursors_reach_the_other_participant() {
        let hub = PresenceHub::new();
        let (alice, _, mut alice_rx) = hub.join(principal("alice", "#ff0000"));
        let (bob, roster, mut bob_rx) = hub.join(principal("bob", "#FF0000"));
        let color =
            |participant: &Participant| participant.principal.as_ref().unwrap().color.clone();
        assert_eq!(color(&alice), "#ff0000");
        assert_ne!(color(&bob), color(&alice), "a taken color is replaced");
        assert_eq!(roster.len(), 1);
        let joined = alice_rx.recv().await.unwrap();
        assert!(matches!(
            joined.event,
            Some(join_response::Event::ParticipantJoined(_))
        ));
        hub.update_cursor(&alice.participant_id, at(0.25, 0.5));
        let moved = tokio::time::timeout(Duration::from_secs(2), bob_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match moved.event {
            Some(join_response::Event::CursorMoved(moved)) => {
                assert_eq!(moved.participant_id, alice.participant_id);
                assert_eq!(moved.cursor.unwrap().position.unwrap().x, 0.25);
                assert!(moved.at.is_some(), "the server receive time");
            }
            other => panic!("unexpected {other:?}"),
        }
        // Senders never receive their own cursor.
        assert!(alice_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn cursor_updates_are_coalesced_to_20_hz() {
        let hub = PresenceHub::new();
        let (alice, _, _alice_rx) = hub.join(principal("alice", ""));
        let (_, _, mut bob_rx) = hub.join(principal("bob", ""));
        let started = Instant::now();
        for step in 0..100 {
            hub.update_cursor(&alice.participant_id, at(f64::from(step) / 100.0, 0.0));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
        let mut moves = 0;
        let mut last_x = 0.0;
        for event in drain(&mut bob_rx) {
            if let join_response::Event::CursorMoved(moved) = event {
                moves += 1;
                last_x = moved.cursor.unwrap().position.unwrap().x;
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(
            f64::from(moves) <= elapsed * 20.0 + 2.0,
            "{moves} moves in {elapsed:.2}s"
        );
        assert!(moves >= 3);
        assert!(
            (last_x - 0.99).abs() < 1e-9,
            "the newest position always arrives"
        );
    }

    #[tokio::test]
    async fn agents_join_on_first_cursor_and_are_marked_as_agents() {
        let hub = PresenceHub::new();
        let (_, _, mut rx) = hub.join(principal("alice", ""));
        hub.agent_cursor("agent-1", "Agent", "0", None, (0.5, 0.5));
        let mut saw_agent = false;
        let mut saw_cursor = false;
        for _ in 0..10 {
            let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await
            else {
                break;
            };
            match event.event {
                Some(join_response::Event::ParticipantJoined(participant)) => {
                    saw_agent |= participant.principal.unwrap().kind == PrincipalKind::Agent as i32;
                }
                Some(join_response::Event::CursorMoved(_)) => saw_cursor = true,
                _ => {}
            }
            if saw_agent && saw_cursor {
                break;
            }
        }
        assert!(saw_agent && saw_cursor);
        assert_eq!(hub.participant_count(), 2);
    }

    /// Old clients (no opt-ins) see no shapes, no shape events, no batches
    /// and no heartbeats; shape-aware ones see all of it, the owner included.
    #[tokio::test]
    async fn shapes_reach_only_participants_that_asked_and_the_owner_too() {
        let hub = PresenceHub::with_timings(fast());
        let requests = Arc::new(Requests::default());
        hub.set_shape_requests(requests.clone());
        let (old, _, mut old_rx) = hub.join(principal("old", ""));
        let shapes = JoinOptions {
            cursor_shapes: true,
            ..JoinOptions::default()
        };
        let (alice, _, mut alice_rx) = hub.join_with(principal("alice", ""), shapes);
        let (_bob, _, mut bob_rx) = hub.join_with(principal("bob", ""), shapes);
        hub.update_cursor(&alice.participant_id, at(0.5, 0.5));
        assert_eq!(
            requests.0.lock().unwrap().last().unwrap().participant_id,
            alice.participant_id,
            "a move asks the prober"
        );
        assert!(hub.set_shape(
            &alice.participant_id,
            CursorShape::Text,
            CursorShapeSource::HitTest
        ));
        assert!(
            !hub.set_shape(
                &alice.participant_id,
                CursorShape::Text,
                CursorShapeSource::HitTest
            ),
            "unchanged shapes are not re-sent"
        );
        hub.update_cursor(&alice.participant_id, at(0.6, 0.5));
        tokio::time::sleep(Duration::from_millis(80)).await;
        hub.tick();
        let shape_events = |events: &[join_response::Event]| {
            events
                .iter()
                .filter(|e| matches!(e, join_response::Event::CursorShapeChanged(_)))
                .count()
        };
        let alice_events = drain(&mut alice_rx);
        assert_eq!(
            shape_events(&alice_events),
            1,
            "the owner hears its own shape"
        );
        let bob_events = drain(&mut bob_rx);
        assert_eq!(shape_events(&bob_events), 1);
        let shaped = bob_events.iter().rev().find_map(|e| match e {
            join_response::Event::CursorMoved(m) => m.cursor.clone(),
            _ => None,
        });
        assert_eq!(shaped.unwrap().shape, CursorShape::Text as i32);
        let old_events = drain(&mut old_rx);
        assert_eq!(shape_events(&old_events), 0);
        for event in &old_events {
            match event {
                join_response::Event::CursorMoved(moved) => {
                    let cursor = moved.cursor.as_ref().unwrap();
                    assert_eq!(cursor.shape, 0);
                    assert_eq!(cursor.shape_source, 0);
                }
                join_response::Event::CursorBatch(_) | join_response::Event::RosterHeartbeat(_) => {
                    panic!("old client got {event:?}")
                }
                _ => {}
            }
        }
        let _ = old;
    }

    #[tokio::test]
    async fn no_shape_requests_without_a_shape_aware_participant() {
        let hub = PresenceHub::with_timings(fast());
        let requests = Arc::new(Requests::default());
        hub.set_shape_requests(requests.clone());
        let (alice, _, _rx) = hub.join(principal("alice", ""));
        hub.update_cursor(&alice.participant_id, at(0.5, 0.5));
        assert!(requests.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn batches_carry_one_tick_of_moves() {
        let hub = PresenceHub::with_timings(fast());
        let (a, _, _a_rx) = hub.join(principal("a", ""));
        let (b, _, _b_rx) = hub.join(principal("b", ""));
        let (_, _, mut rx) = hub.join_with(
            principal("c", ""),
            JoinOptions {
                cursor_batches: true,
                ..JoinOptions::default()
            },
        );
        {
            // Both move within one tick; the flusher sends them together.
            let mut state = hub.state.lock().unwrap();
            for id in [&a.participant_id, &b.participant_id] {
                hub.store_cursor(state.members.get_mut(id).unwrap(), at(0.1, 0.2));
            }
        }
        hub.tick();
        let batches: Vec<CursorBatch> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                join_response::Event::CursorBatch(batch) => Some(batch),
                join_response::Event::CursorMoved(_) => panic!("batched clients get no singles"),
                _ => None,
            })
            .collect();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].moves.len(), 2);
        assert!(batches[0].tick > 0);
    }

    /// A 30 Hz sender reaches others about every cursor interval (50 ms),
    /// not every other tick: a move that was not due is sent as soon as its
    /// interval passes.
    #[tokio::test]
    async fn a_30_hz_sender_is_relayed_at_the_cursor_interval() {
        let hub = PresenceHub::new();
        let (alice, _, _a) = hub.join(principal("alice", ""));
        let (_, _, mut rx) = hub.join(principal("bob", ""));
        let mut arrivals = Vec::new();
        let started = Instant::now();
        let mut next = tokio::time::Instant::now();
        for step in 0..45 {
            hub.update_cursor(&alice.participant_id, at(f64::from(step) / 100.0, 0.5));
            next += Duration::from_millis(33);
            // Drain what arrived meanwhile (bounded).
            while let Ok(Some(event)) = tokio::time::timeout_at(next, rx.recv()).await {
                if let Some(join_response::Event::CursorMoved(_)) = event.event {
                    arrivals.push(started.elapsed());
                }
            }
        }
        let gaps: Vec<Duration> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.len() >= 20, "{} moves", arrivals.len());
        let mean = gaps.iter().sum::<Duration>() / gaps.len() as u32;
        // A due move waits at most one flusher step (a fifth of the
        // interval, or the host's timer granularity when that is coarser:
        // about 15.6 ms on Windows) past the interval. Whole-tick flushing
        // waited up to a whole interval (a ~67 ms mean on a 1 ms timer).
        let step = (CURSOR_INTERVAL / 5).max(timer_granularity().await);
        let bound = CURSOR_INTERVAL + step + Duration::from_millis(2);
        assert!(
            mean < bound,
            "mean gap {mean:?}, bound {bound:?} (was ~67 ms with whole-tick flushing)"
        );
    }

    /// How long this host actually sleeps for a 1 ms timer (the median of a
    /// few tries): about 1 ms on Linux and macOS, about 15.6 ms on Windows.
    async fn timer_granularity() -> Duration {
        let mut samples = Vec::new();
        for _ in 0..9 {
            let t = Instant::now();
            tokio::time::sleep(Duration::from_millis(1)).await;
            samples.push(t.elapsed());
        }
        samples.sort();
        samples[samples.len() / 2]
    }

    #[tokio::test]
    async fn heartbeats_list_everyone_at_the_asked_interval() {
        let hub = PresenceHub::with_timings(fast());
        let (me, _, mut rx) = hub.join_with(
            principal("me", ""),
            JoinOptions {
                roster_interval: Some(MIN_ROSTER_INTERVAL),
                ..JoinOptions::default()
            },
        );
        let (other, _, _o) = hub.join(principal("other", ""));
        hub.tick();
        assert!(drain(&mut rx)
            .iter()
            .all(|e| !matches!(e, join_response::Event::RosterHeartbeat(_))));
        // Pretend a second passed.
        hub.state
            .lock()
            .unwrap()
            .members
            .get_mut(&me.participant_id)
            .unwrap()
            .next_heartbeat = Some(Instant::now());
        hub.tick();
        let beats: Vec<RosterHeartbeat> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                join_response::Event::RosterHeartbeat(h) => Some(h),
                _ => None,
            })
            .collect();
        assert_eq!(beats.len(), 1);
        let mut expected = vec![me.participant_id.clone(), other.participant_id.clone()];
        expected.sort();
        assert_eq!(beats[0].participant_ids, expected);
        // Clamped to at least a second.
        let request = JoinRequest {
            roster_interval: Some(cua_proto::wkt::Duration {
                seconds: 0,
                nanos: 10_000_000,
            }),
            ..JoinRequest::default()
        };
        assert_eq!(
            JoinOptions::from_request(&request).roster_interval,
            Some(MIN_ROSTER_INTERVAL)
        );
    }

    fn left(events: &[join_response::Event], id: &str) -> Option<i32> {
        events.iter().find_map(|e| match e {
            join_response::Event::ParticipantLeft(l) if l.participant_id == id => Some(l.reason),
            _ => None,
        })
    }

    #[tokio::test]
    async fn every_leave_says_why() {
        let hub = PresenceHub::with_timings(fast());
        let (_, _, mut rx) = hub.join(principal("watcher", ""));
        let (a, _, _a) = hub.join(principal("a", ""));
        hub.leave(&a.participant_id, LeaveReason::Left);
        // A run ends: its agent cursor leaves at once.
        hub.agent_cursor("cua-driver:agent-run-1", "Agent", "0", None, (0.5, 0.5));
        let agent_id = hub.state.lock().unwrap().agents["cua-driver:agent-run-1"].clone();
        assert!(hub.agent_ended("cua-driver:agent-run-1"));
        assert!(!hub.agent_ended("cua-driver:agent-run-1"), "once");
        // An idle agent times out.
        hub.agent_cursor("agent-2", "Agent 2", "0", None, (0.5, 0.5));
        let idle_id = hub.state.lock().unwrap().agents["agent-2"].clone();
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub.tick();
        let events = drain(&mut rx);
        assert_eq!(
            left(&events, &a.participant_id),
            Some(LeaveReason::Left as i32)
        );
        assert_eq!(left(&events, &agent_id), Some(LeaveReason::RunEnded as i32));
        assert_eq!(left(&events, &idle_id), Some(LeaveReason::Timeout as i32));
        assert_eq!(hub.participant_count(), 1);
    }

    /// cua-driver reports cursor ids scoped by runtime generation but ends
    /// sessions by their bare id: the run's cursor must still leave.
    #[tokio::test]
    async fn a_run_end_matches_a_runtime_scoped_cursor_id() {
        let hub = PresenceHub::new();
        let (_, _, _rx) = hub.join(principal("alice", ""));
        hub.agent_cursor(
            "cua-driver:__cua_runtime_d092:agent-run-1",
            "Agent",
            "0",
            None,
            (0.5, 0.5),
        );
        hub.agent_cursor(
            "cua-driver:__cua_runtime_d092:agent-run-2",
            "Agent",
            "0",
            None,
            (0.2, 0.2),
        );
        assert_eq!(hub.participant_count(), 3);
        assert!(hub.agent_ended("cua-driver:agent-run-1"));
        assert_eq!(hub.participant_count(), 2, "only run 1 left");
        assert!(!hub.agent_ended("cua-driver:agent-run-3"));
    }

    #[tokio::test]
    async fn a_stalled_subscriber_is_dropped() {
        let hub = PresenceHub::with_timings(fast());
        let (_, _, mut watcher) = hub.join(principal("watcher", ""));
        let (stuck, _, stuck_rx) = hub.join(principal("stuck", ""));
        let (mover, _, _m) = hub.join(principal("mover", ""));
        let mut events = Vec::new();
        // Never read `stuck_rx`: fill its queue.
        for step in 0..(SUBSCRIBER_QUEUE + 10) {
            hub.update_cursor(&mover.participant_id, at((step % 100) as f64 / 100.0, 0.0));
            hub.state
                .lock()
                .unwrap()
                .members
                .get_mut(&mover.participant_id)
                .unwrap()
                .last_broadcast = None;
            hub.flush();
            events.extend(drain(&mut watcher));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub.tick();
        events.extend(drain(&mut watcher));
        assert_eq!(
            left(&events, &stuck.participant_id),
            Some(LeaveReason::Disconnected as i32)
        );
        drop(stuck_rx);
    }

    #[tokio::test]
    async fn an_idle_human_cursor_is_hidden_not_removed() {
        let hub = PresenceHub::with_timings(fast());
        let (a, _, _a) = hub.join(principal("a", ""));
        let (_, _, mut rx) = hub.join(principal("b", ""));
        hub.update_cursor(&a.participant_id, at(0.3, 0.3));
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub.tick();
        tokio::time::sleep(Duration::from_millis(30)).await;
        hub.tick();
        let events = drain(&mut rx);
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                join_response::Event::CursorMoved(m) => m.cursor.clone(),
                _ => None,
            })
            .unwrap();
        assert!(!last.visible);
        assert_eq!(hub.participant_count(), 2);
    }

    #[tokio::test]
    async fn datagram_tickets_are_single_use_and_divert_moves_off_the_stream() {
        let hub = PresenceHub::with_timings(fast());
        let (a, _, _a) = hub.join(principal("a", ""));
        let (b, _, mut b_rx) = hub.join(principal("b", ""));
        let ticket = hub.mint_datagram_ticket(&b.participant_id);
        assert_eq!(hub.attach_datagram(&ticket), Some(b.participant_id.clone()));
        assert_eq!(hub.attach_datagram(&ticket), None, "single use");
        drain(&mut b_rx);
        hub.update_cursor(&a.participant_id, at(0.2, 0.4));
        hub.tick();
        assert!(drain(&mut b_rx)
            .iter()
            .all(|e| !matches!(e, join_response::Event::CursorMoved(_))));
        let view = hub.datagram_view(&b.participant_id).unwrap();
        assert_eq!(view.cursors.len(), 1);
        assert_eq!(view.slots.len(), 2);
        assert_ne!(view.you, 0);
        // Uplink: seq-checked, applied like UpdateCursor.
        assert!(hub.datagram_uplink(&b.participant_id, 5, (0.7, 0.1), true));
        assert!(!hub.datagram_uplink(&b.participant_id, 5, (0.9, 0.9), true));
        assert!(!hub.datagram_uplink(&b.participant_id, 4, (0.9, 0.9), true));
        let seen = hub.datagram_view(&a.participant_id).unwrap();
        let cursor = &seen.cursors[0].cursor;
        assert_eq!(cursor.position.as_ref().unwrap().x, 0.7);
        hub.detach_datagram(&b.participant_id);
        hub.update_cursor(&a.participant_id, at(0.3, 0.4));
        tokio::time::sleep(Duration::from_millis(30)).await;
        hub.tick();
        assert!(drain(&mut b_rx)
            .iter()
            .any(|e| matches!(e, join_response::Event::CursorMoved(_))));
    }
}
