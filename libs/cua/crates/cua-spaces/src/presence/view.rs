//! The presence netcode model every client draws from: cursor shapes, the
//! per-cursor smoother, and [`PresenceView`], which folds presence events
//! into "what to draw now".
//!
//! Pure and deterministic: every method takes the local time in
//! milliseconds, so the same event sequence gives the same frames in every
//! language. The TypeScript mirror (`@trycua/cua/spaces/presence`) runs the
//! same conformance vectors (`assets/presence-conformance.json`); Swift and
//! the other UniFFI bindings call this code directly.
//!
//! Rules (libs/cua/proto/PRESENCE.md section 4):
//! - your own cursor is drawn at the local pointer, never from the network;
//! - remote cursors render `delay_ms` behind the newest sample (100 ms on the
//!   20 Hz stream, 66 ms on the 30 Hz datagram channel), Catmull-Rom through
//!   the samples when there are at least 4, else linear;
//! - past the newest sample they extrapolate for at most 100 ms, then hold
//!   the newest sample;
//! - a jump over 25% of the surface, a target change or a re-show snaps;
//! - a cursor idle for 5 s fades out over 300 ms;
//! - a participant a heartbeat does not list, or everyone when heartbeats
//!   stop for 3 intervals, is removed.

use std::collections::{BTreeMap, VecDeque};

use super::{Cursor, Participant, PresenceEvent};

/// A pointer shape, the portable vocabulary of `cua.env.v1.CursorShape`.
/// The names are the keys of the shared cursor art.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// The arrow (also what an unknown or unprobed shape draws as).
    #[default]
    Arrow,
    /// I-beam.
    Text,
    /// Pointing hand.
    Pointer,
    /// Vertical resize.
    ResizeNs,
    /// Horizontal resize.
    ResizeEw,
    /// Rising-diagonal resize.
    ResizeNesw,
    /// Falling-diagonal resize.
    ResizeNwse,
    /// Busy.
    Wait,
    /// Working in the background.
    Progress,
    /// Not allowed.
    NotAllowed,
    /// Crosshair.
    Crosshair,
    /// Open hand.
    Grab,
    /// Closed hand.
    Grabbing,
    /// Four-way move.
    Move,
}

impl CursorShape {
    /// Every shape, in wire order.
    pub const ALL: [CursorShape; 14] = [
        CursorShape::Arrow,
        CursorShape::Text,
        CursorShape::Pointer,
        CursorShape::ResizeNs,
        CursorShape::ResizeEw,
        CursorShape::ResizeNesw,
        CursorShape::ResizeNwse,
        CursorShape::Wait,
        CursorShape::Progress,
        CursorShape::NotAllowed,
        CursorShape::Crosshair,
        CursorShape::Grab,
        CursorShape::Grabbing,
        CursorShape::Move,
    ];

    /// From the wire value; unspecified and unknown values are the arrow.
    pub fn from_wire(v: i32) -> Self {
        if (1..=14).contains(&v) {
            Self::ALL[(v - 1) as usize]
        } else {
            CursorShape::Arrow
        }
    }

    /// The wire value.
    pub fn to_wire(self) -> i32 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as i32 + 1
    }

    /// The snake_case name (the art key).
    pub fn as_str(self) -> &'static str {
        match self {
            CursorShape::Arrow => "arrow",
            CursorShape::Text => "text",
            CursorShape::Pointer => "pointer",
            CursorShape::ResizeNs => "resize_ns",
            CursorShape::ResizeEw => "resize_ew",
            CursorShape::ResizeNesw => "resize_nesw",
            CursorShape::ResizeNwse => "resize_nwse",
            CursorShape::Wait => "wait",
            CursorShape::Progress => "progress",
            CursorShape::NotAllowed => "not_allowed",
            CursorShape::Crosshair => "crosshair",
            CursorShape::Grab => "grab",
            CursorShape::Grabbing => "grabbing",
            CursorShape::Move => "move",
        }
    }

    /// From a name; unknown names are the arrow.
    pub fn parse(name: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|s| s.as_str() == name)
            .unwrap_or_default()
    }
}

