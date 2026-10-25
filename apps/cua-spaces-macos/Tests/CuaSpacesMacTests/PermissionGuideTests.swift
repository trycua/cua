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

    @Test func offersTheOtherPermissionNextWhileItIsMissing() {
        #expect(PermissionGuide.next(after: .screenRecording, isGranted: { _ in false }) == .guide(.accessibility))
        #expect(PermissionGuide.next(after: .accessibility, isGranted: { _ in false }) == .guide(.screenRecording))
    }

    @Test func finishesWhenTheOtherPermissionIsAlreadyGranted() {
        #expect(PermissionGuide.next(after: .screenRecording, isGranted: { $0 == .accessibility }) == .finish)
        #expect(PermissionGuide.next(after: .accessibility, isGranted: { $0 == .screenRecording }) == .finish)
    }

    @Test func onlyScreenRecordingNeedsAReopen() {
        #expect(PermissionGuide.needsReopen(after: .screenRecording))
        #expect(!PermissionGuide.needsReopen(after: .accessibility))
    }

    @Test func everyGuidedPaneHasItsSettingsURL() {
        for pane in PermissionGuide.guided {
            let url = PermissionGuide.settingsURL(for: pane)
            #expect(url.flatMap(PermissionGuide.pane(for:)) == pane)
        }
    }

    @Test func readsWhatAFreshProcessReports() {
        #expect(PermissionGuide.parseGrants("screen-recording=1 accessibility=0\n") == [.screenRecording])
        #expect(PermissionGuide.parseGrants("screen-recording=0 accessibility=1") == [.accessibility])
        #expect(PermissionGuide.parseGrants("screen-recording=1 accessibility=1") == [.screenRecording, .accessibility])
        #expect(PermissionGuide.parseGrants("").isEmpty)
        #expect(PermissionGuide.parseGrants("garbage=1").isEmpty)
    }
}
