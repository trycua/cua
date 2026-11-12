// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform window-snapshot coverage signals.
//!
//! A window-scoped snapshot describes one native window, but the interaction
//! may have moved to a related surface: a sheet or modal dialog that blocks
//! the requested window, or an owned window that took focus from it.
//! [`interaction_context`] decides, from facts each platform reads about the
//! application's other windows, whether such a surface was observed, and
//! names it by pid and window id so the caller can inspect it next.
//!
//! A browser permission bubble is browser chrome, not page content. Chromium
//! may composite that chrome in the requested window or in a popup/child
//! surface which is visible only in a desktop capture. A single-window backend
//! therefore cannot always prove that no browser-owned blocker exists. Keep
//! that platform-specific limitation explicit and machine-readable.

use serde_json::{json, Value};

pub const BROWSER_CHROME_COVERAGE_STATUS: &str = "not_observable_in_window_scope";
pub const BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS: &str = "may_be_incomplete_in_window_scope";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserChromeCaptureCoverage {
    NotObservable,
    MayBeIncomplete,
}

impl BrowserChromeCaptureCoverage {
    fn status(self) -> &'static str {
        match self {
            Self::NotObservable => BROWSER_CHROME_COVERAGE_STATUS,
            Self::MayBeIncomplete => BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS,
        }
    }
}

/// Describe the browser-chrome coverage limit of a Chromium-family window
/// snapshot.
///
/// This deliberately does **not** claim that a prompt is present. Presence is
/// not reliably complete from a single native-window surface on every
/// supported platform. It also carries no prompt text or choices, so routine
/// traces can retain the recovery signal without retaining permission content.
pub fn mark_browser_chrome_capture_coverage(
    structured: &mut Value,
    coverage: Option<BrowserChromeCaptureCoverage>,
) {
    let Some(coverage) = coverage else { return };

    structured["capture_coverage"] = json!({
        "browser_chrome": {
            "status": coverage.status()
        },
        "recovery": {
            "when": "verified_window_action_ineffective",
            "inspect": "get_desktop_state",
            "act_target": {
                "kind": "desktop",
                "display_id": "primary"
            },
            "verify": "get_desktop_state"
        }
    });
}

/// What a platform read about one other top-level window of the target
/// application (or a window another process parented to it).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelatedWindowFacts {
    pub pid: i64,
    pub window_id: u64,
    /// How the window is attached to others: a macOS sheet's parent, an X11
    /// `WM_TRANSIENT_FOR`, a Win32 `GW_OWNER`.
    pub owner: WindowOwnership,
    /// The window blocks input to its owner (macOS sheets and `AXModal`
    /// windows, `_NET_WM_STATE_MODAL`, a Win32 owner that is disabled).
    pub modal: bool,
    /// The window holds the application's keyboard focus.
    pub focused: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowOwnership {
    /// Attached to no window: application-scoped when modal.
    Unowned,
    /// Its owner chain reaches the requested window.
    Target,
    /// Attached to some other window of the application.
    OtherWindow,
}

/// One other window as an owner-chain platform reads it: X11
/// `WM_TRANSIENT_FOR`, Win32 `GW_OWNER`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedWindow {
    pub pid: i64,
    pub window_id: u64,
    /// The window it is transient for or owned by.
    pub owner: Option<u64>,
    pub modal: bool,
    pub focused: bool,
}

/// Bound on an owner-chain walk; a misbehaving client can create a cycle.
const MAX_OWNER_CHAIN: usize = 16;

