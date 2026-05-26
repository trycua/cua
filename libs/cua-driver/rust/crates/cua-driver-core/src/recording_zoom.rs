//! Pure-Rust port of the Swift `Recording/Zoom/` module (zoom math,
//! action spans, zoom regions). Cross-platform by construction — no OS
//! deps, just `f64` arithmetic + `Vec`.
//!
//! Reference: `libs/cua-driver/swift/Sources/CuaDriverCore/Recording/Zoom/`
//! Comments cite the Swift source line numbers where the algorithm
//! differs from a textbook implementation, so future maintainers can
//! cross-check.

use serde::{Deserialize, Serialize};

// ── math primitives (ZoomMath.swift) ─────────────────────────────────────────

#[inline]
pub fn clamp01(v: f64) -> f64 {
    if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v }
}

#[inline]
pub fn lerp(a: f64, b: f64, amount: f64) -> f64 {
    a + (b - a) * amount
}

#[inline]
fn sample_bezier_1d(a1: f64, a2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * a1 * u * u * t + 3.0 * a2 * u * t * t + t * t * t
}

#[inline]
fn sample_bezier_1d_derivative(a1: f64, a2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * a1 * u * u + 6.0 * (a2 - a1) * u * t + 3.0 * (1.0 - a2) * t * t
}

/// CSS-style cubic-bezier timing function. Given progress along the x-axis,
/// return the y-value. Newton-Raphson + binary-search hybrid (Foley/van Dam §11).
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let target_x = clamp01(t);
    let mut s = target_x;
    // Newton-Raphson: up to 8 iterations
    for _ in 0..8 {
        let residual = sample_bezier_1d(x1, x2, s) - target_x;
        let slope = sample_bezier_1d_derivative(x1, x2, s);
        if residual.abs() < 1e-6 { break; }
        if slope.abs() < 1e-6 { break; }
        s -= residual / slope;
    }
    // Binary-search cleanup
    s = clamp01(s);
    let mut lower = 0.0;
    let mut upper = 1.0;
    for _ in 0..10 {
        let x = sample_bezier_1d(x1, x2, s);
        if (x - target_x).abs() < 1e-6 { break; }
        if x < target_x { lower = s; } else { upper = s; }
        s = (lower + upper) / 2.0;
    }
    sample_bezier_1d(y1, y2, s)
}

// ── ZoomDefaults (ZoomRegion.swift:21-49) ────────────────────────────────────

pub mod defaults {
    pub const TRANSITION_WINDOW_MS: f64 = 1015.05;
    pub const ZOOM_IN_WINDOW_MS: f64 = 1522.575;
    pub const CHAINED_ZOOM_PAN_GAP_MS: f64 = 1500.0;
    /// Pre-click lead-in (zoom starts this many ms before the click).
    pub const PRE_CLICK_LEAD_IN_MS: f64 = 600.0;
    /// Post-click hold (zoom-out starts this many ms after the click).
    pub const POST_CLICK_HOLD_MS: f64 = 1500.0;
    /// Default zoom scale (2× magnification centered on focus).
    pub const DEFAULT_SCALE: f64 = 2.0;

    // ActionSpan defaults (ActionSpan.swift)
    pub const ACTION_PAD_MS: f64 = 500.0;
    pub const ACTION_FAST_SPEED: f64 = 8.0;
    pub const ACTION_MERGE_GAP_MS: f64 = 5000.0;
}

