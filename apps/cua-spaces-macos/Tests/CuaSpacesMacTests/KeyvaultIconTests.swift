// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Icons and memoization: app icons resolved once per app and version, site
/// icons from the browser's own store first, then one capped lookup, then the
/// globe; and the list model recomputed only when the vault or search change.
@MainActor
@Suite("Keyvault icons")
struct KeyvaultIconTests {
    nonisolated static func png(_ side: Int = 8, _ shade: CGFloat = 0.3) -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor(white: shade, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: side, height: side).fill()
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])!
    }

    func tempDir() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("cua-icons-\(UUID().uuidString)")
    }

    /// Counts what a fetch asked for.
    final class Net: @unchecked Sendable {
        let lock = NSLock()
        var urls: [URL] = []
        var status = 200
        var body = KeyvaultIconTests.png()
        var inFlight = 0
        var peak = 0
        func fetch(_ url: URL) async throws -> (Data, Int) {
            lock.lock(); urls.append(url); inFlight += 1; peak = max(peak, inFlight); lock.unlock()
            try await Task.sleep(nanoseconds: 20_000_000)
            lock.lock(); inFlight -= 1; let r = (body, status); lock.unlock()
            return r
        }
    }

    // MARK: App icons

    @Test func anAppIsResolvedOnceNotPerRenderAndSurvivesARestart() throws {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        final class Count: @unchecked Sendable { var resolved = 0; var drawn = 0 }
        let count = Count()
        let make: @Sendable (String) -> AppIconCache.Resolver = { version in
            { _, _ in
                count.resolved += 1
                return AppIconSource(bundleId: "com.google.Chrome", version: version) { count.drawn += 1; return Self.png() }
            }
        }
        let cache = AppIconCache(dir: dir, resolver: make("131.0"))
        for _ in 0..<50 { #expect(cache.image(providerId: "chrome", name: "Google Chrome") != nil) }
        #expect(count.resolved == 1 && count.drawn == 1, "50 renders, one NSWorkspace lookup and one drawing")
        #expect(FileManager.default.fileExists(atPath: dir.appendingPathComponent("com.google.Chrome-131.0.png").path))
        // A restart reads the PNG from disk: the app is looked up, not drawn.
        let again = AppIconCache(dir: dir, resolver: make("131.0"))
        #expect(again.image(providerId: "chrome", name: "Google Chrome") != nil)
        #expect(count.drawn == 1)
        // An update changes the version, so the icon is drawn again.
        let updated = AppIconCache(dir: dir, resolver: make("132.0"))
        #expect(updated.image(providerId: "chrome", name: "Google Chrome") != nil)
        #expect(count.drawn == 2)
        // An app that is not installed stays a letter and is not asked again.
        let none = AppIconCache(dir: dir, resolver: { _, _ in count.resolved += 1; return nil })
        let before = count.resolved
        #expect(none.image(providerId: "arc", name: "Arc") == nil)
        #expect(none.image(providerId: "arc", name: "Arc") == nil)
        #expect(count.resolved == before + 1)
    }

    // MARK: Site icons

    @Test func onlyAPlainRegistrableDomainIsEverSent() {
        for good in ["github.com", "notion.so", "linear.app", "bbc.co.uk", "xn--bcher-kva.example.org"] {
            #expect(SiteIconStore.sendable(good), "\(good)")
        }
        for bad in ["", "localhost", "intranet", "router.local", "192.168.0.1", "10.0.0.5", "a.com/path", "a b.com",
                    "user@evil.com", "x.com?y=1", ".com", "a..com", "https://github.com"] {
            #expect(!SiteIconStore.sendable(bad), "\(bad)")
        }
        let url = SiteIconStore.url(for: "GitHub.com", scale: 2)?.absoluteString
        #expect(url == "https://www.google.com/s2/favicons?domain=github.com&sz=128")
        #expect(SiteIconStore.url(for: "github.com", scale: 1)?.absoluteString.hasSuffix("&sz=64") == true)
    }

    @Test func aDomainIsFetchedAtMostOnceAndCachedAcrossRestarts() async {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let net = Net()
        let store = SiteIconStore(dir: dir, fetch: net.fetch)
        #expect(store.cached("github.com") == nil)
        #expect(await store.load("github.com", scale: 2) != nil)
        #expect(await store.load("github.com", scale: 2) != nil)
        #expect(net.urls.count == 1)
        let restarted = SiteIconStore(dir: dir, fetch: net.fetch)
        #expect(restarted.cached("github.com") != nil, "from disk, no network")
        #expect(await restarted.load("github.com", scale: 2) != nil)
        #expect(net.urls.count == 1)
    }

    @Test func aMissIsRememberedForAWeekThenRetried() async {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let net = Net()
        net.status = 404
        final class Clock: @unchecked Sendable { var now = Date() }
        let clock = Clock()
        let store = SiteIconStore(dir: dir, now: { clock.now }, fetch: net.fetch)
        #expect(await store.load("nosuch.example.org", scale: 2) == nil)
        #expect(store.recentMiss("nosuch.example.org"))
        #expect(await store.load("nosuch.example.org", scale: 2) == nil)
        #expect(net.urls.count == 1, "a miss is not asked again")
        clock.now = clock.now.addingTimeInterval(6 * 86_400)
        #expect(await store.load("nosuch.example.org", scale: 2) == nil)
        #expect(net.urls.count == 1)
        clock.now = clock.now.addingTimeInterval(2 * 86_400)
        net.status = 200
        #expect(await store.load("nosuch.example.org", scale: 2) != nil, "retried after a week")
        #expect(net.urls.count == 2)
    }

    @Test func aFailedNetworkIsNotRememberedAsAMiss() async {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        struct Offline: Error {}
        let store = SiteIconStore(dir: dir, fetch: { _ in throw Offline() })
        #expect(await store.load("github.com", scale: 2) == nil)
        #expect(!store.recentMiss("github.com"))
    }

    @Test func nothingElseThanAPlainImageIsKept() async {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let net = Net()
        net.body = Data("<html>not an icon</html>".utf8)
        let store = SiteIconStore(dir: dir, fetch: net.fetch)
        #expect(await store.load("github.com", scale: 2) == nil)
        #expect(store.cached("github.com") == nil)
        #expect(store.recentMiss("github.com"))
    }

    @Test func fetchesRunAFewAtATime() async {
        let net = Net()
        let store = SiteIconStore(dir: tempDir(), maxConcurrent: 3, fetch: net.fetch)
        await withTaskGroup(of: Void.self) { group in
            for i in 0..<12 { group.addTask { _ = await store.load("site\(i).com", scale: 2) } }
        }
        #expect(net.urls.count == 12)
        #expect(net.peak <= 3, "peak \(net.peak)")
    }

    // MARK: The model: local first, then Google when allowed, then the globe

    func model(favicons: [KvFavicon] = [], net: Net, google: Bool = true) async -> (KeyvaultModel, FakeKeyvault) {
        let fake = FakeKeyvault(KeyvaultFixtures.overview())
        fake.favicons = favicons
        let m = KeyvaultModel(client: fake, clock: { Date(timeIntervalSince1970: Double(KeyvaultFixtures.now) / 1000) })
        m.siteIconStore = SiteIconStore(dir: tempDir(), fetch: net.fetch)
        m.siteIconsFromGoogle = { google }
        await m.refresh()
        return (m, fake)
    }

    @Test func theBrowsersOwnIconWinsAndNeedsNoNetwork() async {
        let net = Net()
        let local = KvFavicon(site: "github.com", png: Self.png(16, 0.8).base64EncodedString())
        let (m, _) = await model(favicons: [local], net: net)
        await m.loadIcons()
        #expect(m.siteIcons["github.com"] != nil)
        await m.siteIcon("github.com")
        #expect(net.urls.isEmpty)
        // Another site the browser had none for: one lookup of that domain.
        await m.siteIcon("notion.so")
        #expect(net.urls.map(\.absoluteString) == ["https://www.google.com/s2/favicons?domain=notion.so&sz=128"])
        #expect(m.siteIcons["notion.so"] != nil)
        await m.siteIcon("notion.so")
        #expect(net.urls.count == 1)
    }

    @Test func withTheSettingOffOnlyLocalIconsShowAndNothingIsSent() async {
        let net = Net()
        let local = KvFavicon(site: "github.com", png: Self.png().base64EncodedString())
        let (m, _) = await model(favicons: [local], net: net, google: false)
        await m.loadIcons()
        await m.siteIcon("github.com")
        await m.siteIcon("notion.so")
        await m.siteIcon("linear.app")
        #expect(net.urls.isEmpty)
        #expect(m.siteIcons["github.com"] != nil)
        #expect(m.siteIcons["notion.so"] == nil, "the globe stands in")
    }

    @Test func theGoogleSwitchIsASettingOnByDefault() async throws {
        let fake = FakeKeyvault(KeyvaultFixtures.overview())
        let app = ViewModelTests().makeModel(kv: fake)
        await app.keyvault.refresh()
        await app.loadSettings()
        let row = try #require(app.settingsPage.sections.first { $0.id == "keyvault" }?.rows.first { $0.id == "keyvault-site-icons" })
        #expect(row.label == "Load site icons from Google")
        #expect(row.options.first { $0.id == "on" }?.active == true)
        await app.choose(row: "keyvault-site-icons", option: "off")
        #expect(app.settings.keyvaultSiteIcons == false)
        #expect(app.keyvault.siteIconsFromGoogle() == false)
        let again = try #require(app.settingsPage.sections.first { $0.id == "keyvault" }?.rows.first { $0.id == "keyvault-site-icons" })
        #expect(again.options.first { $0.id == "off" }?.active == true)
    }

    // MARK: Memoization

    @Test func theListModelRecomputesOnlyWhenTheVaultOrSearchChange() async {
        let (m, _) = await model(net: Net())
        let k = m
        let base = k.computes["vaultView", default: 0]
        for _ in 0..<40 { _ = k.vaultView; _ = k.sidebar; _ = k.page }
        #expect(k.computes["vaultView", default: 0] <= base + 1, "40 renders, one computation")
        #expect(k.computes["sidebar", default: 0] <= 1)
        let once = k.computes["vaultView", default: 0]
        k.query = "github"
        _ = k.vaultView
        _ = k.vaultView
        #expect(k.computes["vaultView"] == once + 1, "a search recomputes once")
        k.send(KvVaultAction.toggleGroup(key: "chrome|github.com"))
        _ = k.vaultView
        #expect(k.computes["vaultView"] == once + 2)
        await k.refresh()
        _ = k.vaultView
        _ = k.vaultView
        #expect(k.computes["vaultView"] == once + 3, "a refresh recomputes once")
        #expect(k.vaultView.apps.flatMap(\.sites).allSatisfy { $0.site.contains("github") })
    }
}
