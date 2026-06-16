import AppKit
import ApplicationServices
import CuaDriverCore
import Foundation
import MCP
import ScreenCaptureKit

/// `health_report` — single-call end-to-end driver diagnostics.
///
/// The point of this tool is to let downstream consumers (Hermes Agent
/// and similar) ship one stable diagnostic call and never have to know
/// cua-driver internals: tool names, TCC field names, bundle IDs, per-
/// platform check matrix. The tool owns the health model; consumers stay
/// thin and cua-driver evolves freely.
///
/// Output shape is the stable contract — documented inline in the schema
/// text. `schema_version: "1"` is the commitment; future breaking changes
/// go to `"2"`.
///
/// Reuses `Permissions.currentStatus()` for the TCC checks rather than
/// duplicating the probe logic. The TCC checks therefore inherit the
/// same caveat as `check_permissions`: in-process TCC reads reflect the
/// calling process's responsibility chain. When called via stdio MCP
/// from a terminal, that's the terminal — not CuaDriver.app. The data
/// payload for each TCC check surfaces this attribution so consumers
/// don't get misled.
public enum HealthReportTool {

    // MARK: - Canonical check names (also the values of `output.checks[].name`)

    static let nameBinaryVersion = "binary_version"
    static let namePlatformSupported = "platform_supported"
    static let nameTccAccessibility = "tcc_accessibility"
    static let nameTccScreenRecording = "tcc_screen_recording"
    static let nameBundleIdentity = "bundle_identity"
    static let nameAxCapability = "ax_capability"
    static let nameScreenCaptureCapability = "screen_capture_capability"
    static let nameSessionActive = "session_active"

    /// Checks whose failure marks the whole report as `failed` (vs `degraded`).
    /// Binary and platform are non-negotiable; everything else is degraded.
    static let coreChecks: Set<String> = [
        nameBinaryVersion,
        namePlatformSupported,
        nameSessionActive,
    ]

    /// All check names, in the canonical macOS run order.
    static let allMacOSChecks: [String] = [
        nameBinaryVersion,
        namePlatformSupported,
        nameSessionActive,
        nameBundleIdentity,
        nameTccAccessibility,
        nameTccScreenRecording,
        nameAxCapability,
        nameScreenCaptureCapability,
    ]

