// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teams as every app shows them: the account's organizations from the
//! Spaces API (`GET /v1/me`, `/v1/orgs/{org}/members`, `/invites`), as
//! Settings rows. A team is its name (the org's `name`, else its alias),
//! then its people: members with their role, pending invites marked
//! "(Invited)". The apps administer nothing: invites, roles and billing
//! live in the account app (`app.cua.ai`), one link away. The data is read
//! by the host (`app_teams`); this module only words it.

use crate::settings::{
    SettingsOption, SettingsRow,
    SettingsRowKind::{Choice, Text},
    row,
};
use serde::{Deserialize, Serialize};

/// The production account app: billing, plans, team administration.
pub const ACCOUNT_APP_URL: &str = "https://app.cua.ai";

/// The Settings section.
pub const TEAMS_TITLE: &str = "Teams";

/// What a pending invite's row says instead of a role.
pub const INVITED: &str = "(Invited)";

/// The button that opens the account app's team page.
pub const MANAGE_TEAM: &str = "Manage team";

/// The button on a cancelled plan; the account app (Billing) renews it in Stripe.
pub const RESUME_PLAN: &str = "Resume plan";

/// The button that opens the account app's billing page.
pub const MANAGE_BILLING: &str = "Billing and plans";

/// The button that opens the account app's billing page from the
/// failed-payment notice.
pub const UPDATE_PAYMENT: &str = "Update payment method";

/// The settings row of the failed-payment notice.
pub const PAYMENT_FAILED_ROW: &str = "payment-failed";

/// The line when the account is in no team.
pub const NO_TEAMS: &str =
    "You're not in a team yet. Create one or accept an invite in the account app.";

/// A person on a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamPersonInput {
    pub email: String,
    /// `owner`, `admin` or `member`.
    pub role: String,
}

/// One team, as the Spaces API describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInput {
    pub id: String,
    /// The org's slug.
    pub alias: String,
    /// Its display name (empty: use the alias).
    pub name: String,
    /// The account's role in it.
    pub role: String,
    /// `free`, `teams`, ... After the plan is cancelled but while it still
    /// runs, `teams@<RFC 3339 end>`.
    pub plan: String,
    /// Paid seats.
    pub seats: i64,
    pub members: Vec<TeamPersonInput>,
    /// Pending invites' emails (only owners and admins can read them).
    pub invites: Vec<String>,
    /// The team's last payment failed and it is in its grace period.
    #[serde(default)]
    pub payment_failed: bool,
    /// The end of the grace period (RFC 3339), when known.
    #[serde(default)]
    pub payment_failed_until: Option<String>,
}

/// Where Teams stand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum TeamsState {
    /// Not signed in.
    #[default]
    SignedOut,
    /// Signed in, but this host does not read teams: only the links.
    Unread,
    /// Being read.
    Loading,
    /// The read failed.
    Failed { message: String },
    /// The account's teams (maybe none).
    Ready { teams: Vec<TeamInput> },
}

/// Settings' Teams input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsInput {
    /// The account app for this environment (`https://app.cua.ai`, or
    /// `https://app.staging.cua.ai` under `CUA_ENV=staging`).
    pub account_app_url: String,
    pub state: TeamsState,
    /// The saved current team (`AppSettings::current_team_id`).
    #[serde(default)]
    pub current_team_id: Option<String>,
}

/// The settings row of the team picker; its options are team ids.
pub const CURRENT_TEAM_ROW: &str = "team-current";

/// The team the account works in: the saved one while the account is still
/// in it, else its first owned team, else its first team.
pub fn current<'a>(teams: &'a [TeamInput], saved: Option<&str>) -> Option<&'a TeamInput> {
    saved
        .and_then(|id| teams.iter().find(|t| t.id == id))
        .or_else(|| teams.iter().find(|t| t.role == "owner"))
        .or_else(|| teams.first())
}

impl TeamInput {
    /// The name shown: `name`, else the alias, else the id.
    pub fn display_name(&self) -> &str {
        [&self.name, &self.alias, &self.id]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .unwrap_or("Team")
    }
}

