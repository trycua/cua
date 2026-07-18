//! Small platform helpers shared by daemon startup paths.

/// Return whether the current executable lives in the installed macOS app
/// bundle. A bundled daemon already has its stable TCC responsibility identity
/// and must not disclaim it during startup.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_cuadriver_app() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .and_then(|path| path.to_str().map(str::to_owned))
        .is_some_and(|path| path.contains("/CuaDriver.app/Contents/MacOS/"))
}

/// Returns `true` when the env var is one of `1|true|yes|on`
/// (case-insensitive). Anything else, including unset, is falsy.
#[cfg(target_os = "windows")]
pub fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn env_truthiness_is_strict() {
        let name = "CUA_DRIVER_RS_TEST_TRUTHY";
        for value in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            std::env::set_var(name, value);
            assert!(is_env_truthy(name), "expected truthy for {value:?}");
        }
        for value in ["0", "false", "no", "off", ""] {
            std::env::set_var(name, value);
            assert!(!is_env_truthy(name), "expected falsy for {value:?}");
        }
        std::env::remove_var(name);
    }
}
