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

    public func load() async {
        guard signedIn else { return }
        if let rows = try? await backend.shares(id: spaceId) { shares = rows }
    }

    /// Where usage events go (shared view-only or not, unshared, a role
    /// changed: never who; the app core's, like the Tauri app).
    public var telemetry: TelemetryRunning?

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
            state = appShareReduce(input: input, state: state, action: .done)
        } catch {
            let failed = AppShareSheetAction.failed(error: "\(error.localizedDescription)")
            telemetry?.record(appTelemetryShare(input: before, state: state, action: failed))
            state = appShareReduce(input: input, state: state, action: failed)
        }
    }
}
