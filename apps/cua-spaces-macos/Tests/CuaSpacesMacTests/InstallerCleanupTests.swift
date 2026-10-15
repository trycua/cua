// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

@Suite("Installer cleanup")
struct InstallerCleanupTests {
    static let id = "com.trycua.spaces.macos"

    func withVolume(holding bundleIdentifier: String?, _ body: (URL) throws -> Void) throws {
        let volume = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-volume-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: volume) }
        try FileManager.default.createDirectory(at: volume, withIntermediateDirectories: true)
        if let bundleIdentifier {
            let contents = volume.appendingPathComponent("Cua Spaces.app/Contents")
            try FileManager.default.createDirectory(at: contents, withIntermediateDirectories: true)
            let info = ["CFBundleIdentifier": bundleIdentifier]
            try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
                .write(to: contents.appendingPathComponent("Info.plist"))
        }
        try body(volume)
    }

    @Test func selectsAnInstallerVolumeHoldingThisApp() throws {
        try withVolume(holding: Self.id) { volume in
            #expect(InstallerCleanup.installers(among: [volume], holding: Self.id) { _ in true } == [volume])
        }
    }

    @Test func skipsVolumesThatAreNotInstallers() throws {
        try withVolume(holding: Self.id) { volume in
            #expect(InstallerCleanup.installers(among: [volume], holding: Self.id) { _ in false }.isEmpty)
        }
    }

    @Test func skipsVolumesHoldingAnotherApp() throws {
        try withVolume(holding: "com.example.other") { volume in
            #expect(InstallerCleanup.installers(among: [volume], holding: Self.id) { _ in true }.isEmpty)
        }
    }

    @Test func skipsVolumesWithoutAnApp() throws {
        try withVolume(holding: nil) { volume in
            #expect(InstallerCleanup.installers(among: [volume], holding: Self.id) { _ in true }.isEmpty)
        }
    }

    @Test func installedOnlyInsideAnApplicationsFolder() {
        let home = URL(fileURLWithPath: "/Users/ada")
        let inside = ["/Applications/X.app", "/Users/ada/Applications/X.app"]
        let outside = ["/Volumes/Cua Spaces/X.app", "/private/var/folders/x/AppTranslocation/1234/d/X.app",
                       "/Users/ada/Downloads/X.app", "/ApplicationsFoo/X.app"]
        for path in inside { #expect(InstallerCleanup.installed(URL(fileURLWithPath: path), home: home)) }
        for path in outside { #expect(!InstallerCleanup.installed(URL(fileURLWithPath: path), home: home)) }
    }
}
