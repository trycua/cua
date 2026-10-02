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
/// the source, and the choices reaching the SDK consent. With
/// `KEYVAULT_EXPORT_DIR` set, the screenshots and GIFs of the redesign are
/// exported too (see KeyvaultSnapshotTests).
@MainActor
@Suite("Keyvault review", .serialized)
struct KeyvaultReviewTests {
    let snap = SnapshotTests()
    static let list = CGSize(width: 780, height: 560)

    func model(_ overview: KeyvaultOverview = KeyvaultFixtures.overview(),
               icons: Bool = false) async -> (AppModel, FakeKeyvault) {
        await KeyvaultSnapshotTests().model(overview, icons: icons)
    }

    var useRealIcons: Bool { ProcessInfo.processInfo.environment["KEYVAULT_REAL_ICONS"] == "1" }

    func vault(_ m: AppModel) -> some View {
        VaultList(keyvault: m.keyvault, page: m.keyvault.page)
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
    func reviewModel(vault: Bool = false) async throws -> (TeleportModel, FakeKeyvault) {
        let (m, fake) = await model(icons: useRealIcons)
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
        try snap.assertSnapshot(TeleportReview(teleport: t, review: review), "keyvault-review-sites",
                                size: CGSize(width: 640, height: 760))
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
        try snap.assertSnapshot(TeleportReview(teleport: t, review: try #require(t.review)),
                                "keyvault-review-vault", size: CGSize(width: 640, height: 760))
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

    @Test func exportScreenshotsAndGifs() async throws {
        guard let dir = ProcessInfo.processInfo.environment["KEYVAULT_EXPORT_DIR"], !dir.isEmpty else { return }
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let size = Self.list
        let recorder = Recorder(snap: snap, size: size, dir: out)

        // Unlock a batch: select a site, Unlock, the prompt, Allow.
        do {
            let (m, _) = await model(icons: true)
            m.keyvault.send(.toggleOpen(key: "chrome|github.com"))
            recorder.frame("Vault list: apps, sites, items. Locked by default.", vault(m), delay: 1.4)
            m.keyvault.send(.toggleGroup(key: "chrome|github.com"))
            recorder.frame("Select a site: its 6 items are selected.", vault(m), delay: 1.4)
            let ids = m.keyvault.vaultView.selection.unlockIds
            await m.keyvault.requestUnlock(ids: ids)
            let prompt = try #require(m.keyvault.unlockPrompt?.prompt)
            recorder.frame("Unlock asks first, once for the whole batch.", vault(m), over: UnlockPromptSheet(prompt: prompt) { _ in },
                           sheetSize: CGSize(width: 440, height: 220), delay: 2.4)
            await m.keyvault.answerUnlock(.allow)
            recorder.frame("Allow: Touch ID is asked once, then the items are unlocked.", vault(m), delay: 2.0)
            try recorder.finish("keyvault-unlock-batch")
        }
        // Search filters by domain, key, app or type.
        do {
            let (m, _) = await model(icons: true)
            for q in ["", "n", "no", "not", "notion", "cookie github", "slack"] {
                m.keyvault.query = q
                recorder.frame(q.isEmpty ? "Search by domain, key, app or type." : "Search: \(q)", vault(m), delay: q.isEmpty ? 1.2 : 1.0)
            }
            try recorder.finish("keyvault-search")
        }
        // Delete wipes live copies too.
        do {
            var o = KeyvaultFixtures.overview()
            o.deliveries = [KvDelivery(importId: "imp-1", target: "local:aurora", providerId: "chrome",
                                       items: ["c-gh-1", "c-gh-2"], callerFp: "fp", deliveredMs: UInt64(KeyvaultFixtures.now - 600_000),
                                       expiresMs: 0, wiped: false)]
            let (m, _) = await model(o, icons: true)
            m.keyvault.send(.toggleGroup(key: "arc|figma.com"))
            recorder.frame("Select Figma's cookies in Arc.", vault(m), delay: 1.2)
            m.keyvault.send(.clear)
            m.keyvault.send(.toggleGroup(key: "chrome|github.com"))
            recorder.frame("Select github.com in Chrome and delete it.", vault(m), delay: 1.2)
            m.keyvault.requestDelete(ids: m.keyvault.vaultView.selection.ids)
            let c = try #require(m.keyvault.deleteConfirm?.confirm)
            recorder.frame("The confirmation says live copies are wiped too.", vault(m),
                           over: DeleteConfirmSheet(confirm: c, onDelete: {}, onCancel: {}),
                           sheetSize: CGSize(width: 420, height: 170), delay: 2.4)
            await m.keyvault.confirmDelete()
            recorder.frame("Deleted. The copy in Aurora was wiped.", vault(m), delay: 2.0)
            try recorder.finish("keyvault-delete")
        }
        // The review: per-site choice, counts, then the Keyvault as the source.
        do {
            let rec = Recorder(snap: snap, size: CGSize(width: 640, height: 760), dir: out)
            let (t, _) = try await reviewModel()
            func review() -> some View { TeleportReview(teleport: t, review: t.review!) }
            rec.frame("Review: the sites to send, with counts. Minimal by default.", review(), delay: 2.2)
            t.send(.toggleDomain(domain: "amazon.com"))
            rec.frame("Pick another site.", review(), delay: 1.4)
            t.send(.toggleDomain(domain: "github.com"))
            rec.frame("Drop one.", review(), delay: 1.4)
            t.send(.domainQuery(text: "n"))
            rec.frame("Search the sites.", review(), delay: 1.4)
            t.send(.domainQuery(text: ""))
            await t.sendFrom(.vault)
            t.sendVault(.toggleOpen(key: "chrome|github.com"))
            rec.frame("Or send the saved Keyvault items. Nothing is read from this Mac.", review(), delay: 2.6)
            try rec.finish("keyvault-review")
        }
        // Stills.
        do {
            let (m, _) = await model(icons: true)
            m.keyvault.send(.toggleOpen(key: "chrome|github.com"))
            m.keyvault.send(.toggleOpen(key: "chrome|\u{1}files"))
            try recorder.still("vault-grouped", vault(m))
            m.keyvault.send(.toggleGroup(key: "chrome|github.com"))
            m.keyvault.send(.toggle(id: "c-no-1"))
            try recorder.still("vault-multiselect", vault(m))
            m.keyvault.send(.clear)
            m.keyvault.query = "notion"
            try recorder.still("vault-search", vault(m))
            let (h, _) = await model(KeyvaultFixtures.overview(namesVisible: false), icons: true)
            try recorder.still("vault-names-hidden", vault(h))
            let one = kvUnlockPromptAlways(count: 1, name: "user_session")
            try recorder.still("unlock-prompt", UnlockPromptSheet(prompt: one) { _ in }, size: CGSize(width: 440, height: 220))
            let batch = kvUnlockPromptAlways(count: 6, name: nil)
            try recorder.still("unlock-prompt-batch", UnlockPromptSheet(prompt: batch) { _ in }, size: CGSize(width: 440, height: 220))
            try recorder.still("delete-confirm", DeleteConfirmSheet(confirm: kvDeleteConfirm(count: 6, liveCopies: 1), onDelete: {}, onCancel: {}),
                               size: CGSize(width: 420, height: 170))
            let (t, _) = try await reviewModel()
            try recorder.still("review-sites", TeleportReview(teleport: t, review: t.review!),
                               size: CGSize(width: 640, height: 760))
            let (v, _) = try await reviewModel(vault: true)
            try recorder.still("review-vault", TeleportReview(teleport: v, review: v.review!),
                               size: CGSize(width: 640, height: 760))
        }
    }
}
