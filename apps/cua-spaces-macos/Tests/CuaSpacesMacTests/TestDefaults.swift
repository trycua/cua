// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// UserDefaults suites for tests that clean up after themselves.
///
/// `UserDefaults(suiteName:)` writes `~/Library/Preferences/<suite>.plist`, and
/// `removePersistentDomain` does not always delete the file, so every test run
/// used to leave one plist per suite behind (90 `cua-ln-*` and
/// `cua-relay-usage-*` files on a dev Mac). `make` hands out a suite with a
/// unique name, and one `atexit` handler removes the domains and their files
/// when the test process ends, however the tests were written. A run also
/// sweeps leftovers of earlier runs that matched these test-only prefixes.
enum TestDefaults {
    /// Prefixes only tests use; `<prefix>-<UUID>` leftovers are swept.
    private static let testPrefixes = [
        "cua-ln-", "cua-relay-usage-", "cua-spaces-launch-", "webui-video-",
        "PermissionGuideTests.",
    ]

    private nonisolated(unsafe) static var names: [String] = []
    private static let lock = NSLock()
    private nonisolated(unsafe) static var registered = false

    /// A fresh suite, removed when the process exits.
    static func make(_ prefix: String) -> UserDefaults {
        let name = "\(prefix)-\(UUID().uuidString)"
        lock.lock()
        if !registered {
            registered = true
            sweepLeftovers()
            atexit { TestDefaults.cleanUp() }
        }
        names.append(name)
        lock.unlock()
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        return defaults
    }

    /// Removes every suite handed out by `make`, domain and file.
    static func cleanUp() {
        lock.lock()
        let all = names
        names = []
        lock.unlock()
        for name in all {
            UserDefaults(suiteName: name)?.removePersistentDomain(forName: name)
            try? FileManager.default.removeItem(at: plistURL(name))
        }
    }

    static func plistURL(_ name: String) -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Preferences/\(name).plist")
    }

    /// Plists earlier runs left: `<test prefix><UUID>` and nothing else.
    private static func sweepLeftovers() {
        let dir = plistURL("x").deletingLastPathComponent()
        let files = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        for f in files where f.hasSuffix(".plist") {
            let stem = String(f.dropLast(".plist".count))
            guard let p = testPrefixes.first(where: { stem.hasPrefix($0) }) else { continue }
            // The tail must be a UUID, so a real preference file can never match.
            guard stem.count >= p.count + 36, UUID(uuidString: String(stem.suffix(36))) != nil else { continue }
            try? FileManager.default.removeItem(at: dir.appendingPathComponent(f))
        }
    }
}
