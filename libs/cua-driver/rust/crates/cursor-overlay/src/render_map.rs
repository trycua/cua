//! Keyed per-session cursor render map shared by every platform overlay.
//!
//! Each declared session owns one cursor, keyed by [`CursorKey`]. The macOS,
//! Windows, X11, and Wayland adapters all drain the same [`OverlayMsg`]
//! stream into this map, so the lifecycle contract lives here once:
//!
//! - lazy creation from the launch template, with the key as the cursor's
//!   color identity (`cursor_id`), so every session inherits the selected
//!   theme and motion;
//! - a stable z-order: insertion order, unchanged when a cursor is touched
//!   again (later keys paint on top);
//! - `session_end` removal plus a tombstone that drops late in-flight
//!   commands instead of resurrecting the cursor, and an explicit revival
//!   that clears the tombstone without recreating the cursor;
//! - the `"default"` guard: the anonymous cursor is never removed or
//!   tombstoned;
//! - the sentinel seed that places a never-shown cursor near its first
//!   target so the first action glides in;
//! - the frame-tick predicate ([`RenderStateCore::needs_frame_tick`]),
//!   including resting motion.
//!
//! Adapters keep only what their windowing system needs: geometry in
//! [`RenderMap::platform`], arrival waiters, and render-loop scheduling.

use std::collections::HashSet;
use std::time::Duration;

use indexmap::IndexMap;

use crate::{
    CursorConfig, CursorKey, KeyedOverlayCommand, OverlayCommand, OverlayMsg, RenderStateCore,
};

/// Key of the anonymous cursor seeded at startup. It backs one-shot calls
/// without a session and survives every `session_end`.
pub const DEFAULT_CURSOR_KEY: &str = "default";

/// Distance between a seeded start point and its first target, per axis.
/// 140 points reads as motion at the reference peak glide speed.
pub const SEED_OFFSET: f64 = 140.0;

/// Insertion-ordered cursor collection. Iteration order is the paint order.
pub type CursorMap<S> = IndexMap<CursorKey, S>;

/// One cursor entry in a [`RenderMap`]. Platforms wrap [`RenderStateCore`]
/// to add their own state (for example the macOS focus rectangle); a plain
/// core is itself an entry.
pub trait RenderEntry {
    /// Build a cursor from its (already keyed) configuration.
    fn from_config(config: CursorConfig) -> Self;
    fn core(&self) -> &RenderStateCore;
    fn core_mut(&mut self) -> &mut RenderStateCore;
    /// Apply one render command. Returns whether pixels may have changed.
    fn apply_command(&mut self, cmd: OverlayCommand) -> bool;
    /// Advance by `dt` seconds. Returns true on the tick a planned path ends.
    fn tick(&mut self, dt: f64) -> bool {
        self.core_mut().tick_motion(dt)
    }
    /// Whether the render loop must run at frame cadence for this cursor.
    /// Overrides must include the shared core predicate.
    fn needs_frame_tick(&self) -> bool {
        self.core().needs_frame_tick()
    }
}

impl RenderEntry for RenderStateCore {
    fn from_config(config: CursorConfig) -> Self {
        RenderStateCore::new(config)
    }

    fn core(&self) -> &RenderStateCore {
        self
    }

    fn core_mut(&mut self) -> &mut RenderStateCore {
        self
    }

    fn apply_command(&mut self, cmd: OverlayCommand) -> bool {
        self.apply_command_base(cmd, false, false)
    }
}

/// What one drained [`OverlayMsg`] did to the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgOutcome {
    /// The command reached `key`'s cursor, creating it when absent.
    Applied { key: CursorKey, dirty: bool },
    /// The command was dropped because `key`'s session already ended.
    Dropped(CursorKey),
    /// `key` was removed and tombstoned. `existed` reports whether a cursor
    /// was actually removed. Adapters release that key's arrival waiter.
    Removed { key: CursorKey, existed: bool },
    /// `key`'s tombstone was cleared; the cursor is recreated lazily.
    Revived(CursorKey),
    /// A lifecycle message for the guarded `"default"` cursor, or an empty
    /// key, was ignored.
    Ignored,
}

