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
    /// The requested window's own owner chain reaches it: a sheet's parent,
    /// a dialog's owner. It never blocks the window attached to it.
    Ancestor,
    /// Attached to some other window of the application.
    OtherWindow,
}

/// One top-level window as an owner-chain platform enumerates it: X11
/// `WM_TRANSIENT_FOR`, Win32 `GW_OWNER`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListedWindow {
    pub window_id: u64,
    /// The owning process, `None` when it cannot be attributed.
    pub pid: Option<u32>,
    /// The window it is transient for or owned by.
    pub owner: Option<u64>,
    pub modal: bool,
    /// Mapped and shown. A hidden window is kept as a link of an owner chain
    /// but is never a surface to act on.
    pub viewable: bool,
}

/// Bound on an owner-chain walk; a misbehaving client can create a cycle.
const MAX_OWNER_CHAIN: usize = 16;

/// The windows [`interaction_context`] weighs for `target`, owned by `pid`:
/// every viewable window of `pid`, and every viewable window of another
/// process whose owner chain reaches one of `pid`'s (a dialog can run in
/// another process), each with its owner chain resolved against `target`:
/// a dialog of a dialog of the target is still the target's.
///
/// A link outside `windows` (an unmapped or unlisted owner) is read with
/// `lookup` as `(owner, pid)`. `None`, so the relation is unobservable, when
/// a link could not be read or a weighed window has no attributable pid.
pub fn related_by_owner_chain(
    target: u64,
    pid: u32,
    windows: &[ListedWindow],
    focused: Option<u64>,
    lookup: impl Fn(u64) -> Option<(Option<u64>, Option<u32>)>,
) -> InteractionFacts {
    let read = |id: u64| match windows.iter().find(|window| window.window_id == id) {
        Some(window) => Some((window.owner, window.pid)),
        None => lookup(id),
    };
    // The links above `start`'s window, each with its pid.
    let chain = |from: u64, start: Option<u64>| -> Option<Vec<(u64, Option<u32>)>> {
        let mut links: Vec<(u64, Option<u32>)> = Vec::new();
        let mut next = start;
        while let Some(id) = next {
            // A cycle proves the chain never reaches anything new; a chain
            // longer than the bound proves nothing.
            if id == from || links.iter().any(|l| l.0 == id) {
                break;
            }
            if links.len() >= MAX_OWNER_CHAIN {
                return None;
            }
            let (owner, owner_pid) = read(id)?;
            links.push((id, owner_pid));
            next = owner;
        }
        Some(links)
    };
    let ancestors = chain(target, read(target)?.0)?;
    let mut related = Vec::new();
    for window in windows
        .iter()
        .filter(|window| window.viewable && window.window_id != target)
    {
        let links = chain(window.window_id, window.owner)?;
        if window.pid != Some(pid) && !links.iter().any(|link| link.1 == Some(pid)) {
            continue;
        }
        let owner = if ancestors.iter().any(|link| link.0 == window.window_id) {
            WindowOwnership::Ancestor
        } else if links.iter().any(|link| link.0 == target) {
            WindowOwnership::Target
        } else if window.owner.is_none() {
            WindowOwnership::Unowned
        } else {
            WindowOwnership::OtherWindow
        };
        related.push(RelatedWindowFacts {
            pid: i64::from(window.pid?),
            window_id: window.window_id,
            owner,
            modal: window.modal,
            focused: focused == Some(window.window_id),
        });
    }
    Some(related)
}

/// Mark what blocks a Win32 target. A modal loop (`DialogBox`, an owned
/// `MessageBox`, WPF `ShowDialog`) disables windows while it runs: the
/// dialog's owner, or every window of its UI thread. So an enabled target is
/// blocked by nothing, and a disabled one by the only related window still
/// enabled that a modal loop could have disabled it for: one the target
/// owns, or one on the target's UI thread, whichever window owns it. A
/// window the target is attached to is never one: a modal loop that left it
/// enabled disabled the target for nothing. An enabled window on another
/// thread with no owner link to the target is no evidence either: an app can
/// disable a window for its own reasons. With none or
/// several candidates, Win32 does not say which one blocks: `None`, so the
/// caller reports the relation as unobservable.
pub fn block_disabled_target(
    mut related: Vec<RelatedWindowFacts>,
    target_enabled: bool,
    enabled: impl Fn(u64) -> bool,
    on_target_thread: impl Fn(u64) -> bool,
) -> InteractionFacts {
    for window in &mut related {
        window.modal = false;
    }
    if target_enabled {
        return Some(related);
    }
    let mut live = related.iter_mut().filter(|window| {
        enabled(window.window_id)
            && window.owner != WindowOwnership::Ancestor
            && (window.owner == WindowOwnership::Target || on_target_thread(window.window_id))
    });
    let (Some(blocker), None) = (live.next(), live.next()) else {
        return None;
    };
    blocker.modal = true;
    if !matches!(blocker.owner, WindowOwnership::Target) {
        // It blocks the target though it hangs off another window.
        blocker.owner = WindowOwnership::Unowned;
    }
    Some(related)
}

