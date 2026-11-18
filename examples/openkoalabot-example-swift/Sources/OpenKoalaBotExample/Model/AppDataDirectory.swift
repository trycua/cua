// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Where the app keeps its roster and routines.
///
/// `~/Library/Application Support/OpenKoalaBots` by default. Application
/// Support is resolved from the user's account, not from `$HOME`, so a
/// temporary `HOME` does not move it: `OPENKOALABOTS_DATA_DIR` does, and the
/// screenshot and capture runs set it so they never touch the real folder.
enum AppDataDirectory {
    static let variable = "OPENKOALABOTS_DATA_DIR"

    static func url(_ env: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        let dir: URL
        if let v = env[variable], !v.isEmpty {
            dir = URL(fileURLWithPath: v, isDirectory: true)
        } else {
            let base = FileManager.default.urls(for: .applicationSupportDirectory,
                                                in: .userDomainMask).first
                ?? URL(fileURLWithPath: NSTemporaryDirectory())
            dir = base.appendingPathComponent("OpenKoalaBots", isDirectory: true)
        }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }
}
