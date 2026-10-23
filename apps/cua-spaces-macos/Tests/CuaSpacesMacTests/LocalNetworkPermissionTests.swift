// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

@MainActor
@Suite("Local Network access")
struct LocalNetworkPermissionTests {
    @Test func subnetHostIsTheFirstAddress() {
        let ip: UInt32 = 0xC0A8_0117 // 192.168.1.23
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FF00) == "192.168.1.1")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xC0A8_0101, mask: 0xFFFF_FF00) == "192.168.1.2")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xA9FE_0102, mask: 0xFFFF_0000) == nil)
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FFFF) == nil)
    }

    /// A probe that answers from a script.
    final class FakeProbe: LocalNetworkProbing, @unchecked Sendable {
        var answers: [AppLocalNetworkAccess]
        var routeTable = ""
        var prompts: [Int]
        private(set) var connects = 0
        init(_ answers: [AppLocalNetworkAccess], prompts: [Int] = [0]) {
            self.answers = answers
            self.prompts = prompts
        }
        func access() async -> AppLocalNetworkAccess {
            connects += 1
            return answers.count > 1 ? answers.removeFirst() : answers[0]
        }
        func routes() async -> String { routeTable }
        func promptsShowing() -> Int { prompts.count > 1 ? prompts.removeFirst() : prompts[0] }
    }

    func gate(_ probe: FakeProbe, click: Bool = true, shown: Shown) -> LocalNetworkGate {
        let defaults = UserDefaults(suiteName: "cua-ln-\(UUID().uuidString)")!
        let g = LocalNetworkGate(probe: probe, present: { step in
            shown.steps.append(step)
            return click
        }, open: { shown.opened.append($0) }, defaults: defaults)
        g.poll = .milliseconds(1)
        g.promptAppears = 0.05
        return g
    }

    @MainActor final class Shown {
        var steps: [AppLocalNetworkStep] = []
        var opened: [String] = []
    }

    @Test func allowedGoesAheadWithoutADialog() async throws {
        let shown = Shown()
        let g = gate(FakeProbe([.allowed]), shown: shown)
        g.asked = true
        try await g.check()
        #expect(shown.steps.isEmpty)
    }

    /// Not allowed yet: the explanation first, then macOS's prompt; Allow
    /// lets the create go ahead.
    @Test func explainedThenAllowedGoesAhead() async throws {
        let shown = Shown()
        let probe = FakeProbe([.blocked, .allowed], prompts: [0, 1, 1])
        let g = gate(probe, shown: shown)
        try await g.check()
        #expect(shown.steps.map(\.kind) == [.explain])
        // Nothing connected before the explanation (that would make macOS ask first).
        #expect(probe.connects == 2)
        #expect(shown.steps[0].title == "Allow Cua Spaces on your local network")
        #expect(g.asked)
    }

    /// Don't Allow (the prompt goes, access stays blocked): the create fails
    /// at once with the fix, and the dialog opens Local Network settings.
    @Test func deniedFailsAtOnceWithTheFix() async throws {
        let shown = Shown()
        let probe = FakeProbe([.blocked], prompts: [0, 1, 0])
        let g = gate(probe, shown: shown)
        // Never the whole wait for an answer: the prompt went, so it is over.
        g.promptAnswered = 600
        let started = Date()
        await #expect(throws: LocalNetworkRefusal.self) { try await g.check() }
        #expect(Date().timeIntervalSince(started) < 60)
        for _ in 0..<50 where shown.opened.isEmpty { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(shown.steps.map(\.kind) == [.explain, .fail])
        #expect(shown.steps[1].message.contains("Allow Cua Spaces in System Settings \u{203a} Privacy & Security \u{203a} Local Network"))
        #expect(shown.opened == ["x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"])
        // Asked before: no second explanation, straight to the fix.
        let again = Shown()
        let second = LocalNetworkGate(probe: FakeProbe([.blocked]), present: { again.steps.append($0); return false },
                                      open: { _ in }, defaults: g.defaults)
        do {
            try await second.check()
            Issue.record("expected a refusal")
        } catch let refusal as LocalNetworkRefusal {
            #expect(refusal.variant == "local_network_denied")
        }
        for _ in 0..<50 where again.steps.isEmpty { try? await Task.sleep(for: .milliseconds(10)) }
        #expect(again.steps.map(\.kind) == [.fail])
    }

    @Test func cancelOnTheExplanationCancelsTheCreate() async {
        let shown = Shown()
        let g = gate(FakeProbe([.blocked]), click: false, shown: shown)
        do {
            try await g.check()
            Issue.record("expected a cancel")
        } catch {
            #expect(isCancelled(error))
        }
        #expect(!g.asked)
    }

    @Test func aVpnRouteFailsWithItsOwnWords() async {
        let shown = Shown()
        let probe = FakeProbe([.allowed])
        probe.routeTable = "192.168.64/24      10.8.0.1           UGSc                utun4\n"
        let g = gate(probe, shown: shown)
        g.asked = true
        do {
            try await g.check()
            Issue.record("expected a refusal")
        } catch let refusal as LocalNetworkRefusal {
            #expect(refusal.variant == "local_network_vpn")
            #expect(refusal.message.hasPrefix("A VPN or firewall is blocking this Mac's own network"))
        } catch {
            Issue.record("\(error)")
        }
    }

    @Test func aRefusedMacosCreateFailsItsRowWithTheCoresWords() async {
        let backend = FixtureSpacesBackend(rows: [])
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-ln-\(UUID().uuidString)")
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil, clock: { fixtureNow }),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("o.json").path),
                             settingsPath: dir.appendingPathComponent("s.json").path, telemetry: FixtureTelemetry())
        let shown = Shown()
        let g = gate(FakeProbe([.blocked]), shown: shown)
        g.asked = true
        model.localNetworkGate = g
        let args = AppCreateSpaceArgs(image: "ghcr.io/trycua/macos:26", on: "local", kind: .vm, runtime: .auto,
                                      name: nil, cpus: nil, memoryMb: nil, diskGb: nil, spacesd: true, gpu: nil)
        await #expect(throws: LocalNetworkRefusal.self) {
            try await model.runCreate(args, os: .macos, pendingId: "pending:mac")
        }
        #expect(backend.created.isEmpty)
        let row = model.creates.pending.first { $0.id == "pending:mac" }
        #expect(row?.error?.hasPrefix("Cua Spaces can't connect to macOS Spaces on this Mac.") == true)
    }

    @Test func insufficientDiskCaseNameIsTheMirrorLabel() {
        let error = CuaError.InsufficientDisk(message: "need 52 GB")
        #expect(Mirror(reflecting: error).children.first?.label == "InsufficientDisk")
    }
}
