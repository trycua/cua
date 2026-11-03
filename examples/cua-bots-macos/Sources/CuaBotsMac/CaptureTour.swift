// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaBotsCore
import Foundation
import SwiftUI

/// Screenshots the running app, one per surface, then quits.
///
/// `CUA_BOTS_CAPTURE=<dir>` walks a real bot through its life against a real
/// Space: create it, let it introduce itself, give it work, schedule a
/// routine, answer an approval and a sign-in, open its computer, pause it.
/// It drives the model (the same calls the buttons make) rather than the
/// pointer, and photographs the composited window with
/// `screencapture -l <window>`, which includes the live stream's layer.
@MainActor
struct CaptureTour {
    let model: AppModel
    let directory: URL

    func run() async {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        log("tour started")
        await pause(1.5)

        model.route = .newBot
        model.draft.name = "Ada"
        await shot("01-new-bot-meet")
        model.draft.step = 1
        model.draft.avatar = AvatarConfig(color: .violet, eyes: .star, ears: .scalloped)
        model.draft.customizedLook = true
        await shot("02-new-bot-look")
        model.draft.step = 2
        model.draft.harness = .claudeCode
        await shot("03-new-bot-where")

        model.createFromDraft()
        await pause(3)
        await shot("04-setting-up")
        guard await waitFor(900, "the introduction", { botMessages() >= 2 && !busy() }) else { return finish() }
        await shot("05-introduced")

        send("Research standing desks and write me a short summary")
        await pause(6)
        await shot("06-working")
        guard await waitFor(240, "the research", { hasResult() }) else { return finish() }
        await pause(1)
        await shot("07-research-ready")

        model.showProfile = false
        model.showComputer = true
        await pause(10)
        await shot("08-computer")
        model.popOut(bot()!)
        await pause(4)
        await shotPiP("09-pip")
        model.showComputer = false
        model.showProfile = true

        send("Every morning at 9, summarize what changed on news.ycombinator.com")
        guard await waitFor(180, "the schedule", { !(model.store.tasks(for: botID()).filter { $0.schedule != nil }.isEmpty) }) else { return finish() }
        await pause(1)
        await shot("10-scheduled-in-chat")
        model.route = .scheduled
        await shot("11-scheduled")
        model.route = .bot(botID())

        send("Order the Flexispot E7 standing desk")
        guard await waitFor(180, "the approval", { !model.store.pendingApprovals(for: botID()).isEmpty }) else { return finish() }
        await pause(1)
        await shot("12-approval")
        model.route = .approvals
        await shot("13-approvals")
        model.route = .bot(botID())
        let before = botMessages()
        if let a = model.store.pendingApprovals(for: botID()).first { model.decide(a, true) }
        guard await waitFor(180, "the reply to the approval", { botMessages() > before && !busy() }) else { return finish() }
        await pause(1)
        await shot("14-approved")

        send("Log me into github.com and check my notifications")
        guard await waitFor(180, "the sign-in request", { model.store.pendingApprovals(for: botID()).contains { if case .login = $0.source { return true }; return false } }) else { return finish() }
        await pause(1)
        await shot("15-sign-in-card")
        if let a = model.store.pendingApprovals(for: botID()).first { model.signInFor = a }
        await pause(1.5)
        await shot("16-sign-in-sheet")
        model.signInFor = nil

        model.editingAvatar = true
        await pause(1)
        await shot("17-edit-look")
        model.editingAvatar = false
        model.showingRules = true
        await pause(1)
        await shot("18-custom-rules")
        model.showingRules = false
        model.showingMemory = true
        await pause(1)
        await shot("19-memory")
        model.showingMemory = false

        await model.store.pause(botID())
        await pause(2)
        await shot("20-paused")
        await model.store.resume(botID())
        await pause(3)

        model.route = .outputs
        await shot("21-outputs")
        model.route = .bot(botID())
        model.showProfile = true
        await shot("22-profile")
        finish()
    }

    // MARK: - Helpers

    func bot() -> Bot? { model.store.bots.first }
    func botID() -> String { bot()?.id ?? "" }
    func busy() -> Bool { model.store.isBusy(botID()) }
    func botMessages() -> Int { model.store.messages(for: botID()).filter { $0.role == .bot }.count }
    func hasResult() -> Bool {
        model.store.messages(for: botID()).contains { if case .result = $0.kind { return true }; return false }
    }

    func send(_ text: String) {
        log("send: \(text)")
        model.send(text)
    }

    func waitFor(_ seconds: Double, _ what: String, _ ok: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if ok() { log("ready: \(what)"); return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        log("timed out waiting for \(what)")
        await shot("zz-timeout-\(what.replacingOccurrences(of: " ", with: "-"))")
        return false
    }

    func pause(_ s: Double) async { try? await Task.sleep(for: .milliseconds(Int(s * 1000))) }

    func shot(_ name: String) async {
        await pause(0.8)
        // The main window: the largest visible window that isn't a panel. A
        // sheet is part of its window's picture.
        guard let window = NSApp.windows.filter({ $0.isVisible && !($0 is NSPanel) && $0.sheetParent == nil })
            .max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height })
        else { log("no window for \(name)"); return }
        capture(window.windowNumber, name)
    }

    func shotPiP(_ name: String) async {
        await pause(0.5)
        guard let panel = NSApp.windows.first(where: { $0 is NSPanel && $0.isVisible }) else {
            log("no PiP panel for \(name)")
            return
        }
        capture(panel.windowNumber, name)
    }

    func capture(_ windowNumber: Int, _ name: String) {
        let out = directory.appendingPathComponent("\(name).png").path
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        p.arguments = ["-x", "-o", "-l", String(windowNumber), out]
        try? p.run()
        p.waitUntilExit()
        log("shot \(name)")
    }

    func log(_ s: String) {
        let line = "[\(Date().formatted(date: .omitted, time: .standard))] \(s)\n"
        let url = directory.appendingPathComponent("tour.log")
        if let h = try? FileHandle(forWritingTo: url) {
            h.seekToEndOfFile()
            h.write(Data(line.utf8))
            try? h.close()
        } else {
            try? Data(line.utf8).write(to: url)
        }
    }

    /// `CUA_BOTS_CAPTURE_STAY=1` keeps the app running after the tour, so
    /// the phone preview can be photographed against it.
    func finish() {
        log("tour finished")
        // Window and split-view state lands in the real preferences (they
        // don't follow HOME); a capture run leaves none behind.
        UserDefaults.standard.removePersistentDomain(forName: Bundle.main.bundleIdentifier ?? "CuaBots")
        if ProcessInfo.processInfo.environment["CUA_BOTS_CAPTURE_STAY"] == "1" { return }
        NSApp.terminate(nil)
    }
}
