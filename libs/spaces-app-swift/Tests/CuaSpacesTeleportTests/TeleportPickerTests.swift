// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpacesFFI
import Foundation
import Testing
@testable import CuaSpacesTeleport

/// A fixture host: canned catalog rows, plans and run events. Nothing on the
/// real machine is read.
final class FakeTeleportHost: TeleportAppHost, @unchecked Sendable {
    var entries: [TeleportCatalogEntry]
    var planned: [(String, TeleportPlanOptions)] = []
    var consents: [TeleportConsent] = []
    var planResult: TeleportPlan?
    var failCatalog = false
    var runError: Error?

    init(entries: [TeleportCatalogEntry]) { self.entries = entries }

    func catalog() async throws -> [TeleportCatalogEntry] {
        if failCatalog { throw CocoaError(.fileReadNoPermission) }
        return entries
    }
    func plan(_ entry: TeleportCatalogEntry, _ options: TeleportPlanOptions) async throws -> TeleportPlan {
        planned.append((entry.id, options))
        return planResult ?? fixturePlan(entry, sensitive: false)
    }
    func run(_ plan: TeleportPlan, consent: TeleportConsent,
             onEvent: @escaping @Sendable (TeleportRunEvent) -> Void) async throws -> TeleportRunReport {
        consents.append(consent)
        if let runError { throw runError }
        onEvent(TeleportRunEvent(step: 0, steps: 2, kind: "install", phase: "started", detail: "", doneBytes: 0, totalBytes: 0))
        return TeleportRunReport(appId: plan.app.id, installed: ["vscode"], sent: [], imported: [], skipped: [], launched: true)
    }
    func icon(_ entry: TeleportCatalogEntry) -> Data? { entry.id == "vscode" ? Data([0x89]) : nil }
    func entry(forDroppedPath path: String) throws -> TeleportCatalogEntry {
        guard let e = entries.first(where: { $0.hostPath == path }) else { throw CocoaError(.fileNoSuchFile) }
        return e
    }
    func parseDrop(_ items: [String]) -> TeleportDrop {
        let apps = items.filter { $0.hasSuffix(".app") }
        let files = items.filter { !$0.hasSuffix(".app") }
        return TeleportDrop(kind: apps.isEmpty ? (files.isEmpty ? "empty" : "files") : "app",
                            apps: apps, files: files, urls: [], ignored: [])
    }
}

/// `sensitiveGroups`: the opt-ins (sign-ins, passwords, history) the
/// signed-in state move offers; none unless a test needs them.
func fixtureEntry(_ id: String, _ name: String, _ c: TeleportCapability, moves: [TeleportMove],
                  recent: UInt64? = nil, reason: String? = nil,
                  sensitiveGroups: [TeleportSensitiveGroup] = []) -> TeleportCatalogEntry {
    TeleportCatalogEntry(id: id, name: name, hostPath: "/fixture/\(name).app", hostAppId: nil, version: nil,
                         capability: c, reason: reason, moves: moves, providerId: nil,
                         sensitiveGroups: sensitiveGroups, installSource: nil,
                         installId: nil, installVersion: nil, launchBin: nil, lastUsedMs: recent, json: "{}")
}

func fixturePlan(_ e: TeleportCatalogEntry, sensitive: Bool) -> TeleportPlan {
    TeleportPlan(app: e, spaceId: "space://direct/127.0.0.1:1", moves: .appOnly,
                 steps: [TeleportPlanStep(kind: "install", summary: "Install vscode (pinned, verified)")],
                 consent: [TeleportConsentItem(kind: sensitive ? .secret : .install, key: "k", label: "L",
                                               detail: "d", bytes: 1, sensitive: sensitive)],
                 sensitive: sensitive, totalBytes: 1, warnings: [], relayUnsealed: false, json: "{}")
}

let vscode = fixtureEntry("vscode", "Visual Studio Code", .installOnly, moves: [.appOnly, .appWithFiles])
let firefox = fixtureEntry("firefox", "Firefox", .full, moves: [.appOnly, .appWithFiles, .appWithState], recent: 5)
let safari = fixtureEntry("com.apple.Safari", "Safari", .unsupported, moves: [], reason: "no Linux build")

