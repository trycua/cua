import AppKit
import ArgumentParser
import CuaDriverCore
import Foundation
import ScreenCaptureKit

/// `cua-driver doctor` — probe TCC / SCK / AX and print a recommendation.
///
/// Unlike `diagnose` (which emits a raw paste-able block for support),
/// `doctor` interprets the probe results and recommends a concrete next
/// step. Use it to quickly discover why captures are failing and which
/// `capture_mode` to set.
struct DoctorCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "doctor",
        abstract: "Check Accessibility, Screen Recording, and SCK; recommend a capture mode."
    )

    @Flag(name: .long, help: "Emit machine-readable JSON instead of human text.")
    var json: Bool = false

    func run() async throws {
        let result = await runProbes()

        if json {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            if let data = try? encoder.encode(result),
               let str = String(data: data, encoding: .utf8)
            {
                print(str)
            }
        } else {
            print(result.formatted())
        }

        if !result.allOk {
            throw ExitCode(1)
        }
    }

    // MARK: - Probe runner

    private func runProbes() async -> DoctorResult {
        // 1. TCC / permission probes.
        let axOk = AXIsProcessTrusted()
        let sckOk = await probeSCK()

        // 2. Attribution check — are we attributed to CuaDriver.app or a shell?
        let bundleID = Bundle.main.bundleIdentifier ?? ""
        let isCorrectBundle = bundleID == "com.trycua.driver"

        // 3. AX tree smoke test on Finder.
        let finderPid = finderPID()
        let axTreeOk: Bool
        if axOk, let pid = finderPid {
            axTreeOk = probeAXTree(pid: pid)
        } else {
            axTreeOk = false
        }

        // 4. Environment info.
        let arch = uname_m()
        let osVersion = ProcessInfo.processInfo.operatingSystemVersionString
        let locale = Locale.current.identifier

        // 5. Derive recommendation.
        let recommendation = recommend(
            axOk: axOk, sckOk: sckOk, isCorrectBundle: isCorrectBundle)

        return DoctorResult(
            axGranted: axOk,
            screenRecordingGranted: sckOk,
            correctBundleAttribution: isCorrectBundle,
            axTreeSmoke: axTreeOk,
            arch: arch,
            osVersion: osVersion,
            locale: locale,
            bundleID: bundleID.isEmpty ? nil : bundleID,
            recommendation: recommendation
        )
    }

    // MARK: - Individual probes

    /// Check SCK by enumerating shareable content. Cheap — no stream is
    /// started. Returns false if SCK is denied or throws (Tahoe regression).
    private func probeSCK() async -> Bool {
        do {
            _ = try await SCShareableContent.excludingDesktopWindows(
                false, onScreenWindowsOnly: false)
            return true
        } catch {
            return false
        }
    }

    /// Fetch the top-level AX children of `pid`. Returns true if we get
    /// at least one element without an error — sufficient to confirm AX
    /// round-trips are working.
    private func probeAXTree(pid: pid_t) -> Bool {
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(
            app, kAXChildrenAttribute as CFString, &value)
        return err == .success
    }

    /// PID of the running Finder process, or nil.
    private func finderPID() -> pid_t? {
        NSWorkspace.shared.runningApplications
            .first { $0.bundleIdentifier == "com.apple.finder" }
            .map { $0.processIdentifier }
    }

    private func uname_m() -> String {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { bytes in
            let str = bytes.bindMemory(to: CChar.self)
            return String(cString: str.baseAddress!)
        }
    }

    // MARK: - Recommendation logic

    private func recommend(
        axOk: Bool, sckOk: Bool, isCorrectBundle: Bool
    ) -> Recommendation {
        if !axOk {
            return Recommendation(
                captureMode: nil,
                severity: .error,
                summary: "Accessibility is denied.",
                detail: """
                    Grant Accessibility to CuaDriver.app in System Settings → Privacy & Security → Accessibility, then restart the daemon:
                      open -n -g -a CuaDriver --args serve
                    If you're running `cua-driver mcp` from a terminal, v0.1.7+ auto-relaunches through CuaDriver.app — make sure you're on the latest release.
                    """
            )
        }

        if !isCorrectBundle {
            return Recommendation(
                captureMode: nil,
                severity: .warning,
                summary: "TCC is attributed to the wrong process (not CuaDriver.app).",
                detail: """
                    Your shell or IDE is the responsible process for TCC, not CuaDriver.app.
                    Run `cua-driver mcp` v0.1.7+ — it auto-relaunches through CuaDriver.app.
                    Or start the daemon manually: open -n -g -a CuaDriver --args serve
                    """
            )
        }

        if sckOk {
            return Recommendation(
                captureMode: "som",
                severity: .ok,
                summary: "All probes passed. Default `capture_mode: som` (or `vision`) recommended.",
                detail: nil
            )
        } else {
            return Recommendation(
                captureMode: "ax",
                severity: .warning,
                summary: "ScreenCaptureKit is unavailable on this build.",
                detail: """
                    This is a known regression on some macOS builds (see #1467).
                    Workaround: set capture_mode to `ax`:
                      cua-driver config set capture_mode ax
                    AX mode skips screen capture entirely and relies solely on the Accessibility tree.
                    """
            )
        }
    }
}

