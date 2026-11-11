// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import SwiftUI

/// A Space's live desktop in its own window ("Open").
struct SpaceWindowView: View {
    let model: AppModel
    let spaceId: String
    @State private var session: LiveStreamSession?
    @State private var error: String?
    /// The Space was listed while this window was open: once it is gone
    /// (deleted), the window closes instead of going blank.
    @State private var seen = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let space = model.spaces.first { $0.id == spaceId }
        Group {
            if let error {
                ZStack {
                    Rectangle().fill(.quaternary)
                    Text(error).foregroundStyle(.secondary)
                }
            } else if let space {
                // Open is a request to connect: the same cover as the
                // detail's until the first frame.
                StreamPhaseReader(session: session) { phase in
                    let cover = model.cover(model.detail(space), requested: true, stream: phase)
                    ZStack {
                        if let session { SpaceScreenView(session: session, isInteractive: true, showsControls: true) }
                        if cover.kind != .stream {
                            DesktopCoverView(cover: cover, image: model.thumbnails[spaceId]) {
                                // Try again.
                                guard let session else { return }
                                Task {
                                    await session.stop()
                                    await session.start()
                                }
                            }
                        }
                    }
                }
            } else {
                ZStack {
                    Rectangle().fill(.quaternary)
                    Text(seen ? "This Space was deleted." : "This Space isn\u{2019}t listed on this Mac.")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .frame(minWidth: 640, minHeight: 400)
        .navigationTitle(space?.name ?? spaceId)
        .onAppear { if space != nil { seen = true } }
        .onChange(of: space != nil) { _, listed in
            if listed {
                seen = true
            } else if seen {
                dismiss()
            }
        }
        .onChange(of: space?.status == .deleting) { _, deleting in
            // Deleting: the stream stops now.
            guard deleting, let session else { return }
            self.session = nil
            Task { await session.stop() }
        }
        .task(id: spaceId) {
            do {
                session = LiveStreamSession(provider: try await model.backend.streamProvider(id: spaceId))
            } catch {
                self.error = LiveSpacesBackend.words(error)
            }
        }
    }
}
