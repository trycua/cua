//! Pure decision logic for scoping an AX walk to one requested CGWindowID.
//!
//! Split out of [`super::tree`] so the decision — which is where issue #2237's
//! silent menu-bar-only "success" came from — is testable without a
//! WindowServer.
//!
//! **Structural invariant:** the walk set is non-empty ONLY for
//! [`WindowScope::Matched`]. Every failure variant walks nothing, so no
//! unresolved scope can produce a healthy-looking tree of some *other* surface
//! (the app's menu bar) that a caller would then click by `element_index`.

use crate::windows::WindowOwner;

/// What the requested `window_id` turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowScope {
    /// A top-level AXWindow reported the requested CGWindowID.
    Matched,
    /// WindowServer has no record of the requested CGWindowID — closed,
    /// stale, or fabricated.
    NotFound,
    /// The CGWindowID exists but another process owns it. macOS hosts every
    /// sandboxed app's Open/Save panel out-of-process, so this is a routine
    /// shape, not an exotic race.
    OwnerPidMismatch {
        owner_pid: i32,
        owner_app_name: String,
    },
    /// The CGWindowID exists under the requested pid, but no top-level
    /// AXWindow claims it. The screenshot is still the requested window; the
    /// AX surface is not, so callers report a degraded (empty-tree) snapshot
    /// rather than an error.
    AxUnresolved { ax_window_count: usize },
}

impl WindowScope {
    /// True only when the requested window's own AX surface was found.
    /// Element caches and element-token snapshots may be written only then.
    pub fn is_matched(&self) -> bool {
        matches!(self, WindowScope::Matched)
    }
}

/// A top-level child of the application AX element, reduced to the two facts
/// the scoping decision needs.
#[derive(Debug, Clone)]
pub struct TopLevelCandidate {
    pub role: String,
    /// AXSubrole for top-level windows. Dialog-like windows must not inherit
    /// the application's menu bar into their window-scoped snapshot.
    pub subrole: Option<String>,
    /// Stable AppKit AX identifier, when exposed. Tahoe's Open/Save panels
    /// report `AXStandardWindow` rather than a dialog subrole, but identify
    /// themselves as `open-panel` / `save-panel`.
    pub identifier: Option<String>,
    /// `_AXUIElementGetWindow` result, when the SPI resolved one. Only read
    /// for `AXWindow` roles.
    pub ax_window_id: Option<u32>,
}

impl TopLevelCandidate {
    pub fn new(role: impl Into<String>, ax_window_id: Option<u32>) -> Self {
        Self {
            role: role.into(),
            subrole: None,
            identifier: None,
            ax_window_id,
        }
    }

    pub fn with_subrole(mut self, subrole: impl Into<String>) -> Self {
        self.subrole = Some(subrole.into());
        self
    }

    pub fn with_identifier(mut self, identifier: impl Into<String>) -> Self {
        self.identifier = Some(identifier.into());
        self
    }

    fn is_dialog_like(&self) -> bool {
        self.role == "AXSheet"
            || self
                .subrole
                .as_deref()
                .is_some_and(|s| matches!(s, "AXDialog" | "AXSystemDialog" | "AXSheet"))
            || self
                .identifier
                .as_deref()
                .is_some_and(|id| matches!(id, "open-panel" | "save-panel"))
    }
}

/// The scope conclusion plus which candidates to walk at depth 0.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeDecision {
    pub scope: WindowScope,
    /// Indices into the candidate slice. Empty for every non-`Matched` scope.
    pub walk: Vec<usize>,
}

/// The scope conclusion reachable from CGWindowList alone, before any AX work.
///
/// `None` means the window exists under the requested pid, so only the AX walk
/// can decide between [`WindowScope::Matched`] and
/// [`WindowScope::AxUnresolved`].
pub fn scope_from_owner(owner: &WindowOwner) -> Option<WindowScope> {
    match owner {
        WindowOwner::SamePid => None,
        WindowOwner::Unknown => Some(WindowScope::NotFound),
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => Some(WindowScope::OwnerPidMismatch {
            owner_pid: *owner_pid,
            owner_app_name: owner_app_name.clone(),
        }),
    }
}