// MARK: - Result types

struct DoctorResult: Encodable {
    let axGranted: Bool
    let screenRecordingGranted: Bool
    let correctBundleAttribution: Bool
    let axTreeSmoke: Bool
    let arch: String
    let osVersion: String
    let locale: String
    let bundleID: String?
    let recommendation: Recommendation

    var allOk: Bool { recommendation.severity == .ok }

    func formatted() -> String {
        let tick = "✅"
        let warn = "⚠️ "
        let fail = "❌"

        func icon(_ ok: Bool) -> String { ok ? tick : fail }

        var lines: [String] = ["── cua-driver doctor ──────────────────────"]
        lines.append("")
        lines.append("System")
        lines.append("  arch:       \(arch)")
        lines.append("  os:         \(osVersion)")
        lines.append("  locale:     \(locale)")
        if let bid = bundleID {
            lines.append("  bundle:     \(bid)")
        }
        lines.append("")
        lines.append("Probes")
        lines.append("  \(icon(axGranted)) Accessibility (AXIsProcessTrusted)")
        lines.append("  \(icon(screenRecordingGranted)) Screen Recording (SCShareableContent)")
        lines.append("  \(icon(correctBundleAttribution)) Correct bundle attribution")
        lines.append("  \(icon(axTreeSmoke)) AX tree smoke test (Finder)")
        lines.append("")
        lines.append("Recommendation")
        let sevIcon: String
        switch recommendation.severity {
        case .ok:      sevIcon = tick
        case .warning: sevIcon = warn
        case .error:   sevIcon = fail
        }
        lines.append("  \(sevIcon) \(recommendation.summary)")
        if let mode = recommendation.captureMode {
            lines.append("  capture_mode: \(mode)")
        }
        if let detail = recommendation.detail {
            lines.append("")
            for line in detail.split(separator: "\n", omittingEmptySubsequences: false) {
                lines.append("  \(line)")
            }
        }
        lines.append("")
        lines.append("────────────────────────────────────────────")
        return lines.joined(separator: "\n")
    }

    private enum CodingKeys: String, CodingKey {
        case axGranted = "ax_granted"
        case screenRecordingGranted = "screen_recording_granted"
        case correctBundleAttribution = "correct_bundle_attribution"
        case axTreeSmoke = "ax_tree_smoke"
        case arch, osVersion = "os_version", locale
        case bundleID = "bundle_id"
        case recommendation
    }
}

struct Recommendation: Encodable {
    enum Severity: String, Encodable, Equatable { case ok, warning, error }

    let captureMode: String?
    let severity: Severity
    let summary: String
    let detail: String?

    private enum CodingKeys: String, CodingKey {
        case captureMode = "capture_mode"
        case severity, summary, detail
    }
}
