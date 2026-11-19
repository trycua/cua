// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The CI Runners page's methods (`ci.*`): the bundled `cua ci app <op>`,
/// run as `CiRunners` describes. A build without the bundled `cua` says so.
extension WebUIBridge {
    func routeCi(_ method: String, _ args: [String: Any]) async throws -> Any? {
        switch method {
        case "ci.overview", "ci.connect", "ci.connectToken", "ci.connectCancel", "ci.repositories", "ci.goal", "ci.log":
            break
        default:
            return nil
        }
        guard let operation = CiRunners.operations[method] else { return nil }
        let input = try CiRunners.input(method: method, args: args)
        guard let cua = AppEnvironment.bundledCuaPath else {
            throw Failure.unsupported("This build has no bundled cua to run CI Runners")
        }
        let run = await CiRunners.run(cua: cua, operation: operation, input: input)
        return try CiRunners.answer(stdout: run.stdout, stderr: run.stderr, status: run.status)
    }
}
