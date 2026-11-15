// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Cua
import CuaBotsCore
import CuaBotsPhone
import CuaBotsRemote
import SwiftUI

// The phone screens in a phone-sized Mac window, against a real bot.
//
//   CUA_BOTS_PREVIEW_SPACE=local:bot-ada   a local Space by name (this Mac's registry)
//   CUA_BOTS_REMOTE=cuabots://direct?url=...&token=...   any spacesd, as the phone would
//   CUA_BOTS_PHONE_CAPTURE=<dir>          walk the screens, screenshot each, quit
//
// It uses the same `RemoteBot` the iPhone app uses; only how the spacesd
// client is found differs (the phone signs in to the relay).

@MainActor
final class PreviewDelegate: NSObject, NSApplicationDelegate {
    let model = PhoneModel()
    var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        switch AppAppearance.scheme {
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        default: break
        }
        let host = NSHostingView(rootView: PhoneFrame(model: model))
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 402, height: 874),
                         styleMask: [.titled, .closable], backing: .buffered, defer: false)
        w.title = "Cua Bots (iPhone)"
        w.contentView = host
        w.center()
        w.makeKeyAndOrderFront(nil)
        window = w
        NSApp.activate(ignoringOtherApps: true)
        Task { await connect() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func connect() async {
        let env = ProcessInfo.processInfo.environment
        do {
            let cua = try Cua.embedded()
            var bots: [RemoteBot] = []
            var label = ""
            if let link = env["CUA_BOTS_REMOTE"], let endpoint = RemoteEndpoint.parse(link) {
                bots = try await RemoteBot.connect(endpoint, cua: cua)
                label = endpoint.label
            } else if let name = env["CUA_BOTS_PREVIEW_SPACE"] {
                let client = try await cua.sandboxes().connect(name: name).spacesd(probeTimeoutMs: 5000)
                bots = try await RemoteBot.bots(on: client)
                label = "\(name) (direct)"
            }
            model.attach(bots, label: label)
            if let dir = env["CUA_BOTS_PHONE_CAPTURE"] {
                await PhoneTour(model: model, window: window!, directory: URL(fileURLWithPath: dir)).run()
            }
        } catch {
            model.connectionLabel = "Couldn't connect: \(error.localizedDescription)"
        }
    }
}

/// The phone screen at iPhone size.
struct PhoneFrame: View {
    @ObservedObject var model: PhoneModel
    var body: some View {
        PhoneRoot(model: model)
            .frame(width: 402, height: 874)
            .preferredColorScheme(AppAppearance.scheme)
    }
}

@MainActor
struct PhoneTour {
    let model: PhoneModel
    let window: NSWindow
    let directory: URL

    func run() async {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        guard await wait(120, { model.bots.first?.snapshot != nil }) else { log("no snapshot"); return done() }
        let bot = model.bots[0]
        await shot("p01-bots")
        model.path = [.chat(bot.botID)]
        await sleep(2)
        await shot("p02-chat")
        model.profileFor = .init(id: bot.botID)
        await sleep(1.5)
        await shotSheet("p03-profile")
        model.profileFor = nil
        await sleep(1)
        model.path = [.chat(bot.botID), .computer(bot.botID)]
        _ = await wait(30, { bot.frame != nil })
        await sleep(2)
        await shot("p04-computer")
        model.path = [.chat(bot.botID)]
        await sleep(1)
        let earlier = Set((bot.snapshot?.approvals ?? []).map(\.id))
        await bot.send(.message("Order the Flexispot E7 standing desk"))
        _ = await wait(240, { bot.snapshot?.approvals.contains { $0.state == .pending && !earlier.contains($0.id) } == true })
        await sleep(2)
        await shot("p05-approval-from-phone")
        model.path = [.approvals]
        await sleep(1)
        await shot("p06-approvals")
        let fresh = bot.snapshot?.approvals.first { $0.state == .pending && !earlier.contains($0.id) }
        if let a = fresh { await bot.send(.decide(approvalID: a.id, approve: true)) }
        model.path = [.chat(bot.botID)]
        _ = await wait(240, { bot.snapshot?.approvals.first { $0.id == fresh?.id }?.state == .approved && bot.snapshot?.busy == false })
        await sleep(3)
        await shot("p07-approved")
        await bot.send(.pause)
        _ = await wait(60, { bot.snapshot?.bot.isPaused == true })
        await sleep(1)
        await shot("p08-paused")
        await bot.send(.resume)
        _ = await wait(60, { bot.snapshot?.bot.isPaused == false })
        done()
    }

    func wait(_ s: Double, _ ok: () -> Bool) async -> Bool {
        let end = Date().addingTimeInterval(s)
        while Date() < end {
            if ok() { return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        log("timed out")
        return false
    }

    func sleep(_ s: Double) async { try? await Task.sleep(for: .milliseconds(Int(s * 1000))) }

    func shot(_ name: String) async {
        await sleep(0.6)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        p.arguments = ["-x", "-o", "-l", String(window.windowNumber), directory.appendingPathComponent("\(name).png").path]
        try? p.run()
        p.waitUntilExit()
        log("shot \(name)")
    }

    /// A sheet is its own window on macOS; photograph it.
    func shotSheet(_ name: String) async {
        await sleep(0.6)
        guard let sheet = window.attachedSheet else { return await shot(name) }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        p.arguments = ["-x", "-o", "-l", String(sheet.windowNumber), directory.appendingPathComponent("\(name).png").path]
        try? p.run()
        p.waitUntilExit()
        log("shot \(name)")
    }

    func log(_ s: String) {
        let url = directory.appendingPathComponent("phone-tour.log")
        let line = "[\(Date().formatted(date: .omitted, time: .standard))] \(s)\n"
        if let h = try? FileHandle(forWritingTo: url) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: url) }
    }

    func done() { NSApp.terminate(nil) }
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = PreviewDelegate()
    app.delegate = delegate
    app.run()
}
