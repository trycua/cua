import AppKit
import ApplicationServices
import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

public enum SetValueTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_value",
            description: """
                Set a value on a UI element. Two modes depending on element role:

                - **AXPopUpButton / select dropdown**: finds the child option whose
                  title or value matches `value` (case-insensitive) and AXPresses it
                  directly — the native macOS popup menu is never opened, so focus
                  is never stolen. Use this for HTML <select> elements in Safari or
                  any native NSPopUpButton. Pass the option's display label as `value`
                  (e.g. "Blue", not "blue").

                - **All other elements**: writes `AXValue` directly (sliders, steppers,
                  date pickers, native text fields that expose settable AXValue).

                For free-form text entry into web inputs, prefer `type_text_chars`
                which synthesises key events — AXValue writes are ignored by WebKit.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "window_id", "element_index", "value"],
                "properties": [
                    "pid": ["type": "integer"],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID for the window whose get_window_state produced the element_index. The element_index cache is scoped per (pid, window_id).",
                    ],
                    "element_index": ["type": "integer"],
                    "value": [
                        "type": "string",
                        "description":
                            "New value. AX will coerce to the element's native type.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: true,
                idempotentHint: true,  // setting same value twice is idempotent
                openWorldHint: true
            )
        ),
        invoke: { arguments in
            guard
                let rawPid = arguments?["pid"]?.intValue,
                let rawWindowId = arguments?["window_id"]?.intValue,
                let index = arguments?["element_index"]?.intValue
            else {
                return errorResult(
                    "Missing required integer fields pid, window_id, and element_index.")
            }
            guard let value = arguments?["value"]?.stringValue else {
                return errorResult("Missing required string field value.")
            }

            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }
            guard let windowId = UInt32(exactly: rawWindowId) else {
                return errorResult(
                    "window_id \(rawWindowId) is outside the supported UInt32 range.")
            }
            do {
                let element = try await AppStateRegistry.engine.lookup(
                    pid: pid,
                    windowId: windowId,
                    elementIndex: index
                )
                let target = AXInput.describe(element)

                // Animate the agent cursor to the target element before
                // setting its value — same visual feedback as click.
                // No-op when the cursor is disabled or the element has no
                // resolvable screen position.
                if let center = AXInput.screenCenter(of: element) {
                    await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                    await AgentCursor.shared.animateAndWait(to: center)
                }

                // ── AXPopUpButton (HTML <select>) special path ──────────
                // Safari WebKit does not propagate AXValue writes back to
                // the HTML DOM for <select> elements — the AX write returns
                // success but the JS `element.value` remains unchanged.
                // Instead, iterate the AX children (the option items) and
                // AXPress the one whose AXTitle or AXValue matches `value`.
                // This bypasses the native macOS popup menu entirely, so the
                // option is selected without the menu needing to be visible.
                let result: CallTool.Result
                if target.role == "AXPopUpButton" {
                    result = try await selectPopupOption(
                        element: element,
                        index: index,
                        pid: pid,
                        value: value,
                        elementTitle: target.title ?? ""
                    )
                } else {
                    // ── Default path: write AXValue directly ────────────────
                    try await AppStateRegistry.focusGuard.withFocusSuppressed(
                        pid: pid,
                        element: element
                    ) {
                        try AXInput.setAttribute(
                            "AXValue",
                            on: element,
                            value: value as CFTypeRef
                        )
                    }
                    let summary =
                        "✅ Set AXValue on [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\"."
                    result = CallTool.Result(
                        content: [.text(text: summary, annotations: nil, _meta: nil)]
                    )
                }
                // Cursor press-pulse after the action — mirrors ClickTool's
                // finishClick sequence. No-op when the cursor is disabled.
                await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                await AgentCursor.shared.playClickPress()
                await AgentCursor.shared.finishClick(pid: pid)
                return result
            } catch let error as AppStateError {
                return errorResult(error.description)
            } catch let error as AXInputError {
                return errorResult(error.description)
            } catch {
                return errorResult("Unexpected error: \(error)")
            }
        }
    )

    /// Select a specific option in an AXPopUpButton.
    ///
    /// Strategy (in order):
    /// 1. AX children — works for native AppKit NSPopUpButton where the option
    ///    elements are AXMenuItem children of the button even when the popup is closed.
    /// 2. JavaScript injection via `osascript` — used when the target is Safari/WebKit,
    ///    whose `<select>` elements expose no AX children without the popup open. Finds
    ///    the matching `<option>` by text or value and sets it via the DOM, dispatching
    ///    a `change` event. No focus steal — osascript's `do JavaScript` runs against
    ///    whichever Safari document is frontmost in the process, regardless of focus.
    private static func selectPopupOption(
        element: AXUIElement,
        index: Int,
        pid: Int32,
        value: String,
        elementTitle: String
    ) async throws -> CallTool.Result {
        // ── Strategy 1: AX children (native AppKit popup buttons) ──────────
        let children = AXInput.children(of: element)
        if !children.isEmpty {
            let valueLower = value.lowercased()
            var matchedIndex = -1
            var availableTitles: [String] = []
            for (i, child) in children.enumerated() {
                let childTitle = AXInput.stringAttribute("AXTitle", of: child) ?? ""
                let childValue = AXInput.stringAttribute("AXValue", of: child) ?? ""
                availableTitles.append(childTitle)
                if childTitle.lowercased() == valueLower
                    || childValue.lowercased() == valueLower
                {
                    matchedIndex = i
                    break
                }
            }
            if matchedIndex >= 0 {
                let option = children[matchedIndex]
                try await AppStateRegistry.focusGuard.withFocusSuppressed(
                    pid: pid,
                    element: option
                ) {
                    try AXInput.performAction("AXPress", on: option)
                }
                let optTitle = AXInput.stringAttribute("AXTitle", of: option) ?? value
                return CallTool.Result(
                    content: [.text(
                        text: "✅ Selected '\(optTitle)' in AXPopUpButton [\(index)] "
                            + "\"\(elementTitle)\" via AX child AXPress.",
                        annotations: nil,
                        _meta: nil
                    )]
                )
            }
            let avail = availableTitles.map { "\"\($0)\"" }.joined(separator: ", ")
            return CallTool.Result(
                content: [.text(
                    text: "❌ No AX child matching '\(value)' in AXPopUpButton [\(index)] "
                        + "\"\(elementTitle)\". Available: [\(avail)].",
                    annotations: nil,
                    _meta: nil
                )],
                isError: true
            )
        }

        // ── Strategy 2: JavaScript injection via osascript (Safari/WebKit) ──
        // WebKit-backed <select> elements expose no AX children when the popup
        // is closed. Fall back to running JavaScript in the Safari document.
        let appName = NSRunningApplication(processIdentifier: pid)?.localizedName ?? ""
        guard appName == "Safari" else {
            return CallTool.Result(
                content: [.text(
                    text: "❌ AXPopUpButton [\(index)] '\(elementTitle)' has no AX children and "
                        + "target is '\(appName)' (not Safari) — no fallback available.",
                    annotations: nil,
                    _meta: nil
                )],
                isError: true
            )
        }
        return await setSelectViaJS(index: index, elementTitle: elementTitle, value: value)
    }

    /// Set an HTML <select> element's value in Safari via `osascript do JavaScript`.
    /// Searches all <select> elements for an <option> whose text or value matches
    /// `value` (case-insensitive), then sets it and dispatches a `change` event.
    private static func setSelectViaJS(
        index: Int,
        elementTitle: String,
        value: String
    ) async -> CallTool.Result {
        // Percent-encode the (lowercased) value using only unreserved
        // URL characters as the allowed set. This means every special
        // character — including single quotes, double quotes, backslashes,
        // and percent signs — is encoded as %XX, which is safe to embed
        // in both a JS single-quoted string (via decodeURIComponent) and
        // an AppleScript double-quoted string without any additional
        // escaping. No double-escape arithmetic needed.
        var unreserved = CharacterSet.alphanumerics
        unreserved.insert(charactersIn: "-._~")
        let vLow = value.lowercased()
        let vEncoded = vLow.addingPercentEncoding(withAllowedCharacters: unreserved) ?? vLow

        let js =
            "(function(){" +
            "var v=decodeURIComponent('\(vEncoded)');" +
            "var ss=document.querySelectorAll('select'),opts=[];" +
            "for(var i=0;i<ss.length;i++){" +
            "for(var j=0;j<ss[i].options.length;j++){" +
            "var t=ss[i].options[j].text.toLowerCase()," +
            "u=ss[i].options[j].value.toLowerCase();" +
            "opts.push(t+'|'+u);" +
            "if(t===v||u===v){" +
            "ss[i].value=ss[i].options[j].value;" +
            "ss[i].dispatchEvent(new Event('change',{bubbles:true}));" +
            "return 'SET:'+ss[i].value;}}" +
            "}return 'NOTFOUND:'+opts.join(',');" +
            "})()"

        let appleScript = "tell application \"Safari\" to do JavaScript \"\(js)\" in front document"
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        proc.arguments = ["-e", appleScript]
        let outPipe = Pipe()
        proc.standardOutput = outPipe
        proc.standardError = Pipe()
        do {
            try proc.run()
        } catch {
            return errorResult("osascript launch failed: \(error)")
        }
        // Wait for osascript with a 10-second deadline. A stuck Safari
        // permission prompt or unresponsive renderer can cause
        // waitUntilExit() to block indefinitely, which would stall
        // the MCP tool handler permanently. Poll on 50 ms ticks so we
        // stay in the async/Swift concurrency cooperative pool.
        let deadline = Date().addingTimeInterval(10.0)
        while proc.isRunning && Date() < deadline {
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        if proc.isRunning {
            proc.terminate()
            return errorResult("osascript timed out after 10 seconds")
        }
        let raw = (String(
            data: outPipe.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)

        if raw.hasPrefix("SET:") {
            let domVal = String(raw.dropFirst(4))
            return CallTool.Result(
                content: [.text(
                    text: "✅ Set select [\(index)] '\(elementTitle)' to '\(value)' via "
                        + "Safari JavaScript (DOM value: \"\(domVal)\").",
                    annotations: nil,
                    _meta: nil
                )]
            )
        }
        if raw.hasPrefix("NOTFOUND:") {
            let available = String(raw.dropFirst(9))
            return errorResult(
                "No <option> matching '\(value)' found in any <select>. "
                + "Available (text|value): \(available)"
            )
        }
        return errorResult("JavaScript returned unexpected output: \(raw.prefix(200))")
    }

    private static func isWindowMinimized(pid: Int32) -> Bool {
        guard let onScreen = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements],
            kCGNullWindowID
        ) as? [[String: Any]] else { return false }
        let hasOnScreen = onScreen.contains {
            ($0[kCGWindowOwnerPID as String] as? Int32) == pid
        }
        if hasOnScreen { return false }
        guard let all = CGWindowListCopyWindowInfo(
            [.optionAll], kCGNullWindowID
        ) as? [[String: Any]] else { return false }
        return all.contains {
            ($0[kCGWindowOwnerPID as String] as? Int32) == pid
            && ($0[kCGWindowLayer as String] as? Int32) == 0
        }
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
