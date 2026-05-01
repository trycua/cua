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

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch."
            );
        }

        // bundle_id wins when both are supplied.
        let launch_result = tokio::task::spawn_blocking(move || {
            let pid = if let Some(ref bid) = bundle_id {
                if urls.is_empty() {
                    crate::apps::launch_app(bid)?
                } else {
                    launch_with_urls_by_bundle(bid, &urls)?
                }
            } else {
                let n = name.as_deref().unwrap();
                if urls.is_empty() {
                    crate::apps::launch_app_by_name(n)?
                } else {
                    launch_with_urls_by_name(n, &urls)?
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

                let mut summary = format!("Launched {app_name} (pid {pid}) in background.");

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

/// Launch a bundle with URLs via `open -g -b bundle_id url1 url2 ...`.
fn launch_with_urls_by_bundle(bundle_id: &str, urls: &[String]) -> anyhow::Result<i32> {
    let mut cmd = std::process::Command::new("open");
    cmd.args(["-g", "-b", bundle_id]);
    for url in urls { cmd.arg(url); }
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch {bundle_id} with urls");
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let apps = crate::apps::list_running_apps();
    apps.into_iter()
        .find(|a| a.bundle_id.as_deref() == Some(bundle_id))
        .map(|a| a.pid)
        .ok_or_else(|| anyhow::anyhow!("Launched {bundle_id} but could not find its pid"))
}

/// Launch by name with URLs via `open -g -a AppName url1 url2 ...`.
fn launch_with_urls_by_name(name: &str, urls: &[String]) -> anyhow::Result<i32> {
    let mut cmd = std::process::Command::new("open");
    cmd.args(["-g", "-a", name]);
    for url in urls { cmd.arg(url); }
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Failed to launch '{name}' with urls");
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
