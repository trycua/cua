import Foundation

/// Describes a macOS app the driver can interact with. Covers both
/// currently-running apps and installed-but-not-running apps the agent
/// might want to open (`launch_app` is idempotent — it opens
/// not-running apps and returns the pid for already-running ones).
///
/// Window-level state (on-screen, on-current-Space, minimized,
/// hidden) lives on window records from `list_windows` — an
/// app-level "visible" flag silently conflates "the app has a
/// window somewhere" with "the user can see it right now," so we
/// keep app identity and window visibility on separate surfaces.
///
/// Semantics of the state flags:
///
/// - `running == false` → the app is installed on disk but no
///   process is live; `pid` is `0`, `active` is `false`. Call
///   `launch_app({bundle_id})` to start it.
/// - `running == true, active == false` → the app is running but
///   not the system-frontmost app.
/// - `active == true` → the app is the system-frontmost app
///   (`NSWorkspace.shared.frontmostApplication`). Implies
///   `running == true`.
public struct AppInfo: Sendable, Codable, Hashable {
    public let pid: Int32
    public let bundleId: String?
    public let name: String
    public let running: Bool
    public let active: Bool

    public init(
        pid: Int32,
        bundleId: String?,
        name: String,
        running: Bool,
        active: Bool
    ) {
        self.pid = pid
        self.bundleId = bundleId
        self.name = name
        self.running = running
        self.active = active
    }

    private enum CodingKeys: String, CodingKey {
        case pid
        case bundleId = "bundle_id"
        case name
        case running
        case active
    }
}
