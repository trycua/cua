//! Keyboard focus on niri: focusing, confirming and restoring windows, and
//! focus transactions.

use super::*;

/// How long niri may take to apply and report a focus change.
pub(super) const FOCUS_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);

// ── Focus ────────────────────────────────────────────────────────────────────

pub(super) fn focus_window_action(id: u64) -> serde_json::Value {
    serde_json::json!({ "FocusWindow": { "id": id } })
}

pub(super) fn foreground_unavailable(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("foreground_unavailable: {detail}")
}

/// The refusal for desktop-frame coordinates addressed to window `id`.
/// Mapping them through the window's current place would aim at the same
/// desktop pixel even after the window moved under it, and niri does not
/// say whether it did since the `get_desktop_state` screenshot.
pub fn desktop_frame_window_refusal(id: u64) -> anyhow::Error {
    foreground_unavailable(format!(
        "niri cannot map desktop-frame coordinates into window {id}, because it does not report whether the window moved since the get_desktop_state screenshot; act at desktop scope without pid and window_id, or pass window-local coordinates"
    ))
}

/// The next step for a niri input refusal: its detail names the remedy.
/// `None` for refusals that do not come from niri, which keep the generic
/// suggestion.
pub fn refusal_suggestion(detail: &str) -> Option<&'static str> {
    if detail.contains("without pid and window_id") {
        return Some("Drop pid and window_id to act at desktop scope on the latest get_desktop_state screenshot, or pass window-local coordinates from get_window_state.");
    }
    detail.contains("niri").then_some(
        "Take a new screenshot (get_desktop_state for desktop-scope points, get_window_state for a floating window) after the remedy the detail names, and retry with its coordinates.",
    )
}

/// An IPC failure on an input route, with its whole cause chain. A refusal
/// that already carries the code is kept as it is.
pub(crate) fn input_unavailable(error: anyhow::Error) -> anyhow::Error {
    if crate::input::foreground::error_code(&error).is_some() {
        return error;
    }
    foreground_unavailable(format!("{error:#}"))
}

/// Check that `id` can take focus for `pid`. Returns the window when it is
/// not focused yet, `None` when it already is.
fn focus_target<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    id: u64,
    pid: Option<u32>,
) -> Result<Option<IpcWindow>> {
    let overview: Overview = ipc.query(Request::OverviewState)?;
    if overview.is_open {
        return Err(foreground_unavailable(
            "the niri overview is open, so input would go to the overview instead of the window",
        ));
    }
    let windows: Vec<IpcWindow> = ipc.query(Request::Windows)?;
    let target = windows
        .into_iter()
        .find(|window| window.id == id)
        .ok_or_else(|| foreground_unavailable(format!("niri window {id} no longer exists")))?;
    let target_pid = target.pid.and_then(|pid| u32::try_from(pid).ok());
    if let Some(pid) = pid {
        if target_pid != Some(pid) {
            return Err(foreground_unavailable(format!(
                "niri window {id} belongs to pid {}, not pid {pid}",
                target_pid.map_or_else(|| "unknown".to_owned(), |pid| pid.to_string())
            )));
        }
    }
    Ok((!target.is_focused).then_some(target))
}

/// Focus exactly `id` and read the focus back. niri answers `Handled` even
/// for an unknown id, and reports no focused window while the overview, a
/// layer-shell surface or the lock screen holds focus, so the read-back is
/// the guarantee.
pub(super) fn activate_in<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    id: u64,
    pid: Option<u32>,
    confirm: Duration,
) -> Result<()> {
    match focus_target(ipc, id, pid)? {
        None => Ok(()),
        Some(target) => focus_window(ipc, id, target.pid, confirm),
    }
}

fn focus_window<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    id: u64,
    pid: Option<i32>,
    confirm: Duration,
) -> Result<()> {
    ipc.action(focus_window_action(id))?;
    confirm_focus(ipc, id, pid, confirm)
}

