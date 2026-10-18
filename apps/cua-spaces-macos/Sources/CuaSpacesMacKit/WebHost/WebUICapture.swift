// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import WebKit

/// `CUA_SPACES_WEBUI_CAPTURE=<dir>` (debug hooks only): after the first load, opens
/// every route in light and dark, checks which bridge the page is on
/// (`<html data-bridge>`), saves the web view's own snapshot of each to
/// `<dir>/<route>-<theme>.png` with a `report.json`, then quits. Window
/// captures from outside the app miss WKWebView's content (it draws in
/// another process), so the screenshots in docs/webui come from here.
extension WebUIWindowController {
    static let captureRoutes = ["spaces", "machines", "agents", "keyvault", "notifications", "settings",
                                 "settings/about", "settings/devices"]

    static var captureDirectory: URL? {
        guard DevHooks.enabled, let dir = DevHooks.environment["CUA_SPACES_WEBUI_CAPTURE"], !dir.isEmpty else { return nil }
        return URL(fileURLWithPath: dir, isDirectory: true)
    }

    func captureRoutes(into dir: URL) async {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var report: [[String: Any]] = []
        for theme in ["light", "dark"] {
            window?.appearance = NSAppearance(named: theme == "dark" ? .darkAqua : .aqua)
            _ = try? await webView.evaluateJavaScript("localStorage.setItem('cua-spaces:theme', '\(theme)')")
            for route in Self.captureRoutes {
                var page: [String: Any] = [:]
                // The first navigation can interrupt the initial load: try again.
                for _ in 0..<3 where !(page["bridge"] is String) {
                    webView.load(URLRequest(url: startURL.appendingPathComponent(route)))
                    page = await settledPage()
                }
                let file = dir.appendingPathComponent("\(route.replacingOccurrences(of: "/", with: "-"))-\(theme).png")
                if let image = try? await webView.takeSnapshot(configuration: nil),
                   let tiff = image.tiffRepresentation,
                   let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
                    try? png.write(to: file)
                }
                report.append(["route": route, "theme": theme, "file": file.lastPathComponent].merging(page) { a, _ in a })
            }
        }
        // What the host answered, next to what the page drew.
        let spaces = (try? await bridge.handle("spaces.list", [:])) ?? NSNull()
        // How the routes beyond Spaces answered: ok (with a count), or the error code.
        var answers: [String: Any] = [:]
        for method in ["agents.list", "agents.setup", "host.status", "machines.list", "settings.get", "about.get",
                       "loginItem.get", "devices.get", "notifications.list", "volume.overview", "storage.get"] {
            do {
                let result = try await bridge.handle(method, [:])
                switch method {
                case "machines.list": answers[method] = ["ok": true, "host": (result as? [String: Any])?["host"] is [String: Any]]
                case "settings.get": answers[method] = ["ok": true, "updateChannel": (result as? [String: Any])?["updateChannel"] ?? NSNull()]
                default: answers[method] = ["ok": true, "count": (result as? [Any])?.count ?? (result is NSNull ? 0 : 1)]
                }
            } catch let f as WebUIBridge.Failure {
                answers[method] = ["ok": false, "code": f.code]
            } catch {
                answers[method] = ["ok": false, "code": "failed"]
            }
        }
        let output: [String: Any] = ["pages": report, "spaces.list": spaces, "answers": answers]
        if let data = try? JSONSerialization.data(withJSONObject: output, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: dir.appendingPathComponent("report.json"))
        }
        NSApp.terminate(nil)
    }

    /// Waits until the page names its bridge and its route has content.
    private func settledPage() async -> [String: Any] {
        let probe = """
        JSON.stringify({ bridge: document.documentElement.dataset.bridge || null,
          dark: document.documentElement.classList.contains('dark'),
          heading: document.querySelector('main h1')?.textContent || null,
          text: (document.querySelector('main')?.innerText || '').trim().length })
        """
        var last: [String: Any] = [:]
        for _ in 0..<60 {
            try? await Task.sleep(for: .milliseconds(150))
            guard let json = try? await webView.evaluateJavaScript(probe) as? String,
                  let page = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else { continue }
            last = page
            if page["bridge"] is String, (page["text"] as? Int ?? 0) > 0 { break }
        }
        try? await Task.sleep(for: .milliseconds(700))
        return last
    }
}
