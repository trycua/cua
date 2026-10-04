// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import SwiftUI
import Testing

/// The vault list a password manager has, grouped by source app: items
/// (cookie, localStorage value, password, file) under sites under apps, a
/// lock on each, multi-select with a batch bar, search by domain, key, app
/// or type, one prompt and one Touch ID for a batch unlock, and delete that
/// wipes live copies. Behavior through the view model and the core; looks as
/// snapshots (see KeyvaultSnapshotTests).
@MainActor
@Suite("Keyvault vault list")
struct KeyvaultVaultTests {
    func model(_ overview: KeyvaultOverview = KeyvaultFixtures.overview()) async -> (AppModel, FakeKeyvault) {
        let fake = FakeKeyvault(overview)
        let m = ViewModelTests().makeModel(kv: fake)
        await m.keyvault.refresh()
        return (m, fake)
    }

    /// Selects every item of a site the way a click on its checkbox does.
    func selectSite(_ m: AppModel, _ app: String, _ site: String) {
        m.keyvault.send(.toggleGroup(key: "\(app)|\(site)"))
    }

    @Test func groupsByAppThenSiteWithFilesApart() async throws {
        let (m, _) = await model()
        let v = m.keyvault.vaultView
        #expect(v.apps.map(\.name) == ["Arc", "Google Chrome", "Slack"])
        let chrome = try #require(v.apps.first { $0.providerId == "chrome" })
        #expect(chrome.open, "an app opens the first time it is shown")
        #expect(chrome.sites.map(\.site) == ["amazon.com", "github.com", "google.com", "linear.app", "notion.so"])
        #expect(chrome.sites.first { $0.site == "github.com" }?.counts == "1 password, 4 cookies, 1 storage value")
        #expect(chrome.files?.count == 3)
        #expect(!v.namesHidden)
        // The sidebar has an app row for each, by name.
        #expect(m.keyvault.sidebar.apps.map(\.title) == ["Arc", "Google Chrome", "Slack"])
    }

    @Test func searchMatchesDomainKeyAppAndType() async throws {
        let (m, _) = await model()
        func shown(_ q: String) -> UInt32 { m.keyvault.query = q; return m.keyvault.vaultView.shown }
        #expect(shown("notion") == 3, "domain")
        #expect(shown("token_v2") == 1, "key")
        #expect(shown("slack") == 5, "app and domain")
        #expect(shown("password") == 2, "type")
        #expect(shown("local storage") == 3, "type, two words")
        #expect(shown("cookie github") == 4)
        #expect(shown("nothing like this") == 0)
        #expect(m.keyvault.vaultView.emptyText == "No matches")
        #expect(shown("") == 27)
    }

    @Test func selectingASiteAnAppOrItemsAndTheBatchBar() async throws {
        let (m, _) = await model()
        selectSite(m, "chrome", "github.com")
        #expect(m.keyvault.vaultView.selection.count == 6)
        m.keyvault.send(.toggle(id: "c-no-1"))
        #expect(m.keyvault.vaultView.selection.title == "7 selected")
        m.keyvault.send(.clear)
        m.keyvault.send(.toggleGroup(key: "slack"))
        #expect(m.keyvault.vaultView.selection.count == 5)
        // Switching the sidebar to another app starts a fresh selection.
        m.keyvault.selection = .app(key: "arc")
        #expect(m.keyvault.vaultView.selection.count == 0)
        #expect(m.keyvault.vaultView.apps.map(\.name) == ["Arc"])
        m.keyvault.selection = .category(category: .all)
        #expect(m.keyvault.vaultView.apps.count == 3)
    }

    @Test func aBatchUnlockAsksOnceThenTheDaemonAsksTouchIdOnce() async throws {
        let (m, fake) = await model()
        selectSite(m, "chrome", "github.com")
        let sel = m.keyvault.vaultView.selection
        #expect(sel.canUnlock && sel.canLock)
        await m.keyvault.requestUnlock(ids: sel.unlockIds)
        // The prompt: the user's words, three answers, and nothing sent yet.
        let pending = try #require(m.keyvault.unlockPrompt)
        #expect(pending.prompt.title == "Allow unattended access?")
        #expect(pending.prompt.message == "This will allow any agent with access to the Cua Spaces MCP to write these keys into your connected Spaces. The secrets are not sent directly to any agents.")
        #expect([pending.prompt.deny, pending.prompt.allow, pending.prompt.neverAsk] == ["Deny", "Allow", "Never ask again"])
        #expect(fake.commands.isEmpty)
        await m.keyvault.answerUnlock(.allow)
        #expect(m.keyvault.unlockPrompt == nil)
        #expect(fake.commands == [.setLocked(itemIds: sel.unlockIds, locked: false)], "one request for the whole batch")
        #expect(m.keyvault.vaultView.apps.flatMap(\.sites).first { $0.site == "github.com" }?.lock == .unlocked)
    }

    @Test func denyChangesNothing() async throws {
        let (m, fake) = await model()
        await m.keyvault.requestUnlock(ids: ["c-gh-3"], name: "_gh_sess")
        #expect(m.keyvault.unlockPrompt?.prompt.subject == "_gh_sess")
        #expect(m.keyvault.unlockPrompt?.prompt.message.contains("write this key") == true)
        await m.keyvault.answerUnlock(.deny)
        #expect(fake.commands.isEmpty)
        #expect(m.keyvault.unlockPrompt == nil)
    }

