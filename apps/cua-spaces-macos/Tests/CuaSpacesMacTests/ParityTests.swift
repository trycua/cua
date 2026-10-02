// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The app core's parity flows (libs/cua/crates/cua-spaces-app-core/parity),
/// replayed through the Swift bindings: typed records and enums in, typed
/// views out, projected into the same frames `parity.rs` records. The
/// transcripts must equal the goldens the Rust core, the Tauri shell and
/// the webview also match.
@Suite("App core parity (Swift)")
struct ParityTests {
    static let dir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("../../libs/cua/crates/cua-spaces-app-core/parity").standardized

    @Test("create-space") func createSpace() throws { try check("create-space", runCreateSpace) }
    @Test("create-resources") func createResources() throws { try check("create-resources", runCreateSpace) }
    @Test("create-gpu") func createGpu() throws { try check("create-gpu", runCreateSpace) }
    @Test("create-cancel") func createCancel() throws { try check("create-cancel", runProvisioning) }
    @Test("teleport-review") func teleportReview() throws { try check("teleport-review", runTeleport) }
    @Test("teleport-sign-ins") func teleportSignIns() throws { try check("teleport-sign-ins", runTeleport) }
    @Test("keyvault-approve-deny") func keyvault() throws { try check("keyvault-approve-deny", runKeyvault) }
    @Test("keyvault-unlock") func keyvaultUnlock() throws { try check("keyvault-unlock", runKeyvaultUnlock) }
    @Test("main-window") func mainWindow() throws { try check("main-window", runMainWindow) }
    @Test("notch") func notch() throws { try check("notch", runNotch) }
    @Test("notch-drag-trigger") func notchDragTrigger() throws { try check("notch-drag-trigger", runNotchDragTrigger) }
    @Test("provisioning") func provisioning() throws { try check("provisioning", runProvisioning) }
    @Test("stream-section") func streamSection() throws { try check("stream-section", runStreamSection) }
    @Test("space-facts") func spaceFacts() throws { try check("space-facts", runSpaceFacts) }
    @Test("picker-grid") func pickerGrid() throws { try check("picker-grid", runPickerGrid) }
    @Test("delete-space") func deleteSpace() throws { try check("delete-space", runDeleteSpace) }
    @Test("space-power") func spacePower() throws { try check("space-power", runSpacePower) }
    @Test("create-progress") func createProgress() throws { try check("create-progress", runProvisioning) }
    @Test("devices") func devices() throws { try check("devices", runDevices) }
    @Test("driver-card") func driverCard() throws { try check("driver-card", runDriverCard) }
    @Test("share-sheet") func shareSheet() throws { try check("share-sheet", runShare) }
    @Test("agents-page") func agentsPage() throws { try check("agents-page", runAgentsPage) }
    @Test("drive-page") func drivePage() throws { try check("drive-page", runDrivePage) }
    @Test("menu-count") func menuCount() throws { try check("menu-count", runMenuCount) }
    @Test("drive-onboarding") func driveOnboarding() throws { try check("drive-onboarding", runDriveOnboarding) }
    @Test("drive-storage") func driveStorage() throws { try check("drive-storage", runDriveStorage) }
    @Test("notifications") func notifications() throws { try check("notifications", runNotifications) }
    @Test("about") func about() throws { try check("about", runAbout) }
    @Test("your-cloud") func yourCloud() throws { try check("your-cloud", runYourCloud) }
    @Test("launch-at-login") func launchAtLogin() throws { try check("launch-at-login", runLaunchAtLogin) }
    @Test("telemetry-funnel") func telemetryFunnel() throws { try check("telemetry-funnel", runTelemetry) }
    @Test("experiments") func experiments() throws { try check("experiments", runExperiments) }
    @Test("placement-picker") func placementPicker() throws { try check("placement-picker", runPlacementPicker) }

    /// The app's own first run records exactly the golden's events: the
    /// `OnboardingModel` the window drives, on a recording telemetry.
    @MainActor @Test("telemetry-funnel through the first-run model")
    func telemetryFunnelThroughTheModel() throws {
        let flow = try json(Self.dir.appendingPathComponent("telemetry-funnel.json"))
        let golden = try json(Self.dir.appendingPathComponent("golden/telemetry-funnel.json"))
        for run in ["onboarding", "skipping", "no-usage-data"] {
            let telemetry = FixtureTelemetry()
            let o = OnboardingModel(statePath: nil)
            o.telemetry = telemetry
            for a in flow[run] as! [[String: Any]] {
                o.send(try appOnboardingActionFromJson(json: text(a)))
            }
            o.finish()
            let want = (golden["frames"] as! [[String: Any]])
                .filter { ($0["at"] as? String) == run }
                .flatMap { $0["signals"] as! [Any] }
            let got = telemetry.recorded.map(Self.signalFrame)
            #expect(canonical(got) == canonical(want), "\(run):\n want \(canonical(want))\n  got \(canonical(got))")
        }
    }

    // MARK: - Harness

    func check(_ name: String, _ run: ([String: Any]) throws -> [Any]) throws {
        let flow = try json(Self.dir.appendingPathComponent("\(name).json"))
        let golden = try json(Self.dir.appendingPathComponent("golden/\(name).json"))
        let want = (golden["frames"] as? [Any]) ?? []
        let got = try run(flow)
        #expect(got.count == want.count, "\(name): frame count \(got.count) vs \(want.count)")
        for (i, (g, w)) in zip(got, want).enumerated() {
            let gs = canonical(g), ws = canonical(w)
            #expect(gs == ws, "\(name) frame \(i):\n want \(ws)\n  got \(gs)")
            if gs != ws { break }
        }
    }

    func json(_ url: URL) throws -> [String: Any] {
        try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    }

