// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// CuaTestHarness.WKWebView — macOS analogue of the Windows WebView2 harness.
//
// A minimal AppKit NSWindow hosting a WKWebView (Apple WebKit, NOT Chromium)
// that loads the SAME shared web DOM as the WebView2 / Electron harnesses
// (tests/fixtures/shared/web/index.html). The build script copies that shared
// file into the app bundle's Resources/web/ so the harness is self-contained
// and reuses the canonical web content rather than duplicating it in source.
//
// Window title is "CuaTestHarness WKWebView" (substring-matched by the
// cua-driver harness-discovery tooling). The content size is pinned and
// isRestorable=false so the window geometry does not drift between launches
// (mirrors the fixed-size fix applied to the other macOS harnesses), which
// keeps pixel-coordinate calibration stable for the modality recorder.

import AppKit
import WebKit

let kWindowTitle = "CuaTestHarness WKWebView"
let kContentSize = NSSize(width: 700, height: 820)

final class HarnessAppDelegate: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    var window: NSWindow!
    var webView: WKWebView!

    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.mainMenu = makeMainMenu()
        let style: NSWindow.StyleMask = [.titled, .closable, .miniaturizable]
        window = NSWindow(
            contentRect: NSRect(origin: NSPoint(x: 100, y: 100), size: kContentSize),
            styleMask: style, backing: .buffered, defer: false)
        window.title = kWindowTitle
        window.isRestorable = false
        // Pin the content size so the window cannot resize / drift.
        window.contentMinSize = kContentSize
        window.contentMaxSize = kContentSize
        window.setContentSize(kContentSize)

        let config = WKWebViewConfiguration()
        let journalURL = ProcessInfo.processInfo.environment["CUA_E2E_FIXTURE_JOURNAL_URL"] ?? ""
        if let encoded = try? JSONEncoder().encode(journalURL),
           let literal = String(data: encoded, encoding: .utf8) {
            let source = "window.__CUA_E2E_FIXTURE_JOURNAL_URL = \(literal);"
            config.userContentController.addUserScript(WKUserScript(
                source: source,
                injectionTime: .atDocumentStart,
                forMainFrameOnly: false))
        }
        webView = WKWebView(frame: NSRect(origin: .zero, size: kContentSize), configuration: config)
        webView.autoresizingMask = [.width, .height]
        webView.navigationDelegate = self
        window.contentView = webView

        // Load the shared web harness copied into the bundle at build time.
        if let url = Bundle.main.url(forResource: "index", withExtension: "html", subdirectory: "web") {
            webView.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
        } else {
            webView.loadHTMLString(
                "<h2>FATAL: bundled web/index.html missing</h2>", baseURL: nil)
        }

        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { true }

    // The standard Edit menu a WebKit app ships. WKWebView pastes only from
    // the `paste:` action, so Cmd+V reaches the page through this menu.
    private func makeMainMenu() -> NSMenu {
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        mainMenu.addItem(appItem)

        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        mainMenu.addItem(editItem)
        return mainMenu
    }
}

@main
struct CuaWKWebViewHarness {
    // Retained for the lifetime of the process (NSApplication.delegate is weak).
    static let delegate = HarnessAppDelegate()

    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        app.delegate = delegate
        app.run()
    }
}
