// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who owns the window input would reach, for
//! `cua_driver_core::security_prompt`.
//!
//! Two kinds of route can reach a prompt, and each is checked where it
//! cannot be skipped:
//!
//! - Input addressed to a process (pid-routed events, AX actions, menus):
//!   the tool refuses a target pid that owns a prompt ([`refuse_pid`]).
//! - Input on the global HID queue, which WindowServer delivers to whatever
//!   is on top: every HID primitive in `crate::input` checks the screen point
//!   ([`refuse_point`]) or, for keys, the frontmost process
//!   ([`refuse_frontmost_keys`]) immediately before its first event, after any
//!   foreground activation has run. That covers every tool route and the
//!   browser setup assists.
//!
//! The call's [`PromptInput`] decision comes from the dispatch boundary
//! (`unrestricted` mode lifts the refusal).

use cua_driver_core::{
    protocol::ToolResult,
    security_prompt::{self, AlertContent, PromptInput, PromptOwner},
};

/// Input refused because it would reach a security or consent prompt. HID
/// primitives return it as their error; [`tool_result`] turns it back into
/// the refusal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{owner} is showing a system security or consent prompt there; nothing was sent")]
pub struct PromptProtected {
    pub owner: String,
}

impl PromptProtected {
    pub fn result(&self) -> ToolResult {
        security_prompt::refusal(&self.owner)
    }
}

/// The refusal carried by a primitive's error, if that is why it failed.
pub fn tool_result(error: &anyhow::Error) -> Option<ToolResult> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<PromptProtected>())
        .map(PromptProtected::result)
}

/// Refuse input addressed to a window or element of `pid` when `pid` draws
/// prompts. A UserNotificationCenter target is refused whenever one of its
/// on-screen alerts asks for consent.
pub fn refuse_pid(input: PromptInput, pid: i32) -> Result<(), ToolResult> {
    if input == PromptInput::Allow {
        return Ok(());
    }
    #[cfg(test)]
    if let Some(shown) = fixture::shown().filter(|shown| shown.owned_by(pid)) {
        return Err(shown.refusal().result());
    }
    let owner = Owner::of(pid);
    let protected = match owner.kind {
        PromptOwner::None => false,
        PromptOwner::Always => true,
        PromptOwner::ConsentAlertsOnly => crate::windows::on_screen_windows_any_layer()
            .into_iter()
            .any(|window| window.pid == pid && is_consent_window(pid, window.window_id)),
    };
    match protected {
        true => Err(owner.refusal().result()),
        false => Ok(()),
    }
}

/// Refuse a global HID pointer event at screen point `(x, y)` when a
/// prompt's window covers it. Every on-screen prompt window counts, on any
/// layer and in any order: prompts float above app windows, and a
/// transparent overlay in front of one must not hide it.
pub fn refuse_point(input: PromptInput, x: f64, y: f64) -> Result<(), PromptProtected> {
    if input == PromptInput::Allow {
        return Ok(());
    }
    #[cfg(test)]
    if let Some(shown) = fixture::shown() {
        return match shown.covers(x, y) {
            true => Err(shown.refusal()),
            false => Ok(()),
        };
    }
    let covering = crate::windows::on_screen_windows_any_layer()
        .into_iter()
        .filter(|window| {
            let b = &window.bounds;
            x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height
        });
    for window in covering {
        let owner = Owner::of(window.pid);
        let protected = match owner.kind {
            PromptOwner::None => false,
            PromptOwner::Always => true,
            PromptOwner::ConsentAlertsOnly => is_consent_window(window.pid, window.window_id),
        };
        if protected {
            return Err(owner.refusal());
        }
    }
    Ok(())
}

/// Refuse global HID keys when the frontmost process draws prompts: the HID
/// queue delivers keys to it. A prompt such as SecurityAgent's password
/// sheet takes the front when it appears. UserNotificationCenter is refused
/// only while it shows a consent alert.
pub fn refuse_frontmost_keys(input: PromptInput) -> Result<(), PromptProtected> {
    if input == PromptInput::Allow {
        return Ok(());
    }
    #[cfg(test)]
    if let Some(shown) = fixture::shown() {
        return Err(shown.refusal());
    }
    let Some(pid) = crate::apps::frontmost_pid() else {
        return Ok(());
    };
    refuse_pid(input, pid).map_err(|_| Owner::of(pid).refusal())
}

