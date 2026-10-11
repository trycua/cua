// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Visibility lifecycle of the on-screen agent cursor overlay.
//!
//! Every platform overlay (macOS, Windows, X11, Wayland layer-shell and the
//! GNOME Shell helper) follows the same contract, kept here so the adapters
//! stay thin:
//!
//! - **Session end.** When the driver session that owns a cursor ends
//!   (`end_session`, `DELETE /mcp`, a closed transport, idle eviction), that
//!   cursor is hidden at once. Nothing fades for longer than
//!   [`AGENT_CURSOR_FADE`], which is well under the 300 ms bound.
//! - **Idle.** A cursor with no activity for [`AGENT_CURSOR_IDLE_TIMEOUT`]
//!   fades out over [`AGENT_CURSOR_FADE`] and reappears on its next action.
//!   The timeout is the one presence uses to drop an idle agent participant
//!   (`libs/cua/proto/PRESENCE.md` section 5), so the in-guest overlay and every
//!   remote client stop drawing an idle agent at the same moment.
//! - **Independence.** Each session owns its own cursor. Ending or idling
//!   one session never hides another's.
//! - **Origin.** Only agent input draws an agent cursor. Input a trusted host
//!   relays for a human (a Cua Spaces viewer clicking through a media
//!   stream) is marked [`InputOrigin::Human`] with the private
//!   [`INPUT_ORIGIN_ARG`]; its session never draws the overlay, because the
//!   viewer already sees its own cursor and a second, agent-styled one on
//!   the guest desktop would misattribute the human's action. Every platform
//!   overlay checks [`overlay_suppressed`] at its single command entry point,
//!   and an embedder publishing presence from the cursor hook checks
//!   [`input_origin`] so a human's input never appears as an agent.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Private (underscore-prefixed, so never caller-forgeable) tool argument a
/// trusted adapter sets to say who originated the input: `"human"` or
/// `"agent"`. Public callers' reserved arguments are stripped at every
/// ingress, so an agent cannot hide its cursor by claiming to be a human.
pub const INPUT_ORIGIN_ARG: &str = "_input_origin";

/// Who originated a driver session's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputOrigin {
    /// An agent (MCP, SDK, CLI). The default; draws the agent cursor.
    #[default]
    Agent,
    /// A human whose client draws its own cursor. Draws no agent cursor.
    Human,
}

impl InputOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Human => "human",
        }
    }

    /// Parse the [`INPUT_ORIGIN_ARG`] value; anything else is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::Agent),
            "human" => Some(Self::Human),
            _ => None,
        }
    }

    /// Whether input of this origin draws the in-guest agent cursor.
    pub fn draws_agent_cursor(self) -> bool {
        self == Self::Agent
    }
}

fn origins() -> &'static Mutex<HashMap<String, InputOrigin>> {
    static ORIGINS: OnceLock<Mutex<HashMap<String, InputOrigin>>> = OnceLock::new();
    ORIGINS.get_or_init(Default::default)
}

/// Record the origin of the input a cursor key (a driver session) carries.
/// The registry calls this for each call with trusted origin evidence; an
/// `Agent` origin forgets the key, so the map holds only human sessions.
pub fn set_input_origin(cursor_key: &str, origin: InputOrigin) {
    if cursor_key.is_empty() || cursor_key == "default" {
        return;
    }
    let mut origins = origins().lock().unwrap_or_else(|e| e.into_inner());
    match origin {
        InputOrigin::Human => {
            origins.insert(cursor_key.to_owned(), origin);
        }
        InputOrigin::Agent => {
            origins.remove(cursor_key);
        }
    }
}

/// The origin of a cursor key's input: [`InputOrigin::Agent`] unless a
/// trusted adapter marked it human.
pub fn input_origin(cursor_key: &str) -> InputOrigin {
    origins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(cursor_key)
        .copied()
        .unwrap_or_default()
}

/// Whether the overlay must drop every command for this cursor key. Each
/// platform overlay's command entry point calls this; see the module docs.
pub fn overlay_suppressed(cursor_key: &str) -> bool {
    !input_origin(cursor_key).draws_agent_cursor()
}

/// Forget a session's origin (its session ended).
pub fn forget_input_origin(cursor_key: &str) {
    origins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(cursor_key);
}