@Suite @MainActor struct TeleportPickerTests {
    /// The line under the bar reads the SDK's step events through the app
    /// core: the Keychain prompt is named before it appears, and bytes show
    /// while they move.
    @Test func theRunSaysWhichStepItIsOn() {
        func ev(_ phase: String, _ detail: String, _ done: UInt64 = 0, _ total: UInt64 = 0) -> TeleportRunEvent {
            TeleportRunEvent(step: 0, steps: 1, kind: "state", phase: phase, detail: detail,
                             doneBytes: done, totalBytes: total)
        }
        let reading = "Reading Chrome cookies (macOS will ask for Keychain access)\u{2026}"
        var events = [ev("started", "Preparing the sign-in")]
        #expect(appTeleportRunStatus(events: events) == "Preparing the sign-in")
        events.append(ev("progress", reading))
        #expect(appTeleportRunStatus(events: events) == reading)
        events.append(ev("progress", "Uploading", 12 << 20, 80 << 20))
        #expect(appTeleportRunStatus(events: events) == "Uploading 12 / 80 MB")
        events.append(ev("progress", "Importing into the Space"))
        #expect(appTeleportRunStatus(events: events) == "Importing into the Space")
        events.append(TeleportRunEvent(step: 1, steps: 1, kind: "done", phase: "done", detail: "Chrome",
                                       doneBytes: 0, totalBytes: 0))
        #expect(appTeleportRunStatus(events: events) == nil)
    }

    /// The Keyvault refuses session teleport from an embedded SDK by design
    /// (`requires_cua_app`); the model shows the Install Cua prompt instead.
    @Test func requiresCuaAppBecomesTheInstallPrompt() async throws {
        let host = FakeTeleportHost(entries: [firefox, vscode])
        host.runError = CuaSDK.CuaError.TeleportRefused(
            message: "requires_cua_app: teleport goes through the Cua Keyvault, which needs the Cua app")
        let m = TeleportPickerModel(host: host, spaceName: "Dev")
        await m.load()
        m.choose("vscode")
        await m.makePlan()
        await m.confirm()
        #expect(m.step == .error)
        let prompt = try #require(m.installPrompt)
        #expect(prompt.title == "Install Cua to teleport your session")
        #expect(prompt.message == "The Cua app keeps your logins in its Keyvault and asks you before sharing them.")
        #expect(prompt.actionLabel == "Install Cua")
        #expect(prompt.url == URL(string: "https://cua.ai/install"))
        m.back()
        #expect(m.step == .consent && m.installPrompt == nil)
        // Installed but not running: open it. Other failures stay raw.
        #expect(InstallCuaPrompt.detect("requires_cua_app: Cua is installed but not running")?.url
            == URL(string: "cua://keyvault"))
        #expect(InstallCuaPrompt.detect(CuaSDK.CuaError.TeleportRefused(message: "denied")) == nil)
    }

    @Test func pickOptionsConsentRunDone() async throws {
        let host = FakeTeleportHost(entries: [firefox, vscode, safari])
        let m = TeleportPickerModel(host: host, spaceName: "Dev")
        await m.load()
        #expect(m.step == .pick)
        #expect(m.selectedId == "firefox")
        #expect(m.sections.map(\.title) == ["Recent", "Apps", "Not available"])
        #expect(m.icons["vscode"] != nil)
        m.query = "visual"
        #expect(m.selectedId == "vscode")
        m.choose("com.apple.Safari")
        #expect(m.step == .pick)
        m.choose()
        #expect(m.step == .options)
        #expect(m.move == .appOnly)
        m.setMove(.appWithFiles)
        #expect(!m.canPlan)
        m.addFiles(["/tmp/project", "/tmp/project"])
        #expect(m.files == ["/tmp/project"])
        m.setMove(.appWithState)
        #expect(m.move == .appWithFiles)
        await m.makePlan()
        #expect(m.step == .consent)
        #expect(host.planned.first?.1.files == ["/tmp/project"])
        await m.confirm()
        #expect(m.step == .done)
        #expect(m.report?.launched == true)
        #expect(host.consents == [TeleportConsent(approved: true, acknowledgeSensitive: false)])
    }

    @Test func secretsNeedAcknowledgementAndErrorsGoBack() async throws {
        let host = FakeTeleportHost(entries: [firefox])
        host.planResult = fixturePlan(firefox, sensitive: true)
        let m = TeleportPickerModel(host: host, spaceName: "Dev")
        m.preselect(firefox)
        #expect(m.step == .options)
        await m.makePlan()
        #expect(!m.canConfirm)
        await m.confirm()
        #expect(m.step == .consent)
        m.acknowledged = true
        await m.confirm()
        #expect(host.consents.last?.acknowledgeSensitive == true)

        let unsupported = TeleportPickerModel(host: host, spaceName: "Dev")
        unsupported.preselect(safari)
        #expect(unsupported.step == .error)
        #expect(unsupported.error?.contains("no Linux build") == true)

        host.failCatalog = true
        let failing = TeleportPickerModel(host: host, spaceName: "Dev")
        await failing.load()
        #expect(failing.step == .error)
        failing.back()
        #expect(failing.step == .loading)
        #expect(TeleportPickerModel.defaultMove(vscode, files: ["/a"]) == .appWithFiles)
    }

    @Test func dropsPreselectAppsAndLeaveFilesAlone() {
        let host = FakeTeleportHost(entries: [vscode])
        let app = URL(fileURLWithPath: "/fixture/Visual Studio Code.app")
        let note = URL(fileURLWithPath: "/tmp/note.txt")
        #expect(TeleportDropHandler.outcome(for: [app, note], host: host) == .app(vscode, files: ["/tmp/note.txt"]))
        #expect(TeleportDropHandler.outcome(for: [note], host: host) == .files([note]))
        #expect(TeleportDropHandler.outcome(for: [], host: host) == .none)
    }

    @Test func windowDropsCaptureCommitAndIgnoreUnsupported() {
        let w = TeleportWindow(windowId: 7, pid: 70, appName: "Visual Studio Code", title: "main.rs", bundlePath: nil)
        var s = WindowDropState()
        let start = s.apply(TeleportWindowDragEvent(phase: "start", x: 0, y: 0, window: w, app: vscode))
        #expect(start.capture == 7)
        s.thumbnail = Data([1, 2, 3])
        s.overId = "space://direct/1"
        let end = s.apply(TeleportWindowDragEvent(phase: "end", x: 1, y: 1, window: w, app: vscode))
        #expect(end.commit?.targetId == "space://direct/1")
        #expect(!s.active && s.thumbnail == nil)
        let ignored = s.apply(TeleportWindowDragEvent(phase: "start", x: 0, y: 0, window: w, app: safari))
        #expect(ignored.capture == nil && !s.active)
    }
}

