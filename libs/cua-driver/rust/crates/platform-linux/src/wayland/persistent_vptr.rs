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
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{bounded, Receiver, Sender};
use wayland_client::{protocol::wl_pointer::ButtonState, Connection};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::{evdev_pointer_button, open_vptr_session};

/// One in-flight command from the public API to the owner thread.
enum Cmd {
    Press {
        cursor_id: String,
        window_id: u64,
        x: i32,
        y: i32,
        button: u8,
        reply: Sender<anyhow::Result<()>>,
    },
    MoveTo {
        cursor_id: String,
        x: i32,
        y: i32,
        reply: Sender<anyhow::Result<()>>,
    },
    Release {
        cursor_id: String,
        button: u8,
        reply: Sender<anyhow::Result<()>>,
    },
    /// Drop the entry for a cursor_id without sending wire events — used to
    /// recover when the compositor disconnected mid-life.
    Forget {
        cursor_id: String,
        reply: Sender<anyhow::Result<()>>,
    },
}

/// State held inside the owner thread for one cursor_id.
struct ActivePointer {
    vptr: ZwlrVirtualPointerV1,
    /// evdev codes of buttons currently held down. When this set becomes
    /// empty the vptr is destroyed and the entry dropped from the map.
    held: HashSet<u32>,
    /// Output extent at session open time — needed for motion_absolute.
    out_w: u32,
    out_h: u32,
}

/// Process-global command channel into the owner thread. Lazily started on
/// first use.
static TX: OnceLock<Sender<Cmd>> = OnceLock::new();

fn tx() -> &'static Sender<Cmd> {
    TX.get_or_init(|| {
        let (tx, rx) = bounded::<Cmd>(32);
        thread::Builder::new()
            .name("cua-persistent-vptr".into())
            .spawn(move || owner_thread(rx))
            .expect("spawn cua-persistent-vptr thread");
        tx
    })
}

fn owner_thread(rx: Receiver<Cmd>) {
    let mut active: HashMap<String, ActivePointer> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Press {
                cursor_id,
                window_id,
                x,
                y,
                button,
                reply,
            } => {
                let r = handle_press(&mut active, &cursor_id, window_id, x, y, button);
                let _ = reply.send(r);
            }
            Cmd::MoveTo {
                cursor_id,
                x,
                y,
                reply,
            } => {
                let r = handle_move(&mut active, &cursor_id, x, y);
                let _ = reply.send(r);
            }
            Cmd::Release {
                cursor_id,
                button,
                reply,
            } => {
                let r = handle_release(&mut active, &cursor_id, button);
                let _ = reply.send(r);
            }
            Cmd::Forget { cursor_id, reply } => {
                if let Some(p) = active.remove(&cursor_id) {
                    p.vptr.destroy();
                }
                let _ = reply.send(Ok(()));
            }
        }
    }
}

fn handle_press(
    active: &mut HashMap<String, ActivePointer>,
    cursor_id: &str,
    window_id: u64,
    x: i32,
    y: i32,
    button: u8,
) -> anyhow::Result<()> {
    // Open a fresh session for this press — this binds the seat, the foreign-
    // toplevel manager, activates the target window, and creates a new vptr.
    // Keep the (out_w, out_h) but drop the queue + state at end of scope; the
    // vptr itself remains alive (Wayland objects survive their original queue
    // as long as the Connection is alive).
    let mut sess = open_vptr_session(Some(window_id as u32))?;
    let (w, h) = (sess.output_w, sess.output_h);
    let px = x.clamp(0, w as i32 - 1) as u32;
    let py = y.clamp(0, h as i32 - 1) as u32;
    let btn = evdev_pointer_button(button);

    sess.vptr.motion_absolute(0, px, py, w, h);
    sess.vptr.frame();
    sess.vptr.button(0, btn, ButtonState::Pressed);
    sess.vptr.frame();
    sess.queue.roundtrip(&mut sess.state)?;

    // Take ownership of the vptr handle by extracting it from the session.
    // ZwlrVirtualPointerV1 is a Wayland proxy — cloning it gives another
    // handle to the same wire object; destroying it sends the destructor.
    let vptr = sess.vptr.clone();
    // Persist the live connection so the proxy stays valid after this fn returns
    // (the session goes out of scope; we need the conn alive).
    // We do this by leaking the connection into a process-static slot keyed by
    // cursor_id. Subsequent commands on the same cursor reuse this conn.
    persist_conn(cursor_id, sess.conn);

    let mut held = HashSet::new();
    held.insert(btn);
    active.insert(
        cursor_id.to_string(),
        ActivePointer {
            vptr,
            held,
            out_w: w,
            out_h: h,
        },
    );
    Ok(())
}

fn handle_move(
    active: &mut HashMap<String, ActivePointer>,
    cursor_id: &str,
    x: i32,
    y: i32,
) -> anyhow::Result<()> {
    let entry = active.get_mut(cursor_id).ok_or_else(|| {
        anyhow::anyhow!(
            "no held mouse button for cursor '{cursor_id}'; call mouse_button_down first"
        )
    })?;
    let px = x.clamp(0, entry.out_w as i32 - 1) as u32;
    let py = y.clamp(0, entry.out_h as i32 - 1) as u32;
    entry
        .vptr
        .motion_absolute(0, px, py, entry.out_w, entry.out_h);
    entry.vptr.frame();
    roundtrip_on_persistent(cursor_id)?;
    Ok(())
}

