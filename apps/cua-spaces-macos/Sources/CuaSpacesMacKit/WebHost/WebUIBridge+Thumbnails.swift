// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation

/// A Space's latest thumbnail for the page: the same image the notch tiles
/// and the native preview cover draw (`SpaceThumbnails`, over the SDK's and
/// the daemon's cache). Nothing here captures a screen itself.
extension WebUIBridge {
    /// `spaces.thumbnail {spaceId, maxAgeMs?}`: asks for an image no older
    /// than `maxAgeMs` (the policy's background interval when absent, as
    /// `SpaceDetailView` asks for its cover), then answers the latest one
    /// as `{url, capturedAtMs}` (a JPEG `data:` URL), or null while there
    /// is none.
    func routeThumbnail(_ method: String, _ args: [String: Any]) async throws -> Any? {
        switch method {
        case "spaces.thumbnail": return try await thumbnail(args)
        default: return nil
        }
    }

    private func thumbnail(_ args: [String: Any]) async throws -> Any {
        let id = try spaceId(args)
        guard model.spaces.contains(where: { $0.id == id }) else { throw Failure.notFound("no Space \(id)") }
        let maxAgeMs = (args["maxAgeMs"] as? NSNumber)?.doubleValue
            ?? Double(SpaceThumbnails.policy.backgroundIntervalMs)
        await model.thumbnails.refresh(id, maxAge: maxAgeMs / 1000)
        guard let entry = model.thumbnails.entries[id], let url = Self.jpegDataURL(entry.image) else { return NSNull() }
        return ["url": url, "capturedAtMs": entry.capturedAt.timeIntervalSince1970 * 1000] as [String: Any]
    }

    /// The image as a JPEG `data:` URL (thumbnails are small: the policy's
    /// `maxDimension` on the long edge).
    static func jpegDataURL(_ image: NSImage) -> String? {
        guard let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        let rep = NSBitmapImageRep(cgImage: cg)
        guard let data = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.8]), !data.isEmpty else { return nil }
        return "data:image/jpeg;base64,\(data.base64EncodedString())"
    }
}
