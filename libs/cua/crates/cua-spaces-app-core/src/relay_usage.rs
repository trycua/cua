// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's relay allowance this month, as every app shows it: one
//! Settings row ("Relay usage") and, past the allowance, one dismissible
//! line above every page with Upgrade. A live session is a connection to
//! one of the user's remote machines through the hosted relay; relay hours
//! are the time with at least one. Nothing stops at the limit, so the apps
//! only say so.
//!
//! Plain data in (the relay's `GET /v1/usage`, as `cua_host::RelayUsage`
//! reads it and `cua usage` prints it) and plain data out. Every app reads
//! it through the same client (`app_relay_usage` in the Spaces app export)
//! and draws it from [`view`], so the words and the threshold live here.

use serde::{Deserialize, Serialize};

/// The Settings row's label.
pub const ROW_LABEL: &str = "Relay usage";

/// The notice's button.
pub const UPGRADE: &str = "Upgrade";

/// The notice's close button (accessibility label).
pub const DISMISS: &str = "Dismiss";

/// Where a free account upgrades: Pro's checkout.
pub const BUY_PRO_URL: &str = "https://app.cua.ai/buy/pro";

/// Where a Pro account (or a team) gets more: Teams' checkout.
pub const BUY_TEAMS_URL: &str = "https://app.cua.ai/buy/teams";

/// The notice appears early, at this share of an allowance.
pub const NEAR_SHARE: f64 = 0.8;

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
    /// Past the allowance.
    pub notice: Option<RelayAllowanceNotice>,
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
    format!("Live sessions {sessions} \u{b7} Relay hours {relay_hours} this month")
}

