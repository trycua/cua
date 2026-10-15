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
//!    matches remain ambiguous.
//! 5. No bounds match → a unique title match degrades to **Heuristic**
//!    (read-only); otherwise **None**.
//!
//! Core upgrades a heuristic or missing match to **Exact** only with a
//! platform-attested cardinality proof ([`cardinality_exact_candidate`]) or
//! a one-to-one scaled-size pairing of every owned window
//! ([`scaled_size_bijection_candidate`]).

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

/// Size of one native top-level window owned by the browser process, as
/// attested by the window system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeWindowSize {
    pub window_id: u64,
    pub width: f64,
    pub height: f64,
}

/// Largest window set the size pairing enumerates (`n!` pairings).
const SCALED_SIZE_MAX_WINDOWS: usize = 6;
/// Plausible browser-unit to window-system scale range.
const SCALED_SIZE_SCALE_RANGE: (f64, f64) = (0.5, 4.0);

/// Pair `native_window_id` with its CDP window by size alone. Chromium on
/// Wayland cannot see its global position (it reports 0,0) and reports
/// window sizes in its own scaled units, so bounds never match the
/// compositor's rect. The pairing is exact when every native window of the
/// process pairs one-to-one with every CDP window under one common scale, and
/// no other one-to-one pairing does. A finite set of sizes maps onto itself
/// under a common scale only by identity, so a second valid pairing exists
/// only for windows of (nearly) the same size; that case refuses.
pub fn scaled_size_bijection_candidate(
    native_title: &str,
    native_window_id: u64,
    native_sizes: &[NativeWindowSize],
    candidates: &[CdpWindowCandidate],
    tolerance: f64,
) -> Option<CdpWindowCandidate> {
    let representatives = window_representatives(native_title, candidates);
    let count = native_sizes.len();
    if count == 0
        || count > SCALED_SIZE_MAX_WINDOWS
        || representatives.len() != count
        || native_sizes
            .iter()
            .filter(|size| size.window_id == native_window_id)
            .count()
            != 1
        || native_sizes
            .iter()
            .any(|size| !(size.width > 0.0 && size.height > 0.0))
    {
        return None;
    }
    let cdp_sizes = representatives
        .iter()
        .map(|candidate| {
            candidate.cdp_window_id?;
            let bounds = candidate.bounds?;
            (bounds.width > 0.0 && bounds.height > 0.0).then_some((bounds.width, bounds.height))
        })
        .collect::<Option<Vec<_>>>()?;

    let native = native_sizes
        .iter()
        .map(|size| (size.width, size.height))
        .collect::<Vec<_>>();
    let pairing = unique_scaled_pairing(&native, &cdp_sizes, tolerance)?;
    let native_index = native_sizes
        .iter()
        .position(|size| size.window_id == native_window_id)?;
    Some(representatives[pairing[native_index]].clone())
}

/// The one-to-one pairing of `native` sizes (window-system units) with
/// `scaled` sizes (an application's own scaled units) under a single common
/// scale, when exactly one such pairing exists. Lists of different lengths,
/// more than [`SCALED_SIZE_MAX_WINDOWS`] entries, non-positive sizes, or two or
/// more valid pairings (equal or near-equal sizes) yield `None`.
/// `pairing[i]` is the index in `scaled` paired with `native[i]`.
pub fn unique_scaled_pairing(
    native: &[(f64, f64)],
    scaled: &[(f64, f64)],
    tolerance: f64,
) -> Option<Vec<usize>> {
    let count = native.len();
    let positive = |sizes: &[(f64, f64)]| sizes.iter().all(|&(w, h)| w > 0.0 && h > 0.0);
    if count == 0
        || count > SCALED_SIZE_MAX_WINDOWS
        || scaled.len() != count
        || !positive(native)
        || !positive(scaled)
    {
        return None;
    }
    let mut pairing: Vec<usize> = (0..count).collect();
    let mut valid: Option<Vec<usize>> = None;
    loop {
        if scaled_pairing_holds(native, scaled, &pairing, tolerance) {
            if valid.is_some() {
                return None;
            }
            valid = Some(pairing.clone());
        }
        if !next_permutation(&mut pairing) {
            return valid;
        }
    }
}

