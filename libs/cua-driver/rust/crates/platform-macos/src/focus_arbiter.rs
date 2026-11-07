// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One rule for whether the driver may undo an app's activation.
//!
//! Several watchers put the user's app back in front: the activation
//! observer ([`crate::focus_steal`]), the key-focus guard's revert and order
//! watch ([`crate::key_focus_guard`]), launch_app's late-activation watchdog
//! and the browser attach watcher. Each takes an [`Epoch`] when it is armed
//! and asks [`decide`] before it undoes anything. It must leave the change
//! alone when the driver itself has made an app frontmost since then
//! ([`note_intentional`]), or when the user just pressed a mouse button or
//! a modifier key and may have switched apps themselves.
//!
//! The arbiter also holds the one lease an action leaves armed after its
//! result returns (the window-change detector's detached watch). A driver
//! activation ends it, and so does the next action ([`end_detached`]).
//!
//! Every driver activation (bring_to_front, launch_app, foreground delivery,
//! `apps::activate_pid`, `skylight::make_exact_window_key`) calls
//! [`note_intentional`]. Putting the user's app back (`apps::reactivate_app`,
//! `skylight::return_key_to`) does not, so an undo never cancels the watcher
//! that made it, or another watcher armed for the same action.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::focus_steal::SuppressionLease;

static EPOCH: AtomicU64 = AtomicU64::new(0);

/// The count of driver activations when a watcher was armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch(u64);

impl Epoch {
    pub fn now() -> Self {
        Self(EPOCH.load(Ordering::SeqCst))
    }

    /// No driver activation since this epoch was taken.
    pub fn is_current(self) -> bool {
        self == Self::now()
    }

    #[cfg(test)]
    pub(crate) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Record an activation the driver asked for. Watchers armed before it no
/// longer undo anything, and the detached lease is released.
pub fn note_intentional() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    end_detached();
}

/// The lease an action keeps after its result returned, and its id.
static DETACHED: Mutex<Option<(u64, Option<SuppressionLease>)>> = Mutex::new(None);
static NEXT_DETACHED: AtomicU64 = AtomicU64::new(1);

fn detached() -> std::sync::MutexGuard<'static, Option<(u64, Option<SuppressionLease>)>> {
    DETACHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keep `lease` armed after its action returned, replacing any earlier one.
/// Returns the id [`holds_detached`] and [`release_detached`] take.
pub fn detach(lease: Option<SuppressionLease>) -> u64 {
    let id = NEXT_DETACHED.fetch_add(1, Ordering::Relaxed);
    let previous = detached().replace((id, lease));
    drop(previous);
    id
}

/// Whether the lease detached as `id` is still held.
pub fn holds_detached(id: u64) -> bool {
    detached().as_ref().is_some_and(|(held, _)| *held == id)
}

/// Release the lease detached as `id`, if it is still the one held.
pub fn release_detached(id: u64) {
    let mut slot = detached();
    let released = slot.take_if(|(held, _)| *held == id);
    drop(slot);
    drop(released);
}

/// Release the detached lease. The driver's next action supersedes the
/// previous one's watch.
pub fn end_detached() {
    let released = detached().take();
    drop(released);
}

#[cfg(test)]
pub(crate) fn has_detached() -> bool {
    detached().is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Undo,
    Leave,
}

/// Whether a watcher armed at `armed` may undo an activation now.
pub fn decide(armed: Epoch) -> Decision {
    decide_with(armed, Epoch::now(), user_switched_recently)
}

fn decide_with(armed: Epoch, now: Epoch, user_switched: impl FnOnce() -> bool) -> Decision {
    if armed == now && !user_switched() {
        Decision::Undo
    } else {
        Decision::Leave
    }
}

/// A hardware mouse-down or modifier change within this long before an
/// activation means the user may have caused it.
const USER_SWITCH_WINDOW_S: f64 = 1.0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// Whether the user pressed a mouse button or a modifier key on the real
/// hardware just now. Events the driver posts to a pid do not count.
fn user_switched_recently() -> bool {
    const HID_SYSTEM_STATE: i32 = 1;
    const LEFT_MOUSE_DOWN: u32 = 1;
    const RIGHT_MOUSE_DOWN: u32 = 3;
    const FLAGS_CHANGED: u32 = 12;
    [LEFT_MOUSE_DOWN, RIGHT_MOUSE_DOWN, FLAGS_CHANGED]
        .into_iter()
        .any(|kind| {
            let since = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, kind) };
            since.is_finite() && since < USER_SWITCH_WINDOW_S
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undo_needs_its_own_epoch_and_no_user_input() {
        let armed = Epoch::now();
        assert_eq!(decide_with(armed, armed, || false), Decision::Undo);
        // The driver activated an app since the watcher was armed.
        assert_eq!(decide_with(armed, armed.next(), || false), Decision::Leave);
        // The user clicked or held a modifier just now.
        assert_eq!(decide_with(armed, armed, || true), Decision::Leave);
    }
}
