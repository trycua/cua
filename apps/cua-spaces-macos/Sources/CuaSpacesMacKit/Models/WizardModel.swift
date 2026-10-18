// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// The New Space wizard: the core's `AppWizardState` plus the environment
/// it is judged against. `view` is what the sheet renders.
@MainActor
@Observable
public final class WizardModel {
    public private(set) var state: AppWizardState
    public private(set) var env: AppWizardEnv

    public init(env: AppWizardEnv) {
        self.env = env
        self.state = appWizardInitial(env: env)
    }

    public var view: AppWizardView { appWizardView(state: state, env: env) }

    public func send(_ action: AppWizardAction) {
        state = appWizardReduce(state: state, action: action, env: env)
    }

    /// New facts about this machine or account (a cloud connected) while
    /// the sheet is open: the state stays where the person is.
    public func update(env: AppWizardEnv) {
        self.env = env
        send(.syncDefault(location: env.defaultLocation))
    }

    /// A fresh wizard for a new sheet.
    public func reset(env: AppWizardEnv) {
        self.env = env
        state = appWizardInitial(env: env)
    }

    /// "Add Space": runs the handshake when the core allows it.
    public func submitAddress(_ add: (String, String?, String?) async throws -> Void) async {
        guard let call = view.address.submit else { return }
        send(.submitAddress)
        do {
            try await add(call.url, call.token, call.name)
        } catch {
            send(.addressFailed(error: LiveSpacesBackend.words(error)))
        }
    }
}
