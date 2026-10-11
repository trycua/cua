// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation

/// A Space's Stream section: its windows, its primary display and its apps'
/// icons. The rows themselves (labels, icons, resolution, buttons) are the
/// core's `appStreamSection`, the same ones the Tauri app draws; this only
/// fetches what they are built from.
@MainActor
final class StreamRowsModel: ObservableObject {
    typealias Windows = @Sendable () async throws -> [StreamWindow]
    typealias Display = @Sendable () async -> AppStreamDisplay?
    /// Icons for many apps in one call, in order (the SDK's `Space.appIcons`:
    /// its one cache, every miss in one guest round trip).
    typealias Icons = @Sendable (_ requests: [SpaceAppIconRequest]) async -> [Data?]

    let os: AppSpaceOs
    let osName: String?
    /// `nil` while the first list loads.
    @Published private(set) var windows: [StreamWindow]?
    @Published private(set) var failed = false
    @Published private(set) var display: AppStreamDisplay?
    /// The current rows' icons, decoded, per app ([`iconKey`]); rebuilt from
    /// the SDK's cache on every load (this keeps no cache of its own). An
    /// app the Space has no icon for is absent: its rows show none.
    @Published private(set) var icons: [String: NSImage] = [:]

    private let listWindows: Windows
    private let primaryDisplay: Display
    private let appIcons: Icons

    init(os: AppSpaceOs, osName: String?, windows: @escaping Windows,
         display: @escaping Display, icons: @escaping Icons) {
        self.os = os
        self.osName = osName
        self.listWindows = windows
        self.primaryDisplay = display
        self.appIcons = icons
    }

    /// From the app's backend.
    convenience init(space: AppSpace, provider: SpaceStreamSourceProviding, backend: SpacesBackend) {
        let id = space.id
        self.init(os: space.os, osName: space.osName,
                  windows: { try await provider.availableWindows() },
                  display: { await backend.primaryDisplay(id: id) },
                  icons: { await backend.appIcons(id: id, requests: $0) })
    }

    /// The rows whose panel is open, from the open panels
    /// (``StreamSource/pipKey``s): the panel set reports them to the core as
    /// a `synced` event, the same one the Tauri shell sends.
    func openRows(openKeys: Set<String>) -> [String] {
        let rows = (openKeys.contains(StreamSource.desktop.pipKey) ? [appStreamDesktopRowId()] : [])
            + (windows ?? []).filter { openKeys.contains(StreamSource.window($0).pipKey) }.map(\.id)
        return appStreamPipReduce(open: [], event: .synced(rows: rows))
    }

    /// The section for these open panels (``StreamSource/pipKey``s).
    func section(openKeys: Set<String> = [], query: String = "") -> AppStreamSection {
        let open = openRows(openKeys: openKeys)
        return appStreamSection(input: AppStreamSectionInput(
            windows: windows.map { $0.map(Self.remote) }, failed: failed, display: display,
            os: os, osName: osName, open: open, query: query))
    }

    /// The window a row stands for.
    func window(id: String) -> StreamWindow? { windows?.first { $0.id == id } }

    /// Reads the windows and the display once.
    /// How long a window listing may take: a hung one would keep "Looking
    /// for this Space's windows…" up forever; past this it reads as failed.
    static var windowsTimeout: Double = 15

    func refresh() async {
        async let d = primaryDisplay()
        do {
            windows = try await withTimeout(seconds: Self.windowsTimeout) { [listWindows] in
                try await listWindows()
            }.get()
            failed = false
        } catch {
            if windows == nil { windows = [] }
            failed = true
        }
        if let d = await d { display = d }
    }

    /// Refreshes every `seconds` until cancelled (the view went away).
    func poll(every seconds: Double = 5) async {
        while !Task.isCancelled {
            await refresh()
            await loadIcons()
            try? await Task.sleep(for: .seconds(seconds))
        }
    }

    /// The current rows' icons in one SDK call (cached there: a repeat costs
    /// microseconds), decoded for the view.
    func loadIcons() async {
        var requests: [SpaceAppIconRequest] = []
        var keys: [String] = []
        for row in section().rows {
            guard case let .app(appName, appId, pid) = row.icon else { continue }
            let key = Self.iconKey(appName: appName, appId: appId)
            guard !keys.contains(key) else { continue }
            keys.append(key)
            requests.append(SpaceAppIconRequest(appName: appName, appId: appId, pid: pid))
        }
        guard !requests.isEmpty else { icons = [:]; return }
        let answers = await appIcons(requests)
        var next: [String: NSImage] = [:]
        for (key, data) in zip(keys, answers) {
            if let data, let image = Self.decode(data) { next[key] = image }
        }
        icons = next
    }

    /// A row's app icon, when the Space has one.
    func image(for icon: AppStreamRowIcon) -> NSImage? {
        guard case let .app(appName, appId, _) = icon else { return nil }
        return icons[Self.iconKey(appName: appName, appId: appId)]
    }

    /// Windows of one app share an icon.
    static func iconKey(appName: String, appId: String) -> String {
        "\(appName.lowercased())\u{1f}\(appId.lowercased())"
    }

    /// PNG or SVG bytes as an image; `nil` for bytes AppKit cannot read.
    static func decode(_ data: Data) -> NSImage? {
        guard !data.isEmpty, let image = NSImage(data: data),
              image.size.width > 0, image.size.height > 0 else { return nil }
        return image
    }

    static func remote(_ w: StreamWindow) -> AppRemoteWindow {
        func px(_ v: CGFloat) -> UInt32? { v > 0 ? UInt32(v.rounded()) : nil }
        return AppRemoteWindow(
            id: w.id, appName: w.app, title: w.title, visible: true, appId: w.appID,
            targetEpoch: w.epoch, widthPx: px(w.surfaceSize.width), heightPx: px(w.surfaceSize.height),
            pid: w.processID > 0 ? w.processID : nil)
    }
}
