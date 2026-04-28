import AppKit
import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

public enum ListWindowsTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "list_windows",
            description: """
                List every layer-0 top-level window currently known to
                WindowServer — including off-screen ones (hidden-launched,
                minimized into the Dock, on another Space). Each record
                self-contains its owning app identity so the caller never
                has to join back against list_apps.

                Use this — not list_apps — for any window-level reasoning:
                "does this app have a visible window right now?", "which
                Space is this window on?", "which of this pid's windows
                is the main one?". list_apps is purely an app / launch
                surface.

                Per-record fields:

                - window_id: CGWindowID, addressable for screenshot /
                  activation / Space-move APIs.
                - window_uid: daemon-stable identity for this OS window
                  observation. Stable across list order changes and title /
                  bounds changes while the CGWindowID remains owned by the
                  same pid.
                - generation / first_seen / last_seen: lifecycle metadata
                  for callers maintaining a task-level window lease.
                - pid + app_name: owning app identity.
                - bundle_id: owning app bundle identifier when available.
                - title: current window title (empty string for helpers
                  and chromeless surfaces).
                - bounds: x / y / width / height, top-left origin in
                  global screen points.
                - display_id: CoreGraphics display containing the largest
                  part of the window, when available.
                - layer: CGWindow stratum. Always 0 in the default
                  filter; reserved for future higher-layer opt-in.
                - z_index: stacking order on the current Space (higher =
                  closer to front). Cross-Space ordering is undefined.
                - is_on_screen: WindowServer's \"visible on the user's
                  current Space right now\" bit. False for hidden /
                  minimized / off-Space windows.
                - on_current_space: true when the window is bound to the
                  user's current Space. Omitted when the SkyLight Space
                  SPIs didn't resolve (rare; sandboxed or very old OS).
                - space_ids: every managed Space id the window is a
                  member of. Compare against current_space_id in the
                  top-level response. Omitted in the same failure modes
                  as on_current_space.

                Top-level fields:

                - windows: array of the per-window records above.
                - current_space_id: user's active Space id, or null when
                  SPI unavailable.

                Inputs: pid (optional — restrict to one pid's windows),
                on_screen_only (bool, default false — surface off-Space /
                minimized windows by default). Layer 0 filtering is
                always applied; menubar strips and dock shields are
                noise for every current caller.
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description":
                            "Optional pid filter. When set, only this pid's windows are returned.",
                    ],
                    "on_screen_only": [
                        "type": "boolean",
                        "description":
                            "When true, drop windows that aren't currently on the user's Space (minimized, hidden, off-Space). Default false.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            let pidFilter: Int32?
            if let rawPid = arguments?["pid"]?.intValue {
                guard let validated = Int32(exactly: rawPid) else {
                    return errorResult(
                        "pid \(rawPid) is outside the supported Int32 range."
                    )
                }
                pidFilter = validated
            } else {
                pidFilter = nil
            }
            let onScreenOnly = arguments?["on_screen_only"]?.boolValue ?? false

            let raw = onScreenOnly
                ? WindowEnumerator.visibleWindows()
                : WindowEnumerator.allWindows()

            let windows = raw
                .filter { $0.layer == 0 }
                .filter { info in
                    guard let pid = pidFilter else { return true }
                    return info.pid == pid
                }

            let currentSpaceID = SpaceMigrator.currentSpaceID()
            let identities = await WindowIdentityStore.shared.metadata(for: windows)
            let records = windows.map { info -> Row in
                row(
                    for: info,
                    currentSpaceID: currentSpaceID,
                    identity: identities[info.id]
                )
            }

            let textContent: Tool.Content = .text(
                text: summary(records, currentSpaceID: currentSpaceID),
                annotations: nil,
                _meta: nil
            )
            let output = Output(windows: records, currentSpaceId: currentSpaceID)
            if let result = try? CallTool.Result(
                content: [textContent],
                structuredContent: output
            ) {
                return result
            }
            return CallTool.Result(content: [textContent])
        }
    )

    struct Row: Codable, Sendable {
        let windowId: Int
        let windowUID: String?
        let generation: Int?
        let firstSeen: String?
        let lastSeen: String?
        let pid: Int32
        let appName: String
        let bundleId: String?
        let title: String
        let bounds: WindowBounds
        let displayId: UInt32?
        let layer: Int
        let zIndex: Int
        let isOnScreen: Bool
        let onCurrentSpace: Bool?
        let spaceIds: [UInt64]?

        private enum CodingKeys: String, CodingKey {
            case windowId = "window_id"
            case windowUID = "window_uid"
            case generation
            case firstSeen = "first_seen"
            case lastSeen = "last_seen"
            case pid
            case appName = "app_name"
            case bundleId = "bundle_id"
            case title
            case bounds
            case displayId = "display_id"
            case layer
            case zIndex = "z_index"
            case isOnScreen = "is_on_screen"
            case onCurrentSpace = "on_current_space"
            case spaceIds = "space_ids"
        }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(windowId, forKey: .windowId)
            try c.encodeIfPresent(windowUID, forKey: .windowUID)
            try c.encodeIfPresent(generation, forKey: .generation)
            try c.encodeIfPresent(firstSeen, forKey: .firstSeen)
            try c.encodeIfPresent(lastSeen, forKey: .lastSeen)
            try c.encode(pid, forKey: .pid)
            try c.encode(appName, forKey: .appName)
            try c.encodeIfPresent(bundleId, forKey: .bundleId)
            try c.encode(title, forKey: .title)
            try c.encode(bounds, forKey: .bounds)
            try c.encodeIfPresent(displayId, forKey: .displayId)
            try c.encode(layer, forKey: .layer)
            try c.encode(zIndex, forKey: .zIndex)
            try c.encode(isOnScreen, forKey: .isOnScreen)
            // Omit the Space fields entirely when unknown so consumers
            // can use membership checks to distinguish "not on current
            // Space" from "we couldn't tell".
            try c.encodeIfPresent(onCurrentSpace, forKey: .onCurrentSpace)
            try c.encodeIfPresent(spaceIds, forKey: .spaceIds)
        }
    }

    struct Output: Codable, Sendable {
        let windows: [Row]
        let currentSpaceId: UInt64?

        private enum CodingKeys: String, CodingKey {
            case windows
            case currentSpaceId = "current_space_id"
        }
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }

    static func row(
        for info: WindowInfo,
        currentSpaceID: UInt64?,
        identity: WindowIdentityMetadata?
    ) -> Row {
        let spaceIDs = SpaceMigrator.spaceIDs(forWindowID: UInt32(info.id))
        let onCurrentSpace: Bool? = {
            guard let spaceIDs, let currentSpaceID else { return nil }
            return spaceIDs.contains(currentSpaceID)
        }()
        return Row(
            windowId: info.id,
            windowUID: identity?.windowUID,
            generation: identity?.generation,
            firstSeen: identity?.firstSeen,
            lastSeen: identity?.lastSeen,
            pid: info.pid,
            appName: info.owner,
            bundleId: bundleIdentifier(for: info.pid),
            title: info.name,
            bounds: info.bounds,
            displayId: displayID(for: info.bounds),
            layer: info.layer,
            zIndex: info.zIndex,
            isOnScreen: info.isOnScreen,
            onCurrentSpace: onCurrentSpace,
            spaceIds: spaceIDs
        )
    }

    private static func bundleIdentifier(for pid: Int32) -> String? {
        NSRunningApplication(processIdentifier: pid)?.bundleIdentifier
    }

    private static func displayID(for bounds: WindowBounds) -> UInt32? {
        let rect = CGRect(
            x: bounds.x,
            y: bounds.y,
            width: max(bounds.width, 1),
            height: max(bounds.height, 1)
        )
        var count: UInt32 = 0
        CGGetDisplaysWithRect(rect, 0, nil, &count)
        guard count > 0 else { return nil }
        var displays = Array(repeating: CGDirectDisplayID(0), count: Int(count))
        CGGetDisplaysWithRect(rect, count, &displays, &count)
        return displays.first
    }

    private static func summary(
        _ windows: [Row], currentSpaceID: UInt64?
    ) -> String {
        let total = windows.count
        let onScreen = windows.filter { $0.isOnScreen }.count
        let onCurrent = windows.filter { $0.onCurrentSpace == true }.count
        let pids = Set(windows.map { $0.pid }).count
        var headline = "✅ Found \(total) window(s) across \(pids) app(s); \(onScreen) on-screen"
        if currentSpaceID != nil {
            headline += ", \(onCurrent) on current Space."
        } else {
            headline += ". (SkyLight Space SPIs unavailable — on_current_space / space_ids omitted.)"
        }
        var lines = [headline]
        for w in windows {
            let title = w.title.isEmpty ? "(no title)" : "\"\(w.title)\""
            let tag = w.isOnScreen ? "" : " [off-screen]"
            lines.append("- \(w.appName) (pid \(w.pid)) \(title) [window_id: \(w.windowId)]\(tag)")
        }
        return lines.joined(separator: "\n")
    }
}
