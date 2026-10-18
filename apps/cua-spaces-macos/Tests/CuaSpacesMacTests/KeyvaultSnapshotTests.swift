// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import ImageIO
@testable import CuaSpacesMacKit
import SwiftUI
import Testing
import UniformTypeIdentifiers

/// Snapshots of the vault list (grouped, multi-select, search, names hidden),
/// the unlock prompt and the delete confirmation, on the simulated vault of
/// `KeyvaultFixtures` (no real secret anywhere), rendered offscreen. The
/// `Recorder` collects frames, one real state of the real views each, into
/// the GIFs `KeyvaultReviewTests.exportScreenshotsAndGifs` writes.
@MainActor
@Suite("Keyvault snapshots", .serialized)
struct KeyvaultSnapshotTests {
    let snap = SnapshotTests()
    static let list = CGSize(width: 780, height: 560)

    func model(_ overview: KeyvaultOverview = KeyvaultFixtures.overview(),
               icons: Bool = false) async -> (AppModel, FakeKeyvault) {
        let fake = FakeKeyvault(overview)
        let m = ViewModelTests().makeModel(kv: fake)
        if icons {
            m.keyvault.appIcons = AppIconCache(
                dir: FileManager.default.temporaryDirectory.appendingPathComponent("cua-snap-icons-\(UUID().uuidString)"),
                resolver: { id, name in
                    var r = AppIconCache.workspace(id, name)
                    if r == nil, id == "arc", let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "company.thebrowser.Browser") {
                        r = AppIconSource(bundleId: "company.thebrowser.Browser", version: "0") {
                            AppIconCache.png(NSWorkspace.shared.icon(forFile: url.path), side: 64)
                        }
                    }
                    return r
                })
        }
        await m.keyvault.refresh()
        if icons { await m.keyvault.loadIcons() }
        return (m, fake)
    }

    var useRealIcons: Bool { ProcessInfo.processInfo.environment["KEYVAULT_REAL_ICONS"] == "1" }

    func vault(_ m: AppModel) -> some View {
        VaultList(keyvault: m.keyvault, page: m.keyvault.page)
    }

    // MARK: - The list

    @Test func vaultGroupedByApp() async throws {
        let (m, _) = await model(icons: useRealIcons)
        // Open one site and the files of Chrome, the way a user browses.
        m.keyvault.send(.toggleOpen(key: "chrome|github.com"))
        m.keyvault.send(.toggleOpen(key: "chrome|\u{1}files"))
        try snap.assertSnapshot(vault(m), "keyvault-vault-grouped", size: Self.list)
    }

    @Test func vaultMultiSelectWithTheBatchBar() async throws {
        let (m, _) = await model(icons: useRealIcons)
        m.keyvault.send(.toggleOpen(key: "chrome|github.com"))
        m.keyvault.send(.toggleGroup(key: "chrome|github.com"))
        m.keyvault.send(.toggle(id: "c-no-1"))
        #expect(m.keyvault.vaultView.selection.title == "7 selected")
        try snap.assertSnapshot(vault(m), "keyvault-vault-multiselect", size: Self.list)
    }

    @Test func vaultSearch() async throws {
        let (m, _) = await model(icons: useRealIcons)
        m.keyvault.query = "notion"
        try snap.assertSnapshot(vault(m), "keyvault-vault-search", size: Self.list)
        m.keyvault.query = "cookie github"
        try snap.assertSnapshot(vault(m), "keyvault-vault-search-type", size: Self.list)
    }

    @Test func vaultNamesHidden() async throws {
        let (m, _) = await model(KeyvaultFixtures.overview(namesVisible: false), icons: useRealIcons)
        try snap.assertSnapshot(vault(m), "keyvault-vault-hidden", size: Self.list)
    }

    // MARK: - The prompts

    @Test func unlockPrompt() async throws {
        let one = kvUnlockPromptAlways(count: 1, name: "user_session")
        try snap.assertSnapshot(UnlockPromptSheet(prompt: one) { _ in }, "keyvault-unlock-prompt",
                                size: CGSize(width: 440, height: 220))
        let batch = kvUnlockPromptAlways(count: 6, name: nil)
        try snap.assertSnapshot(UnlockPromptSheet(prompt: batch) { _ in }, "keyvault-unlock-prompt-batch",
                                size: CGSize(width: 440, height: 220))
    }

    @Test func deleteConfirm() async throws {
        let c = kvDeleteConfirm(count: 6, liveCopies: 1)
        try snap.assertSnapshot(DeleteConfirmSheet(confirm: c, onDelete: {}, onCancel: {}), "keyvault-delete-confirm",
                                size: CGSize(width: 420, height: 170))
    }
}

