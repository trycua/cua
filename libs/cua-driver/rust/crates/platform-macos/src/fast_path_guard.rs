// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS side of the fast path (CUA-3102): record fingerprints and the
//! window image at read time, and read the live state at act time for the
//! shared checks in `cua_driver_core::fast_path`.
//!
//! With `fast_path` off (the default) every entry point returns at once and
//! nothing is recorded.

use crate::ax::bindings::{
    copy_children, copy_element_attr, copy_string_attr, element_screen_rect,
    kAXErrorInvalidUIElement, AXUIElementCopyAttributeValue, AXUIElementCopyElementAtPosition,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};
use crate::ax::tree::AXNode;
use core_foundation::base::{CFEqual, CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;
use cua_driver_core::fast_path::{
    self as fp,
    classify::{classify, HitInfo, TargetClass, TreeStats},
    fingerprint::{verify_element, Delivery, ElementFingerprint, ElementObservation, HitOutcome},
    pixel::GrayImage,
    pixel_guard::{self, CropSource, PixelGuardConfig},
    reason::{Reason, Verdict},
    store::{global, ReadImage, SnapshotFingerprints},
    window::{verify_window, PointerDelivery, WindowFingerprint, WindowObservation},
    wire, Rect,
};
use cua_driver_core::protocol::ToolResult;
use serde_json::Value;
use std::time::Instant;

/// Per-element AX messaging timeout for the act-time hit-test, so a hung
/// app cannot stall the check for the 6 s default.
const CHECK_TIMEOUT_S: f32 = 0.25;
const MAX_ANCESTORS: usize = 5;

fn label_of(title: Option<&str>, description: Option<&str>) -> Option<String> {
    let t = title.map(str::trim).filter(|s| !s.is_empty());
    let d = description.map(str::trim).filter(|s| !s.is_empty());
    t.or(d).map(str::to_owned)
}

// ── Read time ────────────────────────────────────────────────────────────

/// Fingerprints for the actionable rows of a published snapshot, in token
/// index order (the same filter `AxSnapshot::from_nodes` uses).
pub fn record_snapshot(pid: i32, window_id: u64, snapshot_id: u32, nodes: &[AXNode]) {
    if !fp::mode().checks() {
        return;
    }
    let by_index: std::collections::HashMap<usize, &AXNode> = nodes
        .iter()
        .filter_map(|n| n.element_index.map(|i| (i, n)))
        .collect();
    let elements: Vec<ElementFingerprint> = nodes
        .iter()
        .filter(|n| n.element_index.is_some())
        .map(|n| ElementFingerprint {
            role: n.role.clone(),
            subrole: None,
            label: label_of(n.title.as_deref(), n.description.as_deref()),
            identifier: n.identifier.clone().filter(|s| !s.is_empty()),
            frame: n.frame.map(Rect::from_array),
            parent_index: n.parent_element_index,
            parent_label: n
                .parent_element_index
                .and_then(|p| by_index.get(&p))
                .and_then(|p| label_of(p.title.as_deref(), p.description.as_deref())),
        })
        .collect();
    let thin = nodes
        .iter()
        .filter(|n| n.frame.is_some_and(|f| f[3] <= 1.0))
        .count();
    global().put_snapshot(
        snapshot_id,
        SnapshotFingerprints {
            pid,
            window_id,
            elements,
            element_count: nodes.len(),
            thin_frames: thin,
        },
    );
}

/// Keep the read-time window image (greyscale, in memory) and the window
/// fingerprint for the pixel and window guards.
pub fn record_read_image(
    pid: i32,
    window_id: u32,
    raw: &cua_driver_core::image_utils::CapturedImage,
    bounds: &crate::windows::WindowBounds,
) {
    if !fp::mode().checks() {
        return;
    }
    let Some(image) = wire::gray_from_captured(raw) else {
        return;
    };
    let scale = if bounds.width > 0.0 {
        image.width as f64 / bounds.width
    } else {
        1.0
    };
    global().put_image(
        pid,
        ReadImage {
            snapshot_id: 0,
            image,
            window: WindowFingerprint {
                window_id: u64::from(window_id),
                pid,
                frame: Rect::new(bounds.x, bounds.y, bounds.width, bounds.height),
                scale,
            },
        },
    );
}

/// Record only the window fingerprint (tree-only reads).
pub fn record_window(pid: i32, window_id: u32) {
    if !fp::mode().checks() {
        return;
    }
    if let Some(info) = crate::windows::window_info_by_id(window_id) {
        let scale = global()
            .window(pid, u64::from(window_id))
            .map(|w| w.scale)
            .unwrap_or(0.0);
        let b = info.bounds;
        global().put_window(
            pid,
            WindowFingerprint {
                window_id: u64::from(window_id),
                pid,
                frame: Rect::new(b.x, b.y, b.width, b.height),
                scale,
            },
        );
    }
}

// ── Act time: live reads ─────────────────────────────────────────────────

/// Read role, label, identifier and frame from the live element.
///
/// # Safety
/// `element` must be a retained, live-or-dead `AXUIElementRef`.
unsafe fn observe(element: AXUIElementRef) -> ElementObservation {
    let attr = CFString::new("AXRole");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err == kAXErrorInvalidUIElement {
        return ElementObservation::default();
    }
    if !value.is_null() {
        CFRelease(value);
    }
    let role = copy_string_attr(element, "AXRole");
    let title = copy_string_attr(element, "AXTitle");
    let description = copy_string_attr(element, "AXDescription");
    ElementObservation {
        valid: true,
        role,
        subrole: None,
        label: label_of(title.as_deref(), description.as_deref()),
        identifier: copy_string_attr(element, "AXIdentifier").filter(|s| !s.is_empty()),
        frame: element_screen_rect(element).map(Rect::from_array),
        parent_label: None,
    }
}

/// Hit-test `pid` at a screen point with a short timeout. Retained result.
unsafe fn hit_test(pid: i32, x: f64, y: f64) -> Option<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return None;
    }
    AXUIElementSetMessagingTimeout(app, CHECK_TIMEOUT_S);
    let mut element = std::ptr::null_mut();
    let err = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut element);
    CFRelease(app as CFTypeRef);
    (err == 0 && !element.is_null()).then_some(element)
}