/// The processes that draw keychain and password prompts: SecurityAgent
/// ("… wants to use your confidential information stored in … in your
/// keychain", administrator password sheets) and LocalAuthentication's Touch
/// ID / password panel. Bundle id and executable name; either one matches.
const KEYCHAIN_PROMPT_OWNERS: &[(&str, &str)] = &[
    ("com.apple.SecurityAgent", "SecurityAgent"),
    ("com.apple.LocalAuthentication.UIAgent", "coreautha"),
];

fn is_keychain_prompt_owner(bundle_id: Option<&str>, process_name: Option<&str>) -> bool {
    KEYCHAIN_PROMPT_OWNERS
        .iter()
        .any(|(bundle, name)| bundle_id == Some(*bundle) || process_name == Some(*name))
}

/// Whether a keychain or password prompt is on screen. A browser that reads
/// a keychain secret waits on such a prompt, and each further launch queues
/// another, so isolated browser launches stop while one is open. Other
/// prompts (privacy consent, Gatekeeper) do not hold a launch up and do not
/// count. Only looks: nothing here answers, dismisses or types into a prompt.
pub fn security_prompt_on_screen() -> bool {
    #[cfg(test)]
    if fixture::shown().is_some() {
        return true;
    }
    let mut seen = std::collections::HashSet::new();
    crate::windows::on_screen_windows_any_layer()
        .into_iter()
        .filter(|window| seen.insert(window.pid))
        .any(|window| {
            is_keychain_prompt_owner(
                crate::apps::bundle_id_for_pid(window.pid).as_deref(),
                process_name(window.pid).as_deref(),
            )
        })
}

struct Owner {
    kind: PromptOwner,
    name: String,
}

impl Owner {
    fn of(pid: i32) -> Self {
        let bundle_id = crate::apps::bundle_id_for_pid(pid);
        let name = process_name(pid);
        Self {
            kind: security_prompt::macos_prompt_owner(bundle_id.as_deref(), name.as_deref()),
            name: name.or(bundle_id).unwrap_or_else(|| "unknown".to_owned()),
        }
    }

    fn refusal(&self) -> PromptProtected {
        PromptProtected {
            owner: self.name.clone(),
        }
    }
}

/// The executable name of `pid`, as `proc_name` reports it.
fn process_name(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 256];
    let length = unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length]).into_owned())
}

/// Whether UserNotificationCenter's window `window_id` asks for consent.
fn is_consent_window(pid: i32, window_id: u32) -> bool {
    security_prompt::is_consent_alert(alert_content(pid, window_id).as_ref(), consent_labels())
}

/// The secure field and button titles of `pid`'s AX window `window_id`, or
/// `None` when any part of the window cannot be read: a timed-out or failed
/// read must not make a consent alert look like one without consent buttons.
fn alert_content(pid: i32, window_id: u32) -> Option<AlertContent> {
    use crate::ax::bindings::{
        ax_get_window_id, copy_ax_windows_or_error, AXUIElementCreateApplication,
        AXUIElementSetMessagingTimeout,
    };
    use core_foundation::base::{CFRelease, CFTypeRef};

    const READ_TIMEOUT_SECONDS: f32 = 0.25;
    const MAX_ELEMENTS: usize = 200;
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, READ_TIMEOUT_SECONDS);
        let windows = copy_ax_windows_or_error(app);
        CFRelease(app as CFTypeRef);
        let mut window = None;
        for candidate in windows.ok()? {
            if window.is_none() && ax_get_window_id(candidate) == Some(window_id) {
                window = Some(candidate);
            } else {
                CFRelease(candidate as CFTypeRef);
            }
        }
        let mut pending = vec![window?];
        let mut content = Some(AlertContent::default());
        let mut visited = 0;
        while let Some(element) = pending.pop() {
            visited += 1;
            if let Some(found) = content.as_mut().filter(|_| visited <= MAX_ELEMENTS) {
                match read_alert_element(element, found) {
                    Some(children) => pending.extend(children),
                    None => content = None,
                }
            } else {
                content = None;
            }
            CFRelease(element as CFTypeRef);
        }
        content
    }
}

