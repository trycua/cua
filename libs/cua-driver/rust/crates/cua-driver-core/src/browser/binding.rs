//! Native-window ↔ CDP-target correlation (pure, side-effect free).
//!
//! The native entrypoint is `pid + window_id`. CDP exposes page targets
//! (tabs), each mappable to a CDP window via `Browser.getWindowForTarget`
//! + `Browser.getWindowBounds`. Correlation is unique-or-refuse:
//!
//! 1. Filter candidates whose CDP window bounds match the native bounds
//!    within `tolerance` device pixels.
//! 2. Exactly one bounds match → **Exact** binding.
//! 3. Multiple bounds matches (tabs of the same window share bounds) →
//!    tie-break on the title: the native window title normally embeds
//!    the active tab's title. A unique title match → **Exact**;
//!    otherwise → **Ambiguous** (refusal).
//! 4. No bounds match → a unique title match degrades to **Heuristic**
//!    (read-only); otherwise **None**.

use super::types::{BindingQuality, NativeWindowInfo, Rect};

/// One CDP page target with its resolved CDP window geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpWindowCandidate {
    pub cdp_target_id: String,
    pub cdp_window_id: i64,
    pub title: String,
    pub url: String,
    pub bounds: Rect,
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

/// Whether the native window title plausibly displays this tab's title.
/// Browsers render "<tab title> - <product>", so containment (not
/// equality) is the tie-break; empty tab titles never match.
fn title_matches(native_title: &str, candidate_title: &str) -> bool {
    !candidate_title.is_empty() && native_title.contains(candidate_title)
}

/// Correlate `native` against `candidates` with unique-or-refuse
/// semantics. `tolerance` is in device pixels (see [`Rect::approx_eq`]).
pub fn correlate(
    native: &NativeWindowInfo,
    candidates: &[CdpWindowCandidate],
    tolerance: f64,
) -> BindingOutcome {
    let bounds_matches: Vec<&CdpWindowCandidate> = if native.geometry_exact {
        candidates
            .iter()
            .filter(|c| c.bounds.approx_eq(&native.bounds, tolerance))
            .collect()
    } else {
        Vec::new()
    };

    match bounds_matches.len() {
        1 => BindingOutcome::Bound {
            candidate: bounds_matches[0].clone(),
            quality: BindingQuality::Exact,
        },
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
                _ => BindingOutcome::None,
            }
        }
        _ => {
            // Same-window tabs share bounds; the active tab's title is
            // what the native window shows. Unique title hit → exact.
            let title_hits: Vec<&&CdpWindowCandidate> = bounds_matches
                .iter()
                .filter(|c| title_matches(&native.title, &c.title))
                .collect();
            if title_hits.len() == 1 {
                BindingOutcome::Bound {
                    candidate: (*title_hits[0]).clone(),
                    quality: BindingQuality::Exact,
                }
            } else {
                BindingOutcome::Ambiguous(bounds_matches.len())
            }
        }
    }
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
            cdp_window_id: 1,
            title: title.into(),
            url: format!("https://example.test/{id}"),
            bounds,
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
    fn shared_bounds_tie_break_on_title_is_exact() {
        // Two tabs of the same window share bounds; native title shows
        // the active tab.
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
    fn shared_bounds_and_indistinguishable_titles_are_ambiguous() {
        let n = native("Checkout - Chrome", B);
        let cands = [cand("t1", "Checkout", B), cand("t2", "Checkout", B)];
        match correlate(&n, &cands, 4.0) {
            BindingOutcome::Ambiguous(count) => {
                assert_eq!(count, 2);
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
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
}
