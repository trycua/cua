import AppKit
import Foundation
import Testing

@testable import lume

@MainActor
@Test("Native file drops accept local file and directory URLs only")
func nativeFileDropExtractsLocalURLs() throws {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: root) }

  let file = root.appendingPathComponent("File with spaces.txt")
  let directory = root.appendingPathComponent("Folder")
  try Data("drop test".utf8).write(to: file)
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)

  let pasteboard = NSPasteboard(name: .init("ai.cua.lume.tests.file-drop.\(UUID().uuidString)"))
  pasteboard.clearContents()
  #expect(
    pasteboard.writeObjects([
      file as NSURL,
      directory as NSURL,
      URL(string: "https://example.com/not-a-file")! as NSURL,
    ]))

  let urls = NativeFileDrop.fileURLs(from: pasteboard)

  #expect(urls == [file, directory])
}

@Test("SCP safely checks guest Desktop names before transfer")
func scpDesktopPreparationCommand() {
  let client = SystemSSHClient(host: "192.168.64.24")
  let command = client.desktopPreparationCommand(
    itemNames: ["Report's.txt"],
    stagingDirectory: ".lume-drop-test"
  )

  #expect(command.hasPrefix("set -e; mkdir -p \"$HOME/Desktop\""))
  #expect(command.contains(#""$HOME/Desktop/"'Report'\''s.txt'"#))
  #expect(command.contains("exit 73"))
  #expect(command.hasSuffix("mkdir \"$HOME/.lume-drop-test\""))
}

@Test("SCP keeps dropped paths as distinct arguments and targets the guest Desktop")
func scpFileDropArguments() throws {
  let client = SystemSSHClient(
    host: "192.168.64.24",
    port: 2222,
    user: "lume",
    password: "secret"
  )
  let sourcePaths = [
    "/tmp/File with spaces.txt",
    "/tmp/--looks-like-an-option",
  ]

  let arguments = client.scpArguments(
    sourcePaths: sourcePaths,
    destinationDirectory: "Desktop"
  )
  let separator = try #require(arguments.firstIndex(of: "--"))

  #expect(arguments.contains("-r"))
  #expect(arguments.contains("-P"))
  #expect(arguments.contains("2222"))
  #expect(arguments.contains("ServerAliveInterval=15"))
  #expect(arguments.contains("ServerAliveCountMax=4"))
  #expect(Array(arguments[(separator + 1)..<(arguments.count - 1)]) == sourcePaths)
  #expect(arguments.last == "lume@192.168.64.24:Desktop/")
}

@Test("SCP omits a port override for standard SSH")
func scpFileDropDefaultPortArguments() {
  let client = SystemSSHClient(host: "192.168.64.24")
  let arguments = client.scpArguments(
    sourcePaths: ["/tmp/example.txt"],
    destinationDirectory: "Desktop"
  )

  #expect(!arguments.contains("-P"))
}

@Test("SCP rejects an empty file drop before launching a process")
func scpRejectsEmptyFileDrop() async {
  let client = SystemSSHClient(host: "192.168.64.24")

  await #expect(throws: RemoteDesktopCopyError.self) {
    try await client.copyToRemoteDesktop([])
  }
}

@Test("SCP rejects duplicate destination names before connecting")
func scpRejectsDuplicateDestinationNames() async throws {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  let first = root.appendingPathComponent("first/Same.txt")
  let second = root.appendingPathComponent("second/Same.txt")
  try FileManager.default.createDirectory(
    at: first.deletingLastPathComponent(),
    withIntermediateDirectories: true
  )
  try FileManager.default.createDirectory(
    at: second.deletingLastPathComponent(),
    withIntermediateDirectories: true
  )
  try Data().write(to: first)
  try Data().write(to: second)
  defer { try? FileManager.default.removeItem(at: root) }

  let client = SystemSSHClient(host: "invalid.invalid")
  do {
    try await client.copyToRemoteDesktop([first, second])
    Issue.record("Expected duplicate destination names to be rejected")
  } catch let error as RemoteDesktopCopyError {
    #expect(error.localizedDescription == "Dropped items must have unique names")
  } catch {
    Issue.record("Unexpected error: \(error)")
  }
}

