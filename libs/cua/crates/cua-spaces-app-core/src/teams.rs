// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teams as every app shows them: the account's organizations from the
//! Spaces API (`GET /v1/me`, `/v1/orgs/{org}/members`, `/invites`), as
//! Settings rows. A team is its name (the org's `name`, else its alias),
//! then its people: members with their role, pending invites marked
//! "(Invited)". The apps administer nothing: invites, roles and billing
//! live in the account app (`app.cua.ai`), one link away. The data is read
//! by the host (`app_teams`); this module only words it.

use crate::settings::{SettingsRow, SettingsRowKind::Text, row};
use serde::{Deserialize, Serialize};

/// The production account app: billing, plans, team administration.
pub const ACCOUNT_APP_URL: &str = "https://app.cua.ai";

/// The Settings section.
pub const TEAMS_TITLE: &str = "Teams";

/// What a pending invite's row says instead of a role.
pub const INVITED: &str = "(Invited)";

/// The button that opens the account app's team page.
pub const MANAGE_TEAM: &str = "Manage team";

/// The button that opens the account app's billing page.
pub const MANAGE_BILLING: &str = "Billing and plans";

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
    /// `free`, `teams`, ...
    pub plan: String,
    /// Paid seats.
    pub seats: i64,
    pub members: Vec<TeamPersonInput>,
    /// Pending invites' emails (only owners and admins can read them).
    pub invites: Vec<String>,
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

fn plan_label(plan: &str) -> &'static str {
    match plan {
        "teams" | "team" => "Teams plan",
        "pro" => "Pro plan",
        // An unknown plan is Free.
        _ => "Free plan",
    }
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
            for t in teams {
                let seats = match t.seats {
                    0 => String::new(),
                    1 => " \u{b7} 1 seat".into(),
                    n => format!(" \u{b7} {n} seats"),
                };
                let mut head = row(&format!("team:{}", t.id), Text, t.display_name());
                head.value = Some(format!(
                    "{} \u{b7} {}{seats}",
                    role_label(&t.role),
                    plan_label(&t.plan)
                ));
                out.push(head);
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
        }
    }

    fn input(teams: Vec<TeamInput>) -> TeamsInput {
        TeamsInput {
            account_app_url: "https://app.cua.ai/".into(),
            state: TeamsState::Ready { teams },
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
}
