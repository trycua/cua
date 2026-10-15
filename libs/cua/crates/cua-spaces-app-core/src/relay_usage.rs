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

/// See [`RelayAllowanceNotice::dismiss_key`].
pub fn dismiss_key(u: &RelayUsage) -> String {
    let mut reasons = u.reasons.clone();
    reasons.sort();
    format!("{}:{}", u.period, reasons.join(","))
}

/// The notice's sentence.
pub fn notice_text(u: &RelayUsage) -> String {
    let free = if u.plan == "free" { "free " } else { "" };
    format!(
        "You've used your {free}relay allowance for this month. Your Spaces keep working. Upgrade for more."
    )
}

/// The row and the notice; none when the relay meters nothing (the apps
/// show nothing about it). Upgrade opens the relay's page, else the account
/// app's billing page.
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
        notice: u.soft_limit_reached.then(|| RelayAllowanceNotice {
            text: notice_text(u),
            upgrade: UPGRADE.into(),
            upgrade_url: checked(u.clone())
                .upgrade_url
                .or_else(|| Some(format!("{}/billing", account_app.trim_end_matches('/')))),
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
            "You've used your free relay allowance for this month. Your Spaces keep working. Upgrade for more."
        );
        assert_eq!(n.upgrade, "Upgrade");
        assert_eq!(n.upgrade_url.as_deref(), Some("https://cua.ai/pricing"));
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
        assert!(v.notice.unwrap().text.starts_with("You've used your relay"));
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

    #[test]
    fn upgrade_falls_back_to_the_account_app_billing_page() {
        let mut u = free();
        u.soft_limit_reached = true;
        u.reasons = vec!["hours".into()];
        // The relay's page wins.
        let n = view_for(&u, "https://app.staging.cua.ai")
            .unwrap()
            .notice
            .unwrap();
        assert_eq!(n.upgrade_url.as_deref(), Some("https://cua.ai/pricing"));
        // None from the relay: the account app's billing page.
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
    fn only_https_upgrade_links_are_kept() {
        for bad in ["http://cua.ai/pricing", "javascript:alert(1)", ""] {
            let u = RelayUsage {
                upgrade_url: Some(bad.into()),
                soft_limit_reached: true,
                ..free()
            };
            assert_eq!(checked(u.clone()).upgrade_url, None, "{bad}");
            // A link that is not https is dropped; Upgrade opens the
            // account app's billing page instead.
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
            "Live sessions 0 \u{b7} Relay hours 0 this month"
        );
    }
}