/// Resolve each window's owner chain against `target`: a dialog of a dialog
/// of the target is still the target's. A chain that leaves `windows` without
/// reaching the target belongs to another window.
pub fn related_by_owner_chain(target: u64, windows: &[OwnedWindow]) -> Vec<RelatedWindowFacts> {
    let owner_of = |id: u64| {
        windows
            .iter()
            .find(|window| window.window_id == id)
            .and_then(|window| window.owner)
    };
    windows
        .iter()
        .filter(|window| window.window_id != target)
        .map(|window| {
            let mut owner = WindowOwnership::Unowned;
            let mut next = window.owner;
            for _ in 0..MAX_OWNER_CHAIN {
                match next {
                    None => break,
                    Some(id) if id == target => {
                        owner = WindowOwnership::Target;
                        break;
                    }
                    Some(id) => {
                        owner = WindowOwnership::OtherWindow;
                        next = owner_of(id).filter(|_| id != window.window_id);
                    }
                }
            }
            RelatedWindowFacts {
                pid: window.pid,
                window_id: window.window_id,
                owner,
                modal: window.modal,
                focused: window.focused,
            }
        })
        .collect()
}

/// Which owned windows are modal on Win32, where `DialogBox` and an owned
/// `MessageBox` disable their owner while they run: a disabled owner with
/// exactly one visible owned window is blocked by that window. With several
/// (a dialog beside a modeless palette), Win32 does not say which one is
/// modal, and a disabled target owning none is blocked by something not
/// listed: both are `None`, so the caller reports the relation as
/// unobservable.
pub fn modal_by_disabled_owner(
    target: u64,
    windows: &[(u64, Option<u64>)],
    enabled: impl Fn(u64) -> bool,
) -> Option<Vec<u64>> {
    let mut owners: Vec<u64> = windows.iter().filter_map(|(_, owner)| *owner).collect();
    owners.sort_unstable();
    owners.dedup();
    let mut modal = Vec::new();
    for owner in owners.into_iter().filter(|owner| !enabled(*owner)) {
        let mut owned = windows
            .iter()
            .filter(|(_, window_owner)| *window_owner == Some(owner));
        match (owned.next(), owned.next()) {
            (Some((hwnd, _)), None) => modal.push(*hwnd),
            _ => return None,
        }
    }
    if !enabled(target)
        && !windows
            .iter()
            .any(|(hwnd, owner)| *owner == Some(target) && modal.contains(hwnd))
    {
        return None;
    }
    Some(modal)
}

/// The facts behind one [`interaction_context`] decision. `None` means the
/// platform could not enumerate the application's windows or their state, so
/// absence of a related surface proves nothing.
pub type InteractionFacts = Option<Vec<RelatedWindowFacts>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceRelation {
    /// A modal window or sheet attached to the requested window.
    ModalChild,
    /// An application-modal window: it blocks every window of the app.
    BlocksTarget,
    /// A non-modal window owned by the requested window holds the app's
    /// focus, so keyboard input now goes there.
    SupersedesTarget,
}

impl SurfaceRelation {
    fn name(self) -> &'static str {
        match self {
            Self::ModalChild => "modal_child",
            Self::BlocksTarget => "blocks_target",
            Self::SupersedesTarget => "supersedes_target",
        }
    }

    fn blocks(self) -> bool {
        matches!(self, Self::ModalChild | Self::BlocksTarget)
    }
}

fn relation(window: &RelatedWindowFacts) -> Option<SurfaceRelation> {
    match (window.owner, window.modal, window.focused) {
        (WindowOwnership::Target, true, _) => Some(SurfaceRelation::ModalChild),
        (WindowOwnership::Unowned, true, _) => Some(SurfaceRelation::BlocksTarget),
        (WindowOwnership::Target, false, true) => Some(SurfaceRelation::SupersedesTarget),
        // A modal attached to a sibling blocks only that sibling, and an
        // ordinary sibling is not this window's business, focused or not.
        _ => None,
    }
}

