//! Exact Linux AT-SPI setup for Chromium existing-profile attachment.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use cua_driver_core::browser::{
    BrowserProduct, BrowserRefusal, BrowserRefusalCode, BrowserSetupDescriptor,
    EXISTING_PROFILE_SETUP_READY_TIMEOUT,
};

use crate::atspi::AtspiNode;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn field_equals(node: &AtspiNode, expected: &str) -> bool {
    [
        node.name.as_deref(),
        node.value.as_deref(),
        node.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn role_is(node: &AtspiNode, accepted: &[&str]) -> bool {
    let role = node.role.trim().to_ascii_lowercase();
    accepted.iter().any(|candidate| role == *candidate)
}

fn setup_page_proven(
    nodes: &[AtspiNode],
    descriptor: &BrowserSetupDescriptor,
    trusted_navigation: bool,
) -> bool {
    let has_contradictory_address_bar = nodes.iter().any(|node| {
        role_is(node, &["entry", "text"])
            && field_equals(node, "Address and search bar")
            && node.value.as_deref().is_some_and(|value| {
                !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case(descriptor.setup_url)
            })
    });
    let exact_url = nodes.iter().any(|node| {
        role_is(node, &["entry", "text"])
            && field_equals(node, "Address and search bar")
            && node
                .value
                .as_deref()
                .or(node.name.as_deref())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
    });
    let exact_heading = nodes.iter().any(|node| {
        role_is(node, &["heading", "section", "static"])
            && field_equals(node, descriptor.page_heading)
    });
    let exact_page = nodes.iter().any(|node| {
        role_is(node, &["document web", "document frame"])
            && descriptor
                .page_titles
                .iter()
                .any(|title| field_equals(node, title))
    });
    let exact_identity =
        exact_url || (trusted_navigation && !has_contradictory_address_bar && exact_page);
    exact_identity && exact_heading
}

fn exact_setup_checkbox<'a>(
    nodes: &'a [AtspiNode],
    descriptor: &BrowserSetupDescriptor,
    trusted_navigation: bool,
) -> Result<Option<&'a AtspiNode>, BrowserRefusal> {
    if !setup_page_proven(nodes, descriptor, trusted_navigation) {
        return Ok(None);
    }
    let matches = nodes
        .iter()
        .filter(|node| {
            role_is(node, &["check box", "checkbox"])
                && field_equals(node, descriptor.checkbox_label)
                && !node.actions.is_empty()
                && node.element_index.is_some()
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [node] => Ok(Some(*node)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple exact remote-debugging checkboxes were exposed",
        )),
    }
}

fn setup_not_ready_message(descriptor: &BrowserSetupDescriptor) -> String {
    format!(
        "the exact {} remote-debugging setup page did not become ready; on Linux, existing-profile setup requires the browser's complete AT-SPI tree (launch Chromium-family browsers with --force-renderer-accessibility, or use a screen reader that enables full renderer accessibility)",
        descriptor.product_name
    )
}

fn with_target_foreground<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Some(window) =
            crate::wayland::sway_ipc::window_for_id(window_id).filter(|window| window.pid == pid)
        {
            crate::wayland::sway_ipc::with_focused_container(window.id, body)
        } else {
            crate::wayland::shell_helper::with_focused_window(pid, window_id, body)
        }
    } else {
        crate::input::with_x11_foreground(window_id, 80, body)
    }
}

fn close_tab(pid: u32, window_id: u64) -> anyhow::Result<()> {
    with_target_foreground(pid, window_id, || {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            crate::wayland::hotkey(window_id, &["ctrl".to_owned(), "w".to_owned()])
        } else {
            crate::input::send_key_xtest("w", &["ctrl"])
        }
    })
}

/// The exact address-and-search field of the approved window, or `None` while
/// the freshly created tab has not exposed one yet. More than one is refused:
/// the field is where the setup URL is about to be written, so the wrong pick
/// navigates a surface the caller never approved.
fn exact_omnibox<'a>(
    nodes: &'a [AtspiNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<&'a AtspiNode>, BrowserRefusal> {
    let matches = nodes
        .iter()
        .filter(|node| {
            role_is(node, &["entry", "text"])
                && field_equals(node, "Address and search bar")
                && node.element_index.is_some()
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [node] => Ok(Some(*node)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            format!(
                "{} exposed multiple exact address-and-search fields",
                descriptor.product_name
            ),
        )),
    }
}