/// How a shape was determined (`cua.env.v1.CursorShapeSource`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ShapeSource {
    /// Not reported (older daemon, or not probed yet).
    #[default]
    Unspecified,
    /// Accessibility hit-test.
    HitTest,
    /// The guest's real cursor, owned by this participant's input.
    System,
    /// The guest's real cursor, read by the idle-pointer probe.
    Probe,
}

impl ShapeSource {
    /// From the wire value.
    pub fn from_wire(v: i32) -> Self {
        match v {
            1 => ShapeSource::HitTest,
            2 => ShapeSource::System,
            3 => ShapeSource::Probe,
            _ => ShapeSource::Unspecified,
        }
    }

    /// The wire value.
    pub fn to_wire(self) -> i32 {
        match self {
            ShapeSource::Unspecified => 0,
            ShapeSource::HitTest => 1,
            ShapeSource::System => 2,
            ShapeSource::Probe => 3,
        }
    }

    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            ShapeSource::Unspecified => "unspecified",
            ShapeSource::HitTest => "hit_test",
            ShapeSource::System => "system",
            ShapeSource::Probe => "probe",
        }
    }

    /// From a name; unknown names are unspecified.
    pub fn parse(name: &str) -> Self {
        match name {
            "hit_test" => ShapeSource::HitTest,
            "system" => ShapeSource::System,
            "probe" => ShapeSource::Probe,
            _ => ShapeSource::Unspecified,
        }
    }
}

/// Render delay on the 20 Hz `Join` stream.
pub const STREAM_DELAY_MS: f64 = 100.0;
/// Render delay on the 30 Hz datagram channel.
pub const DATAGRAM_DELAY_MS: f64 = 66.0;
/// Longest extrapolation past the newest sample.
pub const EXTRAPOLATE_MS: f64 = 100.0;
/// A jump longer than this (normalized distance) snaps.
pub const SNAP_DISTANCE: f64 = 0.25;
/// A cursor idle this long starts fading.
pub const IDLE_FADE_AFTER_MS: f64 = 5_000.0;
/// Fade duration.
pub const IDLE_FADE_MS: f64 = 300.0;
/// Heartbeat interval the SDKs ask for.
pub const HEARTBEAT_INTERVAL_MS: f64 = 5_000.0;
/// Missed heartbeats before everyone else is dropped.
pub const HEARTBEAT_MISSES: f64 = 3.0;
const MAX_SAMPLES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    t: f64,
    x: f64,
    y: f64,
}

/// One remote cursor's interpolation buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorSmoother {
    samples: VecDeque<Sample>,
    offset: Option<f64>,
    target: String,
    last_move_ms: f64,
    delay_ms: f64,
}

impl CursorSmoother {
    /// A smoother rendering `delay_ms` behind the newest sample.
    pub fn new(delay_ms: f64) -> Self {
        CursorSmoother {
            samples: VecDeque::new(),
            offset: None,
            target: String::new(),
            last_move_ms: f64::NEG_INFINITY,
            delay_ms,
        }
    }

    /// Adds a sample: `server_ms` on the server clock (0 = unknown, the
    /// local time is used), received at `local_ms`, on `target` (display or
    /// window key). Out-of-order samples are dropped.
    pub fn push(&mut self, server_ms: f64, local_ms: f64, x: f64, y: f64, target: &str) {
        let t = if server_ms > 0.0 { server_ms } else { local_ms };
        let candidate = local_ms - t;
        self.offset = Some(match self.offset {
            Some(o) if o <= candidate => o,
            _ => candidate,
        });
        let x = x.clamp(0.0, 1.0);
        let y = y.clamp(0.0, 1.0);
        if let Some(last) = self.samples.back().copied() {
            if t <= last.t {
                return;
            }
            let jump = ((x - last.x).powi(2) + (y - last.y).powi(2)).sqrt();
            if target != self.target || jump > SNAP_DISTANCE {
                self.samples.clear();
            }
            if x != last.x || y != last.y {
                self.last_move_ms = local_ms;
            }
        } else {
            self.last_move_ms = local_ms;
        }
        self.target = target.to_string();
        self.samples.push_back(Sample { t, x, y });
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
    }

