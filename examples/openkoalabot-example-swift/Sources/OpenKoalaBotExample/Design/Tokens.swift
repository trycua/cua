// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// Design tokens.
///
/// Mobile values are laid out on a 1024 x 1820.5 pt canvas, rendered at
/// scale 2 (2048x3641 px).
///
/// Desktop values are laid out on a 1280 x 757.5 pt canvas, rendered at
/// scale 2 (2560x1515 px), in a dark and a light theme.
enum DS {

    // MARK: - Canvas

    /// Design-space size of the mobile canvas. scale 2 -> 2048 x 3641 px.
    static let mobileCanvas = CGSize(width: 1024, height: 1820.5)
    /// Design-space size of the desktop canvas. scale 2 -> 2560 x 1515 px.
    static let desktopCanvas = CGSize(width: 1280, height: 757.5)

    // MARK: - Palette (light / mobile)

    static let bg            = Color(hex: 0xFAFAFA)
    static let surface       = Color(hex: 0xEFEFEF)   // bot bubble, chips, icon buttons
    static let surfaceInner  = Color(hex: 0xFFFFFF)   // white inset inside a card
    static let onSurface     = Color(hex: 0x000000)
    static let secondary     = Color(hex: 0x6E6E6E)   // subtitles, timestamps, placeholder
    static let userBubble    = Color(hex: 0x000000)
    static let onUserBubble  = Color(hex: 0xFFFFFF)
    static let signInPill    = Color(hex: 0x161821)
    static let hairline      = Color(hex: 0xE3E3E3)

    // MARK: - Palette (dark / takeover + desktop dark)

    static let darkBg        = Color(hex: 0x000000)
    static let darkChip      = Color(hex: 0x2C2C2E)
    static let onDark        = Color(hex: 0xFFFFFF)

    // Desktop dark shell
    static let dtBg          = Color(hex: 0x121212)
    static let dtSidebar     = Color(hex: 0x1A1A1A)
    static let dtCard        = Color(hex: 0x1E1E1E)
    static let dtHairline    = Color(hex: 0x2A2A2A)
    static let dtSecondary   = Color(hex: 0x9A9A9A)

    // MARK: - Type
    //
    // The platform's system font (SF Pro on macOS) at a fixed size. No custom
    // faces anywhere in the app.

    static func font(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    // MARK: - Mobile metrics (design pt, canvas width 1024)

    static let gutter: CGFloat        = 44
    /// Roster chrome buttons and the thread's back button are nominally two
    /// sizes (~112px and ~128px), so both tokens are kept.
    static let iconButton: CGFloat    = 117   // search / plus, roster
    static let iconButtonLg: CGFloat  = 117   // back / monitor, thread header
    static let headerChipH: CGFloat   = 100
    static let bubbleRadius: CGFloat  = 32
    /// Fixed, not a fraction: every wrapped bubble stops at exactly x=1728px,
    /// i.e. 822pt in design space.
    static let bubbleMax: CGFloat     = 822
    /// The transcript column is inset asymmetrically — bot bubbles hug x=42,
    /// user bubbles stop 66pt short of the right edge.
    static let transcriptLead: CGFloat  = 42
    static let transcriptTrail: CGFloat = 66
    static let bubblePadH: CGFloat    = 31
    static let bubblePadV: CGFloat    = 23
    /// Tuned for where lines break in the fixture transcripts, which matters
    /// more than matching a nominal size.
    static let bodySize: CGFloat      = 44
    static let rowPitch: CGFloat      = 204
    static let rosterAvatar: CGFloat  = 106
    static let pinnedAvatar: CGFloat  = 245
    static let composerH: CGFloat     = 117
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}
