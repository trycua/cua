// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import UniformTypeIdentifiers
import WebKit

/// Serves the bundled web UI (`Contents/Resources/WebUI`) at
/// `cua-spaces://app/`. A path without a file behind it and without an
/// extension is a client-side route and gets `index.html`. Nothing outside
/// the WebUI directory is ever read.
final class WebUISchemeHandler: NSObject, WKURLSchemeHandler {
    static let scheme = "cua-spaces"
    static let host = "app"
    static let startURL = URL(string: "\(scheme)://\(host)/")!

    /// The web build's directory, or nil when this build carries none (the
    /// placeholder page says how to add it).
    let root: URL?

    init(root: URL? = WebUISchemeHandler.bundledRoot()) {
        self.root = root?.standardizedFileURL
    }

    /// Where the web build is: the app's `Contents/Resources/WebUI`
    /// (`scripts/build-app.sh`), the module's resource bundle, and in a
    /// debug build the web app's own `dist` in this checkout.
    static func bundledRoot() -> URL? {
        var candidates: [URL] = []
        if let r = Bundle.main.resourceURL { candidates.append(r.appendingPathComponent("WebUI")) }
        if let r = ModuleResources.bundle?.resourceURL { candidates.append(r.appendingPathComponent("WebUI")) }
        #if DEBUG
        // Sources/CuaSpacesMacKit/WebHost/<file> -> apps/cua-spaces-web/dist
        let apps = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        candidates.append(apps.appendingPathComponent("cua-spaces-web/dist"))
        #endif
        return candidates.first { FileManager.default.fileExists(atPath: $0.appendingPathComponent("index.html").path) }
    }

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url, url.scheme == Self.scheme, url.host == Self.host else {
            return fail(task, URLError(.unsupportedURL))
        }
        guard let root else {
            let isPage = url.path.isEmpty || url.path == "/" || url.pathExtension.isEmpty
            return isPage ? respond(task, url: url, data: Data(Self.placeholder.utf8), mime: "text/html", status: 200)
                : respond(task, url: url, data: Data(), mime: "text/plain", status: 404)
        }
        guard let file = resolve(url.path, in: root) else {
            return respond(task, url: url, data: Data(), mime: "text/plain", status: 404)
        }
        do {
            let data = try Data(contentsOf: file)
            respond(task, url: url, data: data, mime: Self.mime(file.pathExtension), status: 200)
        } catch {
            fail(task, error)
        }
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}

    /// The file for a request path: inside `root` only; extensionless
    /// misses fall back to `index.html` (the app's own routing).
    func resolve(_ path: String, in root: URL) -> URL? {
        let relative = path.removingPercentEncoding ?? path
        let trimmed = relative.drop { $0 == "/" }
        let candidate = trimmed.isEmpty ? root.appendingPathComponent("index.html")
            : root.appendingPathComponent(String(trimmed)).standardizedFileURL
        guard candidate.path == root.path || candidate.path.hasPrefix(root.path + "/") else { return nil }
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: candidate.path, isDirectory: &isDir) {
            return isDir.boolValue ? root.appendingPathComponent("index.html") : candidate
        }
        return candidate.pathExtension.isEmpty ? root.appendingPathComponent("index.html") : nil
    }

    static func mime(_ ext: String) -> String {
        switch ext.lowercased() {
        case "html", "htm": return "text/html"
        case "js", "mjs": return "text/javascript"
        case "css": return "text/css"
        case "json", "map": return "application/json"
        case "svg": return "image/svg+xml"
        case "wasm": return "application/wasm"
        case "woff2": return "font/woff2"
        default:
            return UTType(filenameExtension: ext)?.preferredMIMEType ?? "application/octet-stream"
        }
    }

    private func respond(_ task: WKURLSchemeTask, url: URL, data: Data, mime: String, status: Int) {
        var headers = ["Content-Type": mime, "Content-Length": "\(data.count)", "Cache-Control": "no-cache"]
        if mime.hasPrefix("text/") { headers["Content-Type"] = "\(mime); charset=utf-8" }
        let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)!
        task.didReceive(response)
        task.didReceive(data)
        task.didFinish()
    }

    private func fail(_ task: WKURLSchemeTask, _ error: Error) {
        task.didFailWithError(error)
    }

    /// Shown when the app was built without the web UI.
    static let placeholder = """
    <!doctype html>
    <html><head><meta charset="utf-8"><title>Cua Spaces</title>
    <style>
    :root { color-scheme: light dark; font: 13px -apple-system, system-ui, sans-serif; }
    body { margin: 0; height: 100vh; display: grid; place-items: center; }
    main { max-width: 460px; padding: 24px; line-height: 1.5; }
    h1 { font-size: 17px; margin: 0 0 8px; }
    code { font: 12px ui-monospace, monospace; }
    </style></head>
    <body><main>
    <h1>The new UI is not in this build</h1>
    <p>Build <code>apps/cua-spaces-web</code> and run <code>scripts/build-app.sh</code> again,
    or start the web dev server and launch a debug build with <code>CUA_WEBUI_DEV=1</code>.</p>
    </main></body></html>
    """
}
