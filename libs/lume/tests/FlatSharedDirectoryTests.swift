import Foundation
import Testing
import Virtualization

@testable import lume

@MainActor
@Test("A single automount folder is shared flat only when the run asks for it")
func flatSharedDirectoryIsOptIn() throws {
    let tag = VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag
    let one = [SharedDirectory(hostPath: "/tmp/flat", tag: tag, readOnly: true)]

    // By default the folder sits in a subfolder beside the live-update placeholder.
    let regular = try #require(
        BaseVirtualizationService.createDirectorySharingDevices(
            sharedDirectories: one, withLiveUpdatePlaceholder: true
        ).first as? VZVirtioFileSystemDeviceConfiguration)
    let regularShare = try #require(regular.share as? VZMultipleDirectoryShare)
    #expect(Swift.Set(regularShare.directories.keys) == [".lume-live-share", "flat"])

    let flat = try #require(
        BaseVirtualizationService.createDirectorySharingDevices(
            sharedDirectories: one, withLiveUpdatePlaceholder: true, flatAutomount: true
        ).first as? VZVirtioFileSystemDeviceConfiguration)
    let flatShare = try #require(flat.share as? VZSingleDirectoryShare)
    #expect(flatShare.directory.url.path == "/tmp/flat")
    #expect(flatShare.directory.isReadOnly)

    // Two automount folders keep the usual share even when flat is asked for.
    let two = one + [SharedDirectory(hostPath: "/tmp/other", tag: tag, readOnly: false)]
    let pair = try #require(
        BaseVirtualizationService.createDirectorySharingDevices(
            sharedDirectories: two, flatAutomount: true
        ).first as? VZVirtioFileSystemDeviceConfiguration)
    #expect(pair.share is VZMultipleDirectoryShare)
}

@Test("--flat-shared-dir needs exactly one --shared-dir")
func flatSharedDirNeedsOneShare() throws {
    try Run.parse(["test-vm", "--flat-shared-dir", "--shared-dir", "/tmp/a"]).validate()
    #expect(throws: Error.self) {
        try Run.parse(["test-vm", "--flat-shared-dir"]).validate()
    }
    #expect(throws: Error.self) {
        try Run.parse([
            "test-vm", "--flat-shared-dir", "--shared-dir", "/tmp/a", "--shared-dir", "/tmp/b",
        ]).validate()
    }
}
