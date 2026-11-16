// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Combine
import Foundation

/// The shell's chrome state, outside the view.
///
/// Everything here is something a control does, and every control's action is
/// a method on this type rather than a closure that mutates `@State`. That buys
/// three things: the affordance swaps are testable without a window server, the
/// screenshot harness can photograph every surface without synthesising clicks
/// (which would need Accessibility authorisation and a TCC dialog — see
/// `FRICTION.md` §29), and the header's controls stay a pure function of one
/// value instead of a set of booleans that can disagree.
@MainActor
final class ShellState: ObservableObject {
    @Published var pane: DetailsPane = .closed
    @Published var compose = ComposeState()
    /// A `Create new Bot` is in flight.
    @Published var creating = false

    // MARK: The controls

    /// Press a header or pane control. The pane decides where it goes.
    func tap(_ kind: DetailsPane.Control.Kind) {
        pane = pane.tapping(kind)
    }

    /// The sidebar `+`. Replaces the header with the `To:` field; no
    /// conversation is selected, so the pane's controls would be acting on
    /// nothing and it closes.
    func beginCompose() {
        compose = ComposeState(isComposing: true, query: "")
        pane = .closed
    }

    func cancelCompose() { compose = ComposeState() }

    /// Anything that lands the user in a conversation leaves compose mode.
    func opened() { compose = ComposeState() }
}
