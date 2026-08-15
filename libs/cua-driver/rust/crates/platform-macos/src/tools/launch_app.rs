use async_trait::async_trait;
use cua_driver_core::{
    launch_state_json,
    protocol::ToolResult,
    resolve_instance_policy,
    tool::{Tool, ToolDef},
    InstancePolicy, ProcessDisposition, WindowDisposition,
};
use serde_json::Value;
use std::{collections::HashSet, path::PathBuf};

pub struct LaunchAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
static APP_ACQUISITION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "launch_app".into(),
        description:
            "Resolve or launch a macOS app in the background — the target does NOT come to the foreground.\n\n\
             Provide either `bundle_id` (preferred — unambiguous, e.g. `com.apple.calculator`) \
             or `name` (e.g. \"Calculator\"). If both are given, bundle_id wins.\n\n\
             `instance_policy` controls acquisition: `reuse_or_launch` (default) atomically \
             reuses one exact existing app window before sending a launch request, `reuse_only` \
             refuses rather than launching, and `new` requests an isolated process. Multiple \
             exact running candidates are reported as ambiguous instead of guessing.\n\n\
             Optional `urls` are handed to the app as open targets — for Finder, pass a folder \
             path to open a backgrounded Finder window there.\n\n\
             Browser DevTools setup belongs to `browser_prepare`, which can prove that a \
             separate isolated profile is driver-owned before enabling CDP.\n\n\
             Optional `webkit_inspector_port`: opens a WebKit inspector server on the specified \
             port (sets WEBKIT_INSPECTOR_SERVER=127.0.0.1:N + TAURI_WEBVIEW_AUTOMATION=1). \
             Use this for Tauri/WebKit-based apps.\n\n\
             `creates_new_application_instance` is a deprecated compatibility alias: true maps \
             to `instance_policy: \"new\"`. Apps that cannot create an isolated process return \
             `NEW_APPLICATION_INSTANCE_UNAVAILABLE`; retry with reuse only when sharing is safe.\n\n\
             Optional `additional_arguments`: extra argv strings appended after --args.\n\n\
             Returns the launched app's pid, bundle_id, name, and a `windows` array \
             (same shape as `list_windows`) so callers can skip an extra round-trip before \
             `get_window_state(pid, window_id)`. `launch_state` distinguishes whether the \
             request was sent, whether the process/window was reused or created/materialized, \
             and whether a window is ready. When the \
             focus-steal belt-and-braces \
             demotion check ran (target pid ≠ prior frontmost), the response also includes \
             `self_activation_suppressed: bool` — true if focus stayed with the prior \
             frontmost, false if the launched app held focus despite the re-demote attempt."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "bundle_id": {
                    "type": "string",
                    "description": "App bundle identifier, e.g. com.apple.calculator. Preferred over name."
                },
                "name": {
                    "type": "string",
                    "description": "App display name. Used only when bundle_id is absent."
                },
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional file paths or URLs to open with the app (e.g. a folder path for Finder)."
                },
                "webkit_inspector_port": {
                    "type": "integer",
                    "description": "Open a WebKit inspector server on this port (sets WEBKIT_INSPECTOR_SERVER env var)."
                },
                "session": cua_driver_core::tool_schema::session_schema(),
                "instance_policy": cua_driver_core::tool_schema::instance_policy_schema(),
                "creates_new_application_instance": {
                    "type": "boolean",
                    "deprecated": true,
                    "description": "Deprecated compatibility alias. True maps to instance_policy=\"new\"; false maps to the default reuse_or_launch when instance_policy is omitted."
                },
                "additional_arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments appended after --args when launching."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
    })
}

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let bundle_id = args.opt_str("bundle_id");
        let name = args.opt_str("name");
        let mut response_bundle_id = bundle_id.clone();
        let response_requested_name = name.clone();
        let urls: Vec<String> = args
            .str_array("urls")
            .into_iter()
            .map(normalize_launch_url)
            .collect();
        if args.get("cdp_debugging_port").is_some() {
            return ToolResult::error(
                "cdp_debugging_port moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }
        let webkit_inspector_port = args.opt_u64("webkit_inspector_port").map(|v| v as u16);
        let instance_policy = match resolve_instance_policy(&args) {
            Ok(policy) => policy,
            Err(error) => return error,
        };
        let creates_new_instance = matches!(&instance_policy, InstancePolicy::New);
        let additional_arguments: Vec<String> = args.str_array("additional_arguments");
        if additional_arguments
            .iter()
            .any(|argument| argument == super::check_permissions::PERMISSIONS_HOST_REQUEST_ARG)
        {
            return protected_host_launch_refusal();
        }
        if additional_arguments
            .iter()
            .any(|argument| contains_remote_debugging_flag(argument))
        {
            return ToolResult::error(
                "Chromium remote-debugging flags moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch.",
            );
        }
        if bundle_id.as_deref().is_some_and(is_cua_driver_bundle_id) {
            return protected_host_launch_refusal();
        }
        let target_is_launchable = if let Some(ref bid) = bundle_id {
            crate::apps::resolve_bundle_id_to_locator(bid).is_some()
        } else if let Some(ref n) = name {
            if let Some(locator) = crate::apps::locate_by_name(n) {
                let (_, resolved_bundle_id) = locator.app_ref_and_bundle_id();
                response_bundle_id = resolved_bundle_id.clone();
                if resolved_bundle_id
                    .as_deref()
                    .is_some_and(is_cua_driver_bundle_id)
                {
                    return protected_host_launch_refusal();
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if let Some(err) = preflight_file_urls(&urls) {
            return err;
        }

        // Build env dict for webkit inspector.
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if let Some(port) = webkit_inspector_port {
            env.insert(
                "WEBKIT_INSPECTOR_SERVER".to_string(),
                format!("127.0.0.1:{port}"),
            );
            env.insert("TAURI_WEBVIEW_AUTOMATION".to_string(), "1".to_string());
        }

        let port_summary = {
            let mut s = String::new();
            if let Some(port) = webkit_inspector_port {
                s.push_str(&format!("\nWebKit inspector available on port {port}."));
            }
            s
        };

        // Serialize the exact resolve/reuse/launch decision inside one driver
        // runtime. Without this critical section, two concurrent MCP calls can
        // both observe "not running" and issue duplicate launches.
        let acquisition_guard = APP_ACQUISITION_LOCK.lock().await;
        let existing_apps = matching_running_apps(
            crate::apps::list_running_apps(),
            response_bundle_id.as_deref(),
            response_requested_name.as_deref(),
        );
        let existing_candidates: Vec<_> = existing_apps
            .iter()
            .map(|app| (app.clone(), windows_for_pid(app.pid)))
            .collect();
        let has_launch_directives = !urls.is_empty()
            || !additional_arguments.is_empty()
            || !env.is_empty()
            || webkit_inspector_port.is_some();

        match prelaunch_decision(
            &instance_policy,
            has_launch_directives,
            &existing_candidates,
        ) {
            PrelaunchDecision::Ambiguous => {
                return ambiguous_app_target(&instance_policy, &existing_candidates);
            }
            PrelaunchDecision::Reuse => {
                let (app, windows) = existing_candidates
                    .first()
                    .expect("reuse decision requires exactly one reusable candidate");
                let (app_name, bid) = response_identity(
                    Some(app),
                    response_bundle_id.as_deref(),
                    response_requested_name.as_deref(),
                );
                return successful_acquisition(
                    app.pid,
                    app_name,
                    bid,
                    windows,
                    &port_summary,
                    false,
                    ProcessDisposition::Reused,
                    WindowDisposition::Reused,
                    None,
                    &instance_policy,
                );
            }
            PrelaunchDecision::ReuseUnavailable => {
                return reuse_only_unavailable(
                    existing_candidates.first(),
                    has_launch_directives,
                    &instance_policy,
                );
            }
            PrelaunchDecision::SendRequest => {}
        }

        // A running app can still be reused even when LaunchServices cannot
        // currently resolve its installed bundle (for example, an app on an
        // unmounted registration path). Only require an installed launch
        // target once this call actually needs to send a request.
        if !target_is_launchable {
            return if let Some(ref bid) = bundle_id {
                structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for bundle_id '{bid}'."),
                    serde_json::json!({ "bundle_id": bid }),
                )
            } else {
                let requested_name = name.as_deref().unwrap_or("?");
                structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for name '{requested_name}'."),
                    serde_json::json!({ "name": requested_name }),
                )
            };
        }

        let preexisting_pids: HashSet<i32> = existing_apps.iter().map(|app| app.pid).collect();
        let preexisting_window_ids: HashSet<u32> = existing_candidates
            .iter()
            .flat_map(|(_, windows)| windows.iter().map(|window| window.window_id))
            .collect();

        // ── Layer-3 focus-steal suppression (3-phase wrap) ───────────────
        //
        // Captures the prior frontmost pid, arms a wildcard suppression
        // BEFORE the launch (covers self-activations the target fires
        // synchronously during `open()`), then upgrades to a targeted
        // suppression keyed to the actual launched pid. Briefly holds
        // BOTH leases so a self-activation arriving in the wildcard→
        // targeted gap is still caught — that race is what hoang17's
        // Swift PR #1521 explicitly fixes; we do not regress it here.
        //
        // After 500ms (enough for `applicationDidFinishLaunching` +
        // any reflex `NSApp.activate(...)` to fire and get suppressed)
        // both leases are dropped. The belt-and-braces step at the end
        // re-activates the prior frontmost if the target is still
        // frontmost — handles the intra-`open()` synchronous activation
        // that fired before we could arm with the real pid.
        let prior_frontmost = crate::apps::frontmost_pid();
        let finder_folder_handoff = response_bundle_id.as_deref().is_some_and(|bundle_id| {
            additional_arguments.is_empty()
                && env.is_empty()
                && !creates_new_instance
                && crate::apps::finder_folder_handoff(bundle_id, &urls)
        });

        // Finder's synchronous folder-open selector must be allowed to activate
        // long enough to perform the request. Use the ordinary targeted
        // post-launch guard to restore the prior foreground app immediately.
        let wildcard_lease = prior_frontmost
            .filter(|_| !finder_folder_handoff)
            .map(|prior| {
                crate::focus_steal::FocusStealPreventer::begin_suppression(
                    None,
                    prior,
                    "LaunchAppTool.pre",
                )
            });

        // Predicate captured BEFORE moving inputs into spawn_blocking.
        // Same condition that selects the `openURLs:withApplicationAtURL:`
        // chain over the simpler `openApplicationAtURL:` path. Used after
        // the spawn returns to size the suppression window — the slow
        // path triggers a SECOND activation when the file-open delivers,
        // which lands AFTER the bundle-only-launch activation window.
        let slow_launch_path = !urls.is_empty()
            || !additional_arguments.is_empty()
            || !env.is_empty()
            || creates_new_instance;

        // Move the launch closure inputs into spawn_blocking. The
        // blocking task returns (pid, app_info, windows). Suppression
        // upgrade happens AFTER the blocking call returns (back on the
        // async runtime), then we sleep holding the targeted lease.
        let validation_pids = preexisting_pids.clone();
        let launch_result = tokio::task::spawn_blocking(move || {
            let pid = if let Some(ref bid) = bundle_id {
                if urls.is_empty()
                    && additional_arguments.is_empty()
                    && env.is_empty()
                    && !creates_new_instance
                {
                    crate::apps::launch_app(bid)?
                } else {
                    crate::apps::launch_with_urls_by_bundle(
                        bid,
                        &urls,
                        &additional_arguments,
                        &env,
                        creates_new_instance,
                    )?
                }
            } else {
                let n = name.as_deref().unwrap();
                if urls.is_empty()
                    && additional_arguments.is_empty()
                    && env.is_empty()
                    && !creates_new_instance
                {
                    crate::apps::launch_app_by_name(n)?
                } else {
                    crate::apps::launch_with_urls_by_name(
                        n,
                        &urls,
                        &additional_arguments,
                        &env,
                        creates_new_instance,
                    )?
                }
            };

            let pid = validate_launched_pid(pid, creates_new_instance, &validation_pids)?;

            // Retry loop: LaunchServices returns before WindowServer has
            // registered the new windows. Poll up to 5x100ms.
            let windows = resolve_windows_for_pid(pid);

            let app_info: Option<crate::apps::AppInfo> = {
                let apps = crate::apps::list_running_apps();
                apps.into_iter().find(|a| a.pid == pid)
            };

            Ok::<_, anyhow::Error>((pid, app_info, windows))
        })
        .await;
        drop(acquisition_guard);

        // Upgrade to targeted suppression now that we know the real pid.
        // Keep the wildcard lease alive until immediately AFTER we've
        // armed the targeted one — that's the PR #1521 overlap window.
        //
        // `self_activation_suppressed` is the outcome of the belt-and-
        // braces demotion check: `None` when the check didn't run
        // (no prior frontmost / launch failed / pid == prior), `Some(true)`
        // when the target was NOT frontmost after the suppression window
        // (or we successfully re-demoted it), `Some(false)` when the
        // re-demote failed and the target is still stealing focus.
        // Surfaced in the structured response so callers can observe
        // whether focus-steal prevention actually held.
        let mut self_activation_suppressed: Option<bool> = None;
        if let Ok(Ok((pid, _, _))) = &launch_result {
            if let Some(prior) = prior_frontmost {
                if *pid != prior {
                    let targeted_lease = crate::focus_steal::FocusStealPreventer::begin_suppression(
                        Some(*pid),
                        prior,
                        "LaunchAppTool.post",
                    );
                    // Now safe to drop the wildcard — targeted is armed.
                    drop(wildcard_lease);
                    // Hold the targeted lease long enough to cover the
                    // ENTIRE post-launch activation window.
                    //
                    // - Fast path (bundle-only launch, no urls/args/env):
                    //   500ms covers `applicationDidFinishLaunching` plus
                    //   any reflex `NSApp.activate(...)`. Matches Swift
                    //   LaunchAppTool.swift exactly.
                    //
                    // - Slow path (urls / additional_arguments / env /
                    //   creates_new_instance): 2500ms. The slow-path
                    //   `openURLs:withApplicationAtURL:` chain triggers a
                    //   second activation when the file-open delivers to
                    //   the just-launched app — Electron apps (VSCode,
                    //   Cursor, Slack) re-`app.focus()` from inside their
                    //   `open-file` JS handler, AFTER our 500ms window
                    //   would have already closed. Empirically VSCode's
                    //   late activation can land anywhere from ~700ms to
                    //   ~2000ms after the openURLs return. The observer-
                    //   based lease catches any activation that lands
                    //   WHILE held, so widening the window converts the
                    //   late activation from a contract violation into
                    //   another auto-demote.
                    let window_ms: u64 = if slow_launch_path { 2500 } else { 500 };
                    tokio::time::sleep(std::time::Duration::from_millis(window_ms)).await;
                    drop(targeted_lease);

                    // Belt-and-braces LOOP: if the target ever pops back
                    // to the foreground after the lease drops (rare —
                    // observer already covered the suppression window —
                    // but happens when the activation fires literally on
                    // the same tokio tick the lease dropped), demote it.
                    // Loop 5x200ms = 1s of post-window coverage. Each
                    // iteration is cheap (one frontmost_pid + maybe one
                    // activate_pid call) so this stays well under the
                    // RPC budget even when the demote keeps working.
                    let mut demotion_succeeded = true;
                    for _ in 0..5 {
                        let frontmost_now = crate::apps::frontmost_pid();
                        if frontmost_now != Some(*pid) {
                            // Not frontmost — nothing to do this tick.
                            continue;
                        }
                        let activated = crate::apps::activate_pid(prior);
                        let still_frontmost = crate::apps::frontmost_pid() == Some(*pid);
                        if still_frontmost {
                            tracing::warn!(
                                target: "platform_macos::tools::launch_app",
                                launched_pid = *pid,
                                prior_pid = prior,
                                activate_pid_returned = activated,
                                "belt-and-braces demotion iteration failed: \
                                 launched app remained frontmost after \
                                 re-activating prior — will retry"
                            );
                            demotion_succeeded = false;
                        } else {
                            demotion_succeeded = true;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    // Final in-call state determines the structured response.
                    let final_frontmost = crate::apps::frontmost_pid();
                    self_activation_suppressed =
                        Some(final_frontmost != Some(*pid) && demotion_succeeded);

                    // Detached late-activation watchdog (slow path only).
                    //
                    // Why: Electron apps with no workspace open (cold-
                    // launched VSCode / Cursor / Slack with a file URL)
                    // re-activate AGAIN when their Welcome window
                    // finishes loading — empirically 4-8 seconds after
                    // the `openURLs:withApplicationAtURL:` call returns.
                    // That's well past the in-call suppression window
                    // and any reasonable extension of it that an agent
                    // workflow would tolerate as caller latency.
                    //
                    // Solution: hold a fresh observer-backed lease in
                    // the background for ~8s, demoting if the launched
                    // pid pops back. The caller doesn't wait — the tool
                    // already returned its honest `self_activation_
                    // suppressed` for the in-call window. The detached
                    // task just keeps the no-foreground-steal contract
                    // honored past the RPC boundary.
                    //
                    // Note on process lifecycle: this watchdog only runs
                    // when the tokio runtime stays alive — i.e. in the
                    // long-running `cua-driver mcp` / `cua-driver serve`
                    // daemon modes. The one-shot `cua-driver call` mode
                    // exits as soon as the tool returns, taking the
                    // detached task with it. Acceptable because the
                    // contract is "no foreground steal during a session
                    // the agent is driving" — `cua-driver call` doesn't
                    // have a session that outlives the call.
                    //
                    // Tradeoffs:
                    // - Caller latency unchanged (~2.5s for slow path).
                    // - Total observer coverage: ~10.5s post-launch.
                    // - CPU: the observer fires per activation event,
                    //   not per poll; the 250ms tick is just for the
                    //   manual belt-and-braces demote. Cheap.
                    // - If a legitimate user click activates Code while
                    //   the watchdog is alive, we'll demote them. Worst
                    //   case ~10s of "I clicked Code and it didn't come
                    //   forward" — acceptable trade for an automation
                    //   scenario where the agent just launched it.
                    if slow_launch_path {
                        let launched_pid = *pid;
                        let prior_pid = prior;
                        tokio::spawn(async move {
                            let _lease = crate::focus_steal::FocusStealPreventer::begin_suppression(
                                Some(launched_pid),
                                prior_pid,
                                "LaunchAppTool.watchdog",
                            );
                            let mut late_activations = 0u32;
                            for _ in 0..32 {
                                // 32 × 250ms = 8s
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                if crate::apps::frontmost_pid() == Some(launched_pid) {
                                    late_activations += 1;
                                    let _ = crate::apps::activate_pid(prior_pid);
                                }
                            }
                            if late_activations > 0 {
                                tracing::warn!(
                                    target: "platform_macos::tools::launch_app",
                                    launched_pid,
                                    prior_pid,
                                    late_activations,
                                    "watchdog demoted post-RPC late activations \
                                     — slow-path window may need tuning"
                                );
                            }
                        });
                    }
                } else {
                    // pid == prior frontmost (re-launch of an already-
                    // frontmost app). Just drop the wildcard.
                    drop(wildcard_lease);
                }
            }
        } else {
            // Launch failed; just drop the lease.
            drop(wildcard_lease);
        }

        match launch_result {
            Ok(Ok((pid, app_info, windows))) => {
                let (app_name, bid) = response_identity(
                    app_info.as_ref(),
                    response_bundle_id.as_deref(),
                    response_requested_name.as_deref(),
                );

                let process_disposition = if preexisting_pids.contains(&pid) {
                    ProcessDisposition::Reused
                } else {
                    ProcessDisposition::Created
                };
                let window_disposition = if windows.is_empty() {
                    WindowDisposition::None
                } else if windows
                    .iter()
                    .any(|window| !preexisting_window_ids.contains(&window.window_id))
                {
                    WindowDisposition::Materialized
                } else {
                    WindowDisposition::Reused
                };

                successful_acquisition(
                    pid,
                    app_name,
                    bid,
                    &windows,
                    &port_summary,
                    true,
                    process_disposition,
                    window_disposition,
                    self_activation_suppressed,
                    &instance_policy,
                )
            }
            Ok(Err(e)) => {
                let existing_app = creates_new_instance
                    .then(|| existing_apps.first())
                    .flatten();
                structured_launch_failure(&e, existing_app)
            }
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn contains_remote_debugging_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("--remote-debugging-port") || lower.contains("--remote-debugging-pipe")
}

fn is_cua_driver_bundle_id(bundle_id: &str) -> bool {
    matches!(bundle_id, "com.trycua.driver" | "com.trycua.driver.local")
}

fn protected_host_launch_refusal() -> ToolResult {
    structured_launch_error(
        "PROTECTED_HOST_ENTRYPOINT",
        "launch_app cannot launch Cua Driver's protected host; operating-system permission UI must originate outside the agent tool stream".to_owned(),
        serde_json::json!({}),
    )
}

// ── Blocking helpers ──────────────────────────────────────────────────────────

/// Poll for the pid's layer-0 windows, retrying up to 5x100ms to absorb
/// LaunchServices → WindowServer latency (mirrors the Swift reference).
fn resolve_windows_for_pid(pid: i32) -> Vec<crate::windows::WindowInfo> {
    for attempt in 0..5 {
        let found = windows_for_pid(pid);
        if !found.is_empty() {
            return found;
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    vec![]
}

fn windows_for_pid(pid: i32) -> Vec<crate::windows::WindowInfo> {
    crate::windows::all_windows()
        .into_iter()
        .filter(|window| window.pid == pid && window.layer == 0)
        .filter(|window| window.bounds.width > 1.0 && window.bounds.height > 1.0)
        .collect()
}

fn structured_launch_error(code: &str, message: String, details: serde_json::Value) -> ToolResult {
    let mut payload = serde_json::json!({
        "error": code,
    });

    match details {
        serde_json::Value::Object(details) => {
            if let serde_json::Value::Object(payload) = &mut payload {
                payload.extend(details);
            }
        }
        details => {
            if let serde_json::Value::Object(payload) = &mut payload {
                payload.insert("details".to_string(), details);
            }
        }
    }

    ToolResult::error(message).with_structured(payload)
}

fn matching_running_apps(
    apps: Vec<crate::apps::AppInfo>,
    requested_bundle_id: Option<&str>,
    requested_name: Option<&str>,
) -> Vec<crate::apps::AppInfo> {
    let normalized_name = requested_app_name(requested_name, requested_bundle_id);
    apps.into_iter()
        .filter(|app| {
            requested_bundle_id.map_or_else(
                || app.name.eq_ignore_ascii_case(&normalized_name),
                |bundle_id| {
                    app.bundle_id.as_deref().is_some_and(|running_bundle_id| {
                        running_bundle_id.eq_ignore_ascii_case(bundle_id)
                    })
                },
            )
        })
        .collect()
}

fn candidate_json(app: &crate::apps::AppInfo, windows: &[crate::windows::WindowInfo]) -> Value {
    serde_json::json!({
        "pid": app.pid,
        "bundle_id": app.bundle_id,
        "name": app.name,
        "windows": windows
            .iter()
            .map(super::list_windows::window_record_json)
            .collect::<Vec<_>>(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrelaunchDecision {
    Reuse,
    Ambiguous,
    ReuseUnavailable,
    SendRequest,
}

fn prelaunch_decision(
    policy: &InstancePolicy,
    has_launch_directives: bool,
    candidates: &[(crate::apps::AppInfo, Vec<crate::windows::WindowInfo>)],
) -> PrelaunchDecision {
    if matches!(policy, InstancePolicy::New) {
        return PrelaunchDecision::SendRequest;
    }
    if candidates.len() > 1 {
        return PrelaunchDecision::Ambiguous;
    }
    if candidates
        .first()
        .is_some_and(|(_, windows)| !has_launch_directives && !windows.is_empty())
    {
        return PrelaunchDecision::Reuse;
    }
    if matches!(policy, InstancePolicy::ReuseOnly) {
        return PrelaunchDecision::ReuseUnavailable;
    }
    PrelaunchDecision::SendRequest
}

fn ambiguous_app_target(
    policy: &InstancePolicy,
    candidates: &[(crate::apps::AppInfo, Vec<crate::windows::WindowInfo>)],
) -> ToolResult {
    structured_launch_error(
        "APP_TARGET_AMBIGUOUS",
        format!(
            "launch_app found {} exact running app processes. Choose an explicit pid/window from candidates or use instance_policy=\"new\" when isolation is required.",
            candidates.len()
        ),
        serde_json::json!({
            "instance_policy": policy.as_str(),
            "candidates": candidates
                .iter()
                .map(|(app, windows)| candidate_json(app, windows))
                .collect::<Vec<_>>(),
            "launch_state": launch_state_json(
                false,
                true,
                false,
                ProcessDisposition::None,
                WindowDisposition::None,
            ),
        }),
    )
}

fn reuse_only_unavailable(
    candidate: Option<&(crate::apps::AppInfo, Vec<crate::windows::WindowInfo>)>,
    has_launch_directives: bool,
    policy: &InstancePolicy,
) -> ToolResult {
    let message = match candidate {
        Some(_) if has_launch_directives => {
            "instance_policy=\"reuse_only\" cannot deliver urls, arguments, or environment without sending an app-open request. No request was sent."
        }
        Some(_) => {
            "The exact app process is running but has no reusable layer-0 window, and instance_policy=\"reuse_only\" forbids an app-open request."
        }
        None => {
            "No exact running app process with a reusable window was found, and instance_policy=\"reuse_only\" forbids launching one."
        }
    };
    let process_running = candidate.is_some();
    let mut details = serde_json::json!({
        "instance_policy": policy.as_str(),
        "launch_state": launch_state_json(
            false,
            process_running,
            false,
            if process_running {
                ProcessDisposition::Reused
            } else {
                ProcessDisposition::None
            },
            WindowDisposition::None,
        ),
    });
    if let Some((app, windows)) = candidate {
        details["candidate"] = candidate_json(app, windows);
    }
    structured_launch_error("APP_REUSE_UNAVAILABLE", message.to_owned(), details)
}

#[allow(clippy::too_many_arguments)]
fn successful_acquisition(
    pid: i32,
    app_name: String,
    bundle_id: String,
    windows: &[crate::windows::WindowInfo],
    port_summary: &str,
    request_sent: bool,
    process_disposition: ProcessDisposition,
    window_disposition: WindowDisposition,
    self_activation_suppressed: Option<bool>,
    policy: &InstancePolicy,
) -> ToolResult {
    let mut summary = match (request_sent, process_disposition) {
        (false, _) => format!("Reused {app_name} (pid {pid}) without sending a launch request."),
        (true, ProcessDisposition::Reused) => {
            format!("Opened {app_name} through its existing process (pid {pid}) in background.{port_summary}")
        }
        _ => format!("Launched {app_name} (pid {pid}) in background.{port_summary}"),
    };

    if !windows.is_empty() {
        summary.push_str("\n\nWindows:");
        for window in windows {
            let title = if window.title.is_empty() {
                "(no title)".to_owned()
            } else {
                format!("\"{}\"", window.title)
            };
            summary.push_str(&format!("\n- {title} [window_id: {}]", window.window_id));
        }
        summary.push_str(&format!(
            "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
        ));
    }

    let windows_json: Vec<Value> = windows
        .iter()
        .map(super::list_windows::window_record_json)
        .collect();
    let mut structured = serde_json::json!({
        "pid": pid,
        "bundle_id": bundle_id,
        "name": app_name,
        "windows": windows_json,
        "instance_policy": policy.as_str(),
        "launch_state": launch_state_json(
            request_sent,
            true,
            !windows.is_empty(),
            process_disposition,
            window_disposition,
        ),
    });
    if let Some(suppressed) = self_activation_suppressed {
        structured["self_activation_suppressed"] = Value::Bool(suppressed);
    }
    ToolResult::text(summary).with_structured(structured)
}

fn response_identity(
    app_info: Option<&crate::apps::AppInfo>,
    requested_bundle_id: Option<&str>,
    requested_name: Option<&str>,
) -> (String, String) {
    let bundle_id = app_info
        .and_then(|app| app.bundle_id.as_deref())
        .filter(|value| !value.is_empty())
        .or(requested_bundle_id)
        .unwrap_or("?")
        .to_owned();

    let name = app_info
        .map(|app| app.name.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| requested_app_name(requested_name, requested_bundle_id));

    (name, bundle_id)
}

fn requested_app_name(requested_name: Option<&str>, requested_bundle_id: Option<&str>) -> String {
    if let Some(name) = requested_name.filter(|name| Some(*name) != requested_bundle_id) {
        let file_name = std::path::Path::new(name)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(name);
        return file_name
            .strip_suffix(".app")
            .unwrap_or(file_name)
            .to_owned();
    }

    requested_bundle_id
        .and_then(|bundle_id| bundle_id.rsplit('.').next())
        .filter(|name| !name.is_empty())
        .unwrap_or("?")
        .to_owned()
}

fn validate_launched_pid(
    pid: i32,
    creates_new_instance: bool,
    preexisting_pids: &HashSet<i32>,
) -> anyhow::Result<i32> {
    if creates_new_instance && pid <= 0 {
        anyhow::bail!(
            "macOS returned invalid process identifier {pid} for the requested new application instance"
        );
    }
    if creates_new_instance && preexisting_pids.contains(&pid) {
        anyhow::bail!(
            "macOS returned existing process identifier {pid} instead of creating the requested new application instance"
        );
    }
    Ok(pid)
}

fn structured_launch_failure(
    error: &anyhow::Error,
    existing_app: Option<&crate::apps::AppInfo>,
) -> ToolResult {
    use crate::apps::nsworkspace::LaunchError;

    let launch_error = error.downcast_ref::<LaunchError>();
    if !matches!(launch_error, Some(LaunchError::BadUrl(_))) {
        if let Some(existing_app) = existing_app {
            let bundle_id = existing_app.bundle_id.as_deref().unwrap_or("?");
            return structured_launch_error(
                "NEW_APPLICATION_INSTANCE_UNAVAILABLE",
                format!(
                    "macOS did not create a new application instance for {}. The existing process (pid {}) is still running; retry without creates_new_application_instance or choose an app that supports multiple instances. Underlying launch error: {error:#}",
                    existing_app.name, existing_app.pid
                ),
                serde_json::json!({
                    "bundle_id": bundle_id,
                    "name": existing_app.name,
                    "pid": existing_app.pid,
                    "creates_new_application_instance": true,
                    "instance_policy": "new",
                    "launch_state": launch_state_json(
                        true,
                        true,
                        false,
                        ProcessDisposition::Reused,
                        WindowDisposition::None,
                    ),
                }),
            );
        }
    }

    let (code, requested) = if let Some(launch_error) = launch_error {
        match launch_error {
            LaunchError::Cocoa(_) => ("NSWORKSPACE_LAUNCH_FAILED", true),
            LaunchError::NoApp => ("LAUNCH_RESULT_MISSING", true),
            LaunchError::Timeout => ("LAUNCH_CALLBACK_TIMEOUT", true),
            LaunchError::BadUrl(_) => ("APP_URL_INVALID", false),
        }
    } else {
        ("LAUNCH_FAILED", false)
    };

    structured_launch_error(
        code,
        format!("Launch failed: {error:#}"),
        serde_json::json!({
            "launch_state": launch_state_json(
                requested,
                false,
                false,
                ProcessDisposition::None,
                WindowDisposition::None,
            ),
        }),
    )
}

fn preflight_file_urls(urls: &[String]) -> Option<ToolResult> {
    for raw in urls {
        let Some(path) = local_file_target(raw) else {
            continue;
        };
        if !path.exists() {
            return Some(structured_launch_error(
                "FILE_NOT_FOUND",
                format!(
                    "Local launch_app url target does not exist: {}",
                    path.display()
                ),
                serde_json::json!({
                    "url": raw,
                    "path": path.display().to_string(),
                }),
            ));
        }
    }
    None
}

fn normalize_launch_url(raw: String) -> String {
    local_file_target(&raw)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or(raw)
}

fn local_file_target(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() {
        return Some(PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode_path(path);
        return Some(expand_tilde(&decoded));
    }
    let looks_like_url = raw.contains(':') && !raw.starts_with('/') && !raw.starts_with('~');
    if looks_like_url {
        return None;
    }
    Some(expand_tilde(raw))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn percent_decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push((high << 4) | low);
                i += 3;
                continue;
            }
        }

        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        contains_remote_debugging_flag, is_cua_driver_bundle_id, local_file_target,
        matching_running_apps, normalize_launch_url, preflight_file_urls, prelaunch_decision,
        response_identity, reuse_only_unavailable, structured_launch_failure,
        successful_acquisition, validate_launched_pid, LaunchAppTool, PrelaunchDecision,
    };
    use cua_driver_core::protocol::Content;
    use cua_driver_core::resolve_instance_policy;
    use cua_driver_core::tool::Tool;
    use cua_driver_core::{InstancePolicy, ProcessDisposition, WindowDisposition};
    use serde_json::json;
    use std::{collections::HashSet, path::PathBuf};

    fn app(name: &str, pid: i32, bundle_id: Option<&str>) -> crate::apps::AppInfo {
        crate::apps::AppInfo {
            name: name.to_owned(),
            pid,
            bundle_id: bundle_id.map(str::to_owned),
            running: true,
            active: false,
            launch_path: None,
            kind: Some("desktop".to_owned()),
            last_used: None,
        }
    }

    fn window(pid: i32, window_id: u32) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id,
            pid,
            app_name: "Example".to_owned(),
            title: "Document".to_owned(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            layer: 0,
            z_index: 0,
            is_on_screen: true,
            current_space_id: Some(1),
            on_current_space: Some(true),
            space_ids: Some(vec![1]),
        }
    }

    #[test]
    fn launch_schema_exposes_session_and_canonical_instance_policy() {
        let properties = LaunchAppTool
            .def()
            .input_schema
            .get("properties")
            .expect("properties");
        assert_eq!(
            properties["session"],
            cua_driver_core::tool_schema::session_schema()
        );
        assert_eq!(
            properties["instance_policy"],
            cua_driver_core::tool_schema::instance_policy_schema()
        );
    }

    #[test]
    fn instance_policy_maps_legacy_new_and_rejects_conflicts() {
        assert_eq!(
            resolve_instance_policy(&json!({}))
                .expect("default policy")
                .as_str(),
            "reuse_or_launch"
        );
        assert_eq!(
            resolve_instance_policy(&json!({"creates_new_application_instance": true}))
                .expect("legacy true")
                .as_str(),
            "new"
        );
        assert_eq!(
            resolve_instance_policy(&json!({
                "instance_policy": "new",
                "creates_new_application_instance": false
            }))
            .expect("explicit policy wins over legacy false")
            .as_str(),
            "new"
        );

        let error = resolve_instance_policy(&json!({
            "instance_policy": "reuse_only",
            "creates_new_application_instance": true
        }))
        .expect_err("conflicting policy must fail");
        assert_eq!(
            error.structured_content.expect("structured error")["error"],
            "INSTANCE_POLICY_CONFLICT"
        );
    }

    #[test]
    fn exact_running_match_prefers_bundle_id_and_normalizes_app_name() {
        let apps = vec![
            app("Example", 10, Some("com.example.Editor")),
            app("Example Helper", 11, Some("com.example.Editor.Helper")),
        ];
        let matched =
            matching_running_apps(apps.clone(), Some("COM.EXAMPLE.EDITOR"), Some("Wrong Name"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].pid, 10);

        let matched = matching_running_apps(apps, None, Some("/Applications/Example.app"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].pid, 10);
    }

    #[test]
    fn reuse_first_decision_avoids_requests_and_refuses_ambiguity() {
        let one_window = vec![(
            app("Example", 10, Some("com.example.Editor")),
            vec![window(10, 20)],
        )];
        assert_eq!(
            prelaunch_decision(&InstancePolicy::ReuseOrLaunch, false, &one_window),
            PrelaunchDecision::Reuse
        );
        assert_eq!(
            prelaunch_decision(&InstancePolicy::ReuseOnly, false, &one_window),
            PrelaunchDecision::Reuse
        );
        assert_eq!(
            prelaunch_decision(&InstancePolicy::ReuseOnly, true, &one_window),
            PrelaunchDecision::ReuseUnavailable
        );

        let multiple = vec![
            (
                app("Example", 10, Some("com.example.Editor")),
                vec![window(10, 20)],
            ),
            (
                app("Example", 11, Some("com.example.Editor")),
                vec![window(11, 21)],
            ),
        ];
        assert_eq!(
            prelaunch_decision(&InstancePolicy::ReuseOrLaunch, false, &multiple),
            PrelaunchDecision::Ambiguous
        );
        assert_eq!(
            prelaunch_decision(&InstancePolicy::New, false, &multiple),
            PrelaunchDecision::SendRequest
        );
    }

    #[test]
    fn reused_success_reports_that_no_launch_request_was_sent() {
        let result = successful_acquisition(
            10,
            "Example".to_owned(),
            "com.example.Editor".to_owned(),
            &[window(10, 20)],
            "",
            false,
            ProcessDisposition::Reused,
            WindowDisposition::Reused,
            None,
            &InstancePolicy::ReuseOrLaunch,
        );
        let structured = result.structured_content.expect("structured result");
        assert_eq!(structured["launch_state"]["requested"], false);
        assert_eq!(structured["launch_state"]["request_sent"], false);
        assert_eq!(structured["launch_state"]["process_disposition"], "reused");
        assert_eq!(structured["launch_state"]["window_disposition"], "reused");
        assert!(result.content.iter().any(|content| matches!(
            content,
            Content::Text { text, .. } if text.contains("Reused Example")
        )));
    }

    #[test]
    fn reuse_only_refusal_never_claims_a_request_was_sent() {
        let candidate = (app("Example", 10, Some("com.example.Editor")), vec![]);
        let result = reuse_only_unavailable(Some(&candidate), false, &InstancePolicy::ReuseOnly);
        let structured = result.structured_content.expect("structured error");
        assert_eq!(structured["error"], "APP_REUSE_UNAVAILABLE");
        assert_eq!(structured["launch_state"]["request_sent"], false);
        assert_eq!(structured["launch_state"]["process_running"], true);
    }

    #[test]
    fn local_file_target_treats_plain_paths_as_files() {
        assert_eq!(
            local_file_target("/tmp/does-not-exist.md"),
            Some(PathBuf::from("/tmp/does-not-exist.md"))
        );
        assert_eq!(
            local_file_target("relative/path.md"),
            Some(PathBuf::from("relative/path.md"))
        );
    }

    #[test]
    fn launch_url_normalization_expands_home_relative_paths() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME must be set for macOS"));

        assert_eq!(
            PathBuf::from(normalize_launch_url("~/Desktop/BenchInbox".to_owned())),
            home.join("Desktop/BenchInbox")
        );
        assert_eq!(
            normalize_launch_url("https://example.com".to_owned()),
            "https://example.com"
        );
    }

    #[test]
    fn local_file_target_skips_remote_and_custom_schemes() {
        assert_eq!(local_file_target("https://example.com"), None);
        assert_eq!(local_file_target("about:blank"), None);
        assert_eq!(local_file_target("myapp://open/item"), None);
    }

    #[test]
    fn preflight_file_urls_returns_structured_file_not_found() {
        let missing = "/tmp/cua-driver-definitely-missing-file-for-test.md".to_string();
        let result = preflight_file_urls(&[missing]).expect("missing file should error");
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured error");
        assert_eq!(structured["error"], "FILE_NOT_FOUND");
        assert_eq!(
            structured["path"],
            "/tmp/cua-driver-definitely-missing-file-for-test.md"
        );
        assert!(structured.get("details").is_none());
    }

    #[test]
    fn local_file_target_percent_decodes_file_urls_before_path_checks() {
        assert_eq!(
            local_file_target("file:///tmp/My%20Doc.txt"),
            Some(PathBuf::from("/tmp/My Doc.txt"))
        );
        assert_eq!(
            local_file_target("file://localhost/tmp/%E2%9C%93.txt"),
            Some(PathBuf::from("/tmp/✓.txt"))
        );
    }

    #[test]
    fn rejects_all_chromium_remote_debugging_spellings() {
        assert!(contains_remote_debugging_flag("--remote-debugging-port=0"));
        assert!(contains_remote_debugging_flag("--REMOTE-DEBUGGING-PIPE"));
        assert!(!contains_remote_debugging_flag(
            "--user-data-dir=/tmp/profile"
        ));
    }

    #[test]
    fn recognizes_release_and_local_protected_host_bundle_ids() {
        assert!(is_cua_driver_bundle_id("com.trycua.driver"));
        assert!(is_cua_driver_bundle_id("com.trycua.driver.local"));
        assert!(!is_cua_driver_bundle_id("com.trycua.harness.tauri"));
    }

    #[test]
    fn launch_timeout_reports_requested_without_process_or_window() {
        let error = anyhow::Error::new(crate::apps::nsworkspace::LaunchError::Timeout)
            .context("Failed to launch com.example.App");
        let result = structured_launch_failure(&error, None);
        let structured = result.structured_content.expect("structured error");

        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured["error"], "LAUNCH_CALLBACK_TIMEOUT");
        assert_eq!(structured["launch_state"]["requested"], true);
        assert_eq!(structured["launch_state"]["process_running"], false);
        assert_eq!(structured["launch_state"]["window_ready"], false);
    }

    #[test]
    fn invalid_url_reports_request_was_not_sent() {
        let error = anyhow::Error::new(crate::apps::nsworkspace::LaunchError::BadUrl(
            "bad url".to_owned(),
        ))
        .context("Failed to launch com.example.App");
        let result = structured_launch_failure(&error, None);
        let structured = result.structured_content.expect("structured error");

        assert_eq!(structured["error"], "APP_URL_INVALID");
        assert_eq!(structured["launch_state"]["requested"], false);
        assert_eq!(structured["launch_state"]["process_running"], false);
        assert_eq!(structured["launch_state"]["window_ready"], false);
    }

    #[test]
    fn unavailable_new_instance_reports_existing_process_and_retry() {
        let error = anyhow::Error::new(crate::apps::nsworkspace::LaunchError::Cocoa(
            "The application could not be launched because it was not found.".to_owned(),
        ))
        .context("Failed to launch com.example.SingleInstance");
        let existing_app = crate::apps::AppInfo {
            name: "Single Instance".to_owned(),
            pid: 4242,
            bundle_id: Some("com.example.SingleInstance".to_owned()),
            running: true,
            active: false,
            launch_path: None,
            kind: Some("desktop".to_owned()),
            last_used: None,
        };

        let result = structured_launch_failure(&error, Some(&existing_app));
        let structured = result.structured_content.expect("structured error");

        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured["error"], "NEW_APPLICATION_INSTANCE_UNAVAILABLE");
        assert_eq!(structured["bundle_id"], "com.example.SingleInstance");
        assert_eq!(structured["pid"], 4242);
        assert_eq!(structured["creates_new_application_instance"], true);
        assert_eq!(structured["launch_state"]["requested"], true);
        assert_eq!(structured["launch_state"]["process_running"], true);
        assert_eq!(structured["launch_state"]["window_ready"], false);
        assert!(result.content.iter().any(|content| matches!(
            content,
            Content::Text { text, .. }
                if text.contains("retry without creates_new_application_instance")
        )));
    }

    #[test]
    fn new_instance_cannot_report_success_without_a_process() {
        let existing = HashSet::from([4242]);
        let error = validate_launched_pid(-1, true, &existing).expect_err("invalid pid must fail");
        assert!(error
            .to_string()
            .contains("invalid process identifier -1 for the requested new application instance"));
        let error = validate_launched_pid(4242, true, &existing)
            .expect_err("existing pid cannot satisfy an isolated launch");
        assert!(error
            .to_string()
            .contains("existing process identifier 4242"));
        assert_eq!(validate_launched_pid(4243, true, &existing).unwrap(), 4243);
    }

    #[test]
    fn process_only_response_falls_back_to_requested_identity() {
        assert_eq!(
            response_identity(None, Some("com.apple.Safari"), None),
            ("Safari".to_owned(), "com.apple.Safari".to_owned())
        );
        assert_eq!(
            response_identity(
                None,
                Some("com.example.Editor"),
                Some("/Applications/Example Editor.app"),
            ),
            ("Example Editor".to_owned(), "com.example.Editor".to_owned())
        );
    }

    #[tokio::test]
    async fn launch_app_cannot_reach_private_permission_host_entrypoint() {
        let result = LaunchAppTool
            .invoke(json!({
                "bundle_id": "com.example.not-installed",
                "additional_arguments": [
                    "__permissions-host-request",
                    "--result-file",
                    "/tmp/cua-driver-permissions-forged.json"
                ]
            }))
            .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["error"],
            "PROTECTED_HOST_ENTRYPOINT"
        );

        let result = LaunchAppTool
            .invoke(json!({ "bundle_id": "com.trycua.driver" }))
            .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["error"],
            "PROTECTED_HOST_ENTRYPOINT"
        );
    }
}
