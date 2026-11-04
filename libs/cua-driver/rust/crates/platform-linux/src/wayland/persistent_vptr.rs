//! Persistent virtual-pointer for stateful `mouse_button_down` / `mouse_drag` /
//! `mouse_button_up`.
//!
//! The non-persistent path in [`crate::wayland::click`] opens its own
//! `ZwlrVirtualPointerV1`, presses, releases, drops the connection — useful
//! for one-shot clicks but useless for held-button drags: each tool call
//! emits a fresh device whose press/release pair is matched by the
//! compositor, so apps that distinguish a real drag (press, motion+, release)
//! from a series of clicks (press, release, press, release, …) miss the
//! drag entirely.
//!
//! This module keeps the virtual-pointer alive across tool calls. A single
//! owner thread per process owns one Wayland `Connection`, one `EventQueue`,
//! and a map of `cursor_id -> ActivePointer`. Commands are sent over a
//! `crossbeam-channel`; replies come back on a per-call reply channel so the
//! caller blocks until the compositor has roundtripped.
//!
//! Lifecycle:
//! - First `press` for a cursor_id binds a fresh `ZwlrVirtualPointerV1`,
//!   activates the foreign-toplevel target window once, presses the button,
//!   adds to the held-button set, roundtrips.
//! - Subsequent `move_to` calls emit `motion_absolute` on the same vptr (no
//!   activate — would steal focus mid-drag) and roundtrip.
//! - `release` emits a button release, removes from the held set; if the
//!   set is empty the vptr is destroyed and the map entry dropped.
//! - On `Connection` roundtrip failure (compositor restart / disconnect)
//!   the owner thread tears down its connection and accepts the next
//!   command on a fresh one, emitting a typed error for the in-flight call.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender};
use wayland_client::{protocol::wl_pointer::ButtonState, Connection};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::{evdev_pointer_button, open_vptr_session, AimedPoint};
use crate::input::action::Action;

/// One in-flight command from the public API to the owner thread.
enum Cmd {
    Press {
        cursor_id: String,
        window_id: u64,
        at: AimedPoint,
        button: u8,
        /// The tool call the press serves: bound on the owner thread, so a
        /// call that has ended presses nothing.
        action: Option<Arc<Action>>,
        reply: Sender<anyhow::Result<()>>,
    },
    MoveTo {
        cursor_id: String,
        x: i32,
        y: i32,
        action: Option<Arc<Action>>,
        reply: Sender<anyhow::Result<()>>,
    },
    Release {
        cursor_id: String,
        button: u8,
        reply: Sender<anyhow::Result<()>>,
    },
    /// Release every button a cursor still holds, whether or not the tool
    /// recorded its hold. Replies whether the cursor held any.
    ReleaseUnrecorded {
        cursor_id: String,
        reply: Sender<anyhow::Result<bool>>,
    },
    /// Drop the entry for a cursor_id without sending wire events — used to
    /// recover when the compositor disconnected mid-life.
    Forget {
        cursor_id: String,
        reply: Sender<anyhow::Result<()>>,
    },
}

/// What the owner thread does with each command. [`Pointers`] drives the
/// compositor's virtual pointers.
trait Owner {
    fn press(
        &mut self,
        cursor_id: &str,
        window_id: u64,
        at: AimedPoint,
        button: u8,
    ) -> anyhow::Result<()>;
    fn move_to(&mut self, cursor_id: &str, x: i32, y: i32) -> anyhow::Result<()>;
    fn release(&mut self, cursor_id: &str, button: u8) -> anyhow::Result<()>;
    fn release_unrecorded(&mut self, cursor_id: &str) -> anyhow::Result<bool>;
    fn forget(&mut self, cursor_id: &str);
    /// Whether `cursor_id` still holds a button.
    fn holds(&self, cursor_id: &str) -> bool;
}

/// The held cursors, by cursor_id.
#[derive(Default)]
struct Pointers(HashMap<String, ActivePointer>);

impl Owner for Pointers {
    fn press(
        &mut self,
        cursor_id: &str,
        window_id: u64,
        at: AimedPoint,
        button: u8,
    ) -> anyhow::Result<()> {
        // A press that went down but was never recorded (its release failed)
        // stays here until released; replacing it would leave it down for good.
        if self.0.contains_key(cursor_id) {
            anyhow::bail!(
            "cursor '{cursor_id}' still holds a button from an earlier call whose release niri did not confirm; call mouse_button_up first"
        );
        }
        // Open a fresh session for this press — this binds the seat, the foreign-
        // toplevel manager, activates the target window, and creates a new vptr.
        // Keep the (out_w, out_h) but drop the queue + state at end of scope; the
        // vptr itself remains alive (Wayland objects survive their original queue
        // as long as the Connection is alive).
        let mut sess = open_vptr_session(Some(window_id))?;
        let (w, h) = (sess.output_w, sess.output_h);
        let (px, py) = sess.abs(at.point.0, at.point.1);
        super::niri_pointer_gate(Some(window_id), None, &sess, &[at])?;
        let btn = evdev_pointer_button(button);
        sess.motion(px, py)?;
        // A press whose round trip fails is released when `sess` drops.
        sess.press(btn)?;

        // The vptr proxy stays valid while its connection lives: keep both for
        // the cursor's later commands.
        let (origin_x, origin_y) = (sess.origin_x, sess.origin_y);
        let (conn, vptr) = sess.keep();
        persist_conn(cursor_id, conn);

        self.0.insert(
            cursor_id.to_string(),
            ActivePointer {
                vptr,
                held: HashSet::from([btn]),
                out_w: w,
                out_h: h,
                origin_x,
                origin_y,
            },
        );
        Ok(())
    }

    fn move_to(&mut self, cursor_id: &str, x: i32, y: i32) -> anyhow::Result<()> {
        move_held(
            &mut self.0,
            cursor_id,
            (x, y),
            super::niri_pointer_space_now,
            roundtrip_on_persistent,
        )
    }

    fn release(&mut self, cursor_id: &str, button: u8) -> anyhow::Result<()> {
        release_held(
            &mut self.0,
            cursor_id,
            evdev_pointer_button(button),
            roundtrip_on_persistent,
            forget_conn,
        )
    }

    fn release_unrecorded(&mut self, cursor_id: &str) -> anyhow::Result<bool> {
        release_all_held(&mut self.0, cursor_id, roundtrip_on_persistent, forget_conn)
    }