impl MsgOutcome {
    /// The cursor a command resolved to, for z-order pinning.
    pub fn applied_key(&self) -> Option<&CursorKey> {
        match self {
            Self::Applied { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Whether the message can have changed rendered pixels.
    pub fn is_dirty(&self) -> bool {
        match self {
            Self::Applied { dirty, .. } => *dirty,
            Self::Removed { existed, .. } => *existed,
            _ => false,
        }
    }
}

/// A screen rectangle in the coordinate space of cursor positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenFrame {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Smallest frame containing every given frame, or `None` when empty.
    pub fn union(frames: impl IntoIterator<Item = ScreenFrame>) -> Option<ScreenFrame> {
        frames.into_iter().reduce(|a, b| {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = (a.x + a.width).max(b.x + b.width);
            let y1 = (a.y + a.height).max(b.y + b.height);
            ScreenFrame::new(x0, y0, x1 - x0, y1 - y0)
        })
    }
}

/// Start point for a never-shown cursor whose first action targets
/// `(target_x, target_y)`: up-left of the target by [`SEED_OFFSET`], clamped
/// 2 points inside `frame`. When clamping collapses the seed onto the target
/// (a target in the top-left corner), the seed flips down-right so the glide
/// stays visible. Without a known frame the seed is only kept off negative
/// coordinates, which would otherwise read as the off-screen sentinel.
pub fn seed_position(target_x: f64, target_y: f64, frame: Option<ScreenFrame>) -> (f64, f64) {
    let mut sx = target_x - SEED_OFFSET;
    let mut sy = target_y - SEED_OFFSET;
    match frame.filter(|frame| frame.width > 4.0 && frame.height > 4.0) {
        Some(frame) => {
            let (min_x, max_x) = (frame.x + 2.0, frame.x + frame.width - 2.0);
            let (min_y, max_y) = (frame.y + 2.0, frame.y + frame.height - 2.0);
            sx = sx.clamp(min_x, max_x);
            sy = sy.clamp(min_y, max_y);
            if (sx - target_x).abs() < 8.0 && (sy - target_y).abs() < 8.0 {
                sx = (target_x + SEED_OFFSET).clamp(min_x, max_x);
                sy = (target_y + SEED_OFFSET).clamp(min_y, max_y);
            }
        }
        None => {
            sx = sx.max(2.0);
            sy = sy.max(2.0);
        }
    }
    (sx, sy)
}

/// Configuration for a lazily created session cursor: the launch template
/// with the key as its color identity.
pub fn keyed_config(template: &CursorConfig, key: &str) -> CursorConfig {
    let mut config = template.clone();
    config.cursor_id = key.to_owned();
    config
}

/// The keyed render collection plus per-platform state `P` (screen geometry,
/// timing stamps) that the adapter owns.
pub struct RenderMap<S, P = ()> {
    /// Owned cursors in stable paint order.
    pub cursors: CursorMap<S>,
    /// Frozen launch-time configuration for lazily created cursors.
    pub template: CursorConfig,
    /// Tombstones of ended session keys. Never contains `"default"`.
    pub ended: HashSet<CursorKey>,
    /// Most recently commanded cursor, cleared when that cursor is removed.
    /// A single overlay window can occupy one z-band, so this cursor's target
    /// wins z-order pinning.
    pub last_active: Option<CursorKey>,
    /// Platform-owned state.
    pub platform: P,
}

impl<S: RenderEntry, P> RenderMap<S, P> {
    /// A map holding only the `"default"` cursor, which keeps the template's
    /// own `cursor_id`.
    pub fn new(template: CursorConfig, platform: P) -> Self {
        let mut cursors = CursorMap::new();
        cursors.insert(
            DEFAULT_CURSOR_KEY.to_owned(),
            S::from_config(template.clone()),
        );
        Self {
            cursors,
            template,
            ended: HashSet::new(),
            last_active: None,
            platform,
        }
    }

    /// Build the cursor a new session key would receive.
    pub fn state_for_key(&self, key: &str) -> S {
        S::from_config(keyed_config(&self.template, key))
    }

    /// Get or lazily create `key`'s cursor. `None` for an ended session, so
    /// no path can resurrect a removed cursor.
    pub fn cursor_mut(&mut self, key: &str) -> Option<&mut S> {
        if key.is_empty() || self.ended.contains(key) {
            return None;
        }
        if !self.cursors.contains_key(key) {
            let state = self.state_for_key(key);
            self.cursors.insert(key.to_owned(), state);
        }
        self.cursors.get_mut(key)
    }

    /// `key`'s cursor, or the `"default"` cursor when the session has not
    /// created its own yet.
    pub fn cursor_or_default(&self, key: &str) -> Option<&S> {
        self.cursors
            .get(key)
            .or_else(|| self.cursors.get(DEFAULT_CURSOR_KEY))
    }

    /// Apply one drained channel message.
    pub fn apply_msg(&mut self, msg: OverlayMsg) -> MsgOutcome {
        match msg {
            OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => self.apply_command(key, cmd),
            OverlayMsg::Remove(key) => self.remove(key),
            OverlayMsg::Revive(key) => self.revive(key),
        }
    }

    /// Apply `cmd` to `key`'s cursor unless that session ended.
    pub fn apply_command(&mut self, key: CursorKey, cmd: OverlayCommand) -> MsgOutcome {
        let Some(cursor) = self.cursor_mut(&key) else {
            tracing::debug!(key = %key, cmd = ?cmd, "overlay: command dropped; key was ended");
            return MsgOutcome::Dropped(key);
        };
        let dirty = cursor.apply_command(cmd);
        self.last_active = Some(key.clone());
        MsgOutcome::Applied { key, dirty }
    }

    /// Remove and tombstone `key`. The `"default"` cursor is guarded.
    pub fn remove(&mut self, key: CursorKey) -> MsgOutcome {
        if key.is_empty() || key == DEFAULT_CURSOR_KEY {
            return MsgOutcome::Ignored;
        }
        let existed = self.cursors.shift_remove(&key).is_some();
        if self.last_active.as_deref() == Some(key.as_str()) {
            self.last_active = None;
        }
        self.ended.insert(key.clone());
        MsgOutcome::Removed { key, existed }
    }

    /// Clear `key`'s tombstone after an explicit session revival.
    pub fn revive(&mut self, key: CursorKey) -> MsgOutcome {
        if key.is_empty() || key == DEFAULT_CURSOR_KEY {
            return MsgOutcome::Ignored;
        }
        self.ended.remove(&key);
        MsgOutcome::Revived(key)
    }

    /// Place a never-shown (sentinel) cursor near its first target so the
    /// following `MoveTo` glides in instead of snapping. Creates the cursor
    /// when absent. Returns whether a seed was applied; a no-op for an ended
    /// session, a disabled cursor, or one already on screen.
    pub fn seed_start_if_sentinel(
        &mut self,
        key: &str,
        target_x: f64,
        target_y: f64,
        frame: Option<ScreenFrame>,
    ) -> bool {
        let Some(cursor) = self.cursor_mut(key) else {
            return false;
        };
        let core = cursor.core_mut();
        if !(core.cfg.enabled && core.pos.0 < -50.0) {
            return false;
        }
        core.pos = seed_position(target_x, target_y, frame);
        true
    }

    /// Advance every cursor; returns the keys whose planned path just ended.
    pub fn tick_all(&mut self, dt: f64) -> Vec<CursorKey> {
        let mut arrived = Vec::new();
        for (key, cursor) in self.cursors.iter_mut() {
            if cursor.tick(dt) {
                arrived.push(key.clone());
            }
        }
        arrived
    }

    /// Whether any cursor needs frame-cadence ticks.
    pub fn needs_frame_tick(&self) -> bool {
        self.cursors.values().any(S::needs_frame_tick)
    }

    /// Earliest idle-fade start across cursors, for a parked loop's deadline.
    pub fn idle_fade_wait(&self) -> Option<Duration> {
        self.cursors
            .values()
            .filter_map(|cursor| cursor.core().idle_fade_wait())
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReducedMotion;

    type Map = RenderMap<RenderStateCore>;

    fn map() -> Map {
        let mut map = Map::new(CursorConfig::default(), ());
        map.cursors[DEFAULT_CURSOR_KEY].motion.idle_hide_ms = 500.0;
        map
    }

    fn move_msg(key: &str, x: f64, y: f64) -> OverlayMsg {
        OverlayMsg::Cmd(KeyedOverlayCommand {
            key: key.to_owned(),
            cmd: OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians: 0.0,
            },
        })
    }

    fn placed<'a>(map: &'a mut Map, key: &str) -> &'a mut RenderStateCore {
        let core = map.cursor_mut(key).unwrap();
        core.pos = (100.0, 100.0);
        core
    }

