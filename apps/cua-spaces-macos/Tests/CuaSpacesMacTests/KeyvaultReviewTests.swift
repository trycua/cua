// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import SwiftUI
import Testing

/// The teleport review as the vault list again: a browser's sites with
/// counts and a minimal default, what was picked last time, the Keyvault as
/// the source, and the choices reaching the SDK consent.
@MainActor
@Suite("Keyvault review", .serialized)
struct KeyvaultReviewTests {
    static let list = CGSize(width: 780, height: 560)

    func model(_ overview: KeyvaultOverview = KeyvaultFixtures.overview()) async -> (AppModel, FakeKeyvault) {
        let fake = FakeKeyvault(overview)
        let m = ViewModelTests().makeModel(kv: fake)
        await m.keyvault.refresh()
        return (m, fake)
    }

    static let chromePlan = """
    {"app":{"id":"com.google.Chrome","name":"Google Chrome","capability":"full","moves":["app_only","app_with_state"],
            "providerId":"chrome","sensitiveGroups":["sign_ins"],"json":"{}"},
     "spaceId":"local:aurora","moves":"app_with_state","steps":[],
     "consent":[
       {"kind":"install","key":"chrome","label":"Google Chrome","detail":"pinned, verified by sha256 0cfabe56","bytes":0,"sensitive":false},
       {"kind":"state","key":"Default/Bookmarks","label":"Bookmarks","detail":"212 bookmarks","bytes":184320,"sensitive":false},
       {"kind":"state","key":"Default/Preferences","label":"Preferences","detail":"Settings and extensions","bytes":22016,"sensitive":false},
       {"kind":"secret","key":".config/google-chrome/Default/Cookies","label":"Cookies","detail":"Keeps you signed in","bytes":1228800,"sensitive":true}],
     "sensitive":true,"totalBytes":1435136,"warnings":[],"relayUnsealed":false,"json":"{}"}
    """

