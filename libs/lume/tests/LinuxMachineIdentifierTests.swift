import Foundation
import Testing
import Virtualization

@testable import lume

/// A saved state restores only into a VM with the machine identifier it was
/// saved with. Linux VMs used to get a new generic identifier on every run,
/// so `lume suspend` could never resume one ("invalid argument").
struct LinuxMachineIdentifierTests {
    private func temporaryDirectory() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    @Test func aLinuxVMKeepsOneGenericMachineIdentifier() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let directory = VMDirectory(Path(root.path))

        let first = directory.linuxMachineIdentifier()
        let second = directory.linuxMachineIdentifier()

        #expect(first == second)
        #expect(VZGenericMachineIdentifier(dataRepresentation: first) != nil)
        #expect(FileManager.default.fileExists(atPath: directory.linuxMachineIdentifierPath.path))
    }

    @Test func anUnreadableIdentifierIsReplaced() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let directory = VMDirectory(Path(root.path))
        try Data("not an identifier".utf8).write(to: directory.linuxMachineIdentifierPath.url)

        let identifier = directory.linuxMachineIdentifier()

        #expect(VZGenericMachineIdentifier(dataRepresentation: identifier) != nil)
        #expect(directory.linuxMachineIdentifier() == identifier)
    }

    @MainActor
    @Test func aCloneGetsItsOwnIdentifierUnlessItKeepsTheSavedState() throws {
        let tempConfigDir = try temporaryDirectory()
        let tempHomeDir = try temporaryDirectory()
        defer {
            try? FileManager.default.removeItem(at: tempConfigDir)
            try? FileManager.default.removeItem(at: tempHomeDir)
        }
        let previousXDGConfigHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"]
        setenv("XDG_CONFIG_HOME", tempConfigDir.path, 1)
        defer {
            if let previousXDGConfigHome {
                setenv("XDG_CONFIG_HOME", previousXDGConfigHome, 1)
            } else {
                unsetenv("XDG_CONFIG_HOME")
            }
        }
        let settingsManager = SettingsManager(fileManager: .default)
        try settingsManager.setHomeDirectory(path: tempHomeDir.path)
        let home = Home(settingsManager: settingsManager, fileManager: .default)

        let source = try home.getVMDirectory("seed")
        try FileManager.default.createDirectory(at: source.dir.url, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 1024).write(to: source.diskPath.url)
        try Data(repeating: 2, count: 64).write(to: source.nvramPath.url)
        try source.saveConfig(try VMConfig(
            os: "linux", cpuCount: 2, memorySize: 1 << 30, diskSize: 1024,
            macAddress: "02:00:00:00:00:09", display: "1024x768"))
        let identifier = source.linuxMachineIdentifier()
        try Data("state".utf8).write(to: source.statePath.url)

        try home.cloneVMDirectory(from: "seed", to: "fresh")
        let fresh = try home.getVMDirectory("fresh")
        #expect(!FileManager.default.fileExists(atPath: fresh.linuxMachineIdentifierPath.path))
        #expect(fresh.linuxMachineIdentifier() != identifier)

        try home.cloneVMDirectory(from: "seed", to: "resumable", includeSavedState: true)
        let resumable = try home.getVMDirectory("resumable")
        #expect(resumable.linuxMachineIdentifier() == identifier)
    }
}
