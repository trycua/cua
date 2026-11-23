// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// An in-memory daemon for Settings, Connections, GitHub: `github.*` as the
/// daemon answers them (hosts, accounts, the source in use; never a token).
final class FakeGithubTools: AgentsToolRunning, @unchecked Sendable {
    var calls: [(String, [String: Any])] = []
    var chosen: String?
    var off = false

    func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        calls.append((tool, args))
        switch tool {
        case "github.status", "github.rescan": break
        case "github.use": chosen = args["login"] as? String
        case "github.enable": off = !(args["enabled"] as? Bool ?? true)
        default: throw AgentsToolError(message: "unknown tool \(tool)")
        }
        let login = chosen ?? "ada"
        let account: ([String: Any]) -> [String: Any] = { $0 }
        return [
            "gh": ["found": true, "version": "2.101.0", "old": false],
            "hosts": [[
                "host": "github.com", "enabled": !off, "source": "gh", "login": login,
                "line": off ? "Turned off: Cua will not use GitHub on this host" : "Signed in as \(login) with gh",
                "problem": false,
                "accounts": [
                    account(["login": "ada", "active": true, "selected": login == "ada", "chosen": chosen == "ada", "valid": true, "scopes": ["repo"]]),
                    account(["login": "bob", "active": false, "selected": login == "bob", "chosen": chosen == "bob", "valid": true, "scopes": ["repo", "workflow"]]),
                ],
                "sources": [["id": "gh", "used": true, "present": true, "line": "gh, signed in as \(login)"]],
            ]],
            "can_save": true, "scanned_ms": 0,
        ] as [String: Any]
    }
}

@MainActor
struct GithubBridgeTests {
    @Test func theWebBridgeRoutesGithubToTheDaemon() {
        for m in ["github.status", "github.rescan", "github.use", "github.enable"] {
            #expect(WebUIBridge.methods.contains(m))
        }
    }

    @Test func eachMethodCallsItsOwnDaemonMethodWithOnlyItsOwnArguments() async throws {
        let (bridge, model, _) = try await BridgeContractTests().fixtureBridge()
        _ = try await bridge.handle("github.status", [:])
        _ = try await bridge.handle("github.use", ["host": "github.com", "login": "bob", "token": "must-not-travel"])
        _ = try await bridge.handle("github.use", ["host": "github.com"])
        _ = try await bridge.handle("github.enable", ["host": "ghe.example.com", "enabled": false])
        let tools = (model.backend as? BridgeContractTests.ToolBackend)?.github
        let calls = tools?.calls ?? []
        #expect(calls.map(\.0) == ["github.status", "github.use", "github.use", "github.enable"])
        #expect(calls[1].1 as NSDictionary == ["host": "github.com", "login": "bob"])
        // No login: gh's own account.
        #expect(calls[2].1 as NSDictionary == ["host": "github.com"])
        #expect(calls[3].1 as NSDictionary == ["host": "ghe.example.com", "enabled": false])
    }

    @Test func aMissingHostOrSwitchFailsBeforeTheDaemonIsAsked() async throws {
        let (bridge, model, _) = try await BridgeContractTests().fixtureBridge()
        await #expect(throws: (any Error).self) { _ = try await bridge.handle("github.use", ["login": "bob"]) }
        await #expect(throws: (any Error).self) { _ = try await bridge.handle("github.enable", ["host": "github.com"]) }
        let tools = (model.backend as? BridgeContractTests.ToolBackend)?.github
        #expect(tools?.calls.isEmpty == true)
    }
}
