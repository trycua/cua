// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Keeps Sparkle's download cache small. Sparkle leaves every downloaded update
/// and its staged installer in `~/Library/Caches/<bundle id>/org.sparkle-project.Sparkle`
/// (and `<bundle id>.ShipIt`), which grew past the size of the app itself.
///
/// * After an update (the first launch of a new version) all of it goes: the
///   update is installed, nothing in there is needed.
/// * Otherwise anything untouched for a week goes, then the oldest entries
///   until the folder is under the cap. The newest entry is always kept, so a
///   download waiting to be installed survives.
public enum SparkleCache {
    /// Largest size the cache may keep.
    public static let maxBytes: Int64 = 200 * 1024 * 1024
    /// Entries untouched for this long are removed.
    public static let maxAge: TimeInterval = 7 * 24 * 3600

    /// The folders Sparkle uses for `bundleID`, under `caches`.
    public static func folders(bundleID: String, caches: URL) -> [URL] {
        [
            caches.appendingPathComponent(bundleID).appendingPathComponent("org.sparkle-project.Sparkle"),
            caches.appendingPathComponent("\(bundleID).ShipIt"),
        ]
    }

    /// Trims the cache; returns the bytes freed.
    @discardableResult
    public static func trim(
        bundleID: String,
        caches: URL = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0],
        afterUpdate: Bool,
        now: Date = Date(),
        maxBytes: Int64 = SparkleCache.maxBytes,
        maxAge: TimeInterval = SparkleCache.maxAge,
        fm: FileManager = .default
    ) -> Int64 {
        var freed: Int64 = 0
        for folder in folders(bundleID: bundleID, caches: caches) {
            guard let names = try? fm.contentsOfDirectory(atPath: folder.path) else { continue }
            var entries: [(url: URL, date: Date, size: Int64)] = names.map {
                let url = folder.appendingPathComponent($0)
                return (url, modified(url, fm), size(url, fm))
            }
            entries.sort { $0.date < $1.date }
            var total = entries.reduce(Int64(0)) { $0 + $1.size }
            let newest = entries.last?.url
            for e in entries {
                let stale = now.timeIntervalSince(e.date) > maxAge
                if afterUpdate || stale || (total > maxBytes && e.url != newest) {
                    if e.url == newest, !afterUpdate, !stale { continue }
                    if (try? fm.removeItem(at: e.url)) != nil {
                        freed += e.size
                        total -= e.size
                    }
                }
            }
        }
        return freed
    }

    private static func modified(_ url: URL, _ fm: FileManager) -> Date {
        (try? fm.attributesOfItem(atPath: url.path)[.modificationDate] as? Date) ?? .distantPast
    }

    private static func size(_ url: URL, _ fm: FileManager) -> Int64 {
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { return 0 }
        if !isDir.boolValue {
            return Int64((try? fm.attributesOfItem(atPath: url.path)[.size] as? UInt64) ?? 0)
        }
        var total: Int64 = 0
        if let it = fm.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey]) {
            for case let f as URL in it {
                total += Int64((try? f.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
            }
        }
        return total
    }
}
