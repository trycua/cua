// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// The **fixture** remote desktop (export screen 5): a browser at crm.acme.com
/// over a grey macOS wallpaper.
///
/// This is no longer what the running app shows. The tiers take an
/// `AgentScreenSource`, and at runtime that is `.live`, which mounts
/// `SpaceScreenView` and decodes real rcdp frames. This fixture remains
/// the source for the thirteen exported PNGs, which must render deterministically
/// with no Space, no network and no window server.
struct RemoteScreen: View {
    var scale: CGFloat = 1.0

    var body: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            ZStack {
                LinearGradient(colors: [Color(hex: 0xBDBDBD), Color(hex: 0x8E8E8E),
                                        Color(hex: 0xC8C8C8), Color(hex: 0x7A7A7A)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)

                // Window-relative metrics. Authoring these against the outer
                // frame overflowed the title bar and collapsed two of the
                // three traffic lights. Fractions are of the window width.
                let bw = w * 0.622
                VStack(spacing: 0) {
                    HStack(spacing: bw * 0.0235) {
                        Circle().fill(Color(hex: 0xFF5F57))
                            .frame(width: bw * 0.0235, height: bw * 0.0235)
                        Circle().fill(Color(hex: 0xFEBC2E))
                            .frame(width: bw * 0.0235, height: bw * 0.0235)
                        Circle().fill(Color(hex: 0x28C840))
                            .frame(width: bw * 0.0235, height: bw * 0.0235)
                        Spacer(minLength: 0)
                        Text("crm.acme.com")
                            .font(DS.font(bw * 0.034))
                            .foregroundStyle(Color(hex: 0x555555))
                            .frame(width: bw * 0.632, height: bw * 0.055)
                            .background(Capsule().fill(.white))
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, bw * 0.034)
                    .frame(height: bw * 0.098)
                    .background(Color(hex: 0xEDEDED))

                    VStack(spacing: bw * 0.036) {
                        Text("Sign in to Acme CRM")
                            .font(DS.font(bw * 0.058, .bold))
                            .foregroundStyle(.black)
                            .padding(.bottom, bw * 0.008)
                        field("alex@acme.com", w: bw, mono: false)
                        field("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}", w: bw, mono: true)
                        Text("Sign in")
                            .font(DS.font(bw * 0.047, .medium))
                            .foregroundStyle(.white)
                            .frame(maxWidth: .infinity)
                            .frame(height: bw * 0.112)
                            .background(RoundedRectangle(cornerRadius: bw * 0.017, style: .continuous)
                                .fill(DS.signInPill))
                    }
                    .padding(.horizontal, bw * 0.122)
                    .padding(.vertical, bw * 0.062)
                    .background(Color.white)
                }
                .frame(width: bw)
                .clipShape(RoundedRectangle(cornerRadius: bw * 0.022, style: .continuous))
                .shadow(color: .black.opacity(0.28), radius: bw * 0.028, y: bw * 0.011)

                Image(systemName: "cursorarrow")
                    .font(.system(size: w * 0.055, weight: .black))
                    .foregroundStyle(.white)
                    .shadow(color: .black.opacity(0.6), radius: 2)
                    .position(x: w * 0.885, y: h * 0.63)
            }
        }
    }

    private func field(_ text: String, w bw: CGFloat, mono: Bool) -> some View {
        HStack {
            Text(text)
                .font(DS.font(bw * 0.043))
                .foregroundStyle(mono ? Color(hex: 0x777777) : Color(hex: 0x333333))
            Spacer(minLength: 0)
        }
        .padding(.horizontal, bw * 0.036)
        .frame(height: bw * 0.101)
        .background(RoundedRectangle(cornerRadius: bw * 0.016, style: .continuous)
            .fill(Color(hex: 0xEFEFEF)))
    }
}