    /// Forgets every sample (the next one snaps).
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// The position to draw at `local_ms`, or `None` before any sample.
    pub fn position(&self, local_ms: f64) -> Option<(f64, f64)> {
        let s = &self.samples;
        let first = *s.front()?;
        let last = *s.back()?;
        let render = local_ms - self.offset.unwrap_or(0.0) - self.delay_ms;
        let clamp = |(x, y): (f64, f64)| (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        if render <= first.t {
            return Some((first.x, first.y));
        }
        if render >= last.t {
            let dt = render - last.t;
            if dt > EXTRAPOLATE_MS || s.len() < 2 {
                return Some((last.x, last.y));
            }
            let prev = s[s.len() - 2];
            let span = last.t - prev.t;
            if span <= 0.0 {
                return Some((last.x, last.y));
            }
            let vx = (last.x - prev.x) / span;
            let vy = (last.y - prev.y) / span;
            return Some(clamp((last.x + vx * dt, last.y + vy * dt)));
        }
        let mut i = 0;
        while i + 1 < s.len() && s[i + 1].t <= render {
            i += 1;
        }
        let a = s[i];
        let b = s[i + 1];
        let u = (render - a.t) / (b.t - a.t);
        if s.len() < 4 {
            return Some((a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u));
        }
        let p0 = if i > 0 { s[i - 1] } else { a };
        let p3 = if i + 2 < s.len() { s[i + 2] } else { b };
        let cr = |p0: f64, p1: f64, p2: f64, p3: f64| {
            let u2 = u * u;
            let u3 = u2 * u;
            0.5 * ((2.0 * p1)
                + (-p0 + p2) * u
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u2
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * u3)
        };
        Some(clamp((cr(p0.x, a.x, b.x, p3.x), cr(p0.y, a.y, b.y, p3.y))))
    }

    /// Opacity at `local_ms`: 1 until [`IDLE_FADE_AFTER_MS`] without
    /// movement, then down to 0 over [`IDLE_FADE_MS`].
    pub fn alpha(&self, local_ms: f64) -> f64 {
        idle_alpha(local_ms - self.last_move_ms)
    }
}

/// Opacity of a cursor idle for `idle_ms`.
pub fn idle_alpha(idle_ms: f64) -> f64 {
    if idle_ms <= IDLE_FADE_AFTER_MS {
        1.0
    } else {
        (1.0 - (idle_ms - IDLE_FADE_AFTER_MS) / IDLE_FADE_MS).clamp(0.0, 1.0)
    }
}

/// One cursor to draw.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Drawable {
    /// Whose.
    pub participant_id: String,
    /// Their display name.
    pub display_name: String,
    /// Their color (`#rrggbb`).
    pub color: String,
    /// This client's own cursor (drawn at the local pointer).
    pub is_me: bool,
    /// An agent.
    pub is_agent: bool,
    /// Normalized x.
    pub x: f64,
    /// Normalized y.
    pub y: f64,
    /// Shape.
    pub shape: CursorShape,
    /// How the shape was determined.
    pub shape_source: ShapeSource,
    /// Opacity in `[0, 1]`.
    pub alpha: f64,
    /// Display id of the cursor's target.
    pub display_id: String,
    /// Window id, when over a window stream.
    pub window_id: Option<String>,
}

#[derive(Clone, Debug)]
struct Member {
    participant: Participant,
    cursor: Option<Cursor>,
    smoother: CursorSmoother,
    shape: CursorShape,
    shape_source: ShapeSource,
}