/// Record `element`'s part of an alert and return its children, or `None`
/// when a read failed.
///
/// # Safety
///
/// `element` must be valid; the caller must release every returned child.
unsafe fn read_alert_element(
    element: crate::ax::bindings::AXUIElementRef,
    content: &mut AlertContent,
) -> Option<Vec<crate::ax::bindings::AXUIElementRef>> {
    match read_string(element, "AXRole")?.as_deref() {
        Some("AXButton") => {
            // A button names itself by its title, else by its description
            // (an icon-only Help button). One with neither stays empty, and
            // an empty label makes the alert count as consent.
            let title = read_string(element, "AXTitle")?.filter(|title| !title.trim().is_empty());
            let label = match title {
                Some(title) => Some(title),
                None => read_string(element, "AXDescription")?,
            };
            content.button_titles.push(label.unwrap_or_default());
        }
        Some("AXTextField") => {
            content.has_secure_field |=
                read_string(element, "AXSubrole")?.as_deref() == Some("AXSecureTextField");
        }
        _ => {}
    }
    read_children(element)
}

/// A string attribute: `Some(None)` when the element has no such value,
/// `None` when the read failed.
unsafe fn read_string(
    element: crate::ax::bindings::AXUIElementRef,
    attribute: &str,
) -> Option<Option<String>> {
    use crate::ax::bindings::{kAXErrorSuccess, AXUIElementCopyAttributeValue};
    use core_foundation::{
        base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType},
        string::CFString,
    };
    let name = CFString::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let error = AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value);
    if error != kAXErrorSuccess {
        return absent_value(error).then_some(None);
    }
    if value.is_null() {
        return Some(None);
    }
    if CFGetTypeID(value) != CFString::type_id() {
        CFRelease(value);
        return Some(None);
    }
    Some(Some(
        CFString::wrap_under_create_rule(value as _).to_string(),
    ))
}

/// An element's children, or `None` when the read failed.
unsafe fn read_children(
    element: crate::ax::bindings::AXUIElementRef,
) -> Option<Vec<crate::ax::bindings::AXUIElementRef>> {
    use crate::ax::bindings::{
        kAXErrorSuccess, AXUIElementCopyAttributeValue, AXUIElementGetTypeID,
    };
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType},
        string::CFString,
    };
    let name = CFString::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let error = AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value);
    if error != kAXErrorSuccess {
        return absent_value(error).then(Vec::new);
    }
    if value.is_null() {
        return Some(Vec::new());
    }
    if CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
        CFRelease(value);
        return None;
    }
    let array = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
    let element_type = AXUIElementGetTypeID();
    let mut children = Vec::with_capacity(array.len() as usize);
    for index in 0..array.len() {
        let child = *array.get(index)?;
        if CFGetTypeID(child) != element_type {
            continue;
        }
        CFRetain(child);
        children.push(child as crate::ax::bindings::AXUIElementRef);
    }
    Some(children)
}

/// Whether an AX error means the attribute simply has no value here, as
/// opposed to a read that failed (a timeout, an invalid element, no access).
fn absent_value(error: crate::ax::bindings::AXError) -> bool {
    use crate::ax::bindings::{kAXErrorAttributeUnsupported, kAXErrorNoValue};
    error == kAXErrorNoValue || error == kAXErrorAttributeUnsupported
}

/// The button labels the system's consent prompts use, in every language it
/// ships: privacy (TCC) requests and local-network requests, both drawn by
/// UserNotificationCenter. Read once from the system's own string tables.
/// Empty when any table yields no labels: a missing table could hold the
/// label of the prompt on screen, so then no alert is ruled out.
fn consent_labels() -> &'static [String] {
    const TABLES: &[(&str, &[&str])] = &[
        (
            "/System/Library/PrivateFrameworks/TCC.framework/Resources/Localizable.loctable",
            &["REQUEST_ACCESS_", "NOTIFY_BUTTON_"],
        ),
        (
            "/System/Library/Frameworks/NetworkExtension.framework/Resources/Localizable.loctable",
            &["ALLOW_BUTTON", "DONT_ALLOW_BUTTON"],
        ),
    ];
    static LABELS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LABELS.get_or_init(|| {
        let tables: Vec<Vec<String>> = TABLES
            .iter()
            .map(|(path, keys)| button_labels(path, keys))
            .collect();
        merge_label_tables(tables)
    })
}

/// All labels of every table, or none when any table is empty.
fn merge_label_tables(tables: Vec<Vec<String>>) -> Vec<String> {
    if tables.iter().any(Vec::is_empty) {
        return Vec::new();
    }
    let mut labels: Vec<String> = tables.into_iter().flatten().collect();
    labels.sort();
    labels.dedup();
    labels
}