    /// A review of Chrome's state on its way to Aurora, as the picker reaches
    /// it: planned, with the sites and the saved items asked for.
    /// `features`: what the fake Keyvault daemon says it supports (none:
    /// one that predates the list).
    func reviewModel(vault: Bool = false, features: [String] = []) async throws -> (TeleportModel, FakeKeyvault) {
        let (m, fake) = await model()
        fake.current.status?.features = features
        let t = TeleportModel(spaceName: "aurora", teleport: nil, space: nil)
        t.keyvault = m.keyvault
        let entry = try #require(try appCatalogEntriesFromJson(json: """
        [{"id":"com.google.Chrome","name":"Google Chrome","capability":"full","moves":["app_only","app_with_state"],
          "providerId":"chrome","sensitiveGroups":["sign_ins"],"json":"{}"}]
        """).first)
        t.send(.preselect(entry: entry, files: []))
        t.send(.move(moves: .appWithState))
        t.send(.sensitive(group: .signIns, value: true))
        t.send(.plan)
        t.send(.planned(plan: try appTeleportPlanFromJson(json: Self.chromePlan)))
        await t.loadChoices()
        t.send(.acknowledge(value: true))
        if vault {
            await t.sendFrom(.vault)
            t.sendVault(.toggleOpen(key: "chrome|github.com"))
        }
        return (t, fake)
    }

    @Test func reviewListsSitesWithCountsAndStartsMinimal() async throws {
        let (t, fake) = try await reviewModel()
        let review = try #require(t.review)
        #expect(fake.inventoryAsks == 1)
        #expect(review.offersDomains && !review.needsDomains)
        #expect(review.domainSummary == "5 of 9 sites", "sign-in sites, never the identity provider")
        #expect(!review.selectedDomains.contains("google.com") && !review.selectedDomains.contains("doubleclick.net"))
        #expect(review.domains.first { $0.domain == "github.com" }?.counts == "12 cookies, 3 storage values, 2 passwords")
        #expect(review.offersVault && review.vault.items == 16, "Chrome has 16 saved items a teleport can send (not its 2 passwords)")
        #expect(review.source == .live)
        let consent = appPickerConsent(state: t.state)
        #expect(consent.cookieDomains == review.selectedDomains && consent.fromVault == nil)
    }

    @Test func cookiesTheBrowserCannotReleaseAreGreyedWithWhyAndNeverSent() async throws {
        let (t, _) = try await reviewModel()
        let bank = try #require(t.review?.domains.first { $0.domain == "bank.example" })
        #expect(!bank.selectable && bank.unavailable == 3)
        #expect(bank.unavailableNote.hasPrefix("3 cookies cannot be sent: Chrome protects it with app-bound encryption"))
        // It starts unchecked and a tap does nothing.
        #expect(!bank.selected)
        t.send(.toggleDomain(domain: "bank.example"))
        #expect(appPickerConsent(state: t.state).cookieDomains?.contains("bank.example") == false)
        t.send(.selectShownDomains(value: true))
        #expect(appPickerConsent(state: t.state).cookieDomains?.contains("bank.example") == false)
    }

    @Test func savedPasswordsAreAnExplicitOffByDefaultChoice() async throws {
        // From the app: the chosen sites' passwords (github.com has 2).
        let (t, _) = try await reviewModel()
        let review = try #require(t.review)
        #expect(review.offersPasswords && !review.includePasswords)
        #expect(review.passwordsLabel == "Also send 2 saved passwords")
        #expect(!appPickerConsent(state: t.state).includePasswords)
        t.send(.togglePasswords(value: true))
        #expect(appPickerConsent(state: t.state).includePasswords)
        #expect(sdkConsent(appPickerConsent(state: t.state)).includePasswords, "carried over to the SDK consent")
        // This daemon predates the per-site choice: nothing to pick, and no
        // narrower choice is sent for it to ignore.
        #expect(t.review?.passwords.isEmpty == true && t.review?.passwordSummary == "")
        #expect(appPickerConsent(state: t.state).passwordDomains == nil)
        // Dropping the only site that has passwords takes the offer away.
        t.send(.toggleDomain(domain: "github.com"))
        #expect(t.review?.offersPasswords == false)
        #expect(!appPickerConsent(state: t.state).includePasswords)
        // From the Keyvault: its saved passwords, only when ticked.
        let (v, _) = try await reviewModel(vault: true)
        #expect(v.review?.passwordsLabel == "Also send 2 saved passwords")
        #expect(appPickerConsent(state: v.state).fromVault?.count == 16)
        v.send(.togglePasswords(value: true))
        #expect(appPickerConsent(state: v.state).fromVault?.count == 18)
    }

    /// A daemon that advertises `password_domains`: ticked, the passwords
    /// are listed by site to pick (CUA-1641), starting from the sites kept
    /// signed in; never a value.
    @Test func savedPasswordsCanBePickedWhereTheKeyvaultKeepsTheChoice() async throws {
        let (t, _) = try await reviewModel(features: ["password_domains"])
        t.send(.togglePasswords(value: true))
        #expect(t.review?.passwords.map(\.site) == ["github.com"])
        #expect(appPickerConsent(state: t.state).passwordDomains == ["github.com"])
        #expect(sdkConsent(appPickerConsent(state: t.state)).passwordDomains == ["github.com"])
        // The password choice is its own: dropping the site's cookies keeps it.
        t.send(.toggleDomain(domain: "github.com"))
        #expect(appPickerConsent(state: t.state).passwordDomains == ["github.com"])
        // Dropping it from the passwords sends none.
        t.send(.togglePassword(key: "github.com"))
        #expect(!appPickerConsent(state: t.state).includePasswords)
        // From the Keyvault: its saved passwords, only when ticked.
        let (v, _) = try await reviewModel(vault: true)
        #expect(v.review?.passwordsLabel == "Also send 2 saved passwords")
        #expect(appPickerConsent(state: v.state).fromVault?.count == 16)
        v.send(.togglePasswords(value: true))
        #expect(appPickerConsent(state: v.state).fromVault?.count == 18)
    }

    @Test func reviewCanSendFromTheSavedItemsWithoutReadingTheApp() async throws {
        let (t, _) = try await reviewModel(vault: true)
        let review = try #require(t.review)
        #expect(review.source == .vault)
        #expect(review.sourceNote == "Nothing is read from this Mac, so macOS does not ask for Keychain access.")
        let consent = appPickerConsent(state: t.state)
        #expect(consent.cookieDomains == nil)
        #expect(consent.fromVault?.count == 16)
        // Deselect a site's items: only the rest are sent.
        t.sendVault(.toggleGroup(key: "chrome|github.com"))
        #expect(appPickerConsent(state: t.state).fromVault?.count == 11, "github.com has 5 items a teleport can send")
        t.sendVault(.toggleGroup(key: "chrome|github.com"))
    }

    @Test func aFailedSiteReadFallsBackToThePlanAsListed() async throws {
        let (m, fake) = await model()
        fake.inventoryFails = true
        let t = TeleportModel(spaceName: "aurora", teleport: nil, space: nil)
        t.keyvault = m.keyvault
        let entry = try #require(try appCatalogEntriesFromJson(json: """
        [{"id":"com.google.Chrome","name":"Google Chrome","capability":"full","moves":["app_only","app_with_state"],
          "providerId":"chrome","sensitiveGroups":["sign_ins"],"json":"{}"}]
        """).first)
        t.send(.preselect(entry: entry, files: []))
        t.send(.move(moves: .appWithState))
        t.send(.plan)
        t.send(.planned(plan: try appTeleportPlanFromJson(json: Self.chromePlan)))
        await t.loadChoices()
        let review = try #require(t.review)
        #expect(!review.needsDomains && review.domains.isEmpty)
        #expect(appPickerConsent(state: t.state).cookieDomains == nil, "sent exactly as the plan lists it")
        #expect(review.toggles.contains { $0.key.hasSuffix("Cookies") }, "the cookie line is a plain line")
    }

    @Test func whatWasPickedLastTimeStartsTheNextReview() async throws {
        let (m, _) = await model()
        let t = TeleportModel(spaceName: "aurora", teleport: nil, space: nil)
        t.keyvault = m.keyvault
        t.rememberedChoices = appReviewRemember(choices: [], key: appReviewRememberKey(app: "chrome", space: "aurora"),
                                                domains: ["doubleclick.net", "nytimes.com"])
        let entry = try #require(try appCatalogEntriesFromJson(json: """
        [{"id":"com.google.Chrome","name":"Google Chrome","capability":"full","moves":["app_only","app_with_state"],
          "providerId":"chrome","sensitiveGroups":["sign_ins"],"json":"{}"}]
        """).first)
        t.send(.preselect(entry: entry, files: []))
        t.send(.move(moves: .appWithState))
        t.send(.plan)
        t.send(.planned(plan: try appTeleportPlanFromJson(json: Self.chromePlan)))
        await t.loadChoices()
        #expect(t.review?.selectedDomains == ["doubleclick.net", "nytimes.com"])
        var saved: [AppRememberedChoice] = []
        t.onRemember = { saved = $0 }
        t.send(.toggleDomain(domain: "github.com"))
        t.rememberChoice()
        #expect(saved.first?.domains == ["doubleclick.net", "github.com", "nytimes.com"])
    }

    // MARK: - Exported screenshots and GIFs (KEYVAULT_EXPORT_DIR)

}
