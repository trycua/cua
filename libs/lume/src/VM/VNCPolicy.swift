import ArgumentParser
import Foundation

/// Whether a run exposes a VNC server at all.
///
/// This is orthogonal to `DisplayMode`: the display mode selects which local
/// viewer Lume opens, while the policy decides whether the remote VNC listener
/// exists. `enabled` is the default and preserves historic behavior for every
/// display mode. `disabled` is an explicit opt-in that starts no listener,
/// publishes no credentials, and reports a nil `vncUrl`.
enum VNCPolicy: String, CaseIterable, Codable, ExpressibleByArgument, Sendable {
  case enabled
  case disabled

  var isEnabled: Bool { self == .enabled }

  /// The option that conflicts with `disabled`, or nil when the combination is
  /// valid. Shared by the CLI, the controller, and the HTTP/MCP surfaces so all
  /// of them reject the same combinations.
  static func conflictingOption(
    policy: VNCPolicy,
    displayMode: DisplayMode,
    vncPort: Int,
    vncPassword: String?
  ) -> String? {
    guard policy == .disabled else { return nil }
    if displayMode == .vnc { return "--display vnc" }
    if vncPort != 0 { return "--vnc-port" }
    if vncPassword != nil { return "--vnc-password" }
    return nil
  }
}
