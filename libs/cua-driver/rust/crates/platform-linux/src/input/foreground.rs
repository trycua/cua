//! X11 foreground transaction (`delivery_mode:"foreground"`).
//!
//! A foreground action behaves like a user at the keyboard: activate + raise the
//! target toplevel, wait until the window manager confirms that it is the
//! active window AND that the core X input focus sits inside it, then inject
//! real input (XTest) and report where focus ended up. The target is left
//! active afterwards — restoring the previously active window would pop down
//! any menu the click just opened and would move keyboard focus away from the
//! widget a following `type_text` needs.
//!
//! Everything here is bounded: the activation/confirmation phase and the
//! post-check run on watchdog threads with hard deadlines, so a wedged X
//! server, a client holding a server grab, or a window that never takes focus
//! yields a structured `foreground_timeout` / `foreground_unavailable` error
//! instead of a hung tool call. Only x11rb (a pure-Rust, thread-safe client)
//! is used: protocol errors come back as `Result`s rather than going through
//! Xlib's process-wide error handler.

use anyhow::{anyhow, Result};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
    PropMode, Window, WindowClass, CLIENT_MESSAGE_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// Error-code prefixes carried in the `anyhow` message (`<code>: <detail>`)
/// so tool handlers can attach a structured `code` without a bespoke error type.
pub const CODE_UNAVAILABLE: &str = "foreground_unavailable";
pub const CODE_TIMEOUT: &str = "foreground_timeout";

/// Tunables for one foreground transaction.
#[derive(Clone, Copy, Debug)]
pub struct ForegroundOptions {
    /// How long to wait for the WM to confirm activation + input focus.
    pub settle: Duration,
    /// Whether the body injects keyboard input. Keyboard delivery is routed by
    /// the X input focus, so the post-check is reported for it.
    pub keyboard: bool,
}

impl ForegroundOptions {
    /// Pointer actions land by stacking; a short confirmation suffices.
    pub fn pointer() -> Self {
        Self {
            settle: Duration::from_millis(800),
            keyboard: false,
        }
    }

    /// Keyboard actions need the input focus, which GTK/VCL toplevels accept a
    /// beat after being raised; give them a real budget.
    pub fn keyboard() -> Self {
        Self {
            settle: Duration::from_millis(1500),
            keyboard: true,
        }
    }

    /// Legacy `settle_ms` hint: folded into the pointer budget (never shorter).
    pub fn from_settle_hint(settle_ms: u64) -> Self {
        let mut opts = Self::pointer();
        opts.settle = opts.settle.max(Duration::from_millis(settle_ms));
        opts
    }
}

/// Where the X input focus was after the body ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusAfter {
    /// Inside the target toplevel.
    Target,
    /// Inside another toplevel owned by the same process (a dialog opened or
    /// closed, a new document window, ...).
    SamePid,
    /// Somewhere else entirely.
    Elsewhere,
    /// Could not be determined before the post-check deadline.
    Unknown,
}

impl FocusAfter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::SamePid => "same_pid",
            Self::Elsewhere => "elsewhere",
            Self::Unknown => "unknown",
        }
    }
}

/// What the transaction observed; surfaced in tool structured content.
#[derive(Clone, Debug)]
pub struct ForegroundReport {
    /// The target was already active and focused; no activation was sent.
    pub already_active: bool,
    /// Activation had to be re-sent once before the WM confirmed it.
    pub retried_activation: bool,
    /// Milliseconds from start until activation + focus were confirmed.
    pub confirm_ms: u64,
    /// Focus location after the body.
    pub focus_after: FocusAfter,
    /// Toplevels of the target's process (managed windows and popup menus)
    /// that appeared or vanished between the activation and the post-check:
    /// a menu opened, a dialog mapped or closed. `None` when nothing changed
    /// or the observation was unavailable.
    pub window_change: Option<String>,
}

impl ForegroundReport {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "activated": !self.already_active,
            "retried_activation": self.retried_activation,
            "confirm_ms": self.confirm_ms,
            "focus_after": self.focus_after.as_str(),
        });
        if let Some(change) = &self.window_change {
            v["window_change"] = serde_json::json!(change);
        }
        v
    }

    /// Focus is proven to have stayed inside the target process after the
    /// body ran (the target toplevel itself, or a dialog/popup it owns).
    pub fn focus_kept(&self) -> bool {
        matches!(self.focus_after, FocusAfter::Target | FocusAfter::SamePid)
    }
}

