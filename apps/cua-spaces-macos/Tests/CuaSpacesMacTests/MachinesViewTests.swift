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
                    + (m.health.isEmpty ? "" : " [\(m.health)]")
            }
            return ["group": title, "rows": rows]
        }
        let pages: [[String]] = v.machines.map { m in
            var lines: [String] = []
            var head = "\(m.name) \u{2014} \(m.status)"
            if let by = m.sharedBy { head += " \u{b7} \(by)" }
            if let r = m.rename { head += " (rename: \(r.target))" }
            lines.append(head)
            for i in m.issues {
                lines.append("\(m.issuesTitle.lowercased()) \(i.level): \(i.text)\(i.action.map { " [\($0.label)]" } ?? "")")
            }
            if let n = m.notice { lines.append("notice: \(n)\(m.noticeAction.map { " [\($0.label)]" } ?? "")") }
            for t in m.toggles {
                let how = !t.editable ? "read-only" : !t.enabled ? "disabled"
                    : t.setup.map { "sets up: \($0)" } ?? "runs: \(t.actions.map(wire).joined(separator: ", "))"
                lines.append("\(t.label): \(t.on ? "on" : "off") (\(how))")
            }
            if let note = m.togglesNote { lines.append("note: \(note)") }
            if let d = m.desktop {
                lines.append("desktop \(d.name) \u{b7} \(d.os) \u{b7} \(d.status) [\(d.label)] -> \(d.id)")
            }
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
        var frame: [String: Any] = ["view": name, "summary": v.summary, "groups": groups, "pages": pages]
        if !v.clouds.group.rows.isEmpty { frame["clouds"] = cloudsFrame(v.clouds, order: v.order) }
        return frame
    }

    /// The Clouds group and each cloud's page in words (`parity.rs`'s `view_frame`).
    static func cloudsFrame(_ c: AppCloudsView, order: [String]) -> [String: Any] {
        let dot = { (h: String) in h == "ok" ? "\u{25cf}" : h == "warn" ? "\u{25d0}" : "\u{25cb}" }
        let rows = c.group.rows.map { "\(dot($0.health)) \($0.name) \u{2014} \($0.line) (\($0.logo))" }
        let pages: [[String]] = c.pages.map { p in
            var lines = ["\(p.name) \u{2014} \(p.status)"]
            for i in p.issues {
                lines.append("issue \(i.level): \(i.text)\(i.action.map { " [\($0.label)]" } ?? "")")
            }
            if let s = p.spend { lines.append("spend \(s.amount) \(s.ofBudget) (\(s.resets)) \(s.note)") }
            if let r = p.running {
                let ms = r.machines.map { "\($0.name) \($0.by) \($0.cost)".trimmingCharacters(in: .whitespaces) }
                lines.append("\(r.title): \(ms.joined(separator: "; "))")
            }
            if let t = p.token { lines.append("token: \(t.lock); \(t.holdersLabel) \(t.holders)") }
            for s in p.settings { lines.append("\(s.label): \(s.value)") }
            if let a = p.switchAuth { lines.append("[\(a.label)]") }
            if let a = p.approval {
                let rows = a.rows.map { "\($0.label) \($0.value)" }.joined(separator: "; ")
                lines.append("approval: \(a.title) \(a.intro) (\(rows)) [\(a.approveLabel) / \(a.denyLabel)]")
            }
            if let d = p.disconnect { lines.append("\(d.label) -> \(d.confirm.confirmLabel)") }
            return lines
        }
        return ["order": order, "title": c.group.title, "rows": rows, "connect": c.group.connectLabel, "pages": pages]
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
        #expect(me?.toggles.map(\.label) == ["Let other machines control this desktop"])
        #expect(me?.toggles.allSatisfy(\.editable) == true)
        #expect(view.summary.contains("online"))
        // Another machine's switches are read-only.
        #expect(view.machines.filter { !$0.current }.allSatisfy { $0.toggles.allSatisfy { !$0.editable } })
        // A device of its own leaves the account by revoking it.
        let work = view.machines.first { $0.name == "Work laptop" }
        #expect(work?.remove?.device == "dev_work")
        #expect(work?.remove?.label == "Revoke")
        #expect(work?.line == "Signed in, not sharing")
        // One list: no separate group for it.
        #expect(view.groups.first { $0.id == "devices" } == nil)
        #expect(view.groups.contains { $0.ids.contains("dev_work") })
        // No Space actions on a machine: its Spaces only lead to the Space.
        #expect(view.machines.filter { $0.line != "Signed in, not sharing" }.allSatisfy { $0.newSpace.label == "New Space here" })
        #expect(work?.newSpace.label == "")

        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let answer = bridge.machines()["view"] as? [String: Any]
        #expect(answer?["summary"] as? String == view.summary)
        let ids = (answer?["machines"] as? [[String: Any]])?.map { $0["id"] as? String }
        #expect(ids == view.machines.map(\.id))
    }

    /// A connected cloud is a row in the Clouds group, with its page; Disconnect forgets it.
    @Test func aConnectedCloudIsInTheCloudsGroup() async throws {
        let backend = FixtureSpacesBackend()
        let m = await signedInModel(backend: backend)
        await m.cloud.refresh()
        #expect(m.machinesView.clouds.group.rows.isEmpty, "nothing connected: no group, only the sheet's own entry")
        backend.fixtureCloudConnected = true
        await m.cloud.refresh()
        let clouds = m.machinesView.clouds
        #expect(clouds.group.title == "Clouds" && clouds.group.connectLabel == "Connect a cloud")
        #expect(m.machinesView.order.first == "clouds", "the clouds come first in the list")
        #expect(clouds.group.rows.map(\.name) == ["AWS"])
        #expect(clouds.pages.first?.status == "Connected")
        #expect(clouds.pages.first?.disconnect?.label == "Disconnect AWS\u{2026}")
        try await m.disconnectCloud("aws", deleteResources: false)
        #expect(backend.fixtureCloudCalls.contains("cloud_disconnect"))
        #expect(m.machinesView.clouds.group.rows.isEmpty)
    }

    /// An agent's link waits; the cloud says this person may approve, so its
    /// page offers the review, and `clouds.approve` reaches `cloud_approve`.
    @Test func aPersonApprovesAnAgentsLink() async throws {
        let backend = FixtureSpacesBackend()
        let m = await signedInModel(backend: backend)
        backend.fixtureCloudConnected = true
        backend.fixtureCloudWaiting = true
        await m.cloud.refresh()
        let page = try #require(m.machinesView.clouds.pages.first)
        #expect(page.status == "Waiting for approval")
        #expect(page.approval?.intro.hasPrefix("eval-runner set up AWS") == true)
        #expect(page.issues.first?.action?.id == "review")
        #expect(WebUIBridge.methods.contains("clouds.approve"))
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        _ = try await bridge.handle("clouds.approve", ["provider": "aws", "approve": true])
        #expect(backend.fixtureCloudCalls.contains("cloud_approve"))
        #expect(m.machinesView.clouds.pages.first?.status == "Connected")
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

    /// `checkMachineHealth` asks only the machines the view says can be
    /// checked: This Mac (through its daemon) and your machines that are
    /// online; never an offline or a team machine.
    @Test func healthIsCheckedOnlyWhereItCanBe() async {
        let backend = FixtureSpacesBackend()
        let m = await signedInModel(backend: backend)
        let view = m.machinesView
        let me = view.machines.first { $0.current }
        #expect(me?.healthTarget == "")
        #expect(view.machines.filter { !$0.current && !$0.online }.allSatisfy { $0.healthTarget == nil })
        #expect(view.machines.filter(\.team).allSatisfy { $0.healthTarget == nil })
        await m.checkMachineHealth()
        #expect(Set(backend.healthChecked) == Set(view.machines.compactMap(\.healthTarget)))
        #expect(backend.healthChecked.contains(""))
        // Healthy: nothing extra on any machine.
        #expect(m.machinesView.machines.allSatisfy { $0.issues.isEmpty && $0.health.isEmpty })
    }

    /// What This Mac's daemon reports shows as its issues, with the fix,
    /// on the classic page and through the bridge (the New UI).
    @Test func issuesShowWithTheirFix() async throws {
        let backend = FixtureSpacesBackend()
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"
        backend.fixtureHealth[""] = AppMachinesViewHealth(
            id: "", checks: [
                AppMachinesViewCheck(id: "machine.local_network", status: "fail", message: "blocked",
                                     remediation: "", facts: ["app": "Cua Spaces", "settings_url": url]),
                AppMachinesViewCheck(id: "machine.disk", status: "pass", message: "200 GB free",
                                     remediation: "", facts: [:]),
            ], cuaVersion: "0.4.1", error: nil)
        let m = await signedInModel(backend: backend)
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        #expect(WebUIBridge.methods.contains("machines.checkHealth"))
        #expect(try await bridge.handle("machines.checkHealth", [:]) is NSNull)
        let me = try #require(m.machinesView.machines.first { $0.current })
        #expect(me.health == "fail")
        #expect(me.issuesTitle == "Issues")
        #expect(me.issues.map(\.text) == [
            "Cua can\u{2019}t reach this Mac\u{2019}s macOS Spaces: Local Network access is off for Cua Spaces.",
        ])
        #expect(me.issues.first?.action?.label == "Open Local Network settings")
        #expect(me.issues.first?.action?.url == url)
        // The New UI reads the same issues in `machines.list`'s view.
        let answer = bridge.machines()["view"] as? [String: Any]
        let mine = (answer?["machines"] as? [[String: Any]])?.first { $0["current"] as? Bool == true }
        #expect((mine?["issues"] as? [[String: Any]])?.count == 1)
        #expect(mine?["health"] as? String == "fail")
        // Once fixed, the issue goes.
        backend.fixtureHealth[""] = nil
        await m.checkMachineHealth()
        #expect(m.machinesView.machines.first { $0.current }?.issues.isEmpty == true)
    }
}