/// Describe whether the requested window is still the surface that receives
/// interaction, as `structuredContent.interaction_context`.
///
/// - `none_observed`: no related blocking or superseding window was seen
///   among the application's windows (`scope`). Surfaces outside that scope
///   (browser chrome, OS prompts in another process) are not proven absent;
///   `capture_coverage` names the known ones.
/// - `blocked`: a modal child or application-modal window blocks the target;
///   act on it (`next_call`) before retrying the target.
/// - `superseded`: an owned window holds focus; it is the likely next target.
/// - `unobservable`: the window set could not be read; after a verified
///   ineffective window action, broaden to desktop scope.
///
/// `focus_elsewhere` names the other window that holds the application's
/// focus, related or not, so two snapshots show a focus move.
///
/// Only identifiers are reported: no titles, prompt text or choices.
pub fn interaction_context(facts: &InteractionFacts) -> Value {
    let Some(windows) = facts else {
        return json!({
            "status": "unobservable",
            "recovery": {
                "when": "verified_window_action_ineffective",
                "inspect": "get_desktop_state",
                "act_target": {"kind": "desktop", "display_id": "primary"},
                "verify": "get_desktop_state"
            }
        });
    };
    let mut related: Vec<(SurfaceRelation, &RelatedWindowFacts)> = windows
        .iter()
        .filter_map(|window| relation(window).map(|relation| (relation, window)))
        .collect();
    // The surface to act on first: a blocker before a superseding window, and
    // among equals the one holding focus.
    related.sort_by_key(|(relation, window)| (!relation.blocks(), !window.focused));
    // Which other window holds the app's focus, even an ordinary sibling:
    // a caller comparing two snapshots sees a newly focused window as
    // progress without being told to escalate.
    let focus = windows
        .iter()
        .find(|window| window.focused)
        .map(|window| json!({"pid": window.pid, "window_id": window.window_id}));
    let Some((first_relation, first)) = related.first() else {
        let mut context = json!({"status": "none_observed", "scope": OBSERVED_SCOPE});
        if let Some(focus) = focus {
            context["focus_elsewhere"] = focus;
        }
        return context;
    };
    let status = if first_relation.blocks() {
        "blocked"
    } else {
        "superseded"
    };
    let mut context = json!({
        "status": status,
        "scope": OBSERVED_SCOPE,
        "related_surfaces": related
            .iter()
            .map(|(relation, window)| json!({
                "relation": relation.name(),
                "pid": window.pid,
                "window_id": window.window_id,
                "focused": window.focused,
            }))
            .collect::<Vec<_>>(),
        "next_call": {
            "tool": "get_window_state",
            "arguments": {"pid": first.pid, "window_id": first.window_id}
        }
    });
    if let Some(focus) = focus {
        context["focus_elsewhere"] = focus;
    }
    context
}

/// What an observed status covers: the application's own windows (and
/// windows another process parented to them), never other processes' prompts.
const OBSERVED_SCOPE: &str = "application_windows";

