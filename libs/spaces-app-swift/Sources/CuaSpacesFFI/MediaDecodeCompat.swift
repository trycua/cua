// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Decoded media ships with Cua Spaces; these keep the method spellings the
// SDK had (`client.openMediaDecoded(...)`) over the export's free functions.

import CuaSDK

public extension SpacesdClient {
    /// Opens a media session and delivers decoded video (packed BGRA) and
    /// control events to `frames`.
    func openMediaDecoded(options: MediaOpenOptions, frames: DecodedFrameSink) async throws -> MediaSession {
        try await spacesdOpenMediaDecoded(client: self, options: options, frames: frames)
    }

    /// `openMediaDecoded` that also delivers decoded PCM audio to `pcm`.
    func openMediaDecodedWithAudio(
        options: MediaOpenOptions, frames: DecodedFrameSink, pcm: PcmSink
    ) async throws -> MediaSession {
        try await spacesdOpenMediaDecodedWithAudio(client: self, options: options, frames: frames, pcm: pcm)
    }
}