// ── data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClickEvent {
    pub t_ms: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CursorSample {
    pub t_ms: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FocusWaypoint {
    pub t_ms: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoomRegion {
    pub start_ms: f64,
    pub end_ms: f64,
    pub focus_x: f64,
    pub focus_y: f64,
    pub scale: f64,
    pub zoom_in_duration_ms: f64,
    pub zoom_out_duration_ms: f64,
    pub waypoints: Option<Vec<FocusWaypoint>>,
}

impl ZoomRegion {
    /// Returns the effective zoom-in / zoom-out durations after shrinking
    /// to fit within the region's total length, if necessary.
    pub fn effective_durations(&self) -> (f64, f64) {
        let duration = (self.end_ms - self.start_ms).max(0.0);
        let mut zi = self.zoom_in_duration_ms;
        let mut zo = self.zoom_out_duration_ms;
        let combined = zi + zo;
        if combined > duration && combined > 0.0 {
            let shrink = duration / combined;
            zi *= shrink;
            zo *= shrink;
        }
        (zi, zo)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpan {
    pub start_ms: f64,
    pub end_ms: f64,
    pub window_bounds: Option<WindowBounds>,
    pub click_point: Option<(f64, f64)>,
    pub focus_waypoints: Option<Vec<FocusWaypoint>>,
}

// ── ZoomRegion generation ────────────────────────────────────────────────────

pub fn generate_zoom_regions(
    clicks: &[ClickEvent],
    default_scale: f64,
) -> Vec<ZoomRegion> {
    let mut sorted: Vec<&ClickEvent> = clicks.iter().collect();
    sorted.sort_by(|a, b| a.t_ms.partial_cmp(&b.t_ms).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<ZoomRegion> = Vec::with_capacity(sorted.len());
    for click in &sorted {
        let seed_start = click.t_ms - defaults::PRE_CLICK_LEAD_IN_MS;
        let seed_end = click.t_ms + defaults::POST_CLICK_HOLD_MS;
        let seed_waypoint = FocusWaypoint { t_ms: click.t_ms, x: click.x, y: click.y };

        // Merge with the previous region if the gap is small enough.
        let should_merge = out.last()
            .map(|last| seed_start - last.end_ms < defaults::CHAINED_ZOOM_PAN_GAP_MS)
            .unwrap_or(false);

        if should_merge {
            let last_idx = out.len() - 1;
            let last = &mut out[last_idx];
            let mut waypoints = last.waypoints.clone().unwrap_or_else(|| {
                vec![FocusWaypoint {
                    t_ms: last.start_ms + defaults::PRE_CLICK_LEAD_IN_MS,
                    x: last.focus_x,
                    y: last.focus_y,
                }]
            });
            waypoints.push(seed_waypoint);
            last.end_ms = seed_end;
            last.zoom_out_duration_ms = defaults::TRANSITION_WINDOW_MS;
            last.scale = last.scale.max(default_scale);
            last.waypoints = Some(waypoints);
        } else {
            out.push(ZoomRegion {
                start_ms: seed_start,
                end_ms: seed_end,
                focus_x: click.x,
                focus_y: click.y,
                scale: default_scale,
                zoom_in_duration_ms: defaults::ZOOM_IN_WINDOW_MS,
                zoom_out_duration_ms: defaults::TRANSITION_WINDOW_MS,
                waypoints: None,
            });
        }
    }
    out
}

/// Sampling the zoom curve at playback time `t_ms`. Returns
/// `(scale, focus_x, focus_y)`. When `t` is outside all regions returns
/// `(1.0, cursor_x, cursor_y)` (resting state — follow cursor at no zoom).
pub fn sample_curve(
    t_ms: f64,
    regions: &[ZoomRegion],
    cursor_samples: &[CursorSample],
) -> (f64, f64, f64) {
    // Default resting state: 1× zoom following the cursor.
    let resting = || -> (f64, f64, f64) {
        if let Some(pos) = position_at(t_ms, cursor_samples) {
            (1.0, pos.0, pos.1)
        } else {
            (1.0, 0.0, 0.0)
        }
    };

    let region = regions.iter()
        .find(|r| t_ms >= r.start_ms && t_ms <= r.end_ms);
    let region = match region { Some(r) => r, None => return resting() };

    let (zoom_in, zoom_out) = region.effective_durations();
    let in_end = region.start_ms + zoom_in;
    let out_start = region.end_ms - zoom_out;

    // Phase progress with cubic-bezier(0.16, 1, 0.3, 1) easing.
    let phase_progress = if t_ms < in_end {
        let span = zoom_in.max(1e-6);
        cubic_bezier(0.16, 1.0, 0.3, 1.0, clamp01((t_ms - region.start_ms) / span))
    } else if t_ms > out_start {
        let span = zoom_out.max(1e-6);
        let tail = clamp01((region.end_ms - t_ms) / span);
        cubic_bezier(0.16, 1.0, 0.3, 1.0, tail)
    } else {
        1.0
    };

    let scale = lerp(1.0, region.scale, phase_progress);
    let (fx, fy) = resolve_focus(t_ms, region);
    (scale, fx, fy)
}

fn resolve_focus(t_ms: f64, region: &ZoomRegion) -> (f64, f64) {
    let waypoints = match region.waypoints.as_ref() {
        Some(w) if !w.is_empty() => w,
        _ => return (region.focus_x, region.focus_y),
    };

    if waypoints.len() == 1 || t_ms <= waypoints[0].t_ms {
        let w = &waypoints[0];
        return (w.x, w.y);
    }
    if let Some(last) = waypoints.last() {
        if t_ms >= last.t_ms {
            return (last.x, last.y);
        }
    }

    // Walk adjacent pairs.
    for i in 0..waypoints.len().saturating_sub(1) {
        let a = &waypoints[i];
        let b = &waypoints[i + 1];
        if t_ms < a.t_ms || t_ms > b.t_ms { continue; }
        let span = (b.t_ms - a.t_ms).max(1e-6);
        let local_t = clamp01((t_ms - a.t_ms) / span);
        // easeConnectedPan: cubic-bezier(0.1, 0, 0.2, 1)
        let eased = cubic_bezier(0.1, 0.0, 0.2, 1.0, local_t);
        return (lerp(a.x, b.x, eased), lerp(a.y, b.y, eased));
    }
    (region.focus_x, region.focus_y)
}

// ── CursorTelemetry (CursorTelemetry.swift) ──────────────────────────────────

/// Binary-search-then-lerp cursor position lookup at `t_ms`. Returns `None`
/// only for an empty slice; out-of-range times clamp to the endpoints.
pub fn position_at(t_ms: f64, samples: &[CursorSample]) -> Option<(f64, f64)> {
    let first = samples.first()?;
    let last = samples.last()?;
    if t_ms <= first.t_ms { return Some((first.x, first.y)); }
    if t_ms >= last.t_ms  { return Some((last.x,  last.y));  }

    let mut lo = 0usize;
    let mut hi = samples.len() - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if samples[mid].t_ms <= t_ms { lo = mid; } else { hi = mid; }
    }
    let before = &samples[lo];
    let after = &samples[hi];
    let span = after.t_ms - before.t_ms;
    let u = if span > 0.0 { (t_ms - before.t_ms) / span } else { 0.0 };
    Some((lerp(before.x, after.x, u), lerp(before.y, after.y, u)))
}

// ── ActionSpan (ActionSpan.swift) ────────────────────────────────────────────

/// Pad raw action spans by ±`ACTION_PAD_MS` and merge overlapping spans.
/// Merged spans accumulate per-action focus waypoints for smooth pan.
pub fn generate_action_spans(raw: &[ActionSpan]) -> Vec<ActionSpan> {
    let mut padded: Vec<ActionSpan> = raw.iter().map(|s| ActionSpan {
        start_ms: (s.start_ms - defaults::ACTION_PAD_MS).max(0.0),
        end_ms: s.end_ms + defaults::ACTION_PAD_MS,
        window_bounds: s.window_bounds,
        click_point: s.click_point,
        focus_waypoints: None,
    }).collect();
    padded.sort_by(|a, b| a.start_ms.partial_cmp(&b.start_ms).unwrap_or(std::cmp::Ordering::Equal));

    let mut result: Vec<ActionSpan> = Vec::new();
    for span in padded {
        let should_merge = result.last()
            .map(|last| span.start_ms <= last.end_ms + defaults::ACTION_MERGE_GAP_MS)
            .unwrap_or(false);

        if should_merge {
            let last_idx = result.len() - 1;
            let last = &mut result[last_idx];
            let last_mid = (last.start_ms + last.end_ms) / 2.0;
            let span_mid = (span.start_ms + span.end_ms) / 2.0;
            let mut waypoints = last.focus_waypoints.clone().unwrap_or_default();
            if waypoints.is_empty() {
                if let Some((lx, ly)) = focus_for_span(last) {
                    waypoints.push(FocusWaypoint { t_ms: last_mid, x: lx, y: ly });
                }
            }
            if let Some((sx, sy)) = focus_for_span(&span) {
                waypoints.push(FocusWaypoint { t_ms: span_mid, x: sx, y: sy });
            }
            last.end_ms = last.end_ms.max(span.end_ms);
            if last.window_bounds.is_none() { last.window_bounds = span.window_bounds; }
            if last.click_point.is_none() { last.click_point = span.click_point; }
            last.focus_waypoints = if waypoints.is_empty() { None } else { Some(waypoints) };
        } else {
            result.push(span);
        }
    }
    result
}

fn focus_for_span(span: &ActionSpan) -> Option<(f64, f64)> {
    if let Some(cp) = span.click_point { return Some(cp); }
    if let Some(wb) = span.window_bounds {
        return Some((wb.x + wb.width / 2.0, wb.y + wb.height / 2.0));
    }
    None
}

/// Piecewise-linear PTS remap: spans play at 1× speed, gaps at
/// `ACTION_FAST_SPEED`×. Monotonically increasing.
pub fn map_pts(input_ms: f64, spans: &[ActionSpan]) -> f64 {
    let mut out_ms = 0.0;
    let mut prev_end = 0.0;
    for span in spans {
        if span.start_ms > prev_end {
            let seg_end = span.start_ms.min(input_ms);
            out_ms += (seg_end - prev_end) / defaults::ACTION_FAST_SPEED;
            if input_ms <= span.start_ms { return out_ms; }
        }
        let start = span.start_ms.max(prev_end);
        let end = span.end_ms.min(input_ms);
        if end > start { out_ms += end - start; }
        if input_ms <= span.end_ms { return out_ms; }
        prev_end = span.end_ms;
    }
    if input_ms > prev_end {
        out_ms += (input_ms - prev_end) / defaults::ACTION_FAST_SPEED;
    }
    out_ms
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_bezier_endpoints_pin_to_0_and_1() {
        assert!((cubic_bezier(0.16, 1.0, 0.3, 1.0, 0.0) - 0.0).abs() < 1e-6);
        assert!((cubic_bezier(0.16, 1.0, 0.3, 1.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cubic_bezier_strong_ease_out_passes_half_early() {
        // (0.16, 1, 0.3, 1) is aggressive ease-out — at t=0.28 the curve
        // should already be past 0.6 (Swift comment: "~80% in first 28%").
        let y = cubic_bezier(0.16, 1.0, 0.3, 1.0, 0.28);
        assert!(y > 0.6, "y at t=0.28 was {y}");
    }

    #[test]
    fn click_becomes_a_region() {
        let regions = generate_zoom_regions(&[
            ClickEvent { t_ms: 5000.0, x: 100.0, y: 200.0 },
        ], 2.0);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert_eq!(r.focus_x, 100.0);
        assert_eq!(r.focus_y, 200.0);
        assert_eq!(r.scale, 2.0);
        assert_eq!(r.start_ms, 5000.0 - defaults::PRE_CLICK_LEAD_IN_MS);
        assert_eq!(r.end_ms, 5000.0 + defaults::POST_CLICK_HOLD_MS);
    }

    #[test]
    fn close_clicks_merge_into_chained_region() {
        let regions = generate_zoom_regions(&[
            ClickEvent { t_ms: 1000.0, x: 100.0, y: 100.0 },
            ClickEvent { t_ms: 2000.0, x: 200.0, y: 200.0 },
        ], 2.0);
        assert_eq!(regions.len(), 1, "close clicks should merge");
        let waypoints = regions[0].waypoints.as_ref().unwrap();
        assert_eq!(waypoints.len(), 2);
    }

    #[test]
    fn far_apart_clicks_dont_merge() {
        let regions = generate_zoom_regions(&[
            ClickEvent { t_ms: 1000.0, x: 100.0, y: 100.0 },
            ClickEvent { t_ms: 10000.0, x: 200.0, y: 200.0 },
        ], 2.0);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn sample_curve_inside_region_zooms() {
        let regions = generate_zoom_regions(&[
            ClickEvent { t_ms: 5000.0, x: 100.0, y: 200.0 },
        ], 2.0);
        // At the click moment, should be near peak zoom.
        let (scale, fx, fy) = sample_curve(5000.0, &regions, &[]);
        assert!(scale > 1.5, "expected near-peak zoom at click time, got {scale}");
        assert!((fx - 100.0).abs() < 0.5);
        assert!((fy - 200.0).abs() < 0.5);
    }

    #[test]
    fn sample_curve_outside_region_is_unity() {
        let regions = generate_zoom_regions(&[
            ClickEvent { t_ms: 5000.0, x: 100.0, y: 200.0 },
        ], 2.0);
        // Far before any zoom region.
        let (scale, _, _) = sample_curve(0.0, &regions, &[]);
        assert!((scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cursor_position_at_clamps_to_endpoints() {
        let samples = vec![
            CursorSample { t_ms: 0.0, x: 0.0, y: 0.0 },
            CursorSample { t_ms: 1000.0, x: 100.0, y: 100.0 },
        ];
        assert_eq!(position_at(-100.0, &samples), Some((0.0, 0.0)));
        assert_eq!(position_at(2000.0, &samples), Some((100.0, 100.0)));
        // Lerp at midpoint
        let mid = position_at(500.0, &samples).unwrap();
        assert!((mid.0 - 50.0).abs() < 1e-6);
        assert!((mid.1 - 50.0).abs() < 1e-6);
    }

    #[test]
    fn map_pts_is_monotonic_and_speeds_outside_spans() {
        let spans = vec![ActionSpan {
            start_ms: 1000.0, end_ms: 2000.0,
            window_bounds: None, click_point: None, focus_waypoints: None,
        }];
        // Before span: fast-forward
        let before = map_pts(500.0, &spans);
        // At span start
        let at_start = map_pts(1000.0, &spans);
        // At span end
        let at_end = map_pts(2000.0, &spans);
        // After span
        let after = map_pts(2500.0, &spans);
        assert!(before < at_start, "pts must be monotonic");
        assert!(at_start < at_end);
        assert!(at_end < after);
        // Inside span advances 1× (500ms → 500ms)
        assert!((at_end - at_start - 1000.0).abs() < 1e-3);
        // Outside span advances at fast_speed (500ms → 62.5ms at 8×)
        assert!((before * defaults::ACTION_FAST_SPEED - 500.0).abs() < 1e-3);
    }
}
