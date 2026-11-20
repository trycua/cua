// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Native-window ↔ CDP-target correlation (pure, side-effect free).
//!
//! The native entrypoint is `pid + window_id`. CDP exposes page targets
//! (tabs), each mappable to a CDP window via `Browser.getWindowForTarget`
//! + `Browser.getWindowBounds`. Correlation is unique-or-refuse:
//!
//! 1. Collapse page targets to one representative per proven CDP window;
//!    tabs are not independent native-window candidates.
//! 2. Filter candidates whose CDP window bounds match the native bounds
//!    within `tolerance` device pixels.
//! 3. Exactly one bounds match → **Exact** binding.
//! 4. Multiple distinct CDP windows can share maximized bounds. One unique
//!    active-tab title match disambiguates them; duplicate or absent title
//!    matches remain ambiguous. A complete native-window snapshot can
//!    additionally resolve them when every one-to-one window assignment
//!    maps the requested native window to the same CDP window.
//! 5. No bounds match → a unique title match degrades to **Heuristic**
//!    (read-only); otherwise **None**.

use super::types::{BindingQuality, NativeWindowInfo, Rect};

/// One CDP page target with its resolved CDP window geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpWindowCandidate {
    pub cdp_target_id: String,
    /// Absent only when an embedded Chromium endpoint does not implement
    /// Browser.getWindowForTarget. Core may use the bounded single-page /
    /// single-native-window fallback in that case.
    pub cdp_window_id: Option<i64>,
    pub title: String,
    pub url: String,
    pub bounds: Option<Rect>,
}

/// Outcome of correlating one native window against the CDP candidates.
#[derive(Debug, Clone, PartialEq)]
pub enum BindingOutcome {
    Bound {
        candidate: CdpWindowCandidate,
        quality: BindingQuality,
    },
    /// Multiple candidates survived; refuse rather than guess. Carries
    /// only the count so native CDP target ids never escape the core.
    Ambiguous(usize),
    /// Nothing correlates at all.
    None,
}

/// Select the bounded fallback for embedded Chromium endpoints that omit the
/// Browser window APIs. Exactness comes from two independent cardinality
/// proofs: one CDP page and exactly one native window for the endpoint owner.
pub fn embedded_single_page_candidate(
    candidates: &[CdpWindowCandidate],
    is_only_exact_native_window: Option<bool>,
) -> Option<CdpWindowCandidate> {
    let candidate = candidates.first()?;
    (candidates.len() == 1
        && candidate.cdp_window_id.is_none()
        && candidate.bounds.is_none()
        && is_only_exact_native_window == Some(true))
    .then(|| candidate.clone())
}

/// Select the only CDP window when the platform independently attests that the
/// endpoint owner has exactly one native window. This remains exact when a
/// tiling compositor overrides Chromium's requested bounds: one owned native
/// window and one owned CDP window form a proven one-to-one mapping.
pub fn cardinality_exact_candidate(
    native_title: &str,
    candidates: &[CdpWindowCandidate],
    is_only_exact_native_window: Option<bool>,
) -> Option<CdpWindowCandidate> {
    if is_only_exact_native_window != Some(true) {
        return None;
    }
    if let Some(candidate) = embedded_single_page_candidate(candidates, Some(true)) {
        return Some(candidate);
    }
    let representatives = window_representatives(native_title, candidates);
    (representatives.len() == 1 && representatives[0].cdp_window_id.is_some())
        .then(|| representatives[0].clone())
}

/// Whether the native window title plausibly displays this tab's title.
/// Browsers render "<tab title> - <product>", so containment (not
/// equality) is the tie-break; empty tab titles never match.
fn title_matches(native_title: &str, candidate_title: &str) -> bool {
    !candidate_title.is_empty() && native_title.contains(candidate_title)
}

