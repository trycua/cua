import AppKit
import ArgumentParser
import Foundation

enum AttachDisplayMode: String, CaseIterable, ExpressibleByArgument, Sendable {
  case native
  case vnc
}

enum AttachError: LocalizedError {
  case vmNotRunning(String)
  case vncUnavailable(String)
  case failedToOpenVNC

  var errorDescription: String? {
    switch self {
    case .vmNotRunning(let name):
      return "VM '\(name)' is not running."
    case .vncUnavailable(let name):
      return "VM '\(name)' does not have an active VNC session."
    case .failedToOpenVNC:
      return "Failed to open the VNC session in Screen Sharing."
    }
  }
}

struct Attach: AsyncParsableCommand {
  static let configuration = CommandConfiguration(
    commandName: "attach",
    abstract: "Open a viewer for a running virtual machine",
    discussion: """
      Native display is preferred when the owning `lume run` process supports
      live attachment. VNC is used as the automatic fallback.
      """
  )

  @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
  var name: String

  @Option(help: "Viewer to open: 'native' or 'vnc' (default: native with VNC fallback)")
  var display: AttachDisplayMode?

  @Option(name: .customLong("storage"), help: "VM storage location to use")
  var storage: String?

  @MainActor
  func run() async throws {
    TelemetryClient.shared.record(event: "lume_attach")

    let controller = LumeController()
    let details = try controller.getDetails(name: name, storage: storage)
    guard details.status == "running" else {
      throw AttachError.vmNotRunning(name)
    }
    let vm = try controller.get(name: name, storage: storage)
    let vmDirectory = vm.vmDirContext.dir

    switch display {
    case .native:
      try NativeDisplayAttachService.requestNativeDisplay(vmDirectory: vmDirectory)
      print("Requested native display for '\(name)'.")
    case .vnc:
      try openVNC(details: details)
    case nil:
      if NativeDisplayAttachService.isAvailable(vmDirectory: vmDirectory) {
        do {
          try NativeDisplayAttachService.requestNativeDisplay(vmDirectory: vmDirectory)
          print("Requested native display for '\(name)'.")
          return
        } catch {
          Logger.debug(
            "Native display attachment failed; falling back to VNC",
            metadata: [
              "name": name,
              "error": error.localizedDescription,
            ])
        }
      }
      try openVNC(details: details)
    }
  }

  private func openVNC(details: VMDetails) throws {
    guard let value = details.vncUrl,
      let url = URL(string: value),
      !value.isEmpty
    else {
      throw AttachError.vncUnavailable(name)
    }
    guard NSWorkspace.shared.open(url) else {
      throw AttachError.failedToOpenVNC
    }
    print("Opened VNC display for '\(name)'.")
  }
}
