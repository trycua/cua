// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import CuaSpaces
import CuaSpacesStreaming
import SwiftUI

/// The shared presence cursor over a screen that is not the Space's own
/// stream view. The live stream (`SpaceScreenView`) already draws the SDK's
/// `PresenceOverlay`: every participant in their color with the shape the
/// Space reports, your own cursor at your pointer, idle cursors faded and
/// agents gone when their run ends. Everywhere else (the fixture
/// screen) this modifier puts the same overlay on top, so the sample never
/// draws a cursor of its own that could outlive its participant.
struct PresenceCursors: ViewModifier {
    @ObservedObject var session: LiveStreamSession

    func body(content: Content) -> some View {
        content.overlay { PresenceOverlay(session: session).allowsHitTesting(false) }
    }
}

/// Who is on this Space right now, one initial each, in their cursor colour.
struct PresenceAvatars: View {
    @ObservedObject var session: LiveStreamSession

    var body: some View {
        HStack(spacing: -4) {
            ForEach(everyone) { who in
                Text(String(who.name.prefix(1)).uppercased())
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 18, height: 18)
                    .background(Circle().fill(Color(hexString: who.color)))
                    .overlay(Circle().strokeBorder(.background, lineWidth: 1.5))
                    .help(who.id == session.localParticipant?.id ? "\(who.name) (you)" : who.name)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("On this Space: " + everyone.map(\.name).joined(separator: ", "))
    }

    private var everyone: [Participant] {
        (session.localParticipant.map { [$0] } ?? []) + session.participants
    }
}

extension View {
    /// Draw presence over `source` unless it is the live stream, which
    /// draws it itself.
    func presenceCursors(_ session: LiveStreamSession?, over source: AgentScreenSource) -> some View {
        Group {
            if let session, !source.isLive { modifier(PresenceCursors(session: session)) } else { self }
        }
    }
}

#endif