/// Whether the omnibox currently holds exactly the fixed setup URL.
fn omnibox_holds_setup_url(node: &AtspiNode, descriptor: &BrowserSetupDescriptor) -> bool {
    node.value
        .as_deref()
        .or(node.name.as_deref())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
}

/// Navigate the approved window to its fixed setup page.
///
/// This mirrors the macOS and Windows adapters rather than synthesizing the URL
/// keystroke by keystroke: write the whole URL into the address field through
/// the accessibility API, read it back to prove it landed, and only then commit
/// with a single Enter. `set_text_contents` is the AT-SPI counterpart of UIA's
/// `ValuePattern::SetValue` and AppKit's `AXValue`.
///
/// Per-character synthesis was the wrong primitive here. XTEST keysym lookup is
/// keyboard-layout dependent and wlroots virtual-keyboard seats drop
/// punctuation, so `chrome://inspect` could arrive as `inspect` — which the
/// omnibox treats as a search term, silently navigating to a search-engine
/// results page. Nothing downstream could tell that apart from a slow-loading
/// setup page, so the flow reported a readiness timeout while leaving the user
/// on someone else's website. Writing the value whole removes the layout
/// dependency, and the read-back turns any residual mangling into an immediate,
/// accurate refusal.
fn trusted_setup_navigation(
    pid: u32,
    window_id: u64,
    descriptor: &BrowserSetupDescriptor,
) -> anyhow::Result<()> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

    // A fresh tab, so the setup page never displaces a page the user was on.
    // `ctrl+l` then focuses the address field. Both are single letters, so
    // neither depends on the keyboard layout the way punctuation does.
    let focus_omnibox = || -> anyhow::Result<()> {
        with_target_foreground(pid, window_id, || {
            if wayland {
                crate::wayland::hotkey_focused(&["ctrl".to_owned(), "l".to_owned()])
            } else {
                crate::input::send_key_xtest("l", &["ctrl"])
            }
        })
    };

    with_target_foreground(pid, window_id, || {
        if wayland {
            crate::wayland::hotkey_focused(&["ctrl".to_owned(), "t".to_owned()])
        } else {
            crate::input::send_key_xtest("t", &["ctrl"])
        }
    })?;
    std::thread::sleep(Duration::from_millis(100));
    focus_omnibox()?;

    // Wait for the new tab to publish its address field before writing to it.
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        if exact_omnibox(&tree.nodes, descriptor)
            .map_err(|error| anyhow::anyhow!(error.message))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "the approved {} window never exposed an exact address-and-search field",
                descriptor.product_name
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    // Transfer the URL through the clipboard rather than the keyboard.
    //
    // Chromium's AT-SPI bridge does not honour EditableText writes on the
    // omnibox, so the accessibility set-value that macOS (`AXValue`) and
    // Windows (`ValuePattern::SetValue`) rely on has no working counterpart
    // here. A paste keeps the property that actually matters: the exact string
    // arrives in one operation, with no per-keysym synthesis to be mistranslated
    // by the active layout or dropped by a virtual-keyboard seat. `ctrl+a` and
    // `ctrl+v` are plain letters, so they carry no layout dependency of their own.
    use cua_driver_core::clipboard::ClipboardBackend;
    let clipboard = crate::clipboard::LinuxClipboard::new();
    let restore = clipboard.read_text().ok().flatten();
    clipboard
        .write_text(descriptor.setup_url.to_owned())
        .map_err(|error| anyhow::anyhow!("could not stage the fixed setup URL: {error}"))?;
    let paste = with_target_foreground(pid, window_id, || {
        if wayland {
            crate::wayland::hotkey_focused(&["ctrl".to_owned(), "a".to_owned()])?;
            std::thread::sleep(Duration::from_millis(60));
            crate::wayland::hotkey_focused(&["ctrl".to_owned(), "v".to_owned()])
        } else {
            crate::input::send_key_xtest("a", &["ctrl"])?;
            std::thread::sleep(Duration::from_millis(60));
            crate::input::send_key_xtest("v", &["ctrl"])
        }
    });
    // The user's clipboard is theirs; put it back whether or not the paste took.
    std::thread::sleep(Duration::from_millis(120));
    if let Some(previous) = restore {
        let _ = clipboard.write_text(previous);
    }
    paste?;

    // Commit. Enter is the one synthesized keystroke left, and it carries no
    // layout dependency.
    with_target_foreground(pid, window_id, || {
        if wayland {
            crate::wayland::hotkey_focused(&["enter".to_owned()])
        } else {
            crate::input::send_key_xtest("enter", &[])
        }
    })?;

    // Verify the destination, not the input. Chromium exposes no readable text
    // on its omnibox over AT-SPI — no Value interface and no Text content even
    // while the field holds a URL — so the read-back that the Windows and macOS
    // adapters perform against the address field has no counterpart here.
    // Proving the tab actually arrived at the fixed setup page is the stronger
    // check anyway: it fails for a mistyped URL, a hijacked search, and a
    // redirect alike, and it names what was reached instead of timing out.
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        if tree.nodes.iter().any(|node| {
            role_is(node, &["document web", "document frame"])
                && descriptor
                    .page_titles
                    .iter()
                    .any(|title| field_equals(node, title))
        }) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let landed = tree
                .nodes
                .iter()
                .find(|node| role_is(node, &["document web", "document frame"]))
                .and_then(|node| node.name.clone())
                .unwrap_or_else(|| "no document".to_owned());
            anyhow::bail!(
                "the approved {} window did not reach its fixed setup page; it is showing {:?}. \
                 The address field is written through the clipboard, so this means the browser \
                 rejected or redirected the URL rather than that a keystroke was dropped",
                descriptor.product_name,
                landed
            );
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Walk the target window's accessibility tree, refusing unless the snapshot is
/// provably confined to that one window.
///
/// AT-SPI publishes a single tree per process, so a browser showing several
/// windows exposes all of their controls together — including one "Allow remote
/// debugging for this browser instance" checkbox per open setup page. Matching a
/// control by label across that tree can therefore find a control the caller did
/// not name. Requiring proven window scope is what makes the exact-window
/// contract real rather than assumed.
fn window_scoped_tree(
    pid: u32,
    window_id: u64,
) -> Result<crate::atspi::AtspiTreeResult, BrowserRefusal> {
    let tree = crate::atspi::walk_tree(pid, window_id, None);
    if !tree.trusted {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no trusted AT-SPI tree for the approved browser window; \
             the accessibility bus must be reachable to prove which window a control belongs to",
        ));
    }
    if !tree.window_scoped {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            format!(
                "could not prove which of pid {pid}'s accessibility top-levels renders window \
                 {window_id}, so a matched control cannot be attributed to the approved window; \
                 relaunch the browser with --remote-debugging-port to skip setup entirely"
            ),
        ));
    }
    Ok(tree)
}