/// Every short value in the localized string table at `path` whose key
/// starts with one of `key_prefixes`, across all languages.
fn button_labels(path: &str, key_prefixes: &[&str]) -> Vec<String> {
    use core_foundation::{
        base::{CFType, TCFType},
        data::CFData,
        dictionary::CFDictionary,
        propertylist::{create_with_data, kCFPropertyListImmutable},
        string::CFString,
    };
    // A label longer than this is a sentence (a prompt body), not a button.
    const MAX_LABEL_CHARS: usize = 40;

    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok((plist, _)) = create_with_data(CFData::from_buffer(&bytes), kCFPropertyListImmutable)
    else {
        return Vec::new();
    };
    let plist = unsafe { CFType::wrap_under_create_rule(plist) };
    let dictionary = |value: &CFType| value.downcast::<CFDictionary>();
    let Some(languages) = dictionary(&plist) else {
        return Vec::new();
    };
    let mut labels = Vec::new();
    let (_, tables) = languages.get_keys_and_values();
    for table in tables {
        let table = unsafe { CFType::wrap_under_get_rule(table) };
        let Some(table) = dictionary(&table) else {
            continue;
        };
        let (keys, values) = table.get_keys_and_values();
        for (key, value) in keys.into_iter().zip(values) {
            let key = unsafe { CFType::wrap_under_get_rule(key) }.downcast::<CFString>();
            let value = unsafe { CFType::wrap_under_get_rule(value) }.downcast::<CFString>();
            let (Some(key), Some(value)) = (key, value) else {
                continue;
            };
            let key = key.to_string();
            let value = value.to_string();
            if key_prefixes.iter().any(|prefix| key.starts_with(prefix))
                && !value.is_empty()
                && value.chars().count() <= MAX_LABEL_CHARS
                && !value.contains("%@")
            {
                labels.push(value.trim().to_owned());
            }
        }
    }
    labels
}

/// Test fixture: a prompt window over a screen region that also holds the
/// front, on this thread, so the HID primitives' guards can be tested
/// without a window server or a real prompt.
#[cfg(test)]
pub(crate) mod fixture {
    use super::PromptProtected;
    use std::cell::Cell;

    /// The prompt's screen rectangle `(x, y, width, height)` and the pid
    /// that draws it.
    #[derive(Clone, Copy)]
    pub(crate) struct PromptShown((f64, f64, f64, f64), i32);

    /// The pid the fixture's prompt belongs to: a value no real process has.
    pub(crate) const PROMPT_PID: i32 = i32::MAX;

    thread_local! {
        static SHOWN: Cell<Option<PromptShown>> = const { Cell::new(None) };
    }

    impl PromptShown {
        /// A prompt covering the whole screen.
        pub(crate) fn start() -> Guard {
            Self::over((f64::MIN / 2.0, f64::MIN / 2.0, f64::MAX, f64::MAX))
        }

        pub(crate) fn over(rect: (f64, f64, f64, f64)) -> Guard {
            SHOWN.with(|shown| shown.set(Some(Self(rect, PROMPT_PID))));
            Guard
        }

        pub(super) fn owned_by(self, pid: i32) -> bool {
            self.1 == pid
        }

        pub(super) fn covers(self, x: f64, y: f64) -> bool {
            let (left, top, width, height) = self.0;
            x >= left && x < left + width && y >= top && y < top + height
        }

