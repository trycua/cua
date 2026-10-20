// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's relay allowance this month, as every app shows it: one
//! Settings row ("Remote use") and, past the allowance, one dismissible
//! line above every page with Upgrade. A live session is a connection to
//! one of the user's remote machines through the hosted relay; relay hours
//! are the time with at least one. Nothing stops at the limit, so the apps
//! only say so.
//!
//! Users see relay hours as "remote hours" (the API and metric names keep
//! `relay_hours`): the time spent using a computer from somewhere else.
//! Using a computer from itself never goes through the relay, so it is free.
//!
//! Plain data in (the relay's `GET /v1/usage`, as `cua_host::RelayUsage`
//! reads it and `cua usage` prints it) and plain data out. Every app reads
//! it through the same client (`app_relay_usage` in the Spaces app export)
//! and draws it from [`view`], so the words and the threshold live here.

use serde::{Deserialize, Serialize};

/// The Settings row's label.
pub const ROW_LABEL: &str = "Remote use";

/// What remote hours are, in plain words: where the term first shows (the
/// Free plan card, `cua usage`). The relay meters the time an account has
/// at least one live session through it; a computer used from itself never
/// goes through the relay.
pub const REMOTE_HOURS_MEANING: &str = "Remote hours: time spent using your computers from somewhere else (your laptop, phone or an agent on another computer). Using a computer from itself is free.";

/// The notice's button.
pub const UPGRADE: &str = "Upgrade";

/// The notice's close button (accessibility label).
pub const DISMISS: &str = "Dismiss";

/// The share of the month's relay hours at which the early notice shows
/// (80%). The hard notice shows when the relay says the allowance is used.
pub const WARN_AT: f64 = 0.8;

/// What Pro gives (cua.ai/pricing): one person, per month.
pub const PRO_SESSIONS: u32 = 5;
/// See [`PRO_SESSIONS`].
pub const PRO_HOURS: u32 = 200;

/// One plain line on Pro's plan card for people who pay monthly. The Billing
/// page checks the plan and shows the switch only to monthly payers, with the
/// exact charge from Stripe; the card says nothing it cannot back up.
pub const PRO_YEARLY_NOTE: &str = "Paying monthly? Switching to yearly saves $36 a year.";

/// Where Pro is bought, on the account app.
pub const BUY_PRO_PATH: &str = "/buy/pro";
/// Where Teams is bought, on the account app.
pub const BUY_TEAMS_PATH: &str = "/buy/teams";

/// The team whose allowance is pooled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RelayUsageTeam {
    /// Team id.
    pub id: String,
    /// Seats sharing the allowance.
    pub seats: u32,
}

/// The relay's answer (`cua_host::RelayUsage`, camelCase: the web bridge's
/// `relay.usage` result). Every field has a default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RelayUsage {
    /// The relay meters this account (false: no allowance applies).
    pub metered: bool,
    /// The month counted (`2026-10`).
    pub period: String,
    /// `free`, `pro`, `team`, ...
    pub plan: String,
    /// The team whose allowance is pooled.
    pub team: Option<RelayUsageTeam>,
    /// Live sessions now.
    pub live_sessions: u32,
    /// Live sessions allowed at once (none: no limit).
    pub live_sessions_limit: Option<u32>,
    /// Relay hours used this month (the team's, when pooled).
    pub hours_used: f64,
    /// Relay hours allowed this month (none: no limit).
    pub hours_limit: Option<f64>,
    /// This account's own relay hours this month.
    pub my_hours_used: f64,
    /// Past the allowance (sessions keep working).
    pub soft_limit_reached: bool,
    /// What is past it: `sessions` and/or `hours`.
    pub reasons: Vec<String>,
    /// Where to get more (https only; see [`checked`]).
    pub upgrade_url: Option<String>,
}

