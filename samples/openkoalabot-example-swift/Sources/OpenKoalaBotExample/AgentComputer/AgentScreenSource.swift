// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import SwiftUI

/// What an Agent Computer tier draws inside its screen rectangle.
///
/// The tiers are used by two callers with opposite needs. The export path
/// (`Export/main.swift`) renders thirteen PNGs offscreen, from fixtures, with
/// no Space created and no network, and it must stay deterministic. The running
/// app wants live rcdp pixels in the same rectangle. So the pixel source is a
/// *parameter* of the tier views rather than something they reach out and get,
/// and it defaults to `.fixture` so every existing call site (including
/// every export screen) is unchanged.
///
/// `.live` carries the session itself rather than the provider, because the
/// session is the thing that must be *shared*: tier 2 and tier 3 of the same
/// Bot, and the PiP pop-out, are one `LiveStreamSession` with several observers.
/// Handing each tier a provider would have each tier open its own rcdp session
/// against the same target: legal (see `Streaming/FRICTION.md` §12) but three
/// decodes for one picture.
@MainActor
struct AgentScreenSource {
    enum Kind {
        /// The fixture remote desktop (export screen 5).
        case fixture
        /// Live decoded frames from the Space.
        case live(LiveStreamSession)
    }

    var kind: Kind

    static let fixture = AgentScreenSource(kind: .fixture)
    static func live(_ session: LiveStreamSession) -> AgentScreenSource {
        AgentScreenSource(kind: .live(session))
    }

    var isLive: Bool {
        if case .live = kind { return true }
        return false
    }

    var session: LiveStreamSession? {
        if case let .live(session) = kind { return session }
        return nil
    }
}

/// The screen rectangle itself, in whichever of the two flavours it was given.
///
/// Every tier (mobile tier 2, mobile tier 3, the desktop right panel, the
/// desktop takeover) goes through this one view, so "is this surface live?" is
/// answered in exactly one place and a tier cannot accidentally be half-wired.
@MainActor
struct AgentScreen: View {
    var source: AgentScreenSource = .fixture
    /// Tier 3 with control taken is `true`; a tier-2 preview is `false`.
    var isInteractive: Bool = false
    /// The stream's own toolbar (source picker, frame counter, pop-out). Off in
    /// the tiers, which have their own chrome.
    var showsControls: Bool = false
    /// Only meaningful for `.fixture`: the fixture screen's detail scale.
    var fixtureScale: CGFloat = 1.0

    var body: some View {
        switch source.kind {
        case .fixture:
            RemoteScreen(scale: fixtureScale)
        case let .live(session):
            SpaceScreenView(session: session,
                            isInteractive: isInteractive,
                            showsControls: showsControls)
        }
    }
}
#endif