/// Whether `hit` is `target` or a descendant within a few ancestors.
unsafe fn is_self_or_ancestor(hit: AXUIElementRef, target: AXUIElementRef) -> bool {
    let mut current = hit;
    let mut owned: Vec<AXUIElementRef> = Vec::new();
    let mut found = false;
    for _ in 0..=MAX_ANCESTORS {
        if CFEqual(current as CFTypeRef, target as CFTypeRef) != 0 {
            found = true;
            break;
        }
        match copy_element_attr(current, "AXParent") {
            Some(parent) => {
                owned.push(parent);
                current = parent;
            }
            None => break,
        }
    }
    for p in owned {
        CFRelease(p as CFTypeRef);
    }
    found
}

/// Whether `hit` or one of its ancestors matches the fingerprint's role and
/// label (pixel clicks carry no reference, only what was seen there).
unsafe fn matches_fingerprint(hit: AXUIElementRef, want: &ElementFingerprint) -> bool {
    let mut current = hit;
    let mut owned: Vec<AXUIElementRef> = Vec::new();
    let mut found = false;
    for _ in 0..=MAX_ANCESTORS {
        let now = observe(current);
        if now.role.as_deref() == Some(want.role.as_str()) && now.label == want.label {
            found = true;
            break;
        }
        match copy_element_attr(current, "AXParent") {
            Some(parent) => {
                owned.push(parent);
                current = parent;
            }
            None => break,
        }
    }
    for p in owned {
        CFRelease(p as CFTypeRef);
    }
    found
}

unsafe fn hit_info(hit: AXUIElementRef) -> HitInfo {
    let o = observe(hit);
    let children = copy_children(hit);
    let child_count = children.len();
    for c in children {
        CFRelease(c as CFTypeRef);
    }
    HitInfo {
        is_window: o.role.as_deref() == Some("AXWindow"),
        role: o.role,
        has_label: o.label.is_some(),
        has_value: copy_string_attr(hit, "AXValue").is_some_and(|v| !v.trim().is_empty()),
        has_identifier: o.identifier.is_some(),
        frame: o.frame,
        child_count: Some(child_count),
    }
}