/// Whether `native[i]` pairs with `scaled[pairing[i]]` for every `i` under
/// one common scale, within `tolerance` scaled units per edge.
fn scaled_pairing_holds(
    native: &[(f64, f64)],
    scaled: &[(f64, f64)],
    pairing: &[usize],
    tolerance: f64,
) -> bool {
    let native_total: f64 = native.iter().map(|(w, h)| w + h).sum();
    let scaled_total: f64 = pairing.iter().map(|&i| scaled[i].0 + scaled[i].1).sum();
    let scale = native_total / scaled_total;
    if !(SCALED_SIZE_SCALE_RANGE.0..=SCALED_SIZE_SCALE_RANGE.1).contains(&scale) {
        return false;
    }
    native.iter().zip(pairing).all(|(&(width, height), &i)| {
        (width / scale - scaled[i].0).abs() <= tolerance
            && (height / scale - scaled[i].1).abs() <= tolerance
    })
}

/// Advance `items` to the next lexicographic permutation; false after the last.
fn next_permutation(items: &mut [usize]) -> bool {
    let Some(pivot) = (1..items.len()).rev().find(|&i| items[i - 1] < items[i]) else {
        return false;
    };
    let successor = (pivot..items.len())
        .rev()
        .find(|&i| items[i] > items[pivot - 1])
        .expect("a larger element follows the pivot");
    items.swap(pivot - 1, successor);
    items[pivot..].reverse();
    true
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

    fn sized(id: &str, window: i64, width: f64, height: f64) -> CdpWindowCandidate {
        CdpWindowCandidate {
            cdp_window_id: Some(window),
            ..cand(id, id, Rect::new(0.0, 0.0, width, height))
        }
    }

    fn native_size(window_id: u64, width: f64, height: f64) -> NativeWindowSize {
        NativeWindowSize {
            window_id,
            width,
            height,
        }
    }

    #[test]
    fn scaled_sizes_pair_each_native_window_with_its_cdp_window() {
        // Measured on Hyprland: Chrome reports 0,0 and DIP sizes at a 1.3636
        // GTK scale; a tiled browser window plus a full-screen app window.
        let natives = [
            native_size(7, 1913.0, 1041.0),
            native_size(9, 3832.0, 2087.0),
        ];
        let cands = [
            sized("app", 2, 2810.0, 1531.0),
            sized("main-a", 1, 1403.0, 764.0),
            sized("main-b", 1, 1403.0, 764.0),
        ];
        let main = scaled_size_bijection_candidate("main-b - Chrome", 7, &natives, &cands, 8.0)
            .expect("main window pairs");
        assert_eq!(main.cdp_window_id, Some(1));
        let app = scaled_size_bijection_candidate("app", 9, &natives, &cands, 8.0)
            .expect("app window pairs");
        assert_eq!(app.cdp_window_id, Some(2));
    }

    #[test]
    fn scaled_sizes_refuse_equal_sized_windows() {
        let natives = [
            native_size(7, 1913.0, 1041.0),
            native_size(9, 1913.0, 1041.0),
        ];
        let cands = [sized("a", 1, 1403.0, 764.0), sized("b", 2, 1403.0, 764.0)];
        assert!(scaled_size_bijection_candidate("a", 7, &natives, &cands, 8.0).is_none());
    }

    #[test]
    fn scaled_sizes_refuse_without_a_common_scale() {
        // Each window matches some scale on its own, but not the same one.
        let natives = [
            native_size(7, 1913.0, 1041.0),
            native_size(9, 3832.0, 2087.0),
        ];
        let cands = [sized("a", 1, 1403.0, 764.0), sized("b", 2, 1916.0, 1044.0)];
        assert!(scaled_size_bijection_candidate("a", 7, &natives, &cands, 8.0).is_none());
    }

    #[test]
    fn scaled_sizes_refuse_unequal_window_counts_and_missing_target() {
        let natives = [
            native_size(7, 1913.0, 1041.0),
            native_size(9, 3832.0, 2087.0),
        ];
        let one = [sized("a", 1, 1403.0, 764.0)];
        assert!(scaled_size_bijection_candidate("a", 7, &natives, &one, 8.0).is_none());
        let two = [sized("a", 1, 1403.0, 764.0), sized("b", 2, 2810.0, 1531.0)];
        assert!(scaled_size_bijection_candidate("a", 8, &natives, &two, 8.0).is_none());
        let unbounded = [
            sized("a", 1, 1403.0, 764.0),
            CdpWindowCandidate {
                bounds: None,
                ..sized("b", 2, 2810.0, 1531.0)
            },
        ];
        assert!(scaled_size_bijection_candidate("a", 7, &natives, &unbounded, 8.0).is_none());
    }

    #[test]
    fn next_permutation_visits_every_ordering_once() {
        let mut items = vec![0, 1, 2];
        let mut seen = vec![items.clone()];
        while next_permutation(&mut items) {
            seen.push(items.clone());
        }
        assert_eq!(seen.len(), 6);
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 6);
    }
}
