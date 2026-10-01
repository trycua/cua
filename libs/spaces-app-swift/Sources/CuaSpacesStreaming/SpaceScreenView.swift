// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import SwiftUI

/// The whole live-screen surface, ready to drop into the Agent Computer tiers:
/// the stream, the desktop/window switcher, and the PiP pop-out control.
///
/// This is the one view the rest of the app needs to know about. Everything
/// below it — the RCDP client, the decoder, the coordinate mapping, the PiP
/// panel — is private to `Streaming/`.
public struct SpaceScreenView: View {
    @ObservedObject public var session: LiveStreamSession
    @StateObject private var pip = StreamPiPController()
    @Environment(\.presenceName) private var presenceName

    /// Whether the stream accepts input, or is a read-only preview. Tier 2
    /// (pinned preview) wants `false`; tier 3 (takeover) wants `true`.
    public var isInteractive: Bool = true
    public var showsControls: Bool = true

    public init(session: LiveStreamSession, isInteractive: Bool = true,
                showsControls: Bool = true) {
        _session = ObservedObject(wrappedValue: session)
        self.isInteractive = isInteractive
        self.showsControls = showsControls
    }

    public var body: some View {
        VStack(spacing: 0) {
            if showsControls {
                controls
            }
            ZStack(alignment: .topLeading) {
                if session.isPoppedOut {
                    poppedOutPlaceholder
                } else {
                    LiveStreamView(session: session, isInteractive: isInteractive,
                                   showsCursorOverlay: session.presenceView == nil)
                    PresenceOverlay(session: session)
                }
                // Idle only until `.task` starts the stream: no badge for it.
                if !session.status.isLive && session.status != .idle {
                    StreamStatusBadge(status: session.status).padding(10)
                } else if isInteractive, let failure = session.inputFailure {
                    StreamInputFailureBadge(reason: failure).padding(10)
                }
            }
        }
        .background(Color.black)
        .task {
            if let presenceName { await session.joinPresence(as: presenceName) }
            await session.refreshWindows()
            if session.status == .idle { await session.start() }
        }
        // A stream whose view is gone is a poll loop nobody is looking at: the
        // desktop source keeps pulling a ~1.6 MB PNG twice a second forever
        // (§10), and the RCDP source keeps a WebSocket and a decoder alive.
        // Navigating away from the Agent Computer used to leave both running
        // for the life of the process. The one exception is the PiP — being
        // popped out is *precisely* the case where the session must outlive the
        // view that started it. `FRICTION.md` §52.
        .onDisappear {
            guard !session.isPoppedOut else { return }
            Task { await session.stop() }
        }
    }

    // MARK: - Controls

    private var controls: some View {
        HStack(spacing: 10) {
            StreamSourcePicker(session: session)
            Spacer(minLength: 8)
            Text(dimensionsLabel)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
            Button {
                pip.toggle(session: session)
            } label: {
                Label(pip.isOpen ? "Pop in" : "Pop out",
                      systemImage: pip.isOpen ? "pip.exit" : "pip.enter")
            }
            .help("Detach this stream into a floating always-on-top window. The session keeps running.")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.regularMaterial)
    }

    private var dimensionsLabel: String {
        guard session.lastFrameDimensions.width > 0 else { return "—" }
        return "\(Int(session.lastFrameDimensions.width))x\(Int(session.lastFrameDimensions.height))"
            + "  ·  \(session.decodedFrameCount) frames"
    }

    private var poppedOutPlaceholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "pip").font(.system(size: 28)).foregroundStyle(.secondary)
            Text("Playing in a floating window").font(.system(size: 12)).foregroundStyle(.secondary)
            Button("Bring it back") { pip.popIn() }.controlSize(.small)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// Input the Space refused (a click that went nowhere), shown over a
/// stream that is otherwise live.
public struct StreamInputFailureBadge: View {
    public var reason: String

    public var body: some View {
        Label("Input not delivered: \(reason)", systemImage: "cursorarrow.slash")
            .font(.system(size: 11, weight: .medium))
            .lineLimit(2)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.black.opacity(0.65), in: Capsule())
            .foregroundStyle(.white)
            .accessibilityIdentifier("stream-input-failure")
    }
}

/// Desktop ↔ single-window switcher.
///
/// Both entries stream live pixels, but over different transports: a window
/// comes from RCDP's H.264 session, the desktop from the driver's full-display
/// capture, because RCDP has no desktop target. The picker deliberately does not
/// advertise that split to the user — it is a source, not a protocol choice —
/// but `FRICTION.md` records what it cost.
public struct StreamSourcePicker: View {
    @ObservedObject public var session: LiveStreamSession

    public var body: some View {
        Menu {
            Button {
                Task { await session.select(.desktop) }
            } label: {
                Label("Full desktop", systemImage: "menubar.dock.rectangle")
            }
            if !session.windows.isEmpty {
                Divider()
                ForEach(session.windows) { window in
                    Button {
                        Task { await session.select(.window(window)) }
                    } label: {
                        Text(window.displayName)
                    }
                }
            }
            Divider()
            Button("Refresh windows") { Task { await session.refreshWindows() } }
        } label: {
            Label(session.source.label, systemImage: icon)
                .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .frame(maxWidth: 320, alignment: .leading)
    }

    private var icon: String {
        switch session.source {
        case .desktop: return "menubar.dock.rectangle"
        case .window: return "macwindow"
        }
    }
}
#endif
