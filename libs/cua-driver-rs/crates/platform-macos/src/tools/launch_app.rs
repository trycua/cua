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
             `get_window_state(pid, window_id)`."
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

        // bundle_id wins when both are supplied.
        let launch_result = tokio::task::spawn_blocking(move || {
            let pid = if let Some(ref bid) = bundle_id {
                if urls.is_empty() && additional_arguments.is_empty() && env.is_empty() && !creates_new_instance {
                    crate::apps::launch_app(bid)?
                } else {
                    launch_with_urls_by_bundle(bid, &urls, &additional_arguments, &env, creates_new_instance)?
                }
            } else {
                let n = name.as_deref().unwrap();
                if urls.is_empty() && additional_arguments.is_empty() && env.is_empty() && !creates_new_instance {
                    crate::apps::launch_app_by_name(n)?
                } else {
                    launch_with_urls_by_name(n, &urls, &additional_arguments, &env, creates_new_instance)?
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

                ToolResult::text(summary).with_structured(serde_json::json!({
                    "pid": pid,
                    "bundle_id": bid,
                    "name": app_name,
                    "windows": windows_json
                }))
            }
            Ok(Err(e)) => ToolResult::error(format!("Launch failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking helpers ──────────────────────────────────────────────────────────

/// Launch a bundle with optional URLs, extra args, env vars, and new-instance flag.
fn launch_with_urls_by_bundle(
    bundle_id: &str,
    urls: &[String],
    additional_args: &[String],
    env: &std::collections::HashMap<String, String>,
    new_instance: bool,
) -> anyhow::Result<i32> {
    let mut cmd = std::process::Command::new("open");
    if new_instance { cmd.arg("-n"); }
    cmd.args(["-g", "-b", bundle_id]);
    for url in urls { cmd.arg(url); }
    if !additional_args.is_empty() {
        cmd.arg("--args");
        for arg in additional_args { cmd.arg(arg); }
    }
    for (k, v) in env { cmd.env(k, v); }
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch {bundle_id}");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let apps = crate::apps::list_running_apps();
    apps.into_iter()
        .find(|a| a.bundle_id.as_deref() == Some(bundle_id))
        .map(|a| a.pid)
        .ok_or_else(|| anyhow::anyhow!("Launched {bundle_id} but could not find its pid"))
}

/// Launch by name with optional URLs, extra args, env vars, and new-instance flag.
fn launch_with_urls_by_name(
    name: &str,
    urls: &[String],
    additional_args: &[String],
    env: &std::collections::HashMap<String, String>,
    new_instance: bool,
) -> anyhow::Result<i32> {
    let mut cmd = std::process::Command::new("open");
    if new_instance { cmd.arg("-n"); }
    cmd.args(["-g", "-a", name]);
    for url in urls { cmd.arg(url); }
    if !additional_args.is_empty() {
        cmd.arg("--args");
        for arg in additional_args { cmd.arg(arg); }
    }
    for (k, v) in env { cmd.env(k, v); }
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch '{name}'");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let apps = crate::apps::list_running_apps();
    apps.into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
        .map(|a| a.pid)
        .ok_or_else(|| anyhow::anyhow!("Launched '{name}' but could not find its pid"))
}

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
