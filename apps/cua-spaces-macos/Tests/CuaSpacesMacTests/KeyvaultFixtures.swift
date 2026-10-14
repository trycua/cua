// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation

/// A simulated vault for the Keyvault tests and screenshots: realistic apps
/// (Chrome, Slack, Arc), domains, cookies, localStorage values, passwords
/// and files, and no real secret anywhere. The broker never returns a value,
/// so a fixture has none either.
enum KeyvaultFixtures {
    static let now: Int64 = 1_800_000_000_000
    static let minute: Int64 = 60_000
    static let hour: Int64 = 3_600_000
    static let day: Int64 = 86_400_000

    static func item(_ id: String, app: (String, String), kind: String, domain: String?, key: String,
                     path: String? = nil, open: Bool = false, idp: Bool = false,
                     ago: Int64 = 3 * hour) -> KvItem {
        KvItem(id: id, kind: kind, providerId: app.0, appDisplay: app.1, domain: domain, key: key, path: path,
               source: "Default", session: false, expiresMs: nil, bytes: 64, blob: nil, identityProvider: idp,
               policy: KvItemPolicy(allowedTargets: [], ttlSecs: 3600, unattended: open),
               createdMs: UInt64(now - 9 * day), updatedMs: UInt64(now - ago), rev: 1, recordDigest: "00")
    }

    static let chrome = ("chrome", "Google Chrome")
    static let slack = ("slack", "Slack")
    static let arc = ("arc", "Arc")

    /// Three apps, a dozen sites, one identity provider, a few unlocked.
    static func items() -> [KvItem] {
        var out: [KvItem] = []
        func cookie(_ id: String, _ app: (String, String), _ domain: String, _ key: String, open: Bool = false,
                    idp: Bool = false, ago: Int64 = 3 * hour) {
            out.append(item(id, app: app, kind: "cookie", domain: domain, key: key, path: "/", open: open, idp: idp, ago: ago))
        }
        cookie("c-gh-1", chrome, ".github.com", "user_session", open: true, ago: 5 * minute)
        cookie("c-gh-2", chrome, ".github.com", "logged_in", open: true, ago: 5 * minute)
        cookie("c-gh-3", chrome, "api.github.com", "_gh_sess", ago: 5 * minute)
        cookie("c-gh-4", chrome, ".github.com", "dotcom_user", ago: 5 * minute)
        out.append(item("p-gh", app: chrome, kind: "password", domain: "https://github.com", key: "octocat", ago: 2 * day))
        out.append(item("l-gh", app: chrome, kind: "local_storage", domain: "https://github.com", key: "color_mode", ago: 5 * minute))
        cookie("c-no-1", chrome, ".notion.so", "token_v2", ago: 2 * hour)
        cookie("c-no-2", chrome, ".notion.so", "notion_user_id", ago: 2 * hour)
        out.append(item("l-no", app: chrome, kind: "local_storage", domain: "https://www.notion.so", key: "LRU:spaceId", ago: 2 * hour))
        cookie("c-li-1", chrome, ".linear.app", "linear_session", open: true, ago: 26 * hour)
        cookie("c-am-1", chrome, ".amazon.com", "session-id", ago: 3 * day)
        cookie("c-am-2", chrome, ".amazon.com", "ubid-main", ago: 3 * day)
        cookie("c-go-1", chrome, ".google.com", "SID", idp: true, ago: 4 * hour)
        cookie("c-go-2", chrome, ".google.com", "__Secure-1PSID", idp: true, ago: 4 * hour)
        out.append(item("p-li", app: chrome, kind: "password", domain: "https://linear.app", key: "ada@example.com", ago: 4 * day))
        out.append(item("f-bm", app: chrome, kind: "file", domain: nil, key: "Default/Bookmarks", ago: 5 * minute))
        out.append(item("f-pr", app: chrome, kind: "file", domain: nil, key: "Default/Preferences", ago: 5 * minute))
        out.append(item("f-ls", app: chrome, kind: "file", domain: nil, key: "Local State", ago: 5 * minute))
        cookie("c-sl-1", slack, ".slack.com", "d", open: true, ago: 40 * minute)
        cookie("c-sl-2", slack, ".slack.com", "d-s", open: true, ago: 40 * minute)
        out.append(item("l-sl", app: slack, kind: "local_storage", domain: "https://app.slack.com", key: "localConfig_v2", ago: 40 * minute))
        out.append(item("f-sl-1", app: slack, kind: "file", domain: nil, key: "storage/root-state.json", ago: 40 * minute))
        out.append(item("f-sl-2", app: slack, kind: "file", domain: nil, key: "storage/slack-workspaces", ago: 40 * minute))
        cookie("c-fi-1", arc, ".figma.com", "__Host-figma.authn", ago: 6 * day)
        cookie("c-fi-2", arc, ".figma.com", "figma.st", ago: 6 * day)
        cookie("c-vc-1", arc, ".vercel.com", "authorization", ago: 6 * day)
        out.append(item("f-arc", app: arc, kind: "file", domain: nil, key: "StorableSidebar.json", ago: 6 * day))
        return out
    }

