// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
@testable import CuaSpacesMacKit
import Foundation
import AppKit
import Testing
import SwiftUI

/// Scripted provider completions; no service, account, or desktop is contacted.
private final class ApprovalBackend: SpacesBackend, @unchecked Sendable {
    var result: Result<[AppSpaceRow], Error> = .success(FixtureSpacesBackend.sample)
    var failFirst = false
    var holdFirst = false
    private var held: CheckedContinuation<Void, Never>?
    func release() { lock.withLock { held?.resume(); held = nil } }
    private let lock = NSLock()
    private var ids: [String] = []
    var requestedIds: [String] { lock.withLock { ids } }
    var isHeld: Bool { lock.withLock { held != nil } }
    func rows() async throws -> [AppSpaceRow] { try result.get() }
    func create(_ args: AppCreateSpaceArgs, createId: String, progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String { fatalError("unexpected create") }
    func cancelCreate(createId: String) async throws {}
    func add(url: String, token: String?, name: String?) async throws {}
    func remove(id: String, removeOnly: Bool) async throws {}
    func setPower(id: String, on: Bool) async throws {}
    func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        let attempt = lock.withLock { ids.append(id); return ids.count }
        if holdFirst && attempt == 1 {
            await withCheckedContinuation { continuation in lock.withLock { held = continuation } }
        }
        if failFirst && attempt == 1 { throw CuaError.Internal(message: "cancelled provider result") }
        return try await FixtureSpacesBackend().streamProvider(id: id)
    }
    func localBackends() async -> [String]? { nil }
    func localStorage() async -> LocalStorage? { nil }
    func cloudAvailable() async -> Bool { false }
    func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? { nil }
    func teleportHandle() -> CuaSpacesFFI.Teleport? { nil }
    func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] { [] }
    func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { [] }
}

@MainActor
@Suite("Detail approval refresh", .serialized)
struct DetailApprovalTests {
    @MainActor private struct Fixture {
        let backend: ApprovalBackend
        let devices: FixtureDevices
        let model: AppModel
        let space: AppSpace
        let window: NSWindow
        let directory: URL

        func close() {
            backend.release()
            window.contentView = nil
            window.close()
            try? FileManager.default.removeItem(at: directory)
        }

        func setApproved(_ approved: Bool) async {
            devices.current.devices[0].state = approved ? "enrolled" : "pending"
            await model.devices.refresh()
            window.contentView?.layoutSubtreeIfNeeded()
        }

        func waitForRequests(_ count: Int) async throws {
            for _ in 0..<40 where backend.requestedIds.count < count {
                try await Task.sleep(for: .milliseconds(50))
            }
            if count == 1 && backend.holdFirst {
                for _ in 0..<40 where !backend.isHeld { try await Task.sleep(for: .milliseconds(50)) }
                #expect(backend.isHeld)
            }
            #expect(backend.requestedIds == Array(repeating: space.id, count: count))
        }
    }

    private func open(autoConnect: Bool, holdFirst: Bool = false, failFirst: Bool = false) async throws -> Fixture {
        let backend = ApprovalBackend()
        backend.holdFirst = holdFirst
        backend.failFirst = failFirst
        var row = FixtureSpacesBackend.sample[0]
        row.id = "relay:studio"
        row.provider = "relay"
        backend.result = .success([row])
        var snapshot = FixtureDevices.sample(now: UInt64(Date().timeIntervalSince1970))
        snapshot.devices[0].state = "pending"
        let devices = FixtureDevices(snapshot: snapshot)
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
            onboarding: OnboardingModel(statePath: nil),
            settingsPath: directory.appendingPathComponent("settings.json").path,
            telemetry: FixtureTelemetry(), devices: devices, presence: FixturePresence())
        model.settings.autoConnect = autoConnect
        await model.refresh()
        model.devices.signedIn = true
        await model.devices.refresh()
        let space = try #require(model.spaces.first { $0.id == row.id })
        #expect(model.detail(space).access != nil)
        let host = NSHostingView(rootView: SpaceDetailView(model: model, space: space)
            .frame(width: 800, height: 800))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 800),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(250))
        #expect(backend.requestedIds.isEmpty)
        return Fixture(backend: backend, devices: devices, model: model, space: space,
                       window: window, directory: directory)
    }

    @Test(arguments: [false, true])
    func approvalReevaluatesTheSameDetail(autoConnect: Bool) async throws {
        let f = try await open(autoConnect: autoConnect)
        defer { f.close() }
        await f.setApproved(true)
        #expect(f.model.detail(f.space).access == nil)
        if autoConnect { try await f.waitForRequests(1) }
        // Repeated device snapshots neither force a manual connection nor
        // restart a connection that was already created for this exact ID.
        await f.setApproved(true)
        await f.setApproved(true)
        try await Task.sleep(for: .milliseconds(250))
        #expect(f.backend.requestedIds == (autoConnect ? [f.space.id] : []))
    }

    @Test(arguments: [false, true])
    func cancelledProviderCannotBlockLaterApproval(failFirst: Bool) async throws {
        let f = try await open(autoConnect: true, holdFirst: true, failFirst: failFirst)
        defer { f.close() }
        await f.setApproved(true)
        try await f.waitForRequests(1)
        await f.setApproved(false)
        try await Task.sleep(for: .milliseconds(100))
        // The backend deliberately returns even though its caller was
        // cancelled. Its old provider must not be installed in this view.
        f.backend.release()
        try await Task.sleep(for: .milliseconds(100))
        #expect(f.model.banner == nil)
        await f.setApproved(true)
        try await f.waitForRequests(2)
    }
}
