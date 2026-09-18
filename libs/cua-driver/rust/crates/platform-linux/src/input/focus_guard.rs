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
//! moved *away from the user*: the previously active window is re-activated
//! through EWMH (so the WM's own bookkeeping and stacking follow) and the
//! previous core focus is re-set.
//!
//! A move that stays inside the target application is not an intrusion: when
//! the target app already owned the focus and it opens a dialog that mutter
//! focuses, the user (or the agent) asked for that dialog. Re-activating the
//! main window there would undo the app's own behaviour and send the next
//! pid-only keystrokes to the wrong window, so the guard leaves it and reports
//! `focus_outcome: same_app_dialog` instead. The outcome is reported, never
//! hidden: a toolkit keyboard grab held by an open menu cannot be undone
//! without closing the menu, so it is surfaced as `grab_held_by`.

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
/// Longer watch once a new top-level appeared during the short one while the
/// focus belonged to *another* application: a dialog (LibreOffice's take
/// ~1 s to build) is focused by the WM only when mapped, and that steal must
/// be undone. When the target app already owns the focus the new window is
/// its own and there is nothing to wait for.
const SETTLE_WATCH_NEW_WINDOW: Duration = Duration::from_millis(1400);
const SETTLE_POLL: Duration = Duration::from_millis(30);
/// Bound on the restore loop: re-activation, verification, one re-send.
const RESTORE_BUDGET: Duration = Duration::from_millis(1200);
const RESTORE_POLL: Duration = Duration::from_millis(50);
/// Consecutive stable polls before the restore is called done.
const STABLE_POLLS: u32 = 3;
/// Bound on the ancestor / transient walk when attributing a window to a pid.
const OWNER_WALK_LIMIT: usize = 16;

struct Atoms {
    net_active_window: u32,
    net_client_list_stacking: u32,
    net_wm_pid: u32,
    net_wm_name: u32,
    utf8_string: u32,
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
            net_wm_name: intern(b"_NET_WM_NAME")?,
            utf8_string: intern(b"UTF8_STRING")?,
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

    fn client_list(&self) -> Vec<Window> {
        self.window_property(self.root, self.atoms.net_client_list_stacking, u32::MAX)
    }

    fn stacking_top(&self) -> Option<Window> {
        self.client_list().last().copied().filter(|w| *w != 0)
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

    fn transient_for(&self, window: Window) -> Option<Window> {
        self.window_property(window, AtomEnum::WM_TRANSIENT_FOR.into(), 1)
            .first()
            .copied()
            .filter(|w| *w != 0 && *w != self.root)
    }

    /// The window still exists and is mapped (a destroyed dialog fails both).
    fn window_viewable(&self, window: Window) -> bool {
        self.conn
            .get_window_attributes(window)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|a| a.map_state == MapState::VIEWABLE)
    }

    fn parent(&self, window: Window) -> Option<Window> {
        let tree = self.conn.query_tree(window).ok()?.reply().ok()?;
        (tree.parent != 0 && tree.parent != self.root).then_some(tree.parent)
    }

    /// The pid that owns `window`: its own `_NET_WM_PID`, else the one of
    /// the window it is transient for, else its ancestors' (the core focus
    /// often sits on a child of the client toplevel).
    fn owner_pid(&self, window: Window) -> Option<u32> {
        let mut current = window;
        for _ in 0..OWNER_WALK_LIMIT {
            if current == 0 || current == self.root {
                return None;
            }
            if let Some(pid) = self.window_pid(current) {
                return Some(pid);
            }
            match self.transient_for(current).or_else(|| self.parent(current)) {
                Some(next) if next != current => current = next,
                _ => return None,
            }
        }
        None
    }

