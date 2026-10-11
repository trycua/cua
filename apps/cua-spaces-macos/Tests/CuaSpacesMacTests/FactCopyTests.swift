// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The Identifier row's copy button, from the core's fact.
@MainActor
@Suite("Fact copy")
struct FactCopyTests {
    static func identifier() -> AppFact {
        let row = AppSpaceRow(id: "direct:127.0.0.1:34752", name: "studio", provider: "direct",
                              spacesdVersion: "0.4.0", features: [], addedAt: nil, os: .linux,
                              osName: nil, osPrettyName: nil, image: nil, imageDigest: nil,
                              kind: nil, arch: nil, reachable: true, error: nil, host: nil, hostName: nil,
                              power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil)
        let facts = appSpaceDetail(space: appRowsToSpaces(rows: [row], nowMs: 0)[0]).facts
        #expect(facts.filter { $0.copy != nil }.map(\.label) == ["Identifier"])
        return facts.first { $0.copy != nil }!
    }

    @Test func copiesTheFullIdThenConfirmsBriefly() async throws {
        let fact = Self.identifier()
        var written: [String] = []
        let m = FactCopyModel(copy: fact.copy!, write: { written.append($0) })
        #expect(m.symbol == "doc.on.doc" && m.help == "Copy")
        m.copy(after: .milliseconds(50))
        #expect(written == ["direct:127.0.0.1:34752"])
        #expect(m.symbol == "checkmark" && m.help == "Copied")
        for _ in 0..<100 where m.copied { try await Task.sleep(for: .milliseconds(20)) }
        #expect(!m.copied && m.symbol == "doc.on.doc")
    }

    @Test func confirmsForTheCoresDuration() {
        #expect(Self.identifier().copy!.confirmMs == 1_500)
    }
}
