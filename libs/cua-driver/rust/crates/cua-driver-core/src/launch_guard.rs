//! Admission checks shared by every platform's `launch_app`.

/// Refusal text for a `launch_app` call that tries to enable Chromium
/// DevTools. `browser_prepare` owns that transition.
pub const REMOTE_DEBUGGING_LAUNCH_REFUSAL: &str = "Chromium remote-debugging flags moved to browser_prepare so DevTools is never enabled on an unproven user profile";

/// Whether a launch argument, executable path, or command line carries a
/// Chromium remote-debugging switch in any spelling or case.
pub fn contains_remote_debugging_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("--remote-debugging-port") || lower.contains("--remote-debugging-pipe")
}

#[cfg(test)]
mod tests {
    use super::contains_remote_debugging_flag;

    #[test]
    fn rejects_all_chromium_remote_debugging_spellings() {
        for flagged in [
            "--remote-debugging-port=0",
            "--REMOTE-DEBUGGING-PIPE",
            "/usr/bin/chrome --remote-debugging-port 9222",
            r#"C:\Program Files\Chrome\chrome.exe --remote-debugging-port 9222"#,
        ] {
            assert!(contains_remote_debugging_flag(flagged), "{flagged}");
        }
        for clean in [
            "--user-data-dir=/tmp/profile",
            r#"--user-data-dir=C:\Temp\profile"#,
        ] {
            assert!(!contains_remote_debugging_flag(clean), "{clean}");
        }
    }
}
