//! Background hover routes that leave the user's real pointer alone.
//!
//! AppKit derives hover (tracking areas, tooltips) from the real pointer.
//! Pointer-move and enter events posted to a background app do reach
//! `-[NSApplication sendEvent:]`, but AppKit never turns them into tracking
//! callbacks (VM probe, 11 Oct 2026). Two routes do work without moving the
//! pointer, focusing or raising anything:
//!
//! * **AX alternate UI.** Many AppKit views model their hover-only controls as
//!   transient UI (`NSAccessibilityContainsTransientUI`) and advertise
//!   `AXShowAlternateUI` / `AXShowDefaultUI`. Sidebar section headers in Finder,
//!   Mail, Music and System Settings reveal their hover-only Show/Hide button
//!   this way.
//! * **Electron input.** `webContents.sendInputEvent` in the app's main process
//!   delivers `mouseEnter`/`mouseMove` straight to the renderer.

use std::sync::Mutex;

use core_foundation::base::{CFEqual, CFRelease, CFTypeRef};

use crate::ax::bindings::{
    ax_get_window_id, copy_action_names, copy_bool_attr, copy_children, copy_element_attr,
    copy_string_attr, kAXErrorSuccess, perform_action, AXUIElementCopyElementAtPosition,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};

/// How far up from the hit element to look for an element with alternate UI.
const ANCESTOR_DEPTH: usize = 6;
const AX_TIMEOUT_S: f32 = 0.25;
const SHOW_ALTERNATE: &str = "AXShowAlternateUI";
const SHOW_DEFAULT: &str = "AXShowDefaultUI";
/// How deep to count the target's subtree when checking for new controls.
const SUBTREE_DEPTH: usize = 4;

/// The element whose alternate UI the last hover revealed: (pid, retained
/// AXUIElementRef as usize). Moving the hover elsewhere restores its default
/// UI, the way the pointer leaving it would.
static REVEALED: Mutex<Option<(i32, usize)>> = Mutex::new(None);

/// What the AX route did.
#[derive(Debug, Clone, PartialEq)]
pub struct AxReveal {
    pub role: String,
    pub label: String,
    /// `AXAlternateUIVisible` read back after the action, when the app exposes it.
    pub visible: Option<bool>,
    /// Elements in the target's subtree before and after the action. More
    /// after means hover-only controls appeared.
    pub elements_before: usize,
    pub elements_after: usize,
}

/// Whether an element's advertised actions let the AX route reveal its
/// hover-only UI.
pub fn advertises_alternate_ui(actions: &[String]) -> bool {
    actions.iter().any(|a| a == SHOW_ALTERNATE)
}

/// Reveal the hover-only UI of the element under a screen point in `pid`'s
/// window `window_id`, through `AXShowAlternateUI` on the hit element or one of
/// its ancestors. Restores the previously revealed element first. Returns
/// `None` when nothing under the point offers alternate UI.
pub fn reveal_alternate_ui(pid: i32, window_id: u32, x: f64, y: f64) -> Option<AxReveal> {
    unsafe {
        let Some(target) = alternate_ui_target(pid, window_id, x, y) else {
            // Hovering where nothing reveals: the last revealed element loses hover.
            restore_default_ui(None);
            return None;
        };
        restore_default_ui(Some(target));
        let elements_before = subtree_size(target, SUBTREE_DEPTH);
        if perform_action(target, SHOW_ALTERNATE) != kAXErrorSuccess {
            CFRelease(target as CFTypeRef);
            return None;
        }
        let reveal = AxReveal {
            role: copy_string_attr(target, "AXRole").unwrap_or_default(),
            label: first_label(target),
            visible: copy_bool_attr(target, "AXAlternateUIVisible"),
            elements_before,
            elements_after: {
                std::thread::sleep(std::time::Duration::from_millis(150));
                subtree_size(target, SUBTREE_DEPTH)
            },
        };
        let mut slot = REVEALED.lock().unwrap_or_else(|e| e.into_inner());
        match *slot {
            Some((_, prev)) if CFEqual(prev as CFTypeRef, target as CFTypeRef) != 0 => {
                CFRelease(target as CFTypeRef);
            }
            _ => *slot = Some((pid, target as usize)),
        }
        Some(reveal)
    }
}

/// The element under the point in window `window_id`, or its nearest
/// ancestor, that advertises `AXShowAlternateUI`. Owned (+1) by the caller.
unsafe fn alternate_ui_target(pid: i32, window_id: u32, x: f64, y: f64) -> Option<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return None;
    }
    AXUIElementSetMessagingTimeout(app, AX_TIMEOUT_S);
    let mut hit: AXUIElementRef = std::ptr::null_mut();
    let err = AXUIElementCopyElementAtPosition(app, x as f32, y as f32, &mut hit);
    CFRelease(app as CFTypeRef);
    if err != kAXErrorSuccess || hit.is_null() {
        return None;
    }
    // The point must land in the target window, not another window of the app.
    let in_window = copy_element_attr(hit, "AXWindow").map(|w| {
        let id = ax_get_window_id(w);
        CFRelease(w as CFTypeRef);
        id == Some(window_id)
    });
    if in_window != Some(true) {
        CFRelease(hit as CFTypeRef);
        return None;
    }
    let mut current = Some(hit);
    for _ in 0..=ANCESTOR_DEPTH {
        let el = current.take()?;
        if advertises_alternate_ui(&copy_action_names(el)) {
            return Some(el);
        }
        current = copy_element_attr(el, "AXParent");
        CFRelease(el as CFTypeRef);
    }
    if let Some(el) = current {
        CFRelease(el as CFTypeRef);
    }
    None
}

