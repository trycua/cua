// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Testing

/// Regression: the review's "Save to Keyvault" checkbox reached the core
/// consent but was dropped when building the SDK consent, so a teleport
/// with it checked never stored anything in the Keyvault.
@Suite("Teleport consent")
struct TeleportConsentTests {
    @Test func saveToKeyvaultReachesTheSdkConsent() {
        let c = sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true,
            saveToKeyvault: true, acknowledgeRelayPlaintext: true,
            cookieDomains: nil, exclude: [], fromVault: nil, includePasswords: false))
        #expect(c == TeleportConsent(approved: true, acknowledgeSensitive: true,
                                     saveToKeyvault: true, acknowledgeRelayPlaintext: true))
        let off = sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true,
            saveToKeyvault: false, acknowledgeRelayPlaintext: false,
            cookieDomains: nil, exclude: [], fromVault: nil, includePasswords: false))
        #expect(!off.saveToKeyvault)
    }

    /// The review's choices must reach the SDK consent, or the review would
    /// show a selection the teleport then ignores: the sites, the lines
    /// turned off, and the Keyvault as the source.
    @Test func theReviewsChoicesReachTheSdkConsent() {
        let c = sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true, saveToKeyvault: false, acknowledgeRelayPlaintext: false,
            cookieDomains: ["github.com", "notion.so"], exclude: ["Default/Bookmarks"], fromVault: nil, includePasswords: false))
        #expect(c.cookieDomains == ["github.com", "notion.so"])
        #expect(c.exclude == ["Default/Bookmarks"])
        #expect(c.fromVault == nil)
        let v = sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true, saveToKeyvault: false, acknowledgeRelayPlaintext: false,
            cookieDomains: nil, exclude: [], fromVault: ["a", "b"], includePasswords: true))
        #expect(v.fromVault == ["a", "b"] && v.cookieDomains == nil && v.includePasswords)
    }
}
