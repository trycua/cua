use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

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
             even if one is already running (passes -n to open).\n\n\
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
                    "description": "When true, force a new app instance even if already running (open -n)."
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
        let bundle_id = args.get("bundle_id").and_then(|v| v.as_str()).map(str::to_owned);
        let name      = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let urls: Vec<String> = args.get("urls")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let electron_debugging_port = args.get("electron_debugging_port").and_then(|v| v.as_u64()).map(|v| v as u16);
        let webkit_inspector_port = args.get("webkit_inspector_port").and_then(|v| v.as_u64()).map(|v| v as u16);
        let creates_new_instance = args.get("creates_new_application_instance").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut additional_arguments: Vec<String> = args.get("additional_arguments")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch."
            );
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

        // Move the launch closure inputs into spawn_blocking. The
        // blocking task returns (pid, app_info, windows). Suppression
        // upgrade happens AFTER the blocking call returns (back on the
        // async runtime), then we sleep 500ms holding the targeted
        // lease before releasing it.
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
                    // 500ms covers `applicationDidFinishLaunching` plus
                    // any reflex `NSApp.activate(...)`. Matches Swift
                    // LaunchAppTool.swift exactly.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    drop(targeted_lease);

                    // Belt-and-braces: if the target is STILL frontmost
                    // after the suppression window, the intra-`open()`
                    // synchronous activation slipped through. Demote
                    // explicitly by re-activating the prior frontmost.
                    let frontmost_now = crate::apps::frontmost_pid();
                    if frontmost_now == Some(*pid) {
                        let activated = crate::apps::activate_pid(prior);
                        // Re-check; the structured response depends on
                        // whether the demote actually took.
                        let still_frontmost =
                            crate::apps::frontmost_pid() == Some(*pid);
                        if still_frontmost {
                            tracing::warn!(
                                target: "platform_macos::tools::launch_app",
                                launched_pid = *pid,
                                prior_pid = prior,
                                activate_pid_returned = activated,
                                "belt-and-braces demotion failed: launched app \
                                 is still frontmost after re-activating prior"
                            );
                            self_activation_suppressed = Some(false);
                        } else {
                            self_activation_suppressed = Some(true);
                        }
                    } else {
                        self_activation_suppressed = Some(true);
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
