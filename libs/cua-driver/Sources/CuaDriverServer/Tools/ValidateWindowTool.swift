import CuaDriverCore
import Foundation
import MCP

public enum ValidateWindowTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "validate_window",
            description: """
                Validate a task-level window lease before acting.

                Use this when a caller has pinned work to a specific
                pid/window_id or window_uid and needs to know whether that
                target still exists, still belongs to the same process, or
                should be treated as lost. The tool never switches windows
                implicitly. It returns explicit status plus same-pid windows
                and possible replacement candidates so the caller can decide
                whether it has enough evidence to reacquire.

                Status values:

                - present: requested window exists and belongs to pid.
                - missing: requested window_id/window_uid is not present.
                - pid_mismatch: window_id exists but belongs to another pid.

                Replacement candidates are conservative: exact title matches
                when `expected_title` is supplied; otherwise the only same-pid
                window when there is exactly one.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Expected owning process id.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description": "CGWindowID from list_windows / launch_app.",
                    ],
                    "window_uid": [
                        "type": "string",
                        "description": "Stable uid from list_windows. Used when window_id is absent.",
                    ],
                    "expected_title": [
                        "type": "string",
                        "description": "Optional title used to identify possible replacements when the window is missing.",
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
            guard let rawPid = arguments?["pid"]?.intValue,
                  let pid = Int32(exactly: rawPid)
            else {
                return errorResult("Missing or invalid required integer field pid.")
            }

            let rawWindowId = arguments?["window_id"]?.intValue
            let uid = arguments?["window_uid"]?.stringValue
            let expectedTitle = arguments?["expected_title"]?.stringValue
            let windowId = rawWindowId ?? parseWindowId(fromUID: uid)
            guard let windowId else {
                return errorResult("validate_window requires window_id or window_uid.")
            }

            let allInfos = WindowEnumerator.allWindows().filter { $0.layer == 0 }
            let samePidInfos = allInfos.filter { $0.pid == pid }
            let currentSpaceID = SpaceMigrator.currentSpaceID()
            let identities = await WindowIdentityStore.shared.metadata(for: allInfos)
            let samePidRows = samePidInfos.map {
                ListWindowsTool.row(
                    for: $0,
                    currentSpaceID: currentSpaceID,
                    identity: identities[$0.id]
                )
            }

            let requested = Requested(
                pid: pid,
                windowId: windowId,
                windowUID: uid
            )

            if let found = allInfos.first(where: { $0.id == windowId }) {
                let row = ListWindowsTool.row(
                    for: found,
                    currentSpaceID: currentSpaceID,
                    identity: identities[found.id]
                )
                if found.pid == pid {
                    return result(
                        Output(
                            status: .present,
                            requested: requested,
                            reason: "window is present and belongs to pid \(pid)",
                            window: row,
                            actualOwnerPid: nil,
                            knownWindows: samePidRows,
                            possibleReplacements: []
                        )
                    )
                }

                return result(
                    Output(
                        status: .pidMismatch,
                        requested: requested,
                        reason: "window_id \(windowId) belongs to pid \(found.pid), not pid \(pid)",
                        window: row,
                        actualOwnerPid: found.pid,
                        knownWindows: samePidRows,
                        possibleReplacements: replacements(
                            from: samePidRows,
                            expectedTitle: expectedTitle
                        )
                    )
                )
            }

            return result(
                Output(
                    status: .missing,
                    requested: requested,
                    reason: "window_id \(windowId) is not present in WindowServer",
                    window: nil,
                    actualOwnerPid: nil,
                    knownWindows: samePidRows,
                    possibleReplacements: replacements(
                        from: samePidRows,
                        expectedTitle: expectedTitle
                    )
                )
            )
        }
    )

    enum Status: String, Codable, Sendable {
        case present
        case missing
        case pidMismatch = "pid_mismatch"
    }

    struct Requested: Codable, Sendable {
        let pid: Int32
        let windowId: Int
        let windowUID: String?

        private enum CodingKeys: String, CodingKey {
            case pid
            case windowId = "window_id"
            case windowUID = "window_uid"
        }
    }

    struct Output: Codable, Sendable {
        let status: Status
        let requested: Requested
        let reason: String
        let window: ListWindowsTool.Row?
        let actualOwnerPid: Int32?
        let knownWindows: [ListWindowsTool.Row]
        let possibleReplacements: [ListWindowsTool.Row]

        private enum CodingKeys: String, CodingKey {
            case status
            case requested
            case reason
            case window
            case actualOwnerPid = "actual_owner_pid"
            case knownWindows = "known_windows"
            case possibleReplacements = "possible_replacements"
        }
    }

    private static func result(_ output: Output) -> CallTool.Result {
        let text = "\(output.status.rawValue): \(output.reason)"
        let content: [Tool.Content] = [
            .text(text: text, annotations: nil, _meta: nil)
        ]
        if let result = try? CallTool.Result(
            content: content,
            structuredContent: output
        ) {
            return result
        }
        return CallTool.Result(content: content)
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }

    private static func replacements(
        from rows: [ListWindowsTool.Row],
        expectedTitle: String?
    ) -> [ListWindowsTool.Row] {
        if let expectedTitle {
            let needle = expectedTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                .lowercased()
            if !needle.isEmpty {
                let matches = rows.filter {
                    $0.title.trimmingCharacters(in: .whitespacesAndNewlines)
                        .lowercased() == needle
                }
                if !matches.isEmpty { return matches }
            }
        }
        return rows.count == 1 ? rows : []
    }

    private static func parseWindowId(fromUID uid: String?) -> Int? {
        guard let uid else { return nil }
        let parts = uid.split(separator: ":")
        guard parts.count >= 2, parts[0] == "cgwindow" else { return nil }
        return Int(parts[1])
    }
}