@Test("A pre-cancelled file copy cannot launch a process")
func scpCancellationIsObservable() {
  let controller = CopyProcessController()
  controller.cancel()

  #expect(throws: CancellationError.self) {
    try controller.register(Process())
  }
}

@Test("File-copy errors are ready for user-facing overlays")
func scpFileCopyErrorDescriptions() {
  #expect(
    RemoteDesktopCopyError.destinationExists.localizedDescription
      == "An item with the same name already exists on the VM Desktop"
  )
  #expect(RemoteDesktopCopyError.timedOut.localizedDescription == "File copy timed out")
}

@Test("SCP commits staged items to the Desktop with fail-fast shell semantics")
func scpDesktopCommitCommand() {
  let client = SystemSSHClient(host: "192.168.64.24")
  let command = client.desktopCommitCommand(
    itemNames: ["Report's.txt"],
    stagingDirectory: ".lume-drop-1234"
  )

  #expect(command.hasPrefix("set -e; "))
  #expect(command.contains(#""$HOME/.lume-drop-1234/"'Report'\''s.txt'"#))
  #expect(command.contains("mv -n"))
  #expect(command.contains("exit 73"))
  #expect(command.contains("trap rollback EXIT"))
  #expect(command.hasSuffix("trap - EXIT"))
}

@Test("One staging directory is threaded through prepare, transfer, and commit")
func scpStagingDirectoryConsistency() {
  let client = SystemSSHClient(host: "192.168.64.24")
  let stagingDirectory = ".lume-drop-fixed"
  let preparation = client.desktopPreparationCommand(
    itemNames: ["example.txt"],
    stagingDirectory: stagingDirectory
  )
  let transfer = client.scpArguments(
    sourcePaths: ["/tmp/example.txt"],
    destinationDirectory: stagingDirectory
  )
  let commit = client.desktopCommitCommand(
    itemNames: ["example.txt"],
    stagingDirectory: stagingDirectory
  )

  #expect(preparation.contains(stagingDirectory))
  #expect(transfer.last == "lume@192.168.64.24:\(stagingDirectory)/")
  #expect(commit.contains(stagingDirectory))
}

@Test("A late Desktop collision rolls every staged item back")
func scpDesktopCommitRollsBackOnCollision() throws {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  let desktop = root.appendingPathComponent("Desktop")
  let stagingDirectory = ".lume-drop-rollback"
  let staging = root.appendingPathComponent(stagingDirectory)
  try FileManager.default.createDirectory(at: desktop, withIntermediateDirectories: true)
  try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: root) }

  try Data("first".utf8).write(to: staging.appendingPathComponent("first.txt"))
  try Data("second".utf8).write(to: staging.appendingPathComponent("second.txt"))
  try Data("existing".utf8).write(to: desktop.appendingPathComponent("second.txt"))

  let client = SystemSSHClient(host: "192.168.64.24")
  let process = Process()
  process.executableURL = URL(fileURLWithPath: "/bin/sh")
  process.arguments = [
    "-c",
    client.desktopCommitCommand(
      itemNames: ["first.txt", "second.txt"],
      stagingDirectory: stagingDirectory
    ),
  ]
  var environment = ProcessInfo.processInfo.environment
  environment["HOME"] = root.path
  process.environment = environment
  process.standardOutput = FileHandle.nullDevice
  process.standardError = FileHandle.nullDevice
  try process.run()
  process.waitUntilExit()

  #expect(process.terminationStatus == 73)
  #expect(FileManager.default.fileExists(atPath: staging.appendingPathComponent("first.txt").path))
  #expect(FileManager.default.fileExists(atPath: staging.appendingPathComponent("second.txt").path))
  #expect(!FileManager.default.fileExists(atPath: desktop.appendingPathComponent("first.txt").path))
  #expect(
    try String(contentsOf: desktop.appendingPathComponent("second.txt"), encoding: .utf8)
      == "existing"
  )
}
