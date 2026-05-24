import Darwin
import Foundation

/// Shared "is this binary running from inside an installed CuaDriver.app
/// bundle?" heuristic used by both `ServeCommand` (for the
/// auto-relaunch-via-`open` path) and `MCPCommand` (for the daemon proxy
/// path). Resolves `Bundle.main.executablePath` (falling back to
/// `CommandLine.arguments.first`) through any symlinks via `realpath` and
/// checks whether the resolved path lives inside some
/// `CuaDriver.app/Contents/MacOS/` directory.
///
/// That's the "installed via install-local.sh / install.sh" shape —
/// `~/.local/bin/cua-driver` is a symlink into `/Applications/CuaDriver.app`,
/// and `realpath` walks into the bundle. Returns `false` for `swift run` /
/// raw `.build/<config>/cua-driver` dev invocations, which have no installed
/// bundle to relaunch into.
///
/// Subcommands may wrap this with additional gating (env vars, flags,
/// parent-pid checks, etc.) when their relaunch heuristics diverge.
func isExecutableInsideCuaDriverApp() -> Bool {
    // Prefer Foundation's executablePath (stable, absolute).
    // Fall back to argv[0] when unset, which realpath() still
    // resolves via $PATH lookup at the shell level — good enough
    // for the cases we care about.
    let candidate = Bundle.main.executablePath
        ?? CommandLine.arguments.first
        ?? ""
    guard !candidate.isEmpty else { return false }

    var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
    guard realpath(candidate, &buffer) != nil else { return false }
    let resolved = String(cString: buffer)
    return resolved.contains("/CuaDriver.app/Contents/MacOS/")
}
