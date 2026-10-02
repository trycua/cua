// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// This machine as an unattended-access host: the cua SDK's `Host` (live),
/// or `FixtureHost` (fixtures, tests). The form is validated in the app
/// core before any service is touched, as in the Tauri app.
public protocol HostRunning: AnyObject, Sendable {
    func status() async throws -> HostStatus
    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus
    func stopSharing() async throws -> HostStatus
    func startSharing() async throws -> HostStatus
    func remove() async throws
    /// Changes what this machine shares: its desktop, and Spaces for your
    /// other devices.
    func configure(change: HostSettingsChange) async throws -> HostStatus
}

extension CuaSDK.Host: HostRunning {}

/// An in-memory host: setup succeeds (or fails with `failSetup`), sharing
/// toggles. It never installs a service, touches launchd or the network.
public final class FixtureHost: HostRunning, @unchecked Sendable {
    public private(set) var current: HostStatus
    public private(set) var calls: [String] = []
    public var failSetup: String?

    public init(status: HostStatus? = nil) {
        current = status ?? FixtureHost.unconfigured
    }

    public static let unconfigured = HostStatus(
        configured: false, mode: nil, relayUrl: nil, directUrl: nil, envTokenPath: nil, machineId: nil,
        name: nil, sharing: false, serviceInstalled: false, serviceRunning: false, serviceKind: "process",
        online: nil, clients: [], allow: [], permissions: [], error: nil)

    public func status() async throws -> HostStatus { current }

    public func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        let request = try appHostValidateSetup(request: request)
        calls.append("setup:\(request.mode):\(request.direct ?? request.relayUrl ?? "https://relay.cua.ai")")
        if let failSetup { throw CuaError.Runtime(message: failSetup) }
        let relay = request.mode == "relay"
        let spare = request.profile == "spare"
        current = HostStatus(
            configured: true, mode: request.mode, relayUrl: relay ? (request.relayUrl ?? "https://relay.cua.ai") : nil,
            directUrl: relay ? nil : "http://\(request.direct ?? "")", envTokenPath: nil, machineId: "0123abcd4567",
            name: request.name ?? "This machine", sharing: true, serviceInstalled: true, serviceRunning: true,
            serviceKind: "launchd", online: true, clients: [], allow: request.allow ?? [],
            permissions: [
                HostPermission(id: "screen-recording", title: "Screen Recording",
                               settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                               instructions: "Turn on Cua Spaces"),
                HostPermission(id: "accessibility", title: "Accessibility",
                               settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                               instructions: "Turn on Cua Spaces"),
            ],
            error: nil,
            shareDesktop: request.shareDesktop ?? !spare,
            provideSpaces: request.provideSpaces ?? spare,
            maxSpaces: 4, maxMacosVms: 2)
        return current
    }

    public func configure(change: HostSettingsChange) async throws -> HostStatus {
        calls.append("configure:\(change.shareDesktop.map { "\($0)" } ?? "-"):\(change.provideSpaces.map { "\($0)" } ?? "-")")
        if let d = change.shareDesktop { current.shareDesktop = d }
        if let p = change.provideSpaces { current.provideSpaces = p }
        return current
    }

    public func stopSharing() async throws -> HostStatus {
        calls.append("stop")
        current.sharing = false
        current.clients = []
        return current
    }

    public func startSharing() async throws -> HostStatus {
        calls.append("start")
        current.sharing = true
        return current
    }

    public func remove() async throws {
        calls.append("remove")
        current = FixtureHost.unconfigured
    }
}

/// "This machine": the host's state, its page and the host setup form.
/// Every word and decision is the core's (`appHostPanel`, `appHostForm*`).
@MainActor
@Observable
public final class HostModel {
    let host: HostRunning?
    /// The host's state (`nil` until it answers).
    public private(set) var state: AppHostState?
    /// The host setup form, while it shows.
    public private(set) var form: AppHostFormState?
    public private(set) var busy = false
    public private(set) var error: String?
    /// The signed-in account (relay mode joins as it).
    public var identity: String?
    /// The account's token for relay setup.
    public var accountToken: (() async -> String?)?
    /// Called whenever the state changes (the roster's entry follows it).
    public var onChange: (() -> Void)?

    public init(host: HostRunning?) {
        self.host = host
    }

    public var panel: AppHostPanelView { appHostPanel(state: state) }
    public var formView: AppHostFormView? { form.map { appHostFormView(state: $0, identity: identity) } }
    /// What the roster's This machine entry reads.
    public var summaryInput: AppHostSummaryInput? { state.map { appHostSummaryInput(state: $0) } }

    func apply(_ status: HostStatus) {
        state = appHostState(status: status)
        onChange?()
    }

    public func refresh() async {
        guard let host else { return }
        do {
            apply(try await host.status())
        } catch {
            self.error = LiveSpacesBackend.words(error)
        }
    }

    // MARK: - Form

    public func openForm() {
        form = appHostFormInitial()
        error = nil
    }

    public func closeForm() { form = nil }

    public func send(_ action: AppHostFormAction) {
        guard let f = form else { return }
        form = appHostFormReduce(state: f, action: action)
    }

    /// "Set up for access": host setup with the core's validated request.
    public func submit() async {
        guard let host, let view = formView, view.canSubmit, let request = view.request else { return }
        send(.submit)
        do {
            let token: String? = request.mode == "relay" ? await accountToken?() : nil
            let status = try await host.setupRequest(request: request, accountToken: token)
            form = nil
            apply(status)
        } catch {
            send(.failed(error: LiveSpacesBackend.words(error)))
        }
    }

    // MARK: - Page buttons

    public func run(_ id: AppHostActionId) async {
        if id == .setUp {
            openForm()
            return
        }
        guard let host, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            // The two settings: the core says what each switch changes.
            if let change = appHostSettingChange(id: id) {
                apply(try await host.configure(change: HostSettingsChange(
                    shareDesktop: change.shareDesktop, provideSpaces: change.provideSpaces)))
                return
            }
            switch id {
            case .stopSharing: apply(try await host.stopSharing())
            case .resumeSharing: apply(try await host.startSharing())
            case .remove:
                try await host.remove()
                apply(try await host.status())
            case .setUp, .shareDesktop, .hideDesktop, .provideSpaces, .stopProvidingSpaces: break
            }
        } catch {
            self.error = LiveSpacesBackend.words(error)
        }
    }
}
