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
import Vision

/// Scripted provider creation and media failures; no host is contacted.
private final class ProviderFailureBackend: SpacesBackend, @unchecked Sendable {
    var result: Result<[AppSpaceRow], Error> = .success(FixtureSpacesBackend.sample)
    var failures = 2
    let provider = RefusingMediaProvider()
    private let lock = NSLock()
    private var ids: [String] = []
    var requestedIds: [String] { lock.withLock { ids } }
    func rows() async throws -> [AppSpaceRow] { try result.get() }
    func create(_ args: AppCreateSpaceArgs, createId: String, progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String { fatalError("unexpected create") }
    func cancelCreate(createId: String) async throws {}
    func add(url: String, token: String?, name: String?) async throws {}
    func remove(id: String, removeOnly: Bool) async throws {}
    func setPower(id: String, on: Bool) async throws {}
    func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        let attempt = lock.withLock { ids.append(id); return ids.count }
        if attempt <= failures { throw CuaError.Internal(message: "scripted provider failure") }
        return provider
    }
    func localBackends() async -> [String]? { nil }
    func localStorage() async -> LocalStorage? { nil }
    func cloudAvailable() async -> Bool { false }
    func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? { nil }
    func teleportHandle() -> CuaSpacesFFI.Teleport? { nil }
    func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] { [] }
    func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { [] }
}

private final class RefusingMediaProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    private let lock = NSLock()
    private var opens = 0
    var attempts: Int { lock.withLock { opens } }
    func availableWindows() async throws -> [StreamWindow] { [] }
    func openSession(_ source: StreamSource, frames: FrameSink, audio: AudioSink?) async throws -> SpaceStreamSession {
        lock.withLock { opens += 1 }
        throw StreamError.noStream("scripted media failure")
    }
    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("no presence in fixture")
    }
}

@MainActor @Suite("Provider creation recovery", .serialized)
struct ProviderFailureTests {
    @MainActor private struct Fixture {
        let backend: ProviderFailureBackend
        let model: AppModel
        let devices: FixtureDevices
        let space: AppSpace
        let host: NSView
        let window: NSWindow
        let directory: URL

        func close() {
            window.contentView = nil
            window.close()
            try? FileManager.default.removeItem(at: directory)
        }

        func text() throws -> [VNRecognizedTextObservation] {
            host.layoutSubtreeIfNeeded()
            window.display()
            let bitmap = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            let request = VNRecognizeTextRequest()
            request.recognitionLanguages = ["en-US"]
            request.recognitionLevel = .accurate
            request.usesLanguageCorrection = false
            try VNImageRequestHandler(cgImage: #require(bitmap.cgImage), options: [:]).perform([request])
            return request.results ?? []
        }

        func press(_ label: String) throws {
            let observations = try text()
            let match = try #require(observations.first { $0.topCandidates(1).first?.string == label },
                                     "the actual detail renders \(label)")
            let box = match.boundingBox
            let local = NSPoint(x: box.midX * host.bounds.width,
                                y: (host.isFlipped ? 1 - box.midY : box.midY) * host.bounds.height)
            let point = host.convert(local, to: nil)
            let key = NSApp.keyWindow
            // AppKit dispatch within this process to this window only: no
            // system event posting, Accessibility grant, or real desktop.
            for kind in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                let event = try #require(NSEvent.mouseEvent(with: kind, location: point, modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                    context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
                try #require(event.window === window)
                NSApp.sendEvent(event)
            }
            #expect(NSApp.keyWindow === key)
        }

        func settle() async throws { try await Task.sleep(for: .milliseconds(350)) }
        func expectProviders(_ count: Int) async throws {
            for _ in 0..<80 where backend.requestedIds.count < count {
                try await Task.sleep(for: .milliseconds(50))
            }
            try await settle()
            #expect(backend.requestedIds == Array(repeating: space.id, count: count))
        }
        func approve(_ approved: Bool) async throws {
            devices.current.devices[0].state = approved ? "enrolled" : "pending"
            await model.devices.refresh()
            try await settle()
        }
    }

    private func open(autoConnect: Bool, failures: Int = 2) async throws -> Fixture {
        _ = NSApplication.shared
        NSApp.finishLaunching()
        let backend = ProviderFailureBackend()
        backend.failures = failures
        var row = FixtureSpacesBackend.sample[0]
        row.id = "relay:studio"
        row.provider = "relay"
        backend.result = .success([row])
        let devices = FixtureDevices(snapshot: FixtureDevices.sample(now: UInt64(Date().timeIntervalSince1970)))
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
            onboarding: OnboardingModel(statePath: nil), settingsPath: directory.appendingPathComponent("settings.json").path,
            telemetry: FixtureTelemetry(), devices: devices, presence: FixturePresence())
        model.settings.autoConnect = autoConnect
        await model.refresh()
        model.devices.signedIn = true
        await model.devices.refresh()
        let space = try #require(model.spaces.first { $0.id == row.id })
        #expect(model.detail(space).access == nil)
        let host = NSHostingView(rootView: SpaceDetailView(model: model, space: space).frame(width: 800, height: 800))
        let window = NSWindow(contentRect: NSRect(x: -20000, y: -20000, width: 800, height: 800),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        let key = NSApp.keyWindow
        window.orderBack(nil)
        #expect(NSApp.keyWindow === key)
        try #require(NSScreen.screens.allSatisfy { !$0.frame.intersects(window.frame) })
        try #require(window.windowNumber > 0, "requires a WindowServer-backed test window")
        host.layoutSubtreeIfNeeded()
        return Fixture(backend: backend, model: model, devices: devices, space: space,
                       host: host, window: window, directory: directory)
    }

    @Test(arguments: [false, true])
    func retriesProviderCreationThenAttemptsMedia(autoConnect: Bool) async throws {
        let f = try await open(autoConnect: autoConnect)
        defer { f.close() }
        if !autoConnect {
            try await f.settle()
            #expect(f.backend.requestedIds.isEmpty)
            try f.press("Connect")
        }
        try await f.expectProviders(1)
        #expect(f.model.banner?.contains("scripted provider failure") == true)
        await f.model.refresh()
        try await f.settle()
        #expect(f.backend.requestedIds == [f.space.id], "polling does not retry a failure")
        for attempt in 2...3 {
            try f.press("Try again")
            try await f.expectProviders(attempt)
        }
        for _ in 0..<80 where f.backend.provider.attempts < 1 {
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(f.backend.provider.attempts == 1, "provider creation recovered and attempted to open media")
        try await f.settle()
        await f.model.refresh()
        try await f.settle()
        #expect(f.backend.provider.attempts == 1)
        #expect(f.backend.requestedIds == Array(repeating: f.space.id, count: 3))
    }

    @Test func approvalAndPollingDoNotRetryAnExistingFailure() async throws {
        let f = try await open(autoConnect: true, failures: 1)
        defer { f.close() }
        try await f.expectProviders(1)
        try await f.approve(false)
        #expect(f.model.detail(f.space).access != nil)
        try await f.approve(true)
        #expect(f.model.detail(f.space).access == nil)
        #expect(f.backend.requestedIds == [f.space.id])
        try f.press("Try again")
        try await f.expectProviders(2)
    }
}