        pub(super) fn refusal(self) -> PromptProtected {
            PromptProtected {
                owner: "SecurityAgent".to_owned(),
            }
        }
    }

    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            SHOWN.with(|shown| shown.set(None));
        }
    }

    pub(super) fn shown() -> Option<PromptShown> {
        SHOWN.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_keychain_and_password_prompts_hold_up_browser_launches() {
        assert!(is_keychain_prompt_owner(
            Some("com.apple.SecurityAgent"),
            None
        ));
        assert!(is_keychain_prompt_owner(None, Some("SecurityAgent")));
        assert!(is_keychain_prompt_owner(None, Some("coreautha")));
        for (bundle, name) in [
            (
                "com.apple.accessibility.universalAccessAuthWarn",
                "universalAccessAuthWarn",
            ),
            ("com.apple.coreservices.uiagent", "CoreServicesUIAgent"),
            ("com.apple.UserNotificationCenter", "UserNotificationCenter"),
            ("com.google.Chrome", "Google Chrome"),
        ] {
            assert!(
                !is_keychain_prompt_owner(Some(bundle), Some(name)),
                "{name}"
            );
        }
        // Every keychain prompt owner is also a prompt that input never reaches.
        for (bundle, name) in KEYCHAIN_PROMPT_OWNERS {
            assert_eq!(
                security_prompt::macos_prompt_owner(Some(bundle), Some(name)),
                PromptOwner::Always
            );
        }
    }

    #[test]
    fn a_shown_prompt_is_reported_to_browser_launches() {
        let _shown = fixture::PromptShown::start();
        assert!(security_prompt_on_screen());
    }

    #[test]
    fn this_process_is_not_a_prompt() {
        let pid = std::process::id() as i32;
        assert!(refuse_pid(PromptInput::Refuse, pid).is_ok());
    }

    #[test]
    fn the_system_consent_labels_include_allow_and_dont_allow() {
        // Read from the OS's own tables; English must be among the languages.
        let labels = consent_labels();
        for label in ["Allow", "Don’t Allow", "Don't Allow"] {
            assert!(
                labels.iter().any(|known| known == label),
                "{label} in {labels:?}"
            );
        }
        for not_consent in ["Ignore", "Report…", "OK", "Cancel"] {
            assert!(
                !labels.iter().any(|known| known == not_consent),
                "{not_consent} is not a consent label"
            );
        }
    }

    /// Live, read-only check of the prompts on this desktop: prints how each
    /// on-screen window of a prompt owner is classified and whether a HID
    /// pointer event at its centre would be refused. Sends no input. Run by
    /// hand with `cargo test -p platform-macos --lib -- --ignored
    /// on_screen_prompts --nocapture`.
    #[test]
    #[ignore = "reads the live desktop's prompt windows"]
    fn on_screen_prompts_are_classified() {
        for window in crate::windows::on_screen_windows_any_layer() {
            let owner = Owner::of(window.pid);
            let consent = match owner.kind {
                PromptOwner::None => continue,
                PromptOwner::Always => None,
                PromptOwner::ConsentAlertsOnly => {
                    let content = alert_content(window.pid, window.window_id);
                    assert!(
                        content.is_some(),
                        "window {} did not read",
                        window.window_id
                    );
                    Some((
                        content.map(|content| content.button_titles),
                        is_consent_window(window.pid, window.window_id),
                    ))
                }
            };
            let b = &window.bounds;
            let refused = refuse_point(
                PromptInput::Refuse,
                b.x + b.width / 2.0,
                b.y + b.height / 2.0,
            )
            .err()
            .map(|refusal| refusal.owner);
            if consent.is_none() || consent.as_ref().is_some_and(|(_, consent)| *consent) {
                assert!(
                    refused.is_some(),
                    "prompt window {} not refused",
                    window.window_id
                );
            }
            assert!(refuse_point(PromptInput::Allow, b.x + 1.0, b.y + 1.0).is_ok());
            println!(
                "{} window {} layer {} {:?} centre refused by {:?}",
                owner.name, window.window_id, window.layer, consent, refused
            );
        }
    }

    #[test]
    fn one_unreadable_label_table_rules_out_nothing() {
        let tcc = vec!["Allow".to_owned()];
        let network = vec!["Allow".to_owned(), "Don’t Allow".to_owned()];
        assert_eq!(
            merge_label_tables(vec![tcc.clone(), network.clone()]),
            vec!["Allow".to_owned(), "Don’t Allow".to_owned()]
        );
        assert!(merge_label_tables(vec![tcc, Vec::new()]).is_empty());
        assert!(merge_label_tables(vec![Vec::new(), network]).is_empty());
    }

    #[test]
    fn a_refused_primitive_error_maps_back_to_the_refusal() {
        let error = anyhow::Error::new(PromptProtected {
            owner: "SecurityAgent".to_owned(),
        })
        .context("desktop click failed");
        let result = tool_result(&error).expect("the refusal survives context");
        let structured = result.structured_content.unwrap();
        assert_eq!(
            structured["refusal"]["code"],
            security_prompt::SECURITY_PROMPT_PROTECTED
        );
        assert_eq!(structured["refusal"]["owner"], "SecurityAgent");
        assert!(tool_result(&anyhow::anyhow!("other failure")).is_none());
    }
}
