// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The in-app Pro and Teams prompts, as every app shows them. A prompt is a
//! value moment, never a nag: it shows after something that worked, says one
//! plain thing, links to the plan's checkout on the account app, and can be
//! closed.
//!
//! The rules live here so every shell follows the same ones:
//!
//! * At most one prompt every [`PROMPT_GAP_DAYS`] days, across all surfaces.
//! * Never during a live session or an agent run (`quiet`).
//! * A closed prompt stays closed for [`REDISMISS_DAYS`] days.
//! * Free is the default and nothing is cut off, so no prompt blocks work.
//!
//! The plan card in Settings and the plan under the account name are not
//! prompts: they are always there ([`crate::relay_usage::PlanCard`]). The
//! relay meter's 80% and 100% notices are in [`crate::relay_usage`]; the
//! 80% one follows the `quiet` rule too ([`allows`]).

use serde::{Deserialize, Serialize};

use crate::relay_usage::{
    BUY_PRO_PATH, BUY_TEAMS_PATH, PRO_HOURS, PRO_SESSIONS, PlanLink, RelayUsage, buy, hours,
    plan_kind,
};

/// Days between two prompts, whatever their surface.
pub const PROMPT_GAP_DAYS: u64 = 3;

/// Days a closed prompt stays closed.
pub const REDISMISS_DAYS: u64 = 30;

const DAY_MS: u64 = 86_400_000;

/// Where a prompt shows.
pub const SURFACES: [&str; 3] = ["meter", "share", "machine"];

/// What an app remembers between launches, and what is going on now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpsellState {
    /// Now, ms since the epoch.
    pub now_ms: u64,
    /// When any prompt was last shown (none: never).
    pub last_prompt_ms: Option<u64>,
    /// When this surface's prompt was last closed (none: never).
    pub dismissed_ms: Option<u64>,
    /// A live session or an agent run is going.
    pub quiet: bool,
}

/// A prompt as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsellPrompt {
    /// `share` or `machine`.
    pub surface: String,
    /// One or two plain sentences.
    pub text: String,
    /// The button; opens the plan's checkout.
    pub cta: PlanLink,
    /// "Not now" (accessibility label of the close button).
    pub dismiss: String,
}

/// The rules every prompt follows: not while quiet, not within the gap of
/// the last prompt, not while this one is closed.
pub fn allows(s: &UpsellState) -> bool {
    if s.quiet {
        return false;
    }
    let within =
        |at: Option<u64>, days: u64| at.is_some_and(|t| s.now_ms < t.saturating_add(days * DAY_MS));
    !within(s.last_prompt_ms, PROMPT_GAP_DAYS) && !within(s.dismissed_ms, REDISMISS_DAYS)
}

fn link(account_app: &str, label: &str, path: &str, from: &str) -> PlanLink {
    PlanLink {
        label: label.into(),
        url: buy(account_app, path, from),
    }
}

/// The prompt for `surface`, when its moment fits the plan and the rules.
///
/// * `share`: you shared a Space with a person and are not on Teams.
///   (Sharing with named people is free; Teams adds team-owned machines,
///   roles and device approval.)
/// * `machine`: a second machine is reached over the relay and you are on
///   Free.
pub fn prompt(
    surface: &str,
    usage: &RelayUsage,
    account_app: &str,
    state: &UpsellState,
) -> Option<UpsellPrompt> {
    if !usage.metered || !allows(state) {
        return None;
    }
    let kind = plan_kind(&usage.plan);
    match surface {
        "share" if kind != "teams" => Some(UpsellPrompt {
            surface: "share".into(),
            text: "Sharing with people you work with? Teams adds team-owned machines, roles and device approval, from $30 a seat.".into(),
            cta: link(account_app, "See Teams", BUY_TEAMS_PATH, "app_share"),
            dismiss: "Not now".into(),
        }),
        "machine" if kind == "free" => Some(UpsellPrompt {
            surface: "machine".into(),
            text: format!(
                "Two machines now. Free includes {} live sessions and {} relay hours a month. Pro makes it {PRO_SESSIONS} and {PRO_HOURS}, for $18 a month.",
                usage.live_sessions_limit.unwrap_or(2),
                hours(usage.hours_limit.unwrap_or(20.0)),
            ),
            cta: link(account_app, "Get Pro", BUY_PRO_PATH, "app_machine"),
            dismiss: "Not now".into(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "https://app.cua.ai";
    const NOW: u64 = 100 * DAY_MS;

    fn usage(plan: &str) -> RelayUsage {
        RelayUsage {
            metered: true,
            plan: plan.into(),
            live_sessions_limit: Some(2),
            hours_limit: Some(20.0),
            ..Default::default()
        }
    }

    fn state() -> UpsellState {
        UpsellState {
            now_ms: NOW,
            ..Default::default()
        }
    }

    #[test]
    fn share_leads_to_teams_unless_already_on_teams() {
        let p = prompt("share", &usage("free"), APP, &state()).unwrap();
        assert_eq!(
            p.text,
            "Sharing with people you work with? Teams adds team-owned machines, roles and device approval, from $30 a seat."
        );
        assert_eq!(p.cta.url, "https://app.cua.ai/buy/teams?from=app_share");
        assert!(prompt("share", &usage("pro"), APP, &state()).is_some());
        assert!(prompt("share", &usage("team"), APP, &state()).is_none());
    }

    #[test]
    fn a_second_machine_leads_free_to_pro() {
        let p = prompt("machine", &usage("free"), APP, &state()).unwrap();
        assert_eq!(
            p.text,
            "Two machines now. Free includes 2 live sessions and 20 relay hours a month. Pro makes it 5 and 200, for $18 a month."
        );
        assert_eq!(p.cta.label, "Get Pro");
        assert_eq!(p.cta.url, "https://app.cua.ai/buy/pro?from=app_machine");
        assert!(prompt("machine", &usage("pro"), APP, &state()).is_none());
        assert!(prompt("machine", &usage("team"), APP, &state()).is_none());
    }

    #[test]
    fn no_prompt_when_the_relay_meters_nothing_or_the_surface_is_unknown() {
        let none = RelayUsage {
            metered: false,
            ..usage("free")
        };
        assert!(prompt("share", &none, APP, &state()).is_none());
        assert!(prompt("settings", &usage("free"), APP, &state()).is_none());
    }

    #[test]
    fn never_during_a_live_session_or_agent_run() {
        let s = UpsellState {
            quiet: true,
            ..state()
        };
        assert!(!allows(&s));
        assert!(prompt("share", &usage("free"), APP, &s).is_none());
    }

    #[test]
    fn one_prompt_every_three_days() {
        let at = |ago_days: u64| UpsellState {
            last_prompt_ms: Some(NOW - ago_days * DAY_MS + 1),
            ..state()
        };
        assert!(!allows(&at(3)), "a prompt just under three days ago");
        assert!(allows(&UpsellState {
            last_prompt_ms: Some(NOW - 3 * DAY_MS),
            ..state()
        }));
        assert!(allows(&state()));
    }

    #[test]
    fn a_closed_prompt_stays_closed_for_thirty_days() {
        let closed = |ago_days: u64| UpsellState {
            dismissed_ms: Some(NOW - ago_days * DAY_MS),
            ..state()
        };
        assert!(!allows(&closed(29)));
        assert!(allows(&closed(30)));
    }
}