    fn settle(core: &mut RenderStateCore) {
        for _ in 0..2000 {
            core.tick_motion(1.0 / 60.0);
            if core.path.is_none() && core.spring.is_none() && core.click_t.is_none() {
                break;
            }
        }
    }

    #[test]
    fn sessions_get_distinct_cursors_that_inherit_the_template() {
        let mut map = map();
        map.apply_msg(move_msg("sessA", 10.0, 10.0));
        map.apply_msg(move_msg("sessB", 20.0, 20.0));

        assert_eq!(map.cursors.len(), 3);
        assert_eq!(map.cursors["sessA"].cfg.cursor_id, "sessA");
        assert_eq!(map.cursors["sessB"].cfg.cursor_id, "sessB");
        // The default slot keeps the launch-time identity.
        assert_eq!(
            map.cursors[DEFAULT_CURSOR_KEY].cfg.cursor_id,
            CursorConfig::default().cursor_id
        );
        for key in ["sessA", "sessB"] {
            assert_eq!(map.cursors[key].cfg.theme_id, map.template.theme_id);
            assert_eq!(map.cursors[key].motion, map.template.motion);
        }
    }

    #[test]
    fn insertion_order_is_a_stable_z_order() {
        let mut map = map();
        map.apply_msg(move_msg("first", 1.0, 1.0));
        map.apply_msg(move_msg("second", 2.0, 2.0));
        map.apply_msg(move_msg("first", 3.0, 3.0));
        let keys: Vec<&str> = map.cursors.keys().map(String::as_str).collect();
        assert_eq!(keys, ["default", "first", "second"]);
    }

