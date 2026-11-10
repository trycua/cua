// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// An agent home's grants reach the daemon through a file in the cua home (CUA-2630).
@Suite("An agent home's grants file")
struct HomeGrantsTests {
    func temp() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("cua-home-grants-\(UUID().uuidString)", isDirectory: true)
    }

    func read(_ home: URL) throws -> [String: Any] {
        let data = try Data(contentsOf: home.appendingPathComponent("agent-home/grants.json"))
        return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    @Test func readsTheRequestsGrantsAndIgnoresTheWrongTypes() {
        #expect(HomeGrants.parse(["listAll": true, "machines": ["b", "a", "a", 3, ""]]) == HomeGrants(listAll: true, machines: ["a", "b"]))
        #expect(HomeGrants.parse(["listAll": "yes"]) == nil)
        #expect(HomeGrants.parse(nil) == nil)
    }

    @Test func isWrittenPerSpaceOwnerOnlyAndKeepsTheOtherHomes() throws {
        let home = temp()
        defer { try? FileManager.default.removeItem(at: home) }
        try HomeGrants.write(home: home, space: "local:home-a", HomeGrants(listAll: true))
        try HomeGrants.write(home: home, space: "local:home-b", HomeGrants(machines: ["m-air"]))
        try HomeGrants.write(home: home, space: "local:home-a", HomeGrants(machines: ["m-mini"]))
        let all = try read(home)
        let a = try #require(all["local:home-a"] as? [String: Any])
        let b = try #require(all["local:home-b"] as? [String: Any])
        #expect(a["list_all"] as? Bool == true && a["machines"] as? [String] == ["m-mini"])
        #expect(b["list_all"] as? Bool == false && b["machines"] as? [String] == ["m-air"])
        let mode = try FileManager.default.attributesOfItem(atPath: home.appendingPathComponent("agent-home/grants.json").path)[.posixPermissions] as? Int
        #expect(mode.map { $0 & 0o077 } == 0)
    }

    @Test func keepsWhatLaterRequestsMentionAddingToItAndNeverForgettingIt() throws {
        #expect(HomeGrants.parse(["mentions": ["ci", "local:ci", "ci", 4]]) == HomeGrants(mentions: ["ci", "local:ci"]))
        let home = temp()
        defer { try? FileManager.default.removeItem(at: home) }
        try HomeGrants.write(home: home, space: "local:home-m", HomeGrants(mentions: ["ci", "local:ci"]))
        try HomeGrants.write(home: home, space: "local:home-m", HomeGrants(mentions: ["studio"]))
        try HomeGrants.write(home: home, space: "local:home-m", HomeGrants(listAll: true))
        let m = try #require(try read(home)["local:home-m"] as? [String: Any])
        #expect(m["list_all"] as? Bool == true)
        #expect(m["mentions"] as? [String] == ["ci", "local:ci", "studio"])
    }

    @Test func usesCuaHomeElseDotCua() {
        #expect(HomeGrants.cuaHome(["CUA_HOME": "/x/y"]).path == "/x/y")
        #expect(HomeGrants.cuaHome(["HOME": "/Users/z"]).path == "/Users/z/.cua")
    }
}
