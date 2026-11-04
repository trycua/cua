//! Windows [`PointerShapeBackend`]: what cursor the guest would show at a
//! point.
//!
//! - **Hit-test ("uia")**: `WindowFromPoint` -> `GetAncestor(GA_ROOT)`, then
//!   `SendMessageTimeoutW(WM_NCHITTEST, SMTO_ABORTIFHUNG, 50 ms)` on the
//!   root window: a sizing border (the invisible DWM resize band included)
//!   is a resize cursor, exactly as the window itself would answer. Otherwise
//!   UI Automation `ElementFromPoint` on the crate's single-flight UIA worker
//!   (150 ms budget), mapped by [`crate::pointer_shape_map`] with up to
//!   three generic parents.
//! - **System ("getcursorinfo")**: `GetCursorInfo`'s `hCursor` compared with
//!   the shared `LoadCursorW(NULL, IDC_*)` handles, which are the same handle
//!   in every process of the session.
//! - **Probe ("setcursorpos")**: `SetCursorPos`, which moves the pointer
//!   without any button and makes the window under it receive
//!   `WM_MOUSEMOVE` / `WM_SETCURSOR`, so it sets its cursor.
//!
//! Coordinates are physical screen pixels; cua-spacesd runs per-monitor DPI
//! aware.
//!
//! Limitations:
//! - An application's own (non-system) cursor is not one of the shared
//!   `IDC_*` handles, so the readout reports `Unknown` for it and callers
//!   fall back to the hit-test.
//! - In Session 0 (a service without an interactive desktop) and on the
//!   secure desktop (UAC prompt, lock screen) there is no pointer to read or
//!   move and no UIA tree for the daemon.
//! - UIA cannot see into processes at a higher integrity level than
//!   cua-spacesd (an elevated app under a non-elevated daemon): those hit as
//!   their outer window and show the arrow; the `WM_NCHITTEST` edge check
//!   still works through UIPI for the resize band.
//! - Cross-process hosts (UWP `ApplicationFrameHost`) may stop at the outer
//!   pane; the answer is then the arrow.

use std::sync::OnceLock;

use cua_driver_core::cursor_shape::SystemCursorShape;
use cua_driver_core::pointer_shape::{BackendNames, HitTest, PointerShapeBackend, ScreenRect};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorInfo, GetCursorPos, GetWindowRect, LoadCursorW, SendMessageTimeoutW,
    SetCursorPos, WindowFromPoint, CURSORINFO, CURSOR_SHOWING, GA_ROOT, SMTO_ABORTIFHUNG,
    WM_NCHITTEST,
};

use crate::pointer_shape_map::{resolve_uia_chain, shape_for_idc, shape_for_nchittest, IDC_IDS};

/// `WM_NCHITTEST` budget: a hung window must not stall the hit-test.
const NCHITTEST_TIMEOUT_MS: u32 = 50;

/// `(HCURSOR as isize, IDC id)` for every standard cursor this session has.
fn standard_cursors() -> &'static [(isize, u16)] {
    static TABLE: OnceLock<Vec<(isize, u16)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        IDC_IDS
            .iter()
            .filter_map(|&id| {
                // MAKEINTRESOURCE: the id in the low word of the pointer.
                let handle =
                    unsafe { LoadCursorW(None, PCWSTR(id as usize as *const u16)) }.ok()?;
                (!handle.0.is_null()).then_some((handle.0 as isize, id))
            })
            .collect()
    })
}

/// The real cursor the OS is drawing now.
pub fn current_shape() -> SystemCursorShape {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut info) }.is_err() {
        return SystemCursorShape::Unknown;
    }
    if info.flags.0 & CURSOR_SHOWING.0 == 0 {
        // Hidden (typing, touch): nothing drawn, so the arrow is the honest
        // answer for a presence cursor.
        return SystemCursorShape::Default;
    }
    let handle = info.hCursor.0 as isize;
    standard_cursors()
        .iter()
        .find(|(h, _)| *h == handle)
        .and_then(|(_, id)| shape_for_idc(*id))
        .unwrap_or(SystemCursorShape::Unknown)
}

