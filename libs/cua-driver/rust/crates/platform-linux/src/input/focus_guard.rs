//! Background non-invasiveness safety net for X11.
//!
//! A `delivery_mode:"background"` action must leave the user's desktop as it
//! found it: the core keyboard focus (`XGetInputFocus`), the window manager's
//! active window (`_NET_ACTIVE_WINDOW`) and the top of the stacking order.
//! The focus-free routes (MPX virtual keyboard, AT-SPI actions) never touch
//! those themselves, but the *application* may: GTK/VCL activate their
//! toplevel when a menu opens, mutter focuses a freshly mapped transient
//! dialog, Nautilus presents its window after a navigation shortcut.
//!
//! [`FocusSnapshot::capture`] records the three signals before an action and
//! [`FocusSnapshot::restore_if_changed`] re-asserts them afterwards when they
//! moved: the previously active window is re-activated through EWMH (so the
//! WM's own bookkeeping and stacking follow) and the previous core focus is
//! re-set. The outcome is reported, never hidden: a toolkit keyboard grab held
//! by an open menu cannot be undone without closing the menu, so it is
//! surfaced as `grab_held_by` instead.

use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
    MapState, PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// How long the guard watches for a late focus change after delivery (a
/// transient dialog is mapped and focused by the WM a beat after the action).
const SETTLE_WATCH: Duration = Duration::from_millis(220);
/// Longer watch once a new top-level appeared during the short one: a dialog
/// (LibreOffice's take ~1 s to build) is focused by the WM only when mapped.
const SETTLE_WATCH_NEW_WINDOW: Duration = Duration::from_millis(1400);
const SETTLE_POLL: Duration = Duration::from_millis(30);
/// Bound on the restore loop: re-activation, verification, one re-send.
const RESTORE_BUDGET: Duration = Duration::from_millis(1200);
const RESTORE_POLL: Duration = Duration::from_millis(50);
/// Consecutive stable polls before the restore is called done.
const STABLE_POLLS: u32 = 3;

struct Atoms {
    net_active_window: u32,
    net_client_list_stacking: u32,
    net_wm_pid: u32,
}

struct X {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl X {
    fn open() -> Result<Self> {
        let (conn, screen) = RustConnection::connect(None)
            .map_err(|e| anyhow!("focus guard: cannot open DISPLAY: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let intern = |name: &[u8]| -> Result<u32> {
            Ok(conn.intern_atom(false, name)?.reply()?.atom)
        };
        let atoms = Atoms {
            net_active_window: intern(b"_NET_ACTIVE_WINDOW")?,
            net_client_list_stacking: intern(b"_NET_CLIENT_LIST_STACKING")?,
            net_wm_pid: intern(b"_NET_WM_PID")?,
        };
        Ok(Self { conn, root, atoms })
    }

    fn window_property(&self, window: Window, atom: u32, len: u32) -> Vec<u32> {
        self.conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, len)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    fn active_window(&self) -> Option<Window> {
        self.window_property(self.root, self.atoms.net_active_window, 1)
            .first()
            .copied()
            .filter(|w| *w != 0)
    }

    fn stacking_top(&self) -> Option<Window> {
        self.window_property(self.root, self.atoms.net_client_list_stacking, u32::MAX)
            .last()
            .copied()
            .filter(|w| *w != 0)
    }

    fn client_count(&self) -> usize {
        self.window_property(self.root, self.atoms.net_client_list_stacking, u32::MAX)
            .len()
    }

    fn core_focus(&self) -> (Window, InputFocus) {
        match self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()) {
            Some(reply) => (reply.focus, reply.revert_to),
            None => (0, InputFocus::NONE),
        }
    }

    fn window_pid(&self, window: Window) -> Option<u32> {
        self.window_property(window, self.atoms.net_wm_pid, 1)
            .first()
            .copied()
            .filter(|p| *p != 0)
    }

    /// Mapped override-redirect children of the root: menus, combo popups,
    /// tooltips. Requests are pipelined so a busy desktop costs one round trip.
    fn mapped_popups(&self) -> HashSet<Window> {
        let tree = match self.conn.query_tree(self.root).map(|c| c.reply()) {
            Ok(Ok(tree)) => tree,
            _ => return HashSet::new(),
        };
        let cookies: Vec<_> = tree
            .children
            .iter()
            .filter_map(|w| self.conn.get_window_attributes(*w).ok().map(|c| (*w, c)))
            .collect();
        cookies
            .into_iter()
            .filter_map(|(w, c)| c.reply().ok().map(|a| (w, a)))
            .filter(|(_, a)| a.override_redirect && a.map_state == MapState::VIEWABLE)
            .map(|(w, _)| w)
            .collect()
    }

