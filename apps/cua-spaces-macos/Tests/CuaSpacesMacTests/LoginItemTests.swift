// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Launch at login, on a fake login item: these tests never register,
/// unregister or read the Mac's real login items.
@MainActor
@Suite("Launch at login")
struct LoginItemTests {
    func general(_ m: AppModel) -> [AppSettingsRow] {
        m.settingsPage.sections.first { $0.id == "general" }?.rows ?? []
    }

    /// A model whose first run finished before launch at login existed.
    func onboardedModel(host: HostRunning? = nil, loginItem: LoginItemControlling) throws -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-login-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let state = dir.appendingPathComponent("onboarding.json")
        try Data(#"{"completed":true,"mode":"host"}"#.utf8).write(to: state)
        return AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil, clock: { fixtureNow }),
                        onboarding: OnboardingModel(statePath: state.path),
                        settingsPath: dir.appendingPathComponent("settings.json").path,
                        host: host, telemetry: FixtureTelemetry(), loginItem: loginItem)
    }

    func toggleOn(_ m: AppModel) -> Bool? {
        general(m).first { $0.id == "launch-at-login" }?.options.first { $0.id == "on" }?.active
    }

    @Test func noRowWithoutALoginItem() async {
        let m = ViewModelTests().makeModel()
        await m.loadSettings()
        #expect(general(m).first?.id == "notch")
    }

    @Test func theToggleShowsWhatTheSystemReportsAndChangesIt() async {
        let item = FixtureLoginItem(.enabled)
        let telemetry = FixtureTelemetry()
        let m = ViewModelTests().makeModel(loginItem: item, telemetry: telemetry)
        await m.loadSettings()
        #expect(general(m).first?.kind == .toggle)
        #expect(toggleOn(m) == true)
        await m.choose(row: "launch-at-login", option: "off")
        #expect(item.calls == ["unregister"])
        #expect(toggleOn(m) == false)
        #expect(m.settings.launchAtLogin == false, "the user's choice is saved")
        await m.choose(row: "launch-at-login", option: "on")
        #expect(item.calls == ["unregister", "register"])
        #expect(toggleOn(m) == true)
        #expect(m.settings.launchAtLogin == true)
        let features = telemetry.recorded.compactMap { s -> String? in
            if case let .feature(f) = s { return f }
            return nil
        }
        #expect(features == ["launch_at_login_off", "launch_at_login_on"])
    }

    @Test func approvalIsShownWithAButtonToLoginItems() async {
        let item = FixtureLoginItem()
        item.approval = true
        let m = ViewModelTests().makeModel(loginItem: item)
        await m.loadSettings()
        await m.choose(row: "launch-at-login", option: "on")
        #expect(m.loginItemStatus == .requiresApproval)
        let approve = general(m).first { $0.id == "launch-at-login-approve" }
        #expect(approve?.label == "Approve in System Settings \u{203a} Login Items")
        await m.press(row: "launch-at-login-approve")
        #expect(item.calls == ["register", "open-settings"])
    }

    @Test func aFailureShowsAndTheStatusIsReadBack() async {
        let item = FixtureLoginItem(.notRegistered)
        item.failure = "Operation not permitted"
        let m = ViewModelTests().makeModel(loginItem: item)
        await m.loadSettings()
        await m.choose(row: "launch-at-login", option: "on")
        #expect(toggleOn(m) == false, "never shows a state the system does not hold")
        #expect(general(m).contains { $0.id == "launch-at-login-error" && $0.label == "Operation not permitted" })
    }

    @Test func notFoundDisablesTheToggle() async {
        let m = ViewModelTests().makeModel(loginItem: FixtureLoginItem(.notFound))
        await m.loadSettings()
        #expect(general(m).first?.enabled == false)
    }

    @Test func doneAppliesTheCheckbox() async {
        let ticked = FixtureLoginItem()
        let a = ViewModelTests().makeModel(loginItem: ticked)
        #expect(a.onboarding.state.launchAtLogin, "ticked by default")
        a.onboarding.finish()
        #expect(ticked.calls == ["register"])
        #expect(a.settings.launchAtLogin == true)

        let unticked = FixtureLoginItem()
        let b = ViewModelTests().makeModel(loginItem: unticked)
        for a: AppOnboardingAction in [.start, .signinDone, .agentsDone(configured: []), .presentationDone,
                                       .driveContinue, .modeChosen(mode: .client), .launchAtLoginToggled(on: false)] {
            b.onboarding.send(a)
        }
        #expect(b.onboarding.view.launchAtLogin?.checked == false)
        b.onboarding.finish()
        #expect(unticked.calls == ["unregister"])
        #expect(b.settings.launchAtLogin == false)
    }

    @Test func atLaunchAnUnchosenServingMacTurnsOnAndAChoiceStands() async throws {
        // Provides Spaces (a spare machine), first run done before the setting.
        let host = FixtureHost()
        _ = try await host.setupRequest(request: AppHostSetupRequest(
            mode: "relay", relayUrl: nil, direct: nil, name: nil, allow: nil, profile: "spare",
            shareDesktop: nil, provideSpaces: nil), accountToken: nil)
        let serving = FixtureLoginItem()
        let m = try onboardedModel(host: host, loginItem: serving)
        #expect(m.onboarding.completed)
        await m.applyLaunchAtLogin()
        #expect(serving.calls == ["register"])
        #expect(m.settings.launchAtLogin == true)

        // Turned off by the user: stays off on the next launch.
        await m.choose(row: "launch-at-login", option: "off")
        await m.applyLaunchAtLogin()
        #expect(serving.calls == ["register", "unregister"])
        #expect(m.loginItemStatus == .notRegistered)
        let note = general(m).first { $0.id == "launch-at-login-note" }?.label
        #expect(note == "This machine provides Spaces, which stop after a restart until you open Cua Spaces.")

        // Not serving and never chosen: left alone.
        let idle = FixtureLoginItem()
        let n = try onboardedModel(loginItem: idle)
        await n.applyLaunchAtLogin()
        #expect(idle.calls.isEmpty)
        #expect(n.settings.launchAtLogin == nil)
    }

    @Test func aLoginLaunchIsRecognised() {
        let open = NSAppleEventDescriptor(eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEOpenApplication),
                                          targetDescriptor: nil, returnID: AEReturnID(kAutoGenerateReturnID),
                                          transactionID: AETransactionID(kAnyTransactionID))
        #expect(!LoginLaunch.isLoginLaunch(open))
        open.setParam(NSAppleEventDescriptor(enumCode: OSType(keyAELaunchedAsLogInItem)), forKeyword: AEKeyword(keyAEPropData))
        #expect(LoginLaunch.isLoginLaunch(open))
        #expect(!LoginLaunch.isLoginLaunch(nil))
    }
}
