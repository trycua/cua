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

use crate::atspi::{native::ObjectRef, AtspiNode, AtspiTreeResult};
use crate::wayland::hyprland_input::{self, Action, ActionCancellation};

/// Invocation-scoped input authority. Only the platform adapter can construct it
/// from registry-admitted private lifecycle metadata; it is never deserialized.
#[derive(Clone)]
pub(crate) struct SetupInput {
    pub owner: String,
    pub lane_owner: String,
    pub fingerprint: cua_driver_core::browser::ProcessFingerprint,
    pub hyprland: bool,
    pub cancellation: ActionCancellation,
}

fn foreground_ack(value: serde_json::Value) -> anyhow::Result<()> {
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
        && value.get("route").and_then(serde_json::Value::as_str) == Some("primary_foreground")
    {
        return Ok(());
    }
    anyhow::bail!("exact Hyprland setup input refused: {value}")
}

impl SetupInput {
    fn check(&self) -> anyhow::Result<()> {
        self.cancellation.check()?;
        if cua_driver_core::session::is_session_ending(&self.lane_owner) {
            anyhow::bail!("browser setup lifecycle is ending");
        }
        Ok(())
    }

    fn hotkey(
        &self,
        pid: u32,
        window_id: u64,
        key: &str,
        modifiers: &[&str],
    ) -> anyhow::Result<()> {
        self.check()?;
        if self.hyprland {
            return foreground_ack(hyprland_input::execute_foreground(
                Some(self.lane_owner.clone()),
                pid,
                window_id,
                Action::Key {
                    key: key.to_owned(),
                    modifiers: modifiers.iter().map(|value| (*value).to_owned()).collect(),
                },
                self.cancellation.clone(),
            )?);
        }
        with_target_foreground(pid, window_id, || {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                let mut keys = modifiers
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>();
                keys.push(key.to_owned());
                crate::wayland::hotkey_focused(&keys)
            } else {
                crate::input::send_key_xtest(key, modifiers)
            }
        })
    }

    fn type_setup_url(&self, pid: u32, window_id: u64, url: &str) -> anyhow::Result<()> {
        self.check()?;
        foreground_ack(hyprland_input::execute_foreground_text(
            Some(self.lane_owner.clone()),
            pid,
            window_id,
            url,
            self.cancellation.clone(),
        )?)
    }
}

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

fn close_tab(pid: u32, window_id: u64, input: &SetupInput) -> anyhow::Result<()> {
    input.hotkey(pid, window_id, "w", &["ctrl"])
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
#[cfg(test)]
fn omnibox_holds_setup_url(node: &AtspiNode, descriptor: &BrowserSetupDescriptor) -> bool {
    node.value
        .as_deref()
        .or(node.name.as_deref())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(descriptor.setup_url))
}

#[derive(Debug)]
struct BrowserTabs {
    objects: Vec<ObjectRef>,
    selected: ObjectRef,
}

fn browser_tabs(tree: &AtspiTreeResult) -> Result<BrowserTabs, BrowserRefusal> {
    require_window_scope(tree.trusted, tree.window_scoped)?;
    if tree.truncated {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "a complete browser tab tree is required to prove setup tab ownership",
        ));
    }
    let mut objects = Vec::new();
    let mut selected = None;
    for tab in tree
        .nodes
        .iter()
        .filter(|node| !node.in_web_content && role_is(node, &["page tab", "tab", "tab item"]))
    {
        let object = tab
            .object_ref
            .as_ref()
            .filter(|object| !object.bus.is_empty() && !object.path.is_empty())
            .ok_or_else(|| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "a browser tab has no stable accessibility object identity",
                )
            })?;
        if objects.contains(object) {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "duplicate browser tab identities prevent exact setup ownership",
            ));
        }
        let is_selected = tab.selected.ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "a browser tab has no verifiable selected state",
            )
        })?;
        if is_selected && selected.replace(object.clone()).is_some() {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "multiple browser tabs claim to be selected",
            ));
        }
        objects.push(object.clone());
    }
    let selected = selected.ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "the browser exposes no uniquely selected tab identity",
        )
    })?;
    Ok(BrowserTabs { objects, selected })
}

