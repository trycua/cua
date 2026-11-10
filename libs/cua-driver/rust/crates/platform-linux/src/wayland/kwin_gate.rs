// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Target-bound input on KWin.
//!
//! KWin delivers libei input to whatever holds focus when it processes each
//! event, so activating a window and then sending input leaves a window in
//! which focus can move and the input lands elsewhere. The KWin helper's input
//! gate (protocol 2, [`super::kwin_helper::InputGate`]) closes it: while armed,
//! an input filter inside KWin lets this process's libei events reach only the
//! target window or its transients, and holds back any event that would reach
//! another window. KWin decides per event, after resolving focus and before any
//! window sees the event, so no check-then-send gap remains.
//!
//! [`gated`] arms the gate around a body of input, and a KWin window is
//! activated for input only inside it ([`activate_for_input`]), so every KWin
//! input route is bound to its target or refused. While a gate is armed the
//! non-libei routes (wlroots virtual pointer and keyboard, `wtype`) stand down
//! ([`armed`]), since the gate sees only libei devices.

use std::cell::Cell;
use std::time::Duration;

use super::{kwin_capture, kwin_helper};

/// How long KWin may take to move keyboard focus after it reports a window
/// active.
const FOCUS_CONFIRM: Duration = Duration::from_millis(500);