    fn forget(&mut self, cursor_id: &str) {
        if let Some(p) = self.0.remove(cursor_id) {
            p.vptr.destroy();
        }
    }

    fn holds(&self, cursor_id: &str) -> bool {
        self.0.contains_key(cursor_id)
    }
}

/// The device a held cursor drives: the virtual pointer, or a recording
/// fake in tests. Each call sends one event and its frame.
trait HeldDevice {
    fn motion(&self, x: u32, y: u32, extent: (u32, u32));
    fn button(&self, button: u32, state: ButtonState);
    fn destroy(&self);
}

impl HeldDevice for ZwlrVirtualPointerV1 {
    fn motion(&self, x: u32, y: u32, (w, h): (u32, u32)) {
        self.motion_absolute(0, x, y, w, h);
        self.frame();
    }

    fn button(&self, button: u32, state: ButtonState) {
        ZwlrVirtualPointerV1::button(self, 0, button, state);
        self.frame();
    }

    fn destroy(&self) {
        ZwlrVirtualPointerV1::destroy(self);
    }
}

/// A held cursor's motion refused because the pointer space it scales by
/// changed, or could not be read. The owner sent the release of its buttons
/// where the pointer was, so the app may have seen a drop, and discarded the
/// cursor: it holds nothing any more, whether or not niri confirmed the
/// release.
#[derive(Debug)]
pub struct HeldPointerEnded {
    detail: String,
    /// niri answered the round trip after the release.
    pub release_confirmed: bool,
}

impl std::fmt::Display for HeldPointerEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for HeldPointerEnded {}

/// State held inside the owner thread for one cursor_id.
struct ActivePointer<D = ZwlrVirtualPointerV1> {
    vptr: D,
    /// evdev codes of buttons currently held down. When this set becomes
    /// empty the vptr is destroyed and the entry dropped from the map.
    held: HashSet<u32>,
    /// Pointer-space extent and layout origin at session open time — needed
    /// for motion_absolute.
    out_w: u32,
    out_h: u32,
    origin_x: i32,
    origin_y: i32,
}

/// Process-global command channel into the owner thread. Lazily started on
/// first use.
static TX: OnceLock<Sender<Cmd>> = OnceLock::new();

fn tx() -> &'static Sender<Cmd> {
    TX.get_or_init(|| {
        let (tx, rx) = bounded::<Cmd>(32);
        let held = held_cursors().clone();
        thread::Builder::new()
            .name("cua-persistent-vptr".into())
            .spawn(move || owner_thread(rx, Pointers::default(), held))
            .expect("spawn cua-persistent-vptr thread");
        tx
    })
}

fn held_cursors() -> &'static Arc<HeldCursors> {
    static HELD: OnceLock<Arc<HeldCursors>> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

fn owner_thread(rx: Receiver<Cmd>, mut owner: impl Owner, held: Arc<HeldCursors>) {
    while let Ok(cmd) = rx.recv() {
        // Each command publishes whether its cursor still holds a button
        // before it replies, so a caller that got its reply sees it.
        match cmd {
            Cmd::Press {
                cursor_id,
                window_id,
                at,
                button,
                action,
                reply,
            } => {
                let _bound = action.map(crate::input::action::bind);
                let pressed = owner.press(&cursor_id, window_id, at, button);
                held.publish(&cursor_id, &owner, true);
                let _ = reply.send(pressed);
            }
            Cmd::MoveTo {
                cursor_id,
                x,
                y,
                action,
                reply,
            } => {
                let _bound = action.map(crate::input::action::bind);
                let moved = owner.move_to(&cursor_id, x, y);
                held.publish(&cursor_id, &owner, false);
                let _ = reply.send(moved);
            }
            // Releases are cleanup: they go out also once the call is over.
            Cmd::Release {
                cursor_id,
                button,
                reply,
            } => {
                let released = owner.release(&cursor_id, button);
                held.publish(&cursor_id, &owner, false);
                let _ = reply.send(released);
            }
            Cmd::ReleaseUnrecorded { cursor_id, reply } => {
                let released = owner.release_unrecorded(&cursor_id);
                held.publish(&cursor_id, &owner, false);
                let _ = reply.send(released);
            }
            Cmd::Forget { cursor_id, reply } => {
                owner.forget(&cursor_id);
                held.publish(&cursor_id, &owner, false);
                let _ = reply.send(Ok(()));
            }
        }
    }
}

/// The cursors that may hold a button, readable without waiting on the
/// owner thread: those it holds one for, and those with a press queued or
/// under way, which may send input at any moment.
#[derive(Default)]
struct HeldCursors(std::sync::Mutex<Cursors>);

#[derive(Default)]
struct Cursors {
    held: HashSet<String>,
    /// Presses queued and not yet handled, by cursor.
    pressing: HashMap<String, usize>,
}

impl Cursors {
    fn press_dropped(&mut self, cursor_id: &str) {
        if let Some(count) = self.pressing.get_mut(cursor_id) {
            *count -= 1;
            if *count == 0 {
                self.pressing.remove(cursor_id);
            }
        }
    }
}

impl HeldCursors {
    fn lock(&self) -> std::sync::MutexGuard<'_, Cursors> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A press for `cursor_id` is about to be queued.
    fn press_queued(&self, cursor_id: &str) {
        *self
            .lock()
            .pressing
            .entry(cursor_id.to_owned())
            .or_default() += 1;
    }

    /// The owner handled a command for `cursor_id`, a press when `pressed`.
    fn publish(&self, cursor_id: &str, owner: &dyn Owner, pressed: bool) {
        let holds = owner.holds(cursor_id);
        let mut cursors = self.lock();
        if pressed {
            cursors.press_dropped(cursor_id);
        }
        if holds {
            cursors.held.insert(cursor_id.to_owned());
        } else {
            cursors.held.remove(cursor_id);
        }
    }

    /// A queued press for `cursor_id` is no longer pending.
    fn press_dropped(&self, cursor_id: &str) {
        self.lock().press_dropped(cursor_id);
    }

    fn contains(&self, cursor_id: &str) -> bool {
        let cursors = self.lock();
        cursors.held.contains(cursor_id) || cursors.pressing.contains_key(cursor_id)
    }
}

