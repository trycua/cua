import Darwin
import Foundation

/// Errors thrown by ``DiskImageSession`` when a low-level disk command fails.
enum DiskImageError: Error, LocalizedError {
  case commandFailed(command: String, output: String)
  case unexpectedOutput(String)

  var errorDescription: String? {
    switch self {
    case .commandFailed(let command, let output):
      return "\(command) failed: \(output)"
    case .unexpectedOutput(let details):
      return details
    }
  }
}

/// Wraps the `hdiutil`/`diskutil` machinery used to work on a stopped VM's raw
/// `disk.img` offline: attach it as a block device, inspect its partitions,
/// resolve the main APFS container, and detach it again.
///
/// The attach/detach/`run`/`runPlist`/`parseWholeDisk`/`apfsContainer` helpers
/// were originally private to `MacOSOfflineSetupPatcher`; they are extracted
/// here so both the unattended patcher and the disk-resize engine share one
/// audited implementation.
final class DiskImageSession {
  let diskPath: String

  /// The attached whole-disk node (e.g. `disk4`) once ``attach()`` succeeds.
  private(set) var attachedWholeDisk: String?

  init(diskPath: String) {
    self.diskPath = diskPath
  }

  // MARK: - Attach / detach

  /// Returns true if this image is already attached anywhere on the host.
  func isAttached() throws -> Bool {
    let plist = try runPlist("/usr/bin/hdiutil", ["info", "-plist"])
    guard let images = plist["images"] as? [[String: Any]] else { return false }
    let target = URL(fileURLWithPath: diskPath).resolvingSymlinksInPath().path
    for image in images {
      guard let imagePath = image["image-path"] as? String else { continue }
      if URL(fileURLWithPath: imagePath).resolvingSymlinksInPath().path == target {
        return true
      }
    }
    return false
  }

  /// Attaches the image without mounting any volumes and records the
  /// whole-disk node. Returns the node name (e.g. `disk4`).
  @discardableResult
  func attach(readWrite: Bool = true) throws -> String {
    var args = ["attach"]
    args.append(readWrite ? "-readwrite" : "-readonly")
    args.append(contentsOf: ["-nomount", diskPath])
    let output = try run("/usr/bin/hdiutil", args)
    guard let wholeDisk = Self.parseWholeDisk(from: output) else {
      detachMatchingImage()
      throw DiskImageError.unexpectedOutput(
        "Could not determine attached disk from hdiutil output")
    }
    attachedWholeDisk = wholeDisk
    return wholeDisk
  }

  /// Detaches the attached whole disk. Retries a few times because diskutil
  /// may briefly hold the device open (e.g. after auto-mounting a volume),
  /// then falls back to `-force`.
  func detach() {
    guard let wholeDisk = attachedWholeDisk else { return }
    let device = "/dev/\(wholeDisk)"
    for _ in 0..<3 {
      if (try? run("/usr/bin/hdiutil", ["detach", device])) != nil {
        attachedWholeDisk = nil
        return
      }
      Thread.sleep(forTimeInterval: 0.5)
    }
    _ = try? run("/usr/bin/hdiutil", ["detach", "-force", device])
    attachedWholeDisk = nil
  }

  /// Cleans up an attachment when `hdiutil attach` succeeded but its textual
  /// output could not be parsed. In that case `attachedWholeDisk` was never set.
  private func detachMatchingImage() {
    guard let plist = try? runPlist("/usr/bin/hdiutil", ["info", "-plist"]),
      let images = plist["images"] as? [[String: Any]]
    else { return }

    let target = URL(fileURLWithPath: diskPath).resolvingSymlinksInPath().path
    for image in images {
      guard let imagePath = image["image-path"] as? String,
        URL(fileURLWithPath: imagePath).resolvingSymlinksInPath().path == target,
        let entities = image["system-entities"] as? [[String: Any]]
      else { continue }

      let devices = entities.compactMap { $0["dev-entry"] as? String }
      if let wholeDisk = devices.first(where: { !$0.dropFirst("/dev/".count).contains("s") }) {
        _ = try? run("/usr/bin/hdiutil", ["detach", "-force", wholeDisk])
      }
    }
  }

  /// Best-effort `diskutil unmountDisk` in case diskutil auto-mounted volumes
  /// while we were attached. Safe to call even if nothing is mounted.
  func unmountDiskIfMounted() {
    guard let wholeDisk = attachedWholeDisk else { return }
    _ = try? run("/usr/sbin/diskutil", ["unmountDisk", "force", "/dev/\(wholeDisk)"])
  }

  // MARK: - Partition inspection

  /// Parses the whole-disk node from `hdiutil attach` output (the line whose
  /// second field is `GUID_partition_scheme`).
  static func parseWholeDisk(from hdiutilOutput: String) -> String? {
    for line in hdiutilOutput.split(separator: "\n") {
      let fields = line.split(whereSeparator: { $0 == " " || $0 == "\t" })
      guard fields.count >= 2 else { continue }
      if fields[1] == "GUID_partition_scheme" {
        return fields[0].replacingOccurrences(of: "/dev/", with: "")
      }
    }
    return nil
  }

  /// Resolves the APFS container reference backing the main partition
  /// (`<wholeDisk>s2`) of an attached macOS disk.
  func apfsContainer(forMainPartitionOf wholeDisk: String) throws -> String {
    let plist = try runPlist("/usr/sbin/diskutil", ["info", "-plist", "\(wholeDisk)s2"])
    guard let container = plist["APFSContainerReference"] as? String else {
      throw DiskImageError.unexpectedOutput(
        "Could not find APFS container for \(wholeDisk)s2")
    }
    return container
  }

  // MARK: - Process helpers

  func runPlist(_ executable: String, _ arguments: [String]) throws -> [String: Any] {
    let output = try run(executable, arguments)
    guard let data = output.data(using: .utf8),
      let plist = try PropertyListSerialization.propertyList(
        from: data, options: [], format: nil) as? [String: Any]
    else {
      throw DiskImageError.unexpectedOutput(
        "Command did not return a plist: \(executable) \(arguments.joined(separator: " "))")
    }
    return plist
  }

  @discardableResult
  func run(_ executable: String, _ arguments: [String]) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments

    let output = Pipe()
    process.standardOutput = output
    process.standardError = output

    try process.run()
    let combined = try output.fileHandleForReading.readToEnd() ?? Data()
    process.waitUntilExit()
    let outputString = String(data: combined, encoding: .utf8) ?? ""

    guard process.terminationStatus == 0 else {
      throw DiskImageError.commandFailed(
        command: "\(executable) \(arguments.joined(separator: " "))",
        output: outputString)
    }

    return outputString
  }
}
