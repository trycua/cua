// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Tips (CUA-2204): the sidebar's tip in the classic UI, Settings' Show
/// tips and Reset tips, and the New UI saving them through the bridge.
/// The rules are the core's (`tips::view`, `settings::choose`); these check
/// the app asks with the right facts and keeps what comes back.
@MainActor
struct TipsTests {
    private func settingsOnDisk(_ m: AppModel) -> AppSettings { appSettingsLoad(path: m.settingsPath) }

    @Test func theSidebarTipShowsOnceASecondComputerIsOnline() {
        let none = AppTipsState(off: false, done: [])
        #expect(AppModel.sidebarTip(machinesOnline: 1, tips: none) == nil)
        let tip = AppModel.sidebarTip(machinesOnline: 2, tips: none)
        #expect(tip?.id == "on-auto")
        #expect(tip?.text == "New Spaces can go to whichever computer has room.")
        #expect(tip?.code == "cua create --on auto")
        #expect(tip?.action.value == "cua create --on auto")
        #expect(tip?.action.doneLabel == "Copied")
        #expect(tip?.next == nil, "the sidebar's tip has only ×")
        #expect(AppModel.sidebarTip(machinesOnline: 2, tips: AppTipsState(off: true, done: [])) == nil, "tips off")
        #expect(AppModel.sidebarTip(machinesOnline: 2, tips: AppTipsState(off: false, done: ["on-auto"])) == nil, "closed")
    }

    @Test func closingTheTipKeepsItClosedAcrossLaunches() async {
        let m = ViewModelTests().makeModel()
        await m.refresh()
        #expect(m.sidebarTip == nil, "the fixtures have one computer online")
        m.sendTips(.close(id: "on-auto"))
        #expect(m.settings.tips.done == ["on-auto"])
        #expect(settingsOnDisk(m).tips.done == ["on-auto"])
        #expect(AppModel.sidebarTip(machinesOnline: 3, tips: m.settings.tips) == nil)
    }

    @Test func sharingASpaceFinishesTheShareTip() {
        let m = ViewModelTests().makeModel()
        m.sendTips(.shared)
        #expect(m.settings.tips.done == ["share"])
        m.sendTips(.shared)
        #expect(m.settings.tips.done == ["share"], "once")
    }

    @Test func showTipsAndResetTipsInSettings() async {
        let m = ViewModelTests().makeModel()
        m.sendTips(.close(id: "power"))
        await m.choose(row: "tips", option: "off")
        #expect(m.settings.tips.off)
        #expect(settingsOnDisk(m).tips.off)
        #expect(m.settingsPage.sections.flatMap(\.rows).first { $0.id == "tips" }?.options.first { $0.active }?.id == "off")
        await m.choose(row: "tips", option: "on")
        #expect(!m.settings.tips.off)
        #expect(m.settings.tips.done == ["power"], "turning them on brings back only the ones not done")
        await m.press(row: "reset-tips")
        #expect(m.settings.tips == AppTipsState(off: false, done: []))
        #expect(settingsOnDisk(m).tips.done.isEmpty)
    }

    @Test func theNewUISavesTipsThroughTheBridge() async throws {
        let m = ViewModelTests().makeModel()
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        let before = try #require(try await bridge.handle("settings.get", [:]) as? [String: Any])
        let tips = try #require(before["tips"] as? [String: Any])
        #expect(tips["off"] as? Bool == false)
        #expect(tips["done"] as? [String] == [])

        let after = try #require(try await bridge.handle("settings.update", [
            "tips": ["off": true, "done": ["try-agent"]],
        ]) as? [String: Any])
        #expect((after["tips"] as? [String: Any])?["done"] as? [String] == ["try-agent"])
        #expect(settingsOnDisk(m).tips == AppTipsState(off: true, done: ["try-agent"]))

        // A key left out stays.
        _ = try await bridge.handle("settings.update", [:])
        #expect(m.settings.tips.done == ["try-agent"])
        await #expect(throws: (any Error).self) { try await bridge.handle("settings.update", ["tips": "on"]) }

        // The core's rows through `settings.choose`, as the New UI's Settings page may send them.
        _ = try await bridge.handle("settings.choose", ["row": "reset-tips", "option": "press"])
        #expect(m.settings.tips == AppTipsState(off: false, done: []))
    }
}