/// Move a held cursor. On niri the pointer space is read again first: the
/// motion is scaled by the extent saved at the press, and niri sends no
/// event when an output is added or rescaled. If it changed, no motion goes
/// out; the held buttons are released and the cursor is dropped.
fn move_held<D: HeldDevice>(
    active: &mut HashMap<String, ActivePointer<D>>,
    cursor_id: &str,
    (x, y): (i32, i32),
    space_now: impl FnOnce() -> anyhow::Result<Option<(i32, i32, u32, u32)>>,
    mut roundtrip: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let entry = active.get_mut(cursor_id).ok_or_else(|| {
        anyhow::anyhow!(
            "no held mouse button for cursor '{cursor_id}'; call mouse_button_down first"
        )
    })?;
    let saved = (entry.origin_x, entry.origin_y, entry.out_w, entry.out_h);
    if let Some(refusal) = space_now()
        .and_then(|now| now.map_or(Ok(()), |now| super::pointer_space_holds(saved, now)))
        .err()
    {
        let entry = active.remove(cursor_id).expect("found above");
        for button in &entry.held {
            entry.vptr.button(*button, ButtonState::Released);
        }
        entry.vptr.destroy();
        let released = roundtrip(cursor_id);
        forget_conn(cursor_id);
        let refusal = refusal
            .to_string()
            .trim_start_matches("foreground_unavailable: ")
            .to_owned();
        return Err(match released {
            Ok(()) => HeldPointerEnded {
                detail: format!(
                    "{refusal}; no motion was sent, and the held button was released where the pointer was, which the app may take as a drop"
                ),
                release_confirmed: true,
            },
            Err(error) => HeldPointerEnded {
                detail: format!(
                    "{refusal}; no motion was sent, and the held button's release was sent but not confirmed ({error:#}); the cursor no longer holds it"
                ),
                release_confirmed: false,
            },
        }
        .into());
    }
    let (px, py) = super::pointer_abs(
        entry.origin_x,
        entry.origin_y,
        entry.out_w,
        entry.out_h,
        x,
        y,
    );
    crate::input::action::admit(|| entry.vptr.motion(px, py, (entry.out_w, entry.out_h)))?;
    roundtrip(cursor_id)
}

/// Release `btn` on a held cursor, dropping the cursor once it holds nothing.
/// If niri only did not answer in time, the cursor stays for another try.
/// If the connection failed otherwise, the compositor has already dropped
/// the virtual pointer and its buttons, so the cursor is dropped too.
fn release_held<D: HeldDevice>(
    active: &mut HashMap<String, ActivePointer<D>>,
    cursor_id: &str,
    btn: u32,
    mut roundtrip: impl FnMut(&str) -> anyhow::Result<()>,
    forget: impl FnOnce(&str),
) -> anyhow::Result<()> {
    active
        .get(cursor_id)
        .ok_or_else(|| anyhow::anyhow!("no held mouse button for cursor '{cursor_id}'"))?
        .vptr
        .button(btn, ButtonState::Released);
    match roundtrip(cursor_id) {
        Err(error) if answers_later(&error) => return Err(error),
        Err(_) => {
            drop_cursor(active, cursor_id, forget);
            return Ok(());
        }
        Ok(()) => {}
    }
    let entry = active.get_mut(cursor_id).expect("found above");
    entry.held.remove(&btn);
    if entry.held.is_empty() {
        let entry = active.remove(cursor_id).expect("found above");
        entry.vptr.destroy();
        roundtrip(cursor_id).ok();
        forget(cursor_id);
    }
    Ok(())
}

/// Release every button `cursor_id` holds and drop the cursor. If niri only
/// did not answer in time, the cursor stays for another try; a connection
/// that failed otherwise took the virtual pointer and its buttons with it.
fn release_all_held<D: HeldDevice>(
    active: &mut HashMap<String, ActivePointer<D>>,
    cursor_id: &str,
    mut roundtrip: impl FnMut(&str) -> anyhow::Result<()>,
    forget: impl FnOnce(&str),
) -> anyhow::Result<bool> {
    let Some(entry) = active.get(cursor_id) else {
        return Ok(false);
    };
    for button in &entry.held {
        entry.vptr.button(*button, ButtonState::Released);
    }
    match roundtrip(cursor_id) {
        Err(error) if answers_later(&error) => Err(error),
        Err(_) => {
            drop_cursor(active, cursor_id, forget);
            Ok(true)
        }
        Ok(()) => {
            let entry = active.remove(cursor_id).expect("found above");
            entry.vptr.destroy();
            roundtrip(cursor_id).ok();
            forget(cursor_id);
            Ok(true)
        }
    }
}

/// Drop a cursor whose connection is gone, as `forget` does, and its
/// connection with it.
fn drop_cursor<D: HeldDevice>(
    active: &mut HashMap<String, ActivePointer<D>>,
    cursor_id: &str,
    forget: impl FnOnce(&str),
) {
    if let Some(entry) = active.remove(cursor_id) {
        entry.vptr.destroy();
    }
    forget(cursor_id);
}

/// A held cursor's failed round trip. `timed_out` when niri only did not
/// answer in time, so the connection may still work.
#[derive(Debug)]
struct RoundTripFailed {
    detail: String,
    timed_out: bool,
}

impl std::fmt::Display for RoundTripFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for RoundTripFailed {}

fn round_trip_failed(error: anyhow::Error) -> anyhow::Error {
    RoundTripFailed {
        timed_out: error.is::<super::RoundTripTimedOut>(),
        detail: format!("compositor roundtrip failed: {error:#}"),
    }
    .into()
}

/// Whether a failed round trip left a connection that may still answer.
fn answers_later(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<RoundTripFailed>()
        .is_some_and(|failed| failed.timed_out)
}

/// A held cursor's connection and its queue, kept thread-local because the
/// queue is `!Send` and only the owner thread touches it.
type Persisted = (Connection, wayland_client::EventQueue<super::State>);

thread_local! {
    static CONNS: std::cell::RefCell<HashMap<String, Persisted>>
        = std::cell::RefCell::new(HashMap::new());
}

fn persist_conn(cursor_id: &str, conn: Connection) {
    let queue = conn.new_event_queue::<super::State>();
    CONNS.with(|c| {
        c.borrow_mut().insert(cursor_id.to_string(), (conn, queue));
    });
}

fn forget_conn(cursor_id: &str) {
    CONNS.with(|c| {
        c.borrow_mut().remove(cursor_id);
    });
}