/// Collects frames (a view and a caption under it) and writes a GIF.
@MainActor
final class Recorder {
    let snap: SnapshotTests
    let size: CGSize
    let dir: URL
    private var frames: [(CGImage, Double)] = []

    init(snap: SnapshotTests, size: CGSize, dir: URL) {
        self.snap = snap
        self.size = size
        self.dir = dir
    }

    /// One frame: `view`, a sheet over it (the window dimmed behind, the way
    /// macOS presents one) and the caption under both. The sheet is rendered
    /// on its own and composited as an image, so the text field under it
    /// draws as it does without a sheet.
    func frame<V: View, O: View>(_ caption: String, _ view: V, over sheet: O, sheetSize: CGSize, delay: Double) {
        let base = snap.render(view.frame(width: size.width, height: size.height), size: size)
        let top = snap.render(sheet.background(Color(nsColor: .windowBackgroundColor)), size: sheetSize)
        let scale: CGFloat = 2
        let out = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                                   bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        out.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: out)
        NSColor.white.setFill()
        CGRect(origin: .zero, size: size).fill()
        base.draw(in: CGRect(origin: .zero, size: size))
        NSColor.black.withAlphaComponent(0.28).setFill()
        CGRect(origin: .zero, size: size).fill()
        let rect = CGRect(x: (size.width - sheetSize.width) / 2, y: size.height - sheetSize.height - 96,
                          width: sheetSize.width, height: sheetSize.height)
        let shadow = NSShadow()
        shadow.shadowBlurRadius = 24
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.35)
        shadow.shadowOffset = NSSize(width: 0, height: -6)
        NSGraphicsContext.saveGraphicsState()
        shadow.set()
        NSBezierPath(roundedRect: rect, xRadius: 12, yRadius: 12).addClip()
        top.draw(in: rect)
        NSGraphicsContext.restoreGraphicsState()
        NSGraphicsContext.restoreGraphicsState()
        add(caption, Image(nsImage: NSImage(cgImage: out.cgImage!, size: size)).resizable().frame(width: size.width, height: size.height),
            delay: delay)
    }

    func frame<V: View>(_ caption: String, _ view: V, delay: Double) { add(caption, view, delay: delay) }

    private func add<V: View>(_ caption: String, _ view: V, delay: Double) {
        let bar: CGFloat = 34
        let composed = VStack(spacing: 0) {
            view.frame(width: size.width, height: size.height)
            Text(caption)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.white)
                .frame(width: size.width, height: bar)
                .background(Color(white: 0.12))
        }
        let rep = snap.render(composed, size: CGSize(width: size.width, height: size.height + bar))
        if let cg = rep.cgImage, let flat = Self.opaque(cg) { frames.append((flat, delay)) }
    }

    /// The frame on white, with no alpha: a GIF has none, and what the render
    /// left transparent would show black.
    static func opaque(_ image: CGImage) -> CGImage? {
        guard let ctx = CGContext(data: nil, width: image.width, height: image.height, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return ctx.makeImage()
    }

    func finish(_ name: String) throws {
        defer { frames = [] }
        guard let dest = CGImageDestinationCreateWithURL(dir.appendingPathComponent("\(name).gif") as CFURL,
                                                         UTType.gif.identifier as CFString, frames.count, nil)
        else { throw CocoaError(.fileWriteUnknown) }
        CGImageDestinationSetProperties(dest, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)
        for (image, delay) in frames {
            CGImageDestinationAddImage(dest, image, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: delay]] as CFDictionary)
        }
        guard CGImageDestinationFinalize(dest) else { throw CocoaError(.fileWriteUnknown) }
    }

    func still<V: View>(_ name: String, _ view: V, size: CGSize? = nil) throws {
        let s = size ?? self.size
        let rep = snap.render(view, size: s)
        try rep.representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("\(name).png"))
    }
}