fn created_tab(
    baseline: &BrowserTabs,
    current: &BrowserTabs,
) -> Result<Option<ObjectRef>, BrowserRefusal> {
    if baseline
        .objects
        .iter()
        .any(|object| !current.objects.contains(object))
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "a pre-existing browser tab changed while setup was creating its tab",
        ));
    }
    let added = current
        .objects
        .iter()
        .filter(|object| !baseline.objects.contains(object))
        .collect::<Vec<_>>();
    match added.as_slice() {
        [] if current.selected == baseline.selected => Ok(None),
        [object] if **object == current.selected => Ok(Some((*object).clone())),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "setup did not create exactly one uniquely selected browser tab",
        )),
    }
}

/// Navigate the approved window to its fixed setup page.
///
/// Hyprland uses the existing exact-window, US-layout-guarded plugin route.
/// Other backends retain their existing whole-value clipboard delivery. Every
/// route still proves arrival at the fixed setup page before toggling anything.
fn trusted_setup_navigation(
    handle: &mut SetupUiHandle,
    initial: &AtspiTreeResult,
) -> anyhow::Result<()> {
    let (pid, window_id, descriptor) = (handle.pid, handle.window_id, handle.descriptor);
    let input = handle.input.clone();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

    // A shortcut acknowledgement is not tab ownership. Preserve every tab
    // from a complete baseline and prove one new, uniquely selected object.
    let baseline = browser_tabs(initial).map_err(|error| anyhow::anyhow!(error.message))?;
    handle.hotkey("t", &["ctrl"])?;
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        input.check()?;
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        let current = browser_tabs(&tree).map_err(|error| anyhow::anyhow!(error.message))?;
        if let Some(object) =
            created_tab(&baseline, &current).map_err(|error| anyhow::anyhow!(error.message))?
        {
            handle.owned_setup_tab = Some(object);
            handle.opened_setup_page = true;
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("the setup shortcut did not produce a verifiably owned browser tab");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    handle.hotkey("l", &["ctrl"])?;

    // Wait for the new tab to publish its address field before writing to it.
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        input.check()?;
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        handle
            .require_owned_selected_tab(&tree)
            .map_err(|error| anyhow::anyhow!(error.message))?;
        if exact_omnibox(&tree.nodes, descriptor)
            .map_err(|error| anyhow::anyhow!(error.message))?
            .is_some()
        {
            handle.focused_setup_address_field = true;
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

    if input.hyprland {
        // No clipboard access: the plugin refuses unsupported layouts and
        // guards every packet, including punctuation and held modifiers.
        handle.hotkey("a", &["ctrl"])?;
        handle
            .owned_tab_tree()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        handle.record_input_result(input.type_setup_url(pid, window_id, descriptor.setup_url))?;
    } else {
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
    }
    handle
        .owned_tab_tree()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    handle.hotkey("enter", &[])?;

    // Verify the destination, not the input. Chromium exposes no readable text
    // on its omnibox over AT-SPI — no Value interface and no Text content even
    // while the field holds a URL — so the read-back that the Windows and macOS
    // adapters perform against the address field has no counterpart here.
    // Proving the tab actually arrived at the fixed setup page is the stronger
    // check anyway: it fails for a mistyped URL, a hijacked search, and a
    // redirect alike, and it names what was reached instead of timing out.
    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        input.check()?;
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        handle
            .require_owned_selected_tab(&tree)
            .map_err(|error| anyhow::anyhow!(error.message))?;
        if setup_page_proven(&tree.nodes, descriptor, true) {
            handle.trusted_setup_navigation = true;
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
                 The exact setup destination could not be verified",
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
pub(crate) fn window_scoped_tree(
    pid: u32,
    window_id: u64,
) -> Result<crate::atspi::AtspiTreeResult, BrowserRefusal> {
    let tree = crate::atspi::walk_tree(pid, window_id, None);
    require_window_scope(tree.trusted, tree.window_scoped)?;
    Ok(tree)
}

fn require_window_scope(trusted: bool, window_scoped: bool) -> Result<(), BrowserRefusal> {
    if !trusted {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no trusted AT-SPI tree for the approved browser window; \
             the accessibility bus must be reachable to prove which window a control belongs to",
        ));
    }
    if !window_scoped {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "could not prove the accessibility top-level belongs to the approved browser window",
        ));
    }
    Ok(())
}

