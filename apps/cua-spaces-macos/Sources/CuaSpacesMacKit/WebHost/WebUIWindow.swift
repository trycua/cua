// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
import SwiftUI
import WebKit

/// New UI (preview): the shared React UI (`apps/cua-spaces-web`) in a window
/// of its own, behind the `web_ui` experiment. The notch, the menu bar extra,
/// Touch ID and video stay native; a Space's desktop opens in its native
/// window.
///
/// - Release: the bundled build (`Contents/Resources/WebUI`) at
///   `cua-spaces://app/` through `WebUISchemeHandler`.
/// - Debug with `CUA_WEBUI_DEV=1`: the web dev server, `http://localhost:5174`.
///
/// The window has no title bar of its own (the page draws to the top edge
/// beside the traffic lights; `--titlebar-left-inset` says how far), takes
/// the theme's background so nothing flashes white, and shows only once the
/// first page has loaded.
@MainActor
public final class WebUIWindowController: NSWindowController, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    /// The open window, if any (one per app).
    public private(set) static var shared: WebUIWindowController?
    /// What the scenes let the bridge do (open a Space's window, the main
    /// window, Settings); set by whichever SwiftUI view saw them last.
    public static var actions: WebUIActions? {
        didSet { shared?.bridge.actions = actions }
    }

    /// The traffic lights' width plus a gap, for `--titlebar-left-inset`.
    static let fallbackLeftInset: CGFloat = 78
    static let titlebarHeight: CGFloat = 38
    /// Where the window's frame is saved (the defaults' `NSWindow Frame <name>`).
    static let frameName = "CuaSpacesWebUI"
    static let devServer = URL(string: "http://localhost:5174/")!

    /// The dev server, when a debug build runs with `CUA_WEBUI_DEV=1`. A
    /// release build always loads its own bundle.
    static var devURL: URL? {
        guard DevHooks.enabled, ProcessInfo.processInfo.environment["CUA_WEBUI_DEV"] == "1" else { return nil }
        return devServer
    }

    /// Opens the window, or brings it forward.
    public static func show(model: AppModel) {
        if let c = shared {
            c.bridge.actions = actions ?? c.bridge.actions
            c.present()
            return
        }
        let c = WebUIWindowController(model: model)
        shared = c
        c.load()
    }

    /// Closes the New UI window, if open (its experiment was turned off),
    /// and shows the native main window in its place.
    public static func closeShared() {
        guard let c = shared else { return }
        let main = c.bridge.actions?.openMain
        // After the bridge answered the page's switch.
        DispatchQueue.main.async {
            c.window?.close()
            main?()
        }
    }

    /// New Space in the New UI window: opens it (or brings it forward) and
    /// asks the page for its wizard ("Run on" set to `on` when given).
    @discardableResult
    public static func showNewSpace(model: AppModel, on: String?) -> Bool {
        show(model: model)
        guard let c = shared else { return false }
        c.bridge.requestNewSpace(on: on)
        return true
    }

    let bridge: WebUIBridge
    let webView: WebUIWebView
    /// Native video under `<StreamSurface>`; nil unless its experiment is on.
    let videoSurfaces: WebUIVideoSurfaces?
    let startURL: URL
    /// Sees every event this window sends the page (tests).
    var onEmit: ((_ event: String, _ payload: Any?) -> Void)?
    private var shown = false
    private var loadedOnce = false

    init(model: AppModel, route: String? = nil) {
        let dev = Self.devURL
        let base = dev ?? WebUISchemeHandler.startURL
        startURL = route.map { base.appendingPathComponent($0) } ?? base
        var origins: Set<String> = ["\(WebUISchemeHandler.scheme)://\(WebUISchemeHandler.host)"]
        if let dev, let host = dev.host {
            origins.insert("\(dev.scheme ?? "http")://\(host)\(dev.port.map { ":\($0)" } ?? "")")
        }
        bridge = WebUIBridge(model: model, allowedOrigins: origins)
        bridge.actions = Self.actions

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(WebUISchemeHandler(), forURLScheme: WebUISchemeHandler.scheme)
        config.websiteDataStore = .default()
        config.preferences.isElementFullscreenEnabled = true
        let content = config.userContentController
        content.addScriptMessageHandler(bridge, contentWorld: .page, name: WebUIBridge.name)
        content.addUserScript(WKUserScript(source: Self.hostScript(leftInset: Self.fallbackLeftInset),
                                           injectionTime: .atDocumentStart, forMainFrameOnly: true))
        webView = WebUIWebView(frame: NSRect(x: 0, y: 0, width: 1200, height: 780), configuration: config)
        webView.allowsBackForwardNavigationGestures = false
        webView.allowsMagnification = false
        Self.showWindowBackgroundUntilPainted(webView)
        #if DEBUG
        webView.isInspectable = true
        #endif
        if let surfaces = WebUIVideoSurfaces.make(webView: webView, model: model) {
            content.add(surfaces, contentWorld: .page, name: WebUIVideoSurfaces.name)
            videoSurfaces = surfaces
        } else {
            videoSurfaces = nil
        }

        let window = WebUIAppWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 780),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: true)
        window.title = "Cua Spaces"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 720, height: 480)
        window.tabbingMode = .disallowed
        window.collectionBehavior.insert(.fullScreenPrimary)
        window.contentView = webView
        // Where it was last (size and place), else centred.
        if !window.setFrameUsingName(Self.frameName) { window.center() }
        window.setFrameAutosaveName(Self.frameName)
        super.init(window: window)
        window.delegate = self
        webView.navigationDelegate = self
        webView.uiDelegate = self
        bridge.host = self
        videoSurfaces?.emit = { [weak self] event, payload in self?.emit(event, payload: payload) }
        setBackground(Self.storedBackground(for: window.effectiveAppearance), keep: false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

    // MARK: - Loading and showing

    func load() {
        bridge.pageListening = false
        webView.load(URLRequest(url: startURL))
        // A page that never finishes (a hung dev server) still gets a window.
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in self?.present() }
    }

    func present() {
        guard let window else { return }
        if !shown {
            shown = true
            applyInset()
        }
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
    }

    public func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        if !loadedOnce {
            loadedOnce = true
            bridge.startEvents()
            if let dir = Self.captureDirectory { Task { await captureRoutes(into: dir) } }
        }
        applyInset()
        present()
    }

    public func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
                        withError error: Error) {
        showLoadError(error)
    }

    public func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        showLoadError(error)
    }

    public func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        // A new page: the old one's video slots are gone with it.
        videoSurfaces?.removeAll()
    }

    public func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        videoSurfaces?.removeAll()
        bridge.pageListening = false
        webView.load(URLRequest(url: startURL))
    }

    private func showLoadError(_ error: Error) {
        let ns = error as NSError
        guard !(ns.domain == NSURLErrorDomain && ns.code == NSURLErrorCancelled) else { return }
        let where_ = startURL.absoluteString
        let hint = startURL.scheme == "http"
            ? "Start the web dev server (<code>apps/cua-spaces-web</code>, port 5174), then reopen this window."
            : "This build's web UI did not load."
        let html = """
        <!doctype html><meta charset="utf-8"><style>:root{color-scheme:light dark;font:13px -apple-system,system-ui}
        body{margin:0;height:100vh;display:grid;place-items:center}main{max-width:460px;padding:24px;line-height:1.5}
        code{font:12px ui-monospace,monospace}</style><main><h3>Could not load \(Self.escape(where_))</h3>
        <p>\(hint)</p><p><code>\(Self.escape(ns.localizedDescription))</code></p></main>
        """
        webView.loadHTMLString(html, baseURL: nil)
    }

    static func escape(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
            .replacingOccurrences(of: ">", with: "&gt;")
    }

    // MARK: - Navigation: the app's own pages only; links open in the browser

    public func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                        decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { return decisionHandler(.cancel) }
        if isAppURL(url) || url.scheme == "about" { return decisionHandler(.allow) }
        if action.navigationType == .linkActivated, ["http", "https", "mailto"].contains(url.scheme ?? "") {
            NSWorkspace.shared.open(url)
        }
        decisionHandler(.cancel)
    }

    public func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                        for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = action.request.url, ["http", "https", "mailto"].contains(url.scheme ?? "") {
            NSWorkspace.shared.open(url)
        }
        return nil
    }

    func isAppURL(_ url: URL) -> Bool {
        if url.scheme == WebUISchemeHandler.scheme { return url.host == WebUISchemeHandler.host }
        guard let dev = Self.devURL else { return false }
        return url.scheme == dev.scheme && url.host == dev.host && url.port == dev.port
    }

    // MARK: - Chrome: inset, background, drag regions

    /// The page's left inset: the traffic lights' right edge plus a gap (0
    /// in full screen, where they are hidden).
    var leftInset: CGFloat {
        guard let window, !window.styleMask.contains(.fullScreen) else { return 0 }
        guard let zoom = window.standardWindowButton(.zoomButton), let bar = zoom.superview else {
            return Self.fallbackLeftInset
        }
        let right = bar.convert(zoom.frame, to: nil).maxX
        return max(Self.fallbackLeftInset, (right + 20).rounded())
    }

    func applyInset() {
        let inset = Int(leftInset)
        webView.titlebarLeftInset = CGFloat(inset)
        webView.evaluateJavaScript(
            "document.documentElement.style.setProperty('--titlebar-left-inset','\(inset)px')",
            in: nil, in: .page, completionHandler: nil)
    }

    /// Sets at document start: the inset and title bar height as CSS
    /// variables, and `window.cuaHost` (what host this is).
    static func hostScript(leftInset: CGFloat) -> String {
        """
        (() => {
          const root = document.documentElement;
          root.style.setProperty('--titlebar-left-inset', '\(Int(leftInset))px');
          root.style.setProperty('--titlebar-height', '\(Int(titlebarHeight))px');
          root.dataset.host = 'macos';
          window.cuaHost = Object.freeze({ platform: 'macos', shell: 'swiftui', bridge: 'webkit',
            handler: '\(WebUIBridge.name)', titlebarLeftInset: \(Int(leftInset)), titlebarHeight: \(Int(titlebarHeight)) });
        })();
        """
    }

    /// The window's and the web view's background: the theme's, so the
    /// page never flashes white. The page reports its own
    /// (`window.setBackgroundColor`) and it is kept for the next launch.
    func setBackground(_ color: NSColor, keep: Bool = true) {
        window?.backgroundColor = color
        webView.underPageBackgroundColor = color
        // Only the page's own colour is kept (never a fallback).
        if keep, let window, let hex = color.webHex {
            UserDefaults.standard.set(hex, forKey: Self.backgroundKey(for: window.effectiveAppearance))
        }
    }

    static func backgroundKey(for appearance: NSAppearance) -> String {
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? "WebUIBackgroundDark" : "WebUIBackgroundLight"
    }

    static func storedBackground(for appearance: NSAppearance) -> NSColor {
        let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        // Older builds kept their plain white fallback here; the page never
        // reports white.
        let stored = UserDefaults.standard.string(forKey: backgroundKey(for: appearance))
            .flatMap { $0.lowercased() == "#ffffff" ? nil : NSColor(webHex: $0) }
        return stored ?? NSColor(webHex: dark ? defaultBackground.dark : defaultBackground.light)!
    }

    /// The page's own backgrounds before it reports one (`index.html` and
    /// `src/lib/theme.ts` in apps/cua-spaces-web).
    static let defaultBackground = (light: "#f7f8fa", dark: "#16181c")

    /// Until the page paints its first frame a web view draws plain white,
    /// however long that takes (a slow launch showed a white window for
    /// 20 s). Without its own background the window's, the theme's,
    /// shows instead; the page paints its own over it.
    static func showWindowBackgroundUntilPainted(_ webView: WKWebView) {
        webView.setValue(false, forKey: "drawsBackground")
    }

    func setDragRegions(_ rects: [CGRect]) {
        webView.dragRegions = rects
    }

    /// Tells the page something changed (`cua:event`).
    func emit(_ event: String, payload: Any? = nil) {
        onEmit?(event, payload)
        let detail: [String: Any] = ["event": event, "payload": payload ?? NSNull()]
        guard JSONSerialization.isValidJSONObject(detail),
              let data = try? JSONSerialization.data(withJSONObject: detail),
              let json = String(data: data, encoding: .utf8) else { return }
        webView.evaluateJavaScript("window.dispatchEvent(new CustomEvent('cua:event', { detail: \(json) }))",
                                   in: nil, in: .page, completionHandler: nil)
    }

    // MARK: - Native prompts

    /// The Keyvault's unlock prompt as a sheet on this window.
    func ask(_ prompt: KvUnlockPrompt) async -> UnlockAnswer {
        let alert = NSAlert()
        alert.messageText = prompt.title
        alert.informativeText = prompt.subject.isEmpty ? prompt.message : "\(prompt.subject)\n\n\(prompt.message)"
        alert.addButton(withTitle: prompt.allow)
        alert.addButton(withTitle: prompt.deny)
        alert.addButton(withTitle: prompt.neverAsk)
        let response: NSApplication.ModalResponse
        if let window, window.isVisible {
            response = await alert.beginSheetModal(for: window)
        } else {
            response = alert.runModal()
        }
        switch response {
        case .alertFirstButtonReturn: return .allow
        case .alertThirdButtonReturn: return .neverAskAgain
        default: return .deny
        }
    }

    /// The Keyvault's delete confirmation as a sheet on this window.
    func ask(_ confirm: KvDeleteConfirm) async -> Bool {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = confirm.title
        alert.informativeText = confirm.message
        alert.addButton(withTitle: confirm.confirm).hasDestructiveAction = true
        alert.addButton(withTitle: confirm.cancel)
        let response: NSApplication.ModalResponse
        if let window, window.isVisible {
            response = await alert.beginSheetModal(for: window)
        } else {
            response = alert.runModal()
        }
        return response == .alertFirstButtonReturn
    }

    // MARK: - Window

    public func windowDidEnterFullScreen(_ notification: Notification) { applyInset() }
    public func windowDidExitFullScreen(_ notification: Notification) { applyInset() }

    public func windowWillClose(_ notification: Notification) {
        // Opened again (⇧⌘U), it comes back where it was.
        window?.saveFrame(usingName: Self.frameName)
        videoSurfaces?.removeAll()
        webView.configuration.userContentController.removeAllScriptMessageHandlers()
        if Self.shared === self { Self.shared = nil }
    }
}