    #[test]
    fn applied_commands_track_the_last_active_cursor() {
        let mut map = map();
        let outcome = map.apply_msg(move_msg("sessA", 10.0, 10.0));
        assert_eq!(outcome.applied_key().map(String::as_str), Some("sessA"));
        assert!(outcome.is_dirty());
        assert_eq!(map.last_active.as_deref(), Some("sessA"));

        map.apply_msg(move_msg("sessB", 10.0, 10.0));
        map.apply_msg(OverlayMsg::Remove("sessA".to_owned()));
        assert_eq!(map.last_active.as_deref(), Some("sessB"));
        map.apply_msg(OverlayMsg::Remove("sessB".to_owned()));
        assert_eq!(map.last_active, None);
    }

    #[test]
    fn removal_tombstones_only_that_session_and_blocks_resurrection() {
        let mut map = map();
        map.apply_msg(move_msg("sessA", 10.0, 10.0));
        map.apply_msg(move_msg("sessB", 20.0, 20.0));

        assert_eq!(
            map.apply_msg(OverlayMsg::Remove("sessA".to_owned())),
            MsgOutcome::Removed {
                key: "sessA".to_owned(),
                existed: true
            }
        );
        assert!(!map.cursors.contains_key("sessA"));
        assert!(map.cursors.contains_key("sessB"));
        assert!(map.ended.contains("sessA"));

        // A late in-flight command and a seed are both dropped.
        assert_eq!(
            map.apply_msg(move_msg("sessA", 99.0, 99.0)),
            MsgOutcome::Dropped("sessA".to_owned())
        );
        assert!(!map.seed_start_if_sentinel("sessA", 60.0, 60.0, None));
        assert!(!map.cursors.contains_key("sessA"));

        // Removing a session that never drew is a harmless tombstone.
        assert_eq!(
            map.apply_msg(OverlayMsg::Remove("never-drew".to_owned())),
            MsgOutcome::Removed {
                key: "never-drew".to_owned(),
                existed: false
            }
        );
        assert_eq!(map.cursors.len(), 2);
    }

