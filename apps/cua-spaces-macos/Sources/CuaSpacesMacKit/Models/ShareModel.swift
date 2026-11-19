// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// The Share sheet: the app core's `share.*` state machine over the SDK's
/// sharing calls. Sharing asks for presence in the SDK (the daemon's Touch
/// ID prompt) before anything reaches the relay.
@MainActor @Observable
public final class ShareModel {
    public let spaceId: String
    public let spaceName: String
    public var signedIn: Bool
    public var shareable: Bool
    /// In a team that can share; nil until read, or when it cannot be said
    /// (the server decides). Set by whoever opens the sheet.
    public var hasTeam: Bool?
    public private(set) var shares: [AppShareEntryInput] = []
    public private(set) var state = appShareInitial()
    private let backend: SpacesBackend

    public init(backend: SpacesBackend, spaceId: String, spaceName: String, signedIn: Bool, shareable: Bool) {
        self.backend = backend
        self.spaceId = spaceId
        self.spaceName = spaceName
        self.signedIn = signedIn
        self.shareable = shareable
    }

    var input: AppShareInput {
        AppShareInput(spaceId: spaceId, spaceName: spaceName, shares: shares, signedIn: signedIn, shareable: shareable)
    }

    public var view: AppShareSheetView { appShareView(input: input, state: state) }

    /// Why the sheet is closed: sharing is for teams, so an account known to
    /// be in none is sent to Teams. This only explains; the control plane
    /// and the relay refuse an outside share either way.
    public var gate: AppShareGate? { signedIn && shareable ? appShareGate(hasTeam: hasTeam) : nil }

    public func load() async {
        guard signedIn else { return }
        if let rows = try? await backend.shares(id: spaceId) { shares = rows }
    }

    /// Where usage events go (shared view-only or not, unshared, a role
    /// changed: never who; the app core's, like the Tauri app).
    public var telemetry: TelemetryRunning?
    /// A Space was shared with someone (the getting-started checklist).
    public var onShared: (() -> Void)?

    public func send(_ action: AppShareSheetAction) async {
        let wasBusy = state.busy
        state = appShareReduce(input: input, state: state, action: action)
        guard !wasBusy, state.busy, let request = state.request else { return }
        // Who it was shared with before the call (a role change or a new share).
        let before = input
        do {
            switch request {
            case let .share(space, who, role): shares = try await backend.share(id: space, who: who, role: role)
            case let .unshare(space, who): shares = try await backend.unshare(id: space, who: who)
            }
            telemetry?.record(appTelemetryShare(input: before, state: state, action: .done))
            if case .share = request { onShared?() }
            state = appShareReduce(input: input, state: state, action: .done)
        } catch {
            // A refusal for who is not in the team (or no team) reads in plain words.
            var who = ""
            if case let .share(_, w, _) = request { who = w }
            let raw = "\(error.localizedDescription) \(error)"
            let words = appShareErrorWords(error: raw, who: who) ?? error.localizedDescription
            let failed = AppShareSheetAction.failed(error: words)
            telemetry?.record(appTelemetryShare(input: before, state: state, action: failed))
            state = appShareReduce(input: input, state: state, action: failed)
        }
    }
}
