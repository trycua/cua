//! WKWebView detection: checks whether a process embeds WebKit.

/// Returns true if the process at `pid` is a WKWebView-based app (but not Electron).
pub fn is_wk_web_view_app(pid: i32) -> bool {
    let apps = crate::apps::list_running_apps();
    let app = apps.iter().find(|a| a.pid == pid);

    let bundle_path: Option<String> = app.and_then(|a| {
        find_bundle_path_for_app_name(&a.name)
    });

    if let Some(ref bp) = bundle_path {
        // If it has Electron Framework it's Electron, not WKWebView.
        let electron_fw = format!("{bp}/Contents/Frameworks/Electron Framework.framework");
        if std::path::Path::new(&electron_fw).exists() {
            return false;
        }

        // Check for Tauri in bundle path (lowercased).
        if bp.to_lowercase().contains("tauri") {
            return true;
        }

        // Check for WebKit.framework in bundle.
        let webkit_fw = format!("{bp}/Contents/Frameworks/WebKit.framework");
        if std::path::Path::new(&webkit_fw).exists() {
            return true;
        }
    }

    // Fallback: check otool -L on the executable for WebKit linkage.
    if let Ok(exe) = get_exe_path(pid) {
        if let Ok(out) = std::process::Command::new("otool")
            .args(["-L", &exe])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.contains("WebKit.framework") || text.contains("libwebkit") {
                return true;
            }
        }
    }

    false
}

fn get_exe_path(pid: i32) -> anyhow::Result<String> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if s.is_empty() {
        anyhow::bail!("No exe for pid {pid}");
    }
    Ok(s)
}

fn find_bundle_path_for_app_name(name: &str) -> Option<String> {
    let dirs = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    let user_apps = format!("{home}/Applications");

    let mut all_dirs: Vec<&str> = dirs.to_vec();
    let user_str: &str = &user_apps;
    all_dirs.push(user_str);

    for dir in all_dirs {
        let candidate = format!("{dir}/{name}.app");
        if std::path::Path::new(&candidate).exists() {
            return Some(candidate);
        }
    }
    None
}