fn roundtrip_on_persistent(cursor_id: &str) -> anyhow::Result<()> {
    CONNS.with(|c| {
        let mut b = c.borrow_mut();
        let (conn, queue) = b
            .get_mut(cursor_id)
            .ok_or_else(|| anyhow::anyhow!("no persistent connection for cursor '{cursor_id}'"))?;
        let mut tmp = super::State::default();
        let deadline = std::time::Instant::now() + super::NIRI_ROUNDTRIP_TIMEOUT;
        super::sync_by(conn, queue, &mut tmp, Some(deadline)).map_err(round_trip_failed)
    })
}

// ── public API ────────────────────────────────────────────────────────────

/// Send one command to the owner thread on `tx` and wait for its reply, until
/// the deadline of the action bound to this thread, or for
/// [`SESSION_END_WAIT`] without one, queueing included.
fn request<T>(
    tx: &Sender<Cmd>,
    cmd: impl FnOnce(Sender<anyhow::Result<T>>) -> Cmd,
) -> anyhow::Result<T> {
    let deadline = crate::input::action::current().map_or_else(
        || std::time::Instant::now() + SESSION_END_WAIT,
        |action| action.deadline(),
    );
    request_by(tx, deadline, cmd)?
}

fn request_by<T>(
    tx: &Sender<Cmd>,
    deadline: std::time::Instant,
    cmd: impl FnOnce(Sender<anyhow::Result<T>>) -> Cmd,
) -> anyhow::Result<anyhow::Result<T>> {
    let (tx_r, rx_r) = bounded(1);
    tx.send_deadline(cmd(tx_r), deadline)
        .map_err(|error| match error {
            crossbeam_channel::SendTimeoutError::Timeout(_) => {
                anyhow::anyhow!("cua-persistent-vptr owner is busy: its queue stayed full")
            }
            crossbeam_channel::SendTimeoutError::Disconnected(_) => {
                anyhow::anyhow!("cua-persistent-vptr thread is dead")
            }
        })?;
    rx_r.recv_deadline(deadline)
        .map_err(|e| anyhow::Error::new(e).context("cua-persistent-vptr owner did not answer"))
}

/// Press and HOLD `button` (evdev code) at output coordinates `(x, y)` on the
/// toplevel identified by `window_id`. Subsequent `move_to` / `release` calls
/// targeting the same `cursor_id` reuse the same virtual-pointer device, so
/// the compositor treats the sequence as one logical drag rather than as
/// independent clicks. Errors if `cursor_id` already has a held button.
///
/// The press serves the tool call bound to this thread: a call that has
/// ended presses nothing. Once the button is down, `record` notes the hold
/// for the caller, under that call; if the call ended meanwhile, or `record`
/// panics, the button is released instead, so no press stays down that
/// nobody recorded. If that release fails, the hold is recorded anyway, so
/// `mouse_button_up` can release it; when `record` itself panicked, the
/// cursor stays with the owner for [`release_unrecorded`].
pub fn press(
    cursor_id: &str,
    window_id: u64,
    at: AimedPoint,
    button: u8,
    record: impl FnOnce(),
) -> anyhow::Result<()> {
    press_on(
        tx(),
        held_cursors(),
        cursor_id,
        window_id,
        at,
        button,
        record,
    )
}

fn press_on(
    tx: &Sender<Cmd>,
    held: &HeldCursors,
    cursor_id: &str,
    window_id: u64,
    at: AimedPoint,
    button: u8,
    record: impl FnOnce(),
) -> anyhow::Result<()> {
    // Session cleanup must see this press before it can send input.
    held.press_queued(cursor_id);
    request(tx, |reply| Cmd::Press {
        cursor_id: cursor_id.to_string(),
        window_id,
        at,
        button,
        action: crate::input::action::current(),
        reply,
    })
    .inspect_err(|error| {
        if !error
            .downcast_ref::<crossbeam_channel::RecvTimeoutError>()
            .is_some_and(crossbeam_channel::RecvTimeoutError::is_timeout)
        {
            held.press_dropped(cursor_id);
        }
    })?;
    // The button is down. Until the hold is recorded, release it on every
    // way out, a panicking `record` included.
    let pressed = UnrecordedPress {
        tx,
        cursor_id,
        button,
    };
    let mut record = Some(record);
    // A call that ended while the button went down cannot record the hold:
    // its caller stopped waiting. Release the button instead.
    let admitted = crate::input::action::admit(|| record.take().expect("recorded once")());
    pressed.disarm();
    let Err(ended) = admitted else {
        return Ok(());
    };
    match release_on(tx, cursor_id, button) {
        Ok(()) => Err(anyhow::anyhow!(
            "{ended}; the button went down as the call ended, and it was released"
        )),
        Err(error) => {
            // The button may still be down, and the owner keeps it: record
            // the hold so that mouse_button_up can release it.
            if let Some(record) = record.take() {
                record();
            }
            Err(anyhow::anyhow!(
                "{ended}; the button went down as the call ended, and its release failed: {error:#}; the hold is kept, so mouse_button_up can release it"
            ))
        }
    }
}

/// A confirmed press whose hold nobody recorded yet: released when dropped.
struct UnrecordedPress<'a> {
    tx: &'a Sender<Cmd>,
    cursor_id: &'a str,
    button: u8,
}

impl UnrecordedPress<'_> {
    /// The hold was recorded, or the caller handles the release itself.
    fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for UnrecordedPress<'_> {
    fn drop(&mut self) {
        // If niri does not confirm this release, the owner keeps the cursor:
        // mouse_button_up and session cleanup release it through
        // [`release_unrecorded`], and a new press on it is refused.
        let _ = release_on(self.tx, self.cursor_id, self.button);
    }
}

/// Emit motion_absolute on the held cursor's virtual-pointer, for the tool
/// call bound to this thread. Errors if there is no held button for
/// `cursor_id`.
pub fn move_to(cursor_id: &str, x: i32, y: i32) -> anyhow::Result<()> {
    move_on(tx(), cursor_id, x, y)
}

fn move_on(tx: &Sender<Cmd>, cursor_id: &str, x: i32, y: i32) -> anyhow::Result<()> {
    request(tx, |reply| Cmd::MoveTo {
        cursor_id: cursor_id.to_string(),
        x,
        y,
        action: crate::input::action::current(),
        reply,
    })
}

/// Release `button` on the held cursor. If no other buttons remain held the
/// virtual-pointer is destroyed and its Wayland connection torn down.
pub fn release(cursor_id: &str, button: u8) -> anyhow::Result<()> {
    release_on(tx(), cursor_id, button)
}

