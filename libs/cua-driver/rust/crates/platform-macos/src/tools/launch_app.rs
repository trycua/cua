use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::path::PathBuf;

pub struct LaunchAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "launch_app".into(),
        description:
            "Launch a macOS app in the background — the target does NOT come to the foreground.\n\n\
             Provide either `bundle_id` (preferred — unambiguous, e.g. `com.apple.calculator`) \
             or `name` (e.g. \"Calculator\"). If both are given, bundle_id wins.\n\n\
             Optional `urls` are handed to the app as open targets — for Finder, pass a folder \
             path to open a backgrounded Finder window there.\n\n\
             Optional `electron_debugging_port`: opens a Chrome DevTools Protocol (CDP) server \
             on the specified port (appends --remote-debugging-port=N to the app's argv). \
             Use this to automate Electron/VS Code/Cursor via CDP.\n\n\
             Optional `webkit_inspector_port`: opens a WebKit inspector server on the specified \
             port (sets WEBKIT_INSPECTOR_SERVER=127.0.0.1:N + TAURI_WEBVIEW_AUTOMATION=1). \
             Use this for Tauri/WebKit-based apps.\n\n\
             Optional `creates_new_application_instance`: when true, forces a new app instance \
             even if one is already running (passes -n to open). Reach for this when another \
             agent or session may drive the SAME app concurrently — it returns a fresh pid + \
             window so each session acts on its own isolated window instead of clobbering one \
             shared instance. Without it, single-instance apps (Calculator, many utilities) hand \
             every caller the same window, so two sessions fight over it.\n\n\
             Optional `additional_arguments`: extra argv strings appended after --args.\n\n\
             Returns the launched app's pid, bundle_id, name, and a `windows` array \
             (same shape as `list_windows`) so callers can skip an extra round-trip before \
             `get_window_state(pid, window_id)`. When the focus-steal belt-and-braces \
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
                "electron_debugging_port": {
                    "type": "integer",
                    "description": "Open a Chrome DevTools Protocol server on this port (appends --remote-debugging-port=N)."
                },
                "webkit_inspector_port": {
                    "type": "integer",
                    "description": "Open a WebKit inspector server on this port (sets WEBKIT_INSPECTOR_SERVER env var)."
                },
                "creates_new_application_instance": {
                    "type": "boolean",
                    "description": "When true, force a new app instance even if already running (open -n). Use for concurrent multi-agent/multi-session work so each session gets an isolated instance + window instead of sharing one — on single-instance apps (e.g. Calculator) every caller otherwise gets the same window and the sessions clobber each other."
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let bundle_id = args.opt_str("bundle_id");
        let name      = args.opt_str("name");
        let urls: Vec<String> = args.str_array("urls");
        let electron_debugging_port = args.opt_u64("electron_debugging_port").map(|v| v as u16);
        let webkit_inspector_port = args.opt_u64("webkit_inspector_port").map(|v| v as u16);
        let creates_new_instance = args.bool_or("creates_new_application_instance", false);
        let mut additional_arguments: Vec<String> = args.str_array("additional_arguments");

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch."
            );
        }
        if let Some(ref bid) = bundle_id {
            if crate::apps::resolve_bundle_id_to_locator(bid).is_none() {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for bundle_id '{bid}'."),
                    serde_json::json!({ "bundle_id": bid }),
                );
            }
        } else if let Some(ref n) = name {
            if crate::apps::locate_by_name(n).is_none() {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for name '{n}'."),
                    serde_json::json!({ "name": n }),
                );
            }
        }
        if let Some(err) = preflight_file_urls(&urls) {
            return err;
        }

        // Build additional arguments (e.g., CDP port).
        if let Some(port) = electron_debugging_port {
            additional_arguments.push(format!("--remote-debugging-port={port}"));
        }

        // Build env dict for webkit inspector.
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if let Some(port) = webkit_inspector_port {
            env.insert("WEBKIT_INSPECTOR_SERVER".to_string(), format!("127.0.0.1:{port}"));
            env.insert("TAURI_WEBVIEW_AUTOMATION".to_string(), "1".to_string());
        }

        let port_summary = {
            let mut s = String::new();
            if let Some(port) = electron_debugging_port {
                s.push_str(&format!("\nCDP renderer available on port {port}."));
            }
            if let Some(port) = webkit_inspector_port {
                s.push_str(&format!("\nWebKit inspector available on port {port}."));
            }
            s
        };

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

        let wildcard_lease = prior_frontmost.map(|prior| {
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

            // Retry loop: LaunchServices returns before WindowServer has
            // registered the new windows. Poll up to 5x100ms.
            let windows = resolve_windows_for_pid(pid);

            let app_info: Option<crate::apps::AppInfo> = {
                let apps = crate::apps::list_running_apps();
                apps.into_iter().find(|a| a.pid == pid)
            };

            Ok::<_, anyhow::Error>((pid, app_info, windows))
        }).await;

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
                    let targeted_lease =
                        crate::focus_steal::FocusStealPreventer::begin_suppression(
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
                        let still_frontmost =
                            crate::apps::frontmost_pid() == Some(*pid);
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
                        tokio::time::sleep(
                            std::time::Duration::from_millis(200)
                        ).await;
                    }
                    // Final in-call state determines the structured response.
                    let final_frontmost = crate::apps::frontmost_pid();
                    self_activation_suppressed = Some(
                        final_frontmost != Some(*pid) && demotion_succeeded
                    );

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
                            for _ in 0..32 {  // 32 × 250ms = 8s
                                tokio::time::sleep(
                                    std::time::Duration::from_millis(250)
                                ).await;
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
                let app_name = app_info.as_ref().map(|a| a.name.as_str()).unwrap_or("?");
                let bid = app_info.as_ref().and_then(|a| a.bundle_id.as_deref()).unwrap_or("?");

                let mut summary = format!("Launched {app_name} (pid {pid}) in background.{port_summary}");

                if !windows.is_empty() {
                    summary.push_str("\n\nWindows:");
                    for w in &windows {
                        let title = if w.title.is_empty() {
                            "(no title)".to_owned()
                        } else {
                            format!("\"{}\"", w.title)
                        };
                        summary.push_str(&format!("\n- {title} [window_id: {}]", w.window_id));
                    }
                    summary.push_str(&format!(
                        "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
                    ));
                }

                let windows_json: Vec<Value> = windows.iter().map(|w| serde_json::json!({
                    "window_id": w.window_id,
                    "pid": w.pid,
                    "app_name": w.app_name,
                    "title": w.title,
                    "bounds": {
                        "x": w.bounds.x, "y": w.bounds.y,
                        "width": w.bounds.width, "height": w.bounds.height
                    },
                    "is_on_screen": w.is_on_screen,
                })).collect();

                let mut structured = serde_json::json!({
                    "pid": pid,
                    "bundle_id": bid,
                    "name": app_name,
                    "windows": windows_json,
                });
                // Only emit `self_activation_suppressed` when the
                // belt-and-braces demotion check actually ran. `None`
                // means the launch didn't enter the focus-steal path
                // (no prior frontmost, or pid == prior) — surfacing
                // a stale `false` would be misleading.
                if let Some(suppressed) = self_activation_suppressed {
                    structured["self_activation_suppressed"] =
                        serde_json::Value::Bool(suppressed);
                }
                ToolResult::text(summary).with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("Launch failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking helpers ──────────────────────────────────────────────────────────

/// Poll for the pid's layer-0 windows, retrying up to 5x100ms to absorb
/// LaunchServices → WindowServer latency (mirrors the Swift reference).
fn resolve_windows_for_pid(pid: i32) -> Vec<crate::windows::WindowInfo> {
    for attempt in 0..5 {
        let found: Vec<_> = crate::windows::all_windows()
            .into_iter()
            .filter(|w| w.pid == pid && w.layer == 0)
            .filter(|w| w.bounds.width > 1.0 && w.bounds.height > 1.0)
            .collect();
        if !found.is_empty() {
            return found;
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    vec![]
}

fn structured_launch_error(
    code: &str,
    message: String,
    details: serde_json::Value,
) -> ToolResult {
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
    use super::{local_file_target, preflight_file_urls};
    use std::path::PathBuf;

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
        assert_eq!(structured["path"], "/tmp/cua-driver-definitely-missing-file-for-test.md");
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
}