/// Prove the selected page in one already-correlated browser window without
/// activating a target. Chromium exposes the native window title but no
/// browser-level selected-tab bit. A unique title match is therefore proof;
/// duplicate, empty, absent, or substring-colliding titles remain unknown.
/// Embedded single-page endpoints are selected by cardinality.
pub fn selected_tab_target_id<'a>(
    native_title: &str,
    candidates: &'a [CdpWindowCandidate],
    cdp_window_id: Option<i64>,
) -> Option<&'a str> {
    let tabs = candidates
        .iter()
        .filter(|candidate| match cdp_window_id {
            Some(window_id) => candidate.cdp_window_id == Some(window_id),
            None => candidate.cdp_window_id.is_none(),
        })
        .collect::<Vec<_>>();

    if cdp_window_id.is_none() {
        return (tabs.len() == 1).then(|| tabs[0].cdp_target_id.as_str());
    }

    let title_hits = tabs
        .into_iter()
        .filter(|candidate| title_matches(native_title, &candidate.title))
        .collect::<Vec<_>>();
    (title_hits.len() == 1).then(|| title_hits[0].cdp_target_id.as_str())
}

/// Reduce page targets to one representative per proven CDP window.
/// The representative prefers a tab title displayed by the native
/// window, but its target id is only a handle for the selected window;
/// the engine retains every original page target as a tab capability.
/// Candidates without a CDP window id remain independent because they
/// cannot prove they share a native surface.
fn window_representatives(
    native_title: &str,
    candidates: &[CdpWindowCandidate],
) -> Vec<CdpWindowCandidate> {
    let mut representatives: Vec<CdpWindowCandidate> = Vec::new();
    for candidate in candidates {
        let Some(window_id) = candidate.cdp_window_id else {
            representatives.push(candidate.clone());
            continue;
        };
        match representatives
            .iter_mut()
            .find(|existing| existing.cdp_window_id == Some(window_id))
        {
            Some(existing)
                if !title_matches(native_title, &existing.title)
                    && title_matches(native_title, &candidate.title) =>
            {
                *existing = candidate.clone();
            }
            Some(_) => {}
            None => representatives.push(candidate.clone()),
        }
    }
    representatives
}

/// Correlate `native` against `candidates` with unique-or-refuse
/// semantics. `tolerance` is in device pixels (see [`Rect::approx_eq`]).
pub fn correlate(
    native: &NativeWindowInfo,
    candidates: &[CdpWindowCandidate],
    tolerance: f64,
) -> BindingOutcome {
    let candidates = window_representatives(&native.title, candidates);
    let bounds_matches: Vec<&CdpWindowCandidate> = if native.geometry_exact {
        candidates
            .iter()
            .filter(|c| {
                c.bounds
                    .is_some_and(|bounds| bounds.approx_eq(&native.bounds, tolerance))
            })
            .collect()
    } else {
        Vec::new()
    };

    match bounds_matches.len() {
        1 if bounds_matches[0].cdp_window_id.is_some() => BindingOutcome::Bound {
            candidate: bounds_matches[0].clone(),
            quality: BindingQuality::Exact,
        },
        1 => BindingOutcome::None,
        0 => {
            // No geometric evidence — title-only fallback is heuristic
            // and therefore read-only downstream.
            let title_hits: Vec<&CdpWindowCandidate> = candidates
                .iter()
                .filter(|c| title_matches(&native.title, &c.title))
                .collect();
            match title_hits.len() {
                1 => BindingOutcome::Bound {
                    candidate: title_hits[0].clone(),
                    quality: BindingQuality::Heuristic,
                },
                count if count > 1 => BindingOutcome::Ambiguous(count),
                _ => BindingOutcome::None,
            }
        }
        _ => {
            let title_hits = bounds_matches
                .iter()
                .filter(|candidate| title_matches(&native.title, &candidate.title))
                .collect::<Vec<_>>();
            match title_hits.as_slice() {
                [candidate] if candidate.cdp_window_id.is_some() => BindingOutcome::Bound {
                    candidate: (**candidate).clone(),
                    quality: BindingQuality::Exact,
                },
                _ => BindingOutcome::Ambiguous(bounds_matches.len()),
            }
        }
    }
}

