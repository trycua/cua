// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The macOS accessibility walk and the two actions the engine needs
//! (set a field's value, press a button). Raw `AXUIElement` FFI: the calls
//! are tiny and keeping them here avoids a dependency on the driver's AX
//! layer, whose window-scoping refuses SecurityAgent's windows.
//!
//! The process calling this needs the Accessibility grant (cua-spacesd's app
//! bundle holds it in the Space images; children inherit it).

use std::ffi::c_void;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

use crate::classify::{FieldSnap, Snapshot};

type AXUIElementRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> i32;
}

const MAX_DEPTH: usize = 10;
const MAX_NODES: usize = 600;

/// Whether this process holds the Accessibility grant.
pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// An owned `AXUIElement`.
#[derive(Clone)]
pub struct Element(CFType);

impl Element {
    fn raw(&self) -> AXUIElementRef {
        self.0.as_CFTypeRef()
    }

    fn attr(&self, name: &'static str) -> Option<CFType> {
        let name = CFString::from_static_string(name);
        let mut out: CFTypeRef = std::ptr::null();
        let status = unsafe {
            AXUIElementCopyAttributeValue(self.raw(), name.as_concrete_TypeRef(), &mut out)
        };
        if status != 0 || out.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(out) })
    }

    fn string(&self, name: &'static str) -> Option<String> {
        self.attr(name)?
            .downcast::<CFString>()
            .map(|s| s.to_string())
    }

    fn children(&self) -> Vec<Element> {
        let Some(value) = self.attr("AXChildren") else {
            return Vec::new();
        };
        let Some(array) = value.downcast::<CFArray<*const c_void>>() else {
            return Vec::new();
        };
        array
            .iter()
            .map(|item| Element(unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) }))
            .collect()
    }

    /// Sets `AXValue` to `text`. Errors carry the AX status code.
    pub fn set_value(&self, text: &str) -> Result<(), i32> {
        let name = CFString::from_static_string("AXValue");
        let value = CFString::new(text);
        let status = unsafe {
            AXUIElementSetAttributeValue(
                self.raw(),
                name.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(status)
        }
    }

    /// `AXPress`.
    pub fn press(&self) -> Result<(), i32> {
        let action = CFString::from_static_string("AXPress");
        let status = unsafe { AXUIElementPerformAction(self.raw(), action.as_concrete_TypeRef()) };
        if status == 0 {
            Ok(())
        } else {
            Err(status)
        }
    }
}

/// A button of a dialog.
pub struct Button {
    /// Title (or description when untitled).
    pub label: String,
    /// The element.
    pub element: Element,
}

/// A text field of a dialog.
pub struct Field {
    /// Password field.
    pub secure: bool,
    /// Nothing typed.
    pub empty: bool,
    /// The element.
    pub element: Element,
}

/// One window of SecurityAgent with its parts.
pub struct Window {
    /// `AXMain`: the dialog SecurityAgent is serving now. It keeps the
    /// windows of finished requests in its tree (hidden, with their old
    /// text and buttons), so only the main one is live.
    pub main: bool,
    /// What it shows.
    pub snapshot: Snapshot,
    /// Buttons, in tree order (parallel to `snapshot.buttons`).
    pub buttons: Vec<Button>,
    /// Fields, in tree order (parallel to `snapshot.fields`).
    pub fields: Vec<Field>,
}

/// Process ids of running processes named exactly `name`.
pub fn pids_named(name: &str) -> Vec<i32> {
    let Ok(out) = std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", name])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// The dialogs (AX windows) of process `pid`.
pub fn windows(pid: i32) -> Vec<Window> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Vec::new();
    }
    let app = Element(unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) });
    // A wedged agent must not hang a scan.
    unsafe { AXUIElementSetMessagingTimeout(app.raw(), 2.0) };
    app.children()
        .into_iter()
        .filter(|w| w.string("AXRole").as_deref() == Some("AXWindow"))
        .map(|w| {
            let mut window = Window {
                main: w.attr("AXMain").is_some_and(|v| {
                    v.downcast::<core_foundation::boolean::CFBoolean>()
                        .is_some_and(bool::from)
                }),
                snapshot: Snapshot::default(),
                buttons: Vec::new(),
                fields: Vec::new(),
            };
            let mut budget = MAX_NODES;
            walk(&w, 0, &mut budget, &mut window);
            window
        })
        .collect()
}

fn walk(element: &Element, depth: usize, budget: &mut usize, out: &mut Window) {
    if depth > MAX_DEPTH || *budget == 0 {
        return;
    }
    *budget -= 1;
    let role = element.string("AXRole").unwrap_or_default();
    match role.as_str() {
        "AXStaticText" => {
            if let Some(v) = element.string("AXValue").filter(|v| !v.trim().is_empty()) {
                out.snapshot.texts.push(v);
            }
        }
        "AXButton" => {
            let label = element
                .string("AXTitle")
                .filter(|t| !t.is_empty())
                .or_else(|| element.string("AXDescription"))
                .unwrap_or_default();
            out.snapshot.buttons.push(label.clone());
            out.buttons.push(Button {
                label,
                element: element.clone(),
            });
        }
        "AXTextField" | "AXTextArea" | "AXComboBox" => {
            let secure = element.string("AXSubrole").as_deref() == Some("AXSecureTextField");
            let empty = element
                .string("AXValue")
                .map(|v| v.is_empty())
                .unwrap_or(true);
            out.snapshot.fields.push(FieldSnap { secure, empty });
            out.fields.push(Field {
                secure,
                empty,
                element: element.clone(),
            });
        }
        _ => {}
    }
    for child in element.children() {
        walk(&child, depth + 1, budget, out);
    }
}