/// Record [`interaction_context`] on a window snapshot.
pub fn mark_interaction_context(structured: &mut Value, facts: &InteractionFacts) {
    structured["interaction_context"] = interaction_context(facts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(
        window_id: u64,
        owner: WindowOwnership,
        modal: bool,
        focused: bool,
    ) -> RelatedWindowFacts {
        RelatedWindowFacts {
            pid: 42,
            window_id,
            owner,
            modal,
            focused,
        }
    }

    #[test]
    fn an_attached_sheet_blocks_the_target_and_is_the_next_call() {
        let context = interaction_context(&Some(vec![
            window(8, WindowOwnership::Unowned, false, false),
            window(9, WindowOwnership::Target, true, true),
        ]));
        assert_eq!(
            context,
            json!({
                "status": "blocked",
                "scope": "application_windows",
                "related_surfaces": [
                    {"relation": "modal_child", "pid": 42, "window_id": 9, "focused": true}
                ],
                "next_call": {
                    "tool": "get_window_state",
                    "arguments": {"pid": 42, "window_id": 9}
                },
                "focus_elsewhere": {"pid": 42, "window_id": 9}
            })
        );
    }

    #[test]
    fn an_application_modal_alert_blocks_every_window() {
        let context = interaction_context(&Some(vec![window(
            11,
            WindowOwnership::Unowned,
            true,
            true,
        )]));
        assert_eq!(context["status"], "blocked");
        assert_eq!(context["related_surfaces"][0]["relation"], "blocks_target");
        assert_eq!(context["next_call"]["arguments"]["window_id"], 11);
    }

    #[test]
    fn a_focused_owned_window_supersedes_but_a_blocker_comes_first() {
        let owned = window(12, WindowOwnership::Target, false, true);
        let superseded = interaction_context(&Some(vec![owned]));
        assert_eq!(superseded["status"], "superseded");
        assert_eq!(
            superseded["related_surfaces"][0]["relation"],
            "supersedes_target"
        );
        assert_eq!(superseded["next_call"]["arguments"]["window_id"], 12);

        let both = interaction_context(&Some(vec![
            owned,
            window(13, WindowOwnership::Unowned, true, false),
        ]));
        assert_eq!(both["status"], "blocked");
        assert_eq!(both["next_call"]["arguments"]["window_id"], 13);
        assert_eq!(both["related_surfaces"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn ordinary_siblings_and_their_own_modals_do_not_escalate() {
        let context = interaction_context(&Some(vec![
            // A focused ordinary sibling window.
            window(20, WindowOwnership::Unowned, false, true),
            // A sheet on that sibling, not on the target.
            window(21, WindowOwnership::OtherWindow, true, false),
            // A non-modal owned palette that does not hold focus.
            window(22, WindowOwnership::Target, false, false),
        ]));
        assert_eq!(
            context,
            json!({
                "status": "none_observed",
                "scope": "application_windows",
                "focus_elsewhere": {"pid": 42, "window_id": 20}
            })
        );
        assert_eq!(
            interaction_context(&Some(Vec::new())),
            json!({"status": "none_observed", "scope": "application_windows"})
        );
    }

    #[test]
    fn owner_chains_resolve_nested_dialogs_and_stop_on_cycles() {
        let owned = |window_id, owner, modal| OwnedWindow {
            pid: 7,
            window_id,
            owner,
            modal,
            focused: false,
        };
        let windows = [
            owned(1, None, false),    // the target
            owned(2, Some(1), false), // its dialog
            owned(3, Some(2), true),  // a modal dialog of that dialog
            owned(4, Some(9), true),  // a modal of a window not listed
            owned(5, Some(6), false), // a cycle that never reaches the target
            owned(6, Some(5), false),
            owned(8, None, true), // application-modal
        ];
        let owners: Vec<(u64, WindowOwnership)> = related_by_owner_chain(1, &windows)
            .into_iter()
            .map(|facts| (facts.window_id, facts.owner))
            .collect();
        assert_eq!(
            owners,
            vec![
                (2, WindowOwnership::Target),
                (3, WindowOwnership::Target),
                (4, WindowOwnership::OtherWindow),
                (5, WindowOwnership::OtherWindow),
                (6, WindowOwnership::OtherWindow),
                (8, WindowOwnership::Unowned),
            ]
        );
        let context = interaction_context(&Some(related_by_owner_chain(1, &windows)));
        let relations: Vec<(u64, &str)> = context["related_surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["window_id"].as_u64().unwrap(),
                    s["relation"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(relations, vec![(3, "modal_child"), (8, "blocks_target")]);
    }

    #[test]
    fn a_disabled_owners_only_owned_window_is_modal() {
        // Target 1 is disabled by its dialog 3; window 5 is an unrelated
        // enabled top-level window.
        let windows = [(1, None), (3, Some(1)), (5, None)];
        assert_eq!(
            modal_by_disabled_owner(1, &windows, |hwnd| hwnd != 1),
            Some(vec![3])
        );
        // An enabled owner's tool window is not modal.
        assert_eq!(modal_by_disabled_owner(1, &windows, |_| true), Some(vec![]));
    }

    #[test]
    fn a_dialog_beside_a_palette_or_an_unseen_blocker_is_unknown() {
        // Disabled owner 1 owns dialog 3 and modeless palette 2.
        let windows = [(1, None), (2, Some(1)), (3, Some(1))];
        assert_eq!(modal_by_disabled_owner(1, &windows, |hwnd| hwnd != 1), None);
        // The target is disabled but owns nothing visible.
        assert_eq!(
            modal_by_disabled_owner(1, &[(1, None)], |hwnd| hwnd != 1),
            None
        );
    }

    #[test]
    fn unreadable_window_set_routes_to_desktop_scope() {
        let context = interaction_context(&None);
        assert_eq!(context["status"], "unobservable");
        assert_eq!(
            context["recovery"]["when"],
            "verified_window_action_ineffective"
        );
        assert_eq!(context["recovery"]["inspect"], "get_desktop_state");
        assert_eq!(
            context["recovery"]["act_target"],
            json!({"kind": "desktop", "display_id": "primary"})
        );
        assert!(context.get("next_call").is_none());
    }

    #[test]
    fn interaction_context_carries_identifiers_only() {
        let public =
            interaction_context(&Some(vec![window(9, WindowOwnership::Target, true, true)]))
                .to_string();
        for sensitive_key in ["title", "text", "message", "choice", "label"] {
            assert!(
                !public.contains(sensitive_key),
                "interaction context leaked a content field: {public}"
            );
        }
    }

    #[test]
    fn browser_window_declares_capture_coverage_without_claiming_presence() {
        let mut before = json!({
            "window_id": 7,
            "pid": 42,
            "tree_markdown": "page=unchanged",
            "screenshot_width": 900,
            "screenshot_height": 640
        });
        let mut after_visible_browser_blocker = before.clone();

        mark_browser_chrome_capture_coverage(
            &mut before,
            Some(BrowserChromeCaptureCoverage::NotObservable),
        );
        mark_browser_chrome_capture_coverage(
            &mut after_visible_browser_blocker,
            Some(BrowserChromeCaptureCoverage::NotObservable),
        );

        // Even when the page-owned state is byte-for-byte unchanged, callers
        // receive an explicit recovery branch instead of treating the window
        // snapshot as proof that the action was ignored.
        assert_eq!(before, after_visible_browser_blocker);
        assert_eq!(
            before["capture_coverage"]["browser_chrome"]["status"],
            BROWSER_CHROME_COVERAGE_STATUS
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["when"],
            "verified_window_action_ineffective"
        );
        assert!(before["capture_coverage"]["recovery"]
            .get("escalate")
            .is_none());
        assert_eq!(
            before["capture_coverage"]["recovery"]["inspect"],
            "get_desktop_state"
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["act_target"],
            json!({"kind": "desktop", "display_id": "primary"})
        );
        assert!(before.get("browser_chrome_prompt").is_none());
    }

    #[test]
    fn signal_is_privacy_minimal_and_does_not_change_non_browser_snapshots() {
        let mut ordinary = json!({"window_id": 9, "pid": 3});
        let unchanged = ordinary.clone();
        mark_browser_chrome_capture_coverage(&mut ordinary, None);
        assert_eq!(ordinary, unchanged);

        let mut browser = unchanged;
        mark_browser_chrome_capture_coverage(
            &mut browser,
            Some(BrowserChromeCaptureCoverage::MayBeIncomplete),
        );
        assert_eq!(
            browser["capture_coverage"]["browser_chrome"]["status"],
            BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS
        );
        let public = browser.to_string();
        for sensitive_key in ["text", "message", "choice", "allow", "deny"] {
            assert!(
                !public.contains(sensitive_key),
                "coverage signal leaked a prompt-content field: {public}"
            );
        }
    }
}
