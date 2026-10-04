// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// Hover and press feedback for the controls inside the notch island.
///
/// Icon buttons fade in a faint white fill on hover with a 0.12 s ease-out,
/// and items lift on hover. Here every control shares one fast spring (about
/// 120 ms, much quicker than the island's open spring), a hover highlight or
/// lift, a quick scale-down while pressed and the pointing-hand cursor. The
/// Tauri notch uses the same numbers (`--notch-press-*` in `app.css`).
public enum NotchPress {
    /// The hover and press spring: settles in about 120 ms.
    public static let spring = Animation.spring(response: 0.12, dampingFraction: 0.72)
    /// Pressed scale for icon buttons and the tab.
    public static let buttonScale: CGFloat = 0.9
    /// Pressed scale for a Space tile.
    public static let tileScale: CGFloat = 0.96
    /// A hovered tile rises this far.
    public static let tileLift: CGFloat = 2
    /// The hovered icon button's fill (white at this opacity).
    public static let hoverFill: Double = 0.14
    /// Minimum hit target, in points.
    public static let minHit: CGFloat = 28
}

/// A control inside the notch, for forcing its look (tests, debug starts).
public enum NotchControl: Hashable {
    case tile(String)
    case button(AppNotchButtonId)
    case search
    case tab
}

/// A forced hover or pressed look on one control. Snapshot tests and the
/// `CUA_SPACES_NOTCH_HIGHLIGHT` debug start use it; real input never sets it.
public struct NotchHighlight: Equatable {
    public var control: NotchControl
    public var pressed: Bool

    public init(_ control: NotchControl, pressed: Bool = false) {
        self.control = control
        self.pressed = pressed
    }

    /// `CUA_SPACES_NOTCH_HIGHLIGHT`: `<target>[:pressed]` with target
    /// `tile` (the first tile), `tile=<space id>`, `list`, `settings`,
    /// `search` or `tab`.
    public static func parse(_ raw: String, firstTile: String?) -> NotchHighlight? {
        // Space ids contain colons, so the state is a suffix.
        let pressed = raw.hasSuffix(":pressed")
        let target = pressed ? String(raw.dropLast(":pressed".count)) : raw
        let control: NotchControl
        switch target {
        case "tile":
            guard let firstTile else { return nil }
            control = .tile(firstTile)
        case let t where t.hasPrefix("tile="):
            control = .tile(String(t.dropFirst(5)))
        case "list": control = .button(.list)
        case "settings": control = .button(.settings)
        case "search": control = .search
        case "tab": control = .tab
        default: return nil
        }
        return NotchHighlight(control, pressed: pressed)
    }
}

/// The interaction state a control draws with.
struct NotchInteractionState: Equatable {
    var hovered = false
    var pressed = false
}

extension Optional where Wrapped == NotchHighlight {
    /// The forced look for `control`, if any.
    func state(for control: NotchControl) -> NotchInteractionState? {
        guard let self, self.control == control else { return nil }
        return NotchInteractionState(hovered: true, pressed: self.pressed)
    }
}

/// A button inside the notch: `draw` gets the hover and pressed state; the
/// style adds the pointing hand, the tooltip-friendly hit shape and the
/// press spring. Pressed scale is skipped under Reduce Motion.
struct NotchButtonStyle<Body: View>: ButtonStyle {
    var forced: NotchInteractionState?
    var pressedScale: CGFloat = NotchPress.buttonScale
    let draw: (Configuration.Label, NotchInteractionState) -> Body

    func makeBody(configuration: Configuration) -> some View {
        NotchButtonBody(configuration: configuration, forced: forced, pressedScale: pressedScale, draw: draw)
    }
}

private struct NotchButtonBody<Body: View>: View {
    let configuration: ButtonStyleConfiguration
    let forced: NotchInteractionState?
    let pressedScale: CGFloat
    let draw: (ButtonStyleConfiguration.Label, NotchInteractionState) -> Body
    @State private var hovered = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let s = NotchInteractionState(hovered: hovered || forced?.hovered == true,
                                      pressed: configuration.isPressed || forced?.pressed == true)
        draw(configuration.label, s)
            .scaleEffect(s.pressed && !reduceMotion ? pressedScale : 1)
            .animation(NotchPress.spring, value: s)
            .onHover { hovered = $0 }
            .pointerStyle(.link)
    }
}

/// The header's icon buttons: a 28 pt square, a faint white rounded fill on
/// hover, the icon brightening from 75% to full white.
struct NotchIconLabel: View {
    let symbol: String
    let state: NotchInteractionState

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(.white.opacity(state.hovered ? 1 : 0.75))
            .frame(width: NotchPress.minHit, height: NotchPress.minHit)
            .background(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(.white.opacity(state.pressed ? NotchPress.hoverFill + 0.06 : state.hovered ? NotchPress.hoverFill : 0)))
            .contentShape(.rect)
    }
}