/// Put the previously revealed element back to its default UI, unless it is
/// `keep` (the element about to be revealed again).
unsafe fn restore_default_ui(keep: Option<AXUIElementRef>) {
    let mut slot = REVEALED.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, prev)) = *slot else { return };
    if let Some(k) = keep {
        if CFEqual(prev as CFTypeRef, k as CFTypeRef) != 0 {
            return;
        }
    }
    perform_action(prev as AXUIElementRef, SHOW_DEFAULT);
    CFRelease(prev as CFTypeRef);
    *slot = None;
}

unsafe fn first_label(element: AXUIElementRef) -> String {
    for attr in ["AXTitle", "AXDescription", "AXValue"] {
        if let Some(s) = copy_string_attr(element, attr).filter(|s| !s.trim().is_empty()) {
            return s;
        }
    }
    // Outline rows carry their label on a child (or grandchild) text element.
    let mut label = String::new();
    for child in copy_children(element) {
        if label.is_empty() {
            label = text_of(child);
            if label.is_empty() {
                for grandchild in copy_children(child) {
                    if label.is_empty() {
                        label = text_of(grandchild);
                    }
                    CFRelease(grandchild as CFTypeRef);
                }
            }
        }
        CFRelease(child as CFTypeRef);
    }
    label
}

unsafe fn text_of(element: AXUIElementRef) -> String {
    copy_string_attr(element, "AXValue")
        .or_else(|| copy_string_attr(element, "AXTitle"))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_default()
}

/// Number of elements under `element`, `depth` levels deep (bounded).
unsafe fn subtree_size(element: AXUIElementRef, depth: usize) -> usize {
    if depth == 0 {
        return 0;
    }
    let mut n = 0;
    for child in copy_children(element) {
        n += 1 + subtree_size(child, depth - 1);
        CFRelease(child as CFTypeRef);
    }
    n
}

impl AxReveal {
    /// Whether the action made new controls appear (or the app says its
    /// alternate UI is visible).
    pub fn revealed_new_controls(&self) -> bool {
        self.elements_after > self.elements_before || self.visible == Some(true)
    }
}

/// Main-process JavaScript that hovers the renderer of the BrowserWindow whose
/// native window number is `window_id`, at window-local point (`x`, `y`) in
/// points. Returns a JSON string: `{"ok":true,"x":..,"y":..}` or
/// `{"ok":false,"reason":..}`.
pub fn electron_hover_js(window_id: u32, x: f64, y: f64) -> String {
    format!(
        r#"(() => {{
  const req = (typeof require === 'function') ? require : (process.mainModule && process.mainModule.require.bind(process.mainModule));
  if (!req) return JSON.stringify({{ ok: false, reason: 'no_require' }});
  const {{ BrowserWindow }} = req('electron');
  const w = BrowserWindow.getAllWindows().find((w) => {{
    try {{ return String(w.getMediaSourceId()).split(':')[1] === '{window_id}'; }} catch (e) {{ return false; }}
  }});
  if (!w) return JSON.stringify({{ ok: false, reason: 'no_window' }});
  const b = w.getBounds(), c = w.getContentBounds();
  const x = Math.round({x} - (c.x - b.x)), y = Math.round({y} - (c.y - b.y));
  if (x < 0 || y < 0 || x >= c.width || y >= c.height) return JSON.stringify({{ ok: false, reason: 'outside_content' }});
  w.webContents.sendInputEvent({{ type: 'mouseEnter', x, y }});
  w.webContents.sendInputEvent({{ type: 'mouseMove', x, y }});
  return JSON.stringify({{ ok: true, x, y }});
}})()"#,
        window_id = window_id,
        x = x,
        y = y
    )
}

/// Whether the main-process reply says the hover was delivered.
pub fn electron_hover_delivered(reply: &str) -> bool {
    let trimmed = reply.trim().trim_matches('"').replace("\\\"", "\"");
    serde_json::from_str::<serde_json::Value>(&trimmed)
        .ok()
        .and_then(|v| v.get("ok").and_then(|ok| ok.as_bool()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reveal_counts_only_when_controls_appear_or_the_app_says_so() {
        let r = |before, after, visible| AxReveal {
            role: "AXRow".into(),
            label: String::new(),
            visible,
            elements_before: before,
            elements_after: after,
        };
        assert!(r(3, 4, None).revealed_new_controls());
        assert!(r(3, 3, Some(true)).revealed_new_controls());
        assert!(!r(3, 3, None).revealed_new_controls());
        assert!(!r(3, 3, Some(false)).revealed_new_controls());
    }

    #[test]
    fn only_show_alternate_ui_qualifies_an_element() {
        assert!(advertises_alternate_ui(&[
            "AXShowDefaultUI".into(),
            "AXShowAlternateUI".into()
        ]));
        assert!(!advertises_alternate_ui(&[
            "AXPress".into(),
            "AXShowMenu".into()
        ]));
    }

    #[test]
    fn electron_js_targets_the_window_by_media_source_id_and_content_offset() {
        let js = electron_hover_js(227, 150.0, 178.5);
        assert!(js.contains("split(':')[1] === '227'"));
        assert!(js.contains("Math.round(150 - (c.x - b.x))"));
        assert!(js.contains("Math.round(178.5 - (c.y - b.y))"));
        assert!(js.contains("type: 'mouseEnter'"));
        assert!(js.contains("type: 'mouseMove'"));
    }

    #[test]
    fn electron_reply_parsing_accepts_raw_and_quoted_json() {
        assert!(electron_hover_delivered(r#"{"ok":true,"x":1,"y":2}"#));
        assert!(electron_hover_delivered(
            r#""{\"ok\":true,\"x\":1,\"y\":2}""#
        ));
        assert!(!electron_hover_delivered(
            r#"{"ok":false,"reason":"no_window"}"#
        ));
        assert!(!electron_hover_delivered("undefined"));
    }
}
