// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// The app window's palette: neutral, opaque, one accent used sparingly.
///
/// Every surface is a solid colour. There is no material or vibrancy anywhere
/// in the window, so a screenshot shows exactly what the palette says.
struct KoalaPalette: Equatable {
    var isDark: Bool
    var bg: Color
    var sidebar: Color
    var surface: Color
    var surface2: Color
    var border: Color
    var text: Color
    var secondary: Color
    var accent: Color
    /// Sidebar row under the pointer / selected.
    var rowHover: Color
    var rowSelected: Color

    static let dark = KoalaPalette(
        isDark: true,
        bg: Color(hex: 0x121212), sidebar: Color(hex: 0x0A0A0A),
        surface: Color(hex: 0x181818), surface2: Color(hex: 0x1F1F1F),
        border: Color(hex: 0x262626), text: Color(hex: 0xF5F6F8),
        secondary: Color(hex: 0xA8ADB6), accent: Color(hex: 0x9FD7FF),
        rowHover: Color(hex: 0x161616), rowSelected: Color(hex: 0x1F1F1F))

    static let light = KoalaPalette(
        isDark: false,
        bg: Color(hex: 0xFFFFFF), sidebar: Color(hex: 0xF7F7F8),
        surface: Color(hex: 0xFFFFFF), surface2: Color(hex: 0xF4F4F5),
        border: Color(hex: 0xE4E4E7), text: Color(hex: 0x0D0D0D),
        secondary: Color(hex: 0x5D5D63), accent: Color(hex: 0x1F6FD1),
        rowHover: Color(hex: 0xEFEFF1), rowSelected: Color(hex: 0xE8E8EB))

    static func resolve(_ scheme: ColorScheme) -> KoalaPalette {
        scheme == .dark ? .dark : .light
    }

    /// Primary buttons are the text colour with the background colour on top:
    /// white on black in light mode, black on white in dark mode.
    var primaryFill: Color { text }
    var onPrimary: Color { bg }

    /// The same palette in the shape the shared views (transcript rows,
    /// markdown, group chat) already take.
    var theme: DesktopTheme {
        DesktopTheme(pane: bg, sidebar: sidebar, divider: border,
                     selectedRow: rowSelected, searchField: surface2,
                     botCard: surface2, userBubble: surface2, onUserBubble: text,
                     text: text, secondary: secondary, accent: accent,
                     inset: isDark ? surface : bg, isDark: isDark)
    }
}

/// The window's appearance setting. `system` follows macOS.
enum AppAppearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: String { rawValue }
    var label: String {
        switch self {
        case .system: return "System"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }
    var colorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }
}