    static func status(skipPrompt: Bool = false) -> KvStatus {
        KvStatus(version: "0.1.0", initialized: true, unlocked: true, disabled: false, callerFirstParty: true,
                 callerDisplay: "Cua Spaces", items: 27, pending: 0, unlockPolicy: "auto", autoWipe: false,
                 osProtectorAvailable: true, passphraseAvailable: true, unlockProtectors: ["macos-keychain"],
                 browseUntilMs: nil, skipUnlockPrompt: skipPrompt, resetNotice: nil)
    }

    /// The vault as the broker serves it (`namesVisible`: the user opened
    /// the browse window with Touch ID).
    static func overview(namesVisible: Bool = true, skipPrompt: Bool = false) -> KeyvaultOverview {
        var all = items()
        if !namesVisible {
            all = all.map { i in
                var r = i
                r.domain = nil
                r.key = ""
                r.path = nil
                r.source = ""
                return r
            }
        }
        return KeyvaultOverview(availability: "ready", message: nil, status: status(skipPrompt: skipPrompt),
                                serverVerified: true, items: all, namesVisible: namesVisible,
                                itemsTotal: UInt32(all.count), pending: [], grants: [], rules: [], deliveries: [],
                                audit: [], auditVerification: nil, partialErrors: [])
    }

    /// A Chrome inventory with counts per site, as the review shows it.
    static func chromeInventory() -> KvInventory {
        func d(_ domain: String, cookies: UInt32, ls: UInt32 = 0, session: UInt32 = 0, passwords: UInt32 = 0,
               unavailable: UInt32 = 0, signin: Bool, idp: Bool = false) -> KvDomainCount {
            KvDomainCount(domain: domain, cookies: cookies, sessionCookies: session, localStorage: ls,
                          passwords: passwords, signin: signin, identityProvider: idp, unavailable: unavailable,
                          unavailableReason: unavailable == 0 ? "" :
                            "Chrome protects it with app-bound encryption, which only Chrome itself can unlock on this PC")
        }
        return KvInventory(providerId: "chrome", appDisplay: "Google Chrome", domains: [
            d("amazon.com", cookies: 14, signin: true),
            d("bank.example", cookies: 0, unavailable: 3, signin: true),
            d("doubleclick.net", cookies: 9, signin: false),
            d("github.com", cookies: 12, ls: 3, session: 4, passwords: 2, signin: true),
            d("google.com", cookies: 31, signin: true, idp: true),
            d("linear.app", cookies: 6, ls: 2, signin: true),
            d("notion.so", cookies: 8, ls: 5, signin: true),
            d("nytimes.com", cookies: 22, signin: false),
            d("slack.com", cookies: 5, signin: true),
        ], notes: [])
    }
}