pub(crate) fn perform_exact_action(
    node: &AtspiNode,
    before_dispatch: impl Fn() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let object = node.object_ref.as_ref().ok_or_else(|| {
        anyhow::anyhow!("exact browser control has no stable accessibility object identity")
    })?;
    let (_, suspected_noop, unacknowledged) =
        crate::atspi::native::perform_action_ref_guarded(object, before_dispatch)?;
    if suspected_noop || unacknowledged {
        anyhow::bail!("exact browser accessibility action was not acknowledged");
    }
    Ok(())
}

pub struct SetupUiHandle {
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    pub(crate) input: SetupInput,
    owned_setup_tab: Option<ObjectRef>,
    trusted_setup_navigation: bool,
    input_delivery_unknown: bool,
    armed: bool,
    enabling: bool,
    enable_attempted: bool,
    trusted_checkbox_fallback_attempted: bool,
    pub opened_setup_page: bool,
    pub enabled_remote_debugging: bool,
    pub focused_setup_address_field: bool,
    pub foregrounded_window: bool,
    pub injected_global_input: bool,
}

struct AbandonedSetupLane {
    owner: String,
    _cancel: hyprland_input::CancelOnDrop,
}

impl Drop for AbandonedSetupLane {
    fn drop(&mut self) {
        hyprland_input::cleanup_session(&self.owner);
    }
}

impl Drop for SetupUiHandle {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        // A returned blocking result can be abandoned before core receives it.
        // Keep its restoration authority on the resource, not on the caller.
        // Drop may run on a Tokio executor when endpoint discovery is
        // cancelled. AT-SPI's bounded runtime must run on a blocking thread.
        std::thread::scope(|scope| {
            if let Ok(worker) = std::thread::Builder::new()
                .name("cua-abandoned-setup".into())
                .spawn_scoped(scope, || {
                    let Some(_cleanup) = self.cleanup_input() else {
                        return;
                    };
                    let _ = self.rollback_remote_debugging();
                    let _ = self.close();
                })
            {
                let _ = worker.join();
            }
        });
    }
}

impl SetupUiHandle {
    fn record_input_result(&mut self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        match result {
            Ok(()) => {
                self.foregrounded_window = true;
                self.injected_global_input = true;
                Ok(())
            }
            Err(error) => {
                self.input_delivery_unknown |= error.is::<hyprland_input::DispatchUnknown>();
                Err(error)
            }
        }
    }

    fn hotkey(&mut self, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
        let result = self.input.hotkey(self.pid, self.window_id, key, modifiers);
        self.record_input_result(result)
    }