/// Who is present and what to draw for each, folded from presence events.
#[derive(Clone, Debug)]
pub struct PresenceView {
    me: String,
    members: BTreeMap<String, Member>,
    order: Vec<String>,
    delay_ms: f64,
    heartbeat_interval_ms: f64,
    last_heartbeat_ms: Option<f64>,
}

fn target_key(c: &Cursor) -> String {
    match &c.window_id {
        Some(w) => format!("w:{w}"),
        None => format!("d:{}", c.display_id),
    }
}

impl PresenceView {
    /// A view for `me` (the caller's participant id), rendering remote
    /// cursors `delay_ms` behind (see [`STREAM_DELAY_MS`],
    /// [`DATAGRAM_DELAY_MS`]).
    pub fn new(me: &str, delay_ms: f64) -> Self {
        PresenceView {
            me: me.to_string(),
            members: BTreeMap::new(),
            order: Vec::new(),
            delay_ms,
            heartbeat_interval_ms: HEARTBEAT_INTERVAL_MS,
            last_heartbeat_ms: None,
        }
    }

    /// The caller's participant id.
    pub fn me(&self) -> &str {
        &self.me
    }

    /// The expected heartbeat interval (default 5 s).
    pub fn set_heartbeat_interval_ms(&mut self, ms: f64) {
        if ms > 0.0 {
            self.heartbeat_interval_ms = ms;
        }
    }

    /// Adds or updates a participant (for seeding from a session).
    pub fn upsert(&mut self, participant: Participant) {
        let id = participant.participant_id.clone();
        match self.members.get_mut(&id) {
            Some(m) => m.participant = participant,
            None => {
                self.order.push(id.clone());
                self.members.insert(
                    id,
                    Member {
                        participant,
                        cursor: None,
                        smoother: CursorSmoother::new(self.delay_ms),
                        shape: CursorShape::Arrow,
                        shape_source: ShapeSource::Unspecified,
                    },
                );
            }
        }
    }

    fn remove(&mut self, id: &str) -> bool {
        if id == self.me {
            return false;
        }
        self.order.retain(|o| o != id);
        self.members.remove(id).is_some()
    }

