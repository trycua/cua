//! Exact Linux AT-SPI setup for Chromium existing-profile attachment.

use std::time::{Duration, Instant};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
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
    pub focus: FocusRestore,
}

fn foreground_ack(value: serde_json::Value) -> anyhow::Result<()> {
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
        && value.get("route").and_then(serde_json::Value::as_str) == Some("primary_foreground")
    {
        return Ok(());
    }
    // A refusal after some packets landed still activated the window and
    // typed into it; report it as possibly delivered, never as a clean refusal.
    if value.get("effect").and_then(serde_json::Value::as_str) == Some("partial") {
        let delivered = value
            .pointer("/delivery/delivered_count")
            .and_then(serde_json::Value::as_u64)
            .map_or(1, |count| u32::try_from(count).unwrap_or(u32::MAX));
        return Err(hyprland_input::unknown_dispatch(
            anyhow::anyhow!("exact Hyprland setup input was partially delivered: {value}"),
            delivered,
        ));
    }
    anyhow::bail!("exact Hyprland setup input refused: {value}")
}

/// The person's Hyprland focus and pointer, captured once before a setup
/// transaction's first foreground packet and handed back once when that
/// transaction ends. The plugin's per-packet activation deliberately persists;
/// this is the single restoration point for the whole Rust transaction.
#[derive(Clone, Default)]
pub(crate) struct FocusRestore(Arc<Mutex<Option<PriorFocus>>>);

struct PriorFocus {
    pid: u32,
    target: u64,
    window: Option<u64>,
    workspace: Option<i64>,
    cursor: (f64, f64),
    /// Proof that the person gave no input during the transaction. `None`
    /// (unsupported, no quiet barrier, watcher failure) never restores.
    input: Option<PersonProof>,
}

/// How a transaction proves the person left keyboard and pointer alone.
enum PersonProof {
    /// The Cua plugin's count of deliberate external input at capture. It
    /// ignores compositor warps and input-method modifier updates, which
    /// fire when focus moves to a text field without anyone typing.
    ExternalInput(u64),
    /// Fallback for an older plugin: ext-idle-notify, which also counts
    /// input-method modifier updates and so can skip a valid hand-back.
    Idle(crate::wayland::input_quiet::InputQuiet),
}

impl PersonProof {
    fn begin() -> Option<Self> {
        if let Ok(Some(count)) = crate::wayland::hyprland::plugin_external_input_count() {
            return Some(Self::ExternalInput(count));
        }
        crate::wayland::input_quiet::InputQuiet::begin(Duration::from_millis(400))
            .map(Self::Idle)
            .map_err(
                |error| tracing::warn!(%error, "focus hand-back disabled: no physical-input watch"),
            )
            .ok()
    }

    fn quiet(&mut self, budget: Duration) -> bool {
        match self {
            Self::ExternalInput(captured) => person_quiet_by_count(
                *captured,
                crate::wayland::hyprland::plugin_external_input_count()
                    .ok()
                    .flatten(),
            ),
            Self::Idle(watch) => watch.quiet_since_begin(budget),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::ExternalInput(_) => "plugin_external_input",
            Self::Idle(_) => "idle_notify",
        }
    }

    fn first_input_after_ms(&self) -> Option<u128> {
        match self {
            Self::ExternalInput(_) => None,
            Self::Idle(watch) => watch.first_input_after_ms(),
        }
    }
}

/// Quiet only when the plugin's count is readable and unchanged since
/// capture; an unreadable count is never proof.
fn person_quiet_by_count(captured: u64, now: Option<u64>) -> bool {
    now == Some(captured)
}

/// Restore only when focus provably still sits on the setup target and the
/// prior window is another window that still exists, with the pointer left
/// where the person or the compositor's focus warp put it. Anything else
/// means the person has moved on, and their choice wins.
fn focus_to_restore(
    target: u64,
    prior: Option<u64>,
    target_active: impl FnOnce() -> bool,
    prior_exists: impl FnOnce(u64) -> bool,
    pointer_unmoved: impl FnOnce() -> bool,
) -> Option<u64> {
    let prior = prior.filter(|prior| *prior != target)?;
    (target_active() && prior_exists(prior) && pointer_unmoved()).then_some(prior)
}

/// With no prior window (an empty workspace), hand back the workspace itself
/// under the same conditions, when the setup left the person elsewhere.
fn workspace_to_restore(
    prior_window: Option<u64>,
    prior_workspace: Option<i64>,
    target_active: impl FnOnce() -> bool,
    pointer_unmoved: impl FnOnce() -> bool,
    current_workspace: impl FnOnce() -> Option<i64>,
) -> Option<i64> {
    if prior_window.is_some() {
        return None;
    }
    let prior = prior_workspace?;
    (target_active() && pointer_unmoved() && current_workspace().is_some_and(|now| now != prior))
        .then_some(prior)
}

fn same_pointer(captured: (f64, f64), current: (f64, f64)) -> bool {
    (captured.0 - current.0).abs() < 1.0 && (captured.1 - current.1).abs() < 1.0
}

/// With `cursor:no_warps` off, Hyprland warps the pointer to the centre of a
/// window it focuses, including one that took focus through xdg-activation.
/// A pointer resting exactly there was moved by the compositor, not the person.
fn compositor_warped_pointer(window: (i32, i32, u32, u32), current: (f64, f64)) -> bool {
    let centre = (
        f64::from(window.0) + f64::from(window.2) / 2.0,
        f64::from(window.1) + f64::from(window.3) / 2.0,
    );
    (centre.0 - current.0).abs() <= 1.0 && (centre.1 - current.1).abs() <= 1.0
}