/// Keep only the windows `get_window_state` can address, so `next_call`
/// never names one it would refuse. A modal or focused window that cannot be
/// addressed could be the surface to act on, so the whole relation becomes
/// unobservable (`None`, inspect the desktop instead); any other unaddressable
/// window is left out.
pub fn keep_addressable(
    related: Vec<RelatedWindowFacts>,
    addressable: impl Fn(u64) -> bool,
) -> InteractionFacts {
    let mut kept = Vec::with_capacity(related.len());
    for window in related {
        if addressable(window.window_id) {
            kept.push(window);
        } else if window.modal || window.focused {
            return None;
        }
    }
    Some(kept)
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
        // A modal attached to a sibling blocks only that sibling, an
        // ordinary sibling is not this window's business, focused or not,
        // and the window the target is attached to never blocks it.
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
/// windows another process parented to them or, on X11, put in their window
/// group), never other processes' prompts.
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

    fn listed(window_id: u64, pid: Option<u32>, owner: Option<u64>, modal: bool) -> ListedWindow {
        ListedWindow {
            window_id,
            pid,
            owner,
            modal,
            viewable: true,
        }
    }

    /// Owners of windows outside the list: 9 belongs to pid 7, 30 hides the
    /// target's dialog chain, and 99 cannot be read.
    fn lookup(id: u64) -> Option<(Option<u64>, Option<u32>)> {
        match id {
            9 => Some((None, Some(7))),
            30 => Some((Some(1), Some(7))),
            _ => None,
        }
    }

    #[test]
    fn owner_chains_resolve_nested_dialogs_and_stop_on_cycles() {
        let windows = [
            listed(1, Some(7), None, false),    // the target
            listed(2, Some(7), Some(1), false), // its dialog
            listed(3, Some(7), Some(2), true),  // a modal dialog of that dialog
            listed(4, Some(7), Some(9), true),  // a modal of a window not listed
            listed(5, Some(7), Some(6), false), // a cycle that never reaches the target
            listed(6, Some(7), Some(5), false),
            listed(8, Some(7), None, true), // application-modal
            // Another process's modal on the target's dialog, and one on a
            // hidden dialog of the target.
            listed(20, Some(8), Some(2), true),
            listed(21, Some(8), Some(30), true),
            // An unrelated app's modal.
            listed(40, Some(9), None, true),
            // A hidden window is a link, never a surface.
            ListedWindow {
                viewable: false,
                ..listed(41, Some(7), Some(1), true)
            },
        ];
        let related = related_by_owner_chain(1, 7, &windows, Some(3), lookup).unwrap();
        let owners: Vec<(u64, i64, WindowOwnership)> = related
            .iter()
            .map(|facts| (facts.window_id, facts.pid, facts.owner))
            .collect();
        assert_eq!(
            owners,
            vec![
                (2, 7, WindowOwnership::Target),
                (3, 7, WindowOwnership::Target),
                (4, 7, WindowOwnership::OtherWindow),
                (5, 7, WindowOwnership::OtherWindow),
                (6, 7, WindowOwnership::OtherWindow),
                (8, 7, WindowOwnership::Unowned),
                (20, 8, WindowOwnership::Target),
                (21, 8, WindowOwnership::Target),
            ]
        );
        let context = interaction_context(&Some(related));
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
        assert_eq!(
            relations,
            vec![
                (3, "modal_child"),
                (8, "blocks_target"),
                (20, "modal_child"),
                (21, "modal_child")
            ]
        );
        assert_eq!(context["next_call"]["arguments"]["window_id"], 3);
    }

    #[test]
    fn a_window_the_target_is_attached_to_never_blocks_it() {
        // Target 2 is a modal dialog of the application-modal window 8.
        let windows = [
            listed(8, Some(7), None, true),
            listed(2, Some(7), Some(8), true),
        ];
        let related = related_by_owner_chain(2, 7, &windows, None, lookup).unwrap();
        assert_eq!(related[0].owner, WindowOwnership::Ancestor);
        assert_eq!(
            interaction_context(&Some(related))["status"],
            "none_observed"
        );
        // And from 8, its dialog is the blocker: no loop between the two.
        let related = related_by_owner_chain(8, 7, &windows, None, lookup).unwrap();
        assert_eq!(
            interaction_context(&Some(related))["next_call"]["arguments"]["window_id"],
            2
        );
    }

    #[test]
    fn an_unreadable_link_or_unattributed_window_is_unobservable() {
        let unreadable = [
            listed(1, Some(7), None, false),
            listed(5, Some(8), Some(99), true),
        ];
        assert_eq!(
            related_by_owner_chain(1, 7, &unreadable, None, lookup),
            None
        );
        let unattributed = [
            listed(1, Some(7), None, false),
            listed(5, None, Some(1), true),
        ];
        assert_eq!(
            related_by_owner_chain(1, 7, &unattributed, None, lookup),
            None
        );
    }

    #[test]
    fn a_disabled_targets_only_enabled_related_window_blocks_it() {
        // WPF ShowDialog: dialog 3 owned by window A (1) disables every
        // window of the thread, so sibling B (2) and its modeless palette 4
        // are disabled too.
        let related = vec![
            window(1, WindowOwnership::Unowned, false, false),
            window(3, WindowOwnership::OtherWindow, false, true),
            window(4, WindowOwnership::Target, false, false),
        ];
        let enabled = |hwnd: u64| hwnd == 3;
        let same_thread = |_: u64| true;
        let facts = block_disabled_target(related.clone(), false, enabled, same_thread).unwrap();
        let context = interaction_context(&Some(facts));
        assert_eq!(context["status"], "blocked");
        assert_eq!(context["related_surfaces"][0]["relation"], "blocks_target");
        assert_eq!(context["next_call"]["arguments"]["window_id"], 3);
        // An enabled target is blocked by nothing.
        let facts = block_disabled_target(related.clone(), true, enabled, same_thread).unwrap();
        assert!(facts.iter().all(|window| !window.modal));
        // The target's own dialog stays its modal child, on any thread.
        let own = vec![window(5, WindowOwnership::Target, false, true)];
        let facts = block_disabled_target(own, false, |hwnd| hwnd == 5, |_| false).unwrap();
        assert_eq!(
            interaction_context(&Some(facts))["related_surfaces"][0]["relation"],
            "modal_child"
        );
    }

    #[test]
    fn a_disabled_target_with_no_or_several_enabled_windows_is_unknown() {
        let related = vec![
            window(2, WindowOwnership::Target, false, false),
            window(3, WindowOwnership::Target, false, false),
        ];
        assert_eq!(
            block_disabled_target(related.clone(), false, |_| true, |_| true),
            None
        );
        assert_eq!(
            block_disabled_target(related, false, |_| false, |_| true),
            None
        );
    }

    #[test]
    fn an_enabled_window_on_another_thread_does_not_block_a_disabled_target() {
        // The app disabled window 1 itself; 2 is a modeless window of
        // another UI thread with no owner link to 1.
        let related = vec![window(2, WindowOwnership::Unowned, false, true)];
        assert_eq!(
            block_disabled_target(related.clone(), false, |_| true, |_| false),
            None
        );
        let other_window = vec![window(3, WindowOwnership::OtherWindow, false, true)];
        assert_eq!(
            block_disabled_target(other_window, false, |_| true, |_| false),
            None
        );
    }

    #[test]
    fn an_enabled_ancestor_does_not_block_a_disabled_target() {
        // The app disabled its modeless palette 1 itself; its owner 2 on the
        // same thread stays enabled and runs no modal loop.
        let related = vec![window(2, WindowOwnership::Ancestor, false, false)];
        assert_eq!(
            block_disabled_target(related, false, |_| true, |_| true),
            None
        );
        // The target's own dialog is the one blocker, its enabled owner aside.
        let related = vec![
            window(2, WindowOwnership::Ancestor, false, false),
            window(3, WindowOwnership::Target, false, true),
        ];
        let facts = block_disabled_target(related, false, |_| true, |_| true).unwrap();
        let context = interaction_context(&Some(facts));
        assert_eq!(context["related_surfaces"][0]["relation"], "modal_child");
        assert_eq!(context["next_call"]["arguments"]["window_id"], 3);
    }

    #[test]
    fn an_owner_chain_past_the_bound_is_unobservable() {
        // 2 -> 3 -> ... -> 40, never reaching the target or repeating.
        let mut windows = vec![listed(1, Some(7), None, false)];
        windows.extend((2..40).map(|id| listed(id, Some(8), Some(id + 1), false)));
        windows.push(listed(40, Some(8), None, false));
        assert_eq!(related_by_owner_chain(1, 7, &windows, None, lookup), None);
    }

    #[test]
    fn an_unaddressable_modal_or_focused_window_makes_the_relation_unobservable() {
        let addressable = |id: u64| id < 100;
        // An untitled inert window is dropped; the rest stay.
        let kept = keep_addressable(
            vec![
                window(5, WindowOwnership::Target, true, false),
                window(105, WindowOwnership::Target, false, false),
            ],
            addressable,
        )
        .unwrap();
        assert_eq!(
            kept.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![5]
        );
        // An untitled modal, or an untitled window holding focus, could be
        // the surface to act on but cannot be named in next_call.
        for unaddressable in [
            window(101, WindowOwnership::Target, true, false),
            window(102, WindowOwnership::Target, false, true),
        ] {
            let facts = keep_addressable(vec![unaddressable], addressable);
            assert_eq!(facts, None);
            assert_eq!(interaction_context(&facts)["status"], "unobservable");
        }
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
