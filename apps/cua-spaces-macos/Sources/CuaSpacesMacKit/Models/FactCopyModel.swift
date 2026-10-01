// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation

/// A fact's copy button state: what the core's `AppFactCopy` says to copy
/// and show, and the brief "Copied" after a copy.
@MainActor
final class FactCopyModel: ObservableObject {
    let copy: AppFactCopy
    private let write: (String) -> Void
    @Published private(set) var copied: Bool
    private var revert: Task<Void, Never>?

    init(copy: AppFactCopy, copied: Bool = false, write: @escaping (String) -> Void = FactCopyModel.pasteboard) {
        self.copy = copy
        self.copied = copied
        self.write = write
    }

    var symbol: String { copied ? copy.doneSymbol : copy.symbol }
    var help: String { copied ? copy.doneHelp : copy.help }

    /// Copies the text, then shows the confirmation for `confirmMs`.
    func copy(after: Duration? = nil) {
        write(copy.text)
        copied = true
        revert?.cancel()
        let wait = after ?? .milliseconds(Int(copy.confirmMs))
        revert = Task { [weak self] in
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled else { return }
            self?.copied = false
        }
    }

    /// The general pasteboard (the app only; tests pass their own writer).
    nonisolated static func pasteboard(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
