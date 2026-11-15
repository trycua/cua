// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import ApplicationServices
import CoreGraphics
import Foundation
import UserNotifications

/// The only place in the app that may raise a macOS permission prompt
/// (Screen Recording, Accessibility, Notifications). Everything else reads
/// status with the non-prompting preflight calls. PermissionPromptGuardTests
/// fails the build when a prompting API appears in any other file.
///
/// Before the first run's permissions step nothing may prompt: macOS's bare
/// popup would appear over the sign-in, ahead of the app's own guide.
public enum PermissionRequester {
    /// Set by the app: notifications may be asked for once the first run is done.
    @MainActor public static var firstRunDone: () -> Bool = { false }

    /// Screen Recording, shown without asking.
    public static func screenRecordingGranted() -> Bool { CGPreflightScreenCaptureAccess() }

    /// Accessibility, shown without asking.
    public static func accessibilityGranted() -> Bool { AXIsProcessTrusted() }

    /// macOS's Screen Recording and Accessibility prompts, for a host set up
    /// without the app ever showing (ServiceLauncher). Never call this from
    /// the app's UI process before its permissions step.
    public static func requestScreenRecordingAndAccessibility() {
        if !CGPreflightScreenCaptureAccess() { _ = CGRequestScreenCaptureAccess() }
        let prompt = "AXTrustedCheckOptionPrompt" // kAXTrustedCheckOptionPrompt
        _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
    }

    /// Whether a notification may be posted. Asks macOS only when the first
    /// run is done; before that an undecided answer is "no", without a prompt.
    @MainActor
    public static func notificationsAuthorized(_ center: UNUserNotificationCenter) async -> Bool {
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral: return true
        case .denied: return false
        default:
            guard firstRunDone() else { return false }
            return (try? await center.requestAuthorization(options: [.alert, .sound])) ?? false
        }
    }
}