/// The Windows backend. Stateless; cheap to construct.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPointerShapes;

impl PointerShapeBackend for WindowsPointerShapes {
    fn names(&self) -> BackendNames {
        BackendNames {
            hit_test: "uia",
            system: "getcursorinfo",
            probe: "setcursorpos",
        }
    }

    fn hit_test(&self, x: f64, y: f64) -> Option<HitTest> {
        let (sx, sy) = (x.round() as i32, y.round() as i32);
        let hwnd = unsafe { WindowFromPoint(POINT { x: sx, y: sy }) };
        if hwnd.0.is_null() {
            return Some(HitTest::nothing());
        }
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        let root = if root.0.is_null() { hwnd } else { root };
        let mut frame_rect = RECT::default();
        let frame = unsafe { GetWindowRect(root, &mut frame_rect) }
            .ok()
            .map(|_| {
                ScreenRect::new(
                    frame_rect.left as f64,
                    frame_rect.top as f64,
                    (frame_rect.right - frame_rect.left) as f64,
                    (frame_rect.bottom - frame_rect.top) as f64,
                )
            });

        if let Some(shape) = nc_hit_test(root, sx, sy).and_then(shape_for_nchittest) {
            return Some(HitTest {
                shape,
                role: "edge".into(),
                window: frame,
                element: None,
            });
        }

        let (chain, rects, in_document) = crate::uia::windows_enum::point_shape_chain(sx, sy)?;
        let (shape, decider) = resolve_uia_chain(&chain, in_document);
        Some(HitTest {
            shape,
            role: decider
                .map(|i| control_type_name(chain[i].control_type).to_owned())
                .unwrap_or_default(),
            window: frame,
            element: decider
                .and_then(|i| rects.get(i).copied().flatten())
                .map(|[x, y, w, h]| ScreenRect::new(x, y, w, h)),
        })
    }

    fn system_shape(&self) -> SystemCursorShape {
        current_shape()
    }

    fn pointer_position(&self) -> Option<(f64, f64)> {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x as f64, p.y as f64))
    }

    fn warp_pointer(&self, x: f64, y: f64) -> bool {
        unsafe { SetCursorPos(x.round() as i32, y.round() as i32) }.is_ok()
    }

    fn limitation(&self) -> Option<String> {
        Some(
            "Application-defined cursors are not classified (hit-test fallback); \
             elevated apps and the secure desktop are opaque to a non-elevated daemon."
                .into(),
        )
    }
}

/// `WM_NCHITTEST` result of `root` at a screen point, `None` when the window
/// did not answer in time.
fn nc_hit_test(root: HWND, sx: i32, sy: i32) -> Option<isize> {
    // MAKELPARAM(x, y) with sign-preserving 16-bit words.
    let lparam = ((sy as u16 as u32) << 16 | (sx as u16 as u32)) as i32 as isize;
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            root,
            WM_NCHITTEST,
            WPARAM(0),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG,
            NCHITTEST_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (ok.0 != 0).then_some(result as isize)
}

fn control_type_name(id: i32) -> &'static str {
    use crate::pointer_shape_map::control_type::*;
    match id {
        BUTTON => "Button",
        CHECK_BOX => "CheckBox",
        COMBO_BOX => "ComboBox",
        EDIT => "Edit",
        HYPERLINK => "Hyperlink",
        MENU_ITEM => "MenuItem",
        RADIO_BUTTON => "RadioButton",
        TAB_ITEM => "TabItem",
        TEXT => "Text",
        DOCUMENT => "Document",
        SPLIT_BUTTON => "SplitButton",
        _ => "",
    }
}

/// Install the Windows backend and the system cursor probe.
pub fn install() -> bool {
    cua_driver_core::cursor_shape::set_cursor_shape_probe(current_shape);
    cua_driver_core::pointer_shape::set_pointer_shape_backend(WindowsPointerShapes)
}
