// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CoreImage
import CuaSpacesFFI
import CuaSpacesStreaming
import SwiftUI

/// What the preview card shows over (or instead of) a Space's live desktop
/// (the core's `appDesktopCover`): the Space's latest thumbnail, strongly
/// blurred and slightly dimmed, with "Connecting…", a Connect button or the
/// Space's own line centered on it. With no thumbnail yet: plain black
/// while connecting (or waiting for Connect), the card's quiet fill for a
/// Space that cannot stream.
struct DesktopCoverView: View {
    let cover: AppDesktopCover
    /// The Space's latest thumbnail (`SpaceThumbnails`), if any.
    let image: NSImage?
    /// While it is created: overall progress in thousandths (the ring).
    var progress: UInt32?
    /// Under the line while it downloads.
    var progressText: String?
    var onButton: () -> Void = {}
    @Environment(\.colorScheme) private var colorScheme

    /// Blur radius as a share of the image's long edge (strong: shapes
    /// and colors only, no text).
    static let blurShare: CGFloat = 0.035
    static let dim = 0.28
    private static let blurs = NSCache<NSImage, NSImage>()
    private static let context = CIContext()

    /// The image blurred once (Core Image, edges clamped so they stay
    /// opaque), cached per image.
    static func blurred(_ image: NSImage) -> NSImage {
        if let done = blurs.object(forKey: image) { return done }
        guard let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return image }
        let input = CIImage(cgImage: cg)
        let radius = max(input.extent.width, input.extent.height) * blurShare
        guard let output = input.clampedToExtent()
            .applyingGaussianBlur(sigma: radius)
            .cropped(to: input.extent) as CIImage?,
            let out = context.createCGImage(output, from: input.extent) else { return image }
        let result = NSImage(cgImage: out, size: image.size)
        blurs.setObject(result, forKey: image)
        return result
    }

    /// On the blurred image or on black, text and buttons draw for a dark
    /// background.
    private var onDark: Bool { image != nil || cover.kind != .status }

    var body: some View {
        ZStack {
            background
            content
                .padding(16)
                .environment(\.colorScheme, onDark ? .dark : colorScheme)
        }
        .clipped()
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("space-cover-\(String(describing: cover.kind))")
    }

    @ViewBuilder private var background: some View {
        if let image {
            Color.black
                .overlay {
                    Image(nsImage: Self.blurred(image))
                        .resizable()
                        .interpolation(.medium)
                        .aspectRatio(contentMode: .fill)
                }
                .overlay(Color.black.opacity(Self.dim))
                .accessibilityHidden(true)
        } else if cover.kind == .status {
            Rectangle().fill(.quaternary)
        } else {
            Color.black
        }
    }

    private var content: some View {
        VStack(spacing: 12) {
            if cover.text != nil || progress != nil {
                HStack(spacing: 8) {
                    if let progress { ProgressRing(permille: progress, size: 16) }
                    if let text = cover.text {
                        Text(text)
                            .font(cover.kind == .status ? .body : .title3)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .accessibilityIdentifier("space-cover-text")
                    }
                }
            }
            // While it downloads: "4.2 of 23.9 GB · 85 MB/s · about 4 min".
            if let progressText {
                Text(progressText)
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .accessibilityIdentifier("space-progress-text")
            }
            if let button = cover.button {
                Button(button, action: onButton)
                    .buttonStyle(.borderedProminent)
                    .controlSize(cover.kind == .connect ? .large : .regular)
                    .help(cover.buttonHelp ?? "")
                    .accessibilityIdentifier(cover.kind == .connect ? "space-connect" : "space-retry")
            }
        }
    }
}

/// Reads a stream session's phase for the core's cover: none without a
/// session, and "connecting" until the first frame arrives (the transport
/// says streaming as soon as it opens).
struct StreamPhaseReader<Content: View>: View {
    var session: LiveStreamSession?
    @ViewBuilder var content: (AppStreamPhase) -> Content

    var body: some View {
        if let session { Observed(session: session, content: content) } else { content(.noSession) }
    }

    static func phase(_ status: LiveStreamSession.Status, hasFrame: Bool) -> AppStreamPhase {
        switch status {
        case .idle: return .idle
        case .connecting: return .connecting
        case .streaming: return hasFrame ? .streaming : .connecting
        case .suspended: return .suspended
        case .failed: return .failed
        }
    }

    private struct Observed: View {
        @ObservedObject var session: LiveStreamSession
        let content: (AppStreamPhase) -> Content
        var body: some View {
            content(StreamPhaseReader.phase(session.status, hasFrame: session.frame != nil))
        }
    }
}
