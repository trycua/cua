import ArgumentParser
import Foundation

struct CheckUpdate: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "check-update",
        abstract: "Check whether a newer Lume release is available",
        discussion: """
            Read-only update check. Uses GitHub Releases, caches the result briefly,
            and never installs anything.
            """
    )

    @Flag(help: "Emit the structured update-state payload as JSON")
    var json: Bool = false

    @Flag(help: "Bypass the local update-check cache")
    var noCache: Bool = false

    func run() async throws {
        guard LumeVersionCheck.updateChecksEnabled() else {
            if json {
                try printJSON(LumeVersionCheck.disabledState())
            } else {
                print("Update checks disabled by LUME_UPDATE_CHECK.")
            }
            return
        }

        let state = await LumeVersionCheck.checkUpdateState(noCache: noCache)

        if json {
            try printJSON(state)
        } else if let error = state.error {
            print("Update check failed: \(error)")
        } else if state.updateAvailable, let latest = state.latestVersion {
            print("Update available: Lume \(latest) (you have \(state.currentVersion)).")
            print("Update with: lume update --apply")
        } else {
            print("Up to date (Lume \(state.currentVersion)).")
        }
    }
}

struct Update: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "update",
        abstract: "Check for a Lume update and optionally apply it",
        discussion: """
            Without --apply, this command only checks for a newer release and prints
            the command to install it. With --apply, it delegates installation to
            the canonical Lume installer pinned to the discovered version.
            """
    )

    @Flag(help: "Apply the update by re-running the official installer")
    var apply: Bool = false

    @Flag(help: "Emit the structured update-state payload as JSON")
    var json: Bool = false

    func run() async throws {
        if json && apply {
            throw ValidationError("--json cannot be combined with --apply because installer output is interactive.")
        }

        guard LumeVersionCheck.updateChecksEnabled() else {
            if json {
                try printJSON(LumeVersionCheck.disabledState())
            } else {
                print("Update checks disabled by LUME_UPDATE_CHECK.")
            }
            return
        }

        let state = await LumeVersionCheck.checkUpdateState(noCache: false)

        if json {
            try printJSON(state)
        }

        if let error = state.error {
            if !json {
                print("Update check failed: \(error)")
            }
            throw ExitCode.failure
        }

        guard state.updateAvailable, let latest = state.latestVersion else {
            if !json {
                print("Up to date (Lume \(state.currentVersion)).")
            }
            return
        }

        if !apply {
            if !json {
                print("Update available: Lume \(latest) (you have \(state.currentVersion)).")
                print("Run `lume update --apply` to install.")
            }
            return
        }

        if !json {
            print("Installing Lume \(latest)...")
        }

        let status = try LumeVersionCheck.runInstallScript(version: latest)
        guard status == 0 else {
            if !json {
                print("Installer failed with exit status \(status).")
                print("You can retry manually: \(LumeVersionCheck.manualInstallCommand())")
            }
            throw ExitCode(status)
        }

        if !json {
            print("Lume \(latest) installed.")
        }
    }
}

private func printJSON<T: Encodable>(_ value: T) throws {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    let data = try encoder.encode(value)
    if let string = String(data: data, encoding: .utf8) {
        print(string)
    }
}
