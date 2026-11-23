// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! VMware Workstation guest-console detection for the keyboard paths.
//!
//! The guest console is an `MKSEmbedded` child (`MKSWindow#N`) nested under
//! the `VMUIFrame` top-level. It forwards keyboard input to the guest through
//! its own input path, not the window's message queue, and does not deliver
//! synthesized input: foreground activation, Unicode and scancode
//! `SendInput`, `WM_KEYDOWN`/`WM_KEYUP` with scancode bits, and `Ctrl+G` all
//! leave the guest prompt unchanged (#3210). `SendInput` still reports every
//! event inserted, so without this check `press_key`, `hotkey` and
//! `type_text` report a false success. Every keyboard route refuses instead.

pub(crate) const VMWARE_FRAME_CLASS: &str = "VMUIFrame";
pub(crate) const VMWARE_CONSOLE_CLASS: &str = "MKSEmbedded";

/// Keyboard input aimed at a VMware Workstation guest console. No input was
/// sent.
#[derive(Debug, thiserror::Error)]
#[error(
    "input_delivery_unavailable: the target is a VMware Workstation guest console \
     ({VMWARE_FRAME_CLASS} with an {VMWARE_CONSOLE_CLASS} child). VMware does not \
     deliver synthesized keyboard input to the guest, so Cua Driver refuses rather \
     than reporting a false success; no input was sent. {SUGGESTION}"
)]
pub(crate) struct VmwareConsoleInput;

const SUGGESTION: &str = "Drive the guest from inside: run Cua Driver in the guest, \
                          or use a guest channel such as SSH or `vmrun runProgramInGuest`.";

/// Whether a top-level of class `root_class` with descendants of
/// `descendant_classes` is a VMware Workstation guest console.
pub(crate) fn is_console_classes<'a>(
    root_class: &str,
    mut descendant_classes: impl Iterator<Item = &'a str>,
) -> bool {
    root_class == VMWARE_FRAME_CLASS && descendant_classes.any(|c| c == VMWARE_CONSOLE_CLASS)
}

/// Whether `hwnd`, or the top-level that owns it, is a VMware Workstation
/// window with a live guest console.
#[cfg(target_os = "windows")]
pub(crate) fn is_console_hwnd(hwnd: u64) -> bool {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetAncestor, GA_ROOT};

    if hwnd == 0 {
        return false;
    }
    let root = unsafe { GetAncestor(HWND(hwnd as *mut _), GA_ROOT) };
    let root = if root.0.is_null() {
        hwnd
    } else {
        root.0 as usize as u64
    };
    let root_class = crate::input::delivery::read_class_name(root);
    if root_class != VMWARE_FRAME_CLASS {
        return false;
    }

    unsafe extern "system" fn collect(child: HWND, lparam: LPARAM) -> BOOL {
        let classes = &mut *(lparam.0 as *mut Vec<String>);
        classes.push(crate::input::delivery::read_class_name(
            child.0 as usize as u64,
        ));
        TRUE
    }
    let mut classes: Vec<String> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(
            Some(HWND(root as *mut _)),
            Some(collect),
            LPARAM(&mut classes as *mut Vec<String> as isize),
        );
    }
    is_console_classes(&root_class, classes.iter().map(String::as_str))
}

/// Refuse keyboard input to a VMware guest console before any is sent.
#[cfg(target_os = "windows")]
pub(crate) fn refuse_console_keyboard(hwnd: u64) -> anyhow::Result<()> {
    if is_console_hwnd(hwnd) {
        return Err(VmwareConsoleInput.into());
    }
    Ok(())
}

/// The structured refusal for [`VmwareConsoleInput`], or `None` for any other
/// error.
pub(crate) fn console_refusal(
    error: &anyhow::Error,
) -> Option<cua_driver_core::protocol::ToolResult> {
    let refusal = error.downcast_ref::<VmwareConsoleInput>()?;
    Some(
        cua_driver_core::protocol::ToolResult::error(refusal.to_string()).with_structured(
            serde_json::json!({
                "code": "input_delivery_unavailable",
                "target_class": VMWARE_FRAME_CLASS,
                "console_class": VMWARE_CONSOLE_CLASS,
                "event_kind": "keyboard",
                "effect": "refused",
                "retryable": false,
                "alternative_route": "guest_side",
                "suggestion": SUGGESTION,
            }),
        ),
    )
}

/// The tool result for a failed keyboard call: the VMware refusal when that is
/// the cause, else the plain error.
pub(crate) fn input_error(error: anyhow::Error) -> cua_driver_core::protocol::ToolResult {
    console_refusal(&error)
        .unwrap_or_else(|| cua_driver_core::protocol::ToolResult::error(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vmware_frame_with_guest_console() {
        // Live child enumeration from #3210.
        let children = [
            "VMUIView",
            "VMware.GuestWindow",
            "MKSEmbedded",
            "MKSEmbedded",
        ];
        assert!(is_console_classes("VMUIFrame", children.into_iter()));
    }

    #[test]
    fn ignores_frame_without_console_and_console_class_elsewhere() {
        // Library view, no VM powered on.
        assert!(!is_console_classes(
            "VMUIFrame",
            ["VMUIView", "SysTreeView32"].into_iter()
        ));
        assert!(!is_console_classes(
            "Chrome_WidgetWin_1",
            ["MKSEmbedded"].into_iter()
        ));
        assert!(!is_console_classes("", std::iter::empty()));
    }

    #[test]
    fn refusal_carries_exact_code_and_no_retry() {
        let result = console_refusal(&VmwareConsoleInput.into()).expect("vmware refusal");
        let value = serde_json::to_value(&result).expect("serialize");
        let text = value.to_string();
        assert!(
            text.contains("input_delivery_unavailable: the target is a VMware Workstation"),
            "{text}"
        );
        assert_eq!(result.is_error, Some(true));
        let data = result.structured_content.as_ref().expect("structured");
        assert_eq!(data["code"], "input_delivery_unavailable");
        assert_eq!(data["effect"], "refused");
        assert_eq!(data["retryable"], false);
        assert_eq!(data["target_class"], "VMUIFrame");
    }

    #[test]
    fn other_errors_pass_through_unstructured() {
        assert!(console_refusal(&anyhow::anyhow!("foreground_unavailable: x")).is_none());
        let result = input_error(anyhow::anyhow!("foreground_unavailable: x"));
        assert!(result.structured_content.is_none());
    }
}
