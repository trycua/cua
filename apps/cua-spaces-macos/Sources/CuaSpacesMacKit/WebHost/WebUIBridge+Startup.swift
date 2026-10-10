// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation
import Observation

/// The launch on New UI's startup screen (`ops/startup.ts` in the web
/// bridge): `startup.get` and `startup.act` `{action}` answer the
/// `StartupState` (`StartupModel.copy` and its phase), and
/// `startup.changed` (with the state) goes out whenever it changes.
extension WebUIBridge {
    func routeStartup(_ method: String, _ args: [String: Any]) throws -> Any? {
        switch method {
        case "startup.get":
            return Self.startupState(model.startup)
        case "startup.act":
            guard let word = args["action"] as? String, let action = StartupModel.Action(rawValue: word) else {
                throw Failure.badArgs("startup.act: action is allowAccess, tryAgain or signInAgain")
            }
            // The keychain prompt shows over the app that asked: be in front.
            if action != .signInAgain { NSApp.activate() }
            model.startup.act(action)
            return Self.startupState(model.startup)
        default:
            return nil
        }
    }

    /// Tells the page each time the launch moves on.
    func followStartup() {
        let state = withObservationTracking { Self.startupState(model.startup) } onChange: { [weak self] in
            DispatchQueue.main.async { self?.followStartup() }
        }
        host?.emit("startup.changed", payload: state)
    }

    static func startupState(_ startup: StartupModel) -> [String: Any] {
        let copy = startup.copy
        return [
            "phase": phaseWord(startup.phase),
            "slow": startup.slow,
            "title": copy.title,
            "body": copy.body,
            "actions": copy.actions.map(\.rawValue),
        ]
    }

    static func phaseWord(_ phase: StartupModel.Phase) -> String {
        switch phase {
        case .starting: return "starting"
        case .needsKeychain: return "needsKeychain"
        case .waitingForKeychain: return "waitingForKeychain"
        case .keychainDenied: return "keychainDenied"
        case .startFailed: return "startFailed"
        case .ready: return "ready"
        }
    }
}