/// The line above every page past the allowance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayAllowanceNotice {
    /// "You've used your free relay allowance for this month. ..."
    pub text: String,
    /// [`UPGRADE`].
    pub upgrade: String,
    /// What Upgrade opens; none: no Upgrade button.
    pub upgrade_url: Option<String>,
    /// [`DISMISS`].
    pub dismiss: String,
    /// What a dismissal remembers: the month and the reasons. A new month,
    /// or another limit reached, shows the notice again.
    pub dismiss_key: String,
    /// `warn` (80% of the relay hours) or `limit` (the allowance is used).
    /// A `warn` is a prompt: the shells hold it back while a live session
    /// or an agent run is going ([`crate::upsell`]). A `limit` is only the
    /// status, so it always shows.
    pub level: String,
}

/// A button that opens a page on the account app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanLink {
    /// "Get Pro".
    pub label: String,
    /// https, on the account app.
    pub url: String,
}

/// The plan as Settings draws it: always there, never a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCard {
    /// `free`, `pro` or `teams`.
    pub plan: String,
    /// "Free plan".
    pub label: String,
    /// What the plan includes, in one sentence.
    pub detail: String,
    /// The main button ("Get Pro"); none when there is nothing to buy.
    pub primary: Option<PlanLink>,
    /// The other one ("Start a team").
    pub secondary: Option<PlanLink>,
}

/// The allowance as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayUsageView {
    /// [`ROW_LABEL`].
    pub label: String,
    /// "Live sessions 1 of 2 · Relay hours 6.5 of 20 this month", then the
    /// team pool when there is one.
    pub line: String,
    /// "Free plan" (empty when the relay names none).
    pub plan: String,
    /// At 80% of the relay hours, or past the allowance.
    pub notice: Option<RelayAllowanceNotice>,
    /// Settings' plan card.
    pub plan_card: PlanCard,
}

/// The answer as the apps keep it: an upgrade link that is not https is
/// dropped (it is opened in the browser without asking).
pub fn checked(mut usage: RelayUsage) -> RelayUsage {
    if !usage
        .upgrade_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"))
    {
        usage.upgrade_url = None;
    }
    usage
}

/// `20` or `6.5`.
pub fn hours(h: f64) -> String {
    let tenths = (h * 10.0).round() / 10.0;
    if tenths.fract() == 0.0 {
        format!("{tenths:.0}")
    } else {
        format!("{tenths:.1}")
    }
}

/// "Live sessions 1 of 2 · Relay hours 6.5 of 20 this month".
pub fn usage_line(u: &RelayUsage) -> String {
    let of = |used: String, limit: Option<String>| match limit {
        Some(l) => format!("{used} of {l}"),
        None => used,
    };
    let sessions = of(
        u.live_sessions.to_string(),
        u.live_sessions_limit.map(|l| l.to_string()),
    );
    let relay_hours = of(hours(u.hours_used), u.hours_limit.map(hours));
    format!("Live sessions {sessions} \u{b7} Remote hours {relay_hours} this month")
}