/// The on-screen toplevels of `pid` (managed windows plus override-redirect
/// popups), as `(window, description)` pairs, for a before/after diff.
fn pid_window_set(pid: Option<u32>) -> Vec<(u64, String)> {
    let mut set: Vec<(u64, String)> = crate::x11::list_windows(pid)
        .into_iter()
        .filter(|w| w.is_on_screen)
        .map(|w| {
            let title = if w.title.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", w.title)
            };
            (w.xid, format!("window {}{title}", w.xid))
        })
        .collect();
    set.extend(
        super::mapped_popup_windows()
            .into_iter()
            .filter(|p| pid.is_none() || p.pid.is_none() || p.pid == pid)
            .map(|p| (p.window, p.describe())),
    );
    set
}

/// Describe what changed between two window sets, or `None` when nothing did.
pub(crate) fn describe_window_change(
    before: &[(u64, String)],
    after: &[(u64, String)],
) -> Option<String> {
    let appeared: Vec<&str> = after
        .iter()
        .filter(|(id, _)| !before.iter().any(|(b, _)| b == id))
        .map(|(_, d)| d.as_str())
        .collect();
    let vanished: Vec<&str> = before
        .iter()
        .filter(|(id, _)| !after.iter().any(|(a, _)| a == id))
        .map(|(_, d)| d.as_str())
        .collect();
    if appeared.is_empty() && vanished.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !appeared.is_empty() {
        parts.push(format!("appeared: {}", appeared.join(", ")));
    }
    if !vanished.is_empty() {
        parts.push(format!("closed: {}", vanished.join(", ")));
    }
    Some(parts.join("; "))
}

/// How long the toolkit gets to map a menu / dialog after the body before
/// the window set is compared.
const WINDOW_CHANGE_SETTLE: Duration = Duration::from_millis(150);

/// Extract the structured code prefix from a foreground error message.
pub fn error_code(error: &anyhow::Error) -> Option<&'static str> {
    let text = error.to_string();
    if text.starts_with(CODE_TIMEOUT) {
        Some(CODE_TIMEOUT)
    } else if text.starts_with(CODE_UNAVAILABLE) {
        Some(CODE_UNAVAILABLE)
    } else {
        None
    }
}

/// Run `f` on a detached thread and wait at most `deadline` for its result.
/// The thread is intentionally leaked on timeout: a blocking X request cannot
/// be cancelled, but the caller must not hang with it.
fn run_with_deadline<T: Send + 'static>(
    deadline: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("cua-x11-foreground".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(deadline).ok()
}

struct X11 {
    conn: RustConnection,
    root: Window,
    net_active_window: u32,
    net_wm_pid: u32,
}

impl X11 {
    fn open() -> Result<Self> {
        let (conn, screen) = RustConnection::connect(None).map_err(|e| {
            anyhow!("{CODE_UNAVAILABLE}: cannot open DISPLAY to verify X11 input focus: {e}")
        })?;
        let root = conn.setup().roots[screen].root;
        let net_active_window = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let net_wm_pid = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        Ok(Self {
            conn,
            root,
            net_active_window,
            net_wm_pid,
        })
    }