pub struct SetupUiHandle {
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    trusted_setup_navigation: bool,
    enable_attempted: bool,
    trusted_checkbox_fallback_attempted: bool,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
}

impl SetupUiHandle {
    fn rollback_remote_debugging(&mut self) -> bool {
        if !self.enabled_remote_debugging {
            return true;
        }
        let tree = crate::atspi::walk_tree(self.pid, self.window_id, None);
        let restored =
            exact_setup_checkbox(&tree.nodes, self.descriptor, self.trusted_setup_navigation)
                .ok()
                .flatten()
                .filter(|node| node.checked == Some(true))
                .and_then(|node| node.element_index)
                .is_some_and(|index| crate::atspi::perform_action(self.pid, index).is_ok());
        if restored {
            self.enabled_remote_debugging = false;
        }
        restored
    }

    pub fn abort(mut self, error: BrowserRefusal) -> BrowserRefusal {
        let enabled_remote_debugging = self.enabled_remote_debugging;
        let restored_remote_debugging = self.rollback_remote_debugging();
        let opened_setup_page = self.opened_setup_page;
        let focused_setup_address_field = self.focused_setup_address_field;
        let foregrounded_window = self.foregrounded_window;
        let injected_global_input = self.injected_global_input;
        let closed_setup_page = self.close().unwrap_or(false);
        let mut error = error;
        let cause = error.detail.take();
        error.with_detail(serde_json::json!({
            "setup_side_effects": {
                "opened_setup_page": opened_setup_page,
                "closed_setup_page": closed_setup_page,
                "focused_setup_address_field": focused_setup_address_field,
                "enabled_remote_debugging": enabled_remote_debugging,
                "foregrounded_window": foregrounded_window,
                "injected_global_input": injected_global_input,
                "restored_remote_debugging": restored_remote_debugging,
            },
            "cause": cause,
        }))
    }

