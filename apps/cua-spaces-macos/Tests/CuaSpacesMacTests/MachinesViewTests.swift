// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The Machines page (CUA-1673): the core's `machines_view::view`, which
/// the classic page and the New UI both draw. Its parity frames replayed
/// through the Swift binding, the app's own view, and the bridge's rename
/// and Remove from account.
extension ParityTests {
    /// The `views` of `parity/machines.json` drawn here equal the golden's
    /// view frames (`parity.rs`'s `view_frame`), word for word.
    @Test("machines views") func machinesViews() throws {
        let flow = try json(Self.dir.appendingPathComponent("machines.json"))
        let golden = try json(Self.dir.appendingPathComponent("golden/machines.json"))
        let want = ((golden["frames"] as? [[String: Any]]) ?? []).filter { $0["view"] != nil }
        let views = flow["views"] as! [[String: Any]]
        #expect(want.count == views.count && !views.isEmpty)
        for (v, w) in zip(views, want) {
            let view = appMachinesView(input: try appMachinesViewInputFromJson(json: text(v["input"])))
            let got = Self.viewFrame(v["name"] as! String, view)
            #expect(canonical(got) == canonical(w), "\(v["name"] as! String)")
        }
    }

    /// A host action as the core's JSON names it (`provide-spaces`).
    static func wire(_ a: AppHostActionId) -> String {
        String(describing: a).map { $0.isUppercase ? "-\($0.lowercased())" : String($0) }.joined()
    }

    static func viewFrame(_ name: String, _ v: AppMachinesView) -> [String: Any] {
        let byId = Dictionary(uniqueKeysWithValues: v.machines.map { ($0.id, $0) })
        let groups: [[String: Any]] = v.groups.map { g in
            var title = g.title
            if g.collapsed { title += " (collapsed)" }
            if let all = g.removeAll { title += " \u{b7} \(all.label)" }
            let rows = g.ids.compactMap { byId[$0] }.map { m in
                "\(m.online ? "\u{25cf}" : "\u{25cb}") \(m.name)\(m.badge.map { " [\($0)]" } ?? "") \u{2014} \(m.line)"
            }
            return ["group": title, "rows": rows]
        }
        let pages: [[String]] = v.machines.map { m in
            var lines: [String] = []
            var head = "\(m.name) \u{2014} \(m.status)"
            if let by = m.sharedBy { head += " \u{b7} \(by)" }
            if let r = m.rename { head += " (rename: \(r.target))" }
            lines.append(head)
            if let n = m.notice { lines.append("notice: \(n)\(m.noticeAction.map { " [\($0.label)]" } ?? "")") }
            for t in m.toggles {
                let how = !t.editable ? "read-only" : !t.enabled ? "disabled"
                    : t.setup.map { "sets up: \($0)" } ?? "runs: \(t.actions.map(wire).joined(separator: ", "))"
                lines.append("\(t.label): \(t.on ? "on" : "off") (\(how))")
            }
            if let note = m.togglesNote { lines.append("note: \(note)") }
            for s in m.spaces { lines.append("space \(s.name) \u{b7} \(s.os) \u{b7} \(s.status) -> \(s.id)") }
            if let e = m.spacesEmpty { lines.append("spaces: \(e)") }
            let ns = m.newSpace
            lines.append("\(ns.label) on \(ns.on)\(ns.enabled ? "" : " (disabled: \(ns.help ?? ""))")")
            lines.append("details: " + m.details.map { "\($0.label): \($0.value)" }.joined(separator: "; "))
            if let r = m.remove {
                let what = r.host ? "host setup" : [r.machine, r.device].compactMap { $0 }.joined(separator: " + ")
                let state = !r.enabled ? r.help.map { " (disabled: \($0))" } ?? "" : ""
                lines.append("\(r.label) -> \(what)\(state)")
            }
            return lines
        }
        return ["view": name, "summary": v.summary, "groups": groups, "pages": pages]
    }
}

@MainActor
@Suite("Machines page")
struct MachinesViewTests {
    /// A model signed in, with the fixture account's devices and machines read.
    private func signedInModel(backend: FixtureSpacesBackend = FixtureSpacesBackend(),
                               devices: FixtureDevices = FixtureDevices()) async -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-machines-\(UUID().uuidString)")
        let m = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry(), devices: devices)
        await m.refresh()
        m.devices.signedIn = true
        await m.devices.refresh()
        return m
    }

    /// The classic page and the New UI draw one view: the same machines,
    /// This Mac first with its switches, a device of its own removable.
    @Test func bothUIsDrawTheCoresView() async {
        let m = await signedInModel()
        let view = m.machinesView
        #expect(view.machines.map(\.id) == m.machinePageRows.map(\.id).filter { id in
            view.groups.flatMap(\.ids).contains(id)
        })
        let me = view.machines.first { $0.current }
        #expect(me?.toggles.map(\.label) == ["Provide Spaces", "Share this desktop"])
        #expect(me?.toggles.allSatisfy(\.editable) == true)
        #expect(view.summary.contains("online"))
        // Another machine's switches are read-only.
        #expect(view.machines.filter { !$0.current }.allSatisfy { $0.toggles.allSatisfy { !$0.editable } })
        // A device of its own leaves the account by revoking it.
        let work = view.machines.first { $0.name == "Work laptop" }
        #expect(work?.remove?.device == "dev_work")
        #expect(work?.remove?.label == "Remove from account")
        // No Space actions on a machine: its Spaces only lead to the Space.
        #expect(view.machines.allSatisfy { $0.newSpace.label == "New Space here" })

        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let answer = bridge.machines()["view"] as? [String: Any]
        #expect(answer?["summary"] as? String == view.summary)
        let ids = (answer?["machines"] as? [[String: Any]])?.map { $0["id"] as? String }
        #expect(ids == view.machines.map(\.id))
    }

    /// `machines.rename` renames the relay machine from this enrolled
    /// device; `machines.remove` takes a machine off the relay and revokes
    /// the device on it.
    @Test func renameAndRemoveFromAccount() async throws {
        let backend = FixtureSpacesBackend()
        let devices = FixtureDevices()
        let m = await signedInModel(backend: backend, devices: devices)
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        #expect(WebUIBridge.methods.contains("machines.rename"))
        #expect(WebUIBridge.methods.contains("machines.remove"))

        _ = try await bridge.handle("machines.rename", ["id": "m1", "name": "  Studio  "])
        #expect(devices.calls.contains("rename-machine:m1:Studio"))

        _ = try await bridge.handle("machines.remove", ["machine": "m2", "device": "dev_old"])
        #expect(backend.unregistered == ["relay:m2"])
        #expect(devices.calls.contains("revoke:dev_old"))

        // Nothing to remove is refused.
        await #expect(throws: (any Error).self) { _ = try await bridge.handle("machines.remove", [:]) }
    }
}