    #[test]
    fn revival_clears_the_tombstone_and_recreates_lazily() {
        let mut map = map();
        map.apply_msg(move_msg("sessA", 10.0, 10.0));
        map.apply_msg(OverlayMsg::Remove("sessA".to_owned()));

        assert_eq!(
            map.apply_msg(OverlayMsg::Revive("sessA".to_owned())),
            MsgOutcome::Revived("sessA".to_owned())
        );
        assert!(!map.ended.contains("sessA"));
        assert!(!map.cursors.contains_key("sessA"));

        let outcome = map.apply_msg(move_msg("sessA", 30.0, 30.0));
        assert_eq!(outcome.applied_key().map(String::as_str), Some("sessA"));
        assert!(map.cursors.contains_key("sessA"));
    }

    #[test]
    fn default_and_empty_keys_are_guarded() {
        let mut map = map();
        for msg in [
            OverlayMsg::Remove(DEFAULT_CURSOR_KEY.to_owned()),
            OverlayMsg::Revive(DEFAULT_CURSOR_KEY.to_owned()),
            OverlayMsg::Remove(String::new()),
        ] {
            assert_eq!(map.apply_msg(msg), MsgOutcome::Ignored);
        }
        assert!(map.cursors.contains_key(DEFAULT_CURSOR_KEY));
        assert!(map.ended.is_empty());
        assert!(map.cursor_mut("").is_none());

        let outcome = map.apply_msg(move_msg(DEFAULT_CURSOR_KEY, 5.0, 5.0));
        assert_eq!(
            outcome.applied_key().map(String::as_str),
            Some(DEFAULT_CURSOR_KEY)
        );
    }

    #[test]
    fn cursor_or_default_falls_back_only_for_absent_keys() {
        let mut map = map();
        map.cursors[DEFAULT_CURSOR_KEY].visible = false;
        assert!(!map.cursor_or_default("sessA").unwrap().visible);
        map.cursor_mut("sessA");
        assert!(map.cursor_or_default("sessA").unwrap().visible);
    }

    #[test]
    fn seed_places_a_sentinel_cursor_on_screen_once() {
        let mut map = map();
        let frame = Some(ScreenFrame::new(0.0, 0.0, 100.0, 100.0));
        assert!(map.seed_start_if_sentinel("sessA", 60.0, 60.0, frame));
        let pos = map.cursors["sessA"].pos;
        assert!(
            pos.0 > -50.0 && pos.1 > -50.0,
            "seed must be on screen: {pos:?}"
        );
        assert!(
            (pos.0 - 60.0).abs() > 4.0 || (pos.1 - 60.0).abs() > 4.0,
            "seed must differ from the target: {pos:?}"
        );

        map.cursors["sessA"].pos = (30.0, 30.0);
        assert!(!map.seed_start_if_sentinel("sessA", 80.0, 80.0, frame));
        assert_eq!(map.cursors["sessA"].pos, (30.0, 30.0));

        map.cursor_mut("disabled").unwrap().cfg.enabled = false;
        assert!(!map.seed_start_if_sentinel("disabled", 80.0, 80.0, frame));
    }