// MARK: - Settings: ⌘, goes where the user is

/// Which window is in front, as far as the Settings command goes.
enum SettingsFrontWindow: Equatable {
    /// The New UI window, or a sheet on it.
    case webUI
    /// Any other window of the app.
    case other
    /// None (the app is in the background, or only a utility panel is key).
    case none
}

/// Where the Settings command (⌘, in the app menu) goes.
enum SettingsRoute: Equatable {
    /// The New UI page's own Settings.
    case web
    /// The SwiftUI Settings scene (the classic UI).
    case native
}

extension WebUIWindowController {
    /// The New UI window's Settings when it is the key window, whether or
    /// not its web view has focus; the native scene when another window is
    /// key. With no key window (the app is in the background, a dialog of
    /// another app has focus) the frontmost window of the app decides.
    static func settingsRoute(key: SettingsFrontWindow, frontmost: SettingsFrontWindow) -> SettingsRoute {
        switch key {
        case .webUI: return .web
        case .other: return .native
        case .none: return frontmost == .webUI ? .web : .native
        }
    }

    /// `window` as the Settings command sees it: this window (or a sheet on
    /// it), another window of the app, or none when it is a utility panel
    /// that can't be the main window (the notch).
    static func settingsFront(_ window: NSWindow?, webUI: NSWindow?) -> SettingsFrontWindow {
        guard let window else { return .none }
        if let parent = window.sheetParent { return parent === webUI ? .webUI : .other }
        if window === webUI { return .webUI }
        return window.canBecomeMain ? .other : .none
    }

