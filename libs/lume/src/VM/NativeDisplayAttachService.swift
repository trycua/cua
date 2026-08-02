import Darwin
import Foundation

enum NativeDisplayAttachError: LocalizedError {
  case unavailable
  case requestFailed

  var errorDescription: String? {
    switch self {
    case .unavailable:
      return "The running VM process does not support native display attachment."
    case .requestFailed:
      return "Failed to ask the running VM process to show its native display."
    }
  }
}

private struct NativeDisplayOwner: Codable {
  let processIdentifier: Int32
}

/// Provides a small, process-local control channel for revealing the native viewer.
///
/// The owning `lume run` process registers SIGUSR1 and writes its PID into the VM
/// directory. An `attach` process verifies that PID still owns the VM configuration
/// lock before signaling it. The signal never crosses into an unrelated Lume daemon.
@MainActor
enum NativeDisplayAttachService {
  private nonisolated static let ownerFileName = ".native-display-owner.json"
  private static var signalSource: DispatchSourceSignal?
  private static var ownerFileURL: URL?
  private static var showAction: (@MainActor @Sendable () async -> Void)?

  static func register(
    vmDirectory: VMDirectory,
    show: @escaping @MainActor @Sendable () async -> Void
  ) throws {
    unregister()

    let owner = NativeDisplayOwner(processIdentifier: getpid())
    let fileURL = ownerURL(for: vmDirectory)
    let data = try JSONEncoder().encode(owner)
    try data.write(to: fileURL, options: .atomic)
    chmod(fileURL.path, S_IRUSR | S_IWUSR)

    showAction = show
    ownerFileURL = fileURL
    signal(SIGUSR1, SIG_IGN)

    let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
    source.setEventHandler {
      Task { @MainActor in
        await showAction?()
      }
    }
    signalSource = source
    source.resume()
  }

  static func unregister() {
    signalSource?.cancel()
    signalSource = nil
    showAction = nil

    if let ownerFileURL,
      let data = try? Data(contentsOf: ownerFileURL),
      let owner = try? JSONDecoder().decode(NativeDisplayOwner.self, from: data),
      owner.processIdentifier == getpid()
    {
      try? FileManager.default.removeItem(at: ownerFileURL)
    }
    ownerFileURL = nil
  }

  nonisolated static func isAvailable(vmDirectory: VMDirectory) -> Bool {
    guard let owner = validatedOwner(for: vmDirectory) else { return false }
    return kill(owner.processIdentifier, 0) == 0
  }

  nonisolated static func requestNativeDisplay(vmDirectory: VMDirectory) throws {
    guard let owner = validatedOwner(for: vmDirectory) else {
      throw NativeDisplayAttachError.unavailable
    }
    guard kill(owner.processIdentifier, SIGUSR1) == 0 else {
      throw NativeDisplayAttachError.requestFailed
    }
  }

  private nonisolated static func validatedOwner(
    for vmDirectory: VMDirectory
  ) -> NativeDisplayOwner? {
    let fileURL = ownerURL(for: vmDirectory)
    guard let data = try? Data(contentsOf: fileURL),
      let owner = try? JSONDecoder().decode(NativeDisplayOwner.self, from: data),
      owner.processIdentifier > 0,
      owner.processIdentifier == lockOwnerPID(for: vmDirectory.configPath.url)
    else {
      return nil
    }
    return owner
  }

  private nonisolated static func ownerURL(for vmDirectory: VMDirectory) -> URL {
    vmDirectory.dir.file(ownerFileName).url
  }

  private nonisolated static func lockOwnerPID(for configURL: URL) -> Int32? {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
    process.arguments = ["-t", configURL.path]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice

    do {
      try process.run()
      process.waitUntilExit()
      guard process.terminationStatus == 0,
        let data = try output.fileHandleForReading.readToEnd(),
        let value = String(data: data, encoding: .utf8)?
          .split(whereSeparator: \.isNewline).first,
        let pid = Int32(value)
      else {
        return nil
      }
      return pid
    } catch {
      return nil
    }
  }
}
