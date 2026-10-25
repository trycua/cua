// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import PermissionFlow
import Testing

/// `host.openSettings` guides Screen Recording and Accessibility (the panes
/// the host's permission hints and the first run's rows name); any other
/// pane just opens.
struct PermissionGuideTests {
    @Test func guidesTheTwoPanesASharedScreenNeeds() {
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") == .screenRecording)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") == .accessibility)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") == nil)
        #expect(PermissionGuide.pane(for: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork") == nil)
    }
}
