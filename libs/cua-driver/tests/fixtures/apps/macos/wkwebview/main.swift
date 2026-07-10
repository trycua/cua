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
