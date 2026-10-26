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

/// Where a prompt shows. `onboarding` is the first run's plan card after the
/// first success ([`crate::first_run::upsell`]): its own rules (once, and
/// "Not now" is for good).
pub const SURFACES: [&str; 4] = ["meter", "share", "machine", "onboarding"];

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
            text: "Sharing with people you work with? Teams adds team-owned computers, roles and device approval, from $30 a seat.".into(),
            cta: link(account_app, "See Teams", BUY_TEAMS_PATH, "app_share"),
            dismiss: "Not now".into(),
        }),
        "machine" if kind == "free" => Some(UpsellPrompt {
            surface: "machine".into(),
            text: format!(
                "Two computers now. Free includes {} live sessions and {} remote hours a month. Pro makes it {PRO_SESSIONS} and {PRO_HOURS}, for $18 a month.",
                usage.live_sessions_limit.unwrap_or(2),
                hours(usage.hours_limit.unwrap_or(20.0)),
            ),
            cta: link(account_app, "Get Pro", BUY_PRO_PATH, "app_machine"),
            dismiss: "Not now".into(),
        }),
        _ => None,
    }
}

/// Where a limit card can show, and what it is about (`card`). The matching
/// usage events are `upsell_card_<reason>_<action>`.
pub const CARD_REASONS: [&str; 5] = ["hours", "sessions", "invite", "seats", "machines"];

/// What a Teams seat costs, as the card says it.
const TEAMS_PRICE: &str = "$30 a seat a month";

/// A card for the moment a limit stops the person: what happened, the one
/// line of what the plan gives, the price, and one button to the checkout.
/// Unlike a [`UpsellPrompt`] it answers something the person just ran into,
/// so the 3-day gap and quiet rules do not apply; "Not now" closes it for the
/// session. Nothing here enforces the limit: the relay and the control plane
/// do, and the app only says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsellCard {
    /// One of [`CARD_REASONS`].
    pub reason: String,
    /// What happened.
    pub title: String,
    /// What the plan gives, one or two plain sentences.
    pub body: String,
    /// "$18 a month".
    pub price: String,
    /// The button; opens the plan's checkout.
    pub cta: PlanLink,
    /// "Not now".
    pub dismiss: String,
}

