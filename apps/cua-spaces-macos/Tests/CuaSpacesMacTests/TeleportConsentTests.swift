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
            cookieDomains: nil, exclude: [], fromVault: nil))
        #expect(c == TeleportConsent(approved: true, acknowledgeSensitive: true,
                                     saveToKeyvault: true, acknowledgeRelayPlaintext: true,
            cookieDomains: nil, exclude: [], fromVault: nil))
        let off = sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true,
            saveToKeyvault: false, acknowledgeRelayPlaintext: false,
            cookieDomains: nil, exclude: [], fromVault: nil))
        #expect(!off.saveToKeyvault)
    }
}
