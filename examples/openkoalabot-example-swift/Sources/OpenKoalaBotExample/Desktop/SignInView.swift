// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// The app's user-visible name: the window title, the sidebar brand and the
/// sign-in screen.
enum AppIdentity {
    static let name = "OpenKoalaBots"
}

/// The sign-in gate.
///
/// Local: there is no account system behind it. It says what is true, which
/// is that the app is about to attach to a Space.
struct SignInView: View {
    var onSignIn: () -> Void
    @Environment(\.colorScheme) private var scheme
    private var p: KoalaPalette { .resolve(scheme) }

    var body: some View {
        ZStack {
            p.bg.ignoresSafeArea()
            VStack(spacing: 16) {
                KoalaMark(size: 88).padding(.bottom, 4)
                Text(Self.title)
                    .font(.system(size: 28, weight: .semibold))
                    .foregroundStyle(p.text)
                Text(Self.tagline)
                    .font(.system(size: 14))
                    .foregroundStyle(p.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
                Button(action: onSignIn) {
                    Text(Self.action)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(p.onPrimary)
                        .frame(width: 220, height: 40)
                        .background(Capsule().fill(p.primaryFill))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.defaultAction)
                .accessibilityLabel(Self.action)
                .padding(.top, 12)
            }
            .padding(40)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    static let title = AppIdentity.name
    static let tagline =
        "Koala Bots work on a real computer in your Space, and keep working while you do."
    static let action = "Get started"
}
