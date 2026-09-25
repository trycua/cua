import Darwin
import Foundation

struct VMProcessOwner: Codable, Equatable {
  let processIdentifier: Int32
  let startTimeSeconds: UInt64
  let startTimeMicroseconds: UInt64

  nonisolated static func current(processIdentifier: Int32 = getpid()) -> Self? {
    guard processIdentifier > 0 else { return nil }

    var processInfo = proc_bsdinfo()
    let expectedSize = Int32(MemoryLayout<proc_bsdinfo>.size)
    let actualSize = withUnsafeMutablePointer(to: &processInfo) { pointer in
      proc_pidinfo(processIdentifier, PROC_PIDTBSDINFO, 0, pointer, expectedSize)
    }
    guard actualSize == expectedSize else { return nil }

    return Self(
      processIdentifier: processIdentifier,
      startTimeSeconds: UInt64(processInfo.pbi_start_tvsec),
      startTimeMicroseconds: UInt64(processInfo.pbi_start_tvusec)
    )
  }
}

enum VMProcessOwnerRegistry {
  private nonisolated static let ownerFileName = ".vm-process-owner.json"

  nonisolated static func register(vmDirectory: VMDirectory) throws -> VMProcessOwner {
    guard let owner = VMProcessOwner.current() else {
      throw VMError.internalError("Failed to identify the VM owner process")
    }

    let fileURL = ownerURL(for: vmDirectory)
    try JSONEncoder().encode(owner).write(to: fileURL, options: .atomic)
    chmod(fileURL.path, S_IRUSR | S_IWUSR)
    return owner
  }

  nonisolated static func unregister(_ owner: VMProcessOwner, vmDirectory: VMDirectory) {
    let fileURL = ownerURL(for: vmDirectory)
    guard read(from: fileURL) == owner else { return }
    try? FileManager.default.removeItem(at: fileURL)
  }

  nonisolated static func validatedOwner(for vmDirectory: VMDirectory) -> VMProcessOwner? {
    guard let owner = read(from: ownerURL(for: vmDirectory)),
      VMProcessOwner.current(processIdentifier: owner.processIdentifier) == owner
    else {
      return nil
    }
    return owner
  }

  nonisolated static func validatedLockOwner(
    for vmDirectory: VMDirectory, using runLockProbe: RunLockProbe
  ) -> VMProcessOwner? {
    guard let owner = validatedOwner(for: vmDirectory),
      runLockProbe.isLiveLockHolder(
        pid: owner.processIdentifier, ofFileAt: vmDirectory.configPath.path) == true
    else {
      return nil
    }
    return owner
  }

  nonisolated static func ownerRecordExists(for vmDirectory: VMDirectory) -> Bool {
    FileManager.default.fileExists(atPath: ownerURL(for: vmDirectory).path)
  }

  private nonisolated static func read(from fileURL: URL) -> VMProcessOwner? {
    guard let data = try? Data(contentsOf: fileURL) else { return nil }
    return try? JSONDecoder().decode(VMProcessOwner.self, from: data)
  }

  private nonisolated static func ownerURL(for vmDirectory: VMDirectory) -> URL {
    vmDirectory.dir.file(ownerFileName).url
  }
}