fn release_on(tx: &Sender<Cmd>, cursor_id: &str, button: u8) -> anyhow::Result<()> {
    request(tx, |reply| Cmd::Release {
        cursor_id: cursor_id.to_string(),
        button,
        reply,
    })
}

/// Release whatever `cursor_id` still holds although the tool has no record
/// of it: a press whose recording failed and whose release niri did not
/// confirm. Returns whether the cursor held anything.
pub fn release_unrecorded(cursor_id: &str) -> anyhow::Result<bool> {
    release_unrecorded_on(tx(), cursor_id)
}

fn release_unrecorded_on(tx: &Sender<Cmd>, cursor_id: &str) -> anyhow::Result<bool> {
    request(tx, |reply| Cmd::ReleaseUnrecorded {
        cursor_id: cursor_id.to_string(),
        reply,
    })
}

/// How long session cleanup waits for the owner thread to take and answer
/// its release: two bounded round trips (the release and the device's
/// destroy). Another cursor's command can keep the thread longer; the hook
/// then fails and is retried.
const SESSION_END_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// [`release_unrecorded`] for session cleanup. A cursor the owner holds
/// nothing for and has no press queued for, as for most sessions, is
/// answered without waiting on the owner thread; otherwise the wait is
/// bounded by [`SESSION_END_WAIT`].
pub fn release_unrecorded_at_session_end(cursor_id: &str) -> anyhow::Result<bool> {
    release_unrecorded_within(held_cursors(), tx, cursor_id, SESSION_END_WAIT)
}

fn release_unrecorded_within<'a>(
    held: &HeldCursors,
    tx: impl FnOnce() -> &'a Sender<Cmd>,
    cursor_id: &str,
    wait: std::time::Duration,
) -> anyhow::Result<bool> {
    if !held.contains(cursor_id) {
        return Ok(false);
    }
    request_within(tx(), cursor_id, wait, |reply| Cmd::ReleaseUnrecorded {
        cursor_id: cursor_id.to_string(),
        reply,
    })
}

/// [`release`] for session cleanup, bounded by [`SESSION_END_WAIT`]. On
/// expiry the caller keeps its hold record and retries. A cursor the owner
/// no longer holds and has no press queued for is already released: a
/// release that timed out ran late.
pub fn release_at_session_end(cursor_id: &str, button: u8) -> anyhow::Result<()> {
    release_within(held_cursors(), tx, cursor_id, button, SESSION_END_WAIT)
}

fn release_within<'a>(
    held: &HeldCursors,
    tx: impl FnOnce() -> &'a Sender<Cmd>,
    cursor_id: &str,
    button: u8,
    wait: std::time::Duration,
) -> anyhow::Result<()> {
    if !held.contains(cursor_id) {
        return Ok(());
    }
    request_within(tx(), cursor_id, wait, |reply| Cmd::Release {
        cursor_id: cursor_id.to_string(),
        button,
        reply,
    })
}

/// [`request`] that gives up after `wait`, queueing included. The command
/// may still run once the owner thread is free.
fn request_within<T>(
    tx: &Sender<Cmd>,
    cursor_id: &str,
    wait: std::time::Duration,
    cmd: impl FnOnce(Sender<anyhow::Result<T>>) -> Cmd,
) -> anyhow::Result<T> {
    request_by(tx, std::time::Instant::now() + wait, cmd).map_err(|error| {
        error.context(format!(
            "the persistent pointer did not release cursor '{cursor_id}' within {wait:?}: another pointer command or the compositor is holding it up; retry"
        ))
    })?
}

