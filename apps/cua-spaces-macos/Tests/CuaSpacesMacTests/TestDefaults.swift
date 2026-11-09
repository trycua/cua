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
/// when the test process ends, however the tests were written.
enum TestDefaults {
    private nonisolated(unsafe) static var names: [String] = []
    private static let lock = NSLock()
    private nonisolated(unsafe) static var registered = false

    /// A fresh suite, removed when the process exits.
    static func make(_ prefix: String) -> UserDefaults {
        let name = "\(prefix)-\(UUID().uuidString)"
        lock.lock()
        if !registered {
            registered = true
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
}