fn pointer_left_alone(
    captured: (f64, f64),
    target: Option<(i32, i32, u32, u32)>,
    current: (f64, f64),
) -> bool {
    same_pointer(captured, current)
        || target.is_some_and(|window| compositor_warped_pointer(window, current))
}

impl FocusRestore {
    fn capture(&self, pid: u32, target: u64) -> anyhow::Result<()> {
        let mut prior = self.0.lock().unwrap();
        if prior.is_none() {
            // Prove first: the proof must precede the state it protects, or
            // a switch the person made meanwhile would be undone.
            let mut input = PersonProof::begin();
            let window = crate::wayland::hyprland::active_window_address()?;
            let workspace = crate::wayland::hyprland::single_output_workspace()?;
            let cursor = crate::wayland::hyprland::cursor_position()?;
            if input
                .as_mut()
                .is_some_and(|proof| !proof.quiet(Duration::from_millis(200)))
            {
                tracing::warn!("focus hand-back disabled: the person acted during capture");
                input = None;
            }
            *prior = Some(PriorFocus {
                pid,
                target,
                window,
                workspace,
                cursor,
                input,
            });
        }
        Ok(())
    }

    /// Guard an endpoint claim whose browser-owned consent prompt the
    /// compositor may focus by itself (xdg-activation with focus-on-activate).
    /// The driver sends no input there, so a pointer the person moved since
    /// capture also means they moved on.
    pub(crate) fn guard_consent_prompt(pid: u32, target: u64) -> Option<RestoreFocusOnDrop> {
        let focus = Self::default();
        if let Err(error) = focus.capture(pid, target) {
            tracing::warn!(%error, "could not capture focus before browser consent");
            return None;
        }
        Some(focus.restore_on_drop())
    }

    fn restore(&self) {
        let Some(mut prior) = self.0.lock().unwrap().take() else {
            return;
        };
        // Location is only corroboration: without proof that the person did
        // not touch keyboard or pointer since capture, their focus is theirs.
        let had_watch = prior.input.is_some();
        let proof = prior.input.as_ref().map_or("none", PersonProof::kind);
        if !prior
            .input
            .as_mut()
            .is_some_and(|proof| proof.quiet(Duration::from_millis(500)))
        {
            let first_input_after_ms = prior
                .input
                .as_ref()
                .and_then(PersonProof::first_input_after_ms);
            // Visible at the daemon's default level only when the browser
            // kept focus the person had held: the case worth explaining.
            let browser_took_focus = prior.window != Some(prior.target)
                && crate::wayland::hyprland::target_is_active(prior.target, Some(prior.pid))
                    .unwrap_or(false);
            if browser_took_focus {
                tracing::warn!(
                    had_watch,
                    proof,
                    first_input_after_ms,
                    "focus hand-back skipped after the browser took focus: the person used the \
                     desktop or input quiet is unproven"
                );
            } else {
                tracing::info!(
                    had_watch,
                    proof,
                    first_input_after_ms,
                    "focus hand-back skipped: the person used the desktop or input quiet is unproven"
                );
            }
            return;
        }
        let target_active = || {
            crate::wayland::hyprland::target_is_active(prior.target, Some(prior.pid))
                .unwrap_or(false)
        };
        // Packet activation and xdg-activation both let Hyprland warp the
        // pointer to the target's centre; any other move is the person's.
        let pointer_unmoved = || {
            crate::wayland::hyprland::cursor_position().is_ok_and(|current| {
                let target = crate::wayland::hyprland::window_for_address(prior.target)
                    .map(|window| (window.x, window.y, window.width, window.height));
                pointer_left_alone(prior.cursor, target, current)
            })
        };
        let plan = if let Some(window) = focus_to_restore(
            prior.target,
            prior.window,
            target_active,
            |window| crate::wayland::hyprland::window_for_address(window).is_some(),
            pointer_unmoved,
        ) {
            HandBack::Window(window)
        } else if let Some(workspace) = workspace_to_restore(
            prior.window,
            prior.workspace,
            target_active,
            pointer_unmoved,
            || {
                crate::wayland::hyprland::single_output_workspace()
                    .ok()
                    .flatten()
            },
        ) {
            HandBack::Workspace(workspace)
        } else {
            // One content-free line per transaction, so a skipped hand-back
            // is diagnosable without debug logging.
            let prior_was_browser = prior.window == Some(prior.target);
            let browser_active = target_active();
            let pointer_left_alone = pointer_unmoved();
            if browser_active && prior.window.is_some() && !prior_was_browser {
                tracing::warn!(
                    prior_window_present = true,
                    browser_active,
                    pointer_left_alone,
                    "focus hand-back skipped after the browser took focus"
                );
            } else {
                tracing::info!(
                    prior_was_browser,
                    prior_window_present = prior.window.is_some(),
                    browser_active,
                    pointer_left_alone,
                    "focus hand-back skipped: focus is where the person or browser left it"
                );
            }
            return;
        };
        let Some(input) = prior.input.as_mut() else {
            return;
        };
        match hand_back(plan, prior.cursor, &mut HyprlandHandBack { input }) {
            Ok(HandBackOutcome::Restored) => {
                tracing::info!("restored prior focus and pointer after browser setup or consent")
            }
            Ok(HandBackOutcome::PersonActed) => {
                tracing::warn!("focus hand-back stopped: the person acted during it")
            }
            Ok(HandBackOutcome::FocusStayed { realigned }) => tracing::warn!(
                realigned,
                "Hyprland accepted the focus hand-back but keyboard focus stayed; the \
                 visible workspace was returned to the focused window"
            ),
            Err(error) => {
                tracing::warn!(%error, "could not restore prior focus after browser setup or consent")
            }
        }
    }