/// The card for `reason` on this plan, or none when there is nothing to buy
/// for it (a plan that already has what the card would offer).
///
/// * `hours`, `sessions`: the relay's allowance is used. Free leads to Pro,
///   Pro to Teams, a team to its seats.
/// * `invite`: sharing needs a team (`sharing_needs_team`). Free and Pro lead
///   to Teams.
/// * `seats`, `machines`: a team is out of seats, or of computers (one per
///   seat). Only for someone who can add seats (the host words the error for
///   an owner), so the shell passes them only then.
pub fn card(reason: &str, usage: &RelayUsage, account_app: &str) -> Option<UpsellCard> {
    let kind = plan_kind(&usage.plan);
    let to = |label: &str, path: &str, from: &str| link(account_app, label, path, from);
    let from = format!("app_card_{reason}");
    let (title, body, price, cta) = match (reason, kind) {
        ("hours", "free") => (
            "You've used this month's remote hours".to_owned(),
            format!(
                "Free includes {} remote hours a month. Your Spaces keep working. Pro gives you {PRO_HOURS} hours and {PRO_SESSIONS} live sessions.",
                hours(usage.hours_limit.unwrap_or(20.0)),
            ),
            "$18 a month".to_owned(),
            to("Upgrade to Pro", BUY_PRO_PATH, &from),
        ),
        ("hours", "pro") => (
            "You've used this month's remote hours".to_owned(),
            "Your Spaces keep working. Teams pools remote hours across your seats.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Upgrade to Teams", BUY_TEAMS_PATH, &from),
        ),
        ("sessions", "free") => (
            format!(
                "You're using {} of {} live sessions",
                usage.live_sessions,
                usage.live_sessions_limit.unwrap_or(2),
            ),
            format!("Close one to start another, or get Pro: {PRO_SESSIONS} live sessions at once and {PRO_HOURS} remote hours a month."),
            "$18 a month".to_owned(),
            to("Upgrade to Pro", BUY_PRO_PATH, &from),
        ),
        ("sessions", "pro") => (
            format!(
                "You're using {} of {} live sessions",
                usage.live_sessions,
                usage.live_sessions_limit.unwrap_or(PRO_SESSIONS),
            ),
            "Close one to start another, or move to Teams for a shared allowance across your seats.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Upgrade to Teams", BUY_TEAMS_PATH, &from),
        ),
        ("hours" | "sessions", "teams") => (
            "Your team has used its remote allowance".to_owned(),
            "Your Spaces keep working. Each seat adds to the team's hours and live sessions.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Add seats", "/billing", &from),
        ),
        ("invite", "free" | "pro") => (
            "Sharing is for teams".to_owned(),
            "Teams lets you share a Space with the people you work with, with roles and device approval.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Start a team", BUY_TEAMS_PATH, &from),
        ),
        ("seats", "free" | "pro" | "teams") => (
            "Your team is out of seats".to_owned(),
            "A seat covers one person and one team computer. Add a seat to invite someone else.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Add seats", "/billing", &from),
        ),
        ("machines", "free" | "pro" | "teams") => (
            "Your team has used all its computers".to_owned(),
            "A team gets one computer for each seat. Add a seat to add another computer.".to_owned(),
            TEAMS_PRICE.to_owned(),
            to("Add seats", "/billing", &from),
        ),
        _ => return None,
    };
    Some(UpsellCard {
        reason: reason.into(),
        title,
        body,
        price,
        cta,
        dismiss: "Not now".into(),
    })
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
            "Sharing with people you work with? Teams adds team-owned computers, roles and device approval, from $30 a seat."
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
            "Two computers now. Free includes 2 live sessions and 20 remote hours a month. Pro makes it 5 and 200, for $18 a month."
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

    #[test]
    fn limit_cards_say_what_happened_what_the_plan_gives_and_the_price() {
        let mut u = usage("free");
        u.hours_used = 20.0;
        let c = card("hours", &u, APP).unwrap();
        assert_eq!(c.title, "You've used this month's remote hours");
        assert_eq!(
            c.body,
            "Free includes 20 remote hours a month. Your Spaces keep working. Pro gives you 200 hours and 5 live sessions."
        );
        assert_eq!(c.price, "$18 a month");
        assert_eq!(c.cta.url, "https://app.cua.ai/buy/pro?from=app_card_hours");
        assert_eq!(c.dismiss, "Not now");

        u.live_sessions = 2;
        let s = card("sessions", &u, APP).unwrap();
        assert_eq!(s.title, "You're using 2 of 2 live sessions");
        assert_eq!(s.cta.label, "Upgrade to Pro");

        let t = card("invite", &u, APP).unwrap();
        assert_eq!(t.price, "$30 a seat a month");
        assert_eq!(
            t.cta.url,
            "https://app.cua.ai/buy/teams?from=app_card_invite"
        );
    }

    #[test]
    fn a_card_leads_each_plan_one_step_up_and_never_offers_what_it_has() {
        let pro = card("hours", &usage("pro"), APP).unwrap();
        assert_eq!(
            pro.cta.url,
            "https://app.cua.ai/buy/teams?from=app_card_hours"
        );
        assert!(card("invite", &usage("team"), APP).is_none());
        assert!(card("hours", &usage("enterprise"), APP).is_none());
        assert!(card("nonsense", &usage("free"), APP).is_none());
        let seats = card("seats", &usage("team"), APP).unwrap();
        assert_eq!(seats.cta.url, "https://app.cua.ai/billing");
        assert!(card("machines", &usage("team"), APP).is_some());
    }

    #[test]
    fn card_words_are_plain() {
        for plan in ["free", "pro", "team"] {
            for reason in CARD_REASONS {
                let Some(c) = card(reason, &usage(plan), APP) else {
                    continue;
                };
                let all = format!("{} {} {}", c.title, c.body, c.price).to_lowercase();
                for bad in [
                    "relay",
                    "entitlement",
                    "quota",
                    "!",
                    "hurry",
                    "limited time",
                ] {
                    assert!(!all.contains(bad), "{reason}/{plan}: {bad}");
                }
            }
        }
    }
}