    #[test]
    fn seed_position_clamps_into_the_frame_and_keeps_a_visible_glide() {
        let frame = ScreenFrame::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(seed_position(500.0, 400.0, Some(frame)), (360.0, 260.0));
        // A top-left corner target flips the seed down-right.
        assert_eq!(seed_position(3.0, 3.0, Some(frame)), (143.0, 143.0));
        // A secondary monitor left of the primary has negative coordinates.
        let virtual_screen = ScreenFrame::new(-1920.0, 0.0, 3840.0, 1080.0);
        assert_eq!(
            seed_position(-1800.0, 500.0, Some(virtual_screen)),
            (-1918.0, 360.0)
        );
        // Unknown geometry never produces a sentinel-like negative seed.
        assert_eq!(seed_position(50.0, 500.0, None), (2.0, 360.0));
        // A degenerate frame is treated as unknown instead of panicking.
        let tiny = ScreenFrame::new(0.0, 0.0, 1.0, 1.0);
        assert_eq!(seed_position(50.0, 500.0, Some(tiny)), (2.0, 360.0));
    }

    #[test]
    fn screen_frame_union_bounds_every_output() {
        let union = ScreenFrame::union([
            ScreenFrame::new(0.0, 0.0, 1920.0, 1080.0),
            ScreenFrame::new(1920.0, -200.0, 1280.0, 1024.0),
        ]);
        assert_eq!(union, Some(ScreenFrame::new(0.0, -200.0, 3200.0, 1280.0)));
        assert_eq!(ScreenFrame::union([]), None);
    }

    #[test]
    fn sentinel_cursor_is_quiescent() {
        let map = map();
        assert!(!map.needs_frame_tick());
        assert_eq!(map.idle_fade_wait(), None);
    }

