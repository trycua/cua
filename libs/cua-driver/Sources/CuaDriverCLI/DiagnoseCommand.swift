import AppKit
import ApplicationServices
import ArgumentParser
import CuaDriverCore
import Foundation
import ScreenCaptureKit
import Security

/// `cua-driver diagnose` — print the full state needed to debug TCC /
/// install-layout issues in one paste-able block.
///
/// Primary motivating case: a user reports the first-launch permissions
/// window asking for grants they've already given. Instead of asking
/// them to run five shell commands and copy output piecemeal, they run
/// this subcommand once and paste what it prints.
///
/// The output covers:
///   - running-process identity (path, bundle id, pid, cdhash)
///   - TCC probe results (Accessibility + Screen Recording, as the live
///     process sees them)
///   - install layout (/Applications/CuaDriver.app bundle + signature,
///     ~/.local/bin/cua-driver symlink resolution)
///   - TCC DB rows for `com.trycua.driver` (best-effort; system TCC DB
///     requires Full Disk Access)
///   - config + state paths (LaunchAgent plist, user-data dir, config
///     dir) with existence booleans
struct DiagnoseCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "diagnose",
        abstract: "Print a paste-able bundle-path / cdhash / TCC-status report for support."
    )

    func run() async throws {
        let sections: [String] = [
            runtimeSection(),
            signatureSection(for: Bundle.main.bundleURL),
            await tccSection(),
            installLayoutSection(),
            tccDatabaseSection(),
            configPathsSection(),
        ]
        print(sections.joined(separator: "\n\n"))
    }

    // MARK: - Sections

    private func runtimeSection() -> String {
        let bundle = Bundle.main
        var lines: [String] = ["## running process"]
        lines.append("version:         \(CuaDriverCore.version)")
        lines.append("argv[0]:         \(CommandLine.arguments.first ?? "<unknown>")")
        lines.append("bundleURL:       \(bundle.bundleURL.path)")
        lines.append("executablePath:  \(bundle.executablePath ?? "<unknown>")")
        lines.append("bundleIdentifier: \(bundle.bundleIdentifier ?? "<unset>")")
        lines.append("pid:             \(ProcessInfo.processInfo.processIdentifier)")
        return lines.joined(separator: "\n")
    }

    private func signatureSection(for bundleURL: URL) -> String {
        var lines: [String] = ["## running process signature"]
        let info = codesignInfo(at: bundleURL)
        lines.append("cdhash:    \(info.cdhash ?? "<unknown>")")
        lines.append("teamID:    \(info.teamID ?? "<none — ad-hoc signed?>")")
        lines.append("authority: \(info.authority ?? "<none — ad-hoc signed?>")")
        return lines.joined(separator: "\n")
    }

    private func tccSection() async -> String {
        let status = await Permissions.currentStatus()
        var lines: [String] = ["## tcc probes (live process)"]
        lines.append("accessibility      (AXIsProcessTrusted):  \(status.accessibility)")
        lines.append("screen recording   (SCShareableContent):  \(status.screenRecording)")
        lines.append("")
        lines.append("if the UI disagrees with these booleans the live process is fine —")
        lines.append("the issue is elsewhere (wrong bundle granted, stale cdhash, etc).")
        return lines.joined(separator: "\n")
    }

    private func installLayoutSection() -> String {
        var lines: [String] = ["## install layout"]

        // App bundle at the canonical /Applications path.
        let appPath = "/Applications/CuaDriver.app"
        let appURL = URL(fileURLWithPath: appPath)
        let appExists = FileManager.default.fileExists(atPath: appPath)
        lines.append("bundle:  \(appPath)   exists=\(appExists)")
        if appExists {
            let info = codesignInfo(at: appURL)
            lines.append("  cdhash:    \(info.cdhash ?? "<unknown>")")
            lines.append("  teamID:    \(info.teamID ?? "<none>")")
            lines.append("  authority: \(info.authority ?? "<none>")")
        }

        // CLI symlink that an MCP client (Claude Code etc.) invokes.
        let cliPaths = [
            ("symlink", "\(NSHomeDirectory())/.local/bin/cua-driver"),
            ("legacy symlink", "/usr/local/bin/cua-driver"),
        ]
        for (label, cliPath) in cliPaths {
            let cliExists = FileManager.default.fileExists(atPath: cliPath)
            lines.append("\(label): \(cliPath)   exists=\(cliExists)")
            if cliExists {
                let resolved = (try? FileManager.default.destinationOfSymbolicLink(
                    atPath: cliPath)) ?? "<not a symlink>"
                lines.append("  resolves to: \(resolved)")
            }
        }

        // Stale dev-install paths — flag these specifically so users who
        // followed an older install-local.sh (pre-path-alignment) see
        // they have leftovers to clean up.
        let stalePaths = [
            "\(NSHomeDirectory())/Applications/CuaDriver.app",
        ]
        for stale in stalePaths where FileManager.default.fileExists(atPath: stale) {
            lines.append("stale:   \(stale)   ← old install-local.sh path, consider removing")
        }

        return lines.joined(separator: "\n")
    }

    private func tccDatabaseSection() -> String {
        var lines: [String] = ["## tcc database rows for com.trycua.driver"]

        let userDB = "\(NSHomeDirectory())/Library/Application Support/com.apple.TCC/TCC.db"
        lines.append("(reading \(userDB) — best-effort; system TCC DB requires FDA)")
        lines.append("")

        let sql = "SELECT service, client, client_type, auth_value, auth_reason, "
            + "hex(csreq) AS csreq_hex FROM access WHERE client='com.trycua.driver';"

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/sqlite3")
        process.arguments = ["-header", "-column", userDB, sql]
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        do {
            try process.run()
            process.waitUntilExit()
            let outData = stdout.fileHandleForReading.readDataToEndOfFile()
            let errData = stderr.fileHandleForReading.readDataToEndOfFile()
            let out = String(data: outData, encoding: .utf8) ?? ""
            let err = String(data: errData, encoding: .utf8) ?? ""
            if !out.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                lines.append(out)
            } else if process.terminationStatus != 0, !err.isEmpty {
                lines.append("(sqlite3 failed: \(err.trimmingCharacters(in: .whitespacesAndNewlines)))")
            } else {
                lines.append("(no rows — either grants never made it to TCC, or they live")
                lines.append(" in the system DB which requires Full Disk Access to read.)")
            }
        } catch {
            lines.append("(could not launch sqlite3: \(error))")
        }

        lines.append("")
        lines.append("# auth_value legend: 0=denied  2=allowed")
        lines.append("# services: kTCCServiceAccessibility, kTCCServiceScreenCapture")
        return lines.joined(separator: "\n")
    }

    private func configPathsSection() -> String {
        let home = NSHomeDirectory()
        let paths: [(String, String)] = [
            ("user data dir",     "\(home)/.cua-driver"),
            ("config dir",        "\(home)/Library/Application Support/Cua Driver"),
            ("telemetry id",      "\(home)/.cua-driver/.telemetry_id"),
            ("install marker",    "\(home)/.cua-driver/.installation_recorded"),
            ("updater plist",     "\(home)/Library/LaunchAgents/com.trycua.cua_driver_updater.plist"),
            ("daemon plist",      "\(home)/Library/LaunchAgents/com.trycua.cua_driver_daemon.plist"),
        ]
        var lines: [String] = ["## config + state paths"]
        for (label, path) in paths {
            let exists = FileManager.default.fileExists(atPath: path)
            lines.append("\(label.padding(toLength: 16, withPad: " ", startingAt: 0)) exists=\(exists)   \(path)")
        }
        return lines.joined(separator: "\n")
    }

    // MARK: - Codesign introspection

    private struct CodeSignInfo {
        let cdhash: String?
        let teamID: String?
        let authority: String?
    }

    /// Read cdhash + team-id + signing authority for a signed bundle or
    /// binary. Returns `nil` fields when the target is ad-hoc-signed,
    /// unsigned, or unreadable — callers surface "<none>" to the user.
    private func codesignInfo(at url: URL) -> CodeSignInfo {
        var staticCode: SecStaticCode?
        guard
            SecStaticCodeCreateWithPath(url as CFURL, [], &staticCode)
                == errSecSuccess,
            let staticCode
        else {
            return CodeSignInfo(cdhash: nil, teamID: nil, authority: nil)
        }

        var info: CFDictionary?
        let flags: SecCSFlags = SecCSFlags(
            rawValue: kSecCSSigningInformation | kSecCSRequirementInformation)
        guard
            SecCodeCopySigningInformation(staticCode, flags, &info) == errSecSuccess,
            let dict = info as? [String: Any]
        else {
            return CodeSignInfo(cdhash: nil, teamID: nil, authority: nil)
        }

        let cdhash: String? = (dict[kSecCodeInfoUnique as String] as? Data).map {
            $0.map { String(format: "%02x", $0) }.joined()
        }
        let teamID = dict[kSecCodeInfoTeamIdentifier as String] as? String
        // `kSecCodeInfoCertificates` → array of SecCertificate in
        // leaf-first order. The leaf's common name is the human-readable
        // "Developer ID Application: Foo Bar (TEAMID)" string users
        // recognize from codesign -dv.
        var authority: String? = nil
        if let certs = dict[kSecCodeInfoCertificates as String] as? [SecCertificate],
           let leaf = certs.first
        {
            var commonName: CFString?
            if SecCertificateCopyCommonName(leaf, &commonName) == errSecSuccess,
               let commonName
            {
                authority = commonName as String
            }
        }
        return CodeSignInfo(cdhash: cdhash, teamID: teamID, authority: authority)
    }
}