    /// Participant ids, the caller first, then in join order.
    pub fn participant_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .order
            .iter()
            .filter(|id| **id == self.me)
            .cloned()
            .collect();
        ids.extend(self.order.iter().filter(|id| **id != self.me).cloned());
        ids
    }

    /// A participant.
    pub fn participant(&self, id: &str) -> Option<&Participant> {
        self.members.get(id).map(|m| &m.participant)
    }

    /// A participant's current shape (the caller's included).
    pub fn shape_of(&self, id: &str) -> Option<(CursorShape, ShapeSource)> {
        self.members.get(id).map(|m| (m.shape, m.shape_source))
    }

    /// Folds one event in at `local_ms`. Returns whether anything changed.
    pub fn apply(&mut self, event: &PresenceEvent, local_ms: f64) -> bool {
        match event {
            PresenceEvent::Joined { participant } => {
                let known = self.members.contains_key(&participant.participant_id);
                self.upsert(participant.clone());
                !known
            }
            PresenceEvent::Left { participant_id, .. } => self.remove(participant_id),
            PresenceEvent::CursorMoved {
                participant_id,
                cursor,
            } => {
                let Some(m) = self.members.get_mut(participant_id) else {
                    return false;
                };
                let received = if cursor.received_ms > 0.0 {
                    cursor.received_ms
                } else {
                    local_ms
                };
                let was_visible = m.cursor.as_ref().is_some_and(|c| c.visible);
                if cursor.visible {
                    if !was_visible {
                        m.smoother.reset();
                    }
                    m.smoother.push(
                        cursor.at_ms,
                        received,
                        cursor.x,
                        cursor.y,
                        &target_key(cursor),
                    );
                }
                if cursor.shape_source != ShapeSource::Unspecified {
                    m.shape = cursor.shape;
                    m.shape_source = cursor.shape_source;
                }
                m.cursor = Some(cursor.clone());
                true
            }
            PresenceEvent::ShapeChanged {
                participant_id,
                shape,
                source,
            } => match self.members.get_mut(participant_id) {
                Some(m) if m.shape != *shape || m.shape_source != *source => {
                    m.shape = *shape;
                    m.shape_source = *source;
                    true
                }
                _ => false,
            },
            PresenceEvent::Heartbeat { participant_ids } => {
                self.last_heartbeat_ms = Some(local_ms);
                let gone: Vec<String> = self
                    .order
                    .iter()
                    .filter(|id| **id != self.me && !participant_ids.contains(id))
                    .cloned()
                    .collect();
                let mut changed = false;
                for id in gone {
                    changed |= self.remove(&id);
                }
                changed
            }
            PresenceEvent::KeepAlive => false,
        }
    }

    /// Drops everyone but the caller when heartbeats were expected and none
    /// arrived for 3 intervals. Returns the removed ids.
    pub fn expire(&mut self, local_ms: f64) -> Vec<String> {
        let Some(last) = self.last_heartbeat_ms else {
            return Vec::new();
        };
        if local_ms - last <= HEARTBEAT_MISSES * self.heartbeat_interval_ms {
            return Vec::new();
        }
        let gone: Vec<String> = self
            .order
            .iter()
            .filter(|id| **id != self.me)
            .cloned()
            .collect();
        for id in &gone {
            self.remove(id);
        }
        gone
    }

    /// Everything to draw at `local_ms`: remote cursors interpolated and
    /// faded, and the caller's own cursor at `local_pointer` (normalized,
    /// `None` when the pointer is not over the surface) with its server
    /// shape. Fully faded cursors are left out. Also applies [`expire`].
    ///
    /// [`expire`]: PresenceView::expire
    pub fn drawables(&mut self, local_ms: f64, local_pointer: Option<(f64, f64)>) -> Vec<Drawable> {
        self.expire(local_ms);
        let mut out = Vec::new();
        for id in self.participant_ids() {
            let Some(m) = self.members.get(&id) else {
                continue;
            };
            let is_me = id == self.me;
            let (x, y, alpha, display_id, window_id) = if is_me {
                let Some((x, y)) = local_pointer else {
                    continue;
                };
                let (d, w) = m
                    .cursor
                    .as_ref()
                    .map(|c| (c.display_id.clone(), c.window_id.clone()))
                    .unwrap_or_default();
                (x, y, 1.0, d, w)
            } else {
                let Some(c) = m.cursor.as_ref().filter(|c| c.visible) else {
                    continue;
                };
                let Some((x, y)) = m.smoother.position(local_ms) else {
                    continue;
                };
                let alpha = m.smoother.alpha(local_ms);
                if alpha <= 0.0 {
                    continue;
                }
                (x, y, alpha, c.display_id.clone(), c.window_id.clone())
            };
            out.push(Drawable {
                participant_id: id.clone(),
                display_name: m.participant.display_name.clone(),
                color: m.participant.color.clone(),
                is_me,
                is_agent: m.participant.kind == "agent",
                x,
                y,
                shape: m.shape,
                shape_source: m.shape_source,
                alpha,
                display_id,
                window_id,
            });
        }
        out
    }
}