fn window_observation(
    pid: i32,
    window_id: u32,
    screen: (f64, f64),
    want: &WindowFingerprint,
) -> WindowObservation {
    let windows = crate::windows::all_windows();
    let Some(info) = windows.iter().find(|w| w.window_id == window_id) else {
        return WindowObservation {
            exists: false,
            pid,
            frame: want.frame,
            scale: want.scale,
            visible: false,
            pid_at_point: None,
            covered_at_point: None,
        };
    };
    let b = &info.bounds;
    // `z_index` grows toward the front (the frontmost window has the highest
    // value), so the window at the point is the one with the largest z_index.
    let front = windows
        .iter()
        .filter(|w| w.layer == 0 && w.is_on_screen)
        .filter(|w| {
            Rect::new(w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height)
                .contains(screen.0, screen.1)
        })
        .max_by_key(|w| w.z_index);
    WindowObservation {
        exists: true,
        pid: info.pid,
        frame: Rect::new(b.x, b.y, b.width, b.height),
        // A scale change shows as a size change in pixels; points are enough.
        scale: want.scale,
        visible: info.is_on_screen && info.on_current_space != Some(false),
        pid_at_point: front.map(|w| w.pid),
        covered_at_point: front.map(|w| w.window_id != window_id),
    }
}

/// Act-time crops from the window itself (never the screen).
struct WindowCrops {
    window_id: u32,
    /// Expected image size; a capture of another size means the window
    /// changed under us.
    size: (usize, usize),
}

impl CropSource for WindowCrops {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        let raw = crate::capture::screenshot_window_image(self.window_id).ok()?;
        let full = wire::gray_from_captured(&raw)?;
        if (full.width, full.height) != self.size {
            return None;
        }
        full.crop(x, y, w, h)
    }
    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

// ── Act time: guards ─────────────────────────────────────────────────────

/// What the caller does after a guard.
pub enum Outcome {
    /// Act. `summary` goes into the result under `fast_path`; `relocate` is
    /// a point offset in screen points for a proven pure move.
    Proceed {
        summary: Option<Value>,
        relocate: Option<(f64, f64)>,
    },
    Refuse(ToolResult),
}

#[allow(clippy::too_many_arguments)]
fn finish(
    tool: &str,
    verdict: Verdict,
    class: Option<TargetClass>,
    verified: &str,
    pid: i32,
    window_id: Option<u64>,
    started: Instant,
    relocate_scale: f64,
) -> Outcome {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let enforce = fp::mode().enforces();
    tracing::debug!(
        target: "cua_driver::fast_path",
        tool, verified, check_ms = ms, verdict = ?verdict, "fast-path check"
    );
    match &verdict {
        Verdict::Refuse { reason, detail } if enforce => Outcome::Refuse(wire::refusal(
            tool, *reason, detail, class, pid, window_id, ms,
        )),
        Verdict::Relocated { dx, dy } if enforce => Outcome::Proceed {
            summary: Some(wire::check_summary(&verdict, class, verified, ms)),
            relocate: Some((dx / relocate_scale, dy / relocate_scale)),
        },
        _ => Outcome::Proceed {
            summary: Some(wire::check_summary(&verdict, class, verified, ms)),
            relocate: None,
        },
    }
}

/// The token path: check the cached element against its fingerprint.
/// `pixel` is true when the action is delivered as a pointer event at the
/// element's centre (button=middle) rather than an AX action.
pub async fn guard_element(
    tool: &str,
    pid: i32,
    window_id: u32,
    args: &Value,
    element_ptr: usize,
    pixel: bool,
) -> Outcome {
    if !fp::mode().checks() {
        return Outcome::Proceed {
            summary: None,
            relocate: None,
        };
    }
    let started = Instant::now();
    let token = args
        .get("element_token")
        .and_then(Value::as_str)
        .and_then(cua_driver_core::element_token::parse_token);
    let Some((snapshot_id, index)) = token else {
        return Outcome::Proceed {
            summary: None,
            relocate: None,
        };
    };
    let Some((_, _, want)) = global().element(snapshot_id, index) else {
        // Read before the flag was on: nothing to compare.
        return Outcome::Proceed {
            summary: Some(wire::check_summary(&Verdict::Ok, None, "none", 0.0)),
            relocate: None,
        };
    };
    let tool = tool.to_owned();
    let verdict = tokio::task::spawn_blocking(move || unsafe {
        let element = element_ptr as AXUIElementRef;
        let now = observe(element);
        let delivery = if pixel {
            Delivery::Pixel
        } else {
            Delivery::Semantic
        };
        let hit = if pixel {
            now.frame.map(|f| {
                let (cx, cy) = f.center();
                match hit_test(pid, cx, cy) {
                    Some(h) => {
                        let out = if is_self_or_ancestor(h, element) {
                            HitOutcome::Target
                        } else {
                            HitOutcome::Other
                        };
                        CFRelease(h as CFTypeRef);
                        out
                    }
                    None => HitOutcome::Nothing,
                }
            })
        } else {
            None
        };
        let mut verdict = verify_element(&want, &now, delivery, hit);
        // Pixel delivery recomputes the point from the live frame anyway, so
        // a proven pure move is fine once the new point hits the element.
        if let Verdict::Relocated { .. } = verdict {
            if hit == Some(HitOutcome::Target) {
                verdict = Verdict::Ok;
            } else {
                verdict = Verdict::refuse(
                    Reason::Moved,
                    "the element moved and is covered at its new spot",
                );
            }
        }
        verdict
    })
    .await
    .unwrap_or(Verdict::Ok);
    finish(
        &tool,
        verdict,
        Some(TargetClass::Rich),
        "ax",
        pid,
        Some(u64::from(window_id)),
        started,
        1.0,
    )
}

