// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/// Gives the keyboard back to a viewer after its stream reconnects.
///
/// When a stream drops, its viewer stops being live (it hides, or its host
/// hands the keyboard to the page), so it loses the keyboard. Once the
/// stream is back it takes the keyboard again, but only if it had it when
/// the stream dropped and nobody else took it meanwhile (the keyboard is
/// still with the window or the page). Without this, keys typed after the
/// reconnect went nowhere until a click in the viewer.
public final class KeyboardReturn {
    private enum State { case idle, dropped(hadKeyboard: Bool) }
    private var state = State.idle

    public init() {}

    /// The stream dropped; `hadKeyboard`: the viewer had the keyboard then.
    /// Only the first call of a drop counts (later ones see it already lost).
    public func dropped(hadKeyboard: Bool) {
        guard case .idle = state else { return }
        state = .dropped(hadKeyboard: hadKeyboard)
    }

    /// The stream is back: whether the viewer should take the keyboard
    /// (`keyboardIsFree`: nobody else has taken it). Ends the drop.
    public func back(keyboardIsFree: Bool) -> Bool {
        defer { state = .idle }
        guard case .dropped(true) = state else { return false }
        return keyboardIsFree
    }

    /// Someone moved the keyboard on purpose: no return.
    public func forget() { state = .idle }

    public var waiting: Bool {
        if case .dropped(true) = state { return true }
        return false
    }
}
