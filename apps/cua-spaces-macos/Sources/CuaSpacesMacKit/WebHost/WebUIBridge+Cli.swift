// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// The app's bundled `cua` on PATH: the first run's "Install the Cua CLI"
/// screen and Settings' Command line row (`ops/cli.ts` in the web bridge).
/// Both run the app core's installer (`AppCliInstaller`): `cli.plan` reads
/// what installing would do and writes nothing; `cli.install` writes only
/// after the person pressed Install. What it installs is this build's
/// channel's (`cua`, or `cua-local` in Cua Spaces Local: the app sets
/// `CUA_CHANNEL` before anything starts), and it adds the bin folder to PATH
/// only when asked (when it is not on PATH).
extension WebUIBridge {
    func routeCli(_ method: String, _ args: [String: Any]) async throws -> Any? {
        switch method {
        case "cli.plan":
            let plan = await makeCliInstaller().plan()
            noteCliPlan(plan)
            return BridgeValue.encode(plan)
        case "cli.install":
            guard let modifyPath = args["modifyPath"] as? Bool else { throw Failure.badArgs("modifyPath: boolean") }
            let plan = try await makeCliInstaller().install(request: AppCliInstallRequest(modifyPath: modifyPath))
            noteCliPlan(plan)
            return BridgeValue.encode(plan)
        default:
            return nil
        }
    }

    /// `settings.get`'s `cli`: the CLI's name (the plan's file) and whether
    /// it is on this computer; null until the plan was read once (read now,
    /// and `settings.changed` follows).
    func cliSetting() -> Any {
        guard let plan = cliPlan else {
            if !cliPlanRequested {
                cliPlanRequested = true
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.noteCliPlan(await self.makeCliInstaller().plan())
                }
            }
            return NSNull()
        }
        let file = URL(fileURLWithPath: plan.target).lastPathComponent
        return [
            "name": file.hasSuffix(".exe") ? String(file.dropLast(4)) : file,
            "installed": plan.upToDate || plan.found != nil,
        ] as [String: Any]
    }

    func noteCliPlan(_ plan: AppCliInstallPlan) {
        let changed = cliPlan != plan
        cliPlan = plan
        if changed { host?.emit("settings.changed") }
    }
}