    /// The decision over real windows: `key` is the key window (nil: none),
    /// `frontmost` the frontmost one (`frontmostWindow`).
    static func settingsRoute(webUI: NSWindow?, key: NSWindow?, frontmost: NSWindow?) -> SettingsRoute {
        guard let webUI else { return .native }
        return settingsRoute(key: settingsFront(key, webUI: webUI), frontmost: settingsFront(frontmost, webUI: webUI))
    }

    /// The frontmost visible window that can be the main one, from the app's
    /// windows ordered front to back.
    static func frontmostWindow(in ordered: [NSWindow], webUI: NSWindow?, visible: (NSWindow) -> Bool = { $0.isVisible }) -> NSWindow? {
        ordered.first { visible($0) && ($0 === webUI || $0.canBecomeMain) }
    }

    /// The Settings command: the page's Settings when the New UI window is
    /// in front, else `native` (the classic UI's Settings scene).
    static func openSettings(native: () -> Void) {
        guard let c = shared, let window = c.window, window.isVisible,
              settingsRoute(webUI: window, key: NSApp.keyWindow,
                            frontmost: frontmostWindow(in: NSApp.orderedWindows, webUI: window)) == .web
        else { return native() }
        c.bridge.requestSettings()
    }
}

/// The New UI window. The app menu's "Settings…" (⌘,) is the `Settings`
/// scene's own item, which sends `showSettingsWindow:` down the responder
/// chain; while this window is key it answers first and opens the page's
/// Settings, else the native scene gets it. One menu item, no
/// `CommandGroup(replacing: .appSettings)` (that left the scene's item next
/// to its own: two "Settings…").
final class WebUIAppWindow: NSWindow {
    static let settingsAction = Selector(("showSettingsWindow:"))