/// Decide what a window-scoped walk of `candidates` should cover.
///
/// `resolve_owner` is only invoked when no candidate claims `requested` — the
/// CGWindowList enumeration it performs is wasted work on the happy path.
pub fn decide_window_scope<F>(
    candidates: &[TopLevelCandidate],
    requested: u32,
    resolve_owner: F,
) -> ScopeDecision
where
    F: FnOnce() -> WindowOwner,
{
    let matched: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.role == "AXWindow" && c.ax_window_id == Some(requested))
        .map(|(i, _)| i)
        .collect();

    if matched.is_empty() {
        // Nothing claims the id. Ask WindowServer *why*: a fabricated/stale id
        // and a live id owned by another process are different refusals, and a
        // live same-pid id whose AX surface never materialised is not a refusal
        // at all.
        let scope = scope_from_owner(&resolve_owner()).unwrap_or(WindowScope::AxUnresolved {
            ax_window_count: candidates.iter().filter(|c| c.role == "AXWindow").count(),
        });
        return ScopeDecision {
            scope,
            walk: Vec::new(),
        };
    }

    // Matched: walk the requested window plus the non-window top-level children.
    //
    // The menu bar is deliberately among them. MACOS.md documents a
    // two-snapshot menu flow whose first step reads `AXMenuBarItem` rows
    // straight out of this window-scoped tree, and `get_window_state` is the
    // only tool that returns a subtree — dropping AXMenuBar here would remove
    // menu navigation with no replacement path. Keeping other non-window
    // children is also what `browser/consent_ui.rs` relies on to reach a
    // top-level `AXSheet` consent prompt.
    let dialog_scope = matched.iter().any(|&i| candidates[i].is_dialog_like());
    let walk = candidates
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            (c.role != "AXWindow" || matched.contains(i))
                && !(dialog_scope && c.role == "AXMenuBar")
        })
        .map(|(i, _)| i)
        .collect();
    ScopeDecision {
        scope: WindowScope::Matched,
        walk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_service_owner() -> WindowOwner {
        WindowOwner::ForeignPid {
            owner_pid: 900,
            owner_app_name: "Open and Save Panel Service".into(),
        }
    }

    fn never_called() -> WindowOwner {
        panic!("owner lookup must be skipped when a candidate claims the id");
    }

    /// The MACOS.md contract: a resolved normal window keeps the menu bar in
    /// its window-scoped tree, so the documented two-snapshot menu flow
    /// (`AXMenuBarItem` → menu item) still works.
    #[test]
    fn matched_window_walks_window_and_menu_bar() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXWindow", Some(11)),
            TopLevelCandidate::new("AXWindow", Some(22)),
        ];
        let d = decide_window_scope(&candidates, 22, never_called);
        assert_eq!(d.scope, WindowScope::Matched);
        assert_eq!(d.walk, vec![0, 2], "menu bar + requested window only");
    }

    #[test]
    fn matched_dialog_excludes_the_application_menu_bar() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXWindow", Some(11)).with_subrole("AXStandardWindow"),
            TopLevelCandidate::new("AXWindow", Some(22)).with_subrole("AXDialog"),
        ];
        let d = decide_window_scope(&candidates, 22, never_called);
        assert_eq!(d.scope, WindowScope::Matched);
        assert_eq!(d.walk, vec![2], "dialog snapshot must not carry AXMenuBar");
    }

    #[test]
    fn matched_appkit_file_panel_excludes_the_application_menu_bar() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXWindow", Some(11)).with_subrole("AXStandardWindow"),
            TopLevelCandidate::new("AXWindow", Some(22))
                .with_subrole("AXStandardWindow")
                .with_identifier("open-panel"),
        ];
        let d = decide_window_scope(&candidates, 22, never_called);
        assert_eq!(d.scope, WindowScope::Matched);
        assert_eq!(
            d.walk,
            vec![2],
            "AppKit file-panel snapshot must not carry AXMenuBar"
        );
    }

    /// Issue #2237's headline defect: an id nothing claims used to fall through
    /// to `[AXMenuBar, ...]` and return it as a healthy snapshot.
    #[test]
    fn unresolved_id_is_not_a_menu_bar_success() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXWindow", Some(11)),
        ];
        let d = decide_window_scope(&candidates, 67340, || WindowOwner::SamePid);
        assert_eq!(d.scope, WindowScope::AxUnresolved { ax_window_count: 1 });
        assert!(d.walk.is_empty(), "must not walk the menu bar instead");
    }

    #[test]
    fn foreign_id_reports_the_owner_pid() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXWindow", Some(11)),
        ];
        let d = decide_window_scope(&candidates, 67340, panel_service_owner);
        assert_eq!(
            d.scope,
            WindowScope::OwnerPidMismatch {
                owner_pid: 900,
                owner_app_name: "Open and Save Panel Service".into(),
            }
        );
        assert!(d.walk.is_empty());
    }

    #[test]
    fn stale_id_is_not_found() {
        let candidates = [TopLevelCandidate::new("AXWindow", Some(11))];
        let d = decide_window_scope(&candidates, 67340, || WindowOwner::Unknown);
        assert_eq!(d.scope, WindowScope::NotFound);
        assert!(d.walk.is_empty());
    }

    /// A top-level `AXSheet` (or any non-window child) must not act as a
    /// wildcard that makes an unmatched id look resolved.
    #[test]
    fn top_level_sheet_without_the_id_is_not_a_wildcard() {
        let candidates = [
            TopLevelCandidate::new("AXSheet", None),
            TopLevelCandidate::new("AXMenuBar", None),
        ];
        let d = decide_window_scope(&candidates, 67340, || WindowOwner::SamePid);
        assert_eq!(d.scope, WindowScope::AxUnresolved { ax_window_count: 0 });
        assert!(d.walk.is_empty());
    }

    /// A resolved window still carries its sibling sheets — `consent_ui.rs`
    /// walks Chrome's other window ids expecting the consent `AXSheet` as a
    /// top-level non-window child.
    #[test]
    fn matched_window_keeps_sibling_sheets() {
        let candidates = [
            TopLevelCandidate::new("AXSheet", None),
            TopLevelCandidate::new("AXWindow", Some(11)),
        ];
        let d = decide_window_scope(&candidates, 11, never_called);
        assert_eq!(d.scope, WindowScope::Matched);
        assert_eq!(d.walk, vec![0, 1]);
    }

    #[test]
    fn menu_bar_only_app_has_no_ax_windows() {
        let candidates = [TopLevelCandidate::new("AXMenuBar", None)];
        let d = decide_window_scope(&candidates, 11, || WindowOwner::SamePid);
        assert_eq!(d.scope, WindowScope::AxUnresolved { ax_window_count: 0 });
        assert!(d.walk.is_empty());
    }

    #[test]
    fn empty_candidate_list_walks_nothing() {
        for owner in [
            WindowOwner::SamePid,
            WindowOwner::Unknown,
            panel_service_owner(),
        ] {
            let d = decide_window_scope(&[], 11, || owner.clone());
            assert!(d.walk.is_empty());
            assert!(!d.scope.is_matched());
        }
    }

    /// The invariant that makes the reported bug unregressable: no failure
    /// variant may ever hand back a walk set.
    #[test]
    fn menu_bar_is_never_walked_alone() {
        let candidates = [
            TopLevelCandidate::new("AXMenuBar", None),
            TopLevelCandidate::new("AXSheet", None),
            TopLevelCandidate::new("AXWindow", Some(11)),
            // An AXWindow whose window-id SPI failed — the ViewBridge-remoted
            // panel shape.
            TopLevelCandidate::new("AXWindow", None),
        ];
        for owner in [
            WindowOwner::SamePid,
            WindowOwner::Unknown,
            panel_service_owner(),
        ] {
            let d = decide_window_scope(&candidates, 67340, || owner.clone());
            assert!(
                !d.scope.is_matched(),
                "id 67340 is claimed by nothing; scope was {:?}",
                d.scope
            );
            assert!(
                d.walk.is_empty(),
                "failure scope {:?} must walk nothing, got {:?}",
                d.scope,
                d.walk
            );
        }
    }

    #[test]
    fn scope_from_owner_defers_only_for_same_pid() {
        assert_eq!(scope_from_owner(&WindowOwner::SamePid), None);
        assert_eq!(
            scope_from_owner(&WindowOwner::Unknown),
            Some(WindowScope::NotFound)
        );
        assert!(matches!(
            scope_from_owner(&panel_service_owner()),
            Some(WindowScope::OwnerPidMismatch { owner_pid: 900, .. })
        ));
    }
}
