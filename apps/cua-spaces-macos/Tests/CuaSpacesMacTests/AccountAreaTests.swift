// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The sidebar's account area (CUA-2204): the "Run more Spaces at once with
/// Pro" banner on Free only (no way to close it), and the account line's
/// menu. The rules are the core's (`upsell::pro_banner`, `account::menu`);
/// these check the app asks with its facts.
@MainActor
struct AccountAreaTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)
    private func free() -> AppRelayUsage { FixtureDevices.usage(nil)! }

    @Test func theBannerShowsOnFreeOnly() {
        let banner = AppModel.proBanner(usage: free())
        #expect(banner?.text == "Run more Spaces at once with Pro")
        #expect(banner?.button == "Upgrade")
        #expect(AppModel.proBanner(usage: FixtureDevices.usage("warn")) != nil, "also near a limit")
        var pro = free()
        pro.plan = "pro"
        #expect(AppModel.proBanner(usage: pro) == nil)
        #expect(AppModel.proBanner(usage: FixtureDevices.usage("team")) == nil)
        #expect(AppModel.proBanner(usage: nil) == nil, "plan not known yet")
    }

    private func lines(_ items: [AppAccountMenuItem]) -> [String] {
        items.map { "\($0.separatorBefore ? "--- " : "")\($0.label)\($0.value.map { " = \($0)" } ?? "")" }
    }

    @Test func theMenuIsSettingsPlanAndSignOut() {
        #expect(lines(AppModel.accountMenu(plan: "free", signedIn: true, authMode: "user"))
            == ["Settings", "Plan = Free", "--- Sign out"])
        #expect(lines(AppModel.accountMenu(plan: "pro", signedIn: true, authMode: "user"))
            == ["Settings", "Plan = Pro", "--- Sign out"])
        #expect(lines(AppModel.accountMenu(plan: "free", signedIn: true, authMode: "none"))
            == ["Settings", "Plan = Free"], "no Sign out without a person's sign-in")
        #expect(lines(AppModel.accountMenu(plan: "", signedIn: false, authMode: "none")) == ["Settings"])
    }

    @Test func aFixtureModelReadsItsOwnFacts() {
        let m = ViewModelTests().makeModel()
        #expect(m.accountAuthMode == "none", "no account")
        #expect(m.accountPlan == "", "the allowance is not read signed out")
        #expect(lines(m.accountMenu) == ["Settings"])
        #expect(m.proBanner() == nil)
    }

    @Test func planAndUpgradeOpenThePlanLinkAndCountTheClick() {
        let telemetry = FixtureTelemetry()
        let devices = DevicesModel(devices: FixtureDevices(), presence: FixturePresence())
        devices.telemetry = telemetry
        let url = devices.openAccountPlanLink()
        #expect(url?.query?.contains("from=app_account_menu") == true)
        #expect(telemetry.recorded.contains { "\($0)".contains("account_menu") })
    }
}