    @objc func showSettingsWindow(_ sender: Any?) {
        WebUIWindowController.openSettings(native: {
            _ = NSApp.tryToPerform(Self.settingsAction, with: sender)
        })
    }
}

/// The web view, with the window's drag regions: the page names them
/// (`window.setDragRegions`, CSS pixels from the top left); until it does,
/// the top strip beside the traffic lights drags the window.
final class WebUIWebView: WKWebView {
    var dragRegions: [CGRect]?
    var titlebarLeftInset: CGFloat = WebUIWindowController.fallbackLeftInset

    override func mouseDown(with event: NSEvent) {
        guard let window, isDragPoint(event) else { return super.mouseDown(with: event) }
        if event.clickCount == 2 {
            switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
            case "Minimize": window.miniaturize(nil)
            case "None": break
            default: window.zoom(nil)
            }
        } else {
            window.performDrag(with: event)
        }
    }

    func isDragPoint(_ event: NSEvent) -> Bool {
        let p = convert(event.locationInWindow, from: nil)
        let top = CGPoint(x: p.x, y: isFlipped ? p.y : bounds.height - p.y)
        let regions = dragRegions ?? [CGRect(x: titlebarLeftInset, y: 0, width: bounds.width - titlebarLeftInset,
                                             height: WebUIWindowController.titlebarHeight)]
        let zoom = pageZoom * magnification
        return regions.contains { $0.applying(CGAffineTransform(scaleX: zoom, y: zoom)).contains(top) }
    }
}