/// Resolve an otherwise ambiguous window using a complete native-window
/// snapshot. Unsupported platforms retain the original unique-or-refuse result.
pub fn correlate_with_native_windows(
    native: &NativeWindowInfo,
    candidates: &[CdpWindowCandidate],
    windows: &[NativeWindowInfo],
    tolerance: f64,
) -> BindingOutcome {
    let original = correlate(native, candidates, tolerance);
    if !matches!(original, BindingOutcome::Ambiguous(_)) {
        return original;
    }
    // Bound the matching work; missing or excessive evidence cannot improve
    // a refusal. A native surface and a CDP window must form a bijection.
    if windows.is_empty() || windows.len() > 32 || !native.geometry_exact {
        return original;
    }
    let mut window_ids = Vec::new();
    let mut geometries = std::collections::HashMap::new();
    for candidate in candidates {
        let Some(id) = candidate.cdp_window_id else {
            return original;
        };
        let Some(bounds) = candidate.bounds else {
            return original;
        };
        if geometries
            .insert(id, bounds)
            .is_some_and(|previous| previous != bounds)
        {
            // The window moved during collection; mixing tab geometries can
            // manufacture a unique mapping that no single snapshot proves.
            return original;
        }
        if !window_ids.contains(&id) {
            window_ids.push(id);
        }
    }
    if window_ids.len() != windows.len() {
        return original;
    }
    let mut native_ids = std::collections::HashSet::new();
    if windows.iter().any(|window| {
        window.pid != native.pid
            || window.ownership.owner_pid != native.pid
            || !window.geometry_exact
            || !native_ids.insert(window.window_id)
    }) {
        return original;
    }
    let Some(requested) = windows.iter().position(|window| {
        window.window_id == native.window_id
            && window.title == native.title
            && window.bounds == native.bounds
    }) else {
        return original;
    };
    let edges = windows
        .iter()
        .map(|window| {
            window_ids
                .iter()
                .enumerate()
                .filter_map(|(index, id)| {
                    candidates
                        .iter()
                        .any(|candidate| {
                            candidate.cdp_window_id == Some(*id)
                                && candidate.bounds.is_some_and(|bounds| {
                                    bounds.approx_eq(&window.bounds, tolerance)
                                })
                                && title_matches(&window.title, &candidate.title)
                        })
                        .then_some(index)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let Some(matching) = window_matching(&edges, None) else {
        return original;
    };
    let selected = matching[requested];
    // If a complete mapping still exists without this edge, the requested
    // window has another possible owner. Do not let target order decide.
    if window_matching(&edges, Some((requested, selected))).is_some() {
        return original;
    }
    let representative = window_representatives(&native.title, candidates)
        .into_iter()
        .find(|candidate| candidate.cdp_window_id == Some(window_ids[selected]));
    match representative {
        Some(candidate) => BindingOutcome::Bound {
            candidate,
            quality: BindingQuality::Exact,
        },
        None => original,
    }
}

/// A complete bipartite matching, optionally excluding one native/CDP edge.
/// Augmenting paths avoid enumerating the permutations of same-sized windows.
fn window_matching(edges: &[Vec<usize>], excluded: Option<(usize, usize)>) -> Option<Vec<usize>> {
    fn assign(
        left: usize,
        edges: &[Vec<usize>],
        excluded: Option<(usize, usize)>,
        seen: &mut [bool],
        owners: &mut [Option<usize>],
    ) -> bool {
        for &right in &edges[left] {
            if excluded == Some((left, right)) || seen[right] {
                continue;
            }
            seen[right] = true;
            if owners[right].is_none()
                || assign(owners[right].unwrap(), edges, excluded, seen, owners)
            {
                owners[right] = Some(left);
                return true;
            }
        }
        false
    }
    let mut owners = vec![None; edges.len()];
    for left in 0..edges.len() {
        if !assign(
            left,
            edges,
            excluded,
            &mut vec![false; edges.len()],
            &mut owners,
        ) {
            return None;
        }
    }
    let mut matching = vec![0; edges.len()];
    for (right, left) in owners.into_iter().enumerate() {
        matching[left?] = right;
    }
    Some(matching)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::{NativeOwnershipMethod, NativeOwnershipProof};

    fn native(title: &str, bounds: Rect) -> NativeWindowInfo {
        NativeWindowInfo {
            pid: 42,
            window_id: 7,
            title: title.into(),
            bounds,
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::WindowServerOwner,
                owner_pid: 42,
                detail: None,
            },
        }
    }

    fn cand(id: &str, title: &str, bounds: Rect) -> CdpWindowCandidate {
        CdpWindowCandidate {
            cdp_target_id: id.into(),
            cdp_window_id: Some(1),
            title: title.into(),
            url: format!("https://example.test/{id}"),
            bounds: Some(bounds),
        }
    }

    fn embedded_cand(id: &str, title: &str) -> CdpWindowCandidate {
        CdpWindowCandidate {
            cdp_target_id: id.into(),
            cdp_window_id: None,
            title: title.into(),
            url: format!("file:///fixture/{id}"),
            bounds: None,
        }
    }

    const B: Rect = Rect {
        x: 100.0,
        y: 50.0,
        width: 1200.0,
        height: 800.0,
    };
    const OTHER: Rect = Rect {
        x: 900.0,
        y: 400.0,
        width: 640.0,
        height: 480.0,
    };

    #[test]
    fn unique_bounds_match_is_exact() {
        let n = native("Docs - Chrome", B);
        let cands = [cand("t1", "Docs", B), cand("t2", "Mail", OTHER)];
        match correlate(&n, &cands, 4.0) {
            BindingOutcome::Bound { candidate, quality } => {
                assert_eq!(candidate.cdp_target_id, "t1");
                assert_eq!(quality, BindingQuality::Exact);
            }
            other => panic!("expected exact bound, got {other:?}"),
        }
    }

    #[test]
    fn bounds_within_tolerance_still_match() {
        let n = native("Docs - Chrome", B);
        let shifted = Rect {
            x: B.x + 3.0,
            y: B.y - 3.0,
            ..B
        };
        let cands = [cand("t1", "Docs", shifted)];
        assert!(matches!(
            correlate(&n, &cands, 4.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Exact,
                ..
            }
        ));
        // Same shift with a tighter tolerance → no geometric evidence,
        // and the title still rescues it as heuristic only.
        assert!(matches!(
            correlate(&n, &cands, 2.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Heuristic,
                ..
            }
        ));
    }

    #[test]
    fn outer_window_rect_restores_exact_match_when_visible_frame_is_inset() {
        let outer = B;
        let visible_frame = Rect {
            x: B.x + 8.0,
            y: B.y,
            width: B.width - 16.0,
            height: B.height - 8.0,
        };
        let candidate = cand("t1", "Docs", outer);

        assert!(matches!(
            correlate(
                &native("Docs - Chrome", visible_frame),
                std::slice::from_ref(&candidate),
                8.0
            ),
            BindingOutcome::Bound {
                quality: BindingQuality::Heuristic,
                ..
            }
        ));
        assert!(matches!(
            correlate(&native("Docs - Chrome", outer), &[candidate], 8.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Exact,
                ..
            }
        ));
    }

    #[test]
    fn multiple_tabs_in_one_window_are_one_exact_candidate() {
        // Multiple page targets are tabs, not competing native windows.
        let n = native("Checkout — Shop - Chrome", B);
        let cands = [cand("t1", "Checkout — Shop", B), cand("t2", "Cart", B)];
        match correlate(&n, &cands, 4.0) {
            BindingOutcome::Bound { candidate, quality } => {
                assert_eq!(candidate.cdp_target_id, "t1");
                assert_eq!(quality, BindingQuality::Exact);
            }
            other => panic!("expected tie-broken exact bound, got {other:?}"),
        }
    }

    #[test]
    fn same_title_tabs_in_one_window_are_still_one_exact_candidate() {
        let n = native("Checkout - Chrome", B);
        let cands = [cand("t1", "Checkout", B), cand("t2", "Checkout", B)];
        assert!(matches!(
            correlate(&n, &cands, 4.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Exact,
                ..
            }
        ));
    }

    #[test]
    fn selected_tab_requires_one_unique_native_title_match() {
        let tabs = [cand("first", "Checkout", B), cand("second", "Cart", B)];
        assert_eq!(
            selected_tab_target_id("Cart - Chrome", &tabs, Some(1)),
            Some("second")
        );

        let duplicate_titles = [cand("first", "Checkout", B), cand("second", "Checkout", B)];
        assert_eq!(
            selected_tab_target_id("Checkout - Chrome", &duplicate_titles, Some(1)),
            None
        );

        let substring_collision = [cand("first", "Mail", B), cand("second", "Mail Inbox", B)];
        assert_eq!(
            selected_tab_target_id("Mail Inbox - Chrome", &substring_collision, Some(1)),
            None
        );

        let empty_titles = [cand("first", "", B), cand("second", "", B)];
        assert_eq!(
            selected_tab_target_id("Chrome", &empty_titles, Some(1)),
            None
        );
    }

    #[test]
    fn embedded_single_page_is_selected_by_cardinality_only() {
        let one = [embedded_cand("only", "Fixture")];
        assert_eq!(selected_tab_target_id("Fixture", &one, None), Some("only"));

        let two = [
            embedded_cand("first", "Fixture"),
            embedded_cand("second", "Fixture"),
        ];
        assert_eq!(selected_tab_target_id("Fixture", &two, None), None);
    }

    #[test]
    fn unique_title_disambiguates_distinct_maximized_windows_with_shared_bounds() {
        let n = native("Docs - Chrome", B);
        let mut other_window = cand("t2", "Docs", B);
        other_window.cdp_window_id = Some(2);
        let cands = [cand("t1", "Mail", B), other_window];
        match correlate(&n, &cands, 4.0) {
            BindingOutcome::Bound { candidate, quality } => {
                assert_eq!(candidate.cdp_target_id, "t2");
                assert_eq!(quality, BindingQuality::Exact);
            }
            other => panic!("expected title-disambiguated exact bound, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_titles_keep_distinct_maximized_windows_ambiguous() {
        let n = native("Docs - Chrome", B);
        let mut other_window = cand("t2", "Docs", B);
        other_window.cdp_window_id = Some(2);
        assert_eq!(
            correlate(&n, &[cand("t1", "Docs", B), other_window], 4.0),
            BindingOutcome::Ambiguous(2)
        );
    }

    #[test]
    fn geometry_without_a_cdp_window_id_is_not_exact() {
        let n = native("Docs - Chrome", B);
        let mut candidate = cand("t1", "Docs", B);
        candidate.cdp_window_id = None;
        assert_eq!(correlate(&n, &[candidate], 4.0), BindingOutcome::None);
    }

    #[test]
    fn no_bounds_match_with_unique_title_is_heuristic() {
        let n = native("Docs - Chrome", B);
        let cands = [cand("t1", "Docs", OTHER), cand("t2", "Mail", OTHER)];
        assert!(matches!(
            correlate(&n, &cands, 4.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Heuristic,
                ..
            }
        ));
    }

    #[test]
    fn singleton_native_and_cdp_windows_are_exact_by_cardinality() {
        let candidates = [cand("t1", "Docs", B), cand("t2", "Mail", B)];
        let selected = cardinality_exact_candidate("Docs - Chrome", &candidates, Some(true))
            .expect("one native window and one CDP window must correlate");
        assert_eq!(selected.cdp_window_id, Some(1));
        assert_eq!(selected.cdp_target_id, "t1");
        assert!(cardinality_exact_candidate("Docs - Chrome", &candidates, Some(false)).is_none());
    }

    #[test]
    fn cardinality_never_picks_between_multiple_cdp_windows() {
        let mut second = cand("t2", "Docs", B);
        second.cdp_window_id = Some(2);
        assert!(cardinality_exact_candidate(
            "Docs - Chrome",
            &[cand("t1", "Docs", B), second],
            Some(true)
        )
        .is_none());
    }

    #[test]
    fn duplicate_title_only_matches_are_ambiguous() {
        let n = native("Docs - Chrome", B);
        let mut second = cand("t2", "Docs", OTHER);
        second.cdp_window_id = Some(2);
        assert_eq!(
            correlate(&n, &[cand("t1", "Docs", OTHER), second], 4.0),
            BindingOutcome::Ambiguous(2)
        );
    }

    #[test]
    fn unattested_geometry_never_mints_an_exact_binding() {
        let mut n = native("Docs - Chrome", B);
        n.geometry_exact = false;
        let cands = [cand("t1", "Docs", B)];
        assert!(matches!(
            correlate(&n, &cands, 4.0),
            BindingOutcome::Bound {
                quality: BindingQuality::Heuristic,
                ..
            }
        ));
    }

    #[test]
    fn embedded_fallback_requires_both_singleton_proofs() {
        let one = [embedded_cand("t1", "Fixture")];
        assert_eq!(
            embedded_single_page_candidate(&one, Some(true))
                .map(|candidate| candidate.cdp_target_id),
            Some("t1".to_owned())
        );
        assert!(embedded_single_page_candidate(&one, Some(false)).is_none());
        assert!(embedded_single_page_candidate(&one, None).is_none());
        let two = [embedded_cand("t1", "Fixture"), embedded_cand("t2", "Other")];
        assert!(embedded_single_page_candidate(&two, Some(true)).is_none());
        assert!(embedded_single_page_candidate(&[cand("t1", "Fixture", B)], Some(true)).is_none());
    }

    #[test]
    fn no_evidence_at_all_is_none() {
        let n = native("Spreadsheet - Chrome", B);
        let cands = [cand("t1", "Docs", OTHER), cand("t2", "Mail", OTHER)];
        assert_eq!(correlate(&n, &cands, 4.0), BindingOutcome::None);
        assert_eq!(correlate(&n, &[], 4.0), BindingOutcome::None);
    }

    #[test]
    fn empty_candidate_titles_never_title_match() {
        // An about:blank tab with an empty title must not win a
        // containment tie-break (every string contains "").
        let n = native("Docs - Chrome", B);
        let cands = [cand("t1", "", B), cand("t2", "Docs", B)];
        match correlate(&n, &cands, 4.0) {
            BindingOutcome::Bound { candidate, quality } => {
                assert_eq!(candidate.cdp_target_id, "t2");
                assert_eq!(quality, BindingQuality::Exact);
            }
            other => panic!("expected t2 exact, got {other:?}"),
        }
    }
    fn shared_bounds_fixture() -> (
        NativeWindowInfo,
        Vec<NativeWindowInfo>,
        Vec<CdpWindowCandidate>,
    ) {
        let a = native("Page X - Chrome", B);
        let mut b = native("Other page - Chrome", B);
        b.window_id = 8;
        let mut b_selected = cand("b-selected", "Other page", B);
        b_selected.cdp_window_id = Some(2);
        let mut b_background = cand("b-background", "Page X", B);
        b_background.cdp_window_id = Some(2);
        (
            a.clone(),
            vec![a, b],
            vec![cand("a-selected", "Page X", B), b_selected, b_background],
        )
    }

    #[test]
    fn peer_native_titles_disambiguate_a_duplicate_background_tab() {
        let (a, windows, candidates) = shared_bounds_fixture();
        assert_eq!(
            correlate(&a, &candidates, 4.0),
            BindingOutcome::Ambiguous(2)
        );
        for candidates in [candidates.clone(), candidates.into_iter().rev().collect()] {
            let result = correlate_with_native_windows(&a, &candidates, &windows, 4.0);
            assert!(
                matches!(result, BindingOutcome::Bound { candidate, quality: BindingQuality::Exact }
                if candidate.cdp_window_id == Some(1) && candidate.cdp_target_id == "a-selected")
            );
        }
    }

    #[test]
    fn peer_native_titles_do_not_resolve_true_ambiguity() {
        let (a, mut windows, candidates) = shared_bounds_fixture();
        windows[1].title = a.title.clone();
        assert_eq!(
            correlate_with_native_windows(&a, &candidates, &windows, 4.0),
            BindingOutcome::Ambiguous(2)
        );
    }

    #[test]
    fn peer_proof_requires_complete_current_owned_geometry() {
        let (a, windows, candidates) = shared_bounds_fixture();
        let assert_ambiguous = |windows: &[NativeWindowInfo]| {
            assert_eq!(
                correlate_with_native_windows(&a, &candidates, windows, 4.0),
                BindingOutcome::Ambiguous(2)
            );
        };
        assert_ambiguous(&windows[..1]);
        for mutate in [0, 1, 2, 3, 4, 5] {
            let mut invalid = windows.clone();
            match mutate {
                0 => invalid[1].ownership.owner_pid = 99,
                1 => invalid[1].pid = 99,
                2 => invalid[1].geometry_exact = false,
                3 => invalid[0].title = "changed while enumerating".into(),
                4 => invalid[1].window_id = a.window_id,
                _ => invalid[1].bounds = OTHER,
            }
            assert_ambiguous(&invalid);
        }
    }

    #[test]
    fn peer_proof_does_not_change_an_existing_exact_binding() {
        let a = native("Page X - Chrome", B);
        let candidates = vec![cand("a-selected", "Page X", B)];
        assert_eq!(
            correlate_with_native_windows(&a, &candidates, &[], 4.0),
            correlate(&a, &candidates, 4.0)
        );
    }
    #[test]
    fn requested_window_can_be_forced_while_peers_remain_ambiguous() {
        let (a, mut windows, mut candidates) = shared_bounds_fixture();
        let mut third = windows[1].clone();
        third.window_id = 9;
        windows.push(third);
        let mut third_tab = cand("third", "Other page", B);
        third_tab.cdp_window_id = Some(3);
        candidates.push(third_tab);
        assert!(
            matches!(correlate_with_native_windows(&a, &candidates, &windows, 4.0),
            BindingOutcome::Bound { candidate, quality: BindingQuality::Exact } if candidate.cdp_window_id == Some(1))
        );
    }

    #[test]
    fn matching_agrees_with_every_three_by_three_assignment() {
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for bits in 0..512 {
            let edges = (0..3)
                .map(|left| {
                    (0..3)
                        .filter(|right| bits & (1 << (left * 3 + right)) != 0)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            for excluded in [None, Some((0, 0)), Some((0, 1)), Some((0, 2))] {
                let expected = permutations.iter().any(|assignment| {
                    (0..3).all(|left| {
                        edges[left].contains(&assignment[left])
                            && excluded != Some((left, assignment[left]))
                    })
                });
                assert_eq!(
                    window_matching(&edges, excluded).is_some(),
                    expected,
                    "bits={bits}, excluded={excluded:?}"
                );
            }
        }
    }
    #[test]
    fn peer_proof_refuses_missing_or_inconsistent_cdp_geometry() {
        let (a, windows, candidates) = shared_bounds_fixture();
        for bounds in [None, Some(OTHER)] {
            let mut inconsistent = candidates.clone();
            inconsistent[2].bounds = bounds;
            assert_eq!(
                correlate_with_native_windows(&a, &inconsistent, &windows, 4.0),
                correlate(&a, &inconsistent, 4.0)
            );
        }
    }
}
