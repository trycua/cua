// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI

/// Settings, Advanced's runtime rows: `runtimes.check` (what a typed path
/// is, before it is saved) and `runtimes.browse` (the file panel). The rows
/// themselves come with `settings.get` (`runtimes`, laid out by the core)
/// and change with `settings.choose`.
extension WebUIBridge {
    func runtimes(_ method: String, _ args: [String: Any]) async throws -> Any {
        let kind = try string(args, "kind")
        switch method {
        case "runtimes.check":
            let path = try string(args, "path")
            do {
                return BridgeValue.encode(try await model.backend.runtimeCheckPath(kind: kind, path: path))
            } catch {
                throw Failure(code: "failed", message: LiveSpacesBackend.words(error))
            }
        default:
            let picked = await pickRuntime(kind)
            return ["path": picked?.path ?? NSNull()] as [String: Any]
        }
    }

    /// A file panel for one program (a binary anywhere, `/usr/local/bin`
    /// first).
    static func runtimePanel(_ kind: String) -> URL? {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.treatsFilePackagesAsDirectories = true
        panel.showsHiddenFiles = true
        panel.directoryURL = URL(fileURLWithPath: "/usr/local/bin")
        panel.message = "Choose the program to use"
        panel.prompt = "Use"
        return panel.runModal() == .OK ? panel.url : nil
    }
}