std::thread_local! {
    /// The KWin window this thread's armed gate binds input to.
    static ARMED: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Whether this thread has a KWin input gate armed, so input must go out
/// through libei, the only route the gate sees.
pub(crate) fn armed() -> bool {
    ARMED.with(Cell::get).is_some()
}

/// Whether KWin can bind input to `window_id`: the helper's input gate is
/// loaded and the window is a KWin handle.
pub(crate) fn ready(window_id: u64) -> bool {
    kwin_capture::uuid_for(window_id).is_some() && kwin_helper::available()
}

/// The refusal for a KWin window when the input gate is missing.
pub(crate) fn unavailable(window_id: u64) -> anyhow::Error {
    anyhow::anyhow!(
        "foreground_unavailable: KWin input can be bound to window {window_id} only through \
         the Cua KWin helper's input gate (kwin-target-helper protocol {}), which is not \
         loaded; run `cua-driver kwin-capture on` to load it, or `cua-driver kwin-capture \
         status` to see what is missing; refusing global input because it could affect the \
         wrong application",
        kwin_helper::GATE_PROTOCOL_VERSION
    )
}

/// Run `body` with KWin holding back any of this process's libei input that
/// would reach a window other than `window_id`. Fails when the gate held any
/// event back, even if `body` succeeded, since then not all of its input
/// landed. A gate already armed on this thread for the same window is reused.
pub(crate) fn gated<T>(
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match ARMED.with(Cell::get) {
        Some(armed) if armed == window_id => return body(),
        Some(armed) => anyhow::bail!(
            "foreground_unavailable: KWin input is already bound to window {armed}; \
             refusing input to window {window_id}"
        ),
        None => {}
    }
    let uuid = kwin_capture::uuid_for(window_id).ok_or_else(|| unavailable(window_id))?;
    let owner = kwin_helper::gate_owner().ok_or_else(|| unavailable(window_id))?;
    let client = libei_client()?;
    let gate = kwin_helper::InputGate::arm(&owner, &uuid, &client)
        .map_err(|error| error.context(unavailable(window_id)))?;
    let result = {
        let _armed = Armed::bind(window_id);
        body()
    };
    verdict(window_id, result, gate.disarm())
}

/// Activate `window_id` and confirm KWin moved keyboard focus to it. Only
/// inside [`gated`] for the same window: outside it, focus could move again
/// before input lands, and the input would reach another window.
pub(crate) fn activate_for_input(window_id: u64, pid: Option<u32>) -> anyhow::Result<()> {
    if ARMED.with(Cell::get) != Some(window_id) {
        return Err(unavailable(window_id));
    }
    let uuid = kwin_capture::uuid_for(window_id).ok_or_else(|| unavailable(window_id))?;
    let owner = kwin_helper::gate_owner().ok_or_else(|| unavailable(window_id))?;
    kwin_capture::activate(window_id, pid)?;
    let deadline = crate::interaction::operation_deadline(FOCUS_CONFIRM);
    let focused = crate::interaction::confirm(deadline, |_| {
        kwin_helper::focus(&owner, &uuid).map(|focus| focus.keyboard)
    })?;
    anyhow::ensure!(
        focused,
        "foreground_unavailable: KWin reported window {window_id} active, but did not give it \
         keyboard focus; no input was sent"
    );
    Ok(())
}

/// The handle of KWin's active window, to give focus back to afterwards.
pub(crate) fn active_window() -> anyhow::Result<Option<u64>> {
    let owner = kwin_helper::gate_owner()
        .ok_or_else(|| anyhow::anyhow!("the Cua KWin helper's input gate is not loaded"))?;
    Ok(kwin_helper::active_uuid(&owner)?.and_then(|uuid| kwin_capture::known_id_for(&uuid)))
}

/// The gate counts only devices KWin created for this process's libei client,
/// so the client must be connected before the gate is armed.
#[cfg(feature = "portal-input")]
fn libei_client() -> anyhow::Result<String> {
    super::libei::wait_keyboard_ready()?;
    Ok(super::libei::client_name())
}

#[cfg(not(feature = "portal-input"))]
fn libei_client() -> anyhow::Result<String> {
    anyhow::bail!(
        "foreground_unavailable: this build has no libei input, the only input KWin can bind \
         to one window"
    )
}

/// Marks this thread's gate armed for one window until dropped.
struct Armed;

impl Armed {
    fn bind(window_id: u64) -> Self {
        ARMED.with(|armed| armed.set(Some(window_id)));
        Self
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        ARMED.with(|armed| armed.set(None));
    }
}

/// The outcome of a gated body: its own result, unless the gate held input
/// back or could not say whether it did.
fn verdict<T>(
    window_id: u64,
    result: anyhow::Result<T>,
    counts: anyhow::Result<kwin_helper::GateCounts>,
) -> anyhow::Result<T> {
    let held_back = match counts {
        Ok(counts) if counts.dropped == 0 => return result,
        Ok(counts) => anyhow::anyhow!(
            "input_interrupted: focus left window {window_id} while its input was sent; \
             KWin held back {} event(s), which reached no other window, after {} reached \
             window {window_id}",
            counts.dropped,
            counts.passed
        ),
        Err(error) => anyhow::anyhow!(
            "input_unverified: KWin's input gate for window {window_id} did not report what \
             it let through ({error:#}); any input it held back reached no other window"
        ),
    };
    match result {
        Ok(_) => Err(held_back),
        Err(error) => {
            let note = format!("{error:#}; {held_back}");
            Err(error.context(note))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kwin_helper::GateCounts;

    fn counts(passed: u64, dropped: u64) -> anyhow::Result<GateCounts> {
        Ok(GateCounts { passed, dropped })
    }

    #[test]
    fn input_that_all_reached_the_target_keeps_the_body_result() {
        assert_eq!(verdict(7, Ok("sent"), counts(4, 0)).unwrap(), "sent");
        let error = verdict::<()>(7, Err(anyhow::anyhow!("body failed")), counts(1, 0));
        assert_eq!(error.unwrap_err().to_string(), "body failed");
    }

    #[test]
    fn held_back_input_fails_the_call_even_when_the_body_succeeded() {
        let error = verdict(7, Ok("sent"), counts(1, 2)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "input_interrupted: focus left window 7 while its input was sent; KWin held back \
             2 event(s), which reached no other window, after 1 reached window 7"
        );
    }

    #[test]
    fn a_gate_that_cannot_report_fails_the_call() {
        let error = verdict(7, Ok(()), Err(anyhow::anyhow!("no reply"))).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("input_unverified: KWin's input gate for window 7"),
            "{error}"
        );
    }

    #[test]
    fn a_failed_body_keeps_its_error_and_names_the_held_back_input() {
        let error =
            verdict::<()>(7, Err(anyhow::anyhow!("libei closed")), counts(0, 3)).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("libei closed"), "{text}");
        assert!(text.contains("KWin held back 3 event(s)"), "{text}");
    }

    #[test]
    fn a_window_is_activated_for_input_only_under_its_own_gate() {
        let window = 0xE000_0001;
        let error = activate_for_input(window, None).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("foreground_unavailable: KWin input can be bound"),
            "{error}"
        );
        let _armed = Armed::bind(window + 1);
        assert!(armed());
        let error = activate_for_input(window, None).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("foreground_unavailable: KWin input can be bound"),
            "{error}"
        );
    }

    #[test]
    fn a_gate_for_one_window_refuses_input_to_another() {
        let _armed = Armed::bind(1);
        let ran = std::cell::Cell::new(false);
        let error = gated(2, || {
            ran.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(!ran.get());
        assert!(
            error.to_string().contains("already bound to window 1"),
            "{error}"
        );
        assert_eq!(gated(1, || Ok("reused")).unwrap(), "reused");
    }

    #[test]
    fn the_armed_mark_clears_when_the_body_ends() {
        {
            let _armed = Armed::bind(3);
            assert!(armed());
        }
        assert!(!armed());
    }
}