    pub(crate) fn restore_on_drop(&self) -> RestoreFocusOnDrop {
        RestoreFocusOnDrop(self.clone())
    }
}

#[derive(Clone, Copy)]
enum HandBack {
    Window(u64),
    Workspace(i64),
}

#[derive(Debug, PartialEq)]
enum HandBackOutcome {
    Restored,
    FocusStayed { realigned: bool },
    PersonActed,
}

/// The compositor operations a hand-back needs, separable for tests.
trait HandBackOps {
    fn focus_window(&mut self, address: u64) -> anyhow::Result<()>;
    fn show_workspace(&mut self, id: i64) -> anyhow::Result<()>;
    fn move_pointer(&mut self, at: (f64, f64)) -> anyhow::Result<()>;
    fn active_window(&mut self) -> anyhow::Result<Option<u64>>;
    fn window_workspace(&mut self, address: u64) -> Option<i64>;
    fn visible_workspace(&mut self) -> Option<i64>;
    fn settle(&mut self);
    /// Fresh proof, immediately before a change, that the person has not
    /// touched keyboard or pointer since the transaction began.
    fn person_quiet(&mut self) -> bool;
}

struct HyprlandHandBack<'a> {
    input: &'a mut PersonProof,
}

impl HandBackOps for HyprlandHandBack<'_> {
    fn focus_window(&mut self, address: u64) -> anyhow::Result<()> {
        crate::wayland::hyprland::restore_focus_to_window(address)
    }
    fn show_workspace(&mut self, id: i64) -> anyhow::Result<()> {
        crate::wayland::hyprland::restore_workspace(id)
    }
    fn move_pointer(&mut self, at: (f64, f64)) -> anyhow::Result<()> {
        crate::wayland::hyprland::move_cursor(at.0, at.1)
    }
    fn active_window(&mut self) -> anyhow::Result<Option<u64>> {
        crate::wayland::hyprland::active_window_address()
    }
    fn window_workspace(&mut self, address: u64) -> Option<i64> {
        crate::wayland::hyprland::window_workspace(address)
    }
    fn visible_workspace(&mut self) -> Option<i64> {
        crate::wayland::hyprland::single_output_workspace()
            .ok()
            .flatten()
    }
    fn settle(&mut self) {
        std::thread::sleep(Duration::from_millis(50));
    }
    fn person_quiet(&mut self) -> bool {
        self.input.quiet(Duration::from_millis(200))
    }
}

/// Hand focus back and prove it. A dispatcher's `ok` is not focus: on
/// Hyprland 0.56 a window focus was observed to switch the visible workspace
/// while keyboard focus stayed on the browser, leaving the person typing
/// into a window they could not see. When focus does not follow, put the
/// visible workspace back under the focused window instead, and never move
/// the pointer on an unproven restore.
fn hand_back(
    plan: HandBack,
    pointer: (f64, f64),
    ops: &mut impl HandBackOps,
) -> anyhow::Result<HandBackOutcome> {
    if !ops.person_quiet() {
        return Ok(HandBackOutcome::PersonActed);
    }
    match plan {
        HandBack::Window(window) => ops.focus_window(window)?,
        HandBack::Workspace(id) => ops.show_workspace(id)?,
    }
    // Keyboard focus and the visible workspace must agree with the plan: the
    // prior window focused, or the prior (empty) workspace shown with no
    // window focused anywhere else.
    let consistent = |ops: &mut dyn HandBackOps| -> anyhow::Result<(bool, Option<u64>)> {
        let active = ops.active_window()?;
        let ok = match plan {
            HandBack::Window(window) => active == Some(window),
            HandBack::Workspace(id) => {
                ops.visible_workspace() == Some(id)
                    && active.is_none_or(|window| ops.window_workspace(window) == Some(id))
            }
        };
        Ok((ok, active))
    };
    let (mut ok, mut active) = consistent(ops)?;
    for _ in 0..10 {
        if ok {
            break;
        }
        ops.settle();
        (ok, active) = consistent(ops)?;
    }
    if !ok {
        let focused_workspace = active.and_then(|window| ops.window_workspace(window));
        let realigned = match focused_workspace {
            Some(id) if ops.visible_workspace() != Some(id) => {
                if !ops.person_quiet() {
                    return Ok(HandBackOutcome::PersonActed);
                }
                ops.show_workspace(id).is_ok()
            }
            _ => false,
        };
        return Ok(HandBackOutcome::FocusStayed { realigned });
    }
    if !ops.person_quiet() {
        return Ok(HandBackOutcome::PersonActed);
    }
    ops.move_pointer(pointer)?;
    // With follow-mouse focus the pointer move can itself refocus whatever
    // lies under it; say so rather than claim a restore.
    if !consistent(ops)?.0 {
        anyhow::bail!("focus moved again after the pointer was restored");
    }
    Ok(HandBackOutcome::Restored)
}

/// Order for every owned-tab keystroke: finish anything slow (the focus
/// capture waits for input to go quiet), then recheck that the owned tab is
/// still selected, then send at once. A tab the person selects during the
/// slow step is seen by the recheck instead of receiving the key.
fn recheck_then_send<C, T>(
    context: &mut C,
    ready: impl FnOnce(&mut C) -> anyhow::Result<()>,
    recheck: impl FnOnce(&mut C) -> anyhow::Result<()>,
    send: impl FnOnce(&mut C) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    ready(context)?;
    recheck(context)?;
    send(context)
}

/// Ends a setup transaction on every path, including errors and panics.
pub(crate) struct RestoreFocusOnDrop(FocusRestore);

impl Drop for RestoreFocusOnDrop {
    fn drop(&mut self) {
        self.0.restore();
    }
}