    fn active_window(&self) -> Option<Window> {
        let reply = self
            .conn
            .get_property(
                false,
                self.root,
                self.net_active_window,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let value = reply.value32()?.next();
        value.filter(|w| *w != 0)
    }

    fn window_pid(&self, window: Window) -> Option<u32> {
        let reply = self
            .conn
            .get_property(false, window, self.net_wm_pid, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        let value = reply.value32()?.next();
        value.filter(|p| *p != 0)
    }

    /// Current server time via the PropertyNotify round-trip. `_NET_ACTIVE_WINDOW`
    /// stamped `CurrentTime` (0) loses to focus-stealing prevention whenever any
    /// newer user input exists. Bounded: falls back to 0 after 300 ms.
    fn server_time(&self) -> u32 {
        let probe = match self.conn.generate_id() {
            Ok(id) => id,
            Err(_) => return 0,
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
        let _ = x11rb::protocol::xproto::change_property(
            &self.conn,
            PropMode::REPLACE,
            probe,
            self.net_wm_pid,
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

    fn activate(&self, target: Window, prior: Window) -> Result<()> {
        let time = self.server_time();
        let event = ClientMessageEvent::new(
            32,
            target,
            self.net_active_window,
            [2, time, prior, 0, 0],
        );
        debug_assert_eq!(event.response_type, CLIENT_MESSAGE_EVENT);
        self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        // Set the core focus explicitly as well (what `xdotool windowactivate`
        // does): WMs with focus-stealing prevention honour `_NET_ACTIVE_WINDOW`
        // as raise-only. BadMatch on a not-yet-viewable window is expected and
        // ignored; the confirmation loop decides.
        if let Ok(cookie) = self
            .conn
            .set_input_focus(InputFocus::PARENT, target, x11rb::CURRENT_TIME)
        {
            let _ = cookie.check();
        }
        self.conn.flush()?;
        Ok(())
    }

    fn focused(&self) -> Option<Window> {
        self.conn
            .get_input_focus()
            .ok()?
            .reply()
            .ok()
            .map(|r| r.focus)
            .filter(|f| *f > 1) // 0 = None, 1 = PointerRoot
    }

    /// Whether `focused` is `target` or one of its descendants.
    fn is_within(&self, mut focused: Window, target: Window) -> bool {
        for _ in 0..64 {
            if focused == target {
                return true;
            }
            if focused == 0 || focused == self.root {
                return false;
            }
            let Ok(cookie) = self.conn.query_tree(focused) else {
                return false;
            };
            let Ok(reply) = cookie.reply() else {
                return false;
            };
            if reply.parent == 0 || reply.parent == focused {
                return false;
            }
            focused = reply.parent;
        }
        false
    }

    /// Walk up from `window` to the first ancestor carrying `_NET_WM_PID`.
    fn owning_pid(&self, mut window: Window) -> Option<u32> {
        for _ in 0..64 {
            if let Some(pid) = self.window_pid(window) {
                return Some(pid);
            }
            let reply = self.conn.query_tree(window).ok()?.reply().ok()?;
            if reply.parent == 0 || reply.parent == self.root || reply.parent == window {
                return None;
            }
            window = reply.parent;
        }
        None
    }

    fn focus_is_within(&self, target: Window) -> bool {
        match self.focused() {
            Some(f) => self.is_within(f, target),
            None => false,
        }
    }
}

struct ConfirmOutcome {
    confirmed: bool,
    already_active: bool,
    retried: bool,
    elapsed: Duration,
    active_after: Window,
    focus_within: bool,
}

fn confirm_phase(target: Window, settle: Duration) -> Result<ConfirmOutcome> {
    let x = X11::open()?;
    let start = Instant::now();
    let prior = x.active_window().unwrap_or(0);
    let mut retried = false;
    // Already active and focused: leave the window (and any open menu / grab)
    // untouched — re-activating is what popped GTK menus back down.
    if prior == target && x.focus_is_within(target) {
        return Ok(ConfirmOutcome {
            confirmed: true,
            already_active: true,
            retried: false,
            elapsed: start.elapsed(),
            active_after: target,
            focus_within: true,
        });
    }
    x.activate(target, prior)?;
    let deadline = start + settle;
    let retry_at = start + settle.mul_f32(0.4);
    let mut retry_pending = true;
    loop {
        let active = x.active_window() == Some(target);
        let focus_within = x.focus_is_within(target);
        if active && focus_within {
            return Ok(ConfirmOutcome {
                confirmed: true,
                already_active: false,
                retried,
                elapsed: start.elapsed(),
                active_after: target,
                focus_within: true,
            });
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(ConfirmOutcome {
                confirmed: false,
                already_active: false,
                retried,
                elapsed: start.elapsed(),
                active_after: x.active_window().unwrap_or(0),
                focus_within,
            });
        }
        if retry_pending && now >= retry_at {
            retry_pending = false;
            retried = true;
            x.activate(target, x.active_window().unwrap_or(0))?;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn post_check(target: Window) -> FocusAfter {
    let Ok(x) = X11::open() else {
        return FocusAfter::Unknown;
    };
    let Some(focused) = x.focused() else {
        return FocusAfter::Elsewhere;
    };
    if x.is_within(focused, target) {
        return FocusAfter::Target;
    }
    match (x.owning_pid(target), x.owning_pid(focused)) {
        (Some(a), Some(b)) if a == b => FocusAfter::SamePid,
        _ => FocusAfter::Elsewhere,
    }
}

/// Activate `xid`, confirm it holds the active window + input focus within
/// `opts.settle`, run `body`, then (for keyboard actions) report where focus
/// ended up. Returns `foreground_unavailable` (no input sent) when the WM never
/// confirms, and `foreground_timeout` when the X server stops answering.
pub fn with_x11_foreground_opts<T>(
    xid: u64,
    opts: ForegroundOptions,
    body: impl FnOnce() -> Result<T>,
) -> Result<(T, ForegroundReport)> {
    let target = xid as Window;
    let phase_budget = opts.settle + Duration::from_millis(1500);
    let outcome = run_with_deadline(phase_budget, move || confirm_phase(target, opts.settle))
        .ok_or_else(|| {
            anyhow!(
                "{CODE_TIMEOUT}: the X server did not answer the activation of window \
                 0x{xid:x} within {phase_budget:?}; no input was sent"
            )
        })??;
    if !outcome.confirmed {
        return Err(anyhow!(
            "{CODE_UNAVAILABLE}: window 0x{xid:x} did not become the active, focused \
             window within {:?} (active=0x{:x}, focus_within_target={}); no input was sent. \
             The window may be minimized, on another workspace, or blocked by a modal \
             dialog — bring_to_front it or target that dialog instead",
            opts.settle,
            outcome.active_after,
            outcome.focus_within
        ));
    }
    let target_pid = crate::x11::window_pid(xid);
    let windows_before = pid_window_set(target_pid);
    let value = body()?;
    // The post-check is what the tool reports as evidence for both keyboard
    // and pointer bodies: a real click that opened a menu or a dialog moves
    // the window set / focus within the target's process, and one that
    // landed elsewhere moves focus out of it.
    std::thread::sleep(WINDOW_CHANGE_SETTLE);
    let focus_after = run_with_deadline(Duration::from_millis(1500), move || post_check(target))
        .unwrap_or(FocusAfter::Unknown);
    let window_change = run_with_deadline(Duration::from_millis(1500), move || {
        pid_window_set(target_pid)
    })
    .and_then(|after| describe_window_change(&windows_before, &after));
    Ok((
        value,
        ForegroundReport {
            already_active: outcome.already_active,
            retried_activation: outcome.retried,
            confirm_ms: outcome.elapsed.as_millis() as u64,
            focus_after,
            window_change,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_recognises_prefixes() {
        assert_eq!(
            error_code(&anyhow!("{CODE_TIMEOUT}: x")),
            Some(CODE_TIMEOUT)
        );
        assert_eq!(
            error_code(&anyhow!("{CODE_UNAVAILABLE}: y")),
            Some(CODE_UNAVAILABLE)
        );
        assert_eq!(error_code(&anyhow!("other")), None);
    }

    #[test]
    fn deadline_returns_none_when_work_hangs() {
        let started = Instant::now();
        let result = run_with_deadline(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        assert!(result.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn deadline_returns_value_when_work_finishes() {
        assert_eq!(run_with_deadline(Duration::from_secs(2), || 7), Some(7));
    }

    #[test]
    fn keyboard_budget_exceeds_pointer_budget() {
        assert!(ForegroundOptions::keyboard().settle > ForegroundOptions::pointer().settle);
        assert!(ForegroundOptions::from_settle_hint(80).settle >= Duration::from_millis(400));
        assert_eq!(
            ForegroundOptions::from_settle_hint(5000).settle,
            Duration::from_millis(5000)
        );
    }

    #[test]
    fn report_json_shape() {
        let report = ForegroundReport {
            already_active: true,
            retried_activation: false,
            confirm_ms: 3,
            focus_after: FocusAfter::SamePid,
            window_change: Some("appeared: popup window 7 (200x300 at 1,2)".into()),
        };
        let json = report.to_json();
        assert_eq!(json["activated"], false);
        assert_eq!(json["focus_after"], "same_pid");
        assert_eq!(json["confirm_ms"], 3);
        assert_eq!(json["window_change"], "appeared: popup window 7 (200x300 at 1,2)");
        assert!(report.focus_kept());
    }

    #[test]
    fn window_change_diff_names_appeared_and_closed() {
        let before = vec![(1u64, "window 1 \"GIMP\"".to_string()), (2, "window 2".to_string())];
        let after = vec![(1u64, "window 1 \"GIMP\"".to_string()), (9, "popup window 9".to_string())];
        assert_eq!(
            describe_window_change(&before, &after).as_deref(),
            Some("appeared: popup window 9; closed: window 2")
        );
        assert_eq!(describe_window_change(&before, &before), None);
    }

    #[test]
    fn unavailable_display_is_structured() {
        // Force a failing connect regardless of the host environment.
        let prior = std::env::var_os("DISPLAY");
        std::env::set_var("DISPLAY", ":9999999");
        let result = with_x11_foreground_opts(0x1234, ForegroundOptions::pointer(), || Ok(()));
        match prior {
            Some(v) => std::env::set_var("DISPLAY", v),
            None => std::env::remove_var("DISPLAY"),
        }
        let error = result.err().expect("connect must fail");
        assert!(error_code(&error).is_some(), "{error}");
    }
}
