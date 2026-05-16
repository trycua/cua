//! macOS bundle-context detection for the TCC auto-relaunch path.
//!
//! Mirrors `libs/cua-driver/Sources/CuaDriverCLI/BundleHelpers.swift`'s
//! `isExecutableInsideCuaDriverApp()` — the heuristic that decides
//! whether `cua-driver-rs mcp` was spawned from an IDE terminal as a
//! bare CLI symlinked into our .app bundle. When true and the parent
//! isn't launchd, we re-launch the daemon via `open -n -g -a
//! CuaDriverRs --args serve` so it picks up the bundle's TCC grants,
//! then proxy stdio MCP traffic through the daemon's Unix socket.
//!
//! Non-macOS targets compile to no-ops so the cross-platform call
//! sites stay tidy.

/// Returns `true` when the currently-running binary resolves into an
/// installed `CuaDriverRs.app` bundle (Rust port). The check is the
/// same shape as the Swift driver's `isExecutableInsideCuaDriverApp`
/// (`/CuaDriver.app/Contents/MacOS/`) but keyed on the Rust port's
/// distinct bundle name so the two installs don't collide.
///
/// `false` for raw `cargo run` / `target/release/cua-driver` dev
/// invocations — there's no installed bundle to relaunch into, so the
/// caller should stay in-process.
///
/// Implementation:
///   1. Resolve `std::env::current_exe()` (preferred; absolute path
///      to the running image).
///   2. Walk symlinks via `std::fs::canonicalize` — the install layout
///      is `~/.local/bin/cua-driver` → `/Applications/CuaDriverRs.app/
///      Contents/MacOS/cua-driver`, so without the canonicalize step
///      we'd see the bare symlink path and miss the bundle.
///   3. Substring-match the canonical path for the bundle marker.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_cuadriverrs_app() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let canonical = match std::fs::canonicalize(&exe) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let s = match canonical.to_str() {
        Some(s) => s,
        None => return false,
    };
    s.contains("/CuaDriverRs.app/Contents/MacOS/")
}

#[cfg(not(target_os = "macos"))]
pub fn is_executable_inside_cuadriverrs_app() -> bool {
    false
}

/// Returns `true` when the parent process is *not* `launchd` (pid 1).
/// Combined with [`is_executable_inside_cuadriverrs_app`], a `true`
/// here means the binary was spawned from a shell / IDE terminal that
/// inherits the wrong TCC responsibility — i.e. the case we want to
/// auto-relaunch from.
///
/// `ppid == 1` means launchd reparented us (we're already running as
/// the LaunchServices-spawned daemon). In that case we stay
/// in-process: TCC grants are already correct, and relaunching would
/// fork-bomb the daemon back into existence on every `mcp` startup.
///
/// Mirrors Swift's `if getppid() == 1 { return false }` gate in
/// `MCPCommand.shouldUseDaemonProxy()`.
#[cfg(unix)]
pub fn parent_is_not_launchd() -> bool {
    // SAFETY: `libc::getppid` is a thread-safe POSIX getter that
    // takes no args and returns the parent pid. No invariants to
    // uphold, no UB to risk.
    let ppid = unsafe { libc::getppid() };
    ppid != 1
}

#[cfg(not(unix))]
pub fn parent_is_not_launchd() -> bool {
    // No launchd on non-Unix; the heuristic is macOS-only anyway.
    // Returning false keeps the caller in-process on unsupported
    // platforms (same effective outcome as the macOS check failing).
    false
}

/// Returns `true` when the env var is one of `1|true|yes|on`
/// (case-insensitive). Anything else, including unset, is falsy.
///
/// Mirrors Swift's `isEnvTruthy` helper on `MCPCommand`.
pub fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_run_is_not_inside_bundle() {
        // The unit-test runner image lives under `target/<config>/
        // deps/`, never inside a .app bundle. Should always return
        // false in CI / local dev, which is exactly the behavior we
        // want so `cargo run` callers stay in-process.
        assert!(!is_executable_inside_cuadriverrs_app());
    }

    #[test]
    fn unset_env_is_falsy() {
        // Use a deliberately unlikely name so we don't depend on the
        // surrounding shell environment.
        std::env::remove_var("CUA_DRIVER_RS_TEST_UNSET_NAME");
        assert!(!is_env_truthy("CUA_DRIVER_RS_TEST_UNSET_NAME"));
    }

    #[test]
    fn truthy_env_values_recognized() {
        let name = "CUA_DRIVER_RS_TEST_TRUTHY";
        for v in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            std::env::set_var(name, v);
            assert!(is_env_truthy(name), "expected truthy for {v:?}");
        }
        for v in ["0", "false", "no", "off", ""] {
            std::env::set_var(name, v);
            assert!(!is_env_truthy(name), "expected falsy for {v:?}");
        }
        std::env::remove_var(name);
    }

    #[test]
    #[cfg(unix)]
    fn parent_is_not_launchd_in_tests() {
        // The cargo test harness is reparented under whatever
        // launched it (cargo / IDE / shell), not directly under
        // launchd. The helper should report true.
        assert!(parent_is_not_launchd());
    }
}