/// The pixel path: window guards for every click, then the AX check where
/// the tree saw an element at the point, or the pixel check where it did
/// not (sparse, no tree, remote surfaces).
pub async fn guard_pixel(
    tool: &str,
    pid: i32,
    window_id: u32,
    local: (f64, f64),
    screen: (f64, f64),
    foreground: bool,
) -> Outcome {
    if !fp::mode().checks() {
        return Outcome::Proceed {
            summary: None,
            relocate: None,
        };
    }
    let started = Instant::now();
    let wid = u64::from(window_id);
    let tool_name = tool.to_owned();
    let (verdict, class, verified, scale) = tokio::task::spawn_blocking(move || unsafe {
        let store = global();
        let image = store.image(pid, wid);
        let window_fp = image
            .as_ref()
            .map(|i| i.window)
            .or_else(|| store.window(pid, wid));
        let delivery = if foreground {
            PointerDelivery::Screen
        } else {
            PointerDelivery::Background
        };
        if let Some(want) = window_fp.as_ref() {
            let now = window_observation(pid, window_id, screen, want);
            let v = verify_window(want, &now, delivery);
            if !v.is_ok() {
                return (v, None, "window", 1.0);
            }
        }
        let hit = hit_test(pid, screen.0, screen.1);
        let info = hit.map(|h| hit_info(h));
        let window_rect = window_fp
            .map(|w| w.frame)
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        let stats = store
            .latest_stats(pid, wid)
            .map(|(elements, thin_frames)| TreeStats {
                elements,
                thin_frames,
            });
        let class = classify(info.as_ref(), stats.as_ref(), window_rect);
        let seen = store.element_at(pid, wid, screen.0, screen.1);

        // Rich target the agent saw in the tree: the element there now must
        // be the one it saw (or contain the hit).
        if class == TargetClass::Rich {
            if let (Some(want), Some(h)) = (seen.as_ref(), hit) {
                let ok = matches_fingerprint(h, want);
                CFRelease(h as CFTypeRef);
                let v = if ok {
                    Verdict::Ok
                } else {
                    Verdict::refuse(
                        Reason::HitMismatch,
                        format!(
                            "you saw {} {:?} at this point; something else is there now",
                            want.role,
                            want.label.clone().unwrap_or_default()
                        ),
                    )
                };
                return (v, Some(class), "ax", 1.0);
            }
        }
        if let Some(h) = hit {
            CFRelease(h as CFTypeRef);
        }
        // Sparse, remote, or a rich point the tree never listed: pixels.
        let Some(read) = image else {
            return (Verdict::Ok, Some(class), "window_only", 1.0);
        };
        let scale = read.window.scale.max(1.0);
        let point = (local.0 * scale, local.1 * scale);
        let cfg = PixelGuardConfig::for_scale(scale, class == TargetClass::Remote);
        let mut src = WindowCrops {
            window_id,
            size: (read.image.width, read.image.height),
        };
        let check = pixel_guard::check(&read.image, point, &cfg, &mut src);
        (check.verdict, Some(class), "pixels", scale)
    })
    .await
    .unwrap_or((Verdict::Ok, None, "none", 1.0));
    finish(
        &tool_name,
        verdict,
        class,
        verified,
        pid,
        Some(wid),
        started,
        scale,
    )
}

/// Attach a check summary to a result.
pub fn annotate(result: &mut ToolResult, summary: Option<Value>) {
    if let Some(summary) = summary {
        wire::annotate(result, summary);
    }
}

#[cfg(test)]
mod tests {
    use super::label_of;

    #[test]
    fn label_prefers_title_then_description() {
        assert_eq!(label_of(Some(" Save "), Some("x")).as_deref(), Some("Save"));
        assert_eq!(label_of(Some(""), Some("2")).as_deref(), Some("2"));
        assert_eq!(label_of(None, Some("  ")), None);
    }
}