    func text(_ v: Any?) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: v ?? NSNull(), options: [.fragmentsAllowed])
        return String(decoding: data, as: UTF8.self)
    }

    func canonical(_ v: Any) -> String {
        let data = try! JSONSerialization.data(withJSONObject: v, options: [.sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes])
        return String(decoding: data, as: UTF8.self)
    }

    func opt(_ s: String?) -> Any { s ?? NSNull() }

    // MARK: - create-space

    func wizardAction(_ a: [String: Any]) -> AppWizardAction {
        switch a["type"] as! String {
        case "choose-os": return .chooseOs(os: os(a["os"] as! String))
        case "set-placement": return .setPlacement(placement: location(a["placement"] as! String))
        case "sync-default": return .syncDefault(location: location(a["location"] as! String))
        case "choose-cloud": return .chooseCloud(cloud: a["cloud"] as! String)
        case "choose-placement": return .choosePlacement(on: a["on"] as! String)
        case "next": return .next
        case "back": return .back
        case "toggle-advanced": return .toggleAdvanced
        case "choose-kind": return .chooseKind(kind: (a["kind"] as! String) == "vm" ? .vm : .container)
        case "set-runtime": return .setRuntime(runtime: runtime(a["runtime"] as! String))
        case "set-cpus": return .setCpus(cpus: UInt32(a["cpus"] as! Int))
        case "set-memory": return .setMemory(memoryGb: UInt32(a["memoryGb"] as! Int))
        case "set-disk": return .setDisk(diskGb: UInt32(a["diskGb"] as! Int))
        case "reset-disk": return .resetDisk
        case "set-gpu": return .setGpu(on: a["on"] as! Bool)
        case "choose-image": return .chooseImage(imageRef: a["ref"] as! String)
        case "set-name": return .setName(name: a["name"] as! String)
        case "set-open-when-ready": return .setOpenWhenReady(on: a["on"] as! Bool)
        case "set-image-text": return .setImageText(text: a["text"] as! String)
        case "open-image-suggestions": return .openImageSuggestions
        case "move-image-suggestion": return .moveImageSuggestion(delta: Int32(a["delta"] as! Int))
        case "pick-image-suggestion": return .pickImageSuggestion
        case "dismiss-image-suggestions": return .dismissImageSuggestions
        case "show-address": return .showAddress
        case "hide-address": return .hideAddress
        case "set-address": return .setAddress(url: a["url"] as! String)
        case "set-token": return .setToken(token: a["token"] as! String)
        case "set-address-name": return .setAddressName(name: a["name"] as! String)
        case let other: fatalError("unmapped wizard action \(other)")
        }
    }

    func location(_ w: String) -> AppLocation {
        switch w {
        case "cloud": .cloud
        case "yours": .yours
        case "host": .host
        default: .local
        }
    }

    func wizardFrame(_ v: AppWizardView) -> [String: Any] {
        [
            "step": Int(v.step),
            "canContinue": v.canContinue,
            "primaryLabel": v.primaryLabel,
            "image": v.image.imageRef,
            "placement": word(v.plan.placement),
            "runtime": word(v.runtime),
            "runtimes": v.runtimes.map(\.value),
            "kinds": v.kindTiles.filter(\.enabled).map(\.id),
            "placements": v.placements.filter(\.enabled).map(\.id),
            "placementError": opt(v.placementError),
            "nameInvalid": v.nameInvalid,
            "cpusText": v.cpusText,
            "memoryText": v.memoryText,
            "ranges": "cpus \(v.minCpus)-\(v.maxCpus), memory \(v.minMemoryGb)-\(v.maxMemoryGb)",
            "disk": v.diskEditable
                ? "\(v.diskText) (\(v.minDiskGb)-\(v.maxDiskGb))\(v.diskResetLabel.map { " [\($0)]" } ?? "") ~\(v.diskNote ?? "") ?\(v.diskHelp ?? "")"
                : NSNull(),
            "resourceFacts": v.resourceFacts.map { f in
                "\(f.label): \(f.value)" + (f.symbol.map { " [\($0) ?\(f.help ?? "")]" } ?? "")
            },
            "resourcesError": opt(v.resourcesError),
            // `Label [x] -> url` (checked), `Label [ ] !reason` (disabled), or null.
            "gpu": v.gpu.map { g in
                "\(g.label) [\(g.on ? "x" : " ")]" + (g.reason.map { " !\($0)" } ?? "")
                    + (g.learnMoreUrl.map { " \(g.learnMoreLabel) -> \($0)" } ?? "")
            } ?? NSNull(),
            "price": opt(v.price),
            "labels": "\(v.labels.cancel) | \(v.labels.back) | \(v.labels.advanced)",
            "fields": v.fields.map(fieldLine),
            "imageField": [
                "text": v.imageField.text,
                "open": v.imageField.open,
                "custom": v.imageField.custom,
                "error": opt(v.imageField.error),
                "rows": v.imageField.open ? v.imageField.groups.flatMap { g in
                    g.rows.map { "\(g.label): \($0.imageRef) (\($0.label))\($0.selected ? " selected" : "")\($0.highlighted ? " highlighted" : "")" }
                } : [],
            ] as [String: Any],
            "address": [
                "canSubmit": v.address.canSubmit,
                "submitLabel": v.address.submitLabel,
                "error": opt(v.address.error),
                "submit": v.address.submit.map { "\($0.url) token=\($0.token ?? "") name=\($0.name ?? "")" } ?? NSNull(),
            ] as [String: Any],
        ]
    }

    func storage(_ s: [String: Any]) -> AppLocalStorage {
        func volume(_ v: Any?) -> AppStorageVolume? {
            guard let v = v as? [String: Any] else { return nil }
            return AppStorageVolume(availableBytes: (v["availableBytes"] as! NSNumber).uint64Value,
                                    totalBytes: (v["totalBytes"] as! NSNumber).uint64Value,
                                    name: v["name"] as! String)
        }
        return AppLocalStorage(reserveBytes: (s["reserveBytes"] as! NSNumber).uint64Value,
                               lume: volume(s["lume"]), qemu: volume(s["qemu"]),
                               container: volume(s["container"]), pulled: s["pulled"] as! [String])
    }

    /// `id: Label [placeholder] !error (advanced)`, as `parity.rs` writes it.
    func fieldLine(_ f: AppWizardField) -> String {
        var out = "\(f.id): \(f.label)"
        if let p = f.placeholder { out += " [\(p)]" }
        if let e = f.error { out += " !\(e)" }
        if f.advanced { out += " (advanced)" }
        return out
    }

    func facts(_ f: [AppFact]) -> [String] {
        f.map { fact in
            let copy = fact.copy.map { c in
                " [copy \(c.text) \(c.symbol) \(c.help) / \(c.doneSymbol) \(c.doneHelp) \(c.confirmMs)ms]"
            } ?? ""
            let help = fact.help.flatMap { $0 == fact.value ? nil : " ?\($0.replacingOccurrences(of: "\n", with: " | "))" } ?? ""
            let warn = fact.warning.map { " [warn \($0.symbol) \($0.help)]" } ?? ""
            return "\(fact.label): \(fact.value)\(help)\(warn)\(copy)"
        }
    }

    func rosterFrame(_ s: AppRosterState) -> [String: Any] {
        [
            "mode": word(s.mode),
            "ids": s.spaces.map(\.id),
            "names": s.spaces.map(\.name),
            "selectedId": s.selectedId,
            "focusIndex": Int(s.focusIndex),
            "notice": opt(s.notice?.text),
        ]
    }

    func sidebarFrame(_ v: AppSidebarView) -> [String: Any] {
        [
            "thisMachine": opt(v.thisMachine?.name),
            "sections": v.sections.map { sec in
                "\(sec.title): " + sec.rows.map { "\($0.name)\($0.dim ? " (dim)" : "")\($0.selected ? " (selected)" : "")" }
                    .joined(separator: ", ")
            },
            "selectedId": opt(v.selectedId),
            "emptyText": opt(v.emptyText),
        ]
    }

    func runCreateSpace(_ f: [String: Any]) throws -> [Any] {
        let env = try appWizardEnvFromJson(json: text(f["env"]))
        var frames: [Any] = []
        var state = appWizardInitial(env: env)
        frames.append(["wizard": "initial", "frame": wizardFrame(appWizardView(state: state, env: env))])
        for a in f["wizard"] as! [[String: Any]] {
            state = appWizardReduce(state: state, action: wizardAction(a), env: env)
            frames.append(["wizard": a["type"]!, "frame": wizardFrame(appWizardView(state: state, env: env))])
        }
        let view = appWizardView(state: state, env: env)
        let args = appWizardCreateArgs(plan: view.plan)
        var create: [String: Any] = [
            "image": args.image, "on": args.on, "kind": args.kind == .vm ? "vm" : "container",
            "runtime": word(args.runtime), "spacesd": args.spacesd,
        ]
        if let n = args.name { create["name"] = n }
        if let c = args.cpus { create["cpus"] = Int(c) }
        if let m = args.memoryMb { create["memoryMb"] = Int(m) }
        if let d = args.diskGb { create["diskGb"] = Int(d) }
        if let g = args.gpu { create["gpu"] = g }
        frames.append(["plan": [
            "createSpace": create, "summary": facts(view.summary), "openDesktop": view.plan.openDesktop,
            "creating": appWizardCreatingText(plan: view.plan),
            "failed": appWizardCreateFailedText(error: f["createError"] as! String),
        ] as [String: Any]])

        let rows = try appSpaceRowsFromJson(json: text(f["rows"]))
        let spaces = appRowsToSpaces(rows: rows, nowMs: Int64(f["now"] as! Int))
        var roster = appRosterInitial(spaces: [])
        roster = appRosterReduce(state: roster, action: .syncSpaces(spaces: spaces))
        frames.append(["roster": "sync-spaces", "frame": rosterFrame(roster)])
        for a in f["roster"] as! [[String: Any]] {
            let action: AppRosterAction
            switch a["type"] as! String {
            case "select": action = .select(id: a["id"] as! String, now: Int64(a["now"] as! Int))
            case "expand": action = .expand
            case "collapse": action = .collapse
            case "focus-move": action = .focusMove(delta: Int32(a["delta"] as! Int))
            case let other: fatalError("unmapped roster action \(other)")
            }
            roster = appRosterReduce(state: roster, action: action)
            frames.append(["roster": a["type"]!, "frame": rosterFrame(roster)])
        }
        frames.append(["sidebar": "", "frame": sidebarFrame(appSidebar(spaces: roster.spaces, query: "", selectedId: roster.selectedId))])
        let q = f["query"] as! String
        frames.append(["sidebar": q, "frame": sidebarFrame(appSidebar(spaces: roster.spaces, query: q, selectedId: roster.selectedId))])
        for s in roster.spaces {
            let d = appSpaceDetail(space: s)
            frames.append(["detail": s.id, "frame": [
                "title": d.title, "facts": facts(d.facts), "canStream": d.canStream,
                "showSections": d.showSections, "deleteLabel": d.deleteLabel, "previewText": d.previewText,
            ] as [String: Any]])
        }
        return frames
    }

    // MARK: - provisioning

    func createAction(_ a: [String: Any]) -> AppCreateAction {
        let id = a["id"] as? String ?? ""
        switch a["type"] as! String {
        case "start":
            let kind = (a["kind"] as? String).flatMap { AppSpaceKind(word: $0) }
            return .start(id: id, name: a["name"] as! String, os: os(a["os"] as! String),
                          provider: (a["provider"] as! String) == "cloud" ? .cloud : .local,
                          now: Int64(a["now"] as! Int), image: a["image"] as? String, kind: kind,
                          hostArch: a["hostArch"] as? String, gpu: a["gpu"] as? Bool ?? false)
        case "progress":
            return .progress(id: id, phase: a["phase"] as! String,
                             fraction: (a["fraction"] as? NSNumber)?.doubleValue,
                             now: (a["now"] as? NSNumber)?.int64Value,
                             bytesDone: (a["bytesDone"] as? NSNumber)?.uint64Value,
                             bytesTotal: (a["bytesTotal"] as? NSNumber)?.uint64Value,
                             bytesPerSecond: (a["bytesPerSecond"] as? NSNumber)?.doubleValue)
        case "tick": return .tick(now: Int64(a["now"] as! Int))
        case "finish": return .finish(id: id, spaceId: a["spaceId"] as! String)
        case "fail": return .fail(id: id, error: a["error"] as! String)
        case "dismiss": return .dismiss(id: id)
        case "delete-start": return .deleteStart(id: id, now: Int64(a["now"] as! Int))
        case "delete-fail": return .deleteFail(id: id)
        case "delete-done": return .deleteDone(id: id)
        case "cancel-start": return .cancelStart(id: id)
        case "cancel-done": return .cancelDone(id: id)
        case "cancel-fail": return .cancelFail(id: id, error: a["error"] as! String)
        case "power-start": return .powerStart(id: id, on: a["on"] as! Bool, now: Int64(a["now"] as! Int))
        case "power-done": return .powerDone(id: id)
        case "power-fail": return .powerFail(id: id, error: a["error"] as! String)
        case let other: fatalError("unmapped create action \(other)")
        }
    }

    func provisioningFrame(_ registry: [AppSpace], _ creates: AppCreatesState,
                           _ roster: inout AppRosterState) -> [String: Any] {
        roster = appRosterReduce(state: roster,
                                 action: .syncSpaces(spaces: appCreatesCompose(spaces: registry, state: creates)))
        let spaces = roster.spaces
        let sb = appSidebar(spaces: spaces, query: "", selectedId: roster.selectedId)
        let rows = sb.sections.flatMap(\.rows).map { r in
            "\(r.osIcon) \(r.name) [\(r.statusText)] \(r.progress.map(String.init) ?? "-") \(r.trailing ?? "-")\(powerText(r.power))"
        }
        let closed = appNotchView(state: appNotchInitial(), spaces: spaces)
        // The open panel's tiles (the Rust flow reads `notch.tiles`: the same list).
        let open = appNotchReduce(state: appNotchInitial(), event: .click).state
        let tiles = appNotchView(state: open, spaces: spaces).tiles.map { t in
            "\(t.symbol) \(t.label) \(t.progress.map(String.init) ?? "-") \(t.progressLabel ?? "-")"
        }
        let details: [Any] = spaces.map { s in
            let d = appSpaceDetail(space: s)
            var detail: [String: Any] = [
                "id": s.id, "facts": facts(d.facts), "previewText": d.previewText,
                "progress": d.progress.map { Int($0) as Any } ?? NSNull(), "canStream": d.canStream,
                "progressText": opt(d.progressText),
                "actions": d.actions.map { "\($0.label)\($0.enabled ? "" : " (off)")" },
                "creditNotice": d.creditNotice.map { "\($0.text) [\($0.button) -> \($0.url)]" } as Any? ?? NSNull(),
            ]
            // Why turning it off or on failed, when it did.
            if let e = d.powerError { detail["powerError"] = e }
            return detail
        }
        return [
            "ids": spaces.map(\.id),
            "rows": rows,
            "activity": [
                "kind": closed.activity.map { activityWord($0.kind) as Any } ?? NSNull(),
                "label": opt(closed.activity?.label),
                "permille": closed.activity?.permille.map { Int($0) as Any } ?? NSNull(),
            ] as [String: Any],
            "tiles": tiles,
            "details": details,
            "pending": appCreatesIsPending(id: "pending:1"),
        ]
    }

    func runProvisioning(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        let registry = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(f["rows"])), nowMs: now)
        var roster = appRosterInitial(spaces: [])
        var creates = AppCreatesState(pending: [], deleting: [], powering: [])
        var frames: [Any] = [["creates": "initial", "frame": provisioningFrame(registry, creates, &roster)]]
        for a in f["creates"] as! [[String: Any]] {
            creates = appCreatesReduce(state: creates, action: createAction(a))
            let label = ["\(a["type"]!)", (a["id"] as? String) ?? "", (a["phase"] as? String) ?? ""]
                .joined(separator: " ").trimmingCharacters(in: .whitespaces)
            frames.append(["creates": label, "frame": provisioningFrame(registry, creates, &roster)])
        }
        let refreshed = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(f["refreshed"])), nowMs: now)
        frames.append(["creates": "refreshed", "frame": provisioningFrame(refreshed, creates, &roster)])
        return frames
    }

    // MARK: - delete-space

    func runDeleteSpace(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        var registry = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(f["rows"])), nowMs: now)
        var roster = appRosterInitial(spaces: [])
        var creates = AppCreatesState(pending: [], deleting: [], powering: [])
        var frames: [Any] = []
        for step in f["steps"] as! [[String: Any]] {
            let action = step["action"] as? [String: Any]
            if let action { creates = appCreatesReduce(state: creates, action: createAction(action)) }
            if let rows = step["rows"] {
                registry = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(rows)), nowMs: now)
                creates = appCreatesSettle(state: creates, spaces: registry)
            }
            let id = action?["id"] as? String
            let banner: Any = (step["error"] as? String).map { e in
                appDeleteFailedText(name: registry.first { $0.id == id }?.name ?? "", error: e)
            } ?? NSNull()
            frames.append([
                "step": step["step"] as! String,
                "frame": provisioningFrame(registry, creates, &roster),
                "deleting": creates.deleting.map { "\($0.id)\($0.done ? " done" : "")" },
                "isDeleting": id.map { appCreatesIsDeleting(state: creates, id: $0) as Any } ?? NSNull(),
                "banner": banner,
            ] as [String: Any])
        }
        return frames
    }

    /// A row's power button: ` <power Suspend>`, ` <power Resuming… on busy>`.
    func powerText(_ b: AppPowerButton?) -> String {
        guard let b else { return "" }
        return " <\(b.symbol) \(b.help)\(b.turnOn ? " on" : "")\(b.busy ? " busy" : "")>"
    }

    // MARK: - space-power

    func runSpacePower(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        var registry = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(f["rows"])), nowMs: now)
        var roster = appRosterInitial(spaces: [])
        var creates = AppCreatesState(pending: [], deleting: [], powering: [])
        var frames: [Any] = []
        for step in f["steps"] as! [[String: Any]] {
            if let action = step["action"] as? [String: Any] {
                creates = appCreatesReduce(state: creates, action: createAction(action))
            }
            if let rows = step["rows"] {
                registry = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(rows)), nowMs: now)
                creates = appCreatesSettle(state: creates, spaces: registry)
            }
            frames.append([
                "step": step["step"] as! String,
                "frame": provisioningFrame(registry, creates, &roster),
                "powering": creates.powering.map {
                    "\($0.id) \($0.on ? "on" : "off")\($0.done ? " done" : "")\($0.error.map { " !\($0)" } ?? "")"
                },
            ] as [String: Any])
        }
        return frames
    }

    // MARK: - teleport-review

    func pickerFrame(_ s: AppPickerState) -> [String: Any] {
        let review: Any = appPickerReview(state: s).map { r -> [String: Any] in
            [
                "title": r.title,
                "items": r.items.map { "\(word($0.kind)):\($0.label)\($0.sensitive ? " (secret)" : "")" },
                "needsAcknowledgement": r.needsAcknowledgement,
                "needsRelayPlaintextAcknowledgement": r.needsRelayPlaintextAcknowledgement,
                "canConfirm": r.canConfirm,
                "leavesText": opt(r.leavesText),
                "warnings": r.warnings,
            ]
        } ?? NSNull()
        return [
            "step": word(s.step),
            "selectedId": opt(s.selectedId),
            "entry": opt(s.entry?.id),
            "move": opt(s.moves.map(word)),
            "canPlan": appPickerCanPlan(state: s),
            "sections": appPickerSections(state: s).map { "\($0.title): " + $0.entries.map(\.name).joined(separator: ", ") },
            "review": review,
            "permille": Int((appPickerProgress(state: s) * 1000).rounded()),
            "error": opt(s.error),
            "report": opt(s.report?.appId),
            "sensitive": appPickerSensitiveOptions(state: s).map {
                "\(word($0.group)):\($0.label):\($0.detail):\($0.checked ? "on" : "off")"
            },
            "planSensitive": appPickerPlanSensitive(state: s).map(word),
        ]
    }

    func word(_ g: AppSensitiveGroup) -> String {
        switch g {
        case .signIns: "sign_ins"
        case .passwords: "passwords"
        case .history: "history"
        }
    }

    func group(_ w: String) -> AppSensitiveGroup {
        switch w {
        case "sign_ins": .signIns
        case "passwords": .passwords
        case "history": .history
        default: fatalError("unknown group \(w)")
        }
    }

    func runTeleport(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        var s = appPickerInitial(spaceName: f["spaceName"] as! String)
        frames.append(["picker": "initial", "frame": pickerFrame(s)])
        let entries = try appCatalogEntriesFromJson(json: text(f["entries"]))
        let plan = try appTeleportPlanFromJson(json: text(f["plan"]))
        for e in f["picker"] as! [[String: Any]] {
            let event: AppPickerEvent
            switch e["type"] as! String {
            case "choose": event = .choose(id: e["id"] as? String)
            case "loaded": event = .loaded(entries: entries)
            case "query": event = .query(query: e["query"] as! String)
            case "select": event = .select(id: e["id"] as! String)
            case "move": event = .move(moves: move(e["move"] as! String))
            case "plan": event = .plan
            case "planned": event = .planned(plan: plan)
            case "confirm": event = .confirm
            case "acknowledge": event = .acknowledge(value: e["value"] as! Bool)
            case "acknowledge-relay-plaintext": event = .acknowledgeRelayPlaintext(value: e["value"] as! Bool)
            case "sensitive": event = .sensitive(group: group(e["group"] as! String), value: e["value"] as! Bool)
            case "back": event = .back
            case "progress":
                let p = e["event"] as! [String: Any]
                event = .progress(event: AppTeleportRunEvent(
                    step: UInt32(p["step"] as! Int), steps: UInt32(p["steps"] as! Int), kind: p["kind"] as! String,
                    phase: runPhase(p["phase"] as! String), detail: p["detail"] as! String,
                    doneBytes: UInt64(p["doneBytes"] as! Int), totalBytes: UInt64(p["totalBytes"] as! Int)))
            case "finished":
                let r = e["report"] as! [String: Any]
                event = .finished(report: AppTeleportRunReport(
                    appId: r["appId"] as! String, installed: r["installed"] as! [String], sent: r["sent"] as! [String],
                    imported: r["imported"] as! [String], skipped: r["skipped"] as! [String], launched: r["launched"] as! Bool))
            case let other: fatalError("unmapped picker event \(other)")
            }
            s = appPickerReduce(state: s, event: event)
            frames.append(["picker": e["type"]!, "frame": pickerFrame(s)])
        }
        let consent = appPickerConsent(state: s)
        frames.append(["consent": ["approved": consent.approved, "acknowledgeSensitive": consent.acknowledgeSensitive, "acknowledgeRelayPlaintext": consent.acknowledgeRelayPlaintext, "saveToKeyvault": consent.saveToKeyvault]])

        let spaces = try appSpacesFromJson(json: text(f["spaces"]))
        frames += notchFrames(f["notch"] as! [[String: Any]], spaces: spaces)
        return frames
    }

    func notchFrames(_ events: [[String: Any]], spaces: [AppSpace]) -> [Any] {
        var frames: [Any] = []
        var n = appNotchInitial()
        for e in events {
            let (event, label) = notchEvent(e)
            let t = appNotchReduce(state: n, event: event)
            n = t.state
            let v = appNotchView(state: n, spaces: spaces)
            frames.append(["notch": label, "frame": [
                "open": n.open,
                "phase": word(v.phase),
                "prompt": opt(v.prompt),
                "tiles": v.tiles.map { "\($0.name) [\($0.symbol)]\($0.dropTarget ? "" : " (no drop)")\($0.targeted ? " (targeted)" : "")" },
                "dropMode": v.dropMode,
                "label": v.label,
                "tab": v.showTab ? [v.tab.count, v.tab.word] : NSNull(),
                "header": v.header.map { h in [
                    "query": h.query,
                    "matches": h.matchCount.map { $0 as Any } ?? NSNull(),
                    "buttons": h.buttons.map { "\($0.label) [\($0.symbol)]" },
                ] as [String: Any] } ?? NSNull(),
                "empty": opt(v.empty),
                "hidden": v.hidden,
                "activity": v.activity.map { a in
                    "\(activityWord(a.kind)): \(a.label)\(a.symbol.map { " [\($0)]" } ?? "")\(a.permille.map { " \($0)/1000" } ?? "")" as Any
                } ?? NSNull(),
                "cue": v.hoverCue,
                "permission": opt(v.permission?.text),
                "effects": t.effects.map(effect),
            ] as [String: Any]])
        }
        return frames
    }

    // MARK: - notch

    func screenFacts(_ d: [String: Any]) -> AppScreenFacts {
        func rect(_ v: Any?) -> AppLogicalRect {
            let r = v as! [String: Any]
            func n(_ k: String) -> Double { (r[k] as! NSNumber).doubleValue }
            return AppLogicalRect(x: n("x"), y: n("y"), width: n("width"), height: n("height"))
        }
        return AppScreenFacts(frame: rect(d["frame"]), visibleFrame: rect(d["visibleFrame"]),
                              safeAreaTop: (d["safeAreaTop"] as! NSNumber).doubleValue,
                              auxLeftWidth: (d["auxLeftWidth"] as? NSNumber)?.doubleValue,
                              auxRightWidth: (d["auxRightWidth"] as? NSNumber)?.doubleValue)
    }

    func runNotch(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        for screen in f["screens"] as! [[String: Any]] {
            let facts = screenFacts(screen["facts"] as! [String: Any])
            for prompt in [false, true] {
                let l = appNotchLayout(screen: facts, prompt: prompt)
                func r(_ r: AppLogicalRect) -> [Double] { [r.x, r.y, r.width, r.height] }
                frames.append([
                    "layout": "\(screen["name"] as! String)\(prompt ? " +prompt" : "")",
                    "frame": [
                        "hasNotch": l.hasNotch, "notchStyle": l.notchStyle,
                        "notch": r(l.notch), "closed": r(l.closedFrame), "open": r(l.openFrame),
                        "prompt": r(l.promptFrame), "tab": r(l.tabFrame), "stage": r(l.stageFrame),
                    ] as [String: Any],
                ])
            }
        }
        let m = appNotchMotion()
        frames.append(["motion": [
            "hoverDwellMs": m.hoverDwellMs, "closeDelayMs": m.closeDelayMs,
            "openResponse": m.openResponse, "openDamping": m.openDamping,
            "closeResponse": m.closeResponse, "closeDamping": m.closeDamping,
            "reducedDuration": m.reducedDuration, "hoverResponse": m.hoverResponse,
            "hoverDamping": m.hoverDamping, "hoverScale": m.hoverScale, "hoverScaleY": m.hoverScaleY,
            "contentDelayMs": m.contentDelayMs, "contentIn": m.contentIn,
            "contentOut": m.contentOut, "contentScale": m.contentScale,
        ] as [String: Any]])
        let spaces = try appSpacesFromJson(json: text(f["spaces"]))
        return frames + notchFrames(f["events"] as! [[String: Any]], spaces: spaces)
    }

    func dragRect(_ v: Any?) -> AppLogicalRect? {
        guard let r = v as? [String: Any] else { return nil }
        func n(_ k: String) -> Double { (r[k] as! NSNumber).doubleValue }
        return AppLogicalRect(x: n("x"), y: n("y"), width: n("width"), height: n("height"))
    }

    func dragTriggerEvent(_ e: [String: Any]) -> AppDragTriggerEvent {
        func n(_ k: String) -> Double { (e[k] as! NSNumber).doubleValue }
        func t() -> UInt64 { (e["tMs"] as! NSNumber).uint64Value }
        switch e["type"] as! String {
        case "start":
            return .start(windowId: (e["windowId"] as? NSNumber)?.uint32Value, appName: e["appName"] as? String,
                          x: n("x"), y: n("y"), tMs: t(),
                          startFrame: dragRect(e["startFrame"]), frame: dragRect(e["frame"]))
        case "cursor": return .cursor(x: n("x"), y: n("y"), tMs: t())
        case "frame": return .frame(frame: dragRect(e["frame"])!)
        case "tick": return .tick(tMs: t())
        case "end": return .end(x: n("x"), y: n("y"), tMs: t())
        default: return .cancel
        }
    }

    func runNotchDragTrigger(_ f: [String: Any]) throws -> [Any] {
        let named = f["screens"] as! [[String: Any]]
        let screens = named.map { screenFacts($0["facts"] as! [String: Any]) }
        let displays = appDragDisplays(screens: screens)
        let portals = appDragPortalDisplays(screens: screens)
        func r(_ r: AppLogicalRect) -> [Double] { [r.x, r.y, r.width, r.height] }
        var frames: [Any] = []
        for (i, s) in named.enumerated() {
            let d = displays[i]
            let portal = portals[i].expanded
            frames.append([
                "display": s["name"] as! String, "frame": r(d.frame), "notch": r(d.notch),
                "prompt": r(d.prompt), "expanded": r(d.expanded), "portalExpanded": r(portal),
            ] as [String: Any])
        }
        func word(_ k: AppDragKind) -> String {
            switch k { case .pending: "pending"; case .move: "move"; case .resize: "resize" }
        }
        func word(_ p: AppTriggerPhase) -> String {
            switch p { case .hidden: "hidden"; case .prompt: "prompt"; case .expanded: "expanded" }
        }
        func word(_ o: AppDragOverlayEvent) -> String {
            switch o {
            case .start: "start"
            case .enterNotch: "enter-notch"
            case .leaveNotch: "leave-notch"
            case .over: "over"
            case .out: "out"
            case .ghostReady: "ghost-ready"
            case .drop: "drop"
            case .cancel: "cancel"
            }
        }
        var state = appDragTriggerInitial()
        for e in f["events"] as! [[String: Any]] {
            let t = appDragTriggerApply(state: state, event: dragTriggerEvent(e), displays: displays)
            state = t.state
            frames.append([
                "event": e["type"] as! String, "phase": word(state.phase), "kind": word(state.kind),
                "display": state.display.map { Int($0) as Any } ?? NSNull(),
                "overlay": t.overlay.map(word),
                "tickAt": t.tickAtMs.map { Int($0) as Any } ?? NSNull(),
            ] as [String: Any])
        }
        return frames
    }

    func notchEvent(_ e: [String: Any]) -> (AppNotchEvent, String) {
        switch e["type"] as! String {
        case "hover-enter": return (.hoverEnter, "hover-enter")
        case "hover-exit": return (.hoverExit, "hover-exit")
        case "dwell-elapsed": return (.dwellElapsed, "dwell-elapsed")
        case "close-elapsed": return (.closeElapsed, "close-elapsed")
        case "click": return (.click, "click")
        case "dismiss": return (.dismiss, "dismiss")
        case "drag-permission":
            let granted = e["granted"] as! Bool
            return (.dragPermission(granted: granted), "drag-permission \(granted)")
        case "drop-targeted":
            let targeted = e["targeted"] as! Bool
            return (.dropTargeted(targeted: targeted), "drop-targeted \(targeted)")
        case "drag":
            let d = e["event"] as! [String: Any]
            let t = d["type"] as! String
            let ev: AppDragOverlayEvent
            switch t {
            case "start": ev = .start(windowId: (d["windowId"] as? Int).map(UInt32.init), appName: d["appName"] as? String)
            case "ghost-ready": ev = .ghostReady(ghost: d["ghost"] as? String)
            case "enter-notch": ev = .enterNotch
            case "leave-notch": ev = .leaveNotch
            case "over": ev = .over(spaceId: d["spaceId"] as! String)
            case "out": ev = .out
            case "drop": ev = .drop(spaceId: d["spaceId"] as? String)
            default: ev = .cancel
            }
            return (.drag(event: ev), "drag \(t)")
        case "search":
            let q = e["query"] as! String
            return (.search(query: q), "search \"\(q)\"")
        case "escape": return (.escape, "escape")
        case "visibility":
            let shown = e["shown"] as! Bool
            return (.visibility(shown: shown), "visibility shown=\(shown)")
        case "activity":
            let hotspot = e["hotspot"] as! Bool
            let t = e["transfer"] as? [String: Any]
            let transfer = t.map { AppNotchTransfer(sent: ($0["sent"] as? NSNumber)?.uint64Value,
                                                    total: ($0["total"] as? NSNumber)?.uint64Value) }
            let word = t.map { d in
                "{" + ["sent", "total"].compactMap { k in (d[k] as? NSNumber).map { "\"\(k)\":\($0)" } }
                    .joined(separator: ",") + "}"
            } ?? "null"
            return (.activity(hotspot: hotspot, transfer: transfer), "activity hotspot=\(hotspot) transfer=\(word)")
        case let other: fatalError("unmapped notch event \(other)")
        }
    }

    func activityWord(_ k: AppNotchActivityKind) -> String {
        switch k {
        case .transfer: return "transfer"
        case .remoteAccess: return "remote-access"
        case .hotspot: return "hotspot"
        case .provisioning: return "provisioning"
        case .deleting: return "deleting"
        case .keyvault: return "keyvault"
        }
    }

    func effect(_ e: AppNotchEffect) -> String {
        switch e {
        case .startDwell(let ms): return "start-dwell:\(ms)"
        case .startClose(let ms): return "start-close:\(ms)"
        case .cancelTimers: return "cancel-timers"
        case .drag(let d):
            switch d {
            case .capture(let id): return "capture:\(id.map { String($0) } ?? "null")"
            case .commit(let space): return "commit:\(space)"
            }
        }
    }

    // MARK: - keyvault-approve-deny

    func kvSidebarFrame(_ v: KvSidebar) -> [String: Any] {
        [
            "categories": v.categories.map { c in c.count.map { "\(c.title):\($0)" } ?? c.title },
            "apps": v.apps.map { "\($0.title) (\($0.items))\($0.waiting ? " waiting" : "")" },
        ]
    }

    func kvPageFrame(_ p: KvPage) -> [String: Any] {
        [
            "ready": p.ready, "killSwitchVisible": p.killSwitchVisible, "disabled": p.disabled,
            "disabledBanner": opt(p.disabledBanner), "logStatus": opt(p.logStatus),
            "revokeAll": p.revokeAll, "pendingCount": Int(p.pendingCount), "protection": facts(p.protection),
        ]
    }

    func kvListFrame(_ v: KvListView) -> [String: Any] {
        [
            "title": v.title,
            "vault": v.vault,
            "pending": v.pending.map { "\($0.caller) (\($0.badge.text)) \($0.summary) for \($0.wants)" },
            "access": v.access.map { "\($0.text) / \($0.detail) [\($0.actionLabel)]" },
            "recent": v.recent.map { "\($0.decision.verb) \($0.decision.what) (\(word($0.decision.tone))) \($0.age)" },
            "empty": opt(v.emptyText),
        ]
    }

    func lockWord(_ l: KvLock) -> String {
        switch l {
        case .locked: return "locked"
        case .unlocked: return "unlocked"
        case .mixed: return "mixed"
        }
    }

    func triMark(_ t: KvTri) -> String {
        switch t {
        case .on: return "[x]"
        case .mixed: return "[-]"
        case .off: return "[ ]"
        }
    }

    func vaultRowLine(_ r: KvVaultRow) -> String {
        "\(r.selected ? "[x]" : "[ ]") \(r.kindLabel) \(r.title) | \(r.subtitle) | \(r.lockSymbol) | \(r.updated)"
            + (r.identityProvider ? " | always asks" : "")
    }

    func kvVaultFrame(_ v: KvVaultView) -> [String: Any] {
        var lines: [String] = []
        for a in v.apps {
            lines.append("\(triMark(a.selected)) \(a.name) \(a.providerId) | \(a.summary) | \(lockWord(a.lock)) | \(a.updated)")
            for s in a.sites {
                lines.append("  \(triMark(s.selected)) \(s.site) | \(s.counts) | \(lockWord(s.lock)) | \(s.updated)\(s.open ? " | open" : "")")
                for r in s.rows { lines.append("    " + vaultRowLine(r)) }
            }
            if let f = a.files {
                lines.append("  \(triMark(f.selected)) files (\(f.count)) | \(lockWord(f.lock))\(f.open ? " | open" : "")")
                for r in f.rows { lines.append("    " + vaultRowLine(r)) }
            }
        }
        let sel = v.selection
        return [
            "shown": "\(v.shown)/\(v.total)",
            "lines": lines,
            "empty": opt(v.emptyText),
            "selection": "\(sel.title) (unlock \(sel.unlockIds.count) / lock \(sel.lockIds.count) / always ask \(sel.alwaysAsk))",
            "canUnlock": sel.canUnlock,
            "canLock": sel.canLock,
        ]
    }

    func kvUnlockPromptFrame(_ p: KvUnlockPrompt?) -> Any {
        guard let p else { return NSNull() }
        return [p.title, p.message, p.subject, "\(p.deny) / \(p.allow) / \(p.neverAsk)"]
    }

    func kvDeleteConfirmFrame(_ c: KvDeleteConfirm) -> Any {
        [c.title, c.message, "\(c.confirm) / \(c.cancel)"]
    }

    func vaultAction(_ a: [String: Any]) -> KvVaultAction {
        switch a["type"] as! String {
        case "query": return .query(text: a["text"] as! String)
        case "toggle": return .toggle(id: a["id"] as! String)
        case "toggle-group": return .toggleGroup(key: a["key"] as! String)
        case "select-all": return .selectAll
        case "toggle-open": return .toggleOpen(key: a["key"] as! String)
        default: return .clear
        }
    }

    func vaultState(_ s: [String: Any]) -> KvVaultState {
        KvVaultState(query: s["query"] as! String, selected: s["selected"] as! [String],
                     expanded: s["expanded"] as! [String], app: s["app"] as? String)
    }

    func approvalFrame(_ v: KvApprovalView) -> [String: Any] {
        [
            "title": v.title, "badge": v.badge.text,
            "rows": v.rows.map { "\($0.selected ? "[x] " : "[ ] ")\($0.title) \($0.account)" },
            "canApprove": v.canApprove, "approveLabel": v.approveLabel,
            "blockedReason": opt(v.blockedReason), "claims": v.claims,
        ]
    }

    func command(_ c: KvCommand?) -> Any {
        switch c {
        case .approve(let id, let items)?: return ["type": "approve", "requestId": id, "items": items.map { $0 as Any } ?? NSNull()]
        case .deny(let id)?: return ["type": "deny", "requestId": id]
        case .setUnattended(let ids, let on)?: return ["type": "set-unattended", "itemIds": ids, "unattended": on]
        case .setLocked(let ids, let locked)?: return ["type": "set-locked", "itemIds": ids, "locked": locked]
        case nil: return NSNull()
        default: return "unmapped"
        }
    }

    func selection(_ s: [String: Any]) -> KvSelection {
        if (s["kind"] as! String) == "app" { return .app(key: s["key"] as! String) }
        let c: KvCategory = switch s["category"] as! String {
        case "waiting": .waiting
        case "access": .access
        case "recent": .recent
        default: .all
        }
        return .category(category: c)
    }

    func approvalAction(_ a: [String: Any]) -> KvApprovalAction {
        switch a["type"] as! String {
        case "toggle": return .toggle(key: a["key"] as! String)
        case "select-all": return .selectAll
        default: return .clear
        }
    }

    func runKeyvault(_ f: [String: Any]) throws -> [Any] {
        let o = try kvOverviewFromJson(json: text(f["overview"]))
        let now = Int64(f["now"] as! Int)
        var frames: [Any] = []
        frames.append(["sidebar": kvSidebarFrame(kvSidebar(overview: o, nowMs: now))])
        frames.append(["page": kvPageFrame(kvPage(overview: o, nowMs: now))])
        for sel in f["selections"] as! [[String: Any]] {
            frames.append(["list": sel, "frame": kvListFrame(kvList(overview: o, selection: selection(sel), nowMs: now))])
        }
        for a in f["approvals"] as! [[String: Any]] {
            let req = a["request"] as! String
            var st = kvApprovalOpen(requestId: req)
            frames.append(["approval": req, "action": "open", "frame": approvalFrame(kvApprovalView(overview: o, state: st))])
            for action in a["actions"] as! [[String: Any]] {
                st = kvApprovalReduce(overview: o, state: st, action: approvalAction(action))
                frames.append(["approval": req, "action": action, "frame": approvalFrame(kvApprovalView(overview: o, state: st))])
            }
            frames.append(["approval": req, "approveCommand": command(kvApprovalApproveCommand(overview: o, state: st))])
        }
        frames.append(["denyCommand": command(kvApprovalDenyCommand(state: KvApprovalState(requestId: f["deny"] as! String, selected: [])))])
        let vault = f["vault"] as! [String: Any]
        var vst = vaultState(vault["state"] as! [String: Any])
        frames.append(["vault": "open", "frame": kvVaultFrame(kvVaultView(overview: o, state: vst, nowMs: now))])
        for action in vault["actions"] as! [[String: Any]] {
            vst = kvVaultReduce(overview: o, state: vst, action: vaultAction(action))
            frames.append(["vault": action, "frame": kvVaultFrame(kvVaultView(overview: o, state: vst, nowMs: now))])
        }
        var one = vaultState(vault["state"] as! [String: Any])
        one.app = (vault["app"] as! String)
        frames.append(["vaultApp": vault["app"] as! String, "frame": kvVaultFrame(kvVaultView(overview: o, state: one, nowMs: now))])
        for u in vault["unlock"] as! [[String: Any]] {
            let p = kvUnlockPrompt(overview: o, count: UInt32(u["count"] as! Int), name: u["name"] as? String)
            frames.append(["unlockPrompt": u, "frame": kvUnlockPromptFrame(p)])
        }
        var skip = o
        skip.status?.skipUnlockPrompt = true
        frames.append(["unlockPromptSkipped": kvUnlockPromptFrame(kvUnlockPrompt(overview: skip, count: 1, name: nil))])
        for d in vault["delete"] as! [[String: Any]] {
            let c = kvDeleteConfirm(count: UInt32(d["count"] as! Int), liveCopies: UInt32(d["liveCopies"] as! Int))
            frames.append(["deleteConfirm": d, "frame": kvDeleteConfirmFrame(c)])
        }
        var off = o
        off.status?.disabled = true
        frames.append(["page": kvPageFrame(kvPage(overview: off, nowMs: now))])
        let d = f["disabled"] as! [String: Any]
        var st = kvApprovalOpen(requestId: d["request"] as! String)
        for action in d["actions"] as! [[String: Any]] {
            st = kvApprovalReduce(overview: off, state: st, action: approvalAction(action))
        }
        frames.append([
            "disabledApproval": approvalFrame(kvApprovalView(overview: off, state: st)),
            "approveCommand": command(kvApprovalApproveCommand(overview: off, state: st)),
        ])
        let on = kvPage(overview: o, nowMs: now)
        let offPage = kvPage(overview: off, nowMs: now)
        let l = on.labels
        frames.append([
            "labels": [
                "deny: \(l.deny)", "review: \(l.review)", "cancel: \(l.cancel)", "setUp: \(l.setUp)",
                "unlock: \(l.unlock)", "revokeAll: \(l.revokeAll)", "confirmNote: \(l.confirmNote)",
                "protectionTitle: \(l.protectionTitle)",
            ],
            "killSwitchHelp": [on.killSwitchHelp, offPage.killSwitchHelp],
            "badges": kvSidebar(overview: o, nowMs: now).categories.map { "\($0.title):\($0.badge.map { String($0) } ?? "-")" },
            "recoveryKey": kvRecoveryKeyText(key: "ABCD-EFGH"),
        ])
        return frames
    }

    // MARK: - keyvault-unlock

    func word(_ v: KvFormMode) -> String { v == .setup ? "setup" : "unlock" }
    func word(_ v: KvMethod) -> String { v == .touchId ? "touch-id" : "passphrase" }
    func word(_ v: KvStrength) -> String { "\(v)" }

    func kvFormFrame(_ f: KvCredentialForm?) -> Any {
        guard let f else { return NSNull() }
        return [
            "\(word(f.mode)) with \(word(f.method))",
            "help: \(f.help)",
            "passphrase: \(dash(f.passphraseLabel))",
            "confirm: \(dash(f.confirmLabel))",
            "submit: \(f.submitLabel)",
        ]
    }

    func runKeyvaultUnlock(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        var frames: [Any] = []
        for c in f["cases"] as! [[String: Any]] {
            let o = try kvOverviewFromJson(json: text(c["overview"]))
            let page = kvPage(overview: o, nowMs: now)
            let form = kvCredentialForm(overview: o)
            #expect(form == page.form, "\(c["name"]!): the page's form differs from credentialForm")
            frames.append([
                "case": c["name"]!, "title": opt(page.unavailableTitle),
                "canSetup": page.canSetup, "canUnlock": page.canUnlock, "form": kvFormFrame(form),
                "unlockFact": opt(page.protection.first { $0.label == "Unlock" }?.value),
            ])
        }
        for c in f["checks"] as! [[String: Any]] {
            let r = kvPassphraseCheck(mode: (c["mode"] as! String) == "setup" ? .setup : .unlock,
                                      passphrase: c["passphrase"] as! String, confirm: c["confirm"] as! String)
            frames.append([
                "check": c["label"]!,
                "result": "submit=\(r.canSubmit) strength=\(r.strength.map(word) ?? "-") hint=\(dash(r.hint))",
            ])
        }
        return frames
    }

    // MARK: - main-window (projections as parity.rs `frame::*`)

    func dash(_ s: String?) -> String { s ?? "-" }

    func sidebarRowLine(_ r: AppSidebarRow) -> String {
        "<\(r.osIcon)> \(r.name)\(r.place.map { " @ \($0)" } ?? "") | \(r.statusText) | \(r.detail)\(r.dim ? " (dim)" : "")\(r.selected ? " (selected)" : "")"
    }

    func windowSidebar(_ v: AppSidebarView) -> [String: Any] {
        [
            "thisMachine": v.thisMachine.map(sidebarRowLine) as Any? ?? NSNull(),
            "sections": v.sections.flatMap { ["# \($0.title)"] + $0.rows.map(sidebarRowLine) },
            "selectedId": opt(v.selectedId),
            "emptyText": opt(v.emptyText),
        ]
    }

    func actionWord(_ id: AppDetailActionId) -> String {
        switch id {
        case .teleport: "teleport"
        case .pip: "pip"
        case .share: "share"
        case .power: "power"
        case .delete: "delete"
        case .open: "open"
        case .cancel: "cancel"
        }
    }

    func confirmLine(_ c: AppDeleteConfirm) -> String {
        let disabled = c.confirmEnabled ? "" : " (disabled: \(c.disabledReason ?? ""))"
        let remove = c.removeLabel.map { " / \($0)" } ?? ""
        return "\(c.title) | \(c.message) | \(c.confirmLabel)\(disabled)\(remove) / \(c.cancelLabel)"
    }

    func windowDetail(_ d: AppSpaceDetail) -> [String: Any] {
        [
            "title": d.title,
            "facts": facts(d.facts),
            "isHost": d.isHost,
            "canStream": d.canStream,
            "previewText": d.previewText,
            "actions": d.actions.map { a in
                "\(actionWord(a.id)): \(a.label) [\(dash(a.symbol))] ?\(a.help)\(a.enabled ? "" : " (disabled)")\(a.destructive ? " (destructive)" : "")\(a.primary ? " (primary)" : "")"
            },
            "confirm": confirmLine(d.confirm),
            "sections": d.sections,
        ]
    }

    func hostActionWord(_ id: AppHostActionId) -> String {
        switch id {
        case .setUp: "set-up"
        case .stopSharing: "stop-sharing"
        case .resumeSharing: "resume-sharing"
        case .remove: "remove"
        case .shareDesktop: "share-desktop"
        case .hideDesktop: "hide-desktop"
        case .provideSpaces: "provide-spaces"
        case .stopProvidingSpaces: "stop-providing-spaces"
        }
    }

    func hostPanelFrame(_ v: AppHostPanelView) -> [String: Any] {
        let clients: Any = v.clientsTitle.map { "\($0): \(v.clientsEmpty ?? v.clients.joined(separator: ", "))" } ?? NSNull()
        return [
            "title": v.title,
            "summary": v.summary,
            "configured": v.configured,
            "facts": facts(v.facts),
            "clients": clients,
            "permissions": (v.permissionsTitle.map { [$0] } ?? [])
                + v.permissions.map { "\($0.id): \($0.title) ?\($0.help) <\(dash($0.settingsUrl))>" },
            "openSettings": v.openSettingsLabel,
            "actions": v.actions.map { "\(hostActionWord($0.id)): \($0.label)\($0.destructive ? " (destructive)" : "")" },
        ]
    }

    func hostFormFrame(_ v: AppHostFormView) -> [String: Any] {
        let request: Any = v.request.map { r in
            "mode=\(r.mode) relay=\(dash(r.relayUrl)) direct=\(dash(r.direct)) name=\(dash(r.name)) allow=\(r.allow.map { $0.joined(separator: ",") } ?? "-")"
        } ?? NSNull()
        return [
            "title": v.title,
            "lede": v.lede,
            "fields": v.fields.map { f in
                f.toggle
                    ? "\(f.id): \(f.label) = \(f.on ? "on" : "off")\(f.advanced ? " (advanced)" : "")"
                    : "\(f.id): \(f.label) [\(dash(f.placeholder))] = \(f.value)\(f.invalid ? " !invalid" : "")\(f.advanced ? " (advanced)" : "")"
            },
            "advanced": "\(v.advancedLabel)\(v.advancedOpen ? " (open)" : "")",
            "buttons": "\(v.backLabel) / \(v.submitLabel)",
            "canSubmit": v.canSubmit,
            "busy": v.busy,
            "error": opt(v.error),
            "request": request,
        ]
    }

    func chromeFrame(_ c: AppMainChrome) -> [String] {
        [
            "title: \(c.title)",
            "newSpace: \(c.newSpaceLabel) \(c.newSpaceShortcut)",
            "search: \(c.searchPlaceholder)",
            "keyvault: \(c.keyvaultTitle)",
            "account: \(c.account)",
            "signIn: \(dash(c.signInLabel))",
            "settings: \(c.settingsLabel) \(c.settingsShortcut)",
            "empty: \(c.emptyTitle) / \(c.emptyAction)",
            "volume: \(dash(c.volumeLabel))",
        ]
    }

    func menuWord(_ id: AppMenuItemId) -> String {
        switch id {
        case .status: "status"
        case .separator: "separator"
        case .open: "open"
        case .newSpace: "new-space"
        case .settings: "settings"
        case .quit: "quit"
        case .volumeConflicts: "volume-conflicts"
        }
    }

    func menuFrame(_ items: [AppMenuItem]) -> [String] {
        items.map { m in
            m.id == .separator ? "---"
                : "\(menuWord(m.id)): \(m.label)\(m.shortcut.map { " \($0)" } ?? "")\(m.enabled ? "" : " (disabled)")"
        }
    }

    func settingsFrame(_ p: AppSettingsPage) -> [String] {
        var out = ["= \(p.title)"]
        for sec in p.sections {
            out.append("# \(sec.title) (\(sec.id))\(sec.button.map { " [\($0)]" } ?? "")\(sec.button == nil || sec.buttonEnabled ? "" : " (disabled)")")
            for r in sec.rows { out.append(settingRow(r)) }
        }
        return out
    }

    /// One Settings row (`frame::setting_row`).
    func settingRow(_ r: AppSettingsRow) -> String {
        let options = r.options.isEmpty ? "" : " {" + r.options.map { "\($0.active ? "*" : "")\($0.label)" }.joined(separator: "|") + "}"
        return "- \(r.id) \(String(describing: r.kind)): \(r.label)"
            + (r.value.map { " = \($0)" } ?? "") + (r.placeholder.map { " ~\($0)" } ?? "")
            + (r.button.map { " [\($0)]" } ?? "") + options
            + (r.enabled ? "" : " (disabled)") + (r.help.map { " ?\($0)" } ?? "")
            + (r.linkUrl.map { " <\(r.linkLabel ?? "") \($0)>" } ?? "")
    }

    /// The Volume page's storage choice (`frame::drive_storage`).
    func driveStorageFrame(_ d: AppDriveCard?) -> Any {
        guard let d, let title = d.storageTitle else { return NSNull() }
        var out = ["\(title) {" + d.storageOptions.map { "\($0.active ? "*" : "")\($0.label)" }.joined(separator: "|") + "}"]
        out += d.storageRows.map(settingRow)
        if let n = d.storageNote { out.append("~\(n)") }
        if let t = d.storedIn { out.append("\(t) (\(d.storedPath ?? ""))") }
        if let t = d.mountedAt { out.append("\(t) (\(d.mountedPath ?? ""))") }
        out.append("continue \(d.canContinue)")
        return out
    }

    func previewScene(_ p: AppPresentationPreview) -> String {
        let size = "\(Int(p.width.rounded()))x\(Int(p.height.rounded())) loop \(p.loopMs)"
        if let n = p.notch {
            return "notch: tiles \(n.view.tiles.map(\.name).joined(separator: ",")); tab \(n.view.tab.count) \(n.view.tab.word); \(size)"
        }
        let rows = (p.menu?.rows ?? []).map { r -> String in
            let label = r.item.id == .separator ? "-" : r.item.label
            return r.item.shortcut.map { "\(label) \($0)" } ?? label
        }
        return "menu: \(rows.joined(separator: "|")); \(size)"
    }

    func previewFrame(_ f: AppPreviewFrame) -> String {
        func n(_ v: Double, _ k: Double) -> Int { Int((v * k).rounded()) }
        return "pointer \(n(f.pointer.x, 10)),\(n(f.pointer.y, 10)) open \(n(f.open, 1000)) content \(n(f.content, 1000))"
            + " tab \(n(f.tab, 1000)) hover \(n(f.hover, 1000)) pressed \(f.pressed) active \(f.active)"
            + " highlighted \(f.highlighted.map { String($0) } ?? "-")"
    }

    func onboardingFrame(_ v: AppOnboardingView) -> [String: Any] {
        var out = onboardingPage(v)
        if let c = v.launchAtLogin {
            out["launchAtLogin"] = "[\(c.checked ? "x" : " ")] \(c.label) ~\(c.note)"
        }
        return out
    }

    func onboardingPage(_ v: AppOnboardingView) -> [String: Any] {
        [
            "step": "\(v.step)",
            "title": v.title,
            "lede": v.lede,
            "primary": v.primaryLabel,
            "canSkip": v.canSkip,
            "canBack": v.canBack,
            "showMark": v.showMark,
            "dots": v.dots.map { "\($0.current ? "*" : "")\($0.label)" }.joined(separator: ","),
            "summary": facts(v.summary),
            "choices": v.choices.map { "\($0.mode): \($0.label)\($0.preselected ? " (preselected)" : "")" },
            "presentations": v.presentations.map { "\($0.title) [\($0.id)]\($0.selected ? " (selected)" : "")" },
            "prompts": v.prompts,
            "notice": opt(v.notice),
            "noticeLink": v.noticeLinkUrl.map { "\(v.noticeLinkLabel ?? "") \($0)" } as Any? ?? NSNull(),
            "usage": v.usage.map { u in
                "\(u.label) [\(u.on ? "x" : " ")]\(u.enabled ? "" : " (disabled)")\(u.help.map { " ~\($0)" } ?? "")"
            } as Any? ?? NSNull(),
            "drive": v.drive.map(driveCardFrame) as Any? ?? NSNull(),
            "driveStorage": driveStorageFrame(v.drive),
        ]
    }

    /// The notch and the menu count the same Spaces (`run_menu_count`).
    func runMenuCount(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        var frames: [Any] = []
        for c in f["cases"] as! [[String: Any]] {
            let spaces = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(c["rows"])), nowMs: now)
            let state: AppHostState? = c["hostState"] is NSNull ? nil : try appHostStateFromJson(json: text(c["hostState"]))
            let roster = appWithThisMachine(spaces: spaces, status: state.map { appHostSummaryInput(state: $0) }, nowMs: now)
            let tab = appNotchView(state: appNotchInitial(), spaces: roster).tab.count
            let count = appOpenableCount(spaces: roster)
            let sync: AppDriveSyncInput? = c["sync"] is NSNull ? nil : try appDriveSyncFromJson(json: text(c["sync"]))
            let menu = appMenu(input: AppMenuInput(spaces: roster, keyvault: nil, sync: sync,
                                                   nowMs: UInt64(c["nowMs"] as! Int64),
                                                   backend: c["backend"] as? String, experiments: nil))
            let first = menu[0].label
            let menuCount = first.hasPrefix("No Spaces") ? "0" : String(first.split(separator: " ")[0])
            frames.append([
                "case": c["name"] as! String,
                "count": count,
                "notch": tab,
                "menu": menu.filter { $0.id != .separator }.map { "\(menuWord($0.id)): \($0.label)\($0.enabled ? "" : " (disabled)")" },
                "agree": menuCount == tab && tab == String(count),
            ])
        }
        return frames
    }

    /// The Cua Volume page's card (`frame::drive_card`).
    func driveCardFrame(_ d: AppDriveCard) -> String {
        "\(d.label) [\(d.checked ? "x" : " ")]\(d.enabled ? "" : " (disabled)")\(d.busy ? " (busy)" : "")"
            + (d.note.map { " ~\($0)" } ?? "") + (d.error.map { " !\($0)" } ?? "")
            + (d.settingsUrl.map { " <\(d.settingsLabel ?? "") \($0)>" } ?? "") + " | \(d.imageLabel)"
    }

    /// The first run's Cua Volume page (`run_drive_onboarding`).
    func runDriveOnboarding(_ f: [String: Any]) throws -> [Any] {
        let p = appDriveMountPreview()
        func n(_ v: Double, _ k: Double) -> Int { Int((v * k).rounded()) }
        func r(_ v: AppLogicalRect) -> String {
            "\(Int(v.x.rounded())),\(Int(v.y.rounded())) \(Int(v.width.rounded()))x\(Int(v.height.rounded()))"
        }
        func frame(_ d: AppDriveMountFrame) -> String {
            let flight = d.flight.map { "\(n($0.x, 10)),\(n($0.y, 10))" } ?? "-"
            return "volume \(n(d.volume, 1000)) flight \(flight) arrived \(d.arrived.map { String(n($0, 1000)) }.joined(separator: ","))"
        }
        var lines = ["space \(r(p.space.frame)); finder \(r(p.finder.frame)); sidebar \(r(p.sidebar)); places \(p.places.count);"
            + " volume \(p.volumeLabel) \(r(p.volume)) at \(Int(p.volumeLabelX.rounded())); files \(p.sourceIcons.count)->\(p.destIcons.count);"
            + " \(Int(p.width.rounded()))x\(Int(p.height.rounded())) loop \(p.loopMs)"]
        for t in f["times"] as! [Int] {
            lines.append("\(t): \(frame(appDriveMountPreviewFrame(tMs: UInt32(t))))")
        }
        lines.append("still: \(frame(appDriveMountPreviewStill()))")
        var frames: [Any] = [["drivePreview": lines]]
        for run in f["runs"] as! [[String: Any]] {
            var st = appOnboardingInitial(installerMode: nil, identity: nil)
            for step in run["steps"] as! [[String: Any]] {
                st = appOnboardingReduce(state: st, action: try appOnboardingActionFromJson(json: text(step["action"])))
                guard let name = step["step"] as? String else { continue }
                frames.append([
                    "run": run["name"] as! String,
                    "step": name,
                    "request": st.driveRequest.map { "\($0)" } as Any? ?? NSNull(),
                    "storageRequest": st.storage.request.map { appStorageRequestText(request: $0) } as Any? ?? NSNull(),
                    "frame": onboardingFrame(appOnboardingView(state: st)),
                ])
            }
        }
        return frames
    }

    /// Settings' Storage section (`run_drive_storage`).
    func runDriveStorage(_ f: [String: Any]) throws -> [Any] {
        var input = f["input"] as! [String: Any]
        var state = appStorageInitial()
        var frames: [Any] = []
        for step in f["steps"] as! [[String: Any]] {
            patched(&input, step)
            let typed = try appStorageInputFromJson(json: text(input))
            var actions: [AppStorageAction] = []
            if let a = step["action"] as? [String: Any] { actions.append(try appStorageActionFromJson(json: text(a))) }
            if let id = step["press"] as? String, let a = appStoragePress(input: typed, id: id) { actions.append(a) }
            if let c = step["choose"] as? [String], let a = appStorageChoose(id: c[0], option: c[1]) { actions.append(a) }
            if let e = step["edit"] as? [String], let a = appStorageEdit(id: e[0], value: e[1]) { actions.append(a) }
            for a in actions { state = appStorageReduce(state: state, action: a) }
            let section = appStorageSection(input: typed, state: state)
            frames.append([
                "step": step["step"] as! String,
                "section": settingsFrame(AppSettingsPage(title: "Settings", sections: [section])),
                "request": state.request.map { appStorageRequestText(request: $0) } as Any? ?? NSNull(),
            ])
        }
        return frames
    }

    func runMainWindow(_ f: [String: Any]) throws -> [Any] {
        let now = Int64(f["now"] as! Int)
        var frames: [Any] = []
        let spaces = appRowsToSpaces(rows: try appSpaceRowsFromJson(json: text(f["rows"])), nowMs: now)
        var last = appRosterInitial(spaces: [])
        for raw in f["hostStates"] as! [Any] {
            let state: AppHostState? = raw is NSNull ? nil : try appHostStateFromJson(json: text(raw))
            let with = appWithThisMachine(spaces: spaces, status: state.map { appHostSummaryInput(state: $0) }, nowMs: now)
            var roster = appRosterInitial(spaces: [])
            roster = appRosterReduce(state: roster, action: .syncSpaces(spaces: with))
            frames.append([
                "host": state.map { $0.configured as Any } ?? NSNull(),
                "sidebar": windowSidebar(appSidebar(spaces: roster.spaces, query: "", selectedId: "")),
                "panel": hostPanelFrame(appHostPanel(state: state)),
            ])
            last = roster
        }
        let q = f["query"] as! String
        frames.append(["query": q, "sidebar": windowSidebar(appSidebar(spaces: last.spaces, query: q, selectedId: "this-mac"))])
        for s in last.spaces {
            frames.append(["detail": s.id, "frame": windowDetail(appSpaceDetail(space: s))])
        }
        let c = appSpaceDetailCopy()
        let df = f["deleteFailed"] as! [String: Any]
        frames.append([
            "detailCopy": [
                "streamLoading: \(c.streamLoading)", "streamEmpty: \(c.streamEmpty)", "streamFailed: \(c.streamFailed)",
                "streamNoMatch: \(c.streamNoMatch)",
                "agentsLoading: \(c.agentsLoading)", "agentsEmpty: \(c.agentsEmpty)", "agentsFailed: \(c.agentsFailed)",
                "agentsNoMatch: \(c.agentsNoMatch)", "dropCaption: \(c.dropCaption)", "sendFile: \(c.sendFile)",
                "teleportApp: \(c.teleportApp)", "teleportSymbol: \(c.teleportSymbol)",
                "teleportSymbolActive: \(c.teleportSymbolActive)",
            ],
            "deleteFailed": appDeleteFailedText(name: df["name"] as! String, error: df["error"] as! String),
        ])

        let identity = f["formIdentity"] as? String
        var form = appHostFormInitial()
        frames.append(["hostForm": "initial", "frame": hostFormFrame(appHostFormView(state: form, identity: identity))])
        for a in f["hostForm"] as! [[String: Any]] {
            form = appHostFormReduce(state: form, action: try appHostFormActionFromJson(json: text(a)))
            frames.append(["hostForm": a["type"]!, "frame": hostFormFrame(appHostFormView(state: form, identity: identity))])
        }
        for input in f["chrome"] as! [Any] {
            frames.append(["chrome": chromeFrame(appMainChrome(input: try appChromeInputFromJson(json: text(input))))])
        }
        for n in f["menuBar"] as! [Int] {
            frames.append(["menuBar": n, "frame": menuFrame(appMenuBar(spaces: UInt32(n)))])
        }

        let ag = f["agents"] as! [String: Any]
        let rows = appAgentSettingsRows(statuses: try appAgentSetupStatusesFromJson(json: text(ag["statuses"])),
                                        total: UInt32(ag["total"] as! Int))
        let outcomes = try appAgentSetupOutcomesFromJson(json: text(ag["outcomes"]))
        frames.append([
            "agents": rows.map { r in
                "\(r.agent): \(r.name)\(r.installed ? " installed" : "")\(r.configured ? " configured" : "") (\(r.detail)) \(r.skillsInstalled)/\(r.skillsTotal)"
            },
            "summaries": rows.filter(\.installed).map { r in
                let s = appAgentSetupSummary(outcomes: outcomes, agent: r.agent, name: r.name)
                return "\(s.line) | \(s.text) | \(s.failed.joined(separator: "; "))"
            },
        ])
        for (i, raw) in (f["settings"] as! [Any]).enumerated() {
            var input = try appSettingsInputFromJson(json: text(raw))
            if i >= 2 { input.agents = rows }
            frames.append(["settings": i, "frame": settingsFrame(appSettingsPage(input: input))])
        }

        let ob = f["onboarding"] as! [String: Any]
        var st = appOnboardingInitial(installerMode: (ob["installerMode"] as? String) == "host" ? .host : .client, identity: nil)
        frames.append(["onboarding": "initial", "frame": onboardingFrame(appOnboardingView(state: st))])
        for a in ob["actions"] as! [[String: Any]] {
            st = appOnboardingReduce(state: st, action: try appOnboardingActionFromJson(json: text(a)))
            frames.append(["onboarding": a["type"]!, "frame": onboardingFrame(appOnboardingView(state: st))])
        }
        let oc = appOnboardingCopy()
        let t = ob["texts"] as! [String: Any]
        var texts: [String] = []
        for id in t["identities"] as! [Any] { texts.append(appOnboardingSignedInText(identity: id as? String)) }
        for code in t["codes"] as! [Any] { texts.append(appOnboardingSignInCodeText(userCode: code as? String)) }
        for v in t["versions"] as! [Any] { texts.append(appOnboardingReplacesText(installedVersion: v as? String)) }
        texts.append(appOnboardingInstalledAtText(target: t["target"] as! String))
        frames.append([
            "onboardingCopy": [
                "back: \(oc.back)", "skip: \(oc.skip)", "continueLabel: \(oc.continueLabel)", "tryAgain: \(oc.tryAgain)",
                "checking: \(oc.checking)", "installScript: \(oc.installScript)", "install: \(oc.install)",
                "installing: \(oc.installing)", "addToPath: \(oc.addToPath)", "shadowed: \(oc.shadowed)",
                "signIn: \(oc.signIn)", "signInWaiting: \(oc.signInWaiting)", "agentsLooking: \(oc.agentsLooking)",
                "agentsNone: \(oc.agentsNone)", "agentsSkills: \(oc.agentsSkills)", "agentsMcp: \(oc.agentsMcp)",
                "agentsSetUp: \(oc.agentsSetUp)", "agentsSettingUp: \(oc.agentsSettingUp)",
                "agentsDoneTitle: \(oc.agentsDoneTitle)", "agentsDoneLede: \(oc.agentsDoneLede)",
                "permissionsTitle: \(oc.permissionsTitle)", "openSettings: \(oc.openSettings)",
            ],
            "texts": texts,
        ])
        frames.append(["presentationPreviews": [false, true].map { menuBar -> [String] in
            let p = appPresentationPreview(menuBar: menuBar)
            var lines = [previewScene(p)]
            for t: UInt32 in [0, 850, 1350, 1550, 1700, 2400, 2750, 3150, 3900] {
                lines.append("\(t): \(previewFrame(appPresentationPreviewFrame(menuBar: menuBar, tMs: t)))")
            }
            lines.append("still: \(previewFrame(appPresentationPreviewStill(menuBar: menuBar)))")
            return lines
        }])
        let d = f["drops"] as! [String: Any]
        frames.append([
            "dropSending": (d["sending"] as! [[String]]).map { appDropSendingText(paths: $0) },
            "dropSent": try (d["sent"] as! [Any]).map { appDropSentText(files: try appSentFilesFromJson(json: text($0))) },
        ])
        return frames
    }

    // MARK: - picker-grid

    func openWindow(_ w: [String: Any]) -> AppOpenWindow {
        AppOpenWindow(windowId: (w["windowId"] as! NSNumber).uint32Value, appId: w["appId"] as! String,
                      appName: w["appName"] as! String, windowTitle: w["windowTitle"] as! String,
                      supported: w["supported"] as! Bool, bundlePath: w["bundlePath"] as? String)
    }

    func gridFrame(_ g: AppPickerGrid) -> [String: Any] {
        var lines: [String] = []
        for sec in g.sections {
            if !sec.title.isEmpty { lines.append("# \(sec.title)") }
            for t in sec.tiles {
                let icon: String
                switch t.icon {
                case let .host(path): icon = "host \(path)"
                case let .guest(appName, appId, pid): icon = "guest \(appName) \(appId) \(pid)"
                default: icon = "-"
                }
                let thumb: String
                switch t.thumbnail {
                case let .hostWindow(windowId): thumb = "window \(windowId)"
                case let .guestWindow(windowId, epoch): thumb = "guest \(windowId)@\(epoch)"
                default: thumb = "-"
                }
                lines.append("\(t.id) | \(t.title) | \(icon) | \(thumb) | \(t.help)\(t.disabled ? " (dim)" : "")\(t.selected ? " (selected)" : "")")
            }
        }
        return ["tiles": lines, "emptyText": opt(g.emptyText)]
    }

    func word(_ v: AppPickerGridTab) -> String { v == .apps ? "apps" : v == .windows ? "windows" : "space" }

    func runPickerGrid(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        let space = f["spaceName"] as! String
        frames.append(["tabs": appPickerGridTabs(spaceName: space).map { ["label": $0.label, "tab": word($0.tab)] }])
        var s = appPickerInitial(spaceName: space)
        s = appPickerReduce(state: s, event: .loaded(entries: try appCatalogEntriesFromJson(json: text(f["entries"]))))
        let windows = (f["windows"] as! [[String: Any]]).map(openWindow)
        frames.append(["apps": "loaded", "grid": gridFrame(appPickerAppGrid(state: s, windows: windows))])
        for case let delta as Int in f["steps"] as! [Any] {
            let grid = appPickerAppGrid(state: s, windows: windows)
            if let next = appPickerGridStep(grid: grid, selected: s.selectedId, delta: Int32(delta)) {
                s = appPickerReduce(state: s, event: .select(id: next))
            }
            frames.append(["step": delta, "selected": opt(s.selectedId)])
        }
        let q = f["query"] as! String
        s = appPickerReduce(state: s, event: .query(query: q))
        frames.append(["apps": "query \(q)", "grid": gridFrame(appPickerAppGrid(state: s, windows: windows))])
        frames.append(["windows": "all", "grid": gridFrame(appPickerWindowGrid(windows: windows, query: "", selected: "12"))])
        frames.append(["windows": "no match",
                       "grid": gridFrame(appPickerWindowGrid(windows: windows, query: "nothing like it", selected: nil))])
        let remote = (f["remote"] as! [[String: Any]]).map(remoteWindow)
        frames.append(["remote": "all", "grid": gridFrame(appPickerRemoteGrid(windows: remote, query: "", selected: nil))])
        let rq = f["remoteQuery"] as! String
        let remoteSelected = appPickerRemoteGrid(windows: remote, query: rq, selected: "w-1")
        frames.append(["remote": "query \(rq)", "grid": gridFrame(remoteSelected)])
        // The primary button of each tab, with and without a selection.
        for (tab, grid) in [
            (AppPickerGridTab.apps, appPickerAppGrid(state: s, windows: windows)),
            (.windows, appPickerWindowGrid(windows: windows, query: "", selected: "12")),
            (.windows, appPickerWindowGrid(windows: windows, query: "", selected: nil)),
            (.space, remoteSelected),
        ] {
            let p = appPickerGridPrimary(tab: tab, spaceName: space, grid: grid)
            frames.append(["primary": word(tab), "label": p.label, "enabled": p.enabled])
        }
        return frames
    }

    // MARK: - space-facts

    func runSpaceFacts(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        for case let c as [String: Any] in f["cases"] as! [Any] {
            let rows = try appSpaceRowsFromJson(json: text([c["row"]!]))
            let space = appRowsToSpaces(rows: rows, nowMs: Int64(f["now"] as! Int))[0]
            let usage = (c["usage"] as? [String: Any]).map { u in
                AppSpaceUsage(memoryUsed: (u["memoryUsed"] as! NSNumber).uint64Value,
                              memoryTotal: (u["memoryTotal"] as! NSNumber).uint64Value,
                              memoryLimited: u["memoryLimited"] as! Bool,
                              diskUsed: (u["diskUsed"] as! NSNumber).uint64Value,
                              diskTotal: (u["diskTotal"] as! NSNumber).uint64Value,
                              diskLimited: u["diskLimited"] as! Bool)
            }
            frames.append(["case": c["name"] as! String,
                           "facts": facts(appSpaceDetailLive(space: space, usage: usage,
                                                             hostArch: f["hostArch"] as? String).facts)])
        }
        return frames
    }

    // MARK: - stream-section

    func remoteWindow(_ w: [String: Any]) -> AppRemoteWindow {
        func u(_ k: String) -> UInt32? { (w[k] as? NSNumber)?.uint32Value }
        return AppRemoteWindow(
            id: w["id"] as! String, appName: w["appName"] as! String, title: w["title"] as! String,
            visible: w["visible"] as? Bool ?? true, appId: w["appId"] as! String,
            targetEpoch: (w["targetEpoch"] as! NSNumber).uint64Value,
            widthPx: u("widthPx"), heightPx: u("heightPx"), pid: u("pid"))
    }

    func runStreamSection(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        for case let c as [String: Any] in f["cases"] as! [Any] {
            let i = c["input"] as! [String: Any]
            let display = (i["display"] as? [String: Any]).map {
                AppStreamDisplay(widthPx: ($0["widthPx"] as! NSNumber).uint32Value,
                                 heightPx: ($0["heightPx"] as! NSNumber).uint32Value)
            }
            let input = AppStreamSectionInput(
                windows: (i["windows"] as? [[String: Any]]).map { $0.map(remoteWindow) },
                failed: i["failed"] as? Bool ?? false, display: display, os: os(i["os"] as! String),
                osName: i["osName"] as? String, open: i["open"] as? [String] ?? [],
                query: i["query"] as? String ?? "")
            let v = appStreamSection(input: input)
            let rows = v.rows.map { r -> String in
                let icon: String
                switch r.icon {
                case let .os(id): icon = "os \(id)"
                case let .app(appName, appId, pid): icon = "app \(appName) \(appId) \(pid)"
                }
                let actions = r.actions.map { a in
                    "\(word(a.id)) \(a.symbol): \(a.help)\(a.active ? " (on)" : "")"
                }.joined(separator: ", ")
                return "\(word(r.kind)) \(r.id) | \(r.label) | \(r.resolution ?? "-") | \(icon) | \(actions)"
            }
            frames.append([
                "case": c["name"] as! String,
                "rows": rows,
                "help": v.rows.map(\.help),
                "statusText": opt(v.statusText),
            ])
        }
        // Picture in picture: the panels the shell reports, each row's
        // button, and what a click does.
        if let pip = f["pip"] as? [String: Any] {
            let i = pip["input"] as! [String: Any]
            let display = (i["display"] as? [String: Any]).map {
                AppStreamDisplay(widthPx: ($0["widthPx"] as! NSNumber).uint32Value,
                                 heightPx: ($0["heightPx"] as! NSNumber).uint32Value)
            }
            var open: [String] = []
            for case let step as [String: Any] in pip["steps"] as! [Any] {
                if let row = step["click"] as? String {
                    switch appStreamPipClick(open: open, row: row) {
                    case let .open(r): frames.append(["click": row, "command": "open", "row": r])
                    case let .close(r): frames.append(["click": row, "command": "close", "row": r])
                    }
                    continue
                }
                let e = step["event"] as! [String: Any]
                let type = e["type"] as! String
                let event: AppPipEvent = switch type {
                case "opened": .opened(row: e["row"] as! String)
                case "closed": .closed(row: e["row"] as! String)
                default: .synced(rows: e["rows"] as! [String])
                }
                open = appStreamPipReduce(open: open, event: event)
                let v = appStreamSection(input: AppStreamSectionInput(
                    windows: (i["windows"] as? [[String: Any]]).map { $0.map(remoteWindow) },
                    failed: false, display: display, os: os(i["os"] as! String),
                    osName: i["osName"] as? String, open: open, query: ""))
                let buttons = v.rows.map { r in "\(r.id) \(r.actions[0].symbol) \(r.actions[0].active)" }
                frames.append(["event": type, "open": open, "buttons": buttons])
            }
        }
        return frames
    }

    // MARK: - devices

    func num(_ v: UInt64?) -> String { v.map(String.init) ?? "null" }

    func devicesFrame(_ v: AppDevicesView) -> [String: Any] {
        func lines(_ rows: [AppActivityRow]) -> [Any] {
            rows.map { "\($0.ts) \($0.text)\($0.notable ? " (notable)" : "")" }
        }
        let t = v.thisDevice, l = v.labels
        return [
            "banner": v.banner.map { b -> Any in
                "\(word(b.tone)): \(b.text) [\(b.actionLabel ?? "-")]" } ?? NSNull(),
            "enrolled": v.enrolled,
            "thisDevice": "\(word(t.kind)): \(t.title) @\(num(t.at)) name=\(t.name ?? "-") [\(t.actionLabel ?? "-")]",
            "rows": v.rows.map { r -> Any in
                "\(r.title) | \(r.detail) | seen \(num(r.lastSeen)) | \(r.actions.map(word).joined(separator: ","))"
                    + (r.current ? " | current" : "")
                    + (r.revokeConfirm.map { " | \($0.title) / \($0.message)" } ?? "")
            },
            "approvals": v.approvals.map { a -> Any in
                "\(a.deviceId)\(a.expired ? " (expired)" : ""): \(a.text) | \(a.notifyTitle) / \(a.notifyBody)"
            },
            "recent": lines(v.recent),
            "activity": lines(v.activity),
            "unconfirmedMachines": v.unconfirmedMachines.map { m -> Any in
                "\(m.title): \(m.confirm.title) / \(m.confirm.message)" },
            "labels": "\(l.title) / \(l.thisDevice) / \(l.devices) / \(l.recent) (\(l.recentEmpty)) / \(l.lastSeen) / "
                + "\(l.approve) \(l.deny) \(l.rename) \(l.revoke) / \(l.renameTitle) \(l.renameConfirm) \(l.cancel)"
                + " / \(l.newMachines) \(l.confirmMachine)",
        ]
    }

    func enrollFrame(_ v: AppEnrollView) -> [String: Any] {
        [
            "title": "\(v.title) / \(v.lede)",
            "options": v.options.map { o -> Any in "\(word(o.method)): \(o.title) / \(o.detail)" },
            "code": opt(v.code),
            "codeHelp": opt(v.codeHelp),
            "status": opt(v.status),
            "error": opt(v.error),
            "busy": v.busy,
            "done": v.done,
            "buttons": "\(v.backLabel ?? "-") / \(v.closeLabel)",
        ]
    }

    func approveFrame(_ v: AppApproveSheetView) -> [String: Any] {
        [
            "title": v.title,
            "message": v.message,
            "code": v.needsCode ? "\(v.codeLabel) [\(v.codePlaceholder)] = \(v.code)" as Any : NSNull(),
            "canApprove": v.canApprove,
            "buttons": "\(v.approveLabel) / \(v.denyLabel)\(v.denyRevokes ? " (revokes)" : "")",
            "presence": v.presenceReason,
            "busy": v.busy,
            "error": opt(v.error),
            "request": v.request.map { r -> Any in
                "code=\(r.code ?? "-") device=\(r.deviceId ?? "-")" } ?? NSNull(),
        ]
    }

    /// The AI agents page's background computer-use card (`run_driver_card`).
    func runDriverCard(_ f: [String: Any]) throws -> [Any] {
        let copy = appOnboardingCopy()
        var frames: [Any] = [["driverCopy": ["agentsDriver: \(copy.agentsDriver)", "agentsDriverImage: \(copy.agentsDriverImage)"]]]
        let p = appDriverPreview()
        func n(_ v: Double, _ k: Double) -> Int { Int((v * k).rounded()) }
        func r(_ v: AppLogicalRect) -> String {
            "\(Int(v.x.rounded())),\(Int(v.y.rounded())) \(Int(v.width.rounded()))x\(Int(v.height.rounded()))"
        }
        func frame(_ d: AppDriverFrame) -> String {
            "user \(n(d.pointer.x, 10)),\(n(d.pointer.y, 10)) pressed \(d.pressed) selection \(n(d.selection, 1000));"
                + " agent \(n(d.agent.x, 10)),\(n(d.agent.y, 10)) pressed \(d.agentPressed) ripple \(n(d.ripple, 1000));"
                + " checked \(d.checked.map { String(n($0, 1000)) }.joined(separator: ","))"
        }
        var lines = ["back \(r(p.back.frame)); front \(r(p.front.frame)); boxes \(p.checkboxes.count);"
            + " lines \(p.lines.count) (selects \(p.selectedLine)); agent \(p.agentFill) \(p.agentPointer.count) points,"
            + " \(p.agentRays.count) rays; \(Int(p.width.rounded()))x\(Int(p.height.rounded())) loop \(p.loopMs)"]
        for t in f["times"] as! [Int] {
            lines.append("\(t): \(frame(appDriverPreviewFrame(tMs: UInt32(t))))")
        }
        lines.append("still: \(frame(appDriverPreviewStill()))")
        frames.append(["driverPreview": lines])
        var summaries: [String] = []
        for s in f["summaries"] as! [[String: Any]] {
            let outcomes = try appAgentSetupOutcomesFromJson(json: text(s["outcomes"]))
            let v = appAgentSetupSummary(outcomes: outcomes, agent: s["agent"] as! String, name: s["name"] as! String)
            summaries.append("\(v.line) | \(v.text) | \(v.failed.joined(separator: "; "))")
        }
        frames.append(["summaries": summaries])
        return frames
    }

    func runDevices(_ f: [String: Any]) throws -> [Any] {
        let now = (f["now"] as! NSNumber).uint64Value
        var frames: [Any] = []
        var last: AppDevicesView?
        var lastDevices: [AppDeviceInput] = []
        for case let step as [String: Any] in f["pages"] as! [Any] {
            let input = try appDevicesInputFromJson(json: text(step["input"]))
            let v = appDevicesView(input: input, now: now)
            last = v
            lastDevices = input.devices
            frames.append(["page": step["step"] as! String, "view": devicesFrame(v)])
        }
        for case let run as [String: Any] in f["enroll"] as! [Any] {
            var state = appEnrollInitial()
            frames.append(["enroll": run["name"] as! String, "action": "open",
                           "view": enrollFrame(appEnrollView(state: state))])
            for case let a as [String: Any] in run["actions"] as! [Any] {
                state = appEnrollReduce(state: state, action: try appEnrollActionFromJson(json: text(a)))
                frames.append(["enroll": run["name"] as! String, "action": a["type"] as! String,
                               "phase": word(state.phase), "view": enrollFrame(appEnrollView(state: state))])
            }
        }
        for (i, item) in (f["approve"] as! [Any]).enumerated() {
            let run = item as! [Any]
            let prompt = last!.approvals[i]
            var state = appApproveOpen(prompt: prompt)
            frames.append(["approve": prompt.deviceId, "action": "open",
                           "view": approveFrame(appApproveView(state: state, devices: lastDevices))])
            for case let a as [String: Any] in run {
                state = appApproveReduce(state: state, action: try appApproveSheetActionFromJson(json: text(a)), devices: lastDevices)
                frames.append(["approve": prompt.deviceId, "action": a["type"] as! String,
                               "view": approveFrame(appApproveView(state: state, devices: lastDevices))])
            }
        }
        return frames
    }

    func word(_ v: AppBannerTone) -> String { "\(v)" }
    func word(_ v: AppDeviceAction) -> String { "\(v)" }
    func word(_ v: AppEnrollmentKind) -> String {
        switch v {
        case .enrolled: "enrolled"
        case .grace: "grace"
        case .needsEnrollment: "needs-enrollment"
        case .waiting: "waiting"
        case .due: "due"
        case .revoked: "revoked"
        }
    }
    func word(_ v: AppEnrollMethod) -> String { v == .signIn ? "sign-in" : "approve" }
    func word(_ v: AppEnrollPhase) -> String {
        switch v {
        case .choose: "choose"
        case .signingIn: "signing-in"
        case .registering: "registering"
        case .waiting: "waiting"
        case .enrolled: "enrolled"
        case .failed: "failed"
        }
    }

    // MARK: - Enum words (the core's serde names)

    func word(_ v: AppStreamRowKind) -> String { v == .desktop ? "desktop" : "window" }
    func word(_ v: AppStreamRowActionId) -> String { "\(v)" }

    func os(_ w: String) -> AppSpaceOs { w == "macos" ? .macos : w == "windows" ? .windows : .linux }
    func runtime(_ w: String) -> AppRuntime {
        [.auto, .gvisor, .runc, .qemu, .lume, .kubevirt].first { word($0) == w } ?? .auto
    }
    func move(_ w: String) -> AppTeleportMove {
        [.appOnly, .appWithFiles, .appWithState].first { word($0) == w }!
    }
    func runPhase(_ w: String) -> AppTeleportRunPhase {
        [.started, .progress, .finished, .failed, .done].first { word($0) == w }!
    }
    func word(_ v: AppLocation) -> String { v == .cloud ? "cloud" : "local" }
    func word(_ v: AppRuntime) -> String {
        switch v {
        case .auto: "auto"
        case .gvisor: "gvisor"
        case .runc: "runc"
        case .qemu: "qemu"
        case .lume: "lume"
        case .kubevirt: "kubevirt"
        }
    }
    func word(_ v: AppWindowMode) -> String {
        switch v {
        case .ambient: "ambient"
        case .ambientTeleport: "ambient-teleport"
        case .switcher: "switcher"
        case .createFleet: "create-fleet"
        }
    }
    func word(_ v: AppPickerStep) -> String { "\(v)" }
    func word(_ v: AppTeleportMove) -> String {
        switch v {
        case .appOnly: "app_only"
        case .appWithFiles: "app_with_files"
        case .appWithState: "app_with_state"
        }
    }
    func word(_ v: AppTeleportRunPhase) -> String { "\(v)" }
    func word(_ v: AppConsentKind) -> String { "\(v)" }
    func word(_ v: AppNotchPhase) -> String { "\(v)" }
    func word(_ v: KvTri) -> String { "\(v)" }
    func word(_ v: KvDecisionTone) -> String { "\(v)" }

    // MARK: - your-cloud

    func runYourCloud(_ f: [String: Any]) throws -> [Any] {
        let known = f["clouds"] as! [String: Any]
        var env = try appWizardEnvFromJson(json: text(f["env"]))
        let sheet = try appCloudConnectInputFromJson(json: text(f["sheet"]))
        var wizard = appWizardInitial(env: env)
        var state = appCloudConnectInitial()
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            if let names = step["clouds"] as? [String] {
                env.clouds = try names.map { try appConnectedCloudFromJson(json: text(known[$0])) }
            }
            if let d = step["defaultLocation"] as? String { env.defaultLocation = location(d) }
            if step.keys.contains("sheet") {
                if let a = step["sheet"] as? [String: Any] {
                    state = appCloudConnectReduce(input: sheet, state: state,
                                                  action: try appCloudConnectActionFromJson(json: text(a)))
                }
                frames.append(["step": step["step"] as! String,
                               "sheet": cloudConnectFrame(appCloudConnectView(input: sheet, state: state))])
                continue
            }
            if let a = step["wizard"] as? [String: Any] {
                wizard = appWizardReduce(state: wizard, action: wizardAction(a), env: env)
            }
            let v = appWizardView(state: wizard, env: env)
            let args = appWizardCreateArgs(plan: v.plan)
            let n = { (x: UInt32?) in x.map { "\($0)" } ?? "null" }
            frames.append([
                "step": step["step"] as! String,
                "wizard": yourCloudFrame(v),
                "createSpace": "on=\(args.on) image=\(args.image) cpus=\(n(args.cpus)) memoryMb=\(n(args.memoryMb))",
            ])
        }
        return frames
    }

    func yourCloudFrame(_ v: AppWizardView) -> [String: Any] {
        func tiles(_ ts: [AppTile], onlyOff: Bool) -> [String] {
            ts.filter { !onlyOff || !$0.enabled }
                .map { "\($0.id) | \($0.title) | \($0.detail) | \($0.enabled ? "on" : "off")" }
        }
        return [
            "step": Int(v.step),
            "canContinue": v.canContinue,
            "placement": v.plan.placement.word,
            "cloud": opt(v.cloud),
            "placements": placementMenu(v),
            "systemsOff": tiles(v.osTiles, onlyOff: true),
            "kindsOff": tiles(v.kindTiles, onlyOff: true),
            "placementError": opt(v.placementError),
            "fields": v.fields.map(\.id),
            "price": opt(v.price),
            "resourceFacts": v.resourceFacts.map { "\($0.label): \($0.value)" },
            "summary": v.summary.map { "\($0.label): \($0.value)" },
        ]
    }

    /// The Run on menu as `parity.rs` writes it: `id | label | detail | on`
    /// (` *` when chosen), each group after a `---` line.
    func placementMenu(_ v: AppWizardView) -> [String] {
        var out: [String] = []
        var group: String?
        for o in v.placements {
            if let g = group, g != o.group { out.append("---") }
            group = o.group
            out.append("\(o.id) | \(o.label) | \(o.detail) | \(o.enabled ? "on" : "off")\(o.selected ? " *" : "")")
        }
        return out
    }

    func cloudConnectFrame(_ v: AppCloudConnectView) -> [String: Any] {
        func field(_ f: AppCloudField?) -> Any {
            f.map { "\($0.label) [\($0.placeholder)] = \($0.value)" } ?? NSNull()
        }
        func target(_ t: AppCloudTargetArgs) -> [String: Any] {
            var d: [String: Any] = ["provider": t.provider]
            if let v = t.profile { d["profile"] = v }
            if let v = t.region { d["region"] = v }
            if let v = t.project { d["project"] = v }
            if let v = t.environment { d["environment"] = v }
            return d
        }
        let request: Any = switch v.request {
        case let .test(t)?: ["kind": "test", "target": target(t)]
        case let .connect(t, makeDefault)?: ["kind": "connect", "target": target(t), "make_default": makeDefault]
        case nil: NSNull()
        }
        return [
            "title": v.title,
            "rows": v.rows.map {
                "\($0.id) | \($0.title) | \($0.detail)\($0.found ? " | found" : "")\($0.selected ? " | selected" : "")"
            },
            "field": field(v.field),
            "profile": field(v.profileField),
            "checks": v.checks.map { "\($0.ok ? "ok" : "failed") \($0.text)" },
            "result": opt(v.result),
            "touches": v.touches,
            "buttons": "\(v.testLabel) \(v.canTest ? "on" : "off") ?\(v.testHelp) / \(v.connectLabel) "
                + "\(v.canConnect ? "on" : "off") / \(v.cancelLabel) / \(v.makeDefaultLabel) \(v.makeDefault ? "on" : "off")",
            "error": opt(v.error),
            "done": v.done,
            "request": request,
        ]
    }

    // MARK: - share-sheet

    func runShare(_ f: [String: Any]) throws -> [Any] {
        var input = f["input"] as! [String: Any]
        var state = appShareInitial()
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            if let patch = step["input"] as? [String: Any] {
                for (k, v) in patch { input[k] = v }
            }
            let typed = try appShareInputFromJson(json: text(input))
            if let action = step["action"] as? [String: Any] {
                state = appShareReduce(input: typed, state: state, action: try appShareActionFromJson(json: text(action)))
            }
            frames.append(["step": step["step"] as! String, "view": shareFrame(appShareView(input: typed, state: state))])
        }
        return frames
    }

    func shareFrame(_ v: AppShareSheetView) -> [String: Any] {
        let request: Any
        switch v.request {
        case let .share(space, who, role)?: request = ["kind": "share", "space": space, "who": who, "role": role]
        case let .unshare(space, who)?: request = ["kind": "unshare", "space": space, "who": who]
        case nil: request = NSNull()
        }
        return [
            "title": v.title,
            "rows": v.rows.map { "\($0.who) | \($0.role)\($0.connected ? " | connected" : "")" },
            "empty": v.emptyText,
            "field": "\(v.whoPlaceholder) [\(v.who)] \(v.hint ?? "-") as \(v.role)",
            "roles": v.roles.map { "\($0.id)=\($0.label)" }.joined(separator: ", "),
            "buttons": "\(v.shareLabel) \(v.canShare ? "on" : "off") / \(v.removeLabel) / \(v.doneLabel)",
            "disabled": opt(v.disabledReason),
            "busy": v.busy,
            "error": opt(v.error),
            "request": request,
        ]
    }

    // MARK: - agents-page, drive-page, notifications

    func lines(_ ls: [AppLineView]) -> [String] {
        ls.map { l in
            let on = l.on.map { $0 ? "on" : "off" } ?? "-"
            return "\(l.text) | \(l.trailing) | \(l.actionLabel ?? "-") | \(l.secondaryLabel ?? "-") | \(on)"
        }
    }

    func patched(_ input: inout [String: Any], _ step: [String: Any]) {
        if let patch = step["input"] as? [String: Any] {
            for (k, v) in patch { input[k] = v }
        }
    }

    func runAgentsPage(_ f: [String: Any]) throws -> [Any] {
        var input = f["input"] as! [String: Any]
        let now = UInt64(f["nowMs"] as! Int64)
        var state = appAgentsInitial()
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            patched(&input, step)
            let typed = try appAgentsInputFromJson(json: text(input))
            if let action = step["action"] as? [String: Any] {
                state = appAgentsReduce(input: typed, state: state, action: try appAgentsActionFromJson(json: text(action)))
            }
            frames.append(["step": step["step"] as! String, "view": agentsFrame(appAgentsView(input: typed, state: state, nowMs: now))])
        }
        return frames
    }

    func agentsFrame(_ v: AppAgentsView) -> [String: Any] {
        var detail: Any = NSNull()
        if let d = v.detail {
            let f = d.form
            let schedule: String = switch f.schedule { case .every: "every"; case .daily: "daily"; case .weekly: "weekly" }
            let file: Any = d.file.map { ["path": $0.path, "text": $0.text, "versions": lines($0.versions), "close": $0.closeLabel] as [String: Any] } ?? NSNull()
            detail = [
                "name": d.name,
                "subtitle": d.subtitle,
                "tabs": d.tabs.map { "\($0.label)\($0.selected ? "*" : "")" }.joined(separator: ", "),
                "memory": lines(d.memory),
                "memoryEmpty": d.memoryEmpty,
                "file": file,
                "routines": lines(d.routines),
                "routinesEmpty": d.routinesEmpty,
                "form": "\(d.addRoutineLabel) [\(f.title)] [\(f.prompt)] \(schedule) \(f.minutes) \(f.time) \(f.weekday)",
                "schedules": lines(d.schedules),
                "canAdd": d.canAddRoutine,
                "access": lines(d.access),
                "accessEmpty": d.accessEmpty,
                "allowThisMachine": opt(d.allowThisMachineLabel),
                "audit": lines(d.audit),
                "error": opt(d.error),
            ] as [String: Any]
        }
        return [
            "title": v.title,
            "rows": v.rows.map { "\($0.name) | \($0.detail) | \($0.state) | \($0.actionLabel)\($0.selected ? " (selected)" : "")" },
            "empty": v.emptyText,
            "detail": detail,
            "busy": v.busy,
            "error": opt(v.error),
            "request": opt(v.requestText),
        ]
    }

    func runDrivePage(_ f: [String: Any]) throws -> [Any] {
        var input = f["input"] as! [String: Any]
        var state = appDriveInitial()
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            patched(&input, step)
            let typed = try appDriveInputFromJson(json: text(input))
            if let action = step["action"] as? [String: Any] {
                state = appDriveReduce(state: state, action: try appDriveActionFromJson(json: text(action)))
            }
            let v = appDriveView(input: typed, state: state)
            frames.append(["step": step["step"] as! String, "view": [
                "title": v.title,
                "requests": "\(v.requestsTitle): \(v.requests.count)",
                "requestRows": lines(v.requests),
                "grants": "\(v.grantsTitle): \(v.grantsEmpty)",
                "grantRows": lines(v.grants),
                "busy": v.busy,
                "error": opt(v.error),
                "request": opt(v.requestText),
                "open": v.openLabel.map { "\($0) \(v.mountPath ?? "-")" } as Any? ?? NSNull(),
                "mountLine": opt(v.mountLine),
                "devices": "\(v.devicesTitle): \(v.devices.count)",
                "deviceRows": lines(v.devices),
                "syncNote": v.syncNote.map { "\(v.syncError ? "!" : "")\($0)" } as Any? ?? NSNull(),
                "conflicts": v.conflicts.map { c in
                    "\(c.text) | \(c.trailing) | \(c.openLabel ?? "-") \(c.reveal ?? "-") | \(c.resolveLabel)"
                },
            ] as [String: Any]])
        }
        return frames
    }

    func runNotifications(_ f: [String: Any]) throws -> [Any] {
        let now = UInt64(f["nowMs"] as! Int64)
        var seen: UInt64 = 0
        var feed: [AppNotificationInput] = []
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            if let s = step["seenMs"] as? Int64 { seen = UInt64(s) }
            if let raw = step["feed"] { feed = try appNotificationsFromJson(json: text(raw)) }
            let plan = appNotificationsPlan(feed: feed, seenMs: seen)
            seen = plan.seenMs
            let v = appNotificationsView(feed: feed, nowMs: now)
            frames.append(["step": step["step"] as! String, "view": [
                "post": plan.post.map { "\($0.id) | \($0.title) | \($0.body)" },
                "seenMs": plan.seenMs,
                "title": v.title,
                "rows": lines(v.rows),
                "empty": v.emptyText,
                "unread": v.unread,
                "markAll": opt(v.markAllLabel),
            ] as [String: Any]])
        }
        return frames
    }

    // MARK: - telemetry-funnel

    /// A signal as the golden records it (the core's serde shape).
    static func signalFrame(_ s: AppTelemetrySignal) -> [String: Any] {
        switch s {
        case let .feature(feature): return ["type": "feature", "feature": feature]
        case let .step(step, ok): return ["type": "step", "step": step, "ok": ok]
        case let .onboardingPage(page, action, choice):
            return ["type": "onboarding-page", "page": page, "action": action, "choice": choice]
        case let .spaceWizard(action): return ["type": "space-wizard", "action": action]
        case let .spaceCreate(location, guestOs, kind, outcome, failedPhase, stalled, elapsedMs, gpu):
            return ["type": "space-create", "location": location, "guestOs": guestOs, "kind": kind,
                    "outcome": outcome, "failedPhase": failedPhase, "stalled": stalled, "elapsedMs": Int(elapsedMs),
                    "gpu": gpu]
        case let .spaceCreateStarted(location, guestOs, kind, gpu):
            return ["type": "space-create-started", "location": location, "guestOs": guestOs, "kind": kind, "gpu": gpu]
        case let .volumeSetup(surface, storage, addToFinder, mountMethod, outcome):
            return ["type": "volume-setup", "surface": surface, "storage": storage, "addToFinder": addToFinder,
                    "mountMethod": mountMethod, "outcome": outcome]
        case let .share(action, role, outcome): return ["type": "share", "action": action, "role": role, "outcome": outcome]
        case let .appUpdate(action, channel, trigger):
            return ["type": "app-update", "action": action, "channel": channel, "trigger": trigger]
        case let .deviceEnroll(method, outcome): return ["type": "device-enroll", "method": method, "outcome": outcome]
        case let .experiment(action, experiment):
            return ["type": "experiment", "action": action, "experiment": experiment]
        case let .experimentsOn(experiments): return ["type": "experiments-on", "experiments": experiments]
        }
    }

    func signals(_ v: [AppTelemetrySignal]) -> [Any] { v.map(Self.signalFrame) }

    /// `run_telemetry`: the usage events each reducer step means, through
    /// the Swift bindings the app's models call.
    func runTelemetry(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = [["at": "launch", "signals": signals(appTelemetryLaunched())]]
        for run in ["onboarding", "skipping", "no-usage-data"] {
            var st = appOnboardingInitial(installerMode: nil, identity: nil)
            for a in f[run] as! [[String: Any]] {
                let action = try appOnboardingActionFromJson(json: text(a))
                let sig = appTelemetryOnboarding(state: st, action: action)
                st = appOnboardingReduce(state: st, action: action)
                frames.append(["at": run, "action": a["type"] as! String, "page": "\(st.step)", "signals": signals(sig)])
            }
            frames.append(["at": run, "action": "finish", "signals": signals(appTelemetryOnboardingFinished(state: st))])
        }
        var creates = AppCreatesState(pending: [], deleting: [], powering: [])
        for step in f["creates"] as! [[String: Any]] {
            let a = step["action"] as! [String: Any]
            let action = createAction(a)
            let sig = appTelemetryCreates(state: creates, action: action, nowMs: Int64(step["now"] as! Int))
            creates = appCreatesReduce(state: creates, action: action)
            frames.append(["at": "creates", "action": a["type"] as! String, "signals": signals(sig)])
        }
        let storage = f["storage"] as! [String: Any]
        let storageInput = try appStorageInputFromJson(json: text(storage["input"]))
        for c in storage["cases"] as! [[String: Any]] {
            let sig = appTelemetryStorage(input: storageInput, state: try appStorageStateFromJson(json: text(c["state"])),
                                          action: try appStorageActionFromJson(json: text(c["action"])))
            frames.append(["at": "storage", "step": c["step"] as! String, "signals": signals(sig)])
        }
        let share = f["share"] as! [String: Any]
        let shareInput = try appShareInputFromJson(json: text(share["input"]))
        for c in share["cases"] as! [[String: Any]] {
            let sig = appTelemetryShare(input: shareInput, state: try appShareStateFromJson(json: text(c["state"])),
                                        action: try appShareActionFromJson(json: text(c["action"])))
            frames.append(["at": "share", "step": c["step"] as! String, "signals": signals(sig)])
        }
        for c in f["enroll"] as! [[String: Any]] {
            let sig = appTelemetryEnroll(state: try appEnrollStateFromJson(json: text(c["state"])),
                                         action: try appEnrollActionFromJson(json: text(c["action"])))
            frames.append(["at": "enroll", "step": c["step"] as! String, "signals": signals(sig)])
        }
        return frames
    }

    // MARK: - about

    func aboutLinkId(_ id: AppAboutLinkId) -> String {
        switch id {
        case .acknowledgements: "acknowledgements"
        case .privacy: "privacy"
        case .terms: "terms"
        case .issue: "issue"
        }
    }

    func runAbout(_ f: [String: Any]) throws -> [Any] {
        var input = f["input"] as! [String: Any]
        var frames: [Any] = []
        for case let step as [String: Any] in f["panes"] as! [Any] {
            for (k, v) in step["set"] as? [String: Any] ?? [:] { input[k] = v }
            let about = try appAboutInputFromJson(json: text(input))
            let v = appAboutView(input: about)
            let check = { (on: Bool, label: String, enabled: Bool) in
                "[\(on ? "x" : " ")] \(label)\(enabled ? "" : " (disabled)")"
            }
            var updates: Any = NSNull()
            if let u = v.updates {
                updates = [
                    "autoCheck": check(u.autoCheck, u.autoCheckLabel, true),
                    "autoInstall": check(u.autoInstall, u.autoInstallLabel, u.autoInstallEnabled),
                    "channel": "\(u.channelLabel) " + u.channels.map { "\($0.label)\($0.active ? "*" : "")" }
                        .joined(separator: " | "),
                    "help": u.channelHelp,
                    "check": "\(u.checkLabel) \(u.checkEnabled ? "on" : "off")",
                    "lastCheck": u.lastCheck,
                    "sparkleChannels": appAboutAllowedChannels(channel: about.channel),
                ] as [String: Any]
            }
            frames.append(["step": step["step"] as! String, "about": [
                "title": v.title,
                "version": v.versionLine,
                "links": v.links.map { "\(aboutLinkId($0.id)) \($0.label) -> \($0.url ?? "(bundled notices)")" },
                "copyright": v.copyright,
                "updates": updates,
            ] as [String: Any]])
        }
        for case let step as [String: Any] in f["launches"] as! [Any] {
            let plan = appAboutAfterLaunch(input: try appLaunchInputFromJson(json: text(step["input"])))
            frames.append(["step": step["step"] as! String,
                           "launch": "refresh \(plan.refresh) save \(plan.save ?? "-")"])
        }
        for case let step as [String: Any] in f["daemons"] as! [Any] {
            let restart = appAboutRestartDaemon(check: try appDaemonCheckFromJson(json: text(step["check"])))
            frames.append(["step": step["step"] as! String, "restartDaemon": restart])
        }
        for case let step as [String: Any] in f["reports"] as! [Any] {
            let notice = appAboutRefreshNotice(report: try appRefreshReportFromJson(json: text(step["report"])))
            frames.append(["step": step["step"] as! String, "notice": opt(notice)])
        }
        return frames
    }

    /// Launch at login (`run_launch_at_login`): Done's checkbox, Settings,
    /// General for each login-item input, and the launch plans.
    func runLaunchAtLogin(_ f: [String: Any]) throws -> [Any] {
        var frames: [Any] = []
        var st = appOnboardingInitial(installerMode: nil, identity: nil)
        for a in f["onboarding"] as! [[String: Any]] {
            st = appOnboardingReduce(state: st, action: try appOnboardingActionFromJson(json: text(a)))
            let v = appOnboardingView(state: st)
            if v.step == .done {
                frames.append(["onboarding": a["type"] as! String, "launchAtLogin": st.launchAtLogin,
                               "frame": onboardingFrame(v)])
            }
        }
        for step in f["settings"] as! [[String: Any]] {
            let input = try appSettingsInputFromJson(json: text([
                "signIn": ["kind": "idle"], "canSignOut": false, "menuBar": false, "defaultLocation": "local",
                "agentsBusy": false, "agentsPending": [], "loginItem": step["loginItem"] as Any,
            ] as [String: Any]))
            let general = appSettingsPage(input: input).sections.filter { $0.id == "general" }.flatMap(\.rows)
            frames.append(["step": step["step"] as! String, "general": general.map(settingRow)])
        }
        func status(_ word: String) -> AppLoginItemStatus {
            switch word {
            case "enabled": .enabled
            case "requiresApproval": .requiresApproval
            case "notFound": .notFound
            default: .notRegistered
            }
        }
        for step in f["launches"] as! [[String: Any]] {
            let plan = appLoginItemLaunchPlan(choice: step["choice"] as? Bool, onboarded: step["onboarded"] as! Bool,
                                              serves: step["serves"] as! Bool, status: status(step["status"] as! String))
            frames.append(["step": step["step"] as! String,
                           "launch": "register \(plan.register) record \(plan.record.map { String($0) } ?? "null")"])
        }
        return frames
    }

    // MARK: - experiments

    /// `run_experiments`: Settings, Experiments and what each switch decides,
    /// after each change.
    func runExperiments(_ f: [String: Any]) throws -> [Any] {
        let storage = appStorageSection(input: try appStorageInputFromJson(json: text(f["storageInput"])),
                                        state: appStorageInitial())
        let rows = try appSpaceRowsFromJson(json: text([f["space"]!]))
        let space = appRowsToSpaces(rows: rows, nowMs: Int64(f["now"] as! Int))[0]
        var x = try appExperimentsFromJson(json: "{}")
        var first = try experimentsFrame(f, storage: storage, space: space, x)
        first["step"] = "defaults"
        first["signals"] = [Any]()
        var frames: [Any] = [first]
        for change in f["changes"] as! [[String: Any]] {
            let before = x
            x = appExperimentsChoose(experiments: before, row: change["row"] as! String,
                                     option: change["option"] as! String)
            var frame = try experimentsFrame(f, storage: storage, space: space, x)
            frame["step"] = change["step"] as! String
            frame["signals"] = signals(appTelemetryExperimentsChanged(before: before, after: x))
            frames.append(frame)
        }
        return frames
    }

    func experimentsFrame(_ f: [String: Any], storage: AppSettingsSection, space: AppSpace,
                          _ x: AppExperiments) throws -> [String: Any] {
        var input = try appSettingsInputFromJson(json: text(f["settings"]))
        input.experiments = x
        let settings = appSettingsWithStorage(page: appSettingsPage(input: input), storage: storage, experiments: x)
        var st = appOnboardingReduce(state: appOnboardingInitial(installerMode: nil, identity: nil),
                                     action: .experimentsLoaded(experiments: x))
        var pages = ["\(st.step)"]
        for a in f["onboarding"] as! [[String: Any]] {
            st = appOnboardingReduce(state: st, action: try appOnboardingActionFromJson(json: text(a)))
            if pages.last != "\(st.step)" { pages.append("\(st.step)") }
        }
        var env = try appWizardEnvFromJson(json: text(f["env"]))
        env.experiments = x
        let wizard = appWizardView(state: appWizardInitial(env: env), env: env)
        let detail = windowDetail(appSpaceDetailWith(space: space, usage: nil, hostArch: nil, experiments: x))
        var menuInput = try appMenuInputFromJson(json: text(f["menu"]))
        menuInput.experiments = x
        let chrome = appMainChrome(input: AppChromeInput(identity: nil, cloudConfigured: false, canSignIn: false,
                                                         experiments: x))
        return [
            "tab": settingsFrame(appExperimentsPage(experiments: x)),
            "settings": settingsFrame(settings),
            "onboarding": ["pages": pages, "done": onboardingFrame(appOnboardingView(state: st))] as [String: Any],
            "actions": detail["actions"]!,
            "volumePage": opt(chrome.volumeLabel),
            "menu": menuFrame(appMenu(input: menuInput)),
            "wizard": ["placements": placementMenu(wizard), "fields": wizard.fields.map(\.id)] as [String: Any],
        ]
    }

    // MARK: - placement-picker

    /// `run_placement_picker`: the Run on menu; a step may patch the env
    /// (`env`: the experiments, the hosts) before its action.
    func runPlacementPicker(_ f: [String: Any]) throws -> [Any] {
        var envJson = f["env"] as! [String: Any]
        var env = try appWizardEnvFromJson(json: text(envJson))
        var wizard = appWizardInitial(env: env)
        var frames: [Any] = []
        for case let step as [String: Any] in f["steps"] as! [Any] {
            if let patch = step["env"] as? [String: Any] {
                for (k, v) in patch { envJson[k] = v }
                env = try appWizardEnvFromJson(json: text(envJson))
            }
            if let a = step["wizard"] as? [String: Any] {
                wizard = appWizardReduce(state: wizard, action: try appWizardActionFromJson(json: text(a)), env: env)
            }
            let v = appWizardView(state: wizard, env: env)
            frames.append([
                "step": step["step"] as! String,
                "wizard": yourCloudFrame(v),
                "on": appWizardCreateArgs(plan: v.plan).on,
            ])
        }
        return frames
    }
}
