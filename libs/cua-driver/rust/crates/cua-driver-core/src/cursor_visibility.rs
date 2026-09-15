// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Agent-cursor visibility for background window-targeted actions.
//!
//! The agent-cursor overlay joins every Space/desktop so the cursor stays
//! visible wherever its target lives. That Space-blindness becomes a bug when
//! a background action addresses a window that is not painted on the user's
//! current Space — e.g. the user sits in a fullscreen app's Space while the
//! target lives on the desktop Space (issue trycua/cua#3801): pinning and
//! gliding the cursor to the target's screen coordinates floats the agent
//! cursor over the unrelated foreground app.
//!
//! This module owns the single platform-neutral decision: suppress the cursor
//! only on positive evidence that a *background* delivery targets a window
//! off the current Space. Unknown Space state fails visible (legacy
//! behavior) so a missing compositor query can never hide the cursor in
//! normal single-Space use. Delivery itself is unaffected — the cursor is
//! pure visual telemetry.
//!
//! Platform adapters resolve `target_on_current_space` from their own window
//! enumeration (on macOS: the on-screen `CGWindowList` plus the Skylight
//! Space metadata in `platform-macos::windows`) and then call
//! [`suppress_agent_cursor_for_background_target`].

/// Decide whether the agent cursor must stay hidden for one window-targeted
/// action.
///
/// - `background` — true when the action uses background delivery (the target
///   is not fronted for this action).
/// - `target_on_current_space` — whether the target window is painted on the
///   user's current Space/desktop: `Some(true)` painted here, `Some(false)`
///   lives on another Space, `None` the platform could not determine it.
///
/// Returns true only for background delivery with positive off-Space
/// evidence. Every other combination shows the cursor (legacy behavior),
/// including unknown Space state.
pub fn suppress_agent_cursor_for_background_target(
    background: bool,
    target_on_current_space: Option<bool>,
) -> bool {
    background && matches!(target_on_current_space, Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_action_against_off_space_target_is_suppressed() {
        // Issue trycua/cua#3801: fullscreen Space in front, background AX
        // click against a desktop-Space window. Painting would float the
        // agent cursor over the unrelated fullscreen app.
        assert!(suppress_agent_cursor_for_background_target(
            true,
            Some(false)
        ));
    }

    #[test]
    fn background_action_on_current_space_stays_visible() {
        // Same-Space background work (e.g. AX press on an occluded window
        // behind the frontmost app) keeps the legacy visible cursor: the
        // coordinates land on atlased content, not an unrelated Space.
        assert!(!suppress_agent_cursor_for_background_target(
            true,
            Some(true)
        ));
    }

    #[test]
    fn foreground_delivery_is_never_suppressed() {
        // Foreground fronts the target before dispatch, so the cursor always
        // paints at content the user can actually see.
        assert!(!suppress_agent_cursor_for_background_target(
            false,
            Some(false)
        ));
        assert!(!suppress_agent_cursor_for_background_target(
            false,
            Some(true)
        ));
        assert!(!suppress_agent_cursor_for_background_target(false, None));
    }

    #[test]
    fn unknown_space_state_fails_visible() {
        // A missing compositor/Space query must never hide the cursor: hiding
        // on uncertain evidence would regress normal single-Space sessions
        // into cursor-less actions with no delivery benefit.
        assert!(!suppress_agent_cursor_for_background_target(true, None));
    }
}
