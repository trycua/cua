// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
import Foundation
#if canImport(AppKit)
import AppKit

/// The Computer pane's window list icons, from the Space itself.
///
/// The lookup and the cache are the SDK's (`Space.appIcons`: one icon cache
/// for every cua UI, keyed by the app and the Space's image, every miss in
/// one guest round trip). This keeps no cache: each `load` asks for the
/// current list's apps in one call and replaces the decoded images. A
/// Space with no icon for an app gives `nil`, and the row then shows no icon
/// at all: never a stand-in glyph.
@MainActor
final class WindowIcons: ObservableObject {
    typealias Fetch = ([AppIconRequest]) async -> [AppIcon?]

    private let fetch: Fetch
    @Published private(set) var images: [String: NSImage] = [:]

    init(fetch: @escaping Fetch) { self.fetch = fetch }

    /// Icons from a Space handle.
    convenience init(space: CuaSpaces.Space) {
        self.init { requests in
            (try? await space.appIcons(requests)) ?? requests.map { _ in nil }
        }
    }

    /// Windows of one app share an icon.
    static func key(_ w: StreamWindow) -> String {
        "\(w.app.lowercased())\u{1f}\(w.appID.lowercased())"
    }

    func image(for w: StreamWindow) -> NSImage? { images[Self.key(w)] }

    /// The icons of `windows`' apps, in one SDK call.
    func load(_ windows: [StreamWindow]) async {
        var keys: [String] = []
        var requests: [AppIconRequest] = []
        for w in windows where !keys.contains(Self.key(w)) {
            keys.append(Self.key(w))
            requests.append(AppIconRequest(app: w.app, appID: w.appID, processID: w.processID))
        }
        guard !requests.isEmpty else { images = [:]; return }
        let icons = await fetch(requests)
        var next: [String: NSImage] = [:]
        for (key, icon) in zip(keys, icons) {
            if let icon, let image = Self.decode(icon) { next[key] = image }
        }
        images = next
    }

    /// PNG or SVG bytes as an image (AppKit draws both); `nil` for bytes it
    /// cannot read.
    static func decode(_ icon: AppIcon) -> NSImage? {
        guard !icon.data.isEmpty, let image = NSImage(data: icon.data),
              image.size.width > 0, image.size.height > 0 else { return nil }
        return image
    }
}
#endif
