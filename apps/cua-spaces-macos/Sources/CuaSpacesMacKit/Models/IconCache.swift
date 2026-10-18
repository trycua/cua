// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// Where an app's icon comes from, resolved once per app: its bundle id and
/// version key the caches, `render` draws the PNG only on a cache miss.
public struct AppIconSource: Sendable {
    public let bundleId: String
    public let version: String
    public let render: @Sendable () -> Data?

    public init(bundleId: String, version: String, render: @escaping @Sendable () -> Data?) {
        self.bundleId = bundleId
        self.version = version
        self.render = render
    }
}

/// App icons for the Keyvault list. NSWorkspace is asked once per app and
/// version, never per render: a decoded image lives in an `NSCache`, the PNG
/// on disk under the bundle id and version, so a restart (or an app update,
/// which changes the key) costs one render at most.
public final class AppIconCache: @unchecked Sendable {
    public typealias Resolver = @Sendable (_ providerId: String, _ name: String) -> AppIconSource?

    private let dir: URL?
    private let resolver: Resolver
    private let memory = NSCache<NSString, NSImage>()
    private let lock = NSLock()
    /// provider id to cache key (resolved once; "" when the app is not here).
    private var keys: [String: String] = [:]
    /// How often NSWorkspace was asked and an icon was drawn (the tests'
    /// proof that nothing is resolved per render).
    public private(set) var resolves = 0
    public private(set) var renders = 0

    public init(dir: URL?, resolver: @escaping Resolver = AppIconCache.workspace) {
        self.dir = dir
        self.resolver = resolver
        memory.countLimit = 256
    }

    /// The icon of an app, or nil (the list draws its first letter).
    public func image(providerId: String, name: String) -> NSImage? {
        lock.lock()
        defer { lock.unlock() }
        if let key = keys[providerId] {
            return key.isEmpty ? nil : memory.object(forKey: key as NSString) ?? loadDisk(key)
        }
        resolves += 1
        guard let source = resolver(providerId, name) else {
            keys[providerId] = ""
            return nil
        }
        let key = AppIconCache.fileKey(source)
        keys[providerId] = key
        if let hit = memory.object(forKey: key as NSString) ?? loadDisk(key) { return hit }
        guard let png = source.render(), let image = NSImage(data: png) else {
            keys[providerId] = ""
            return nil
        }
        renders += 1
        memory.setObject(image, forKey: key as NSString)
        if let url = file(key) {
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? png.write(to: url, options: .atomic)
        }
        return image
    }

    static func fileKey(_ s: AppIconSource) -> String {
        let safe = { (v: String) in v.map { $0.isLetter || $0.isNumber || ".-_".contains($0) ? String($0) : "_" }.joined() }
        return "\(safe(s.bundleId))-\(safe(s.version))"
    }

    private func file(_ key: String) -> URL? { dir?.appendingPathComponent("\(key).png") }

    private func loadDisk(_ key: String) -> NSImage? {
        guard let url = file(key), let data = try? Data(contentsOf: url), let image = NSImage(data: data) else { return nil }
        memory.setObject(image, forKey: key as NSString)
        return image
    }

    /// The bundle ids an app may have (the provider catalog's ids).
    static let bundleIds: [String: [String]] = [
        "chrome": ["com.google.Chrome"],
        "firefox": ["org.mozilla.firefox"],
        "slack": ["com.tinyspeck.slackmacgap"],
        "discord": ["com.hnc.Discord"],
        "steam": ["com.valvesoftware.steam"],
        "whatsapp": ["net.whatsapp.WhatsApp", "desktop.WhatsApp"],
        "claude-code": ["com.anthropic.claudefordesktop"],
    ]

    /// The real resolver: Launch Services for the app's location, its
    /// bundle for the version, NSWorkspace for the icon.
    public static let workspace: Resolver = { providerId, name in
        let ws = NSWorkspace.shared
        var url: URL?
        var id = ""
        for candidate in bundleIds[providerId] ?? [] {
            if let found = ws.urlForApplication(withBundleIdentifier: candidate) { url = found; id = candidate; break }
        }
        if url == nil {
            let guess = URL(fileURLWithPath: "/Applications/\(name).app")
            if FileManager.default.fileExists(atPath: guess.path) {
                url = guess
                id = Bundle(url: guess)?.bundleIdentifier ?? providerId
            }
        }
        guard let url else { return nil }
        let info = Bundle(url: url)?.infoDictionary
        let version = (info?["CFBundleShortVersionString"] as? String) ?? (info?["CFBundleVersion"] as? String) ?? "0"
        return AppIconSource(bundleId: id, version: version) {
            let icon = ws.icon(forFile: url.path)
            return AppIconCache.png(icon, side: 64)
        }
    }

    static func png(_ image: NSImage, side: Int) -> Data? {
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        image.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }
}

/// How a site icon is downloaded (a stub in tests).
public typealias IconFetch = @Sendable (URL) async throws -> (Data, Int)