impl SetupInput {
    fn check(&self) -> anyhow::Result<()> {
        self.cancellation.check()?;
        if cua_driver_core::session::is_session_ending(&self.lane_owner) {
            anyhow::bail!("browser setup lifecycle is ending");
        }
        Ok(())
    }

    /// Finish the one-time focus capture before an owned-tab recheck: the
    /// capture waits for the person's input to go quiet, and a tab switch
    /// during that wait must be seen by the recheck, not follow it.
    fn ready(&self, pid: u32, window_id: u64) -> anyhow::Result<()> {
        if self.hyprland {
            self.focus.capture(pid, window_id)?;
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
            self.focus.capture(pid, window_id)?;
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
        self.focus.capture(pid, window_id)?;
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

/// Every identified native tab in one walk. Only proven ambiguity fails here;
/// incomplete evidence is reported so callers decide whether it is transient.
struct TabScan {
    objects: Vec<ObjectRef>,
    selected: Vec<ObjectRef>,
    truncated: bool,
    unidentified: bool,
    unknown_state: bool,
}

fn scan_tabs(tree: &AtspiTreeResult) -> Result<TabScan, BrowserRefusal> {
    require_window_scope(tree.trusted, tree.window_scoped)?;
    let mut scan = TabScan {
        objects: Vec::new(),
        selected: Vec::new(),
        truncated: tree.truncated,
        unidentified: false,
        unknown_state: false,
    };
    for tab in tree
        .nodes
        .iter()
        .filter(|node| !node.in_web_content && role_is(node, &["page tab", "tab", "tab item"]))
    {
        let Some(object) = tab
            .object_ref
            .as_ref()
            .filter(|object| !object.bus.is_empty() && !object.path.is_empty())
        else {
            scan.unidentified = true;
            continue;
        };
        if scan.objects.contains(object) {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "duplicate browser tab identities prevent exact setup ownership",
            ));
        }
        match tab.selected {
            Some(true) => scan.selected.push(object.clone()),
            Some(false) => {}
            None => scan.unknown_state = true,
        }
        scan.objects.push(object.clone());
    }
    Ok(scan)
}

fn browser_tabs(tree: &AtspiTreeResult) -> Result<BrowserTabs, BrowserRefusal> {
    let scan = scan_tabs(tree)?;
    let unavailable = |message| {
        Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            message,
        ))
    };
    if scan.truncated {
        return unavailable("a complete browser tab tree is required to prove setup tab ownership");
    }
    if scan.unidentified {
        return unavailable("a browser tab has no stable accessibility object identity");
    }
    if scan.unknown_state {
        return unavailable("a browser tab has no verifiable selected state");
    }
    match <[ObjectRef; 1]>::try_from(scan.selected) {
        Ok([selected]) => Ok(BrowserTabs {
            objects: scan.objects,
            selected,
        }),
        Err(selected) if selected.is_empty() => {
            unavailable("the browser exposes no uniquely selected tab identity")
        }
        Err(_) => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "multiple browser tabs claim to be selected",
        )),
    }
}

#[derive(Debug, PartialEq)]
enum CreatedTab {
    /// Not proven yet: Chromium publishes tab state asynchronously.
    Pending { added: usize },
    /// Exactly one added tab, uniquely selected in a complete walk.
    Selected(ObjectRef),
}

/// One walk's evidence after the new-tab shortcut. Only a lost baseline tab
/// or more than one added tab is final; every other state may still settle.
fn created_tab(
    baseline: &BrowserTabs,
    tree: &AtspiTreeResult,
) -> Result<CreatedTab, BrowserRefusal> {
    let scan = scan_tabs(tree)?;
    let added = scan
        .objects
        .iter()
        .filter(|object| !baseline.objects.contains(object))
        .collect::<Vec<_>>();
    if added.len() > 1 {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "setup did not create exactly one browser tab",
        ));
    }
    let complete = !scan.truncated && !scan.unidentified;
    if complete
        && baseline
            .objects
            .iter()
            .any(|object| !scan.objects.contains(object))
    {
        return Err(refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "a pre-existing browser tab changed while setup was creating its tab",
        ));
    }
    match (added.as_slice(), scan.selected.as_slice()) {
        ([object], [selected]) if complete && !scan.unknown_state && *object == selected => {
            Ok(CreatedTab::Selected((*object).clone()))
        }
        _ => Ok(CreatedTab::Pending { added: added.len() }),
    }
}

/// Ownership needs the same added tab uniquely selected in two consecutive
/// walks, so one racing snapshot cannot grant cleanup authority.
#[derive(Default)]
struct CreatedTabProof {
    candidate: Option<ObjectRef>,
    added: usize,
    selection_seen: bool,
}

impl CreatedTabProof {
    fn observe(&mut self, step: CreatedTab) -> Option<ObjectRef> {
        match step {
            CreatedTab::Selected(object) => {
                (self.added, self.selection_seen) = (1, true);
                if self.candidate.as_ref() == Some(&object) {
                    return Some(object);
                }
                self.candidate = Some(object);
            }
            CreatedTab::Pending { added } => (self.candidate, self.added) = (None, added),
        }
        None
    }