/// How long an agent cursor may sit without activity before it fades out.
///
/// This is also cua-spacesd's presence `AGENT_IDLE`, the timeout after which an
/// agent participant without a stream leaves with `LEAVE_REASON_TIMEOUT`.
pub const AGENT_CURSOR_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Length of the idle fade. Session end hides at once, so no overlay spends
/// more than this fading a cursor out.
pub const AGENT_CURSOR_FADE: Duration = Duration::from_millis(180);

/// Alpha below which a fading cursor counts as gone.
pub const AGENT_CURSOR_HIDDEN_ALPHA: f64 = 0.004;

/// [`AGENT_CURSOR_IDLE_TIMEOUT`] in milliseconds, the unit of the public
/// `idle_hide_ms` motion setting.
pub fn default_idle_hide_ms() -> f64 {
    AGENT_CURSOR_IDLE_TIMEOUT.as_secs_f64() * 1000.0
}

/// Whether `idle_hide_ms` enables idle hiding (positive and finite).
fn idle_hiding(idle_hide_ms: f64) -> bool {
    idle_hide_ms.is_finite() && idle_hide_ms > 0.0
}

/// Opacity of a cursor that has been idle for `idle_secs`, with the fade
/// starting after `idle_hide_ms` (0 disables idle hiding).
pub fn idle_alpha(idle_secs: f64, idle_hide_ms: f64) -> f64 {
    if !idle_hiding(idle_hide_ms) {
        return 1.0;
    }
    let fade_start = idle_hide_ms / 1000.0;
    let fade = AGENT_CURSOR_FADE.as_secs_f64();
    if idle_secs <= fade_start {
        1.0
    } else if idle_secs >= fade_start + fade {
        0.0
    } else {
        1.0 - (idle_secs - fade_start) / fade
    }
}

/// Idle and session-end state of one agent cursor, advanced by the render
/// loop's frame delta.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentCursorVisibility {
    idle_secs: f64,
    ended: bool,
}

impl Default for AgentCursorVisibility {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentCursorVisibility {
    pub const fn new() -> Self {
        Self {
            idle_secs: 0.0,
            ended: false,
        }
    }

    /// The session acted through this cursor: show it and restart the idle
    /// clock. An ended cursor stays hidden; the owning session must be
    /// revived first (see [`Self::revive`]).
    pub fn activity(&mut self) {
        self.idle_secs = 0.0;
    }

    /// The owning session ended: hide at once.
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// An explicit `start_session` revived the owning session.
    pub fn revive(&mut self) {
        self.ended = false;
        self.idle_secs = 0.0;
    }

    /// Advance by `dt` seconds. `busy` (a glide, spring or click in progress)
    /// counts as activity.
    pub fn tick(&mut self, dt: f64, busy: bool) {
        if busy {
            self.idle_secs = 0.0;
        } else if dt.is_finite() && dt > 0.0 {
            self.idle_secs += dt;
        }
    }

    pub fn idle_secs(&self) -> f64 {
        self.idle_secs
    }

    /// Set the idle clock directly (render loops that credit wall-clock time
    /// while parked).
    pub fn set_idle_secs(&mut self, idle_secs: f64) {
        if idle_secs.is_finite() {
            self.idle_secs = idle_secs.max(0.0);
        }
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Current opacity multiplier, 0 to 1.
    pub fn alpha(&self, idle_hide_ms: f64) -> f64 {
        if self.ended {
            0.0
        } else {
            idle_alpha(self.idle_secs, idle_hide_ms)
        }
    }

    pub fn is_hidden(&self, idle_hide_ms: f64) -> bool {
        self.alpha(idle_hide_ms) < AGENT_CURSOR_HIDDEN_ALPHA
    }

    /// Time until the idle fade starts, or `None` when it already started,
    /// the cursor ended, or idle hiding is off. Render loops that park while
    /// nothing moves sleep at most this long.
    pub fn until_idle_fade(&self, idle_hide_ms: f64) -> Option<Duration> {
        if self.ended || !idle_hiding(idle_hide_ms) {
            return None;
        }
        let remaining = idle_hide_ms / 1000.0 - self.idle_secs;
        (remaining > 0.0).then(|| Duration::from_secs_f64(remaining))
    }
}

/// What a single-cursor surface should do after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedSurfaceAction {
    /// Draw the command; `switched` is true when a different session now owns
    /// the surface (restyle it for that session).
    Draw { switched: bool },
    /// Hide the surface.
    Hide,
    /// Leave the surface as it is.
    Keep,
}

/// Arbitration for a backend that can draw only one agent cursor, such as the
/// GNOME Shell helper. The surface belongs to the session that drew last; only
/// that session's end, or its idle timeout, hides it. Another session ending
/// leaves it alone.
#[derive(Debug, Clone, Default)]
pub struct SharedCursorSurface {
    owner: Option<String>,
    last_activity: Option<Instant>,
    ended: std::collections::HashSet<String>,
}

impl SharedCursorSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// A render command for `key` at `now`.
    pub fn on_command(&mut self, key: &str, now: Instant) -> SharedSurfaceAction {
        if self.ended.contains(key) {
            return SharedSurfaceAction::Keep;
        }
        let switched = self.owner.as_deref() != Some(key);
        self.owner = Some(key.to_owned());
        self.last_activity = Some(now);
        SharedSurfaceAction::Draw { switched }
    }

