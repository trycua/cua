import CuaDriverCore
import Foundation
import MCP

public enum ListAppsTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "list_apps",
            description: """
                List macOS apps — both currently running and installed-but-not-running —
                with per-app state flags:

                - running: is a process for this app live? (pid is 0 when false)
                - active: is it the system-frontmost app? (implies running)

                Only apps with NSApplicationActivationPolicyRegular are included —
                background helpers and system UI agents are filtered out. Installed
                apps come from scanning /Applications, /Applications/Utilities,
                ~/Applications, /System/Applications, and /System/Applications/Utilities.

                Use this for "is X installed?" as well as "is X running?". For
                per-window state — on-screen, on-current-Space, minimized,
                window titles — call list_windows instead. For just opening an
                app — running or not — call launch_app({bundle_id: ...}) directly;
                list_apps is not a prerequisite.
                """,
            inputSchema: [
                "type": "object",
                "properties": [:],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: true,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { _ in
            let apps = AppEnumerator.apps()
            let textContent: Tool.Content = .text(
                text: summary(apps), annotations: nil, _meta: nil
            )
            let output = Output(apps: apps)
            if let result = try? CallTool.Result(
                content: [textContent],
                structuredContent: output
            ) {
                return result
            }
            return CallTool.Result(content: [textContent])
        }
    )

    struct Output: Codable, Sendable {
        let apps: [AppInfo]
    }

    private static func summary(_ apps: [AppInfo]) -> String {
        let running = apps.filter { $0.running }
        let total = apps.count
        var lines = [
            "✅ Found \(total) app(s): \(running.count) running, \(total - running.count) installed-not-running."
        ]
        for app in running {
            let bundle = app.bundleId.map { " [\($0)]" } ?? ""
            lines.append("- \(app.name) (pid \(app.pid))\(bundle)")
        }
        return lines.joined(separator: "\n")
    }
}
