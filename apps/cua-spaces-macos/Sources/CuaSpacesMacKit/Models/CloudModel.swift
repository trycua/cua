// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// The user's own clouds (AWS, Google Cloud, Modal): what `cloud_status`
/// says, the "Your cloud" tile's connected clouds, and the "Connect a
/// cloud" sheet. The sheet's rows, words and requests are the app core's
/// (`appCloudConnect*`, the Tauri app draws the same); this runs the
/// daemon's `cloud_*` tools it asks for. Credentials never pass through
/// here: each cloud uses its own CLI sign-in.
/// Runs the daemon's `cloud_*` tools (the live backend; fixtures answer
/// with synthetic clouds).
public protocol CloudToolRunning: AnyObject, Sendable {
    func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any
}

extension LiveSpacesBackend: CloudToolRunning {
    public func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        try await agentsTool(tool, args)
    }
}

/// Synthetic clouds: AWS found and, once connected, the default.
extension FixtureSpacesBackend: CloudToolRunning {
    public func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        try await MainActor.run {
            fixtureCloudCalls.append(tool)
            switch tool {
            case "cloud_status":
                return ["default_on": fixtureCloudConnected ? "aws" : "local",
                        "providers": [Self.fixtureAws(connected: fixtureCloudConnected)]]
            case "cloud_test":
                return ["provider": "aws", "ok": true, "account": "1",
                        "checks": [["name": "credentials", "ok": true, "detail": "account 1"]]]
            case "cloud_connect":
                fixtureCloudConnected = true
                var row = Self.fixtureAws(connected: true)
                row["checks"] = [["name": "credentials", "ok": true, "detail": "account 1"]]
                return row
            default:
                throw FixtureError(message: "unknown tool \(tool)")
            }
        }
    }

    static func fixtureAws(connected: Bool) -> [String: Any] {
        ["name": "aws", "title": "AWS", "tier": "vm", "connected": connected,
         "credentials": ["found": true, "source": "~/.aws profile default"],
         "region": "us-west-2", "label": "AWS \u{00B7} us-west-2", "ttl_hours": 8,
         "kinds": [["image": "linux", "kind": "container", "supported": true,
                    "machine_type": "t4g.medium", "usd_per_hour": 0.0368],
                   ["image": "windows", "kind": "vm", "supported": false,
                    "reason": "Windows on AWS is not offered yet."]]]
    }
}

@MainActor @Observable
public final class CloudModel {
    private let tools: CloudToolRunning?
    /// `cloud_status` as it came (nil: not read, or no answer).
    var status: Any?
    public private(set) var state = appCloudConnectInitial()
    /// The sheet shows.
    public var showing = false
    /// Connected: the wizard reads the clouds again.
    public var onConnected: () -> Void = {}

    public init(tools: CloudToolRunning?) {
        self.tools = tools
    }

    private var statusJson: String {
        guard let status else { return "{}" }
        return (try? JSONSerialization.data(withJSONObject: status, options: [.fragmentsAllowed]))
            .map { String(decoding: $0, as: UTF8.self) } ?? "{}"
    }

    public var input: AppCloudConnectInput {
        (try? appCloudConnectInputFromStatusJson(json: statusJson)) ?? AppCloudConnectInput(providers: [])
    }

    /// The connected clouds, for the New Space wizard.
    public var clouds: [AppConnectedCloud] {
        (try? appConnectedCloudsFromStatusJson(json: statusJson)) ?? []
    }

    /// The default location's word (`default.on`), when the status says.
    public var defaultOn: String? {
        (status as? [String: Any])?["default_on"] as? String
    }

    public var view: AppCloudConnectView { appCloudConnectView(input: input, state: state) }

    /// Reads `cloud_status` again (a missing daemon or tool leaves none).
    public func refresh() async {
        guard let tools else { return }
        status = try? await tools.cloudTool("cloud_status", [:])
    }

    /// Opens the sheet on a fresh state.
    public func open() {
        state = appCloudConnectInitial()
        showing = true
        Task { await refresh() }
    }

    /// Applies `action`; Test and Connect run the request the core returns.
    /// "No cloud" (and any other no-request Connect) finishes the flow on
    /// the spot, with no `cloud_test` / `cloud_connect` call at all.
    public func send(_ action: AppCloudConnectAction) async {
        let before = input
        state = appCloudConnectReduce(input: before, state: state, action: action)
        switch action {
        case .test, .connect: break
        default: return
        }
        if view.done {
            showing = false
            return
        }
        guard let request = view.request else { return }
        guard let tools else {
            state = appCloudConnectReduce(input: before, state: state, action: .failed(error: "No cua daemon"))
            return
        }
        do {
            switch request {
            case let .test(target):
                let r = try await tools.cloudTool("cloud_test", Self.args(target)) as? [String: Any] ?? [:]
                let checks = (r["checks"] as? [[String: Any]] ?? []).map {
                    AppCloudCheckInput(name: $0["name"] as? String ?? "", ok: $0["ok"] as? Bool ?? false,
                                       detail: $0["detail"] as? String ?? "")
                }
                state = appCloudConnectReduce(input: before, state: state, action: .tested(
                    ok: r["ok"] as? Bool ?? false, account: r["account"] as? String ?? "", checks: checks))
            case let .connect(target, makeDefault):
                var args = Self.args(target)
                args["make_default"] = makeDefault
                let r = try await tools.cloudTool("cloud_connect", args) as? [String: Any] ?? [:]
                state = appCloudConnectReduce(input: before, state: state, action: .connected(
                    label: r["label"] as? String ?? target.provider))
                await refresh()
                showing = false
                onConnected()
            }
        } catch {
            state = appCloudConnectReduce(input: before, state: state,
                                          action: .failed(error: LiveSpacesBackend.words(error)))
        }
    }

    static func args(_ t: AppCloudTargetArgs) -> [String: Any] {
        var a: [String: Any] = ["provider": t.provider]
        if let v = t.profile { a["profile"] = v }
        if let v = t.region { a["region"] = v }
        if let v = t.project { a["project"] = v }
        if let v = t.environment { a["environment"] = v }
        return a
    }
}
