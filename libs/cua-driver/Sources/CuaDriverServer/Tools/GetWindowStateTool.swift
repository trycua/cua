import CuaDriverCore
import Foundation
import MCP

public enum GetWindowStateTool {
    // Shared actor — screenshots serialize through it, matching the rest
    // of the server's capture paths.
    private static let capture = WindowCapture()

    public static let handler = ToolHandler(
        tool: Tool(
            name: "get_window_state",
            description: """
                Walk a running app's AX tree and return a Markdown rendering of its
                UI, tagging every actionable element with [element_index N]. Pass
                those indices to `click`, `type_text_in`, `scroll_in`, etc. — those
                tools resolve the index to the cached AXUIElement on the server.

                INVARIANT: call `get_window_state` once per turn per (pid, window_id)
                before any element-indexed action against that window. The index
                map is replaced by the next snapshot of the same (pid, window_id).

                The AX tree walked is the pid's tree, but the screenshot and window
                bounds reported come from the specified `window_id`. This is the
                source of truth for which window the caller intends to reason about
                — the driver never picks a window implicitly. Use `list_windows` to
                enumerate candidates, or read `launch_app`'s `windows` field for
                freshly-launched apps.

                `window_id` MUST belong to `pid` and MUST be on the user's current
                Space; the call returns `isError: true` otherwise. The driver does
                not auto-fall-back to a different window — window selection is the
                caller's responsibility.

                The screenshot of the specified window is delivered as a native MCP
                image content block (not a JSON text field), so clients receive it
                as a proper image rather than raw base64 text.

                Set `query` to a case-insensitive substring to filter
                `tree_markdown` down to matching lines plus their ancestor
                chain (so the structural context above each match stays
                visible). The element_index values and `element_count` are
                unchanged — the filter only trims the rendered Markdown, so
                `click({pid, window_id, element_index: N})` still resolves
                against the full cached tree. When nothing matches,
                `tree_markdown` is an empty string; the caller can retry
                without the filter.

                Response shape is controlled by the persistent
                `capture_mode` config setting (default `som`). Each
                mode skips the half of the work it doesn't need — not
                just the half of the response. Note that `vision` names
                the capture MODE; the separate `screenshot` tool (which
                captures a raw PNG without walking AX) is unrelated and
                keeps its name:
                  - `som`    — walks AX tree AND captures screenshot
                               (default). Element-indexed clicks work
                               out of the box; screenshot is there for
                               disambiguation.
                  - `vision` — captures the window PNG; AX tree walk
                               is skipped entirely (no Accessibility
                               hit, no element_index cache update).
                               `tree_markdown`, `element_count`, and
                               `turn_id` omitted. Element-indexed
                               clicks will fail until a non-`vision`
                               snapshot runs — pair with pixel-addressed
                               `click({pid, x, y})`. Previously named
                               `screenshot`; the raw string
                               `"screenshot"` still decodes to `vision`
                               as a deprecated alias.
                  - `ax`     — walks AX tree; screen-capture call is
                               skipped entirely (no Screen Recording
                               hit). `screenshot_*` fields omitted.
                Change with `cua-driver config set capture_mode <mode>` or
                the `set_config` tool.

                Requires Accessibility and Screen Recording permissions.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "window_id"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Process ID from `list_apps`.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID of the target window. Must belong to `pid` and be on the user's current Space. Enumerate via `list_windows` or read from `launch_app`'s `windows` array.",
                    ],
                    "query": [
                        "type": "string",
                        "description":
                            "Optional case-insensitive substring. When set, `tree_markdown` only contains lines that match plus their ancestor chain; element indices and `element_count` are unchanged.",
                    ],
                    "javascript": [
                        "type": "string",
                        "description": """
                            Optional JavaScript to execute in the browser tab and return \
                            alongside the AX snapshot — one round-trip instead of two. \
                            Only works for Chromium-family browsers (Chrome, Brave, Edge) \
                            and Safari; requires 'Allow JavaScript from Apple Events' to be \
                            enabled first (see WEB_APPS.md). The result is appended to the \
                            response as a `## JavaScript result` section. Use for read-only \
                            queries (document.title, innerText, querySelectorAll, etc.). \
                            For mutations or side effects use the `page` tool instead.
                            """,
                    ],
                    "screenshot_out_file": [
                        "type": "string",
                        "description": """
                            Optional absolute path to write the screenshot to (e.g. \
                            "/tmp/shot.jpg"). When set, the screenshot bytes are written \
                            to this file and the MCP image content block is omitted from \
                            the response — `screenshot_file_path` is returned instead of \
                            `screenshot_png_b64`. Useful for CLI callers and agents that \
                            cannot consume inline base64 without saturating their context \
                            window (e.g. OpenCode with a local Ollama model). The directory \
                            must already exist; the file is created or overwritten.
                            """,
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: false,  // new turn_id each call
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue else {
                return errorResult("Missing required integer field pid.")
            }
            guard let rawWindowId = arguments?["window_id"]?.intValue else {
                return errorResult(
                    "Missing required integer field window_id. Use `list_windows` "
                    + "to enumerate the target app's windows, or read `launch_app`'s "
                    + "`windows` array.")
            }
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }
            guard let windowId = UInt32(exactly: rawWindowId) else {
                return errorResult(
                    "window_id \(rawWindowId) is outside the supported UInt32 range.")
            }
            let query = arguments?["query"]?.stringValue
            let javascript = arguments?["javascript"]?.stringValue
            let screenshotOutFile = arguments?["screenshot_out_file"]?.stringValue

            // Validate that the window belongs to this pid. The driver
            // never guesses which window to snapshot — the caller names
            // it explicitly — so a mismatched pid/window is a hard error.
            // Off-Space windows are NOT rejected: the snapshot still
            // carries `off_space: true` and the caller decides whether
            // to proceed (useful for reading menu state on a window the
            // user has parked elsewhere).
            let allWindows = WindowEnumerator.allWindows()
            guard let window = allWindows.first(where: {
                UInt32($0.id) == windowId
            }) else {
                return errorResult(
                    "No window with window_id \(windowId) exists. "
                    + "Call `list_windows({pid: \(rawPid)})` for candidates.")
            }
            if window.pid != pid {
                return errorResult(
                    "window_id \(windowId) belongs to pid \(window.pid), not pid "
                    + "\(rawPid). Call `list_windows({pid: \(rawPid)})` to get this "
                    + "pid's own windows.")
            }

            // Re-read the persisted capture_mode on every invocation so a
            // `cua-driver config set capture_mode …` in-flight takes effect
            // without a daemon bounce — the config store's load path is
            // cheap (single JSON decode of a tiny file, or fall-through to
            // the cached default when the file is absent).
            let config = await ConfigStore.shared.load()
            let captureMode = config.captureMode
            let maxImageDim = config.maxImageDimension

            do {
                // `.vision` skips the AX walk entirely — no tree
                // computation, no element_index cache update. The whole
                // point of that mode is to cut CPU and latency for
                // vision-only / pixel-click workflows that never consult
                // the AX tree. `.ax` and `.som` still walk normally.
                var snapshot: AppStateSnapshot
                if captureMode == .vision {
                    snapshot = try await AppStateRegistry.engine.metadataOnly(pid: pid)
                } else {
                    snapshot = try await AppStateRegistry.engine.snapshot(
                        pid: pid, windowId: windowId)
                    if let query, !query.isEmpty {
                        snapshot = snapshot.withFilteredTree(
                            treeMarkdown: filterTreeMarkdown(
                                snapshot.treeMarkdown, query: query
                            )
                        )
                    }
                }

                // `.ax` skips the screen-capture call entirely — the whole
                // point of that mode is to cut token cost AND CPU for
                // element_index-only workflows. Other modes capture the
                // specific window the caller named. A `windowNotFound`
                // race (window closed between validation and capture)
                // leaves the snapshot without a screenshot; the structured
                // response's `has_screenshot=false` surfaces the omission.
                if captureMode != .ax {
                    do {
                        let shot = try await capture.captureWindow(
                            windowID: windowId,
                            format: .jpeg,
                            quality: 85,
                            maxImageDimension: maxImageDim
                        )
                        snapshot = snapshot.withScreenshot(
                            pngBase64: shot.imageData.base64EncodedString(),
                            width: shot.width,
                            height: shot.height,
                            scaleFactor: shot.scaleFactor,
                            originalWidth: shot.originalWidth,
                            originalHeight: shot.originalHeight
                        )
                        // Record resize ratio so ClickTool can scale back up.
                        if let origW = shot.originalWidth, shot.width > 0 {
                            await ImageResizeRegistry.shared.setRatio(
                                Double(origW) / Double(shot.width), forPid: pid)
                        } else {
                            await ImageResizeRegistry.shared.clearRatio(forPid: pid)
                        }
                    } catch CaptureError.windowNotFound {
                        // Window raced — swallow and emit a screenshot-less
                        // response.
                    }
                }

                var textContent = buildSummary(
                    snapshot: snapshot, pid: pid, mode: captureMode
                )
                if captureMode != .vision && !snapshot.treeMarkdown.isEmpty {
                    textContent += "\n\n" + snapshot.treeMarkdown
                }
                // If the caller passed a javascript snippet, run it in the
                // browser tab and append the result — one round-trip instead
                // of two separate tool calls.
                if let js = javascript {
                    let bundleId = snapshot.bundleId ?? ""
                    do {
                        let jsResult: String
                        if BrowserJS.supports(bundleId: bundleId) {
                            jsResult = try await BrowserJS.execute(
                                javascript: js, bundleId: bundleId, windowId: windowId)
                        } else if ElectronJS.isElectron(pid: pid) {
                            jsResult = try await ElectronJS.execute(
                                javascript: js, pid: pid)
                        } else {
                            throw BrowserJS.Error.unsupportedBrowser(bundleId)
                        }
                        textContent += "\n\n## JavaScript result\n\n```\n\(jsResult)\n```"
                    } catch {
                        textContent += "\n\n## JavaScript result\n\n❌ \(error)"
                    }
                }

                // When the caller supplied screenshot_out_file, write the bytes
                // to disk and omit the MCP image content block entirely — the
                // path is surfaced via structuredContent.screenshot_file_path
                // so the caller knows where to find the image without paying
                // the base64-in-context token cost.
                var resolvedScreenshotFilePath: String? = nil
                if let outPath = screenshotOutFile, let b64 = snapshot.screenshotPngBase64 {
                    let expandedPath = (outPath as NSString).expandingTildeInPath
                    if let bytes = Data(base64Encoded: b64) {
                        let url = URL(fileURLWithPath: expandedPath)
                        do {
                            try bytes.write(to: url)
                            resolvedScreenshotFilePath = expandedPath
                        } catch {
                            // Write failed — fall through so the inline image
                            // content block is still emitted rather than silently
                            // dropping the screenshot entirely.
                        }
                    }
                }

                var content: [Tool.Content] = []
                if resolvedScreenshotFilePath == nil, let b64 = snapshot.screenshotPngBase64 {
                    content.append(
                        .image(data: b64, mimeType: "image/jpeg", annotations: nil, _meta: nil)
                    )
                }
                content.append(.text(text: textContent, annotations: nil, _meta: nil))
                // Strip the b64 bytes from the structured snapshot — the image
                // is already the first content block (or on disk) and tests
                // just need the metadata fields.
                let structuredSnapshot = AppStateSnapshot(
                    pid: snapshot.pid,
                    bundleId: snapshot.bundleId,
                    name: snapshot.name,
                    treeMarkdown: snapshot.treeMarkdown,
                    elementCount: snapshot.elementCount,
                    turnId: snapshot.turnId,
                    screenshotPngBase64: nil,
                    screenshotWidth: snapshot.screenshotWidth,
                    screenshotHeight: snapshot.screenshotHeight,
                    screenshotScaleFactor: snapshot.screenshotScaleFactor,
                    screenshotOriginalWidth: snapshot.screenshotOriginalWidth,
                    screenshotOriginalHeight: snapshot.screenshotOriginalHeight,
                    screenshotFilePath: resolvedScreenshotFilePath
                )
                if let result = try? CallTool.Result(
                    content: content,
                    structuredContent: structuredSnapshot
                ) {
                    return result
                }
                return CallTool.Result(content: content)
            } catch let error as AppStateError {
                return errorResult(error.description)
            } catch let error as CaptureError {
                return errorResult("Screenshot failed: \(error.description)")
            } catch {
                return errorResult("Unexpected error: \(error)")
            }
        }
    )

    /// Mode-aware summary block. First line is always a ✅ headline with
    /// only the fields the agent needs. Warnings follow on separate lines.
    private static func buildSummary(
        snapshot: AppStateSnapshot, pid: Int32, mode: CaptureMode
    ) -> String {
        let label = snapshot.name ?? "pid \(pid)"
        var lines: [String] = []

        switch mode {
        case .ax:
            lines.append(
                "✅ \(label) — \(snapshot.elementCount) elements, turn \(snapshot.turnId)"
                    + " [ax mode — no screenshot]"
            )
            if snapshot.elementCount <= 15 {
                lines.append(
                    "⚠️  Small AX tree (\(snapshot.elementCount) elements) — this app"
                        + " likely uses custom rendering. Prefer pixel clicks:"
                        + " click(pid, x, y).")
            }

        case .vision:
            if snapshot.screenshotPngBase64 != nil {
                lines.append("✅ \(label) — screenshot captured")
                if let w = snapshot.screenshotWidth, w > 1000 {
                    lines.append(
                        "⚠️  Screenshot is \(w)px wide."
                            + " Use `zoom` to inspect regions at full resolution.")
                }
            } else {
                lines.append("❌ \(label) — window capture failed")
            }

        case .som:
            var headline =
                "✅ \(label) — \(snapshot.elementCount) elements, turn \(snapshot.turnId)"
            if snapshot.screenshotPngBase64 != nil {
                headline += " + screenshot"
            } else {
                headline += " (screenshot capture failed)"
            }
            lines.append(headline)

            if snapshot.elementCount <= 15 {
                lines.append(
                    "⚠️  Small AX tree (\(snapshot.elementCount) elements) — this app"
                        + " likely uses custom rendering (e.g. Blender, games, Electron)."
                        + " Use pixel clicks: click(pid, x, y) with coordinates from the screenshot.")
            }

            if let w = snapshot.screenshotWidth, w > 1000 {
                lines.append(
                    "⚠️  Screenshot is \(w)px wide."
                        + " Use `zoom` to inspect regions at full resolution.")
            }
        }

        return lines.joined(separator: "\n")
    }

    /// Return a version of `tree_markdown` containing only lines that
    /// case-insensitively match `query`, plus each match's ancestor-chain
    /// lines so the tree stays structurally coherent. Returns an empty
    /// string when nothing matches.
    ///
    /// The renderer (``AppStateEngine.renderTree``) prefixes each line with
    /// `"  " * depth + "- "`, so depth is `(leadingSpaces / 2)`. For every
    /// line we see we remember it at its depth (the "current ancestor" at
    /// each level); when a line matches, we emit the ancestor at every
    /// depth from 0..<match-depth followed by the match itself.
    ///
    /// Ancestors are de-duplicated across matches by tracking, per-depth,
    /// the last ancestor we actually emitted — a shared parent isn't
    /// emitted twice for two sibling matches underneath it. `nil` at a
    /// depth slot means "never emitted at this depth yet" and differs from
    /// the empty-string placeholder for "no ancestor line has reached this
    /// depth" (happens for queries that match a top-level line).
    static func filterTreeMarkdown(_ markdown: String, query: String) -> String {
        let needle = query.lowercased()
        let lines = markdown.split(
            separator: "\n", omittingEmptySubsequences: false
        ).map(String.init)
        let meaningful = (lines.last == "") ? Array(lines.dropLast()) : lines

        var currentAncestorAt: [String] = []
        var lastEmittedAt: [String?] = []
        var output: [String] = []

        for line in meaningful {
            let depth = leadingIndentDepth(line)
            // Size both arrays to accommodate this depth.
            while currentAncestorAt.count <= depth {
                currentAncestorAt.append("")
                lastEmittedAt.append(nil)
            }
            currentAncestorAt[depth] = line
            // Moving laterally or up resets "last emitted" knowledge for
            // any deeper levels — those slots now refer to a stale subtree.
            for deeper in (depth + 1)..<lastEmittedAt.count {
                lastEmittedAt[deeper] = nil
            }

            if line.lowercased().contains(needle) {
                for ancestorDepth in 0..<depth {
                    let ancestor = currentAncestorAt[ancestorDepth]
                    if ancestor.isEmpty { continue }
                    if lastEmittedAt[ancestorDepth] == ancestor { continue }
                    lastEmittedAt[ancestorDepth] = ancestor
                    output.append(ancestor)
                }
                lastEmittedAt[depth] = line
                output.append(line)
            }
        }

        if output.isEmpty { return "" }
        return output.joined(separator: "\n") + "\n"
    }

    private static func leadingIndentDepth(_ line: String) -> Int {
        var count = 0
        for char in line {
            if char == " " { count += 1 } else { break }
        }
        return count / 2
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }

    /// Format a scale factor for the summary line: integer scales ("2") stay
    /// compact, fractional scales ("1.5") keep one decimal so the LLM can
    /// spot an unusual-looking value.
    private static func formatScale(_ scale: Double) -> String {
        if scale == scale.rounded() {
            return String(Int(scale))
        }
        return String(format: "%.1f", scale)
    }

}

extension AppStateSnapshot {
    fileprivate func withScreenshot(
        pngBase64: String,
        width: Int,
        height: Int,
        scaleFactor: Double,
        originalWidth: Int? = nil,
        originalHeight: Int? = nil
    ) -> AppStateSnapshot {
        AppStateSnapshot(
            pid: pid,
            bundleId: bundleId,
            name: name,
            treeMarkdown: treeMarkdown,
            elementCount: elementCount,
            turnId: turnId,
            screenshotPngBase64: pngBase64,
            screenshotWidth: width,
            screenshotHeight: height,
            screenshotScaleFactor: scaleFactor,
            screenshotOriginalWidth: originalWidth,
            screenshotOriginalHeight: originalHeight
        )
    }

    /// Replace the rendered `tree_markdown` (e.g. with a query-filtered
    /// subset) while preserving every other field — critically
    /// `elementCount`, which the server-side cache is keyed against and
    /// which must keep reflecting the count of actionable elements in the
    /// *full* tree.
    fileprivate func withFilteredTree(treeMarkdown: String) -> AppStateSnapshot {
        AppStateSnapshot(
            pid: pid,
            bundleId: bundleId,
            name: name,
            treeMarkdown: treeMarkdown,
            elementCount: elementCount,
            turnId: turnId,
            screenshotPngBase64: screenshotPngBase64,
            screenshotWidth: screenshotWidth,
            screenshotHeight: screenshotHeight,
            screenshotScaleFactor: screenshotScaleFactor,
            screenshotOriginalWidth: screenshotOriginalWidth,
            screenshotOriginalHeight: screenshotOriginalHeight
        )
    }
}
