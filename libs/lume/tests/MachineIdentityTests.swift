import Foundation
import Testing
import Virtualization

@testable import lume

/// `lume set --machine-identifier / --mac-address` exists so a VM's machine
/// identity can be changed or *pinned* without rebuilding the image. Software
/// that licenses per-machine (Unity, for one) treats a changed identifier as a
/// brand-new machine, so pinning a fleet of throwaway clones to one identifier
/// keeps them all on a single activation instead of burning one per clone.
struct MachineIdentityTests {
    @Test func randomMachineIdentifierIsValidAndFresh() throws {
        let first = try LumeController.parseMachineIdentifier("random")
        let second = try LumeController.parseMachineIdentifier("random")

        #expect(!first.isEmpty)
        #expect(VZMacMachineIdentifier(dataRepresentation: first) != nil)
        #expect(first != second, "each 'random' must yield a distinct machine identity")
        // `new` is accepted as a synonym, and casing is not significant.
        #expect(try !LumeController.parseMachineIdentifier("NEW").isEmpty)
    }

    /// The pinning path: a base64 identifier read back from
    /// `lume get --format json` must round-trip byte-for-byte, or the target VM
    /// would not actually look like the same machine.
    @Test func base64MachineIdentifierRoundTrips() throws {
        let original = try LumeController.parseMachineIdentifier("random")
        let pinned = try LumeController.parseMachineIdentifier(original.base64EncodedString())

        #expect(pinned == original)
    }

    @Test func surroundingWhitespaceIsTolerated() throws {
        let original = try LumeController.parseMachineIdentifier("random")
        let encoded = original.base64EncodedString()

        #expect(try LumeController.parseMachineIdentifier("  \(encoded)  ") == original)
    }

    /// A bad value must fail at `set` time rather than at the VM's next boot.
    @Test func invalidMachineIdentifiersAreRejected() {
        #expect(throws: (any Error).self) {
            try LumeController.parseMachineIdentifier("not-base64!!")
        }
        #expect(throws: (any Error).self) {
            try LumeController.parseMachineIdentifier("")
        }
        // Valid base64, but not a machine identifier.
        #expect(throws: (any Error).self) {
            try LumeController.parseMachineIdentifier(Data([0x01, 0x02]).base64EncodedString())
        }
    }

    @Test func randomMacAddressIsValidAndFresh() throws {
        let first = try LumeController.parseMacAddress("random")
        let second = try LumeController.parseMacAddress("random")

        #expect(VZMACAddress(string: first) != nil)
        #expect(first != second)
    }

    @Test func explicitMacAddressIsPreservedAndValidated() throws {
        #expect(try LumeController.parseMacAddress("aa:bb:cc:dd:ee:ff") == "aa:bb:cc:dd:ee:ff")
        #expect(throws: (any Error).self) {
            try LumeController.parseMacAddress("aa:bb:cc:dd:ee")
        }
        #expect(throws: (any Error).self) {
            try LumeController.parseMacAddress("nope")
        }
    }
}
