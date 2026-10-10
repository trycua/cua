// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesNotchUI
import SwiftUI

/// A Space's OS icon from the core's id: the system symbol when the core
/// names one (macOS: `apple.logo`), else the core's SVG as a template image.
/// The notch tiles' icon view, looked up in the core here.
struct OsIconImage: View {
    let id: String
    var size: CGFloat = 11

    var body: some View {
        NotchOsIconImage(id: id, size: size)
            .environment(\.notchOsIcon, NotchModel.osIcon)
    }
}
