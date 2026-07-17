//! Exact Linux AT-SPI setup for Chromium existing-profile attachment.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use cua_driver_core::browser::{
    BrowserRefusal, BrowserRefusalCode, BrowserSetupDescriptor,
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

fn exact_setup_navigation<'a>(
    nodes: &'a [AtspiNode],
    descriptor: &BrowserSetupDescriptor,
) -> Result<Option<&'a AtspiNode>, BrowserRefusal> {
    let exact_page = nodes.iter().any(|node| {
        role_is(node, &["document web", "document frame"])
            && descriptor
                .page_titles
                .iter()
                .any(|title| field_equals(node, title))
    });
    if !exact_page {
        return Ok(None);
    }
    let matches = nodes
        .iter()
        .filter(|node| {
            role_is(node, &["push button", "button"])
                && field_equals(node, descriptor.page_heading)
                && !node.actions.is_empty()
                && node.element_index.is_some()
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [node] => Ok(Some(*node)),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "multiple exact remote-debugging navigation controls were exposed",
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
        let window = crate::wayland::sway_ipc::window_for_id(window_id)
            .filter(|window| window.pid == pid)
            .ok_or_else(|| anyhow::anyhow!("no exact Sway container owns the approved window"))?;
        crate::wayland::sway_ipc::with_focused_container(window.id, body)
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

fn trusted_keyboard_setup_navigation(
    pid: u32,
    window_id: u64,
    descriptor: &BrowserSetupDescriptor,
) -> anyhow::Result<()> {
    let (base, _fragment) = descriptor
        .setup_url
        .split_once("/#")
        .ok_or_else(|| anyhow::anyhow!("the fixed setup URL has no /# delimiter"))?;
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    with_target_foreground(pid, window_id, || {
        if wayland {
            crate::wayland::hotkey_focused(&["ctrl".to_owned(), "t".to_owned()])?;
            std::thread::sleep(Duration::from_millis(100));
            crate::wayland::hotkey_focused(&["ctrl".to_owned(), "l".to_owned()])?;
            crate::wayland::type_text_then_key_focused(base, "enter")
        } else {
            crate::input::send_key_xtest("t", &["ctrl"])?;
            std::thread::sleep(Duration::from_millis(100));
            crate::input::send_key_xtest("l", &["ctrl"])?;
            crate::input::send_type_text_xtest(base)?;
            crate::input::send_key_xtest("enter", &[])
        }
    })?;

    // Avoid keyboard-layout-dependent `/#` synthesis on X11 and punctuation
    // loss on fresh wlroots virtual-keyboard seats. Open the fixed base page,
    // then invoke its unique semantic navigation control in the exact PID.
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        let tree = crate::atspi::walk_tree(pid, window_id, None);
        let navigation = exact_setup_navigation(&tree.nodes, descriptor)
            .map_err(|error| anyhow::anyhow!(error.message))?;
        match navigation {
            Some(node) => {
                return crate::atspi::perform_action(
                    pid,
                    node.element_index.expect("actionable navigation index"),
                )
                .map(|_| ())
            }
            None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            None => anyhow::bail!(
                "the exact {} setup navigation control did not become ready",
                descriptor.product_name
            ),
        }
    }
}

pub struct SetupUiHandle {
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    trusted_setup_navigation: bool,
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
    let initial = crate::atspi::walk_tree(pid, window_id, None);
    let initial_checkbox = exact_setup_checkbox(&initial.nodes, descriptor, false)?;
    let mut handle = if initial_checkbox.is_some() {
        SetupUiHandle {
            pid,
            window_id,
            descriptor,
            trusted_setup_navigation: false,
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
            opened_setup_page: true,
            enabled_remote_debugging: false,
            focused_setup_address_field: true,
            foregrounded_window: true,
            injected_global_input: true,
        };
        if let Err(error) = trusted_keyboard_setup_navigation(pid, window_id, descriptor) {
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
        let tree = crate::atspi::walk_tree(pid, window_id, None);
        match exact_setup_checkbox(&tree.nodes, descriptor, handle.trusted_setup_navigation) {
            Ok(Some(node)) => match node.checked {
                Some(true) => return Ok(handle),
                Some(false) if !handle.enabled_remote_debugging => {
                    let index = node.element_index.expect("actionable checkbox index");
                    if let Err(error) = crate::atspi::perform_action(pid, index) {
                        return Err(handle.abort(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!("the exact checkbox action failed: {error}"),
                        )));
                    }
                    handle.enabled_remote_debugging = true;
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

    #[test]
    fn setup_navigation_requires_one_actionable_control_on_the_exact_page() {
        let nodes = vec![
            node("document web", descriptor().page_titles[0], None, &[]),
            node("push button", descriptor().page_heading, None, &["press"]),
            node("heading", descriptor().page_heading, None, &[]),
        ];
        assert!(exact_setup_navigation(&nodes, descriptor())
            .unwrap()
            .is_some());
        assert!(exact_setup_navigation(&nodes[1..], descriptor())
            .unwrap()
            .is_none());

        let mut ambiguous = nodes;
        ambiguous.push(node(
            "push button",
            descriptor().page_heading,
            None,
            &["press"],
        ));
        assert!(exact_setup_navigation(&ambiguous, descriptor()).is_err());
    }
}
