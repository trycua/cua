// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// A state file that is not what it should be (cut short by a crash or a full
/// disk, or garbage) must not crash the app or lose what it held: the app
/// starts from the defaults and the file stays beside it as `<name>.damaged`
/// until the next save writes a good one over the original. The core does the
/// same for settings.json (`settings::load`).
enum DamagedFile {
    /// Keeps `path` as `<path>.damaged` (the latest damaged one replaces an
    /// older copy; a copy that is already the same is left alone, so a file
    /// read on every launch step is not copied again). Returns where it went,
    /// or nil when it could not be kept. Copies keep the file's mode.
    @discardableResult
    static func keep(_ path: String) -> String? {
        let fm = FileManager.default
        let copy = path + ".damaged"
        guard let data = fm.contents(atPath: path) else { return nil }
        if let kept = fm.contents(atPath: copy), kept == data { return copy }
        do {
            if fm.fileExists(atPath: copy) { try fm.removeItem(atPath: copy) }
            try fm.copyItem(atPath: path, toPath: copy)
            NSLog("cua-spaces: %@ is damaged; starting from the defaults, the file is kept as %@", path, copy)
            return copy
        } catch {
            return nil
        }
    }
}