extension NSColor {
    /// `#rrggbb` in sRGB.
    var webHex: String? {
        guard let c = usingColorSpace(.sRGB) else { return nil }
        let v = [c.redComponent, c.greenComponent, c.blueComponent].map { Int(($0 * 255).rounded()) }
        return String(format: "#%02x%02x%02x", v[0], v[1], v[2])
    }
}

/// "Open New UI (preview)": the menu command and the main window's toolbar
/// button, while the experiment is on. It also hands the bridge the scenes'
/// window actions.
public struct OpenWebUIButton: View {
    let model: AppModel
    var label: String = "Open New UI (preview)"
    @Environment(\.openWindow) private var openWindow

    public init(model: AppModel, label: String = "Open New UI (preview)") {
        self.model = model
        self.label = label
    }

    public var body: some View {
        Button(label) {
            WebUIWindowController.actions = actions
            WebUIWindowController.show(model: model)
        }
    }

    var actions: WebUIActions {
        WebUIActions.make(openWindow: openWindow)
    }
}

extension WebUIActions {
    /// The scenes' window actions for the bridge.
    static func make(openWindow: OpenWindowAction) -> WebUIActions {
        WebUIActions(
            openSpace: { id in NSApp.activate(); openWindow(id: "space", value: id) },
            openMain: { NSApp.activate(); openWindow(id: "main") })
    }
}