/// Drop the entry for `cursor_id` without emitting any Wayland events.
/// Useful for recovery — if the agent thinks a button is held but the
/// compositor disagrees, this clears the local state without trying to
/// send a release that would error.
pub fn forget(cursor_id: &str) -> anyhow::Result<()> {
    request(tx(), |reply| Cmd::Forget {
        cursor_id: cursor_id.to_string(),
        reply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, PartialEq)]
    enum Event {
        Motion(u32, u32, (u32, u32)),
        Release(u32),
        Destroy,
    }

    #[derive(Default)]
    struct Recorder(RefCell<Vec<Event>>);

    impl HeldDevice for &Recorder {
        fn motion(&self, x: u32, y: u32, extent: (u32, u32)) {
            self.0.borrow_mut().push(Event::Motion(x, y, extent));
        }

        fn button(&self, button: u32, state: ButtonState) {
            assert_eq!(state, ButtonState::Released);
            self.0.borrow_mut().push(Event::Release(button));
        }

        fn destroy(&self) {
            self.0.borrow_mut().push(Event::Destroy);
        }
    }

    fn held(device: &Recorder) -> HashMap<String, ActivePointer<&Recorder>> {
        HashMap::from([(
            "drag".to_owned(),
            ActivePointer {
                vptr: device,
                held: HashSet::from([0x110]),
                out_w: 1280,
                out_h: 720,
                origin_x: 0,
                origin_y: 0,
            },
        )])
    }

    /// An owner that records what reaches the compositor, sending input as
    /// the virtual pointer does: through the action bound to its thread.
    struct Wire {
        sent: Arc<std::sync::Mutex<Vec<&'static str>>>,
        /// The caller stops waiting while the press is under way.
        caller_leaves_during_press: bool,
        /// niri does not confirm a release.
        release_fails: Arc<std::sync::atomic::AtomicBool>,
        /// Pressed and not yet released, as the owner keeps it.
        held: bool,
    }

    impl Owner for Wire {
        fn press(&mut self, _: &str, _: u64, _: AimedPoint, _: u8) -> anyhow::Result<()> {
            crate::input::action::admit(|| self.sent.lock().unwrap().push("press"))?;
            self.held = true;
            if self.caller_leaves_during_press {
                crate::input::action::current().unwrap().abandon();
            }
            Ok(())
        }

        fn move_to(&mut self, _: &str, _: i32, _: i32) -> anyhow::Result<()> {
            crate::input::action::admit(|| self.sent.lock().unwrap().push("motion"))
        }

        fn release(&mut self, cursor_id: &str, _: u8) -> anyhow::Result<()> {
            if !self.held {
                anyhow::bail!("no held mouse button for cursor '{cursor_id}'");
            }
            self.sent.lock().unwrap().push("release");
            if self.release_fails.load(std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("niri did not answer a Wayland round trip in time");
            }
            self.held = false;
            Ok(())
        }

        fn release_unrecorded(&mut self, cursor_id: &str) -> anyhow::Result<bool> {
            if !self.held {
                return Ok(false);
            }
            self.release(cursor_id, 1).map(|()| true)
        }

        fn forget(&mut self, _: &str) {}

        fn holds(&self, _: &str) -> bool {
            self.held
        }
    }

    /// What reached the compositor, in order.
    type Sent = Arc<std::sync::Mutex<Vec<&'static str>>>;

    /// The owner thread's channel, with `wire` as its owner.
    fn owner(
        caller_leaves_during_press: bool,
        release_fails: bool,
    ) -> (Sender<Cmd>, Arc<std::sync::Mutex<Vec<&'static str>>>) {
        owner_with(
            caller_leaves_during_press,
            Arc::new(std::sync::atomic::AtomicBool::new(release_fails)),
        )
    }

    fn owner_with(
        caller_leaves_during_press: bool,
        release_fails: Arc<std::sync::atomic::AtomicBool>,
    ) -> (Sender<Cmd>, Arc<std::sync::Mutex<Vec<&'static str>>>) {
        let (tx, sent, _) = owner_publishing(caller_leaves_during_press, release_fails);
        (tx, sent)
    }

    fn owner_publishing(
        caller_leaves_during_press: bool,
        release_fails: Arc<std::sync::atomic::AtomicBool>,
    ) -> (Sender<Cmd>, Sent, Arc<HeldCursors>) {
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = bounded(32);
        let wire = Wire {
            sent: sent.clone(),
            caller_leaves_during_press,
            release_fails,
            held: false,
        };
        let held = Arc::new(HeldCursors::default());
        thread::spawn({
            let held = held.clone();
            move || owner_thread(rx, wire, held)
        });
        (tx, sent, held)
    }

    fn timed_out(error: &anyhow::Error) -> bool {
        crate::input::foreground::error_code(error) == Some(crate::input::foreground::CODE_TIMEOUT)
    }

    #[test]
    fn a_held_pointer_sends_nothing_for_a_call_that_has_ended() {
        // mouse_button_down and mouse_drag hand their input to the owner
        // thread. A call that timed out or whose caller went away must not
        // press or move there afterwards.
        let (tx, sent) = owner(false, false);
        let action = Action::new(std::time::Duration::from_secs(60));
        action.abandon();
        let _bound = crate::input::action::bind(action);
        let mut recorded = false;
        let pressed = press_on(
            &tx,
            &HeldCursors::default(),
            "drag",
            1,
            (10, 10).into(),
            1,
            || recorded = true,
        )
        .unwrap_err();
        assert!(timed_out(&pressed), "{pressed:#}");
        let moved = move_on(&tx, "drag", 20, 20).unwrap_err();
        assert!(timed_out(&moved), "{moved:#}");
        assert!(sent.lock().unwrap().is_empty());
        assert!(!recorded);
    }

    #[test]
    fn a_press_whose_caller_left_meanwhile_is_released_not_left_held() {
        // The caller stopped waiting while the button went down, so it will
        // never record the hold: the press must not stay down.
        let (tx, sent) = owner(true, false);
        let _bound = crate::input::action::bind(Action::new(std::time::Duration::from_secs(60)));
        let mut recorded = false;
        let pressed = press_on(
            &tx,
            &HeldCursors::default(),
            "drag",
            1,
            (10, 10).into(),
            1,
            || recorded = true,
        )
        .unwrap_err();
        assert!(timed_out(&pressed), "{pressed:#}");
        assert_eq!(*sent.lock().unwrap(), ["press", "release"]);
        assert!(!recorded);

        // A caller still waiting records the hold, and the button stays down.
        let (tx, sent) = owner(false, false);
        let _bound = crate::input::action::bind(Action::new(std::time::Duration::from_secs(60)));
        let mut recorded = false;
        press_on(
            &tx,
            &HeldCursors::default(),
            "drag",
            1,
            (10, 10).into(),
            1,
            || recorded = true,
        )
        .unwrap();
        assert_eq!(*sent.lock().unwrap(), ["press"]);
        assert!(recorded);
    }

    #[test]
    fn a_press_whose_recording_panics_is_released() {
        let (tx, sent) = owner(false, false);
        let _bound = crate::input::action::bind(Action::new(std::time::Duration::from_secs(60)));
        let pressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            press_on(
                &tx,
                &HeldCursors::default(),
                "drag",
                1,
                (10, 10).into(),
                1,
                || panic!("recording failed"),
            )
        }));
        assert!(pressed.is_err());
        assert_eq!(*sent.lock().unwrap(), ["press", "release"]);
    }

    #[test]
    fn a_press_whose_recording_panics_and_whose_release_fails_stays_releasable() {
        let release_fails = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (tx, sent) = owner_with(false, release_fails.clone());
        let _bound = crate::input::action::bind(Action::new(std::time::Duration::from_secs(60)));
        let pressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            press_on(
                &tx,
                &HeldCursors::default(),
                "drag",
                1,
                (10, 10).into(),
                1,
                || panic!("recording failed"),
            )
        }));
        assert!(pressed.is_err());
        assert_eq!(*sent.lock().unwrap(), ["press", "release"]);
        // niri answers again: mouse_button_up or session cleanup finds the
        // press that nobody recorded and releases it.
        release_fails.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(release_unrecorded_on(&tx, "drag").unwrap());
        assert_eq!(*sent.lock().unwrap(), ["press", "release", "release"]);
        assert!(!release_unrecorded_on(&tx, "drag").unwrap());
    }

    #[test]
    fn a_press_whose_release_fails_after_its_call_ended_keeps_its_hold() {
        // The button may still be down: mouse_button_up needs the hold to
        // release it.
        let (tx, sent) = owner(true, true);
        let _bound = crate::input::action::bind(Action::new(std::time::Duration::from_secs(60)));
        let mut recorded = false;
        let pressed = press_on(
            &tx,
            &HeldCursors::default(),
            "drag",
            1,
            (10, 10).into(),
            1,
            || recorded = true,
        )
        .unwrap_err();
        assert!(timed_out(&pressed), "{pressed:#}");
        assert!(
            pressed.to_string().contains("the hold is kept"),
            "{pressed:#}"
        );
        assert_eq!(*sent.lock().unwrap(), ["press", "release"]);
        assert!(recorded);
    }

    #[test]
    fn session_end_does_not_wait_on_the_owner_for_a_session_without_a_pointer() {
        // Another session's command is stuck in a Sway round trip.
        let held = HeldCursors::default();
        held.lock().held.insert("other-session".to_owned());
        let released = release_unrecorded_within(
            &held,
            || -> &Sender<Cmd> { unreachable!("asked the stalled owner") },
            "empty-session",
            std::time::Duration::from_secs(60),
        );
        assert!(!released.unwrap());
    }

    #[test]
    fn session_end_gives_up_on_a_stalled_owner_within_its_bound() {
        let held = Arc::new(HeldCursors::default());
        held.lock().held.insert("session".to_owned());
        // The owner thread is stuck in another command: nothing reads the
        // channel. With room in the queue the reply never comes; with the
        // queue full the command cannot even be queued.
        for queue_full in [false, true] {
            let (tx, rx) = bounded::<Cmd>(1);
            if queue_full {
                tx.send(Cmd::Forget {
                    cursor_id: "other".to_owned(),
                    reply: bounded(1).0,
                })
                .unwrap();
            }
            let wait = std::time::Duration::from_millis(200);
            let (done, finished) = bounded(2);
            let started = std::time::Instant::now();
            thread::spawn({
                let held = held.clone();
                move || {
                    // The recorded hold's release, then the unrecorded one.
                    let recorded = release_within(&held, || &tx, "session", 1, wait);
                    let _ = done.send(recorded.map_err(|error| error.to_string()));
                    let unrecorded = release_unrecorded_within(&held, || &tx, "session", wait);
                    let _ = done.send(unrecorded.map(drop).map_err(|error| error.to_string()));
                }
            });
            for release in ["recorded", "unrecorded"] {
                let released = finished
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap_or_else(|_| panic!("the {release} release waited on a stalled owner"));
                let refused = released.unwrap_err();
                assert!(refused.ends_with("retry"), "{refused}");
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            drop(rx);
        }
    }

    #[test]
    fn session_end_retry_completes_after_a_late_recorded_release() {
        // Session end's release of a recorded hold timed out; the release
        // ran late and lifted the button. The retry finds nothing held.
        let (tx, sent, held) = owner_publishing(false, Default::default());
        press_on(&tx, &held, "session", 1, (10, 10).into(), 1, || ()).unwrap();
        release_on(&tx, "session", 1).unwrap();
        let wait = std::time::Duration::from_secs(5);
        let retried = release_within(&held, || &tx, "session", 1, wait);
        assert!(retried.is_ok(), "{retried:?}");
        assert_eq!(*sent.lock().unwrap(), ["press", "release"]);
    }

    #[test]
    fn a_press_the_owner_never_handles_stops_counting_as_under_way() {
        let held = HeldCursors::default();
        let (tx, rx) = bounded::<Cmd>(1);
        drop(rx);
        press_on(&tx, &held, "dead", 1, (10, 10).into(), 1, || ()).unwrap_err();
        assert!(!held.contains("dead"));
    }

    #[test]
    fn a_request_to_a_full_queue_gives_up_with_or_without_an_action() {
        let (tx, rx) = bounded::<Cmd>(1);
        let forget = |reply| Cmd::Forget {
            cursor_id: "stuck".into(),
            reply,
        };
        tx.send(forget(bounded(1).0)).unwrap();
        let (done, finished) = bounded(2);
        thread::spawn(move || {
            let _ = done.send(request::<()>(&tx, forget).is_err());
            let action = crate::input::action::Action::new(std::time::Duration::from_millis(100));
            let _bound = crate::input::action::bind(action);
            let _ = done.send(request::<()>(&tx, forget).is_err());
        });
        for _ in 0..2 {
            let refused = finished.recv_timeout(SESSION_END_WAIT * 3);
            assert_eq!(refused, Ok(true));
        }
        drop(rx);
    }

    #[test]
    fn a_request_waits_for_the_owner_only_until_the_action_deadline() {
        let (tx, rx) = bounded::<Cmd>(1);
        let action = crate::input::action::Action::new(std::time::Duration::from_millis(100));
        let _bound = crate::input::action::bind(action);
        let started = std::time::Instant::now();
        request::<()>(&tx, |reply| Cmd::Forget {
            cursor_id: "stuck".into(),
            reply,
        })
        .unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        drop(rx);
    }

    #[test]
    fn cleanup_sees_a_handled_press_pending_while_its_hold_is_published() {
        struct Holding(
            std::sync::mpsc::SyncSender<()>,
            std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        );
        impl Owner for Holding {
            fn press(&mut self, _: &str, _: u64, _: AimedPoint, _: u8) -> anyhow::Result<()> {
                unreachable!()
            }
            fn move_to(&mut self, _: &str, _: i32, _: i32) -> anyhow::Result<()> {
                unreachable!()
            }
            fn release(&mut self, _: &str, _: u8) -> anyhow::Result<()> {
                unreachable!()
            }
            fn release_unrecorded(&mut self, _: &str) -> anyhow::Result<bool> {
                unreachable!()
            }
            fn forget(&mut self, _: &str) {}
            fn holds(&self, _: &str) -> bool {
                self.0.send(()).unwrap();
                self.1.lock().unwrap().recv().unwrap();
                true
            }
        }
        let held = Arc::new(HeldCursors::default());
        held.press_queued("c");
        let (asked, asking) = std::sync::mpsc::sync_channel(0);
        let (answer, answering) = std::sync::mpsc::channel();
        let publisher = {
            let held = held.clone();
            thread::spawn(move || {
                held.publish("c", &Holding(asked, std::sync::Mutex::new(answering)), true)
            })
        };
        asking.recv().unwrap();
        let (seen, seeing) = std::sync::mpsc::channel();
        thread::spawn({
            let held = held.clone();
            move || seen.send(held.contains("c"))
        });
        let visible = seeing.recv_timeout(std::time::Duration::from_secs(2));
        answer.send(()).unwrap();
        assert_eq!(visible, Ok(true));
        publisher.join().unwrap();
        assert!(held.contains("c"));
    }

    #[test]
    fn session_end_waits_for_a_press_still_under_way() {
        // The client cancelled mouse_button_down while its press waited on
        // the owner, then ended the session. The press can still go down.
        let held = Arc::new(HeldCursors::default());
        let (tx, rx) = bounded::<Cmd>(1);
        let pressing = thread::spawn({
            let (tx, held) = (tx.clone(), held.clone());
            move || {
                let _ = press_on(&tx, &held, "session", 1, (10, 10).into(), 1, || ());
            }
        });
        while tx.is_empty() {
            thread::yield_now();
        }
        let wait = std::time::Duration::from_millis(200);
        let released = release_unrecorded_within(&held, || &tx, "session", wait);
        assert!(
            released
                .as_ref()
                .is_err_and(|error| error.to_string().ends_with("retry")),
            "{released:?}"
        );
        // Answer nothing: dropping the queued press ends its wait.
        drop(rx.recv().unwrap());
        pressing.join().unwrap();
    }

    #[test]
    fn session_end_releases_the_unrecorded_press_of_its_own_session() {
        let release_fails = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (tx, sent, held) = owner_publishing(false, release_fails.clone());
        let pressed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            press_on(&tx, &held, "session", 1, (10, 10).into(), 1, || {
                panic!("recording failed")
            })
        }));
        assert!(pressed.is_err());
        release_fails.store(false, std::sync::atomic::Ordering::SeqCst);
        let wait = std::time::Duration::from_secs(5);
        assert!(release_unrecorded_within(&held, || &tx, "session", wait).unwrap());
        assert_eq!(*sent.lock().unwrap(), ["press", "release", "release"]);
        assert!(!held.contains("session"));
    }

    #[test]
    fn a_held_motion_for_a_call_that_has_ended_is_not_sent() {
        let device = Recorder::default();
        let mut active = held(&device);
        let action = Action::new(std::time::Duration::from_secs(60));
        action.abandon();
        let _bound = crate::input::action::bind(action);
        let refused = move_held(
            &mut active,
            "drag",
            (200, 200),
            || Ok(Some((0, 0, 1280, 720))),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(timed_out(&refused), "{refused:#}");
        assert!(device.0.borrow().is_empty());
        // The button stays held for the next call's motion or release.
        assert!(active.contains_key("drag"));
    }

    /// Release `drag`'s button with `release_held`, or with
    /// `release_all_held` when `unrecorded`, through `roundtrip`.
    fn release_drag(
        active: &mut HashMap<String, ActivePointer<&Recorder>>,
        unrecorded: bool,
        roundtrip: impl FnMut(&str) -> anyhow::Result<()>,
        forgotten: &mut bool,
    ) -> anyhow::Result<()> {
        let forget = |_: &str| *forgotten = true;
        if unrecorded {
            release_all_held(active, "drag", roundtrip, forget).map(|held| assert!(held))
        } else {
            release_held(active, "drag", 0x110, roundtrip, forget)
        }
    }

    #[test]
    fn a_release_niri_did_not_answer_in_time_keeps_the_cursor_for_another_try() {
        for unrecorded in [false, true] {
            let device = Recorder::default();
            let mut active = held(&device);
            let mut forgotten = false;
            let refused = release_drag(
                &mut active,
                unrecorded,
                |_| Err(round_trip_failed(crate::wayland::RoundTripTimedOut.into())),
                &mut forgotten,
            )
            .unwrap_err();
            assert_eq!(
                refused.to_string(),
                "compositor roundtrip failed: niri did not answer a Wayland round trip in time"
            );
            assert!(active.contains_key("drag"), "unrecorded: {unrecorded}");
            assert!(!forgotten);
        }
    }

    #[test]
    fn a_release_on_a_dead_connection_drops_the_cursor() {
        // The compositor dropped the client, and with it the virtual pointer
        // and its buttons: keeping the cursor would refuse every new press.
        for unrecorded in [false, true] {
            let device = Recorder::default();
            let mut active = held(&device);
            let mut forgotten = false;
            release_drag(
                &mut active,
                unrecorded,
                |_| {
                    Err(round_trip_failed(anyhow::anyhow!(
                        "niri Wayland flush failed: Broken pipe"
                    )))
                },
                &mut forgotten,
            )
            .unwrap();
            assert!(active.is_empty(), "unrecorded: {unrecorded}");
            assert!(forgotten);
            assert_eq!(*device.0.borrow(), [Event::Release(0x110), Event::Destroy]);
        }
    }

    #[test]
    fn a_held_motion_after_the_pointer_space_changed_releases_instead_of_moving() {
        let device = Recorder::default();
        let mut active = held(&device);
        // Unchanged: the motion goes out in the saved extent.
        move_held(
            &mut active,
            "drag",
            (200, 200),
            || Ok(Some((0, 0, 1280, 720))),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(*device.0.borrow(), [Event::Motion(200, 200, (1280, 720))]);

        // The output went from scale 1 to 1.5 between two motions: scaling
        // by the old extent would land far from the aimed point.
        let stopped = move_held(
            &mut active,
            "drag",
            (267, 267),
            || Ok(Some((0, 0, 854, 480))),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(stopped
            .downcast_ref::<HeldPointerEnded>()
            .is_some_and(|ended| ended.release_confirmed));
        assert_eq!(
            stopped.to_string(),
            "the niri pointer space changed from (0, 0, 1280, 720) to (0, 0, 854, 480); take a new screenshot; no motion was sent, and the held button was released where the pointer was, which the app may take as a drop"
        );
        assert_eq!(
            device.0.borrow()[1..],
            [Event::Release(0x110), Event::Destroy]
        );
        assert!(active.is_empty());
    }

    #[test]
    fn a_held_motion_whose_cleanup_niri_never_answers_still_ends_the_hold() {
        // niri stopped answering: the pointer space cannot be read and the
        // release is not confirmed. The owner discards the cursor anyway, so
        // the tool must learn that the hold ended, not see a plain failure.
        let device = Recorder::default();
        let mut active = held(&device);
        let stopped = move_held(
            &mut active,
            "drag",
            (200, 200),
            || anyhow::bail!("niri did not answer in time"),
            |_| anyhow::bail!("niri did not answer a Wayland round trip in time"),
        )
        .unwrap_err();
        let ended = stopped
            .downcast_ref::<HeldPointerEnded>()
            .expect("the hold ended");
        assert!(!ended.release_confirmed);
        assert_eq!(
            stopped.to_string(),
            "niri did not answer in time; no motion was sent, and the held button's release was sent but not confirmed (niri did not answer a Wayland round trip in time); the cursor no longer holds it"
        );
        assert_eq!(*device.0.borrow(), [Event::Release(0x110), Event::Destroy]);
        assert!(active.is_empty());
    }
}