/// Site icons for the Keyvault rows, in order: the icon the source browser
/// held (read locally when the items were saved, handed in through
/// `remember`), then Google's favicon service when the switch is on, then the
/// globe (nil here). Fetched lazily as rows appear, a few at a time with a
/// short timeout, successes and misses kept on disk (a miss is retried after
/// a week), so a domain is asked for at most once. Only the registrable
/// domain is ever sent. Nothing here blocks a render.
public final class SiteIconStore: @unchecked Sendable {
    private let dir: URL?
    private let fetcher: IconFetch
    private let now: @Sendable () -> Date
    private let retryAfter: TimeInterval
    private let gate: Gate
    private let memory = NSCache<NSString, NSImage>()
    private let lock = NSLock()
    private var inFlight: Set<String> = []
    /// Requests that left (the tests count them).
    public private(set) var requests: [URL] = []

    public init(dir: URL?, maxConcurrent: Int = 3, retryAfter: TimeInterval = 7 * 86_400,
                now: @escaping @Sendable () -> Date = Date.init, fetch: @escaping IconFetch = SiteIconStore.network) {
        self.dir = dir
        self.fetcher = fetch
        self.now = now
        self.retryAfter = retryAfter
        self.gate = Gate(limit: maxConcurrent)
        memory.countLimit = 512
    }

    /// A registrable domain that is safe to send: letters, digits, dots and
    /// hyphens, at least two labels, not an address and not a local name.
    public static func sendable(_ site: String) -> Bool {
        let s = site.lowercased()
        let labels = s.split(separator: ".", omittingEmptySubsequences: false)
        guard labels.count >= 2, s.count <= 253, !labels.contains(where: { $0.isEmpty || $0.count > 63 }) else { return false }
        guard s.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "." || $0 == "-") }) else { return false }
        guard !labels.last!.allSatisfy(\.isNumber) else { return false }
        return !["local", "localhost", "internal", "lan", "home", "corp", "test", "invalid"].contains(String(labels.last!))
    }

    public static func url(for site: String, scale: Int) -> URL? {
        guard sendable(site) else { return nil }
        let size = scale >= 2 ? 128 : 64
        return URL(string: "https://www.google.com/s2/favicons?domain=\(site.lowercased())&sz=\(size)")
    }

    private func key(_ site: String) -> String { site.lowercased() }
    private func file(_ site: String, _ ext: String) -> URL? { dir?.appendingPathComponent("\(key(site)).\(ext)") }

    /// A downloaded icon already on hand (memory, then disk). No network.
    public func cached(_ site: String) -> NSImage? {
        if let m = memory.object(forKey: key(site) as NSString) { return m }
        guard let url = file(site, "png"), let data = try? Data(contentsOf: url), let image = NSImage(data: data) else { return nil }
        memory.setObject(image, forKey: key(site) as NSString)
        return image
    }

    /// A miss newer than the retry window (do not ask again yet).
    public func recentMiss(_ site: String) -> Bool {
        guard let url = file(site, "miss"),
              let at = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.modificationDate] as? Date else { return false }
        return now().timeIntervalSince(at) < retryAfter
    }

    /// Looks the icon up on the network unless it is cached, was missed
    /// lately or is already being fetched. Returns the image or nil.
    public func load(_ site: String, scale: Int) async -> NSImage? {
        if let hit = cached(site) { return hit }
        guard !recentMiss(site), let url = SiteIconStore.url(for: site, scale: scale) else { return nil }
        lock.lock()
        if inFlight.contains(key(site)) { lock.unlock(); return nil }
        inFlight.insert(key(site))
        lock.unlock()
        defer { lock.lock(); inFlight.remove(key(site)); lock.unlock() }
        await gate.enter()
        lock.lock(); requests.append(url); lock.unlock()
        let result: (Data, Int)?
        do { result = try await fetcher(url) } catch { result = nil }
        await gate.leave()
        do {
            guard let (data, status) = result else { throw CancellationError() }
            guard status == 200, data.count <= 64 * 1024, let image = NSImage(data: data), image.isValid else {
                markMiss(site)
                return nil
            }
            memory.setObject(image, forKey: key(site) as NSString)
            if let f = file(site, "png") {
                try? FileManager.default.createDirectory(at: f.deletingLastPathComponent(), withIntermediateDirectories: true)
                try? data.write(to: f, options: .atomic)
                if let m = file(site, "miss") { try? FileManager.default.removeItem(at: m) }
            }
            return image
        } catch {
            // A timeout or no network is not the site's fault; ask again
            // next launch rather than remembering a miss for a week.
            return nil
        }
    }

    private func markMiss(_ site: String) {
        guard let url = file(site, "miss") else { return }
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? Data().write(to: url)
        try? FileManager.default.setAttributes([.modificationDate: now()], ofItemAtPath: url.path)
    }

    /// The real download: no cookies, no cache, four seconds.
    public static let network: IconFetch = { url in
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 4
        config.timeoutIntervalForResource = 6
        config.httpCookieStorage = nil
        config.urlCache = nil
        let (data, response) = try await URLSession(configuration: config).data(from: url)
        return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
    }

    actor Gate {
        private let limit: Int
        private var active = 0
        private var waiting: [CheckedContinuation<Void, Never>] = []
        init(limit: Int) { self.limit = max(1, limit) }
        func enter() async {
            if active < limit { active += 1; return }
            await withCheckedContinuation { waiting.append($0) }
        }
        func leave() {
            if waiting.isEmpty { active -= 1 } else { waiting.removeFirst().resume() }
        }
    }
}
