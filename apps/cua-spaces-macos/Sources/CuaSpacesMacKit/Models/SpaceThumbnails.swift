// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CoreImage
import CoreVideo
import CuaSpacesFFI
import Foundation

/// A Space's thumbnail as the SDK returns it (`Space.thumbnail`): the
/// encoded image and when it was captured.
public struct SpaceThumbnailData: Sendable, Equatable {
    public var image: Data
    public var capturedAt: Date

    public init(image: Data, capturedAt: Date) {
        self.image = image
        self.capturedAt = capturedAt
    }
}

/// The app's in-memory layer over the SDK's thumbnail cache (the cua
/// daemon's, shared by every client on this Mac): the latest image per
/// Space, so the notch tiles and the preview cover paint at once. The
/// notch and the cover read the same entry; nothing here captures a
/// screen itself.
///
/// - `refresh(_:maxAge:)` asks the SDK for an image no older than `maxAge`
///   (the daemon answers from its cache, or captures).
/// - `warm(_:)` fills Spaces with no image yet from the daemon's cache
///   (any age), so a preview exists right after launch.
/// - `keepFresh` asks again for every running Space every
///   `backgroundInterval` while the app is visible and not in Low Power
///   Mode, which also keeps the daemon's own background refresh going.
@MainActor
@Observable
public final class SpaceThumbnails {
    public struct Entry: Equatable {
        public var image: NSImage
        public var capturedAt: Date
    }

    public private(set) var entries: [String: Entry] = [:]
    /// The SDK call (`Space.thumbnail(maxAgeMs:)`); none: memory only.
    @ObservationIgnored public var fetch: ((String, UInt64?) async -> SpaceThumbnailData?)?
    @ObservationIgnored private var inFlight: Set<String> = []
    /// Spaces `warm` already asked about (once each per launch).
    @ObservationIgnored private var warmed: Set<String> = []
    @ObservationIgnored private var keeping: Task<Void, Never>?
    public static let policy = appThumbnailPolicy()

    public init() {}

    /// The Space's latest image, read synchronously (nil: none yet).
    public subscript(id: String) -> NSImage? {
        get { entries[id]?.image }
        set {
            if let newValue { set(id, newValue) } else { remove(id) }
        }
    }

    /// Keeps `image` as the Space's latest, unless a newer one is held.
    public func set(_ id: String, _ image: NSImage, at capturedAt: Date = Date()) {
        if let held = entries[id], held.capturedAt > capturedAt { return }
        entries[id] = Entry(image: image, capturedAt: capturedAt)
    }

    public func remove(_ id: String) {
        entries[id] = nil
    }

    /// Forgets the Spaces not in `ids` (deleted or forgotten).
    public func retain(_ ids: Set<String>) {
        for id in entries.keys where !ids.contains(id) { entries[id] = nil }
    }

    /// Asks the SDK for the Space's thumbnail no older than `maxAge`
    /// (seconds; nil: any age) and keeps it. Returns the latest image.
    @discardableResult
    public func refresh(_ id: String, maxAge: TimeInterval?) async -> NSImage? {
        guard let fetch, !inFlight.contains(id) else { return self[id] }
        inFlight.insert(id)
        defer { inFlight.remove(id) }
        let ms = maxAge.map { UInt64(max(0, $0) * 1000) }
        if let data = await fetch(id, ms), let image = NSImage(data: data.image) {
            set(id, image, at: data.capturedAt)
        }
        return self[id]
    }

    /// Fills each Space in `ids` that has no image yet from the daemon's
    /// cache (any age).
    public func warm(_ ids: [String]) async {
        let missing = ids.filter { entries[$0] == nil && !warmed.contains($0) }
        warmed.formUnion(missing)
        await withTaskGroup(of: Void.self) { group in
            for id in missing {
                group.addTask { @MainActor in _ = await self.refresh(id, maxAge: nil) }
            }
        }
    }

    /// While the app runs: every running Space (as `running` lists them)
    /// gets an image no older than the policy's background interval, every
    /// interval, skipped while the app is hidden or in Low Power Mode.
    public func keepFresh(running: @escaping @MainActor () -> [String],
                          active: @escaping @MainActor () -> Bool = SpaceThumbnails.appActive) {
        keeping?.cancel()
        let interval = TimeInterval(Self.policy.backgroundIntervalMs) / 1000
        keeping = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                if let self, active() {
                    for id in running() {
                        _ = await self.refresh(id, maxAge: interval)
                    }
                }
                try? await Task.sleep(for: .seconds(interval))
            }
        }
    }

    /// Visible and not saving power.
    public static func appActive() -> Bool {
        !NSApp.isHidden && !ProcessInfo.processInfo.isLowPowerModeEnabled
    }

    /// The last frame of a stream as an image (it is newer than any
    /// thumbnail when the stream stops).
    public static func image(_ buffer: CVPixelBuffer) -> NSImage? {
        let ci = CIImage(cvPixelBuffer: buffer)
        guard let cg = CIContext().createCGImage(ci, from: ci.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}
