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
        /// The decoded pixels' size in bytes (what the cap counts).
        public var bytes: Int
    }

    public private(set) var entries: [String: Entry] = [:]
    /// The most bytes of images held; past it the least recently set or
    /// read Space's image goes first. Images are thumbnails (the policy's
    /// `maxDimension`, about 0.4 MB each), so the cap holds well over a
    /// hundred Spaces and only bites on an oversized image.
    @ObservationIgnored public var byteCap = SpaceThumbnails.defaultByteCap
    public static let defaultByteCap = 64 << 20
    /// Bytes held now.
    public private(set) var totalBytes = 0
    /// Space ids, least recently used first.
    @ObservationIgnored private var recency: [String] = []
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
        get {
            guard let entry = entries[id] else { return nil }
            touch(id)
            return entry.image
        }
        set {
            if let newValue { set(id, newValue) } else { remove(id) }
        }
    }

    /// Keeps `image` as the Space's latest, unless a newer one is held.
    public func set(_ id: String, _ image: NSImage, at capturedAt: Date = Date()) {
        if let held = entries[id], held.capturedAt > capturedAt { return }
        let bytes = Self.bytes(of: image)
        totalBytes += bytes - (entries[id]?.bytes ?? 0)
        entries[id] = Entry(image: image, capturedAt: capturedAt, bytes: bytes)
        touch(id)
        evict(keeping: id)
    }

    public func remove(_ id: String) {
        guard let held = entries.removeValue(forKey: id) else { return }
        totalBytes -= held.bytes
        recency.removeAll { $0 == id }
    }

    /// Forgets the Spaces not in `ids` (deleted or forgotten).
    public func retain(_ ids: Set<String>) {
        for id in Array(entries.keys) where !ids.contains(id) { remove(id) }
    }

    private func touch(_ id: String) {
        if recency.last == id { return }
        recency.removeAll { $0 == id }
        recency.append(id)
    }

    /// Drops the least recently used images until the rest fit the cap
    /// (`keeping`'s own image stays even when it alone is over).
    private func evict(keeping id: String) {
        var i = 0
        while totalBytes > byteCap, i < recency.count {
            let victim = recency[i]
            if victim == id { i += 1; continue }
            remove(victim)
        }
    }

    /// The decoded size of an image's largest representation.
    static func bytes(of image: NSImage) -> Int {
        let reps = image.representations.map { $0.pixelsWide * $0.pixelsHigh * 4 }
        if let largest = reps.max(), largest > 0 { return largest }
        return Int(image.size.width * image.size.height * 4)
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
    /// thumbnail when the stream stops), scaled down to a thumbnail's size:
    /// a full-size frame is tens of megabytes held for as long as the app
    /// runs.
    public static func image(_ buffer: CVPixelBuffer,
                             maxDimension: Int = Int(SpaceThumbnails.policy.maxDimension)) -> NSImage? {
        var ci = CIImage(cvPixelBuffer: buffer)
        let long = max(ci.extent.width, ci.extent.height)
        if maxDimension > 0, long > CGFloat(maxDimension) {
            let k = CGFloat(maxDimension) / long
            ci = ci.transformed(by: CGAffineTransform(scaleX: k, y: k))
        }
        let rect = ci.extent.integral
        guard let cg = context.createCGImage(ci, from: rect) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }

    /// One Core Image context for the app (each one holds its own GPU
    /// caches; one per call grew them per call).
    static let context = CIContext()
}