/// "Free plan", "Pro plan"; empty when the relay names none.
pub fn plan_text(plan: &str) -> String {
    let plan = match plan.trim() {
        "team" => "teams",
        p => p,
    };
    let mut c = plan.chars();
    match c.next() {
        Some(f) => format!("{}{} plan", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// `free`, `pro` or `teams`; anything else the relay names is its own plan.
pub(crate) fn plan_kind(plan: &str) -> &str {
    match plan.trim() {
        "team" | "teams" => "teams",
        p => p,
    }
}

/// The share of the month's relay hours used (none: no hour limit).
pub fn hours_share(u: &RelayUsage) -> Option<f64> {
    u.hours_limit.filter(|l| *l > 0.0).map(|l| u.hours_used / l)
}

/// The early notice applies: 80% or more of the hours, not yet past the
/// allowance, on the Free plan (the plan with something to buy that adds
/// hours: Pro).
pub fn warns(u: &RelayUsage) -> bool {
    !u.soft_limit_reached
        && plan_kind(&u.plan) == "free"
        && hours_share(u).is_some_and(|s| s >= WARN_AT)
}

/// See [`RelayAllowanceNotice::dismiss_key`].
pub fn dismiss_key(u: &RelayUsage) -> String {
    if !u.soft_limit_reached {
        return format!("{}:warn", u.period);
    }
    let mut reasons = u.reasons.clone();
    reasons.sort();
    format!("{}:{}", u.period, reasons.join(","))
}

/// The notice's sentence.
pub fn notice_text(u: &RelayUsage) -> String {
    if warns(u) {
        return format!(
            "You've used {} of {} remote hours this month. Nothing stops at the limit. Pro gives you {PRO_HOURS} hours and {PRO_SESSIONS} live sessions.",
            hours(u.hours_used),
            hours(u.hours_limit.unwrap_or_default()),
        );
    }
    match plan_kind(&u.plan) {
        "free" => "You've reached the free plan's remote limit for this month. Your Spaces keep working. Pro gives you more.".into(),
        "pro" => "You've reached your plan's remote limit for this month. Your Spaces keep working. Teams pools remote hours across seats.".into(),
        _ => "You've reached your team's remote limit for this month. Your Spaces keep working. Add seats for more.".into(),
    }
}

/// A page on the account app. A checkout page carries where the person came
/// from (`?from=app_meter`, a fixed word, never anything about them), so the
/// account app's anonymous purchase funnel can tell the in-app surfaces apart.
pub(crate) fn buy(account_app: &str, path: &str, from: &str) -> String {
    let base = format!("{}{path}", account_app.trim_end_matches('/'));
    if path.starts_with("/buy/") {
        format!("{base}?from={from}")
    } else {
        base
    }
}

/// What the notice's Upgrade opens: Free goes to Pro, Pro to Teams, a team
/// to its billing page; a plan the relay names that this app does not know
/// keeps the relay's own link, else the billing page.
fn upgrade_target(u: &RelayUsage, account_app: &str) -> String {
    match plan_kind(&u.plan) {
        "free" => buy(account_app, BUY_PRO_PATH, "app_meter"),
        "pro" => buy(account_app, BUY_TEAMS_PATH, "app_meter"),
        "teams" => buy(account_app, "/billing", "app_meter"),
        _ => checked(u.clone())
            .upgrade_url
            .unwrap_or_else(|| buy(account_app, "/billing", "app_meter")),
    }
}

/// Settings' plan card.
pub fn plan_card(u: &RelayUsage, account_app: &str) -> PlanCard {
    let kind = plan_kind(&u.plan);
    let link = |label: &str, path: &str| PlanLink {
        label: label.into(),
        url: buy(account_app, path, "app_plan_card"),
    };
    let allowance = |s: Option<u32>, h: Option<f64>| match (s, h) {
        (Some(s), Some(h)) => format!("{s} live sessions and {} remote hours a month", hours(h)),
        _ => "No limit on remote use".into(),
    };
    let (detail, primary, secondary) = match kind {
        "free" => (
            format!(
                "{}. {REMOTE_HOURS_MEANING}",
                allowance(u.live_sessions_limit, u.hours_limit)
            ),
            Some(link("Get Pro", BUY_PRO_PATH)),
            Some(link("Start a team", BUY_TEAMS_PATH)),
        ),
        "pro" => (
            format!(
                "{}, for you. Teams adds team-owned computers, roles and device approval. {PRO_YEARLY_NOTE}",
                allowance(u.live_sessions_limit, u.hours_limit)
            ),
            Some(PlanLink {
                label: "Manage plan".into(),
                url: format!(
                    "{}/billing?from=app_plan_card",
                    account_app.trim_end_matches('/')
                ),
            }),
            Some(link("See Teams", BUY_TEAMS_PATH)),
        ),
        "teams" => (
            match &u.team {
                Some(t) => format!(
                    "{} across {} {}, shared by the team.",
                    allowance(u.live_sessions_limit, u.hours_limit),
                    t.seats,
                    if t.seats == 1 { "seat" } else { "seats" }
                ),
                None => format!(
                    "{}, shared by the team.",
                    allowance(u.live_sessions_limit, u.hours_limit)
                ),
            },
            Some(link("Manage seats", "/billing")),
            None,
        ),
        _ => (
            allowance(u.live_sessions_limit, u.hours_limit),
            Some(link("Billing and plans", "/billing")),
            None,
        ),
    };
    PlanCard {
        plan: kind.into(),
        label: plan_text(&u.plan),
        detail,
        primary,
        secondary,
    }
}

/// The row, the notice and the plan card; none when the relay meters
/// nothing (the apps show nothing about it). Upgrade opens the plan's page
/// on the account app ([`upgrade_target`]).
pub fn view(u: &RelayUsage) -> Option<RelayUsageView> {
    view_for(u, &cua_home::env::account_app_url())
}

/// [`view`] against `account_app`'s pages.
pub fn view_for(u: &RelayUsage, account_app: &str) -> Option<RelayUsageView> {
    if !u.metered {
        return None;
    }
    let team = u
        .team
        .as_ref()
        .map(|t| {
            format!(
                " \u{b7} Team pool, {} {}",
                t.seats,
                if t.seats == 1 { "seat" } else { "seats" }
            )
        })
        .unwrap_or_default();
    Some(RelayUsageView {
        label: ROW_LABEL.into(),
        line: format!("{}{team}", usage_line(u)),
        plan: plan_text(&u.plan),
        notice: (u.soft_limit_reached || warns(u)).then(|| RelayAllowanceNotice {
            text: notice_text(u),
            upgrade: UPGRADE.into(),
            upgrade_url: Some(upgrade_target(u, account_app)),
            dismiss: DISMISS.into(),
            dismiss_key: dismiss_key(u),
            level: if u.soft_limit_reached {
                "limit"
            } else {
                "warn"
            }
            .into(),
        }),
        plan_card: plan_card(u, account_app),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free() -> RelayUsage {
        RelayUsage {
            metered: true,
            period: "2026-10".into(),
            plan: "free".into(),
            team: None,
            live_sessions: 1,
            live_sessions_limit: Some(2),
            hours_used: 6.5,
            hours_limit: Some(20.0),
            my_hours_used: 6.5,
            soft_limit_reached: false,
            reasons: vec![],
            upgrade_url: Some("https://cua.ai/pricing".into()),
        }
    }

    #[test]
    fn within_the_allowance_one_row_and_no_notice() {
        let v = view(&free()).unwrap();
        assert_eq!(v.label, "Remote use");
        assert_eq!(
            v.line,
            "Live sessions 1 of 2 \u{b7} Remote hours 6.5 of 20 this month"
        );
        assert_eq!(v.plan, "Free plan");
        assert_eq!(v.notice, None);
    }

    #[test]
    fn past_the_allowance_the_notice_says_spaces_keep_working() {
        let u = RelayUsage {
            hours_used: 20.24,
            soft_limit_reached: true,
            reasons: vec!["sessions".into(), "hours".into()],
            team: Some(RelayUsageTeam {
                id: "acme".into(),
                seats: 3,
            }),
            ..free()
        };
        let v = view_for(&u, "https://app.cua.ai").unwrap();
        assert_eq!(
            v.line,
            "Live sessions 1 of 2 \u{b7} Remote hours 20.2 of 20 this month \u{b7} Team pool, 3 seats"
        );
        let n = v.notice.unwrap();
        assert_eq!(
            n.text,
            "You've reached the free plan's remote limit for this month. Your Spaces keep working. Pro gives you more."
        );
        assert_eq!(n.upgrade, "Upgrade");
        assert_eq!(n.level, "limit");
        assert_eq!(
            n.upgrade_url.as_deref(),
            Some("https://app.cua.ai/buy/pro?from=app_meter")
        );
        assert_eq!(n.dismiss_key, "2026-10:hours,sessions");

        // Pro past the allowance: Teams pools hours across seats.
        let pro = RelayUsage {
            plan: "pro".into(),
            ..u.clone()
        };
        let n = view_for(&pro, "https://app.cua.ai")
            .unwrap()
            .notice
            .unwrap();
        assert!(n.text.starts_with("You've reached your"), "{}", n.text);
        assert_eq!(
            n.upgrade_url.as_deref(),
            Some("https://app.cua.ai/buy/teams?from=app_meter")
        );

        // A team: its own billing page.
        let team = RelayUsage {
            plan: "team".into(),
            ..u
        };
        let v = view_for(&team, "https://app.cua.ai").unwrap();
        assert!(v.line.ends_with("Team pool, 3 seats"), "{}", v.line);
        assert_eq!(v.plan, "Teams plan");
        assert_eq!(
            v.notice.unwrap().upgrade_url.as_deref(),
            Some("https://app.cua.ai/billing")
        );
    }

    #[test]
    fn pro_plan_card_mentions_yearly_once_and_links_to_billing() {
        let pro = RelayUsage {
            plan: "pro".into(),
            live_sessions_limit: Some(5),
            hours_limit: Some(200.0),
            ..free()
        };
        let card = plan_card(&pro, "https://app.cua.ai/");
        assert!(
            card.detail
                .ends_with("Paying monthly? Switching to yearly saves $36 a year."),
            "{}",
            card.detail
        );
        // The mention is plain: no urgency, no pressure.
        let d = card.detail.to_lowercase();
        for w in [
            "hurry",
            "limited",
            "last chance",
            "expires",
            "act now",
            "don't miss",
        ] {
            assert!(!d.contains(w), "{w}");
        }
        let primary = card.primary.unwrap();
        assert_eq!(primary.label, "Manage plan");
        assert_eq!(primary.url, "https://app.cua.ai/billing?from=app_plan_card");
        // Free and Teams cards do not carry it.
        assert!(
            !plan_card(&free(), "https://app.cua.ai")
                .detail
                .contains("yearly")
        );
        let team = RelayUsage {
            plan: "team".into(),
            ..free()
        };
        assert!(
            !plan_card(&team, "https://app.cua.ai")
                .detail
                .contains("yearly")
        );
    }

    #[test]
    fn at_eighty_percent_of_hours_a_free_plan_gets_the_early_notice() {
        let mut u = free();
        u.hours_used = 15.9;
        assert!(view_for(&u, "https://app.cua.ai").unwrap().notice.is_none());
        u.hours_used = 16.0;
        let n = view_for(&u, "https://app.cua.ai").unwrap().notice.unwrap();
        assert_eq!(n.level, "warn");
        assert_eq!(
            n.text,
            "You've used 16 of 20 remote hours this month. Nothing stops at the limit. Pro gives you 200 hours and 5 live sessions."
        );
        assert_eq!(
            n.upgrade_url.as_deref(),
            Some("https://app.cua.ai/buy/pro?from=app_meter")
        );
        assert_eq!(n.dismiss_key, "2026-10:warn");
        // The hard notice has its own key: a dismissed warning does not hide it.
        u.soft_limit_reached = true;
        u.reasons = vec!["hours".into()];
        let n = view_for(&u, "https://app.cua.ai").unwrap().notice.unwrap();
        assert_eq!(n.level, "limit");
        assert_eq!(n.dismiss_key, "2026-10:hours");
        // Paid plans have no early notice.
        let pro = RelayUsage {
            plan: "pro".into(),
            hours_used: 190.0,
            hours_limit: Some(200.0),
            ..free()
        };
        assert!(
            view_for(&pro, "https://app.cua.ai")
                .unwrap()
                .notice
                .is_none()
        );
        // Nor a plan with no hour limit.
        let open = RelayUsage {
            hours_limit: None,
            hours_used: 500.0,
            ..free()
        };
        assert!(
            view_for(&open, "https://app.cua.ai")
                .unwrap()
                .notice
                .is_none()
        );
    }

    #[test]
    fn the_plan_card_names_the_plan_and_what_to_buy() {
        let card = |u: RelayUsage| view_for(&u, "https://app.cua.ai/").unwrap().plan_card;
        let f = card(free());
        assert_eq!((f.plan.as_str(), f.label.as_str()), ("free", "Free plan"));
        assert_eq!(
            f.detail,
            "2 live sessions and 20 remote hours a month. Remote hours: time spent using your computers from somewhere else (your laptop, phone or an agent on another computer). Using a computer from itself is free."
        );
        let p = f.primary.unwrap();
        assert_eq!(
            (p.label.as_str(), p.url.as_str()),
            ("Get Pro", "https://app.cua.ai/buy/pro?from=app_plan_card")
        );
        let s = f.secondary.unwrap();
        assert_eq!(
            (s.label.as_str(), s.url.as_str()),
            (
                "Start a team",
                "https://app.cua.ai/buy/teams?from=app_plan_card"
            )
        );

        let pro = card(RelayUsage {
            plan: "pro".into(),
            live_sessions_limit: Some(5),
            hours_limit: Some(200.0),
            ..free()
        });
        assert_eq!(pro.label, "Pro plan");
        assert_eq!(
            pro.primary.unwrap().url,
            "https://app.cua.ai/billing?from=app_plan_card"
        );
        assert_eq!(
            pro.secondary.unwrap().url,
            "https://app.cua.ai/buy/teams?from=app_plan_card"
        );

        let team = card(RelayUsage {
            plan: "team".into(),
            team: Some(RelayUsageTeam {
                id: "t".into(),
                seats: 2,
            }),
            live_sessions_limit: Some(10),
            hours_limit: Some(400.0),
            ..free()
        });
        assert_eq!(team.plan, "teams");
        assert_eq!(
            team.detail,
            "10 live sessions and 400 remote hours a month across 2 seats, shared by the team."
        );
        assert!(team.secondary.is_none());
    }

    #[test]
    fn no_limits_and_unmetered() {
        let u = RelayUsage {
            live_sessions_limit: None,
            hours_limit: None,
            hours_used: 3.0,
            plan: String::new(),
            ..free()
        };
        let v = view(&u).unwrap();
        assert_eq!(v.line, "Live sessions 1 \u{b7} Remote hours 3 this month");
        assert_eq!(v.plan, "");
        assert_eq!(
            view(&RelayUsage {
                metered: false,
                ..free()
            }),
            None
        );
    }

    #[test]
    fn an_unknown_plan_keeps_the_relays_link_else_the_billing_page() {
        let mut u = RelayUsage {
            plan: "enterprise".into(),
            soft_limit_reached: true,
            reasons: vec!["hours".into()],
            ..free()
        };
        let n = view_for(&u, "https://app.staging.cua.ai")
            .unwrap()
            .notice
            .unwrap();
        assert_eq!(n.upgrade_url.as_deref(), Some("https://cua.ai/pricing"));
        u.upgrade_url = None;
        let n = view_for(&u, "https://app.staging.cua.ai/")
            .unwrap()
            .notice
            .unwrap();
        assert_eq!(
            n.upgrade_url.as_deref(),
            Some("https://app.staging.cua.ai/billing")
        );
    }

    #[test]
    fn only_https_links_are_kept() {
        for bad in ["http://cua.ai/pricing", "javascript:alert(1)", ""] {
            let u = RelayUsage {
                plan: "enterprise".into(),
                upgrade_url: Some(bad.into()),
                soft_limit_reached: true,
                ..free()
            };
            assert_eq!(checked(u.clone()).upgrade_url, None, "{bad}");
            assert_eq!(
                view_for(&u, "https://app.cua.ai")
                    .unwrap()
                    .notice
                    .unwrap()
                    .upgrade_url
                    .as_deref(),
                Some("https://app.cua.ai/billing")
            );
        }
    }

    #[test]
    fn reads_the_bridge_shape_with_defaults() {
        let u: RelayUsage = serde_json::from_str(
            r#"{"metered":true,"period":"2026-10","plan":"free","team":{"id":"t","seats":2},
                "liveSessions":0,"liveSessionsLimit":null,"hoursUsed":1.25,"hoursLimit":20,
                "myHoursUsed":1,"softLimitReached":false,"reasons":[],"upgradeUrl":null}"#,
        )
        .unwrap();
        assert_eq!(u.team.as_ref().map(|t| t.seats), Some(2));
        assert_eq!(hours(u.hours_used), "1.3");
        let bare: RelayUsage = serde_json::from_str(r#"{"metered":true}"#).unwrap();
        assert_eq!(
            view(&bare).unwrap().line,
            "Live sessions 0 \u{b7} Remote hours 0 this month"
        );
    }
}