    fn window_title(&self, window: Window) -> String {
        let read = |atom: u32, ty: u32| -> Option<String> {
            let reply = self
                .conn
                .get_property(false, window, atom, ty, 0, 256)
                .ok()?
                .reply()
                .ok()?;
            (!reply.value.is_empty()).then(|| String::from_utf8_lossy(&reply.value).into_owned())
        };
        read(self.atoms.net_wm_name, self.atoms.utf8_string)
            .or_else(|| read(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
            .unwrap_or_default()
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

/// Where a focus move went, relative to the action's target application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMove {
    /// The target app owned the focus before and still does: it opened one
    /// of its own windows (a dialog). Not an intrusion; leave it.
    SameAppDialog,
    /// The focus left the user's application (or arrived in the target from
    /// another app): restore it.
    OtherApp,
}

/// The same-app rule: a move is the app's own dialog only when the target
/// pid owned the focus before the action *and* owns it now. A move from a
/// decoy (another pid) into the target's dialog is a steal, as is a move
/// from the target to anything else.
pub fn classify_focus_move(
    target_pid: Option<u32>,
    previous_owner: Option<u32>,
    new_owner: Option<u32>,
) -> FocusMove {
    match (target_pid, previous_owner, new_owner) {
        (Some(target), Some(previous), Some(new)) if previous == target && new == target => {
            FocusMove::SameAppDialog
        }
        _ => FocusMove::OtherApp,
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
    clients: HashSet<Window>,
    /// Pid that owned the active window (else the core focus) before the
    /// action: the application the user was working in.
    previous_owner: Option<u32>,
}

/// A window of the target application that appeared or took the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SameAppWindow {
    pub window: u64,
    pub title: String,
    /// It received the desktop focus (else it was merely mapped).
    pub focused: bool,
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
    /// The target application opened (and possibly focused) one of its own
    /// windows; the guard left it alone.
    pub same_app_window: Option<SameAppWindow>,
    /// The window that held the focus before the action no longer exists
    /// (the action closed a dialog); there is nothing to restore to, the
    /// focus went to `same_app_window` / wherever the WM put it.
    pub closed_window: Option<u64>,
    /// Milliseconds spent watching and restoring after delivery.
    pub elapsed_ms: u64,
}

impl FocusGuardReport {
    /// `window_closed` / `same_app_dialog` / `restored` / `not_restored`
    /// once something moved.
    pub fn outcome(&self) -> Option<&'static str> {
        if !self.changed {
            return None;
        }
        Some(if self.closed_window.is_some() {
            "window_closed"
        } else if self.same_app_window.as_ref().is_some_and(|w| w.focused) {
            "same_app_dialog"
        } else if self.restored {
            "restored"
        } else {
            "not_restored"
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "focus_changed": self.changed,
            "focus_restored": self.restored,
        });
        if let Some(outcome) = self.outcome() {
            v["focus_outcome"] = serde_json::json!(outcome);
        }
        if !self.changes.is_empty() {
            v["focus_changes"] = serde_json::json!(self.changes);
        }
        if let Some(pid) = self.grab_held_by {
            v["grab_held_by"] = serde_json::json!(pid);
        }
        if let Some(closed) = self.closed_window {
            v["focus_window_closed"] = serde_json::json!(closed);
        }
        if let Some(window) = &self.same_app_window {
            v["app_window_opened"] = serde_json::json!({
                "window_id": window.window,
                "title": window.title,
                "focused": window.focused,
            });
        }
        v
    }

    /// A `window_change` evidence item when the target application mapped or
    /// focused one of its own windows, or closed the one holding the focus:
    /// the public action contract keeps `evidence[]`, while the flat guard
    /// fields are reduced away.
    pub fn evidence_item(&self) -> Option<serde_json::Value> {
        if let Some(closed) = self.closed_window {
            return Some(serde_json::json!({
                "kind": "window_change",
                "detail": format!(
                    "window {closed}, which held the focus, was closed by the action{}",
                    self.same_app_window
                        .as_ref()
                        .map(|w| format!("; the focus moved to {} \"{}\"", w.window, w.title))
                        .unwrap_or_default()
                ),
            }));
        }
        let window = self.same_app_window.as_ref()?;
        Some(serde_json::json!({
            "kind": "window_change",
            "detail": format!(
                "the application {} its own window {} \"{}\"",
                if window.focused { "opened and focused" } else { "opened" },
                window.window,
                window.title
            ),
        }))
    }

    /// One sentence for the tool's text content; empty when nothing moved.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        match (&self.same_app_window, self.changed) {
            _ if self.closed_window.is_some() => s.push_str(&format!(
                " The window that held the focus ({}) was closed by this action \
                 (focus_outcome=window_closed); the focus moved to {}.",
                self.closed_window.unwrap_or(0),
                self.same_app_window
                    .as_ref()
                    .map(|w| format!("{} \"{}\"", w.window, w.title))
                    .unwrap_or_else(|| "another window".to_owned())
            )),
            (Some(window), _) if window.focused => s.push_str(&format!(
                " The application opened its own window {} \"{}\" and it now holds the \
                 focus (focus_outcome=same_app_dialog, left in place); pid-only \
                 type_text/press_key go to that window.",
                window.window, window.title
            )),
            (Some(window), false) => s.push_str(&format!(
                " The application opened its own window {} \"{}\".",
                window.window, window.title
            )),
            (_, true) => s.push_str(&format!(
                " The application moved the desktop focus ({}); {}.",
                self.changes.join(", "),
                if self.restored {
                    "it was restored to the previous window (focus_outcome=restored)"
                } else {
                    "restoring it did not hold (focus_outcome=not_restored)"
                }
            )),
            _ => {}
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
        let active = x.active_window();
        let previous_owner = active
            .or_else(|| (core_focus > 1).then_some(core_focus))
            .and_then(|w| x.owner_pid(w));
        Some(Self {
            core_focus,
            revert_to,
            active,
            stacking_top: x.stacking_top(),
            popups: x.mapped_popups(),
            clients: x.client_list().into_iter().collect(),
            previous_owner,
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

    /// Managed toplevels that were not in the client list at capture time,
    /// topmost last.
    fn new_clients(&self, x: &X) -> Vec<Window> {
        x.client_list()
            .into_iter()
            .filter(|w| *w != 0 && !self.clients.contains(w))
            .collect()
    }

    /// The window the desktop focus sits on now (the WM's active window when
    /// it still exists — `_NET_ACTIVE_WINDOW` lags a destroyed dialog by a
    /// beat — else the core focus).
    fn current_focus_window(&self, x: &X) -> Option<Window> {
        x.active_window().filter(|w| x.window_viewable(*w)).or_else(|| {
            let (focus, _) = x.core_focus();
            (focus > 1).then_some(focus)
        })
    }

    /// The window that held the focus at capture time (active, else core).
    fn previous_focus_window(&self) -> Option<Window> {
        self.active
            .or_else(|| (self.core_focus > 1).then_some(self.core_focus))
    }

    /// Watch briefly for a change, restore if one happened, and report.
    /// `target_pid` names the action's application for the grab attribution
    /// and the same-app rule.
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
        // The target app owns the focus: whatever it maps next is its own.
        let target_owns_focus = target_pid.is_some() && self.previous_owner == target_pid;

        // Settle watch: stop at the first observed change. A new top-level
        // (dialog being mapped) extends the watch when the focus belongs to
        // another application, since the WM focuses it only once mapped and
        // that steal must be undone; when the target app already owns the
        // focus the new window is its own and the watch ends at once.
        let mut watch_until = if settle_watch {
            started + SETTLE_WATCH
        } else {
            started
        };
        let mut extended = false;
        let mut changes = self.diff(&x);
        let mut new_clients = self.new_clients(&x);
        let mut own_window = self.own_new_window(&x, target_pid, &new_clients);
        if !settle_watch && !new_clients.is_empty() && own_window.is_none() {
            extended = true;
            watch_until = started + SETTLE_WATCH_NEW_WINDOW;
        }
        while changes.is_empty() && own_window.is_none() && Instant::now() < watch_until {
            std::thread::sleep(SETTLE_POLL);
            changes = self.diff(&x);
            new_clients = self.new_clients(&x);
            own_window = self.own_new_window(&x, target_pid, &new_clients);
            if !extended && !new_clients.is_empty() && own_window.is_none() {
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
            report.same_app_window = own_window;
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            return report;
        }
        report.changed = true;
        report.changes = changes;

        // The action closed the window that held the focus (Escape / OK on
        // a dialog): nothing to restore to, the WM already moved on.
        let focus_window = self.current_focus_window(&x);
        if let Some(previous) = self
            .previous_focus_window()
            .filter(|w| !x.window_viewable(*w))
        {
            report.closed_window = Some(u64::from(previous));
            report.same_app_window = focus_window.map(|w| SameAppWindow {
                window: u64::from(w),
                title: x.window_title(w),
                focused: true,
            });
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            return report;
        }

        // Same-app rule: the focus stayed inside the target application
        // (it opened a dialog). Leave it and say so.
        let new_owner = focus_window.and_then(|w| x.owner_pid(w));
        if target_owns_focus
            && classify_focus_move(target_pid, self.previous_owner, new_owner)
                == FocusMove::SameAppDialog
        {
            let window = focus_window.unwrap_or(0);
            report.same_app_window = Some(SameAppWindow {
                window: u64::from(window),
                title: x.window_title(window),
                focused: true,
            });
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            return report;
        }

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

    /// The topmost freshly mapped toplevel of the target pid, when the target
    /// already owned the focus (so the window is the app's own and the watch
    /// need not wait for the WM to focus it).
    fn own_new_window(
        &self,
        x: &X,
        target_pid: Option<u32>,
        new_clients: &[Window],
    ) -> Option<SameAppWindow> {
        if target_pid.is_none() || self.previous_owner != target_pid {
            return None;
        }
        let window = new_clients
            .iter()
            .rev()
            .copied()
            .find(|w| x.owner_pid(*w) == target_pid)?;
        Some(SameAppWindow {
            window: u64::from(window),
            title: x.window_title(window),
            focused: false,
        })
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
            same_app_window: None,
            closed_window: None,
            elapsed_ms: 7,
        };
        let json = report.to_json();
        assert_eq!(json["focus_changed"], true);
        assert_eq!(json["focus_restored"], true);
        assert_eq!(json["focus_outcome"], "restored");
        assert_eq!(json["grab_held_by"], 42);
        assert_eq!(json["focus_changes"][0], "focus 0x1->0x2");
        assert!(report.evidence_item().is_none());
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
        assert!(json.get("focus_outcome").is_none());
        assert!(json.get("grab_held_by").is_none());
        assert!(json.get("focus_changes").is_none());
    }

    #[test]
    fn same_app_dialog_is_reported_not_restored() {
        let report = FocusGuardReport {
            changed: true,
            restored: false,
            changes: vec!["active 0x1->0x2".into()],
            grab_held_by: None,
            same_app_window: Some(SameAppWindow {
                window: 2,
                title: "Brightness-Contrast".into(),
                focused: true,
            }),
            closed_window: None,
            elapsed_ms: 3,
        };
        let json = report.to_json();
        assert_eq!(json["focus_outcome"], "same_app_dialog");
        assert_eq!(json["app_window_opened"]["window_id"], 2);
        assert_eq!(json["app_window_opened"]["focused"], true);
        let evidence = report.evidence_item().expect("window_change evidence");
        assert_eq!(evidence["kind"], "window_change");
        assert!(evidence["detail"]
            .as_str()
            .unwrap()
            .contains("Brightness-Contrast"));
        let summary = report.summary();
        assert!(summary.contains("same_app_dialog"), "{summary}");
        assert!(summary.contains("Brightness-Contrast"), "{summary}");
        assert!(!summary.contains("restored"), "{summary}");
    }

    #[test]
    fn mapped_but_unfocused_own_window_is_mentioned_without_a_focus_outcome() {
        let report = FocusGuardReport {
            same_app_window: Some(SameAppWindow {
                window: 9,
                title: "Format Cells".into(),
                focused: false,
            }),
            ..FocusGuardReport::default()
        };
        assert!(report.outcome().is_none());
        assert!(report.summary().contains("Format Cells"));
        assert!(report.evidence_item().is_some());
    }

    #[test]
    fn a_closed_focus_holder_is_window_closed_evidence_not_a_failed_restore() {
        let report = FocusGuardReport {
            changed: true,
            restored: false,
            changes: vec!["focus 0x2->0x1".into()],
            closed_window: Some(2),
            same_app_window: Some(SameAppWindow {
                window: 1,
                title: "GNU Image Manipulation Program".into(),
                focused: true,
            }),
            ..FocusGuardReport::default()
        };
        assert_eq!(report.outcome(), Some("window_closed"));
        assert_eq!(report.to_json()["focus_outcome"], "window_closed");
        assert_eq!(report.to_json()["focus_window_closed"], 2);
        let evidence = report.evidence_item().unwrap();
        assert!(evidence["detail"].as_str().unwrap().contains("closed"));
        assert!(report.summary().contains("window_closed"));
        assert!(!report.summary().contains("not_restored"));
    }

    #[test]
    fn same_app_rule_only_when_the_target_owned_the_focus_before_and_after() {
        // The target app was active and opened its own dialog: leave it.
        assert_eq!(
            classify_focus_move(Some(7), Some(7), Some(7)),
            FocusMove::SameAppDialog
        );
        // Decoy case: another app owned the focus and the target's dialog
        // stole it -> restore.
        assert_eq!(
            classify_focus_move(Some(7), Some(3), Some(7)),
            FocusMove::OtherApp
        );
        // The target was active and something else took the focus -> restore.
        assert_eq!(
            classify_focus_move(Some(7), Some(7), Some(3)),
            FocusMove::OtherApp
        );
        // Unknown owners never qualify.
        assert_eq!(
            classify_focus_move(Some(7), None, Some(7)),
            FocusMove::OtherApp
        );
        assert_eq!(
            classify_focus_move(Some(7), Some(7), None),
            FocusMove::OtherApp
        );
        assert_eq!(classify_focus_move(None, Some(7), Some(7)), FocusMove::OtherApp);
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
