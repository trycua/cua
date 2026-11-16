// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// What `spaces.create` forwards for an agent home (CUA-2630): the spawn capability and its `cua.home.*` labels.
@Suite("A create's extras")
struct CreateExtrasTests {
    @Test func theWarmingHomesLabelsComeAsTheyCome() {
        let e = CreateExtrasValue.parse(["image": "x", "labels": ["cua.hidden": "home", "cua.relay": "off"]])
        #expect(e.labels == ["cua.hidden": "home", "cua.relay": "off"])
    }

    @Test func carryAnAgentHomesChildrenAndLabels() {
        let e = CreateExtrasValue.parse([
            "image": "x", "children": true,
            "labels": ["cua.home": "1", "cua.home.agent": "claude-code"],
        ])
        #expect(e.children)
        #expect(e.labels == ["cua.home": "1", "cua.home.agent": "claude-code"])
        #expect(!e.isEmpty)
    }

    @Test func areEmptyForAPlainCreate() {
        #expect(CreateExtrasValue.parse(["image": "x", "network": "none"]).isEmpty)
    }

    @Test func ignoreTheWrongTypesAndLabelsThatAreNotText() {
        let e = CreateExtrasValue.parse(["children": 1, "labels": ["a": 1, "": "x", "b": "", "c": "ok"]])
        #expect(!e.children)
        #expect(e.labels == ["c": "ok"])
    }

    @Test func areKeptByCreateIdUntilCleared() {
        let id = "pending:\(UUID().uuidString)"
        CreateExtras.shared.set(id, CreateExtrasValue(children: true, labels: ["cua.home": "1"]))
        #expect(CreateExtras.shared.get(id)?.children == true)
        CreateExtras.shared.clear(id)
        #expect(CreateExtras.shared.get(id) == nil)
    }
}