    @Test func neverAskAgainIsStoredShownAndRevertibleInSettings() async throws {
        let (m, fake) = await model()
        await m.keyvault.requestUnlock(ids: ["c-gh-3"], name: "_gh_sess")
        await m.keyvault.answerUnlock(.neverAskAgain)
        #expect(fake.commands == [.setSkipUnlockPrompt(on: true), .setLocked(itemIds: ["c-gh-3"], locked: false)],
                "it still unlocks, and the daemon still asks for Touch ID")
        #expect(m.keyvault.unlockPromptShows == false)
        // The next unlock skips the prompt and goes straight to Touch ID.
        await m.keyvault.requestUnlock(ids: ["c-am-1"])
        #expect(m.keyvault.unlockPrompt == nil)
        #expect(fake.commands.last == .setLocked(itemIds: ["c-am-1"], locked: false))
        // Settings shows it, off, and turning it on restores the prompt.
        await m.loadSettings()
        let row = try #require(m.settingsPage.sections.first { $0.id == "keyvault" }?.rows.first { $0.id == "keyvault-unlock-prompt" })
        #expect(row.options.first { $0.id == "on" }?.active == false)
        await m.choose(row: "keyvault-unlock-prompt", option: "on")
        #expect(fake.commands.last == .setSkipUnlockPrompt(on: false))
        #expect(m.keyvault.unlockPromptShows == true)
        await m.keyvault.requestUnlock(ids: ["c-am-2"])
        #expect(m.keyvault.unlockPrompt != nil)
    }

    @Test func protectionLivesInSettingsNotTheList() async throws {
        let (m, _) = await model()
        await m.loadSettings()
        let section = try #require(m.settingsPage.sections.first { $0.id == "keyvault" })
        let facts = section.rows.filter { $0.id.hasPrefix("keyvault-protection:") }
        #expect(facts.map(\.label).contains("Touch ID"))
        #expect(facts.allSatisfy { $0.value != nil })
    }

    @Test func anIdentityProviderStaysLocked() async throws {
        let (m, fake) = await model()
        selectSite(m, "chrome", "google.com")
        let sel = m.keyvault.vaultView.selection
        #expect(!sel.canUnlock && sel.alwaysAsk == 2)
        await m.keyvault.requestUnlock(ids: sel.unlockIds)
        #expect(m.keyvault.unlockPrompt == nil && fake.commands.isEmpty)
    }

    @Test func lockingNeedsNoPrompt() async throws {
        let (m, fake) = await model()
        let open = m.keyvault.overview.items.filter(\.policy.unattended).map(\.id)
        await m.keyvault.lock(ids: open)
        #expect(m.keyvault.unlockPrompt == nil)
        #expect(fake.commands == [.setLocked(itemIds: open, locked: true)])
        #expect(m.keyvault.overview.items.allSatisfy { !$0.policy.unattended })
    }

    @Test func deleteConfirmsNamesTheCopiesItWipesAndWipesThem() async throws {
        var o = KeyvaultFixtures.overview()
        o.deliveries = [KvDelivery(importId: "imp-1", target: "local:aurora", providerId: "chrome",
                                   items: ["c-gh-1", "c-gh-2"], callerFp: "fp", deliveredMs: 1_799_999_000_000,
                                   expiresMs: 0, wiped: false)]
        let (m, fake) = await model(o)
        selectSite(m, "chrome", "github.com")
        let ids = m.keyvault.vaultView.selection.ids
        m.keyvault.requestDelete(ids: ids)
        let confirm = try #require(m.keyvault.deleteConfirm)
        #expect(confirm.confirm.title == "Delete 6 items?")
        #expect(confirm.confirm.message.contains("Copies delivered to a Space will be wiped as well."))
        #expect(fake.commands.isEmpty, "nothing happens until it is confirmed")
        await m.keyvault.confirmDelete()
        #expect(fake.commands == [.deleteItems(itemIds: ids)])
        #expect(m.keyvault.overview.items.count == 21)
        #expect(m.keyvault.overview.deliveries[0].wiped, "the live copy was wiped")
        #expect(m.keyvault.vaultView.selection.count == 0, "the selection forgets what is gone")
        #expect(!m.keyvault.vaultView.apps.flatMap(\.sites).contains { $0.site == "github.com" })
    }

    @Test func namesStayHiddenUntilTheUserConfirmsWithTouchId() async throws {
        let (m, fake) = await model(KeyvaultFixtures.overview(namesVisible: false))
        let v = m.keyvault.vaultView
        #expect(v.namesHidden)
        #expect(v.hiddenNote == "Names are hidden. Confirm with Touch ID to see what is saved.")
        #expect(v.apps.map(\.count) == [4, 18, 5], "counts survive while names are hidden")
        await m.keyvault.showItems()
        #expect(fake.commands == [.browse])
        #expect(!m.keyvault.vaultView.namesHidden)
        #expect(m.keyvault.vaultView.apps.contains { $0.sites.contains { $0.site == "github.com" } })
    }
}