    fn timeout_message(&self) -> String {
        format!(
            "the setup shortcut did not produce a verifiably owned browser tab \
             ({} added tab(s) in the last walk; a uniquely selected added tab was {})",
            self.added,
            if self.selection_seen {
                "seen but not confirmed by a consecutive walk"
            } else {
                "never seen"
            }
        )
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
    let mut proof = CreatedTabProof::default();
    loop {
        input.check()?;
        let tree =
            window_scoped_tree(pid, window_id).map_err(|error| anyhow::anyhow!(error.message))?;
        let step = created_tab(&baseline, &tree).map_err(|error| anyhow::anyhow!(error.message))?;
        if let Some(object) = proof.observe(step) {
            handle.owned_setup_tab = Some(object);
            handle.opened_setup_page = true;
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("{}", proof.timeout_message());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    handle.owned_hotkey("l", &["ctrl"])?;

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
        handle.owned_hotkey("a", &["ctrl"])?;
        handle
            .owned_tab_tree()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        recheck_then_send(
            handle,
            |handle| handle.input.ready(pid, window_id),
            |handle| handle.require_owned_tab_selected_now(),
            |handle| {
                let typed =
                    handle
                        .owned_input()?
                        .type_setup_url(pid, window_id, descriptor.setup_url);
                handle.record_input_result(typed)
            },
        )?;
    } else {
        handle.require_owned_tab_selected_now()?;
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
    handle.owned_hotkey("enter", &[])?;

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
        let restore = self.input.focus.restore_on_drop();
        std::thread::scope(|scope| {
            if let Ok(worker) = std::thread::Builder::new()
                .name("cua-abandoned-setup".into())
                .spawn_scoped(scope, move || {
                    let _restore = restore;
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
                // Delivery may have happened: report its side effects
                // conservatively. This is reporting only, never ownership.
                if error.is::<hyprland_input::DispatchUnknown>() {
                    self.input_delivery_unknown = true;
                    self.foregrounded_window = true;
                    self.injected_global_input = true;
                }
                Err(error)
            }
        }
    }

    fn hotkey(&mut self, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
        let result = self.input.hotkey(self.pid, self.window_id, key, modifiers);
        self.record_input_result(result)
    }

    /// One targeted state read of the owned tab, as close to the keystroke as
    /// possible: a full walk is seconds old by the time it returns.
    fn require_owned_tab_selected_now(&self) -> anyhow::Result<()> {
        let owned = self
            .owned_setup_tab
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("this setup has no independently proven created tab"))?;
        if !crate::atspi::native::element_selected_ref(owned)? {
            anyhow::bail!("the selected browser tab is no longer this setup's created tab");
        }
        Ok(())
    }

    /// This setup's input, refusing each packet unless the owned tab is
    /// still the selected one at the moment it would be dispatched.
    fn owned_input(&self) -> anyhow::Result<SetupInput> {
        let owned = self
            .owned_setup_tab
            .clone()
            .ok_or_else(|| anyhow::anyhow!("this setup has no independently proven created tab"))?;
        let mut input = self.input.clone();
        input.cancellation = input.cancellation.with_dispatch_guard(move || {
            if !crate::atspi::native::element_selected_ref(&owned)? {
                anyhow::bail!("the selected browser tab is no longer this setup's created tab");
            }
            Ok(())
        });
        Ok(input)
    }

    fn owned_hotkey(&mut self, key: &str, modifiers: &[&str]) -> anyhow::Result<()> {
        recheck_then_send(
            self,
            |handle| handle.input.ready(handle.pid, handle.window_id),
            |handle| handle.require_owned_tab_selected_now(),
            |handle| {
                let result =
                    handle
                        .owned_input()?
                        .hotkey(handle.pid, handle.window_id, key, modifiers);
                handle.record_input_result(result)
            },
        )
    }

    /// Close the owned tab with the window's shortcut, then prove its exact
    /// accessibility object is gone. `Ok(false)` means the shortcut was never
    /// sent; an error after delivery must not be followed by another close.
    fn close_owned_tab(&mut self) -> anyhow::Result<bool> {
        let owned = self
            .owned_setup_tab
            .clone()
            .ok_or_else(|| anyhow::anyhow!("this setup has no independently proven created tab"))?;
        let mut recheck_failed = false;
        let sent = recheck_then_send(
            self,
            |handle| handle.input.ready(handle.pid, handle.window_id),
            |handle| {
                let result = handle.require_owned_tab_selected_now();
                recheck_failed = result.is_err();
                result
            },
            |handle| {
                let result =
                    handle
                        .owned_input()?
                        .hotkey(handle.pid, handle.window_id, "w", &["ctrl"]);
                handle.record_input_result(result)
            },
        );
        if recheck_failed {
            return Ok(false);
        }
        if let Err(error) = sent {
            // A refused shortcut was never sent. One that may have landed
            // must not be followed by rollback input on whatever tab
            // Chromium selected next.
            if error.is::<hyprland_input::DispatchUnknown>() {
                return Err(error);
            }
            return Ok(false);
        }
        let deadline = Instant::now() + EXISTING_PROFILE_SETUP_READY_TIMEOUT;
        loop {
            let tree = window_scoped_tree(self.pid, self.window_id)
                .map_err(|error| anyhow::anyhow!(error.message))?;
            let scan = scan_tabs(&tree).map_err(|error| anyhow::anyhow!(error.message))?;
            if !scan.truncated && !scan.unidentified && !scan.objects.contains(&owned) {
                self.opened_setup_page = false;
                self.owned_setup_tab = None;
                return Ok(true);
            }
            if Instant::now() >= deadline {
                anyhow::bail!("the temporary setup tab was still present after its close shortcut");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
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
        let closed_setup_page = self.close().unwrap_or(false);
        // Read after the close: its shortcut is input too.
        let foregrounded_window = self.foregrounded_window;
        let injected_global_input = self.injected_global_input;
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
        match self.close_owned_tab() {
            Ok(true) => {
                self.armed = false;
                Ok(Some(true))
            }
            Ok(false) => Err(self.abort(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "could not close the exact temporary setup tab",
            ))),
            // The shortcut was delivered: a second close or a rollback could
            // land on whatever tab Chromium selected next.
            Err(error) => {
                self.armed = false;
                Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    format!("could not verify the exact temporary setup tab closed: {error}"),
                ))
            }
        }
    }

    fn close(&mut self) -> Option<bool> {
        if !self.opened_setup_page {
            return None;
        }
        let Ok(tree) = window_scoped_tree(self.pid, self.window_id) else {
            return Some(false);
        };
        Some(
            self.require_owned_setup_page(&tree).is_ok() && self.close_owned_tab().unwrap_or(false),
        )
    }
}

type PendingSetupKey = (u32, u64);

fn pending_setups() -> &'static Mutex<HashMap<PendingSetupKey, SetupUiHandle>> {
    static PENDING: OnceLock<Mutex<HashMap<PendingSetupKey, SetupUiHandle>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Retain an armed setup for its later commit or rollback. The check and the
/// insert stay synchronous so a cancelled caller cannot strand an armed
/// handle; a duplicate is handed back for the caller to abort off the async
/// executor.
pub fn retain_pending(
    pid: u32,
    window_id: u64,
    handle: SetupUiHandle,
) -> Result<(), Box<SetupUiHandle>> {
    let mut pending = pending_setups().lock().unwrap();
    if pending.contains_key(&(pid, window_id)) {
        return Err(Box::new(handle));
    }
    pending.insert((pid, window_id), handle);
    Ok(())
}

pub(crate) fn duplicate_pending_refusal() -> BrowserRefusal {
    refusal(
        BrowserRefusalCode::BrowserBindingAmbiguous,
        "another approved browser setup is already pending for this exact window",
    )
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
            focus: FocusRestore::default(),
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
                focus: FocusRestore::default(),
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
        assert!(retain_pending(pid, window, handle).is_ok());
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
            let error = foreground_ack(reply).unwrap_err();
            assert!(!error.is::<hyprland_input::DispatchUnknown>());
        }
        // A refusal after delivered packets is possibly-delivered input.
        let partial = foreground_ack(json!({
            "ok": false,
            "code": "text_interrupted",
            "effect": "partial",
            "delivery": {"mode": "foreground", "delivered_count": 7},
        }))
        .unwrap_err();
        assert_eq!(
            partial
                .downcast_ref::<hyprland_input::DispatchUnknown>()
                .unwrap()
                .acknowledged_phases,
            7
        );
        let mut handle = inert_setup_handle();
        assert!(handle.record_input_result(Err(partial)).is_err());
        assert!(handle.input_delivery_unknown);
        assert!(handle.foregrounded_window);
        assert!(handle.injected_global_input);
        assert!(handle.owned_setup_tab.is_none());
    }