    fn require_owned_selected_tab(&self, tree: &AtspiTreeResult) -> Result<(), BrowserRefusal> {
        let owned = self.owned_setup_tab.as_ref().ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "this setup has no independently proven created tab",
            )
        })?;
        if browser_tabs(tree)?.selected != *owned {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the selected browser tab is no longer this setup's created tab",
            ));
        }
        Ok(())
    }

    fn owned_tab_tree(&self) -> Result<AtspiTreeResult, BrowserRefusal> {
        let tree = window_scoped_tree(self.pid, self.window_id)?;
        self.require_owned_selected_tab(&tree)?;
        Ok(tree)
    }

    fn require_owned_setup_page(&self, tree: &AtspiTreeResult) -> Result<(), BrowserRefusal> {
        self.require_owned_selected_tab(tree)?;
        if !setup_page_proven(&tree.nodes, self.descriptor, self.trusted_setup_navigation) {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the owned temporary tab no longer shows the exact fixed setup page",
            ));
        }
        Ok(())
    }

    fn cleanup_input(&mut self) -> Option<AbandonedSetupLane> {
        crate::browser_platform::verify_setup_identity(
            i64::from(self.pid),
            self.window_id,
            &self.input.fingerprint,
        )
        .ok()?;
        static NEXT_ABANDON: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let owner = format!(
            "{}:browser-abandon:{}",
            self.input.owner,
            NEXT_ABANDON.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let (_cancel, cancellation) = ActionCancellation::invocation();
        self.input.lane_owner = owner.clone();
        self.input.cancellation = cancellation;
        Some(AbandonedSetupLane { owner, _cancel })
    }

    fn rollback_remote_debugging(&mut self) -> bool {
        if self.input.check().is_err() {
            return false;
        }
        if !(self.enabled_remote_debugging || (self.enable_attempted && self.enabling)) {
            return true;
        }
        let Ok(tree) = window_scoped_tree(self.pid, self.window_id) else {
            return false;
        };
        let restored =
            exact_setup_checkbox(&tree.nodes, self.descriptor, self.trusted_setup_navigation)
                .ok()
                .flatten()
                .is_some_and(|node| {
                    node.checked == Some(false)
                        || (node.checked == Some(true)
                            && perform_exact_action(node, || self.input.check()).is_ok())
                });
        let deadline = Instant::now() + Duration::from_secs(1);
        while restored && self.input.check().is_ok() {
            if window_scoped_tree(self.pid, self.window_id)
                .ok()
                .and_then(|tree| {
                    exact_setup_checkbox(
                        &tree.nodes,
                        self.descriptor,
                        self.trusted_setup_navigation,
                    )
                    .ok()
                    .flatten()
                    .map(|node| node.checked == Some(false))
                })
                == Some(true)
            {
                self.enabled_remote_debugging = false;
                return true;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn abort(mut self, error: BrowserRefusal) -> BrowserRefusal {
        self.armed = false;
        let _cleanup = if self.input.check().is_err() {
            self.cleanup_input()
        } else {
            None
        };
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
                "input_delivery_unknown": self.input_delivery_unknown,
                "restored_remote_debugging": restored_remote_debugging,
            },
            "cause": cause,
        }))
    }

    pub fn close_for_success(mut self) -> Result<Option<bool>, BrowserRefusal> {
        if !self.opened_setup_page {
            self.armed = false;
            return Ok(None);
        }
        let tree = match window_scoped_tree(self.pid, self.window_id) {
            Ok(tree) => tree,
            Err(error) => return Err(self.abort(error)),
        };
        if let Err(error) = self.require_owned_setup_page(&tree) {
            return Err(self.abort(error));
        }
        if let Err(error) = close_tab(self.pid, self.window_id, &self.input) {
            let error = refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("could not close the exact temporary setup tab: {error}"),
            );
            return Err(self.abort(error));
        }
        self.opened_setup_page = false;
        self.owned_setup_tab = None;
        self.armed = false;
        Ok(Some(true))
    }

    fn close(&mut self) -> Option<bool> {
        if !self.opened_setup_page {
            return None;
        }
        let Ok(tree) = window_scoped_tree(self.pid, self.window_id) else {
            return Some(false);
        };
        let closed = self.require_owned_setup_page(&tree).is_ok()
            && close_tab(self.pid, self.window_id, &self.input).is_ok();
        self.opened_setup_page &= !closed;
        if closed {
            self.owned_setup_tab = None;
        }
        Some(closed)
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

pub fn commit_pending(pid: u32, window_id: u64, input: SetupInput) -> Result<bool, BrowserRefusal> {
    let mut pending = pending_setups().lock().unwrap();
    if pending
        .get(&(pid, window_id))
        .is_some_and(|handle| handle.input.owner != input.owner)
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "pending browser setup belongs to another admitted session",
        ));
    }
    let mut handle = pending.remove(&(pid, window_id)).ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            "the exact pending browser setup cleanup handle is missing",
        )
    })?;
    drop(pending);
    handle.input = input;
    Ok(handle.close_for_success()?.unwrap_or(false))
}

