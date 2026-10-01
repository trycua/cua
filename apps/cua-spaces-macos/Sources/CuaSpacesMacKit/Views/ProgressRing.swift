// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// A thin determinate ring: a Space being created (the sidebar row, the
/// detail's preview). `permille` is the core's overall progress.
struct ProgressRing: View {
    let permille: UInt32
    var size: CGFloat = 12
    var line: CGFloat = 2

    var body: some View {
        let f = Double(min(permille, 1000)) / 1000
        ZStack {
            Circle().stroke(Color.secondary.opacity(0.25), lineWidth: line)
            Circle()
                .trim(from: 0, to: f)
                .stroke(Color.accentColor, style: StrokeStyle(lineWidth: line, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: size - line, height: size - line)
        .padding(line / 2)
        .animation(.easeOut(duration: 0.25), value: f)
        .accessibilityElement()
        .accessibilityLabel("\(Int(f * 100)) percent")
    }
}