fn role_label(role: &str) -> String {
    match role {
        "owner" => "Owner".into(),
        "admin" => "Admin".into(),
        "member" | "" => "Member".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

/// `plan` and, when the plan is cancelled but still running, its end date.
fn split_plan(plan: &str) -> (&str, Option<String>) {
    match plan.split_once('@') {
        Some((p, ends)) => (p, short_date(ends)),
        None => (plan, None),
    }
}

/// `2026-11-14T12:00:00Z` as `14 Nov 2026` (the date as the API wrote it).
fn short_date(rfc3339: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut d = rfc3339.get(..10)?.split('-');
    let year = d.next()?.parse::<u32>().ok()?;
    let month = d.next()?.parse::<usize>().ok()?;
    let day = d.next()?.parse::<u32>().ok()?;
    Some(format!(
        "{day} {} {year}",
        MONTHS.get(month.checked_sub(1)?)?
    ))
}

fn plan_label(plan: &str) -> String {
    let (plan, ends) = split_plan(plan);
    let name = match plan {
        "teams" | "team" => "Teams",
        "pro" => "Pro",
        // An unknown plan is Free.
        _ => return "Free plan".into(),
    };
    match ends {
        Some(date) => format!("{name} until {date}"),
        None => format!("{name} plan"),
    }
}

/// The notice above a team in grace: owners are told to act (and get the
/// billing button), everyone else who to ask.
fn payment_notice(t: &TeamInput, app: &str) -> SettingsRow {
    let by = t
        .payment_failed_until
        .as_deref()
        .and_then(short_date)
        .map(|d| format!(" by {d}"))
        .unwrap_or_default();
    let mut r = row(PAYMENT_FAILED_ROW, Text, "Payment failed");
    if t.role == "owner" {
        r.value = Some(format!(
            "Your last payment failed. Update your payment method{by} to keep Teams."
        ));
        r.button = Some(UPDATE_PAYMENT.into());
        r.link_url = Some(format!("{app}/billing"));
    } else {
        r.value = Some(format!(
            "The last payment for {} failed. The owner needs to update the payment method{by} to keep Teams.",
            t.display_name()
        ));
    }
    r
}

fn base(input: &TeamsInput) -> String {
    let url = input.account_app_url.trim().trim_end_matches('/');
    if url.starts_with("https://") {
        url.to_string()
    } else {
        ACCOUNT_APP_URL.to_string()
    }
}

/// The Settings rows for Teams (the section's body).
pub fn rows(input: &TeamsInput) -> Vec<SettingsRow> {
    let app = base(input);
    let mut out = Vec::new();
    match &input.state {
        TeamsState::SignedOut => {
            let mut r = row("teams", Text, "Teams");
            r.value = Some("Sign in to see your teams".into());
            out.push(r);
        }
        TeamsState::Unread => {
            let mut r = row("teams", Text, "Teams");
            r.value = Some("Manage your team in the account app".into());
            out.push(r);
        }
        TeamsState::Loading => {
            let mut r = row("teams", Text, "Teams");
            r.value = Some("Loading\u{2026}".into());
            out.push(r);
        }
        TeamsState::Failed { message } => {
            let mut r = row("teams", Text, "Teams");
            r.value = Some(format!("Couldn't load your teams: {message}"));
            out.push(r);
        }
        TeamsState::Ready { teams } if teams.is_empty() => {
            let mut r = row("teams", Text, "Teams");
            r.value = Some(NO_TEAMS.into());
            out.push(r);
        }
        TeamsState::Ready { teams } => {
            let cur = current(teams, input.current_team_id.as_deref());
            if teams.len() > 1 {
                // Several teams: pick the one to work in.
                let mut pick = row(CURRENT_TEAM_ROW, Choice, "Current team");
                pick.options = teams
                    .iter()
                    .map(|t| SettingsOption {
                        id: t.id.clone(),
                        label: t.display_name().to_string(),
                        active: cur.is_some_and(|c| c.id == t.id),
                    })
                    .collect();
                out.push(pick);
            }
            for t in teams
                .iter()
                .filter(|t| teams.len() == 1 || cur.is_some_and(|c| c.id == t.id))
            {
                let seats = match t.seats {
                    0 => String::new(),
                    1 => " \u{b7} 1 seat".into(),
                    n => format!(" \u{b7} {n} seats"),
                };
                if t.payment_failed {
                    out.push(payment_notice(t, &app));
                }
                let mut head = row(&format!("team:{}", t.id), Text, t.display_name());
                head.value = Some(format!(
                    "{} \u{b7} {}{seats}",
                    role_label(&t.role),
                    plan_label(&t.plan)
                ));
                out.push(head);
                if split_plan(&t.plan).1.is_some() {
                    let mut note = row(&format!("team:{}:ending", t.id), Text, "Plan ends");
                    note.value = Some("The team keeps working until then".into());
                    note.button = Some(RESUME_PLAN.into());
                    note.link_url = Some(format!("{app}/billing"));
                    out.push(note);
                }
                for (i, m) in t.members.iter().enumerate() {
                    let mut r = row(&format!("team:{}:member:{i}", t.id), Text, &m.email);
                    r.value = Some(role_label(&m.role));
                    out.push(r);
                }
                for (i, email) in t.invites.iter().enumerate() {
                    let mut r = row(&format!("team:{}:invite:{i}", t.id), Text, email);
                    r.value = Some(INVITED.into());
                    out.push(r);
                }
            }
        }
    }
    // Invites, roles and billing are the account app's.
    let mut manage = row("teams-manage", Text, "Invite people, roles and plans");
    manage.button = Some(MANAGE_TEAM.into());
    manage.link_url = Some(format!("{app}/team"));
    out.push(manage);
    let mut billing = row("teams-billing", Text, "Billing");
    billing.button = Some(MANAGE_BILLING.into());
    billing.link_url = Some(format!("{app}/billing"));
    out.push(billing);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(name: &str, alias: &str) -> TeamInput {
        TeamInput {
            id: "o1".into(),
            alias: alias.into(),
            name: name.into(),
            role: "owner".into(),
            plan: "teams".into(),
            seats: 3,
            members: vec![
                TeamPersonInput {
                    email: "ada@example.com".into(),
                    role: "owner".into(),
                },
                TeamPersonInput {
                    email: "bob@example.com".into(),
                    role: "member".into(),
                },
            ],
            invites: vec!["cy@example.com".into()],
            payment_failed: false,
            payment_failed_until: None,
        }
    }

    fn input(teams: Vec<TeamInput>) -> TeamsInput {
        TeamsInput {
            account_app_url: "https://app.cua.ai/".into(),
            state: TeamsState::Ready { teams },
            current_team_id: None,
        }
    }

    #[test]
    fn a_team_shows_its_name_then_members_then_invites() {
        let rows = rows(&input(vec![team("Acme Inc", "acme")]));
        let flat: Vec<(String, Option<String>)> = rows
            .iter()
            .map(|r| (r.label.clone(), r.value.clone()))
            .collect();
        assert_eq!(
            flat[0],
            (
                "Acme Inc".into(),
                Some("Owner \u{b7} Teams plan \u{b7} 3 seats".into())
            )
        );
        assert_eq!(flat[1], ("ada@example.com".into(), Some("Owner".into())));
        assert_eq!(flat[2], ("bob@example.com".into(), Some("Member".into())));
        assert_eq!(flat[3], ("cy@example.com".into(), Some("(Invited)".into())));
    }

    #[test]
    fn a_cancelled_plan_says_when_it_ends_and_offers_resume() {
        let mut t = team("Acme Inc", "acme");
        t.plan = "teams@2026-11-14T12:00:00Z".into();
        let rows = rows(&input(vec![t]));
        assert_eq!(
            rows[0].value.as_deref(),
            Some("Owner \u{b7} Teams until 14 Nov 2026 \u{b7} 3 seats")
        );
        let ending = rows.iter().find(|r| r.label == "Plan ends").unwrap();
        assert_eq!(
            ending.value.as_deref(),
            Some("The team keeps working until then")
        );
        assert_eq!(ending.button.as_deref(), Some(RESUME_PLAN));
        assert_eq!(
            ending.link_url.as_deref(),
            Some("https://app.cua.ai/billing")
        );
        assert_eq!(
            plan_label("pro@2026-02-03T00:00:00Z"),
            "Pro until 3 Feb 2026"
        );
        assert_eq!(plan_label("teams@garbage"), "Teams plan");
    }

    #[test]
    fn an_owner_in_grace_is_told_to_update_the_payment_method() {
        let mut t = team("Acme Inc", "acme");
        t.payment_failed = true;
        t.payment_failed_until = Some("2026-11-03T10:00:00Z".into());
        let rows = rows(&input(vec![t]));
        assert_eq!(rows[0].id, "payment-failed");
        assert_eq!(
            rows[0].value.as_deref(),
            Some(
                "Your last payment failed. Update your payment method by 3 Nov 2026 to keep Teams."
            )
        );
        assert_eq!(rows[0].button.as_deref(), Some("Update payment method"));
        assert_eq!(
            rows[0].link_url.as_deref(),
            Some("https://app.cua.ai/billing")
        );
        assert_eq!(rows[1].id, "team:o1");
    }

    #[test]
    fn a_member_in_grace_is_told_who_acts_and_gets_no_button() {
        let mut t = team("Acme Inc", "acme");
        t.role = "member".into();
        t.payment_failed = true;
        let rows = rows(&input(vec![t]));
        assert_eq!(
            rows[0].value.as_deref(),
            Some(
                "The last payment for Acme Inc failed. The owner needs to update the payment method to keep Teams."
            )
        );
        assert_eq!(rows[0].button, None);
        assert_eq!(rows[0].link_url, None);
    }

    #[test]
    fn a_team_in_good_standing_has_no_notice() {
        let rows = rows(&input(vec![team("Acme Inc", "acme")]));
        assert!(rows.iter().all(|r| r.id != "payment-failed"));
    }

    #[test]
    fn the_name_falls_back_to_the_alias() {
        let rows = rows(&input(vec![team("", "acme")]));
        assert_eq!(rows[0].label, "acme");
        assert_eq!(team(" ", "").display_name(), "o1");
    }

    #[test]
    fn invites_and_billing_open_the_account_app() {
        let rows = rows(&input(vec![team("Acme", "acme")]));
        let manage = rows.iter().find(|r| r.id == "teams-manage").unwrap();
        assert_eq!(manage.link_url.as_deref(), Some("https://app.cua.ai/team"));
        let billing = rows.iter().find(|r| r.id == "teams-billing").unwrap();
        assert_eq!(
            billing.link_url.as_deref(),
            Some("https://app.cua.ai/billing")
        );
    }

    #[test]
    fn staging_links_stay_on_staging_and_junk_falls_back_to_production() {
        let mut i = input(vec![]);
        i.account_app_url = "https://app.staging.cua.ai".into();
        let r = rows(&i);
        assert_eq!(
            r.last().unwrap().link_url.as_deref(),
            Some("https://app.staging.cua.ai/billing")
        );
        i.account_app_url = "javascript:alert(1)".into();
        assert_eq!(
            rows(&i).last().unwrap().link_url.as_deref(),
            Some("https://app.cua.ai/billing")
        );
    }

    #[test]
    fn no_teams_signed_out_loading_and_failed_each_say_so() {
        let say = |s: TeamsState| {
            rows(&TeamsInput {
                account_app_url: String::new(),
                state: s,
                current_team_id: None,
            })[0]
                .value
                .clone()
                .unwrap()
        };
        assert_eq!(say(TeamsState::Ready { teams: vec![] }), NO_TEAMS);
        assert!(say(TeamsState::SignedOut).contains("Sign in"));
        assert!(say(TeamsState::Loading).contains("Loading"));
        assert!(say(TeamsState::Unread).contains("account app"));
        assert!(
            say(TeamsState::Failed {
                message: "offline".into()
            })
            .contains("offline")
        );
        assert!(!NO_TEAMS.to_lowercase().contains("waitlist"));
    }

    #[test]
    fn nothing_says_coming_soon() {
        for r in rows(&input(vec![team("A", "a")])) {
            let all = format!("{} {:?} {:?}", r.label, r.value, r.button).to_lowercase();
            assert!(
                !all.contains("waitlist") && !all.contains("coming soon"),
                "{all}"
            );
        }
    }

    fn named(id: &str, role: &str) -> TeamInput {
        TeamInput {
            id: id.into(),
            alias: id.into(),
            name: String::new(),
            role: role.into(),
            plan: "free".into(),
            seats: 0,
            members: vec![],
            invites: vec![],
            payment_failed: false,
            payment_failed_until: None,
        }
    }

    #[test]
    fn the_current_team_is_the_saved_one_else_the_first_owned_else_the_first() {
        let teams = vec![
            named("a", "member"),
            named("b", "owner"),
            named("c", "owner"),
        ];
        assert_eq!(current(&teams, Some("c")).unwrap().id, "c");
        assert_eq!(current(&teams, None).unwrap().id, "b");
        // A saved team the account left falls back.
        assert_eq!(current(&teams, Some("gone")).unwrap().id, "b");
        let members = vec![named("a", "member"), named("z", "admin")];
        assert_eq!(current(&members, None).unwrap().id, "a");
        assert!(current(&[], Some("a")).is_none());
    }

    #[test]
    fn two_teams_show_a_picker_and_the_chosen_teams_people() {
        let mut a = team("Acme", "acme");
        a.id = "a".into();
        let mut b = team("Beta", "beta");
        b.id = "b".into();
        b.role = "member".into();
        b.members.truncate(1);
        b.invites.clear();
        let mut i = input(vec![a, b]);
        let r = rows(&i);
        let pick = r.iter().find(|r| r.id == CURRENT_TEAM_ROW).unwrap();
        assert_eq!(pick.kind, Choice);
        let opts: Vec<(&str, bool)> = pick
            .options
            .iter()
            .map(|o| (o.id.as_str(), o.active))
            .collect();
        // Default: the first owned team.
        assert_eq!(opts, [("a", true), ("b", false)]);
        assert!(r.iter().any(|r| r.id == "team:a"));
        assert!(!r.iter().any(|r| r.id == "team:b"));
        i.current_team_id = Some("b".into());
        let r = rows(&i);
        let pick = r.iter().find(|r| r.id == CURRENT_TEAM_ROW).unwrap();
        assert!(pick.options[1].active && !pick.options[0].active);
        assert!(r.iter().any(|r| r.id == "team:b"));
        assert!(!r.iter().any(|r| r.id == "team:a"));
    }

    #[test]
    fn one_team_has_no_picker() {
        let r = rows(&input(vec![team("Acme", "acme")]));
        assert!(!r.iter().any(|r| r.id == CURRENT_TEAM_ROW));
    }
}
