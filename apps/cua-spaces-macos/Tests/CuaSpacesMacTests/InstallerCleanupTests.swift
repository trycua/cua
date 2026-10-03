// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

@Suite("Installer cleanup")
struct InstallerCleanupTests {
    static let id = "com.trycua.spaces.macos"
    static let home = "/Users/ada"
    static let dmg = "/Users/ada/Downloads/cua-spaces-0.6.1-darwin-universal.dmg"

    static func hdiutil(_ images: [(path: String, mounts: [String])]) -> Data {
        let list: [[String: Any]] = images.map { image in
            ["image-path": image.path,
             "system-entities": [["dev-entry": "/dev/disk9"]] + image.mounts.map { ["mount-point": $0] }]
        }
        return try! PropertyListSerialization.data(fromPropertyList: ["images": list], format: .xml, options: 0)
    }

    func cleanup(_ volumes: FixtureInstallerVolumes, bundlePath: String = "/Applications/Cua Spaces.app",
                 defaults: UserDefaults = UserDefaults(suiteName: "cua-installer-\(UUID().uuidString)")!) -> InstallerCleanup {
        InstallerCleanup(volumes: volumes, bundleIdentifier: Self.id, bundlePath: bundlePath,
                         home: Self.home, defaults: defaults)
    }

    @Test func parsesImagesAndTheirMountPoints() {
        let images = InstallerCleanup.images(Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"]), ("/tmp/x.dmg", [])]))
        #expect(images == [InstallerImage(path: Self.dmg, mountPoints: ["/Volumes/Cua Spaces"]),
                           InstallerImage(path: "/tmp/x.dmg", mountPoints: [])])
        #expect(InstallerCleanup.images(Data("not a plist".utf8)).isEmpty)
    }

    @Test func onlyAnApplicationsFolderCountsAsInstalled() {
        #expect(InstallerCleanup.installed("/Applications/Cua Spaces.app", home: Self.home))
        #expect(InstallerCleanup.installed("/Users/ada/Applications/Cua Spaces.app", home: Self.home))
        #expect(!InstallerCleanup.installed("/Volumes/Cua Spaces/Cua Spaces.app", home: Self.home))
        #expect(!InstallerCleanup.installed(
            "/private/var/folders/x/T/AppTranslocation/1234/d/Cua Spaces.app", home: Self.home))
        #expect(!InstallerCleanup.installed("/Users/ada/Downloads/Cua Spaces.app", home: Self.home))
    }

    @Test func ejectsTheInstallerAndOffersItsDownloadForTheTrash() {
        let volumes = FixtureInstallerVolumes(images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"])]),
                                              apps: ["/Volumes/Cua Spaces": [Self.id]])
        let offered = cleanup(volumes).ejectLeftovers()
        #expect(volumes.calls == ["eject /Volumes/Cua Spaces"])
        #expect(offered.map(\.path) == [Self.dmg])
    }

    @Test func ejectsEveryCopyOfTheInstallerButLeavesOtherImages() {
        let other = "/Users/ada/Downloads/Figma.dmg"
        let volumes = FixtureInstallerVolumes(
            images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"]),
                                  ("/Users/ada/Desktop/cua-spaces-0.6.0.dmg", ["/Volumes/Cua Spaces 1"]),
                                  (other, ["/Volumes/Figma"]),
                                  ("/tmp/detached.dmg", [])]),
            apps: ["/Volumes/Cua Spaces": [Self.id], "/Volumes/Cua Spaces 1": [Self.id],
                   "/Volumes/Figma": ["com.figma.Desktop"]])
        let offered = cleanup(volumes).ejectLeftovers()
        #expect(volumes.calls == ["eject /Volumes/Cua Spaces", "eject /Volumes/Cua Spaces 1"])
        #expect(offered.map(\.path) == [Self.dmg], "only a .dmg in Downloads is offered for the Trash")
    }

    @Test func neverEjectsTheImageTheAppRunsFrom() {
        let volumes = FixtureInstallerVolumes(images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"])]),
                                              apps: ["/Volumes/Cua Spaces": [Self.id]])
        #expect(cleanup(volumes, bundlePath: "/Volumes/Cua Spaces/Cua Spaces.app").ejectLeftovers().isEmpty)
        #expect(cleanup(volumes, bundlePath: "/private/var/folders/x/T/AppTranslocation/1/d/Cua Spaces.app")
            .ejectLeftovers().isEmpty)
        #expect(volumes.calls.isEmpty)
    }

    @Test func anImageThatWouldNotEjectIsNotOffered() {
        let volumes = FixtureInstallerVolumes(images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"])]),
                                              apps: ["/Volumes/Cua Spaces": [Self.id]])
        volumes.ejectFailure = "busy"
        #expect(cleanup(volumes).ejectLeftovers().isEmpty)
        #expect(volumes.calls == ["eject /Volumes/Cua Spaces"])
    }

    @Test func aKeptDownloadIsEjectedButNotOfferedAgain() throws {
        let defaults = try #require(UserDefaults(suiteName: "cua-installer-\(UUID().uuidString)"))
        let volumes = FixtureInstallerVolumes(images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"])]),
                                              apps: ["/Volumes/Cua Spaces": [Self.id]])
        let c = cleanup(volumes, defaults: defaults)
        let offered = c.ejectLeftovers()
        offered.forEach(c.keep)
        #expect(c.ejectLeftovers().isEmpty)
        #expect(volumes.calls == ["eject /Volumes/Cua Spaces", "eject /Volumes/Cua Spaces"])
    }

    @Test func trashingMovesTheDownload() throws {
        let volumes = FixtureInstallerVolumes(images: Self.hdiutil([(Self.dmg, ["/Volumes/Cua Spaces"])]),
                                              apps: ["/Volumes/Cua Spaces": [Self.id]])
        let c = cleanup(volumes)
        try c.ejectLeftovers().forEach(c.trash)
        #expect(volumes.calls == ["eject /Volumes/Cua Spaces", "trash \(Self.dmg)"])
    }
}

@MainActor
@Suite("Installer cleanup after onboarding")
struct InstallerCleanupOnboardingTests {
    @Test func asksAtOnceWhenOnboardingIsDone() {
        let onboarding = OnboardingModel(statePath: nil)
        onboarding.finish()
        var asked = 0
        AppDelegate.afterOnboarding(onboarding) { asked += 1 }
        #expect(asked == 1)
    }

    @Test func waitsForTheFirstRunToFinish() async {
        let onboarding = OnboardingModel(statePath: nil)
        var asked = 0
        AppDelegate.afterOnboarding(onboarding) { asked += 1 }
        #expect(asked == 0, "not while onboarding is showing")
        onboarding.finish()
        for _ in 0..<50 where asked == 0 { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(asked == 1)
    }
}
