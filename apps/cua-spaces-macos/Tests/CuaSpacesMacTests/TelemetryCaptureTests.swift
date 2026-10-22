// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// One first run to a first ready Space through the app's own models, the
/// bridge calls the web onboarding makes (`telemetry.track`,
/// `settings.choose`, `onboarding.complete`) and the live telemetry path
/// (`LiveTelemetry`, the FFI, the SDK's client and its HTTP sender), sent to
/// a loopback capture endpoint. The page's events are the core's, derived
/// here from the parity flow the way the page derives them.
///
/// `CUA_TELEMETRY_CAPTURE_SHARE=off` turns Welcome's usage-data switch off
/// first: then nothing at all may reach the endpoint.
///
/// Opt-in: `CUA_TELEMETRY_CAPTURE_TEST=1` with a throwaway `CUA_HOME`,
/// `CUA_TELEMETRY_FORBID_NETWORK=1` and `CUA_TELEMETRY_ENDPOINT` on
/// 127.0.0.1 (the forbid switch refuses any other host). Leave
/// `CUA_TELEMETRY`, `DO_NOT_TRACK` and `CI` unset: the environment wins over
/// the setting, so the switch would be locked (this test checks that it
/// then shows as locked rather than pretending to turn anything off). The Spaces are the fixture backend's; nothing touches the
/// host, the account or `~/.cua`.
@Suite("Telemetry capture (opt-in)",
       .enabled(if: ProcessInfo.processInfo.environment["CUA_TELEMETRY_CAPTURE_TEST"] == "1"))
@MainActor
struct TelemetryCaptureTests {
    @Test func firstRunToAFirstReadySpace() async throws {
        let env = ProcessInfo.processInfo.environment
        let home = try #require(env["CUA_HOME"])
        let real = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".cua").standardized.path
        try #require(URL(fileURLWithPath: home).standardized.path != real, "a throwaway CUA_HOME, never ~/.cua")
        try #require(env["CUA_TELEMETRY_FORBID_NETWORK"] == "1")
        let endpoint = try #require(env["CUA_TELEMETRY_ENDPOINT"])
        try #require(endpoint.hasPrefix("http://127.0.0.1:"))

        // What the app does at launch (before the notice: `app_launched` waits).
        #expect(appTelemetryStart(version: "0.2.0", onboardingEligible: true) == false)
        let telemetry = LiveTelemetry()
        let share = env["CUA_TELEMETRY_CAPTURE_SHARE"] != "off"
        let onboarding = OnboardingModel(statePath: nil)
        let settings = URL(fileURLWithPath: home).appendingPathComponent("settings.json").path
        let backend = FixtureSpacesBackend()
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil), onboarding: onboarding,
                             settingsPath: settings, telemetry: telemetry)
        let bridge = WebUIBridge(model: model, allowedOrigins: [])

        /// The page's `telemetry.track`.
        func track(_ signals: [AppTelemetrySignal]) async throws {
            guard !signals.isEmpty else { return }
            _ = try await bridge.handle("telemetry.track", ["signals": signals.map(ParityTests.signalFrame)])
        }

        // The first run, page by page (the parity flow's run).
        let flow = try JSONSerialization.jsonObject(with: Data(contentsOf: ParityTests.dir
            .appendingPathComponent("telemetry-funnel.json"))) as! [String: Any]
        let status = telemetry.status()
        if let forced = env["CUA_TELEMETRY"] {
            // The environment decides: the switch is locked and says why.
            #expect(status.enabled == false)
            #expect(status.lockedBy?.contains("CUA_TELEMETRY") == true, "CUA_TELEMETRY=\(forced)")
            try #require(share, "the off case must run without CUA_TELEMETRY (it overrides the setting)")
        } else {
            #expect(status.enabled == true, "on by default")
        }
        var state = appOnboardingInitial(installerMode: nil, identity: nil)
        func step(_ action: AppOnboardingAction) async throws {
            try await track(appTelemetryOnboarding(state: state, action: action))
            state = appOnboardingReduce(state: state, action: action)
        }
        try await step(.welcomeShown)
        // Welcome's usage-data switch (the page's `settings.choose`).
        if !share { _ = try await bridge.handle("settings.choose", ["row": "telemetry", "option": "off"]) }
        // Nothing is queued while Welcome shows.
        #expect((try? JSONSerialization.jsonObject(with: Data(telemetryShowLast(limit: 100).utf8)) as? [Any])?.isEmpty ?? true)
        // The run's pages (the setting was read above, from this machine).
        for a in flow["onboarding"] as! [[String: Any]] where a["type"] as? String != "telemetry-loaded" {
            let data = try JSONSerialization.data(withJSONObject: a)
            try await step(try appOnboardingActionFromJson(json: String(decoding: data, as: UTF8.self)))
        }
        try await track(appTelemetryOnboardingFinished(state: state))
        _ = try await bridge.handle("onboarding.complete", ["mode": "client"])
        #expect(onboarding.completed)

        // New Space: open the panel, create the default Linux Space, wait
        // until it is ready.
        await model.openNewSpace()
        model.create(model.wizard.view.plan)
        for _ in 0..<200 {
            if backend.created.count == 1, model.creates.pending.isEmpty { break }
            try await Task.sleep(for: .milliseconds(25))
        }
        #expect(backend.created.count == 1)

        // Out to the capture endpoint (bounded).
        telemetryFlush(timeoutMs: 5000)
        let sent = (try? JSONSerialization.jsonObject(with: Data(telemetryShowLast(limit: 100).utf8))) as? [[String: Any]] ?? []
        let names = sent.compactMap { ($0["payload"] as? [String: Any])?["event"] as? String }
        if !share {
            #expect(names.isEmpty, "\(names)")
            #expect(telemetryStatus().enabled == false, "the machine's setting is off")
            return
        }
        #expect(names.contains("cua_first_run"))
        #expect(names.contains("cua_onboarding_page"))
        #expect(names.contains("cua_space_create"))
        #expect(sent.allSatisfy { ($0["status"] as? String) == "sent" }, "\(sent.map { $0["status"] ?? "" })")
    }
}