/// The conformance vectors every implementation of this model runs.
pub const CONFORMANCE_JSON: &str = include_str!("../../assets/presence-conformance.json");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn shapes_round_trip_wire_and_names() {
        assert_eq!(CursorShape::from_wire(0), CursorShape::Arrow);
        assert_eq!(CursorShape::from_wire(99), CursorShape::Arrow);
        for (i, s) in CursorShape::ALL.iter().enumerate() {
            assert_eq!(s.to_wire(), i as i32 + 1);
            assert_eq!(CursorShape::from_wire(s.to_wire()), *s);
            assert_eq!(CursorShape::parse(s.as_str()), *s);
            assert_eq!(
                serde_json::to_value(s).unwrap(),
                Value::String(s.as_str().into())
            );
        }
        assert_eq!(CursorShape::from_wire(2), CursorShape::Text);
        assert_eq!(CursorShape::from_wire(3), CursorShape::Pointer);
        assert_eq!(CursorShape::from_wire(14), CursorShape::Move);
        for v in 0..4 {
            assert_eq!(ShapeSource::from_wire(v).to_wire(), v);
            assert_eq!(
                ShapeSource::parse(ShapeSource::from_wire(v).as_str()).to_wire(),
                v
            );
        }
    }

    #[test]
    fn fade_curve() {
        assert_eq!(idle_alpha(0.0), 1.0);
        assert_eq!(idle_alpha(5_000.0), 1.0);
        assert!((idle_alpha(5_150.0) - 0.5).abs() < 1e-9);
        assert_eq!(idle_alpha(5_300.0), 0.0);
        assert_eq!(idle_alpha(1e9), 0.0);
    }

    #[test]
    fn out_of_order_samples_are_dropped() {
        let mut s = CursorSmoother::new(0.0);
        s.push(100.0, 100.0, 0.1, 0.1, "d:");
        s.push(90.0, 101.0, 0.9, 0.9, "d:");
        assert_eq!(s.position(100.0), Some((0.1, 0.1)));
    }

    #[test]
    fn the_caller_is_never_removed() {
        let mut v = PresenceView::new("me", STREAM_DELAY_MS);
        v.upsert(Participant {
            participant_id: "me".into(),
            ..Default::default()
        });
        v.apply(
            &PresenceEvent::Heartbeat {
                participant_ids: vec![],
            },
            0.0,
        );
        v.apply(
            &PresenceEvent::Left {
                participant_id: "me".into(),
                reason: "left".into(),
            },
            0.0,
        );
        assert!(v.expire(1e9).is_empty());
        assert_eq!(v.participant_ids(), vec!["me".to_string()]);
    }

    /// Runs `assets/presence-conformance.json`; the TypeScript and Swift
    /// suites run the same file.
    #[test]
    fn conformance_vectors() {
        let doc: Value = serde_json::from_str(CONFORMANCE_JSON).unwrap();
        let tol = doc["tolerance"].as_f64().unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let mut v = PresenceView::new(
                case["me"].as_str().unwrap(),
                case["delay_ms"].as_f64().unwrap(),
            );
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                let at = step["at"].as_f64().unwrap();
                if let Some(e) = step.get("event") {
                    let e: PresenceEvent = serde_json::from_value(e.clone())
                        .unwrap_or_else(|err| panic!("{name}#{i}: {err}"));
                    v.apply(&e, at);
                    continue;
                }
                let pointer = step
                    .get("pointer")
                    .and_then(|p| p.as_array())
                    .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()));
                let got = v.drawables(at, pointer);
                let want = step["expect"].as_array().unwrap();
                assert_eq!(got.len(), want.len(), "{name}#{i}: {got:?}");
                for (g, w) in got.iter().zip(want) {
                    assert_eq!(
                        g.participant_id,
                        w["participant_id"].as_str().unwrap(),
                        "{name}#{i}"
                    );
                    assert!(
                        (g.x - w["x"].as_f64().unwrap()).abs() < tol,
                        "{name}#{i} x {g:?}"
                    );
                    assert!(
                        (g.y - w["y"].as_f64().unwrap()).abs() < tol,
                        "{name}#{i} y {g:?}"
                    );
                    assert!(
                        (g.alpha - w["alpha"].as_f64().unwrap()).abs() < tol,
                        "{name}#{i} alpha {g:?}"
                    );
                    assert_eq!(g.shape.as_str(), w["shape"].as_str().unwrap(), "{name}#{i}");
                    assert_eq!(g.is_me, w["is_me"].as_bool().unwrap(), "{name}#{i}");
                }
            }
        }
    }
}
