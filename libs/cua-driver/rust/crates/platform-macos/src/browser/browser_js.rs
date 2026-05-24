//! BrowserJS: run JavaScript in Chrome/Brave/Edge/Safari via AppleScript/osascript.

use std::time::Duration;
use anyhow::Context;

pub struct BrowserJs;

fn app_name_for_bundle(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.google.Chrome"      => Some("Google Chrome"),
        "com.brave.Browser"      => Some("Brave Browser"),
        "com.microsoft.edgemac"  => Some("Microsoft Edge"),
        "com.apple.Safari"       => Some("Safari"),
        _                        => None,
    }
}

impl BrowserJs {
    /// Returns true if this bundle ID is a supported browser.
    pub fn supports(bundle_id: &str) -> bool {
        app_name_for_bundle(bundle_id).is_some()
    }

    /// Execute JavaScript in the browser window identified by window_id.
    pub async fn execute(javascript: &str, bundle_id: &str, window_id: u32) -> anyhow::Result<String> {
        let app_name = app_name_for_bundle(bundle_id)
            .ok_or_else(|| anyhow::anyhow!("Unsupported browser bundle: {bundle_id}"))?;

        // Get window title from CGWindowID.
        let win_title = {
            let wid = window_id;
            tokio::task::spawn_blocking(move || {
                crate::windows::all_windows()
                    .into_iter()
                    .find(|w| w.window_id == wid)
                    .map(|w| w.title)
                    .unwrap_or_default()
            }).await?
        };

        let escaped_title = escape_for_applescript_string(&win_title);
        let escaped_js = escape_js_for_applescript(javascript);

        let script = if bundle_id == "com.apple.Safari" {
            format!(
                r#"tell application "Safari"
  set matchedDoc to missing value
  repeat with d in documents
    if name of d contains "{escaped_title}" then
      set matchedDoc to d
      exit repeat
    end if
  end repeat
  if matchedDoc is missing value then
    set matchedDoc to document 1
  end if
  do JavaScript {escaped_js} in matchedDoc
end tell"#
            )
        } else {
            format!(
                r#"tell application "{app_name}"
  set matchedWindow to missing value
  repeat with w in windows
    if name of w contains "{escaped_title}" then
      set matchedWindow to w
      exit repeat
    end if
  end repeat
  if matchedWindow is missing value then
    set matchedWindow to front window
  end if
  tell active tab of matchedWindow
    execute javascript {escaped_js}
  end tell
end tell"#
            )
        };

        run_osascript(&script).await
    }

    /// Patch the browser Preferences JSON to enable Allow JavaScript from Apple Events,
    /// then relaunch the browser.
    pub async fn enable_javascript_apple_events(bundle_id: &str) -> anyhow::Result<()> {
        let app_name = app_name_for_bundle(bundle_id)
            .ok_or_else(|| anyhow::anyhow!("Unsupported browser bundle: {bundle_id}"))?;

        // Quit the browser.
        let quit_script = format!("tell application \"{app_name}\" to quit");
        let _ = tokio::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&quit_script)
            .output()
            .await;

        tokio::time::sleep(Duration::from_secs(1)).await;

        // Find profile directory.
        let home = std::env::var("HOME").unwrap_or_default();
        let profiles_dir = match bundle_id {
            "com.google.Chrome"     => format!("{home}/Library/Application Support/Google/Chrome"),
            "com.brave.Browser"     => format!("{home}/Library/Application Support/BraveSoftware/Brave-Browser"),
            "com.microsoft.edgemac" => format!("{home}/Library/Application Support/Microsoft Edge"),
            _ => anyhow::bail!("No profiles directory for {bundle_id}"),
        };

        // Find all Preferences files.
        let prefs_files = find_preferences_files(&profiles_dir);

        for path in prefs_files {
            if let Err(e) = patch_preferences_file(&path) {
                tracing::warn!("Failed to patch {path}: {e}");
            }
        }

        // Relaunch.
        tokio::process::Command::new("open")
            .arg("-a")
            .arg(app_name)
            .spawn()?;

        Ok(())
    }
}

fn find_preferences_files(profiles_dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else { return vec![] };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let prefs = path.join("Preferences");
        if prefs.exists() {
            if let Some(s) = prefs.to_str() {
                result.push(s.to_owned());
            }
        }
    }
    result
}

fn patch_preferences_file(path: &str) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Reading {path}"))?;
    let mut json: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Parsing {path}"))?;

    // Set browser.allow_javascript_apple_events = true.
    if let Some(obj) = json.as_object_mut() {
        let browser = obj.entry("browser")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(b) = browser.as_object_mut() {
            b.insert("allow_javascript_apple_events".to_owned(), serde_json::Value::Bool(true));
        }
        // Also set account_values.browser.allow_javascript_apple_events.
        let account_values = obj.entry("account_values")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(av) = account_values.as_object_mut() {
            let av_browser = av.entry("browser")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Some(avb) = av_browser.as_object_mut() {
                avb.insert("allow_javascript_apple_events".to_owned(), serde_json::Value::Bool(true));
            }
        }
    }

    let new_content = serde_json::to_string(&json)?;
    std::fs::write(path, new_content)
        .with_context(|| format!("Writing {path}"))?;
    Ok(())
}

/// Escape a plain string value for embedding inside an AppleScript double-quoted string.
fn escape_for_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escape JS for embedding in an AppleScript string literal.
/// Multi-line JS is split by newline and concatenated with `& (ASCII character 10) &`.
fn escape_js_for_applescript(js: &str) -> String {
    let lines: Vec<String> = js.lines()
        .map(|l| {
            let escaped = l.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect();
    if lines.is_empty() {
        return "\"\"".to_owned();
    }
    if lines.len() == 1 {
        return lines[0].clone();
    }
    lines.join(" & (ASCII character 10) & ")
}

/// Write script to a temp file and run osascript with a 15s timeout.
pub async fn run_osascript(script: &str) -> anyhow::Result<String> {
    let uuid = format!("{:x}", rand_u64());
    let path = format!("/tmp/{uuid}.applescript");

    tokio::fs::write(&path, script).await
        .with_context(|| format!("Writing temp applescript to {path}"))?;

    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new("/usr/bin/osascript")
            .arg(&path)
            .output()
    ).await
    .context("osascript timed out after 15s")?
    .context("osascript failed to spawn")?;

    // Clean up.
    let _ = tokio::fs::remove_file(&path).await;

    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("turned off") || stderr.contains("AppleScript is turned off") {
        anyhow::bail!(
            "JavaScript from Apple Events is disabled. Use action=enable_javascript_apple_events \
             to enable it (requires browser restart)."
        );
    }

    if !out.status.success() && out.stdout.is_empty() {
        let msg = stderr.trim().to_owned();
        anyhow::bail!("osascript error: {msg}");
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_owned())
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    // Mix in thread id for uniqueness.
    let tid = std::thread::current().id();
    t.subsec_nanos() as u64 ^ t.as_secs().wrapping_mul(6364136223846793005) ^ format!("{tid:?}").len() as u64
}
