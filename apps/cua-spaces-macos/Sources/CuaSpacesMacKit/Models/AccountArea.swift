// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// The sidebar's account area (CUA-2204): one line, the account, with a
/// menu (Settings, Plan, Sign out), and on Free only the "Run more Spaces
/// at once with Pro" banner above it. What shows and what it says are the core's
/// (`account::menu`, `upsell::pro_banner`); this reads the app's facts.
extension AppModel {
    /// The relay's plan word (`free`, `pro`, `team`; empty: not known yet).
    public var accountPlan: String { devices.relayUsage?.plan ?? "" }

    /// How this app is signed in: `user` (a person's sign-in, which signs
    /// out) or `none`.
    public var accountAuthMode: String { account != nil && identity != nil ? "user" : "none" }

    /// The account line's menu.
    public var accountMenu: [AppAccountMenuItem] {
        Self.accountMenu(plan: accountPlan, signedIn: identity != nil, authMode: accountAuthMode)
    }

    /// The account menu for these facts (the core's `account::menu`).
    public static func accountMenu(plan: String, signedIn: Bool, authMode: String) -> [AppAccountMenuItem] {
        appAccountMenu(input: AppAccountMenuInput(plan: plan, signedIn: signedIn, authMode: authMode))
    }

    /// The Pro banner now (nil: none).
    public func proBanner() -> AppProBanner? {
        Self.proBanner(usage: devices.relayUsage)
    }

    /// The Pro banner for this allowance (nil: none): Free only, and it
    /// cannot be closed.
    public static func proBanner(usage: AppRelayUsage?) -> AppProBanner? {
        guard let usage else { return nil }
        let banner = appProBanner(input: AppProBannerInput(plan: usage.plan))
        return banner.visible ? banner : nil
    }

    static func ms(_ date: Date) -> UInt64 { UInt64(max(0, date.timeIntervalSince1970 * 1000)) }
}

extension DevicesModel {
    /// Plan in the account menu, and the Pro banner's Upgrade: records
    /// `upsell_account_menu_click` and answers the plan card's link (Pro's
    /// checkout on Free), else the relay's upgrade link, else the account
    /// app, saying where it was opened from (`?from=app_account_menu`).
    public func openAccountPlanLink() -> URL? {
        let link = relayUsageView?.planCard.primary?.url ?? relayUsage?.upgradeUrl ?? appAccountAppUrl()
        telemetry?.record(appTelemetryUpsell(surface: "account_menu", action: "click", kind: nil, plan: relayUsage?.plan))
        return URL(string: appUpgradeLink(url: link, surface: "app_account_menu"))
    }
}
