// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing

/// No macOS permission prompt before the first run's permissions step: only
/// PermissionRequester.swift may call an API that raises one. Status checks
/// elsewhere use the preflight calls (CGPreflightScreenCaptureAccess,
/// AXIsProcessTrusted, notificationSettings).
@Suite("Permission prompts come from one file")
struct PermissionPromptGuardTests {
    static let prompting = [
        "CGRequestScreenCaptureAccess", "CGRequestListenEventAccess", "CGRequestPostEventAccess",
        "AXIsProcessTrustedWithOptions", "kAXTrustedCheckOptionPrompt", "AXTrustedCheckOptionPrompt",
        "requestAuthorization(", "SCShareableContent", "SCScreenshotManager", "CGWindowListCreateImage",
        "CGDisplayCreateImage", "CGDisplayStream", "AVCaptureDevice.requestAccess",
        "CLLocationManager", "NWBrowser", "NetServiceBrowser", "NSAppleScript",
        "AEDeterminePermissionToAutomateTarget", "CNContactStore", "EKEventStore", "PHPhotoLibrary",
    ]

    @Test("prompting APIs appear only in PermissionRequester.swift")
    func onlyTheRequesterPrompts() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources")
        let files = try #require(FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil))
        var offences: [String] = []
        for case let url as URL in files where url.pathExtension == "swift"
            && url.lastPathComponent != "PermissionRequester.swift" {
            let text = try String(contentsOf: url, encoding: .utf8)
            for line in text.split(separator: "\n") where !line.trimmingCharacters(in: .whitespaces).hasPrefix("//") {
                for api in Self.prompting where line.contains(api) {
                    offences.append("\(url.lastPathComponent): \(api)")
                }
            }
        }
        #expect(offences.isEmpty, "move these into PermissionRequester: \(offences)")
    }
}