pub fn abort_pending(
    pid: u32,
    window_id: u64,
    error: BrowserRefusal,
    input: SetupInput,
) -> BrowserRefusal {
    let mut pending = pending_setups().lock().unwrap();
    if pending
        .get(&(pid, window_id))
        .is_some_and(|handle| handle.input.owner != input.owner)
    {
        return error;
    }
    let handle = pending.remove(&(pid, window_id));
    drop(pending);
    match handle {
        Some(mut handle) => {
            handle.input = input;
            handle.abort(error)
        }
        None => error.with_detail(serde_json::json!({
            "setup_cleanup": "the exact pending browser setup cleanup handle was missing"
        })),
    }
}

pub(crate) fn discard_pending(pid: u32, window_id: u64, owner: &str) {
    let mut pending = pending_setups().lock().unwrap();
    let handle = if pending
        .get(&(pid, window_id))
        .is_some_and(|handle| handle.input.owner == owner)
    {
        pending.remove(&(pid, window_id))
    } else {
        None
    };
    drop(pending);
    drop(handle);
}

fn set_remote_debugging(
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    desired_enabled: bool,
    input: SetupInput,
) -> Result<SetupUiHandle, BrowserRefusal> {
    input.check().map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            error.to_string(),
        )
    })?;
    let initial = window_scoped_tree(pid, window_id)?;
    let initial_checkbox = exact_setup_checkbox(&initial.nodes, descriptor, false)?;
    let mut handle = SetupUiHandle {
        pid,
        window_id,
        descriptor,
        input: input.clone(),
        owned_setup_tab: None,
        trusted_setup_navigation: false,
        input_delivery_unknown: false,
        armed: true,
        enabling: desired_enabled,
        enable_attempted: false,
        trusted_checkbox_fallback_attempted: false,
        opened_setup_page: false,
        enabled_remote_debugging: false,
        focused_setup_address_field: false,
        foregrounded_window: false,
        injected_global_input: false,
    };
    if initial_checkbox.is_none() {
        if let Err(error) = trusted_setup_navigation(&mut handle, &initial) {
            return Err(handle.abort(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!(
                    "could not navigate the exact {} window to its fixed setup page: {error}",
                    descriptor.product_name
                ),
            )));
        }
    }

    let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
    loop {
        if let Err(error) = input.check() {
            return Err(handle.abort(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                error.to_string(),
            )));
        }
        let tree = match window_scoped_tree(pid, window_id) {
            Ok(tree) => tree,
            Err(error) => return Err(handle.abort(error)),
        };
        match exact_setup_checkbox(&tree.nodes, descriptor, handle.trusted_setup_navigation) {
            Ok(Some(node)) => match node.checked {
                Some(state) if state == desired_enabled => {
                    if desired_enabled && handle.enable_attempted {
                        handle.enabled_remote_debugging = true;
                    } else if !desired_enabled {
                        handle.enabled_remote_debugging = false;
                    }
                    return Ok(handle);
                }
                Some(_) if !handle.enable_attempted => {
                    handle.enable_attempted = true;
                    if let Err(error) = perform_exact_action(node, || input.check()) {
                        return Err(handle.abort(refusal(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            format!("the exact checkbox action failed: {error}"),
                        )));
                    }
                }
                Some(_)
                    if descriptor.product == BrowserProduct::MicrosoftEdge
                        && !handle.trusted_checkbox_fallback_attempted =>
                {
                    handle.trusted_checkbox_fallback_attempted = true;
                    handle.foregrounded_window = true;
                    let trusted_navigation = handle.trusted_setup_navigation;
                    if input.hyprland {
                        return Err(handle.abort(refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            "trusted Edge checkbox fallback is not qualified on Hyprland",
                        )));
                    }
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
                        if checkbox.checked == Some(desired_enabled) {
                            return Ok(false);
                        }
                        if checkbox.checked != Some(!desired_enabled) {
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
                            // AT-SPI screen bounds are layout coordinates, as in
                            // browser_consent_ui's click_focused path, not
                            // desktop-frame points, so no frame conversion applies.
                            let space = crate::wayland::DesktopInputSpace::default();
                            crate::wayland::click_desktop(&space, center_x, center_y, 1, 1)?;
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
                Some(_) => {}
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

pub fn enable(
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    input: SetupInput,
) -> Result<SetupUiHandle, BrowserRefusal> {
    set_remote_debugging(pid, window_id, descriptor, true, input)
}

pub fn disable(
    pid: u32,
    window_id: u64,
    descriptor: &'static BrowserSetupDescriptor,
    input: SetupInput,
) -> Result<bool, BrowserRefusal> {
    let handle = set_remote_debugging(pid, window_id, descriptor, false, input)?;
    Ok(handle.close_for_success()?.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::browser::{existing_profile_setup_descriptor, BrowserProduct};

    #[test]
    fn semantic_setup_refuses_an_ending_lifecycle() {
        use cua_driver_core::session::{self, SessionClientKind, SessionTransport};
        let sid = "setup-semantic-end-private";
        let transport = "setup-semantic-end-transport";
        let guard = session::begin_session_dispatch(
            sid,
            None,
            transport,
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();
        let input = SetupInput {
            owner: sid.to_owned(),
            lane_owner: sid.to_owned(),
            hyprland: true,
            fingerprint: cua_driver_core::browser::ProcessFingerprint {
                pid: 0,
                start_time: Some(0),
                executable: None,
            },
            cancellation: ActionCancellation::default(),
        };
        assert!(input.check().is_ok());
        assert!(session::end_session_for_owner(sid, transport));
        assert!(input.check().is_err());
        drop(guard);
        assert!(session::is_session_ended(sid));
    }

    #[test]
    fn matching_owner_retires_pending_setup_without_touching_a_foreign_handle() {
        let pid = 0;
        let window = 0x5156;
        let owner = "pending-setup-private-owner";
        let handle = SetupUiHandle {
            pid,
            window_id: window,
            descriptor: descriptor(),
            input: SetupInput {
                owner: owner.to_owned(),
                lane_owner: owner.to_owned(),
                hyprland: true,
                fingerprint: cua_driver_core::browser::ProcessFingerprint {
                    pid: 0,
                    start_time: Some(0),
                    executable: None,
                },
                cancellation: ActionCancellation::default(),
            },
            owned_setup_tab: None,
            trusted_setup_navigation: false,
            input_delivery_unknown: false,
            armed: false,
            enabling: true,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
        };
        retain_pending(pid, window, handle).unwrap();
        discard_pending(pid, window, "another-private-owner");
        assert!(pending_setups()
            .lock()
            .unwrap()
            .contains_key(&(pid, window)));
        discard_pending(pid, window, owner);
        assert!(!pending_setups()
            .lock()
            .unwrap()
            .contains_key(&(pid, window)));
    }

    #[test]
    fn setup_and_cleanup_require_a_trusted_window_scoped_tree() {
        assert!(require_window_scope(true, true).is_ok());
        assert_eq!(
            require_window_scope(true, false).unwrap_err().code,
            BrowserRefusalCode::BrowserBindingAmbiguous
        );
        assert_eq!(
            require_window_scope(false, true).unwrap_err().code,
            BrowserRefusalCode::BrowserRouteUnavailable
        );
    }

    #[test]
    fn setup_requires_success_on_the_exact_foreground_route() {
        use serde_json::json;
        assert!(foreground_ack(json!({"ok": true, "route": "primary_foreground"})).is_ok());
        for reply in [
            json!({"ok": false, "code": "physical_input_busy", "route": "primary_foreground"}),
            json!({"ok": true, "route": "synthetic_events"}),
            json!({"ok": true}),
            json!({"route": "primary_foreground"}),
        ] {
            assert!(foreground_ack(reply).is_err());
        }
    }

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
            identity: None,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
            object_ref: None,
        }
    }

    fn tab(path: &str, selected: bool) -> AtspiNode {
        let mut tab = node("page tab", path, None, &["activate"]);
        tab.selected = Some(selected);
        tab.object_ref = Some(ObjectRef {
            bus: ":1.5156".to_owned(),
            path: path.to_owned(),
        });
        tab
    }

    fn setup_tree(tabs: Vec<AtspiNode>) -> AtspiTreeResult {
        let mut nodes = tabs;
        nodes.extend([
            node(
                "entry",
                "Address and search bar",
                Some(descriptor().setup_url),
                &["activate"],
            ),
            node("document web", descriptor().page_titles[0], None, &[]),
            node("heading", descriptor().page_heading, None, &[]),
        ]);
        AtspiTreeResult {
            tree_markdown: String::new(),
            nodes_visited: nodes.len(),
            nodes,
            bounds: Vec::new(),
            trusted: true,
            degraded_reason: None,
            window_scoped: true,
            truncated: false,
            truncation_reason: None,
            nodes_pending: 0,
            bounds_complete: true,
            elapsed_ms: 0,
        }
    }

    fn inert_setup_handle() -> SetupUiHandle {
        SetupUiHandle {
            pid: 0,
            window_id: 0,
            descriptor: descriptor(),
            input: SetupInput {
                owner: "setup-ownership-private".to_owned(),
                lane_owner: "setup-ownership-private".to_owned(),
                fingerprint: cua_driver_core::browser::ProcessFingerprint {
                    pid: 0,
                    start_time: Some(0),
                    executable: None,
                },
                hyprland: true,
                cancellation: ActionCancellation::default(),
            },
            owned_setup_tab: None,
            trusted_setup_navigation: false,
            input_delivery_unknown: false,
            armed: false,
            enabling: true,
            enable_attempted: false,
            trusted_checkbox_fallback_attempted: false,
            opened_setup_page: false,
            enabled_remote_debugging: false,
            focused_setup_address_field: false,
            foregrounded_window: false,
            injected_global_input: false,
        }
    }

    #[test]
    fn refused_first_shortcut_cannot_cleanup_a_preexisting_setup_page() {
        let existing = setup_tree(vec![tab("/existing", true)]);
        assert!(setup_page_proven(&existing.nodes, descriptor(), false));
        assert!(exact_setup_checkbox(&existing.nodes, descriptor(), false)
            .unwrap()
            .is_none());
        let mut handle = inert_setup_handle();
        let (cancel, cancellation) = ActionCancellation::invocation();
        handle.input.cancellation = cancellation;
        drop(cancel);

        // Exercise the real navigation boundary: the first shortcut refuses
        // before native delivery, despite the pre-existing fixed page labels.
        assert!(trusted_setup_navigation(&mut handle, &existing).is_err());
        assert!(!handle.opened_setup_page);
        assert!(handle.owned_setup_tab.is_none());
        assert!(!handle.foregrounded_window);
        assert!(!handle.injected_global_input);
        assert_eq!(
            handle.require_owned_setup_page(&existing).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(handle.close_for_success().unwrap(), None);

        let mut refused = inert_setup_handle();
        assert!(refused
            .record_input_result(foreground_ack(serde_json::json!({
                "ok": false,
                "code": "primary_target_busy",
                "detail": "foreground_grab",
            })))
            .is_err());
        assert!(!refused.foregrounded_window);
        assert!(!refused.injected_global_input);
        assert_eq!(refused.close(), None);
    }

    #[test]
    fn unknown_or_acknowledged_input_alone_cannot_authorize_setup_cleanup() {
        let existing = setup_tree(vec![tab("/existing", true)]);
        let mut handle = inert_setup_handle();
        assert!(handle
            .record_input_result(Err(hyprland_input::unknown_dispatch(
                anyhow::anyhow!("shortcut final reply lost"),
                0,
            )))
            .is_err());
        assert!(handle.input_delivery_unknown);
        assert!(!handle.foregrounded_window);
        assert!(!handle.injected_global_input);
        assert!(!handle.opened_setup_page);
        assert!(handle.owned_setup_tab.is_none());
        assert_eq!(
            handle.require_owned_setup_page(&existing).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(handle.close(), None);

        // An acknowledged shortcut records delivery, not ownership or arrival.
        handle.record_input_result(Ok(())).unwrap();
        assert!(handle.foregrounded_window);
        assert!(handle.injected_global_input);
        assert!(!handle.opened_setup_page);
        assert!(!handle.trusted_setup_navigation);
        assert_eq!(
            handle.require_owned_setup_page(&existing).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        assert_eq!(handle.close_for_success().unwrap(), None);
    }

    #[test]
    fn owned_setup_cleanup_requires_the_same_uniquely_selected_created_tab() {
        let baseline = setup_tree(vec![tab("/existing", true)]);
        let current = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        let mut handle = inert_setup_handle();
        handle.owned_setup_tab = created_tab(
            &browser_tabs(&baseline).unwrap(),
            &browser_tabs(&current).unwrap(),
        )
        .unwrap();
        handle.opened_setup_page = true;
        handle.trusted_setup_navigation = true;
        assert!(handle.require_owned_setup_page(&current).is_ok());

        let drifted = setup_tree(vec![tab("/existing", true), tab("/created", false)]);
        assert_eq!(
            handle.require_owned_setup_page(&drifted).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        let ambiguous = setup_tree(vec![tab("/existing", true), tab("/created", true)]);
        assert_eq!(
            handle
                .require_owned_setup_page(&ambiguous)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserBindingAmbiguous
        );
        let missing = setup_tree(vec![tab("/existing", true)]);
        assert!(handle.require_owned_setup_page(&missing).is_err());

        let mut incomplete = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        incomplete.truncated = true;
        assert_eq!(
            handle
                .require_owned_setup_page(&incomplete)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserRouteUnavailable
        );
        let mut another_page = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        another_page
            .nodes
            .retain(|node| role_is(node, &["page tab"]));
        assert!(handle.require_owned_setup_page(&another_page).is_err());
    }

    #[test]
    fn setup_tab_creation_requires_a_complete_preserved_native_tab_baseline() {
        let baseline = browser_tabs(&setup_tree(vec![tab("/existing", true)])).unwrap();
        let unchanged = setup_tree(vec![tab("/existing", true)]);
        assert!(created_tab(&baseline, &browser_tabs(&unchanged).unwrap())
            .unwrap()
            .is_none());
        let replaced = setup_tree(vec![tab("/created", true)]);
        assert_eq!(
            created_tab(&baseline, &browser_tabs(&replaced).unwrap())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        let extra = setup_tree(vec![
            tab("/existing", false),
            tab("/created", true),
            tab("/unexpected", false),
        ]);
        assert_eq!(
            created_tab(&baseline, &browser_tabs(&extra).unwrap())
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserBindingAmbiguous
        );
        let mut incomplete = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        incomplete.truncated = true;
        assert_eq!(
            browser_tabs(&incomplete).unwrap_err().code,
            BrowserRefusalCode::BrowserRouteUnavailable
        );
        let mut unidentified = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        unidentified.nodes[1].object_ref = None;
        assert_eq!(
            browser_tabs(&unidentified).unwrap_err().code,
            BrowserRefusalCode::BrowserRouteUnavailable
        );

        let mut web_tab = tab("/web-content-tab", true);
        web_tab.in_web_content = true;
        let web_content = setup_tree(vec![tab("/existing", true), web_tab]);
        assert!(created_tab(&baseline, &browser_tabs(&web_content).unwrap())
            .unwrap()
            .is_none());
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