    pub fn close_for_success(mut self) -> Result<Option<bool>, BrowserRefusal> {
        if !self.opened_setup_page {
            return Ok(None);
        }
        let tree = crate::atspi::walk_tree(self.pid, self.window_id, None);
        if !setup_page_proven(&tree.nodes, self.descriptor, self.trusted_setup_navigation) {
            let error = refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the temporary setup page was no longer exact before cleanup",
            );
            return Err(self.abort(error));
        }
        if let Err(error) = close_tab(self.pid, self.window_id) {
            let error = refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("could not close the exact temporary setup tab: {error}"),
            );
            return Err(self.abort(error));
        }
        self.opened_setup_page = false;
        Ok(Some(true))
    }

    pub fn close(self) -> Option<bool> {
        if !self.opened_setup_page {
            return None;
        }
        let tree = crate::atspi::walk_tree(self.pid, self.window_id, None);
        Some(
            setup_page_proven(&tree.nodes, self.descriptor, self.trusted_setup_navigation)
                && close_tab(self.pid, self.window_id).is_ok(),
        )
    }
}

type PendingSetupKey = (u32, u64);

fn pending_setups() -> &'static Mutex<HashMap<PendingSetupKey, SetupUiHandle>> {
    static PENDING: OnceLock<Mutex<HashMap<PendingSetupKey, SetupUiHandle>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn retain_pending(
    pid: u32,
    window_id: u64,
    handle: SetupUiHandle,
) -> Result<(), BrowserRefusal> {
    let mut pending = pending_setups().lock().unwrap();
    if pending.contains_key(&(pid, window_id)) {
        drop(pending);
        return Err(handle.abort(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "another approved browser setup is already pending for this exact window",
        )));
    }
    pending.insert((pid, window_id), handle);
    Ok(())
}

pub fn commit_pending(pid: u32, window_id: u64) -> Result<bool, BrowserRefusal> {
    let handle = pending_setups()
        .lock()
        .unwrap()
        .remove(&(pid, window_id))
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                "the exact pending browser setup cleanup handle is missing",
            )
        })?;
    Ok(handle.close_for_success()?.unwrap_or(false))
}

pub fn abort_pending(pid: u32, window_id: u64, error: BrowserRefusal) -> BrowserRefusal {
    match pending_setups().lock().unwrap().remove(&(pid, window_id)) {
        Some(handle) => handle.abort(error),
        None => error.with_detail(serde_json::json!({
            "setup_cleanup": "the exact pending browser setup cleanup handle was missing"
        })),
    }
}