    public static let handler = ToolHandler(
        tool: Tool(
            name: "health_report",
            description: """
                Single-call end-to-end driver diagnostics. Designed to let
                downstream consumers (Hermes Agent and similar) ship one
                stable call instead of stitching together check_permissions,
                doctor, version, bundle attribution, and a screenshot probe.
                cua-driver owns the health model; consumers stay thin.

                Input — all optional:
                  {
                    "include": ["<check_name>", ...],   // run only these
                    "skip":    ["<check_name>", ...]    // skip these
                  }
                If both are given, `include` wins.

                Canonical check names (macOS):
                  binary_version, platform_supported, session_active,
                  bundle_identity, tcc_accessibility, tcc_screen_recording,
                  ax_capability, screen_capture_capability

                Output — stable contract, schema_version="1":
                  {
                    "schema_version": "1",
                    "platform": "darwin" | "win32" | "linux",
                    "driver_version": "<semver>",
                    "overall": "ok" | "degraded" | "failed",
                    "checks": [
                      {
                        "name": "<one of the canonical names above>",
                        "status": "pass" | "fail" | "skip",
                        "message": "<one-line summary, always present>",
                        "hint": "<remediation step, present when status=fail>",
                        "data": { /* check-specific structured fields */ }
                      },
                      ...
                    ]
                  }

                `overall` rules:
                  - `ok`       — every non-skipped check passes
                  - `degraded` — at least one non-core check fails
                                 (binary is still usable)
                  - `failed`   — any core check fails
                                 (binary_version, platform_supported,
                                  session_active)

                Stability: schema_version="1" is the contract. Future
                breaking changes will be `"2"`. Adding new check names
                under the same schema_version is non-breaking; consumers
                must tolerate unknown check names.

                TCC caveat: in-process TCC reads reflect the calling
                process's responsibility chain. When called via stdio MCP
                spawned from a terminal, that's the terminal — not
                CuaDriver.app. Each TCC check's `data` block exposes the
                runtime bundle identifier so consumers can detect the
                mismatch.
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "include": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Only run these checks (canonical names). Wins over `skip`.",
                    ],
                    "skip": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Skip these checks (canonical names). Ignored when `include` is set.",
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
            let include = parseStringSet(arguments?["include"])
            let skip = parseStringSet(arguments?["skip"])

            let toRun = selectChecks(
                allChecks: allMacOSChecks, include: include, skip: skip
            )

            // Drive the probes. Skipped checks still appear in the
            // output with status=skip so consumers see a complete map
            // of which checks were considered.
            var checks: [CheckEntry] = []
            for name in allMacOSChecks {
                if !toRun.contains(name) {
                    checks.append(
                        CheckEntry(
                            name: name,
                            status: .skip,
                            message: "Skipped by include/skip filter.",
                            hint: nil,
                            data: nil
                        )
                    )
                    continue
                }
                let entry = await runCheck(name: name)
                checks.append(entry)
            }

            let overall = computeOverall(checks: checks)
            let report = Report(
                schemaVersion: "1",
                platform: "darwin",
                driverVersion: CuaDriverCore.version,
                overall: overall,
                checks: checks
            )

            let textContent: Tool.Content = .text(
                text: textSummary(report: report), annotations: nil, _meta: nil
            )
            if let result = try? CallTool.Result(
                content: [textContent],
                structuredContent: report
            ) {
                return result
            }
            return CallTool.Result(content: [textContent])
        }
    )

    // MARK: - Output model

    struct Report: Codable, Sendable {
        let schemaVersion: String
        let platform: String
        let driverVersion: String
        let overall: Overall
        let checks: [CheckEntry]

        private enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case platform
            case driverVersion = "driver_version"
            case overall
            case checks
        }
    }

    enum Overall: String, Codable, Sendable {
        case ok
        case degraded
        case failed
    }

    enum Status: String, Codable, Sendable {
        case pass
        case fail
        case skip
    }

    struct CheckEntry: Codable, Sendable {
        let name: String
        let status: Status
        let message: String
        let hint: String?
        let data: CheckData?
    }

    /// Per-check structured data. Kept as a small, fixed set of optional
    /// fields rather than a free-form dictionary so the JSON shape is
    /// statically known and Codable. Adding new fields here is
    /// non-breaking (consumers ignore unknown fields); removing fields
    /// would break clients and is gated by `schema_version`.
    struct CheckData: Codable, Sendable {
        var bundleIdentifier: String?
        var executablePath: String?
        var osVersion: String?
        var architecture: String?
        var displayCount: Int?
        var errorDetail: String?

        private enum CodingKeys: String, CodingKey {
            case bundleIdentifier = "bundle_identifier"
            case executablePath = "executable_path"
            case osVersion = "os_version"
            case architecture
            case displayCount = "display_count"
            case errorDetail = "error_detail"
        }

        var isEmpty: Bool {
            bundleIdentifier == nil && executablePath == nil
                && osVersion == nil && architecture == nil
                && displayCount == nil && errorDetail == nil
        }
    }

    // MARK: - Check dispatch

    static func runCheck(name: String) async -> CheckEntry {
        switch name {
        case nameBinaryVersion:
            return checkBinaryVersion()
        case namePlatformSupported:
            return checkPlatformSupported()
        case nameSessionActive:
            return checkSessionActive()
        case nameBundleIdentity:
            return checkBundleIdentity()
        case nameTccAccessibility:
            return await checkTccAccessibility()
        case nameTccScreenRecording:
            return await checkTccScreenRecording()
        case nameAxCapability:
            return checkAxCapability()
        case nameScreenCaptureCapability:
            return await checkScreenCaptureCapability()
        default:
            // Should never happen — `selectChecks` only forwards canonical
            // names. Still, surface unknowns rather than crashing.
            return CheckEntry(
                name: name,
                status: .skip,
                message: "Unknown check name (not implemented on this platform).",
                hint: nil,
                data: nil
            )
        }
    }

    // MARK: - Individual checks

    static func checkBinaryVersion() -> CheckEntry {
        // CuaDriverCore.version is a compiled-in constant; failure here
        // would mean a corrupted binary. Always pass.
        return CheckEntry(
            name: nameBinaryVersion,
            status: .pass,
            message: "cua-driver \(CuaDriverCore.version)",
            hint: nil,
            data: nil
        )
    }

    static func checkPlatformSupported() -> CheckEntry {
        // The Swift binary is macOS-only (Package.swift pins .macOS(.v14)),
        // so reaching this code already implies a supported platform.
        let osVersion = ProcessInfo.processInfo.operatingSystemVersionString
        let arch = uname_m()
        return CheckEntry(
            name: namePlatformSupported,
            status: .pass,
            message: "macOS — \(osVersion) (\(arch))",
            hint: nil,
            data: CheckData(
                bundleIdentifier: nil,
                executablePath: nil,
                osVersion: osVersion,
                architecture: arch,
                displayCount: nil,
                errorDetail: nil
            )
        )
    }

    static func checkSessionActive() -> CheckEntry {
        // We are servicing this very MCP call, so by construction the
        // session is up. The check exists so consumers can have a
        // canonical "is the server reachable?" signal in a fixed shape.
        return CheckEntry(
            name: nameSessionActive,
            status: .pass,
            message: "MCP session is active.",
            hint: nil,
            data: nil
        )
    }

    static func checkBundleIdentity() -> CheckEntry {
        let bid = Bundle.main.bundleIdentifier ?? ""
        let exe = Bundle.main.executablePath ?? ""
        // We treat presence of `com.trycua.driver` as the canonical
        // pass — that's the bundle whose TCC grants matter. Anything
        // else (running from a shell-spawned binary, from Xcode test
        // host, etc.) is a `fail` with a hint pointing at the daemon
        // launch path. It's `degraded`, not `failed`, in overall terms.
        let isCorrect = bid == "com.trycua.driver"
        let status: Status = isCorrect ? .pass : .fail
        let message: String
        let hint: String?
        if isCorrect {
            message = "Bundle is com.trycua.driver."
            hint = nil
        } else if bid.isEmpty {
            message = "Process has no CFBundleIdentifier."
            hint = """
                Run the binary inside CuaDriver.app so TCC grants attribute correctly. \
                Start the daemon with `open -n -g -a CuaDriver --args serve` and \
                connect via `cua-driver mcp`.
                """
        } else {
            message = "Bundle is \(bid), not com.trycua.driver."
            hint = """
                TCC grants will be attributed to \(bid), not the cua-driver daemon. \
                Run via `cua-driver mcp` (auto-relaunches inside CuaDriver.app) or \
                start the daemon manually: `open -n -g -a CuaDriver --args serve`.
                """
        }
        return CheckEntry(
            name: nameBundleIdentity,
            status: status,
            message: message,
            hint: hint,
            data: CheckData(
                bundleIdentifier: bid.isEmpty ? nil : bid,
                executablePath: exe.isEmpty ? nil : exe,
                osVersion: nil,
                architecture: nil,
                displayCount: nil,
                errorDetail: nil
            )
        )
    }

    static func checkTccAccessibility() async -> CheckEntry {
        // Reuse the shared probe — do not duplicate AXIsProcessTrusted.
        let status = await Permissions.currentStatus()
        let bid = Bundle.main.bundleIdentifier ?? ""
        if status.accessibility {
            return CheckEntry(
                name: nameTccAccessibility,
                status: .pass,
                message: "Accessibility is granted.",
                hint: nil,
                data: CheckData(
                    bundleIdentifier: bid.isEmpty ? nil : bid,
                    executablePath: nil,
                    osVersion: nil,
                    architecture: nil,
                    displayCount: nil,
                    errorDetail: nil
                )
            )
        }
        return CheckEntry(
            name: nameTccAccessibility,
            status: .fail,
            message: "Accessibility is NOT granted for this process.",
            hint: """
                Grant Accessibility to CuaDriver.app in System Settings → Privacy & Security → \
                Accessibility. If the process bundle is not com.trycua.driver (see bundle_identity), \
                the grant must target the responsible app — restart via `cua-driver mcp` to relaunch \
                inside CuaDriver.app.
                """,
            data: CheckData(
                bundleIdentifier: bid.isEmpty ? nil : bid,
                executablePath: nil,
                osVersion: nil,
                architecture: nil,
                displayCount: nil,
                errorDetail: nil
            )
        )
    }

    static func checkTccScreenRecording() async -> CheckEntry {
        let status = await Permissions.currentStatus()
        let bid = Bundle.main.bundleIdentifier ?? ""
        if status.screenRecording {
            return CheckEntry(
                name: nameTccScreenRecording,
                status: .pass,
                message: "Screen Recording is granted.",
                hint: nil,
                data: CheckData(
                    bundleIdentifier: bid.isEmpty ? nil : bid,
                    executablePath: nil,
                    osVersion: nil,
                    architecture: nil,
                    displayCount: nil,
                    errorDetail: nil
                )
            )
        }
        return CheckEntry(
            name: nameTccScreenRecording,
            status: .fail,
            message: "Screen Recording is NOT granted for this process.",
            hint: """
                Grant Screen Recording to CuaDriver.app in System Settings → Privacy & Security → \
                Screen Recording. The grant is attributed to the responsible process — see \
                bundle_identity to confirm the right binary is being prompted.
                """,
            data: CheckData(
                bundleIdentifier: bid.isEmpty ? nil : bid,
                executablePath: nil,
                osVersion: nil,
                architecture: nil,
                displayCount: nil,
                errorDetail: nil
            )
        )
    }

    static func checkAxCapability() -> CheckEntry {
        // The AX trust gate is the same kAXIsProcessTrusted probe
        // `tcc_accessibility` runs, but ax_capability is the consumer-
        // facing "can I actually drive UI" signal. We separate them so
        // a future change (e.g. capability degraded without TCC denial)
        // doesn't break either contract.
        let trusted = AXIsProcessTrusted()
        if trusted {
            return CheckEntry(
                name: nameAxCapability,
                status: .pass,
                message: "AX is trusted and reachable.",
                hint: nil,
                data: nil
            )
        }
        return CheckEntry(
            name: nameAxCapability,
            status: .fail,
            message: "AX is not trusted; UI inspection and event posting will fail.",
            hint: """
                Resolve tcc_accessibility first — AX capability follows directly from the \
                Accessibility TCC grant.
                """,
            data: nil
        )
    }

    static func checkScreenCaptureCapability() async -> CheckEntry {
        // Live ScreenCaptureKit probe: enumerate shareable content.
        // Nothing hits disk; we never start a stream. This is the same
        // probe `Permissions.probeScreenRecording` uses but explicit
        // about being a capability check, not a TCC-grant check.
        // Surfaces the actual display count so consumers can detect a
        // headless / no-display situation distinct from a denied grant.
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(
                false, onScreenWindowsOnly: true
            )
            let count = content.displays.count
            return CheckEntry(
                name: nameScreenCaptureCapability,
                status: .pass,
                message: "ScreenCaptureKit reachable; \(count) display(s) shareable.",
                hint: nil,
                data: CheckData(
                    bundleIdentifier: nil,
                    executablePath: nil,
                    osVersion: nil,
                    architecture: nil,
                    displayCount: count,
                    errorDetail: nil
                )
            )
        } catch {
            return CheckEntry(
                name: nameScreenCaptureCapability,
                status: .fail,
                message: "ScreenCaptureKit probe failed.",
                hint: """
                    Confirm tcc_screen_recording is granted. If it is, this is a SCK regression \
                    (see #1467) — set capture_mode to `ax` to skip screen capture entirely.
                    """,
                data: CheckData(
                    bundleIdentifier: nil,
                    executablePath: nil,
                    osVersion: nil,
                    architecture: nil,
                    displayCount: nil,
                    errorDetail: String(describing: error)
                )
            )
        }
    }

    // MARK: - Overall rollup

    static func computeOverall(checks: [CheckEntry]) -> Overall {
        var anyFail = false
        var anyCoreFail = false
        for entry in checks {
            guard entry.status == .fail else { continue }
            anyFail = true
            if coreChecks.contains(entry.name) {
                anyCoreFail = true
            }
        }
        if anyCoreFail { return .failed }
        if anyFail { return .degraded }
        return .ok
    }

    // MARK: - Argument parsing helpers

    /// Pull a `Set<String>` out of an MCP `Value` array argument. Missing
    /// / nil / non-array values yield an empty set, treated as "no
    /// filter applied" by `selectChecks`.
    static func parseStringSet(_ value: Value?) -> Set<String> {
        guard let value, case let .array(items) = value else { return [] }
        var out: Set<String> = []
        for item in items {
            if case let .string(s) = item { out.insert(s) }
        }
        return out
    }

    /// Decide which checks to actually run. Rules:
    ///   - `include` wins if non-empty: run exactly the canonical names
    ///     that intersect `include`. Unknown names in `include` are
    ///     silently ignored (forward-compat — consumer may know a name
    ///     from a newer driver build).
    ///   - else `skip` filters out from the full canonical list.
    ///   - else run everything.
    static func selectChecks(
        allChecks: [String], include: Set<String>, skip: Set<String>
    ) -> Set<String> {
        if !include.isEmpty {
            return Set(allChecks).intersection(include)
        }
        if !skip.isEmpty {
            return Set(allChecks).subtracting(skip)
        }
        return Set(allChecks)
    }

    // MARK: - Text summary

    /// Compact human-readable summary, mirroring the doctor command's
    /// vibe but driven off the structured output. The structuredContent
    /// payload is the authoritative format; this text content is for
    /// humans squinting at MCP traces.
    static func textSummary(report: Report) -> String {
        var lines: [String] = []
        let overallIcon: String
        switch report.overall {
        case .ok: overallIcon = "✅"
        case .degraded: overallIcon = "⚠️"
        case .failed: overallIcon = "❌"
        }
        lines.append(
            "\(overallIcon) cua-driver \(report.driverVersion) on \(report.platform) — \(report.overall.rawValue)"
        )
        for entry in report.checks {
            let icon: String
            switch entry.status {
            case .pass: icon = "✅"
            case .fail: icon = "❌"
            case .skip: icon = "⏭"
            }
            lines.append("  \(icon) \(entry.name): \(entry.message)")
        }
        return lines.joined(separator: "\n")
    }

    // MARK: - Misc

    private static func uname_m() -> String {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { bytes in
            let str = bytes.bindMemory(to: CChar.self)
            return String(cString: str.baseAddress!)
        }
    }
}