fn handle_release(
    active: &mut HashMap<String, ActivePointer>,
    cursor_id: &str,
    button: u8,
) -> anyhow::Result<()> {
    let btn = evdev_pointer_button(button);
    let drop_entry = {
        let entry = active
            .get_mut(cursor_id)
            .ok_or_else(|| anyhow::anyhow!("no held mouse button for cursor '{cursor_id}'"))?;
        entry.vptr.button(0, btn, ButtonState::Released);
        entry.vptr.frame();
        roundtrip_on_persistent(cursor_id)?;
        entry.held.remove(&btn);
        entry.held.is_empty()
    };
    if drop_entry {
        if let Some(p) = active.remove(cursor_id) {
            p.vptr.destroy();
            roundtrip_on_persistent(cursor_id).ok();
        }
        forget_conn(cursor_id);
    }
    Ok(())
}

// Process-static slots for Connection + EventQueue keyed by cursor_id. The
// EventQueue is !Send but we only touch these on the owner thread, so wrap
// in a thread-local-by-construction pattern: store inside the same map so
// the owner thread is the sole accessor.
//
// We use a per-thread static rather than a Mutex<HashMap> because the owner
// thread is the only accessor (no contention possible).
thread_local! {
    static CONNS: std::cell::RefCell<HashMap<String, (Connection, wayland_client::EventQueue<super::State>)>>
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
        let (_conn, queue) = b
            .get_mut(cursor_id)
            .ok_or_else(|| anyhow::anyhow!("no persistent connection for cursor '{cursor_id}'"))?;
        let mut tmp = super::State::default();
        queue
            .roundtrip(&mut tmp)
            .map_err(|e| anyhow::anyhow!("compositor roundtrip failed: {e}"))?;
        Ok(())
    })
}

// ── public API ────────────────────────────────────────────────────────────

/// Press and HOLD `button` (evdev code) at output coordinates `(x, y)` on the
/// toplevel identified by `window_id`. Subsequent `move_to` / `release` calls
/// targeting the same `cursor_id` reuse the same virtual-pointer device, so
/// the compositor treats the sequence as one logical drag rather than as
/// independent clicks. Errors if `cursor_id` already has a held button.
pub fn press(cursor_id: &str, window_id: u64, x: i32, y: i32, button: u8) -> anyhow::Result<()> {
    let (tx_r, rx_r) = bounded(1);
    tx().send(Cmd::Press {
        cursor_id: cursor_id.to_string(),
        window_id,
        x,
        y,
        button,
        reply: tx_r,
    })
    .map_err(|e| anyhow::anyhow!("cua-persistent-vptr thread is dead: {e}"))?;
    rx_r.recv()
        .map_err(|e| anyhow::anyhow!("reply channel closed: {e}"))?
}

/// Emit motion_absolute on the held cursor's virtual-pointer. Errors if there
/// is no held button for `cursor_id`.
pub fn move_to(cursor_id: &str, x: i32, y: i32) -> anyhow::Result<()> {
    let (tx_r, rx_r) = bounded(1);
    tx().send(Cmd::MoveTo {
        cursor_id: cursor_id.to_string(),
        x,
        y,
        reply: tx_r,
    })
    .map_err(|e| anyhow::anyhow!("cua-persistent-vptr thread is dead: {e}"))?;
    rx_r.recv()
        .map_err(|e| anyhow::anyhow!("reply channel closed: {e}"))?
}

/// Release `button` on the held cursor. If no other buttons remain held the
/// virtual-pointer is destroyed and its Wayland connection torn down.
pub fn release(cursor_id: &str, button: u8) -> anyhow::Result<()> {
    let (tx_r, rx_r) = bounded(1);
    tx().send(Cmd::Release {
        cursor_id: cursor_id.to_string(),
        button,
        reply: tx_r,
    })
    .map_err(|e| anyhow::anyhow!("cua-persistent-vptr thread is dead: {e}"))?;
    rx_r.recv()
        .map_err(|e| anyhow::anyhow!("reply channel closed: {e}"))?
}

/// Drop the entry for `cursor_id` without emitting any Wayland events.
/// Useful for recovery — if the agent thinks a button is held but the
/// compositor disagrees, this clears the local state without trying to
/// send a release that would error.
pub fn forget(cursor_id: &str) -> anyhow::Result<()> {
    let (tx_r, rx_r) = bounded(1);
    tx().send(Cmd::Forget {
        cursor_id: cursor_id.to_string(),
        reply: tx_r,
    })
    .map_err(|e| anyhow::anyhow!("cua-persistent-vptr thread is dead: {e}"))?;
    rx_r.recv()
        .map_err(|e| anyhow::anyhow!("reply channel closed: {e}"))?
}