/// The real SDK over fixture app bundles in a temp directory (never the
/// machine's /Applications; `CUA_ENV_TEST_SANDBOX=1` refuses those).
@Suite struct TeleportSDKCatalogTests {
    @Test func realCatalogOverFixtureBundles() async throws {
        setenv("CUA_ENV_TEST_SANDBOX", "1", 1)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("cua-teleport-swift-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        for (name, id) in [("Visual Studio Code", "com.microsoft.VSCode"), ("Firefox", "org.mozilla.firefox"),
                           ("Fixture Notes", "com.example.notes")] {
            let c = root.appendingPathComponent("\(name).app/Contents")
            try FileManager.default.createDirectory(at: c, withIntermediateDirectories: true)
            try """
            <?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
            <key>CFBundleIdentifier</key><string>\(id)</string><key>CFBundleName</key><string>\(name)</string></dict></plist>
            """.write(to: c.appendingPathComponent("Info.plist"), atomically: true, encoding: .utf8)
        }
        let t = try Cua.embedded().teleport()
        let rows = try await t.catalog(options: TeleportCatalogOptions(
            roots: [root.path], spaceOs: "linux", spaceArch: "aarch64",
            recentsPath: root.appendingPathComponent("r.json").path))
        #expect(rows.map(\.id) == ["firefox", "vscode", "com.example.notes"])
        #expect(rows.map(\.capability) == [.full, .installOnly, .unsupported])
        let drop = t.parseDrop(items: [root.appendingPathComponent("Firefox.app").absoluteString])
        #expect(drop.kind == "app")
        #expect(try t.catalogEntryForPath(path: drop.apps[0], options: nil).id == "firefox")
        await #expect(throws: (any Error).self) { try await t.catalog(options: nil) }
    }
}