/// "Free plan", "Pro plan"; empty when the relay names none.
pub fn plan_text(plan: &str) -> String {
    let mut c = plan.trim().chars();
    match c.next() {
        Some(f) => format!("{}{} plan", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// Which part of the allowance is in play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Close to a limit (at [`NEAR_SHARE`]).
    Near,
    /// Past a limit.
    Over,
}

fn near(used: f64, limit: Option<f64>) -> bool {
    limit.is_some_and(|l| l > 0.0 && used >= l * NEAR_SHARE && used <= l)
}

/// What is close to its limit: `sessions` and/or `hours` (not yet past).
fn near_reasons(u: &RelayUsage) -> Vec<String> {
    let mut r = Vec::new();
    if near(
        f64::from(u.live_sessions),
        u.live_sessions_limit.map(f64::from),
    ) {
        r.push("sessions".to_string());
    }
    if near(u.hours_used, u.hours_limit) {
        r.push("hours".to_string());
    }
    r
}

/// The notice's level and reasons: past the allowance, else close to it.
fn level(u: &RelayUsage) -> Option<(Level, Vec<String>)> {
    if u.soft_limit_reached {
        return Some((Level::Over, u.reasons.clone()));
    }
    let r = near_reasons(u);
    (!r.is_empty()).then_some((Level::Near, r))
}

/// See [`RelayAllowanceNotice::dismiss_key`]. Close to a limit and past it
/// are separate, so passing the limit shows the notice again.
pub fn dismiss_key(u: &RelayUsage) -> String {
    let (level, mut reasons) = level(u).unwrap_or((Level::Over, u.reasons.clone()));
    reasons.sort();
    let prefix = if level == Level::Near { "near:" } else { "" };
    format!("{prefix}{}:{}", u.period, reasons.join(","))
}

/// The notice's words, with the current usage. Sessions and hours are
/// said apart.
pub fn notice_text(u: &RelayUsage) -> String {
    let Some((level, reasons)) = level(u) else {
        return String::new();
    };
    let free = if u.plan == "free" { "free " } else { "" };
    let mut parts = Vec::new();
    if reasons.iter().any(|r| r == "hours") {
        let used = hours(u.hours_used);
        parts.push(match u.hours_limit {
            Some(l) => format!(
                "You've used {used} of your {} {free}relay hours this month.",
                hours(l)
            ),
            None => format!("You've used {used} relay hours this month."),
        });
    }
    if reasons.iter().any(|r| r == "sessions") {
        let n = u.live_sessions;
        let noun = if n == 1 { "session" } else { "sessions" };
        parts.push(match u.live_sessions_limit {
            Some(l) => {
                format!("You have {n} live {noun} open. Your {free}plan includes {l} at once.")
            }
            None => format!("You have {n} live {noun} open."),
        });
    }
    let tail = match level {
        Level::Over => "Your Spaces keep working. Upgrade for more.",
        Level::Near => "Your Spaces keep working, even past the limit.",
    };
    format!("{} {tail}", parts.join(" "))
}

/// Where Upgrade goes for this account: Pro's checkout for a free account,
/// Teams' for a Pro account, a team's own billing page for a team (it adds
/// seats there). An unnamed plan keeps the relay's link.
fn upgrade_url(u: &RelayUsage, account_app: &str) -> String {
    let billing = format!("{}/billing", account_app.trim_end_matches('/'));
    match u.plan.as_str() {
        "free" => BUY_PRO_URL.to_string(),
        "pro" if u.team.is_none() => BUY_TEAMS_URL.to_string(),
        "team" | "teams" | "pro" => billing,
        _ => checked(u.clone()).upgrade_url.unwrap_or(billing),
    }
}

/// The row and the notice; none when the relay meters nothing (the apps
/// show nothing about it). The notice shows at 80% of an allowance and past
/// it; Upgrade opens the checkout that fits the plan (see [`upgrade_url`]).
pub fn view(u: &RelayUsage) -> Option<RelayUsageView> {
    view_for(u, &cua_home::env::account_app_url())
}

/// [`view`] with Upgrade falling back to `account_app`'s billing page.
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
        notice: level(u).map(|_| RelayAllowanceNotice {
            text: notice_text(u),
            upgrade: UPGRADE.into(),
            upgrade_url: Some(upgrade_url(u, account_app)),
            dismiss: DISMISS.into(),
            dismiss_key: dismiss_key(u),
        }),
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
        assert_eq!(v.label, "Relay usage");
        assert_eq!(
            v.line,
            "Live sessions 1 of 2 \u{b7} Relay hours 6.5 of 20 this month"
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
        let v = view(&u).unwrap();
        assert_eq!(
            v.line,
            "Live sessions 1 of 2 \u{b7} Relay hours 20.2 of 20 this month \u{b7} Team pool, 3 seats"
        );
        let n = v.notice.unwrap();
        assert_eq!(
            n.text,
            "You've used 20.2 of your 20 free relay hours this month. You have 1 live session open. Your free plan includes 2 at once. Your Spaces keep working. Upgrade for more."
        );
        assert_eq!(n.upgrade, "Upgrade");
        assert_eq!(n.upgrade_url.as_deref(), Some("https://app.cua.ai/buy/pro"));
        assert_eq!(n.dismiss_key, "2026-10:hours,sessions");

        let pro = RelayUsage {
            plan: "pro".into(),
            team: Some(RelayUsageTeam {
                id: "acme".into(),
                seats: 1,
            }),
            ..u
        };
        let v = view(&pro).unwrap();
        assert!(v.line.ends_with("Team pool, 1 seat"), "{}", v.line);
        assert_eq!(v.plan, "Pro plan");
        let n = v.notice.unwrap();
        assert!(
            n.text
                .starts_with("You've used 20.2 of your 20 relay hours"),
            "{}",
            n.text
        );
        // A Pro account in a team pool adds seats on the billing page.
        assert!(n.upgrade_url.unwrap().ends_with("/billing"));
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
        assert_eq!(v.line, "Live sessions 1 \u{b7} Relay hours 3 this month");
        assert_eq!(v.plan, "");
        assert_eq!(
            view(&RelayUsage {
                metered: false,
                ..free()
            }),
            None
        );
    }

    fn over_hours(plan: &str) -> RelayUsage {
        RelayUsage {
            plan: plan.into(),
            hours_used: 21.0,
            soft_limit_reached: true,
            reasons: vec!["hours".into()],
            ..free()
        }
    }

    fn url(u: &RelayUsage, app: &str) -> Option<String> {
        view_for(u, app).unwrap().notice.unwrap().upgrade_url
    }

    #[test]
    fn upgrade_opens_the_checkout_for_the_plan() {
        let app = "https://app.staging.cua.ai/";
        // Free: Pro's checkout, whatever the relay sent.
        assert_eq!(
            url(&over_hours("free"), app).as_deref(),
            Some("https://app.cua.ai/buy/pro")
        );
        // Pro alone: Teams' checkout.
        assert_eq!(
            url(&over_hours("pro"), app).as_deref(),
            Some("https://app.cua.ai/buy/teams")
        );
        // A team adds seats on the billing page.
        let team = RelayUsage {
            team: Some(RelayUsageTeam {
                id: "t".into(),
                seats: 3,
            }),
            ..over_hours("team")
        };
        assert_eq!(
            url(&team, app).as_deref(),
            Some("https://app.staging.cua.ai/billing")
        );
        // An unnamed plan: the relay's page, else the billing page.
        let mut u = over_hours("");
        assert_eq!(url(&u, app).as_deref(), Some("https://cua.ai/pricing"));
        u.upgrade_url = None;
        assert_eq!(
            url(&u, app).as_deref(),
            Some("https://app.staging.cua.ai/billing")
        );
    }

    #[test]
    fn the_notice_comes_early_at_80_percent() {
        // 16 of 20 hours: close, with the usage and no block.
        let u = RelayUsage {
            hours_used: 16.0,
            ..free()
        };
        let n = view(&u).unwrap().notice.unwrap();
        assert_eq!(
            n.text,
            "You've used 16 of your 20 free relay hours this month. Your Spaces keep working, even past the limit."
        );
        assert_eq!(n.dismiss_key, "near:2026-10:hours");
        // 15.9 is not yet.
        assert_eq!(
            view(&RelayUsage {
                hours_used: 15.9,
                ..free()
            })
            .unwrap()
            .notice,
            None
        );
        // Sessions are said apart from hours.
        let s = RelayUsage {
            live_sessions: 2,
            ..free()
        };
        assert_eq!(
            view(&s).unwrap().notice.unwrap().text,
            "You have 2 live sessions open. Your free plan includes 2 at once. Your Spaces keep working, even past the limit."
        );
        // Past the limit is a different key: the notice shows again.
        assert_ne!(dismiss_key(&u), dismiss_key(&over_hours("free")));
        // No limit, no early notice.
        let none = RelayUsage {
            hours_limit: None,
            live_sessions_limit: None,
            hours_used: 500.0,
            ..free()
        };
        assert_eq!(view(&none).unwrap().notice, None);
    }

    #[test]
    fn only_https_upgrade_links_are_kept() {
        for bad in ["http://cua.ai/pricing", "javascript:alert(1)", ""] {
            let u = RelayUsage {
                upgrade_url: Some(bad.into()),
                soft_limit_reached: true,
                plan: String::new(),
                ..free()
            };
            assert_eq!(checked(u.clone()).upgrade_url, None, "{bad}");
            // A link that is not https is dropped; Upgrade opens the
            // account app's billing page instead.
            assert_eq!(
                url(&u, "https://app.cua.ai").as_deref(),
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
            "Live sessions 0 \u{b7} Relay hours 0 this month"
        );
    }
}