    /// Current server time (PropertyNotify round trip), bounded at 300 ms;
    /// `CurrentTime` (0) would lose to focus-stealing prevention.
    fn server_time(&self) -> u32 {
        let Ok(probe) = self.conn.generate_id() else {
            return 0;
        };
        let aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        if self
            .conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                probe,
                self.root,
                -1,
                -1,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &aux,
            )
            .is_err()
        {
            return 0;
        }
        let _ = self.conn.change_property(
            PropMode::REPLACE,
            probe,
            self.atoms.net_wm_pid,
            AtomEnum::STRING,
            8,
            1,
            &[0u8],
        );
        let _ = self.conn.flush();
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut time = 0;
        while Instant::now() < deadline {
            match self.conn.poll_for_event() {
                Ok(Some(Event::PropertyNotify(e))) if e.window == probe => {
                    time = e.time;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        let _ = self.conn.destroy_window(probe);
        let _ = self.conn.flush();
        time
    }

    fn activate(&self, target: Window, current: Window) {
        let event = ClientMessageEvent::new(
            32,
            target,
            self.atoms.net_active_window,
            [2, self.server_time(), current, 0, 0],
        );
        let _ = self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
        let _ = self.conn.flush();
    }

    fn set_focus(&self, window: Window, revert_to: InputFocus) {
        // BadWindow / BadMatch (destroyed or unmapped meanwhile) are expected
        // and swallowed: the verification loop decides what to do next.
        if let Ok(cookie) = self
            .conn
            .set_input_focus(revert_to, window, x11rb::CURRENT_TIME)
        {
            let _ = cookie.check();
        }
        let _ = self.conn.flush();
    }
}

/// What a background action must not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusSnapshot {
    core_focus: Window,
    revert_to: InputFocus,
    active: Option<Window>,
    stacking_top: Option<Window>,
    popups: HashSet<Window>,
    client_count: usize,
}

/// Outcome of the post-action check and restore.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusGuardReport {
    /// The core focus, active window or stacking top moved after the action.
    pub changed: bool,
    /// They were moved back (verified stable) before the tool returned.
    pub restored: bool,
    /// Human-readable list of what moved (`focus 0x..->0x..`, ...).
    pub changes: Vec<String>,
    /// A new override-redirect popup (menu, combo list) appeared, so its
    /// toolkit now holds a keyboard grab that cannot be released without
    /// closing it; the pid that owns it (falls back to the action's target).
    pub grab_held_by: Option<u32>,
    /// Milliseconds spent watching and restoring after delivery.
    pub elapsed_ms: u64,
}

impl FocusGuardReport {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "focus_changed": self.changed,
            "focus_restored": self.restored,
        });
        if !self.changes.is_empty() {
            v["focus_changes"] = serde_json::json!(self.changes);
        }
        if let Some(pid) = self.grab_held_by {
            v["grab_held_by"] = serde_json::json!(pid);
        }
        v
    }

    /// One sentence for the tool's text content; empty when nothing moved.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        if self.changed {
            s.push_str(&format!(
                " The application moved the desktop focus ({}); {}.",
                self.changes.join(", "),
                if self.restored {
                    "it was restored to the previous window"
                } else {
                    "restoring it did not hold"
                }
            ));
        }
        if let Some(pid) = self.grab_held_by {
            s.push_str(&format!(
                " A popup menu is open, so pid {pid} holds a keyboard grab until it closes."
            ));
        }
        s
    }
}

impl FocusSnapshot {
    /// Record the current state. `None` when there is no usable X display
    /// (the guard then degrades to a no-op rather than failing the action).
    pub fn capture() -> Option<Self> {
        let x = X::open().ok()?;
        let (core_focus, revert_to) = x.core_focus();
        Some(Self {
            core_focus,
            revert_to,
            active: x.active_window(),
            stacking_top: x.stacking_top(),
            popups: x.mapped_popups(),
            client_count: x.client_count(),
        })
    }

    fn diff(&self, x: &X) -> Vec<String> {
        let mut changes = Vec::new();
        let (focus, _) = x.core_focus();
        // PointerRoot (1) / None (0) are transient states while the WM moves
        // focus; the settle loop sees the final value.
        if focus > 1 && focus != self.core_focus {
            changes.push(format!("focus 0x{:x}->0x{:x}", self.core_focus, focus));
        }
        let active = x.active_window();
        if active != self.active && active.is_some() {
            changes.push(format!(
                "active 0x{:x}->0x{:x}",
                self.active.unwrap_or(0),
                active.unwrap_or(0)
            ));
        }
        let top = x.stacking_top();
        if top != self.stacking_top && top.is_some() {
            changes.push(format!(
                "stacking_top 0x{:x}->0x{:x}",
                self.stacking_top.unwrap_or(0),
                top.unwrap_or(0)
            ));
        }
        changes
    }

    /// Watch briefly for a change, restore if one happened, and report.
    /// `target_pid` names the action's application for the grab attribution.
    pub fn restore_if_changed(&self, target_pid: Option<u32>) -> FocusGuardReport {
        self.restore_if_changed_opts(target_pid, true)
    }