    #[test]
    fn glide_and_click_pulse_require_frame_ticks() {
        let mut map = map();
        placed(&mut map, "glide");
        map.apply_msg(move_msg("glide", 250.0, 150.0));
        assert!(map.cursors["glide"].needs_frame_tick());

        let mut map = self::map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        core.visual.reduced_motion = ReducedMotion::On;
        core.motion.idle_hide_ms = 0.0;
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 20.0, y: 30.0 },
            false,
            false,
        );
        assert!(core.needs_frame_tick());
        settle(core);
        for _ in 0..120 {
            core.tick_motion(1.0 / 60.0);
        }
        assert!(!map.needs_frame_tick(), "a finished pulse must park");
    }

    // The resting bob is painted by every adapter through `paint_cursor`, so a
    // visible resting cursor must keep receiving frames on every platform
    // until its idle fade hides it.
    #[test]
    fn a_visible_resting_cursor_keeps_ticking_for_its_resting_motion() {
        let mut map = map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        core.motion.idle_hide_ms = 20_000.0;
        settle(core);
        assert!(core.has_resting_motion());
        assert!(map.needs_frame_tick());

        // The bob actually moves pixels between two resting frames.
        let paint = |core: &RenderStateCore| {
            let mut pixmap = tiny_skia::Pixmap::new(200, 200).unwrap();
            crate::paint_cursor(&mut pixmap, core, 0.0, 0.0, None, 1.0);
            pixmap.data().to_vec()
        };
        let core = &mut map.cursors[DEFAULT_CURSOR_KEY];
        let before = paint(core);
        core.tick_motion(0.4);
        assert_ne!(before, paint(core), "resting frames must differ");
    }

    #[test]
    fn resting_motion_stops_for_reduced_motion_hidden_and_faded_cursors() {
        let mut map = map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        core.motion.idle_hide_ms = 20_000.0;
        assert!(core.needs_frame_tick());

        core.visual.reduced_motion = ReducedMotion::On;
        assert!(!core.has_resting_motion());
        assert!(!core.needs_frame_tick());
        core.visual.reduced_motion = ReducedMotion::Auto;

        core.visible = false;
        assert!(!core.needs_frame_tick());
        core.visible = true;

        core.pinned_target_off_workspace = true;
        assert!(!core.has_resting_motion());
        core.pinned_target_off_workspace = false;

        core.idle_alpha = 0.0;
        assert!(!core.needs_frame_tick());
    }

    // A never-hiding cursor stays on screen indefinitely, so its resting
    // motion would keep the overlay rendering forever. It rests still and the
    // overlay quiesces, which the hosted Wayland CPU certification enforces.
    #[test]
    fn a_never_hiding_cursor_rests_still_so_the_overlay_quiesces() {
        let mut map = map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        core.motion.idle_hide_ms = 0.0;
        settle(core);
        for _ in 0..600 {
            core.tick_motion(1.0 / 60.0);
        }
        assert!(core.is_revealed());
        assert!(!core.has_resting_motion());
        assert!(!map.needs_frame_tick());
        assert_eq!(map.idle_fade_wait(), None);
    }

    #[test]
    fn a_single_frame_custom_theme_has_no_resting_motion() {
        let mut theme = (*crate::embedded_default_theme()).clone();
        theme.id = "example.still".to_owned();
        let mut map = map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        for animation in theme.actions.values_mut() {
            animation.frames.truncate(1);
            animation.still_frame = 0;
        }
        core.theme = Some(std::sync::Arc::new(theme));
        assert!(!core.has_resting_motion());

        let mut looping = (*crate::embedded_default_theme()).clone();
        looping.id = "example.loop".to_owned();
        for animation in looping.actions.values_mut() {
            let first = animation.frames[0].clone();
            animation.frames = vec![first.clone(), first];
        }
        core.theme = Some(std::sync::Arc::new(looping));
        assert!(core.has_resting_motion());
    }

    #[test]
    fn reduced_motion_parks_through_the_opaque_delay_then_ticks_the_fade() {
        let mut map = map();
        let core = placed(&mut map, DEFAULT_CURSOR_KEY);
        core.visual.reduced_motion = ReducedMotion::On;
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 250.0,
                y: 150.0,
                end_heading_radians: 0.0,
            },
            false,
            false,
        );
        for _ in 0..1200 {
            core.tick_motion(1.0 / 60.0);
            if !core.needs_frame_tick() {
                break;
            }
        }
        assert!(!core.needs_frame_tick(), "opaque idle delay must park");
        assert_eq!(core.idle_alpha, 1.0);

        let wait = map.idle_fade_wait().expect("idle fade deadline");
        let core = &mut map.cursors[DEFAULT_CURSOR_KEY];
        assert!(wait > Duration::ZERO && wait <= Duration::from_millis(500));
        // Wake just past the deadline, as a parked loop's timeout does.
        core.tick_motion(wait.as_secs_f64() + 0.001);
        assert!(core.idle_fade_in_progress());
        assert!(core.needs_frame_tick());
        assert_eq!(map.idle_fade_wait(), None);

        for _ in 0..60 {
            map.tick_all(1.0 / 60.0);
        }
        assert_eq!(map.cursors[DEFAULT_CURSOR_KEY].idle_alpha, 0.0);
        assert!(!map.needs_frame_tick(), "a fully faded cursor must park");
    }

    #[test]
    fn tick_all_reports_each_arrival_once() {
        let mut map = map();
        placed(&mut map, "sessA");
        map.apply_msg(move_msg("sessA", 120.0, 120.0));
        let mut arrivals = Vec::new();
        for _ in 0..600 {
            arrivals.extend(map.tick_all(1.0 / 60.0));
        }
        assert_eq!(arrivals, ["sessA"]);
    }
}
