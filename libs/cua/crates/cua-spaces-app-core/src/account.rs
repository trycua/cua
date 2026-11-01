// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account line at the sidebar's foot: avatar and name (or email), no
//! "Signed in as", no plan line, no sign-out icon. Clicking it opens this
//! menu: Settings, Plan (with the current plan's word, opening the plan
//! card's link), then Sign out, set apart, when signing out applies (a
//! user's own session, not an API key or a fixed token).
//!
//! The Plan item's click is the existing `upsell_account_menu_click` usage
//! event (`telemetry.upsell` with surface `account_menu`).

use serde::{Deserialize, Serialize};

/// What the menu reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountMenuInput {
    /// The plan, as the relay names it (`free`, `pro`, `team`; empty: not
    /// known yet).
    pub plan: String,
    /// Signed in to a Cua account.
    pub signed_in: bool,
    /// How (`user`, `client-credentials`, `static-token`, `none`): only a
    /// user's session signs out.
    pub auth_mode: String,
}

/// What an item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountMenuItemId {
    /// Opens Settings.
    Settings,
    /// Opens the plan card's link (the account app).
    Plan,
    /// Signs out.
    SignOut,
}

/// One item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountMenuItem {
    /// What it does.
    pub id: AccountMenuItemId,
    /// "Settings", "Plan", "Sign out".
    pub label: String,
    /// Muted, on the right: the plan ("Free", "Pro", "Teams").
    pub value: Option<String>,
    /// A separator goes above it.
    pub separator_before: bool,
}

/// "Free", "Pro", "Teams"; none for a plan not known.
pub fn plan_word(plan: &str) -> Option<String> {
    match crate::relay_usage::plan_word(plan) {
        "free" => Some("Free".into()),
        "pro" => Some("Pro".into()),
        "teams" => Some("Teams".into()),
        _ => None,
    }
}

fn item(id: AccountMenuItemId, label: &str) -> AccountMenuItem {
    AccountMenuItem {
        id,
        label: label.into(),
        value: None,
        separator_before: false,
    }
}

/// The account menu.
pub fn menu(input: &AccountMenuInput) -> Vec<AccountMenuItem> {
    let mut items = vec![item(AccountMenuItemId::Settings, "Settings")];
    if !input.signed_in {
        return items;
    }
    let mut plan = item(AccountMenuItemId::Plan, "Plan");
    plan.value = plan_word(&input.plan);
    items.push(plan);
    if input.auth_mode == "user" {
        let mut out = item(AccountMenuItemId::SignOut, "Sign out");
        out.separator_before = true;
        items.push(out);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[AccountMenuItem]) -> Vec<String> {
        v.iter()
            .map(|i| {
                format!(
                    "{}{}{}",
                    if i.separator_before { "--- " } else { "" },
                    i.label,
                    i.value
                        .as_deref()
                        .map(|v| format!(" = {v}"))
                        .unwrap_or_default()
                )
            })
            .collect()
    }

    fn signed_in(plan: &str) -> AccountMenuInput {
        AccountMenuInput {
            plan: plan.into(),
            signed_in: true,
            auth_mode: "user".into(),
        }
    }

    #[test]
    fn settings_plan_then_sign_out() {
        assert_eq!(
            lines(&menu(&signed_in("free"))),
            ["Settings", "Plan = Free", "--- Sign out"]
        );
        assert_eq!(lines(&menu(&signed_in("pro")))[1], "Plan = Pro");
        assert_eq!(lines(&menu(&signed_in("team")))[1], "Plan = Teams");
        assert_eq!(lines(&menu(&signed_in("")))[1], "Plan");
    }

    #[test]
    fn sign_out_only_for_a_users_session() {
        for mode in ["client-credentials", "static-token", "none", ""] {
            let m = menu(&AccountMenuInput {
                auth_mode: mode.into(),
                ..signed_in("free")
            });
            assert_eq!(lines(&m), ["Settings", "Plan = Free"], "{mode}");
        }
        assert_eq!(
            lines(&menu(&AccountMenuInput::default())),
            ["Settings"],
            "signed out"
        );
    }

    #[test]
    fn ids_on_the_wire() {
        let v = serde_json::to_value(menu(&signed_in("free"))).unwrap();
        assert_eq!(v[2]["id"], "signOut");
        assert_eq!(v[2]["separatorBefore"], true);
    }
}