    /// [`Self::restore_if_changed`] with the settle watch optional: a body that
    /// already observed the desktop for longer than the watch (the MPX pointer
    /// route samples the screen ~250 ms after the release and has its own
    /// fast-path restore) checks once and returns unless a new top-level is
    /// still being mapped.
    pub fn restore_if_changed_opts(
        &self,
        target_pid: Option<u32>,
        settle_watch: bool,
    ) -> FocusGuardReport {
        let started = Instant::now();
        let Ok(x) = X::open() else {
            return FocusGuardReport::default();
        };
        let mut report = FocusGuardReport::default();

        // Settle watch: stop at the first observed change. A new top-level
        // (dialog being mapped) extends the watch, since the WM focuses it
        // only once it is mapped.
        let mut watch_until = if settle_watch {
            started + SETTLE_WATCH
        } else {
            started
        };
        if !settle_watch && x.client_count() > self.client_count {
            watch_until = started + SETTLE_WATCH_NEW_WINDOW;
        }
        let mut extended = false;
        let mut changes = self.diff(&x);
        while changes.is_empty() && Instant::now() < watch_until {
            std::thread::sleep(SETTLE_POLL);
            changes = self.diff(&x);
            if !extended && x.client_count() > self.client_count {
                extended = true;
                watch_until = started + SETTLE_WATCH_NEW_WINDOW;
            }
        }

        // Popups opened by the action: an open GTK/VCL menu grabs the keyboard.
        let new_popups: Vec<Window> = x
            .mapped_popups()
            .difference(&self.popups)
            .copied()
            .collect();
        if !new_popups.is_empty() {
            report.grab_held_by = new_popups
                .iter()
                .find_map(|w| x.window_pid(*w))
                .or(target_pid);
        }

        if changes.is_empty() {
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            return report;
        }
        report.changed = true;
        report.changes = changes;

        // Restore. EWMH re-activation makes the WM restore stacking and its
        // own focus bookkeeping; the explicit XSetInputFocus covers WMs that
        // treat _NET_ACTIVE_WINDOW as raise-only and the no-WM case.
        let mut stable = 0;
        let mut resent = 0;
        let deadline = started + RESTORE_BUDGET;
        self.reassert(&x);
        while Instant::now() < deadline {
            std::thread::sleep(RESTORE_POLL);
            let now = self.diff(&x);
            if now.is_empty() {
                stable += 1;
                if stable >= STABLE_POLLS {
                    report.restored = true;
                    break;
                }
                continue;
            }
            stable = 0;
            if resent < 2 {
                resent += 1;
                self.reassert(&x);
            }
        }
        if !report.restored {
            tracing::warn!(
                "background focus guard: could not restore {:?}",
                report.changes
            );
        }
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        report
    }

    fn reassert(&self, x: &X) {
        if let Some(prev) = self.active {
            x.activate(prev, x.active_window().unwrap_or(0));
        }
        if self.core_focus > 1 {
            x.set_focus(self.core_focus, self.revert_to);
        }
    }
}

/// Run `body` (a background delivery) between a snapshot and a restore.
/// The body's error still propagates; the guard only adds its report.
pub fn guarded<T>(
    target_pid: Option<u32>,
    body: impl FnOnce() -> Result<T>,
) -> Result<(T, Option<FocusGuardReport>)> {
    let snapshot = FocusSnapshot::capture();
    let value = body()?;
    let report = snapshot.map(|s| s.restore_if_changed(target_pid));
    Ok((value, report))
}

/// [`guarded`] for a body that has already let the desktop settle (the MPX
/// pointer press train and drag gesture): one post-check, no extra settle
/// watch, so the pointer branch's fast path keeps its latency.
pub fn guarded_settled<T>(
    target_pid: Option<u32>,
    body: impl FnOnce() -> Result<T>,
) -> Result<(T, Option<FocusGuardReport>)> {
    let snapshot = FocusSnapshot::capture();
    let value = body()?;
    let report = snapshot.map(|s| s.restore_if_changed_opts(target_pid, false));
    Ok((value, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_and_summary_name_what_moved() {
        let report = FocusGuardReport {
            changed: true,
            restored: true,
            changes: vec!["focus 0x1->0x2".into()],
            grab_held_by: Some(42),
            elapsed_ms: 7,
        };
        let json = report.to_json();
        assert_eq!(json["focus_changed"], true);
        assert_eq!(json["focus_restored"], true);
        assert_eq!(json["grab_held_by"], 42);
        assert_eq!(json["focus_changes"][0], "focus 0x1->0x2");
        let summary = report.summary();
        assert!(summary.contains("restored"), "{summary}");
        assert!(summary.contains("pid 42"), "{summary}");
    }

    #[test]
    fn quiet_report_is_silent() {
        let report = FocusGuardReport::default();
        assert_eq!(report.summary(), "");
        let json = report.to_json();
        assert_eq!(json["focus_changed"], false);
        assert!(json.get("grab_held_by").is_none());
        assert!(json.get("focus_changes").is_none());
    }

    #[test]
    fn capture_without_display_degrades_to_none() {
        let prior = std::env::var_os("DISPLAY");
        std::env::set_var("DISPLAY", ":9999999");
        let snapshot = FocusSnapshot::capture();
        match prior {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        assert!(snapshot.is_none());
    }
}