    #[derive(Default)]
    struct FakeHandBack {
        active: Option<u64>,
        follows_focus: bool,
        visible: Option<i64>,
        workspaces: HashMap<u64, i64>,
        log: Vec<String>,
        /// Quiet checks that pass before the person acts; `None` never acts.
        acts_after: Option<usize>,
    }

    impl HandBackOps for FakeHandBack {
        fn focus_window(&mut self, address: u64) -> anyhow::Result<()> {
            self.log.push(format!("focus {address:x}"));
            // Hyprland switches to the window's workspace either way.
            self.visible = self.workspaces.get(&address).copied();
            if self.follows_focus {
                self.active = Some(address);
            }
            Ok(())
        }
        fn show_workspace(&mut self, id: i64) -> anyhow::Result<()> {
            self.log.push(format!("workspace {id}"));
            self.visible = Some(id);
            Ok(())
        }
        fn move_pointer(&mut self, at: (f64, f64)) -> anyhow::Result<()> {
            self.log.push(format!("pointer {} {}", at.0, at.1));
            Ok(())
        }
        fn active_window(&mut self) -> anyhow::Result<Option<u64>> {
            Ok(self.active)
        }
        fn window_workspace(&mut self, address: u64) -> Option<i64> {
            self.workspaces.get(&address).copied()
        }
        fn visible_workspace(&mut self) -> Option<i64> {
            self.visible
        }
        fn settle(&mut self) {}
        fn person_quiet(&mut self) -> bool {
            match self.acts_after.as_mut() {
                None => true,
                Some(0) => false,
                Some(left) => {
                    *left -= 1;
                    true
                }
            }
        }
    }

