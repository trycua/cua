import ArgumentParser
import Foundation

/// Controls which local viewer Lume presents. The VNC server remains available in every mode.
enum DisplayMode: String, CaseIterable, Codable, ExpressibleByArgument, Sendable {
  case vnc
  case native
  case none

  static func resolve(requested: DisplayMode, noDisplay: Bool) -> DisplayMode {
    noDisplay ? .none : requested
  }
}