fn confirm_focus<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    id: u64,
    pid: Option<i32>,
    confirm: Duration,
) -> Result<()> {
    let deadline = Instant::now() + confirm;
    loop {
        let focused = ipc.focused_window()?;
        if focused
            .as_ref()
            .is_some_and(|focused| focused.id == id && focused.pid == pid)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(foreground_unavailable(format!(
                "niri did not confirm focus on window {id} (focused: {})",
                focused.map_or_else(|| "none".to_owned(), |focused| focused.id.to_string())
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Focus an exact niri window for input, confirmed by read-back.
pub fn activate(id: u64, pid: Option<u32>) -> Result<()> {
    activate_in(&mut ipc(), id, pid, FOCUS_CONFIRM_TIMEOUT).map_err(input_unavailable)
}

/// The window niri sends keyboard input to, or `None` when no window has it.
/// niri marks a window focused only while its layout holds keyboard focus,
/// so the lock screen, the overview, the exit dialog, the screenshot UI and
/// a focused layer-shell surface all read as `None`.
pub fn keyboard_focus() -> Result<Option<u64>> {
    Ok(ipc().focused_window()?.map(|window| window.id))
}

/// The index of niri's active keyboard layout: the xkb group keys go out in.
pub fn keyboard_layout() -> Result<u32> {
    let layouts: KeyboardLayouts = ipc().query(Request::KeyboardLayouts)?;
    if usize::try_from(layouts.current_idx).map_or(true, |idx| idx >= layouts.names.len()) {
        bail!(
            "niri's active keyboard layout {} is not one of its {} layouts",
            layouts.current_idx,
            layouts.names.len()
        );
    }
    Ok(layouts.current_idx)
}

/// Briefly focus one niri window for `pid`, run `body`, then restore the
/// previously focused window, or the previously focused workspace when no
/// window had focus, and each output's previously active workspace.
/// Focusing a window on another workspace switches to it; the restore
/// switches back, also after a focus niri did not confirm.
pub fn with_focused_window<T>(pid: u32, id: u64, body: impl FnOnce() -> Result<T>) -> Result<T> {
    with_focused_window_in(&mut ipc(), pid, id, FOCUS_CONFIRM_TIMEOUT, body)
}

fn with_focused_window_in<C: FnMut(Instant) -> Result<UnixStream>, T>(
    ipc: &mut Ipc<C>,
    pid: u32,
    id: u64,
    confirm: Duration,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _transaction = focus_transaction()?;
    let prior = ipc.focused_window().map_err(input_unavailable)?;
    let workspaces: Vec<IpcWorkspace> =
        ipc.query(Request::Workspaces).map_err(input_unavailable)?;
    let Some(target) = focus_target(ipc, id, Some(pid)).map_err(input_unavailable)? else {
        return body();
    };
    // Once FocusWindow is sent, focus may have moved even when niri does not
    // confirm it, so the restore runs either way.
    let result = focus_window(ipc, id, target.pid, confirm)
        .map_err(input_unavailable)
        .and_then(|()| body());
    let restore = restore_focus(ipc, prior.as_ref(), &workspaces, &target, confirm);
    finish(result, restore)
}

/// Run `body`, which focuses niri window `id` of `pid` itself once its input
/// is admitted, then undo what that focus changed, as [`with_focused_window`]
/// does. Unlike it, nothing is focused before `body`: a body refused before it
/// focused the window leaves focus, the view and the floating stacking order
/// as they were.
pub fn with_focus_restored<T>(pid: u32, id: u64, body: impl FnOnce() -> Result<T>) -> Result<T> {
    with_focus_restored_in(&mut ipc(), pid, id, FOCUS_CONFIRM_TIMEOUT, body)
}

fn with_focus_restored_in<C: FnMut(Instant) -> Result<UnixStream>, T>(
    ipc: &mut Ipc<C>,
    pid: u32,
    id: u64,
    confirm: Duration,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _transaction = focus_transaction()?;
    let prior = ipc.focused_window().map_err(input_unavailable)?;
    let workspaces: Vec<IpcWorkspace> =
        ipc.query(Request::Workspaces).map_err(input_unavailable)?;
    let Some(target) = focus_target(ipc, id, Some(pid)).map_err(input_unavailable)? else {
        return body();
    };
    let result = body();
    let restore = match ipc.focused_window() {
        Ok(now)
            if now.as_ref().map(|window| window.id) == prior.as_ref().map(|window| window.id) =>
        {
            Ok(())
        }
        Ok(_) => restore_focus(ipc, prior.as_ref(), &workspaces, &target, confirm),
        Err(error) => Err(error),
    };
    finish(result, restore)
}

/// Wait for any other focus transaction to give focus back, then hold this
/// one until it does. A worker whose call timed out still gives focus back
/// late; meanwhile a retry on the same window would read that window as the
/// prior focus and lose it to the late restore. The holder's waits are
/// bounded, and a caller whose own call ends stops waiting.
fn focus_transaction() -> Result<std::sync::MutexGuard<'static, ()>> {
    static TRANSACTION: Mutex<()> = Mutex::new(());
    loop {
        match TRANSACTION.try_lock() {
            Ok(held) => return Ok(held),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {
                crate::input::action::admit(|| ())?;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// The outcome of a focus transaction: `body`'s, with a failed restore.
fn finish<T>(result: Result<T>, restore: Result<()>) -> Result<T> {
    match (result, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        // The body sent its input: say so, so the caller does not repeat it.
        (Ok(_), Err(error)) => {
            Err(error.context("the input was sent, but niri could not restore the prior focus"))
        }
        (Err(error), Err(restore)) => {
            let note =
                format!("{error}; the prior niri focus also could not be restored: {restore:#}");
            Err(error.context(note))
        }
    }
}

/// Undo what focusing `target` changed: the active window of its workspace,
/// the active workspace of every output, then the focused window or
/// workspace.
fn restore_focus<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    prior: Option<&IpcWindow>,
    before: &[IpcWorkspace],
    target: &IpcWindow,
    confirm: Duration,
) -> Result<()> {
    let prior_id = prior.map(|prior| prior.id);
    let focused = ipc.focused_window()?.map(|focused| focused.id);
    let workspaces: Vec<IpcWorkspace> = ipc.query(Request::Workspaces)?;
    // Nothing moved, as when the call ran out of time before it focused the
    // target: there is nothing to give back.
    if focused == prior_id && workspaces == before {
        return Ok(());
    }
    // Someone focused another window meanwhile: theirs is the newer choice.
    if let Some(chosen) =
        focused.filter(|focused| *focused != target.id && Some(*focused) != prior_id)
    {
        tracing::info!(
            window = chosen,
            "niri focus moved to another window during input; leaving it there"
        );
        return Ok(());
    }
    // The same for a workspace, which may hold no window to focus.
    let prior_workspace = before
        .iter()
        .find(|workspace| workspace.is_focused)
        .map(|workspace| workspace.id);
    if let Some(chosen) = workspaces
        .into_iter()
        .find(|workspace| workspace.is_focused)
        .map(|workspace| workspace.id)
        .filter(|focused| {
            Some(*focused) != target.workspace_id && Some(*focused) != prior_workspace
        })
    {
        tracing::info!(
            workspace = chosen,
            "niri focus moved to another workspace during input; leaving it there"
        );
        return Ok(());
    }
    if let Some(active) = before
        .iter()
        .find(|workspace| Some(workspace.id) == target.workspace_id)
        .and_then(|workspace| workspace.active_window_id)
        .filter(|active| *active != target.id && Some(*active) != prior_id)
    {
        ipc.restore_action(focus_window_action(active))
            .with_context(|| format!("niri could not restore active window {active}"))?;
    }
    let now: Vec<IpcWorkspace> = ipc.query(Request::Workspaces)?;
    for was in before
        .iter()
        .filter(|was| was.is_active && !was.is_focused)
        .filter(|was| now.iter().any(|w| w.id == was.id && !w.is_active))
    {
        ipc.restore_action(focus_workspace_action(was.id))
            .with_context(|| format!("niri could not restore workspace {}", was.id))?;
    }
    match (prior, before.iter().find(|workspace| workspace.is_focused)) {
        (Some(prior), _) => ipc
            .restore_action(focus_window_action(prior.id))
            .and_then(|()| confirm_focus(ipc, prior.id, prior.pid, confirm))
            .with_context(|| format!("niri could not restore prior window {}", prior.id))?,
        (None, Some(workspace)) => restore_workspace(ipc, workspace.id, confirm)
            .with_context(|| format!("niri could not restore prior workspace {}", workspace.id))?,
        (None, None) => {}
    }
    // niri applies workspace switches asynchronously: poll like the
    // single-workspace restore does.
    let deadline = Instant::now() + confirm;
    loop {
        let now: Vec<IpcWorkspace> = ipc.query(Request::Workspaces)?;
        let Some(was) = before
            .iter()
            .filter(|was| was.is_active)
            .find(|was| now.iter().any(|w| w.id == was.id && !w.is_active))
        else {
            return Ok(());
        };
        if Instant::now() >= deadline {
            bail!(
                "niri did not restore workspace {} on output {}",
                was.id,
                was.output.as_deref().unwrap_or("unknown")
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn focus_workspace_action(id: u64) -> serde_json::Value {
    serde_json::json!({ "FocusWorkspace": { "reference": { "Id": id } } })
}

fn restore_workspace<C: FnMut(Instant) -> Result<UnixStream>>(
    ipc: &mut Ipc<C>,
    workspace: u64,
    confirm: Duration,
) -> Result<()> {
    ipc.restore_action(focus_workspace_action(workspace))?;
    let deadline = Instant::now() + confirm;
    loop {
        let workspaces: Vec<IpcWorkspace> = ipc.query(Request::Workspaces)?;
        if workspaces
            .iter()
            .any(|candidate| candidate.id == workspace && candidate.is_focused)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("niri did not confirm focus on workspace {workspace}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_niri_refusal_suggests_the_remedy_its_detail_names() {
        let tiled = aim_at(&tiled_scene(Some(5)).window(5).unwrap(), 10, 20).unwrap_err();
        assert!(tiled.to_string().contains("float the window"));
        assert!(refusal_suggestion(&tiled.to_string())
            .unwrap()
            .starts_with("Take a new screenshot"));
        let history = history();
        let edge = desktop_frame(history)
            .admit(&tiled_scene(Some(5)), &[(10, 100)], history)
            .unwrap_err();
        assert!(refusal_suggestion(&edge.to_string()).is_some());
        // A refusal that is not niri's keeps the generic suggestion.
        assert_eq!(
            refusal_suggestion("foreground_unavailable: no window"),
            None
        );
    }

    #[test]
    fn focus_transaction_confirms_runs_and_restores() {
        let fake = focus_fixture();
        let seen = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || Ok(fake.lock().unwrap().focused()),
        )
        .unwrap();
        assert_eq!(seen, Some(7));
        assert_eq!(fake.lock().unwrap().focused(), Some(5));
    }

    #[test]
    fn unconfirmed_focus_refuses_before_the_body() {
        let fake = focus_fixture();
        fake.lock().unwrap().ignore_focus = true;
        let mut ran = false;
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || {
                ran = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!ran);
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri did not confirm focus on window 7 (focused: 5)"
        );
    }

    #[test]
    fn a_lost_niri_refuses_focus_bound_input_with_the_typed_code() {
        let mut ran = false;
        let error = with_focused_window_in(
            &mut ipc_with(
                |_| bail!("niri was recognised as pid 20, but the compositor is now pid 40"),
                Duration::from_millis(50),
                Duration::from_millis(100),
            ),
            107,
            7,
            Duration::from_millis(50),
            || {
                ran = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!ran);
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri was recognised as pid 20, but the compositor is now pid 40"
        );
    }

    #[test]
    fn another_pid_refuses_without_focusing() {
        let fake = focus_fixture();
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            999,
            7,
            Duration::from_millis(50),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri window 7 belongs to pid 107, not pid 999"
        );
        assert!(!fake
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.contains("FocusWindow")));
    }

    #[test]
    fn null_pid_window_refuses_a_pid_bound_transaction() {
        let fake = focus_fixture();
        fake.lock().unwrap().windows[1]["pid"] = Value::Null;
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri window 7 belongs to pid unknown, not pid 107"
        );
    }

    #[test]
    fn open_overview_refuses_focus_bound_input() {
        let fake = focus_fixture();
        fake.lock().unwrap().overview = true;
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: the niri overview is open, so input would go to the overview instead of the window"
        );
    }

    #[test]
    fn a_call_that_ran_out_of_time_moves_no_niri_focus() {
        let fake = focus_fixture();
        let action = crate::input::action::Action::new(Duration::from_secs(60));
        action.abandon();
        let _bound = crate::input::action::bind(action);
        let mut ran = false;
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || {
                ran = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!ran);
        assert_eq!(
            crate::input::foreground::error_code(&error),
            Some(crate::input::foreground::CODE_TIMEOUT)
        );
        let fake = fake.lock().unwrap();
        assert!(
            !fake
                .requests
                .iter()
                .any(|request| request.contains("Action")),
            "a timed-out call sent {:?}",
            fake.requests
        );
        assert_eq!(fake.focused(), Some(5));
    }

    #[test]
    fn a_pointer_action_refused_before_it_focused_leaves_niri_untouched() {
        let fake = focus_fixture();
        let error = with_focus_restored_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || Err::<(), _>(foreground_unavailable("the point is outside niri window 7")),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: the point is outside niri window 7"
        );
        let fake = fake.lock().unwrap();
        assert!(
            !fake
                .requests
                .iter()
                .any(|request| request.contains("Action")),
            "a refused action sent {:?}",
            fake.requests
        );
        assert_eq!(fake.focused(), Some(5));
    }

    #[test]
    fn a_pointer_action_that_focused_its_target_gives_focus_back() {
        let fake = focus_fixture();
        let seen = with_focus_restored_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                // The pointer gate focuses the target once it admits the input.
                let mut fake = fake.lock().unwrap();
                fake.reply(r#"{"Action":{"FocusWindow":{"id":7}}}"#);
                Ok(fake.focused())
            },
        )
        .unwrap();
        assert_eq!(seen, Some(7));
        let fake = fake.lock().unwrap();
        assert_eq!(fake.focused(), Some(5));
        assert!(fake.active(1));
    }

    #[test]
    fn a_call_that_ran_out_of_time_after_focusing_its_target_still_gives_focus_back() {
        let timed_out = || {
            anyhow!(
                "{}: the call ran out of time, so no further input was sent",
                crate::input::foreground::CODE_TIMEOUT
            )
        };
        // A pointer action whose gate focused the target.
        let fake = focus_fixture();
        let action = crate::input::action::Action::new(Duration::from_secs(60));
        let _bound = crate::input::action::bind(action.clone());
        let error = with_focus_restored_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                fake.lock()
                    .unwrap()
                    .reply(r#"{"Action":{"FocusWindow":{"id":7}}}"#);
                action.abandon();
                Err::<(), _>(timed_out())
            },
        )
        .unwrap_err();
        assert_eq!(
            crate::input::foreground::error_code(&error),
            Some(crate::input::foreground::CODE_TIMEOUT)
        );
        assert_eq!(fake.lock().unwrap().focused(), Some(5));

        // Keys and text, which focus the target before they start.
        let fake = focus_fixture();
        let action = crate::input::action::Action::new(Duration::from_secs(60));
        let _bound = crate::input::action::bind(action.clone());
        with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                action.abandon();
                Err::<(), _>(timed_out())
            },
        )
        .unwrap_err();
        assert_eq!(fake.lock().unwrap().focused(), Some(5));
    }

    #[test]
    fn a_late_restore_does_not_take_focus_from_the_next_call_on_its_target() {
        // Call 1 focused window 7 and is still running (a worker past its
        // timeout, about to give focus back). Call 2, a retry on window 7,
        // must not find 7 focused by call 1 and then lose it to call 1's
        // restore while it types.
        let fake = focus_fixture();
        let (in_body, call_1_in_body) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (finished, call_1_finished) = std::sync::mpsc::channel();
        let call_1 = {
            let fake = fake.clone();
            std::thread::spawn(move || {
                with_focus_restored_in(
                    &mut fake_ipc(&fake),
                    107,
                    7,
                    Duration::from_millis(100),
                    || {
                        fake.lock()
                            .unwrap()
                            .reply(r#"{"Action":{"FocusWindow":{"id":7}}}"#);
                        in_body.send(()).unwrap();
                        released.recv().unwrap();
                        Ok(())
                    },
                )
                .unwrap();
                finished.send(()).unwrap();
            })
        };
        call_1_in_body.recv().unwrap();
        let call_2 = {
            let fake = fake.clone();
            std::thread::spawn(move || {
                with_focused_window_in(
                    &mut fake_ipc(&fake),
                    107,
                    7,
                    Duration::from_millis(100),
                    || {
                        // Typing outlasts call 1's restore.
                        let _ = call_1_finished.recv_timeout(Duration::from_secs(2));
                        Ok(fake.lock().unwrap().focused())
                    },
                )
                .unwrap()
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        release.send(()).unwrap();
        call_1.join().unwrap();
        assert_eq!(call_2.join().unwrap(), Some(7));
        assert_eq!(fake.lock().unwrap().focused(), Some(5));
    }

    #[test]
    fn failed_restore_is_reported_after_the_body_ran() {
        let fake = focus_fixture();
        fake.lock().unwrap().ignore_focus_on = vec![5];
        let mut ran = false;
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || {
                ran = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(ran);
        assert_eq!(
            format!("{error:#}"),
            "the input was sent, but niri could not restore the prior focus: niri could not restore prior window 5: foreground_unavailable: niri did not confirm focus on window 5 (focused: 7)"
        );
        assert_eq!(
            format!("{:#}", error.root_cause()),
            "foreground_unavailable: niri did not confirm focus on window 5 (focused: 7)"
        );
    }

    #[test]
    fn body_error_keeps_the_restore_error_as_context() {
        let fake = focus_fixture();
        fake.lock().unwrap().ignore_focus_on = vec![5];
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || Err::<(), _>(anyhow!("wtype failed")),
        )
        .unwrap_err();
        assert!(error.to_string().starts_with(
            "wtype failed; the prior niri focus also could not be restored: niri could not restore prior window 5"
        ));
        assert_eq!(error.root_cause().to_string(), "wtype failed");
    }

    #[test]
    fn focus_niri_did_not_confirm_is_still_undone() {
        let fake = focus_fixture();
        fake.lock().unwrap().focus_taken_on = vec![7];
        let error = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(50),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri did not confirm focus on window 7 (focused: none)"
        );
        let fake = fake.lock().unwrap();
        assert!(fake.active(1) && !fake.active(2));
        assert_eq!(fake.focused(), Some(5));
    }

    #[test]
    fn a_window_focused_by_someone_else_during_input_keeps_focus() {
        let fake = focus_fixture();
        fake.lock()
            .unwrap()
            .windows
            .push(floating(9, 1, (500.0, 60.0), (100, 100)));
        with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                fake.lock()
                    .unwrap()
                    .reply(r#"{"Action":{"FocusWindow":{"id":9}}}"#);
                Ok(())
            },
        )
        .unwrap();
        let fake = fake.lock().unwrap();
        assert_eq!(fake.focused(), Some(9));
        assert!(!fake
            .requests
            .iter()
            .any(|r| r.contains(r#""FocusWindow":{"id":5}"#)));
    }

    #[test]
    fn an_empty_workspace_chosen_during_input_keeps_focus() {
        let fake = focus_fixture();
        fake.lock()
            .unwrap()
            .workspaces
            .push(workspace(3, "winit", false, None));
        with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                fake.lock()
                    .unwrap()
                    .reply(r#"{"Action":{"FocusWorkspace":{"reference":{"Id":3}}}}"#);
                Ok(())
            },
        )
        .unwrap();
        let fake = fake.lock().unwrap();
        assert!(fake.active(3) && !fake.active(1));
        assert_eq!(fake.focused(), None);
    }

    #[test]
    fn every_output_gets_its_active_workspace_back() {
        let mut prior = tiled(5, 1, (402.0, 448.0));
        prior["is_focused"] = json!(true);
        let fake = Arc::new(Mutex::new(Fake {
            windows: vec![prior, floating(7, 4, (100.0, 60.0), (300, 200))],
            workspaces: vec![
                workspace(1, "left", true, Some(5)),
                workspace(3, "right", true, None),
                workspace(4, "right", false, Some(7)),
            ],
            ..Fake::default()
        }));
        {
            let mut fake = fake.lock().unwrap();
            fake.workspaces[1]["is_focused"] = json!(false);
            fake.workspace_lag = 2;
        }
        let seen = with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || {
                let fake = fake.lock().unwrap();
                Ok((fake.focused(), fake.active(4)))
            },
        )
        .unwrap();
        assert_eq!(seen, (Some(7), true));
        let fake = fake.lock().unwrap();
        assert!(fake.active(1) && fake.active(3) && !fake.active(4));
        assert_eq!(fake.focused(), Some(5));
    }

    #[test]
    fn no_prior_window_restores_the_prior_workspace() {
        let fake = focus_fixture();
        {
            let mut fake = fake.lock().unwrap();
            fake.windows[0]["is_focused"] = json!(false);
            fake.workspaces.push(workspace(3, "winit", false, None));
            for ws in &mut fake.workspaces {
                ws["is_focused"] = json!(ws["id"] == 3);
                ws["is_active"] = json!(ws["id"] == 3);
            }
        }
        with_focused_window_in(
            &mut fake_ipc(&fake),
            107,
            7,
            Duration::from_millis(100),
            || Ok(()),
        )
        .unwrap();
        let fake = fake.lock().unwrap();
        assert!(fake
            .requests
            .iter()
            .any(|r| r.contains(r#""FocusWorkspace":{"reference":{"Id":3}}"#)));
        assert!(fake
            .workspaces
            .iter()
            .any(|ws| ws["id"] == 3 && ws["is_focused"] == true));
    }

    #[test]
    fn an_unknown_window_refuses_instead_of_trusting_handled() {
        // niri answers FocusWindow on an unknown id with Handled.
        let fake = focus_fixture();
        let error =
            activate_in(&mut fake_ipc(&fake), 999, None, Duration::from_millis(50)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "foreground_unavailable: niri window 999 no longer exists"
        );
    }
}