    #[test]
    fn a_tab_selected_during_slow_preparation_never_receives_the_key() {
        struct Browser {
            owned_selected: bool,
            sent: Vec<&'static str>,
        }
        let mut browser = Browser {
            owned_selected: true,
            sent: Vec::new(),
        };
        // The person selects their own tab while the capture waits.
        let result = recheck_then_send(
            &mut browser,
            |browser| {
                browser.owned_selected = false;
                Ok(())
            },
            |browser| {
                if browser.owned_selected {
                    Ok(())
                } else {
                    anyhow::bail!("owned tab no longer selected")
                }
            },
            |browser| {
                browser.sent.push("ctrl+w");
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(browser.sent.is_empty());

        browser.owned_selected = true;
        recheck_then_send(
            &mut browser,
            |_| Ok(()),
            |browser| {
                assert!(browser.owned_selected);
                Ok(())
            },
            |browser| {
                browser.sent.push("ctrl+w");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(browser.sent, ["ctrl+w"]);
    }

    #[test]
    fn the_plugin_input_count_proves_quiet_only_when_unchanged_and_readable() {
        assert!(person_quiet_by_count(42, Some(42)));
        // A key, button, wheel, touch or tablet event since capture.
        assert!(!person_quiet_by_count(42, Some(43)));
        // A plugin restart (count reset) is not proof either.
        assert!(!person_quiet_by_count(42, Some(0)));
        // Unreadable now: never proof.
        assert!(!person_quiet_by_count(42, None));
    }

    #[test]
    fn hand_back_stops_the_moment_the_person_acts() {
        let (chrome, t3) = (0x55fe0d6b8140, 0x55fe0d713130);
        let workspaces = HashMap::from([(chrome, 5), (t3, 1)]);
        // Acted before anything changed: nothing is touched.
        let mut ops = FakeHandBack {
            active: Some(chrome),
            follows_focus: true,
            visible: Some(5),
            workspaces: workspaces.clone(),
            acts_after: Some(0),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Window(t3), (942.0, 1047.0), &mut ops).unwrap(),
            HandBackOutcome::PersonActed
        );
        assert!(ops.log.is_empty());
        // Acted after focus returned: their pointer is not overwritten.
        let mut ops = FakeHandBack {
            active: Some(chrome),
            follows_focus: true,
            visible: Some(5),
            workspaces: workspaces.clone(),
            acts_after: Some(1),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Window(t3), (942.0, 1047.0), &mut ops).unwrap(),
            HandBackOutcome::PersonActed
        );
        assert_eq!(ops.log, ["focus 55fe0d713130"]);
        // Acted while focus stayed: no realignment over their choice.
        let mut ops = FakeHandBack {
            active: Some(chrome),
            follows_focus: false,
            visible: Some(5),
            workspaces,
            acts_after: Some(1),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Window(t3), (942.0, 1047.0), &mut ops).unwrap(),
            HandBackOutcome::PersonActed
        );
        assert_eq!(ops.log, ["focus 55fe0d713130"]);
    }

    #[test]
    fn hand_back_proves_focus_before_moving_the_pointer() {
        let (chrome, t3) = (0x55fe0d6b8140, 0x55fe0d713130);
        let workspaces = HashMap::from([(chrome, 5), (t3, 1)]);
        let mut ops = FakeHandBack {
            active: Some(chrome),
            follows_focus: true,
            visible: Some(5),
            workspaces: workspaces.clone(),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Window(t3), (942.0, 1047.0), &mut ops).unwrap(),
            HandBackOutcome::Restored
        );
        assert_eq!(ops.log, ["focus 55fe0d713130", "pointer 942 1047"]);

        // Observed live on Hyprland 0.56: `ok`, workspace 1 shown, keyboard
        // focus still on Chrome. Show Chrome's workspace again; leave the
        // pointer alone.
        let mut ops = FakeHandBack {
            active: Some(chrome),
            follows_focus: false,
            visible: Some(5),
            workspaces,
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Window(t3), (942.0, 1047.0), &mut ops).unwrap(),
            HandBackOutcome::FocusStayed { realigned: true }
        );
        assert_eq!(ops.log, ["focus 55fe0d713130", "workspace 5"]);
        assert_eq!(ops.visible, Some(5));
    }

    #[test]
    fn hand_back_of_an_empty_workspace_requires_focus_to_leave_the_browser() {
        let chrome = 0x55fe0d6b8140;
        // Focus followed: nothing focused, workspace 1 shown.
        let mut ops = FakeHandBack {
            active: None,
            visible: Some(5),
            workspaces: HashMap::from([(chrome, 5)]),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Workspace(1), (10.0, 20.0), &mut ops).unwrap(),
            HandBackOutcome::Restored
        );
        assert_eq!(ops.log, ["workspace 1", "pointer 10 20"]);

        // Workspace 1 shown but keyboard focus still on Chrome on 5: hidden
        // focus is not a restore. Show Chrome again; leave the pointer.
        let mut ops = FakeHandBack {
            active: Some(chrome),
            visible: Some(5),
            workspaces: HashMap::from([(chrome, 5)]),
            ..Default::default()
        };
        assert_eq!(
            hand_back(HandBack::Workspace(1), (10.0, 20.0), &mut ops).unwrap(),
            HandBackOutcome::FocusStayed { realigned: true }
        );
        assert_eq!(ops.log, ["workspace 1", "workspace 5"]);
        assert_eq!(ops.visible, Some(5));
    }

    #[test]
    fn an_empty_prior_workspace_is_handed_back_only_when_the_person_stayed() {
        // Empty workspace 1, setup left them on the browser's workspace 5.
        assert_eq!(
            workspace_to_restore(None, Some(1), || true, || true, || Some(5)),
            Some(1)
        );
        // A prior window takes the window route instead.
        assert_eq!(
            workspace_to_restore(Some(0x4931), Some(1), || true, || true, || Some(5)),
            None
        );
        // Already back, several outputs, the person moved on, or the target
        // is no longer active: leave the workspace alone.
        assert_eq!(
            workspace_to_restore(None, Some(1), || true, || true, || Some(1)),
            None
        );
        assert_eq!(
            workspace_to_restore(None, None, || true, || true, || Some(5)),
            None
        );
        assert_eq!(
            workspace_to_restore(None, Some(1), || true, || false, || Some(5)),
            None
        );
        assert_eq!(
            workspace_to_restore(None, Some(1), || false, || true, || Some(5)),
            None
        );
    }

    #[test]
    fn focus_returns_only_from_the_still_active_target_to_a_live_prior_window() {
        let (target, prior) = (0x5156, 0x4931);
        assert_eq!(
            focus_to_restore(target, Some(prior), || true, |_| true, || true),
            Some(prior)
        );
        // A consent prompt sends no driver input: a moved pointer is the
        // person's own activity, so focus stays where they took it.
        assert_eq!(
            focus_to_restore(target, Some(prior), || true, |_| true, || false),
            None
        );
        assert!(same_pointer((10.0, 20.0), (10.4, 19.6)));
        assert!(!same_pointer((10.0, 20.0), (12.0, 20.0)));
        // Hyprland's own warp to the newly focused target's centre (observed
        // live: a 3832x2087 window at 4,4 left the pointer at 1920/1047) is
        // the compositor's move, not the person's.
        let chrome = Some((4, 4, 3832, 2087));
        assert!(pointer_left_alone(
            (942.0, 1047.0),
            chrome,
            (1920.0, 1047.0)
        ));
        assert!(pointer_left_alone((942.0, 1047.0), chrome, (942.0, 1047.0)));
        assert!(!pointer_left_alone(
            (942.0, 1047.0),
            chrome,
            (1500.0, 900.0)
        ));
        assert!(!pointer_left_alone((942.0, 1047.0), None, (1920.0, 1047.0)));
        // The person moved on, the prior window closed, or there was nothing
        // (or only the target itself) to return to: leave focus alone.
        assert_eq!(
            focus_to_restore(target, Some(prior), || false, |_| true, || true),
            None
        );
        assert_eq!(
            focus_to_restore(target, Some(prior), || true, |_| false, || true),
            None
        );
        assert_eq!(
            focus_to_restore(
                target,
                Some(target),
                || panic!("no query"),
                |_| true,
                || true
            ),
            None
        );
        assert_eq!(
            focus_to_restore(target, None, || panic!("no query"), |_| true, || true),
            None
        );
        // Nothing captured means no compositor query and no restoration.
        FocusRestore::default().restore();
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
                focus: FocusRestore::default(),
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
        // Possibly delivered input is reported, but grants no ownership.
        assert!(handle.foregrounded_window);
        assert!(handle.injected_global_input);
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
        let CreatedTab::Selected(created) =
            created_tab(&browser_tabs(&baseline).unwrap(), &current).unwrap()
        else {
            panic!("one added, uniquely selected tab is a created tab");
        };
        handle.owned_setup_tab = Some(created);
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
        assert_eq!(
            created_tab(&baseline, &unchanged).unwrap(),
            CreatedTab::Pending { added: 0 }
        );
        let replaced = setup_tree(vec![tab("/created", true)]);
        assert_eq!(
            created_tab(&baseline, &replaced).unwrap_err().code,
            BrowserRefusalCode::BrowserWrongTargetRefused
        );
        let extra = setup_tree(vec![
            tab("/existing", false),
            tab("/created", true),
            tab("/unexpected", false),
        ]);
        assert_eq!(
            created_tab(&baseline, &extra).unwrap_err().code,
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
        assert_eq!(
            created_tab(&baseline, &web_content).unwrap(),
            CreatedTab::Pending { added: 0 }
        );
    }

    #[test]
    fn created_tab_waits_through_asynchronous_tab_state() {
        let baseline = browser_tabs(&setup_tree(vec![tab("/existing", true)])).unwrap();
        let created = ObjectRef {
            bus: ":1.5156".to_owned(),
            path: "/created".to_owned(),
        };
        let mut unstated = tab("/created", false);
        unstated.selected = None;
        let mut unidentified = tab("/created", true);
        unidentified.object_ref = None;
        let mut truncated = setup_tree(vec![tab("/existing", false), tab("/created", true)]);
        truncated.truncated = true;
        // Scripted states Chromium publishes between Ctrl+T and a settled
        // strip: none of them is final, and none of them is ownership.
        for (tree, added) in [
            (setup_tree(vec![tab("/existing", false)]), 0),
            (
                setup_tree(vec![tab("/existing", true), tab("/created", false)]),
                1,
            ),
            (
                setup_tree(vec![tab("/existing", false), tab("/created", false)]),
                1,
            ),
            (
                setup_tree(vec![tab("/existing", true), tab("/created", true)]),
                1,
            ),
            (setup_tree(vec![tab("/existing", false), unstated]), 1),
            (setup_tree(vec![tab("/existing", false), unidentified]), 0),
            (truncated, 1),
        ] {
            assert_eq!(
                created_tab(&baseline, &tree).unwrap(),
                CreatedTab::Pending { added }
            );
        }
        // A lost baseline tab is final only when the walk could have seen it.
        let mut partial = setup_tree(vec![tab("/created", true)]);
        partial.truncated = true;
        assert_eq!(
            created_tab(&baseline, &partial).unwrap(),
            CreatedTab::Pending { added: 1 }
        );
        assert_eq!(
            created_tab(
                &baseline,
                &setup_tree(vec![tab("/existing", false), tab("/created", true)])
            )
            .unwrap(),
            CreatedTab::Selected(created.clone())
        );
        // Ownership needs two consecutive confirmations of the same tab.
        let other = ObjectRef {
            bus: ":1.5156".to_owned(),
            path: "/other".to_owned(),
        };
        let mut proof = CreatedTabProof::default();
        assert!(proof.timeout_message().contains("0 added tab(s)"));
        assert!(proof.timeout_message().contains("never seen"));
        assert_eq!(proof.observe(CreatedTab::Selected(created.clone())), None);
        assert_eq!(proof.observe(CreatedTab::Pending { added: 1 }), None);
        assert_eq!(proof.observe(CreatedTab::Selected(created.clone())), None);
        assert_eq!(proof.observe(CreatedTab::Selected(other.clone())), None);
        assert!(proof
            .timeout_message()
            .contains("seen but not confirmed by a consecutive walk"));
        assert_eq!(
            proof.observe(CreatedTab::Selected(other.clone())),
            Some(other)
        );

        let mut duplicated = setup_tree(vec![tab("/existing", false), tab("/existing", true)]);
        duplicated.nodes.truncate(2);
        assert_eq!(
            created_tab(&baseline, &duplicated).unwrap_err().code,
            BrowserRefusalCode::BrowserBindingAmbiguous
        );
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
