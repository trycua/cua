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
  let command = client.desktopPreparationCommand(itemNames: ["Report's.txt"])

  #expect(command.hasPrefix("mkdir -p \"$HOME/Desktop\""))
  #expect(command.contains(#""$HOME/Desktop/"'Report'\''s.txt'"#))
  #expect(command.contains("exit 73"))
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

  let arguments = client.scpArguments(sourcePaths: sourcePaths)
  let separator = try #require(arguments.firstIndex(of: "--"))

  #expect(arguments.contains("-r"))
  #expect(arguments.contains("-P"))
  #expect(arguments.contains("2222"))
  #expect(Array(arguments[(separator + 1)..<(arguments.count - 1)]) == sourcePaths)
  #expect(arguments.last == "lume@192.168.64.24:Desktop/")
}

@Test("SCP omits a port override for standard SSH")
func scpFileDropDefaultPortArguments() {
  let client = SystemSSHClient(host: "192.168.64.24")
  let arguments = client.scpArguments(sourcePaths: ["/tmp/example.txt"])

  #expect(!arguments.contains("-P"))
}

@Test("SCP rejects an empty file drop before launching a process")
func scpRejectsEmptyFileDrop() {
  let client = SystemSSHClient(host: "192.168.64.24")

  #expect(throws: SSHError.self) {
    try client.copyToRemoteDesktop([])
  }
}

@Test("SCP rejects duplicate destination names before connecting")
func scpRejectsDuplicateDestinationNames() throws {
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
    try client.copyToRemoteDesktop([first, second])
    Issue.record("Expected duplicate destination names to be rejected")
  } catch let error as SSHError {
    guard case .commandFailed(_, let message) = error else {
      Issue.record("Unexpected SSH error: \(error)")
      return
    }
    #expect(message == "Dropped items must have unique names")
  } catch {
    Issue.record("Unexpected error: \(error)")
  }
}