    /// The session behind `key` ended.
    pub fn on_session_end(&mut self, key: &str) -> SharedSurfaceAction {
        if key.is_empty() || key == "default" {
            return SharedSurfaceAction::Keep;
        }
        self.ended.insert(key.to_owned());
        if self.owner.as_deref() == Some(key) {
            self.owner = None;
            self.last_activity = None;
            SharedSurfaceAction::Hide
        } else {
            SharedSurfaceAction::Keep
        }
    }

    /// The session behind `key` was explicitly revived.
    pub fn on_session_revive(&mut self, key: &str) {
        self.ended.remove(key);
    }

    /// Check the idle timeout at `now`.
    pub fn poll_idle(&mut self, now: Instant, idle_timeout: Duration) -> SharedSurfaceAction {
        match self.last_activity {
            Some(at) if now.saturating_duration_since(at) >= idle_timeout => {
                self.last_activity = None;
                SharedSurfaceAction::Hide
            }
            _ => SharedSurfaceAction::Keep,
        }
    }

    /// When the idle timeout hides the surface, if it is showing.
    pub fn idle_deadline(&self, idle_timeout: Duration) -> Option<Instant> {
        self.last_activity.map(|at| at + idle_timeout)
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_human_origin_suppresses_the_overlay() {
        let key = "test-origin-human-session";
        assert_eq!(input_origin(key), InputOrigin::Agent);
        assert!(!overlay_suppressed(key));
        set_input_origin(key, InputOrigin::Human);
        assert_eq!(input_origin(key), InputOrigin::Human);
        assert!(overlay_suppressed(key));
        // Another session is unaffected.
        assert!(!overlay_suppressed("test-origin-agent-session"));
        set_input_origin(key, InputOrigin::Agent);
        assert!(!overlay_suppressed(key));
        set_input_origin(key, InputOrigin::Human);
        forget_input_origin(key);
        assert!(!overlay_suppressed(key));
    }

    #[test]
    fn the_shared_default_cursor_is_never_marked_human() {
        for key in ["", "default"] {
            set_input_origin(key, InputOrigin::Human);
            assert!(!overlay_suppressed(key));
        }
    }

    #[test]
    fn origin_argument_values() {
        assert_eq!(InputOrigin::parse("human"), Some(InputOrigin::Human));
        assert_eq!(InputOrigin::parse("agent"), Some(InputOrigin::Agent));
        assert_eq!(InputOrigin::parse("robot"), None);
        assert!(InputOrigin::Agent.draws_agent_cursor());
        assert!(!InputOrigin::Human.draws_agent_cursor());
        assert!(
            INPUT_ORIGIN_ARG.starts_with('_'),
            "must be a reserved argument"
        );
    }

    const IDLE_MS: f64 = 15_000.0;

    #[test]
    fn idle_timeout_matches_presence_and_fade_is_short() {
        assert_eq!(AGENT_CURSOR_IDLE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(default_idle_hide_ms(), IDLE_MS);
        assert!(AGENT_CURSOR_FADE <= Duration::from_millis(300));
    }

    #[test]
    fn cursor_stays_visible_until_idle_timeout_then_fades() {
        let mut cursor = AgentCursorVisibility::new();
        cursor.tick(14.9, false);
        assert_eq!(cursor.alpha(IDLE_MS), 1.0);
        assert!(cursor.until_idle_fade(IDLE_MS).is_some());
        cursor.tick(0.1 + AGENT_CURSOR_FADE.as_secs_f64() / 2.0, false);
        let mid = cursor.alpha(IDLE_MS);
        assert!(mid > 0.0 && mid < 1.0, "mid-fade alpha {mid}");
        assert_eq!(cursor.until_idle_fade(IDLE_MS), None);
        cursor.tick(AGENT_CURSOR_FADE.as_secs_f64(), false);
        assert!(cursor.is_hidden(IDLE_MS));
    }

    #[test]
    fn activity_and_motion_reset_the_idle_clock() {
        let mut cursor = AgentCursorVisibility::new();
        cursor.tick(20.0, false);
        assert!(cursor.is_hidden(IDLE_MS));
        cursor.activity();
        assert_eq!(cursor.alpha(IDLE_MS), 1.0);
        cursor.tick(10.0, false);
        cursor.tick(10.0, true);
        assert_eq!(cursor.idle_secs(), 0.0, "a glide in progress is activity");
        assert_eq!(cursor.alpha(IDLE_MS), 1.0);
    }

    #[test]
    fn zero_idle_hide_never_fades() {
        let mut cursor = AgentCursorVisibility::new();
        cursor.tick(3600.0, false);
        assert_eq!(cursor.alpha(0.0), 1.0);
        assert_eq!(cursor.until_idle_fade(0.0), None);
    }

    #[test]
    fn session_end_hides_at_once_and_only_revive_restores() {
        let mut cursor = AgentCursorVisibility::new();
        cursor.end();
        assert_eq!(cursor.alpha(IDLE_MS), 0.0, "no fade after session end");
        assert_eq!(cursor.until_idle_fade(IDLE_MS), None);
        cursor.activity();
        assert!(
            cursor.is_hidden(IDLE_MS),
            "a late action cannot resurrect it"
        );
        cursor.revive();
        assert_eq!(cursor.alpha(IDLE_MS), 1.0);
    }

    #[test]
    fn ending_one_cursor_leaves_another_visible() {
        let mut a = AgentCursorVisibility::new();
        let mut b = AgentCursorVisibility::new();
        a.end();
        b.tick(1.0, false);
        assert!(a.is_hidden(IDLE_MS));
        assert_eq!(b.alpha(IDLE_MS), 1.0);
    }

    #[test]
    fn shared_surface_hides_only_for_its_owner() {
        let now = Instant::now();
        let mut surface = SharedCursorSurface::new();
        assert_eq!(
            surface.on_command("run-a", now),
            SharedSurfaceAction::Draw { switched: true }
        );
        assert_eq!(
            surface.on_command("run-a", now),
            SharedSurfaceAction::Draw { switched: false }
        );
        // Another session (that never drew, or drew earlier) ending keeps it.
        assert_eq!(surface.on_session_end("run-b"), SharedSurfaceAction::Keep);
        assert_eq!(surface.owner(), Some("run-a"));
        // A late command from the ended session cannot take the surface.
        assert_eq!(surface.on_command("run-b", now), SharedSurfaceAction::Keep);
        assert_eq!(surface.on_session_end("run-a"), SharedSurfaceAction::Hide);
        assert_eq!(surface.owner(), None);
        assert_eq!(surface.on_session_end("default"), SharedSurfaceAction::Keep);
        surface.on_session_revive("run-b");
        assert_eq!(
            surface.on_command("run-b", now),
            SharedSurfaceAction::Draw { switched: true }
        );
    }

    #[test]
    fn shared_surface_hides_after_idle_timeout() {
        let start = Instant::now();
        let mut surface = SharedCursorSurface::new();
        surface.on_command("run-a", start);
        assert_eq!(
            surface.idle_deadline(AGENT_CURSOR_IDLE_TIMEOUT),
            Some(start + AGENT_CURSOR_IDLE_TIMEOUT)
        );
        let before = start + AGENT_CURSOR_IDLE_TIMEOUT - Duration::from_millis(1);
        assert_eq!(
            surface.poll_idle(before, AGENT_CURSOR_IDLE_TIMEOUT),
            SharedSurfaceAction::Keep
        );
        let after = start + AGENT_CURSOR_IDLE_TIMEOUT;
        assert_eq!(
            surface.poll_idle(after, AGENT_CURSOR_IDLE_TIMEOUT),
            SharedSurfaceAction::Hide
        );
        // Hidden once: no repeated hides, no deadline until the next command.
        assert_eq!(
            surface.poll_idle(after, AGENT_CURSOR_IDLE_TIMEOUT),
            SharedSurfaceAction::Keep
        );
        assert_eq!(surface.idle_deadline(AGENT_CURSOR_IDLE_TIMEOUT), None);
        // The idle owner still owns it; its next action redraws in place.
        assert_eq!(
            surface.on_command("run-a", after),
            SharedSurfaceAction::Draw { switched: false }
        );
    }
}