pub fn enable(
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
) -> Result<SetupUiHandle, BrowserRefusal> {
    let initial = window_scoped_tree(pid, window_id)?;
    let initial_checkbox = exact_setup_checkbox(&initial.nodes, descriptor, false)?;
    let mut handle = if initial_checkbox.is_some() {
        SetupUiHandle {
            pid,
            window_id,
            descriptor,
            trusted_setup_navigation: false,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
        }
    } else {
        let handle = SetupUiHandle {
            pid,
            window_id,
            descriptor,
            trusted_setup_navigation: true,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            opened_setup_page: true,
            enabled_remote_debugging: false,
            focused_setup_address_field: true,
            foregrounded_window: true,
            injected_global_input: true,
        };
        if let Err(error) = trusted_setup_navigation(pid, window_id, descriptor) {
            return Err(handle.abort(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "could not navigate the exact {} window to its fixed setup page: {error}",
                    descriptor.product_name
                ),
            )));
        }
        handle
    };

    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree = match window_scoped_tree(pid, window_id) {
            Ok(tree) => tree,
            Err(error) => return Err(handle.abort(error)),
        };
        match exact_setup_checkbox(&tree.nodes, descriptor, handle.trusted_setup_navigation) {
            Ok(Some(node)) => match node.checked {
                Some(true) => {
                    if handle.enable_attempted {
                        handle.enabled_remote_debugging = true;
                    }
                    return Ok(handle);
                }
                Some(false) if !handle.enable_attempted => {
                    let index = node.element_index.expect("actionable checkbox index");
                    if let Err(error) = crate::atspi::perform_action(pid, index) {
                        return Err(handle.abort(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!("the exact checkbox action failed: {error}"),
                        )));
                    }
                    handle.enable_attempted = true;
                }
                Some(false)
                    if descriptor.product == BrowserProduct::MicrosoftEdge
                        && !handle.trusted_checkbox_fallback_attempted =>
                {
                    handle.trusted_checkbox_fallback_attempted = true;
                    handle.foregrounded_window = true;
                    let trusted_navigation = handle.trusted_setup_navigation;
                    let clicked = with_target_foreground(pid, window_id, || {
                        std::thread::sleep(Duration::from_millis(60));
                        let tree = crate::atspi::walk_tree(pid, window_id, None);
                        let checkbox = exact_setup_checkbox(
                            &tree.nodes,
                            descriptor,
                            trusted_navigation,
                        )
                        .map_err(|error| anyhow::anyhow!(error.message))?
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "the exact Microsoft Edge remote-debugging checkbox became stale before the trusted click"
                            )
                        })?;
                        if checkbox.checked == Some(true) {
                            return Ok(false);
                        }
                        if checkbox.checked != Some(false) {
                            anyhow::bail!(
                                "the exact Microsoft Edge remote-debugging checkbox had an unknown state before the trusted click"
                            );
                        }
                        let index = checkbox.element_index.expect("actionable checkbox index");
                        let (x, y, width, height) = crate::atspi::get_element_bounds(pid, index)?;
                        if width <= 1 || height <= 1 {
                            anyhow::bail!(
                                "the exact Microsoft Edge remote-debugging checkbox had empty screen bounds"
                            );
                        }
                        let center_x = x
                            .checked_add(i32::try_from(width / 2)?)
                            .ok_or_else(|| anyhow::anyhow!("checkbox center x overflowed"))?;
                        let center_y = y
                            .checked_add(i32::try_from(height / 2)?)
                            .ok_or_else(|| anyhow::anyhow!("checkbox center y overflowed"))?;
                        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                            crate::wayland::click_desktop(center_x, center_y, 1, 1)?;
                        } else {
                            crate::input::send_click_xtest_desktop(center_x, center_y, 1, 1)?;
                        }
                        Ok(true)
                    });
                    match clicked {
                        Ok(injected) => handle.injected_global_input |= injected,
                        Err(error) => {
                            return Err(handle.abort(refusal(
                                BrowserRefusalCode::BrowserWrongTargetRefused,
                                format!(
                                    "could not toggle the exact Microsoft Edge remote-debugging checkbox: {error}"
                                ),
                            )))
                        }
                    }
                }
                Some(false) => {}
                None => {
                    return Err(handle.abort(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "AT-SPI did not expose the exact checkbox checked state",
                    )))
                }
            },
            Ok(None) => {}
            Err(error) => return Err(handle.abort(error)),
        }
        if Instant::now() >= deadline {
            return Err(handle.abort(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                setup_not_ready_message(descriptor),
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::{existing_profile_setup_descriptor, BrowserProduct};

    fn descriptor() -> &'static BrowserSetupDescriptor {
        existing_profile_setup_descriptor(BrowserProduct::GoogleChrome).unwrap()
    }

    fn node(role: &str, name: &str, value: Option<&str>, actions: &[&str]) -> AtspiNode {
        AtspiNode {
            element_index: (!actions.is_empty()).then_some(0),
            role: role.to_owned(),
            name: Some(name.to_owned()),
            value: value.map(str::to_owned),
            checked: None,
            enabled: None,
            selected: None,
            description: None,
            actions: actions.iter().map(|value| (*value).to_owned()).collect(),
            element_key: 0,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    #[test]
    fn checkbox_requires_exact_url_heading_and_unique_action() {
        let mut checkbox = node("check box", descriptor().checkbox_label, None, &["toggle"]);
        checkbox.checked = Some(false);
        let nodes = vec![
            node(
                "entry",
                "Address and search bar",
                Some(descriptor().setup_url),
                &["activate"],
            ),
            node("document web", descriptor().page_titles[0], None, &[]),
            node("heading", descriptor().page_heading, None, &[]),
            checkbox,
        ];
        assert_eq!(
            exact_setup_checkbox(&nodes, descriptor(), false)
                .unwrap()
                .unwrap()
                .checked,
            Some(false)
        );

        let titleless = vec![nodes[0].clone(), nodes[2].clone(), nodes[3].clone()];
        assert!(
            exact_setup_checkbox(&titleless, descriptor(), false)
                .unwrap()
                .is_some(),
            "an exact internal URL and heading prove products that omit the document title from AT-SPI"
        );

        let addressless = nodes[1..].to_vec();
        assert!(
            exact_setup_checkbox(&addressless, descriptor(), false)
                .unwrap()
                .is_none(),
            "page labels alone must not authorize a setup action"
        );
        assert!(
            exact_setup_checkbox(&addressless, descriptor(), true)
                .unwrap()
                .is_some(),
            "the exact compositor-routed fixed navigation may substitute for hidden browser chrome"
        );

        let mut redacted_address = nodes.clone();
        redacted_address[0].value = None;
        assert!(
            exact_setup_checkbox(&redacted_address, descriptor(), true)
                .unwrap()
                .is_some(),
            "an address control with a withheld value is not contradictory evidence"
        );

        let mut contradictory = nodes;
        contradictory[0].value = Some("https://example.test/spoof".to_owned());
        assert!(
            exact_setup_checkbox(&contradictory, descriptor(), true)
                .unwrap()
                .is_none(),
            "trusted navigation must not override a visible contradictory address bar"
        );
    }

    #[test]
    fn setup_timeout_explains_linux_renderer_accessibility_precondition() {
        let message = setup_not_ready_message(descriptor());
        assert!(message.contains("complete AT-SPI tree"));
        assert!(message.contains("--force-renderer-accessibility"));
    }

    fn omnibox(value: Option<&str>) -> AtspiNode {
        node("entry", "Address and search bar", value, &["activate"])
    }

    #[test]
    fn omnibox_selection_is_exact_or_refused() {
        let nodes = vec![
            node("push button", "Reload", None, &["press"]),
            omnibox(Some("about:blank")),
        ];
        assert!(exact_omnibox(&nodes, descriptor()).unwrap().is_some());
        assert!(exact_omnibox(&nodes[..1], descriptor()).unwrap().is_none());

        // Two address fields means two candidate destinations; writing the
        // setup URL into a guess could navigate a surface nobody approved.
        let mut ambiguous = nodes;
        ambiguous.push(omnibox(None));
        assert!(exact_omnibox(&ambiguous, descriptor()).is_err());
    }

    /// The regression that motivated the rewrite: XTEST dropped characters and
    /// `chrome://inspect` reached the omnibox as `inspect`, which Chrome
    /// submitted as a search query. The read-back has to reject that before it
    /// is ever committed.
    #[test]
    fn partially_applied_setup_url_is_not_accepted() {
        assert!(!omnibox_holds_setup_url(
            &omnibox(Some("inspect")),
            descriptor()
        ));
        assert!(!omnibox_holds_setup_url(
            &omnibox(Some("chrome://inspect")),
            descriptor()
        ));
        assert!(!omnibox_holds_setup_url(&omnibox(None), descriptor()));
        assert!(!omnibox_holds_setup_url(
            &omnibox(Some("https://www.google.com/search?q=inspect")),
            descriptor()
        ));
    }

    #[test]
    fn fully_applied_setup_url_is_accepted() {
        assert!(omnibox_holds_setup_url(
            &omnibox(Some(descriptor().setup_url)),
            descriptor()
        ));
        // Chromium reports the omnibox value with surrounding whitespace on
        // some toolkit versions, and case is not significant in a scheme.
        assert!(omnibox_holds_setup_url(
            &omnibox(Some("  CHROME://inspect/#remote-debugging  ")),
            descriptor()
        ));
    }
}
