// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Cloud billing as both apps show it. The apps have no payment
//! onboarding: the website does it (sign-up credit, adding a card). The
//! apps only show the account's credit in Settings (with "Manage billing",
//! the website's billing page) and, when Fleet refuses a new cloud Space
//! because the account is out of credit, one plain line with "Add credit".
//! Local Spaces never depend on any of it.

use serde::{Deserialize, Serialize};

/// The apps show Cua Cloud billing: Settings' Billing row and a refused
/// cloud Space's "Add credit". Off while the apps do not offer Cua Cloud
/// ([`crate::model::CLOUD_SPACES_OFFERED`]); the SDK's billing calls stay.
pub const BILLING_SHOWN: bool = crate::model::CLOUD_SPACES_OFFERED;

/// The saved card (never its number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingCard {
    /// `visa`, `mastercard`, ...
    pub brand: String,
    /// Last four digits.
    pub last4: String,
}

/// The account's Cua Cloud credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingCredit {
    /// What is left, US cents.
    pub balance_usd_cents: i64,
}

/// The account's billing (the SDK's `Fleet.billing_status()`, Fleet's
/// `GET /api/billing/status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingStatus {
    /// Billing is on for this account (off: Settings shows no Billing row).
    pub billing_enabled: bool,
    /// The saved default card.
    #[serde(default)]
    pub card: Option<BillingCard>,
    /// The account's credit, when it has any.
    #[serde(default)]
    pub credit: Option<BillingCredit>,
    /// The website billing page (credit, cards, plans).
    #[serde(default)]
    pub billing_url: Option<String>,
}

/// The line a refused cloud Space shows.
pub const OUT_OF_CREDIT: &str = "You're out of Cua Cloud credit.";

/// Its button.
pub const ADD_CREDIT: &str = "Add credit";

/// "Manage billing" (Settings).
pub const MANAGE_BILLING: &str = "Manage billing";

/// A cloud Space Fleet refused for want of credit: the line, the button,
/// and the website billing page it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditNotice {
    /// [`OUT_OF_CREDIT`].
    pub text: String,
    /// [`ADD_CREDIT`].
    pub button: String,
    /// The website billing page.
    pub url: String,
}

/// The out-of-credit notice for an SDK error, when that is what `raw` is:
/// the SDK's `CloudCreditExhausted` message ("You're out of Cua Cloud
/// credit. Add credit at <url>"), with or without a variant prefix.
pub fn credit_notice(raw: &str) -> Option<CreditNotice> {
    let lower = raw.to_ascii_lowercase();
    if !lower.contains("out of cua cloud credit") {
        return None;
    }
    let marker = "add credit at ";
    let at = lower.find(marker)? + marker.len();
    let url = raw[at..]
        .split_whitespace()
        .next()?
        .trim_end_matches(['.', ')', ',', '"', '\'']);
    (url.starts_with("https://") || url.starts_with("http://")).then(|| CreditNotice {
        text: OUT_OF_CREDIT.into(),
        button: ADD_CREDIT.into(),
        url: url.into(),
    })
}

/// `$10`, `$7.42`, `-$0.30`.
pub fn dollars(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let c = cents.unsigned_abs();
    if c.is_multiple_of(100) {
        format!("{sign}${}", c / 100)
    } else {
        format!("{sign}${}.{:02}", c / 100, c % 100)
    }
}

/// `Visa ending 4242`.
pub fn card_text(card: &BillingCard) -> String {
    let brand = match card.brand.to_ascii_lowercase().as_str() {
        "amex" | "american_express" => "American Express".to_string(),
        "mastercard" => "Mastercard".to_string(),
        "" | "unknown" => "Card".to_string(),
        b => {
            let mut c = b.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    };
    format!("{brand} ending {}", card.last4)
}

/// Settings' Billing line: `$7.42 credit left`, else the card, else
/// `No card`.
pub fn billing_line(status: &BillingStatus) -> String {
    match (&status.credit, &status.card) {
        (Some(c), _) => format!("{} credit left", dollars(c.balance_usd_cents.max(0))),
        (None, Some(card)) => card_text(card),
        (None, None) => "No card".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(dollars(1000), "$10");
        assert_eq!(dollars(742), "$7.42");
        assert_eq!(dollars(-30), "-$0.30");
        let visa = BillingCard {
            brand: "visa".into(),
            last4: "4242".into(),
        };
        assert_eq!(card_text(&visa), "Visa ending 4242");
        let mut s = BillingStatus {
            billing_enabled: true,
            card: None,
            credit: Some(BillingCredit {
                balance_usd_cents: 742,
            }),
            billing_url: Some("https://run.cua.ai/billing".into()),
        };
        assert_eq!(billing_line(&s), "$7.42 credit left");
        s.credit = None;
        assert_eq!(billing_line(&s), "No card");
        s.card = Some(visa);
        assert_eq!(billing_line(&s), "Visa ending 4242");
    }

    #[test]
    fn out_of_credit_errors_become_one_line_and_a_button() {
        for raw in [
            "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing",
            "cloud_credit_exhausted: You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing.",
            "CloudCreditExhausted(\"You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing\")",
        ] {
            assert_eq!(
                credit_notice(raw),
                Some(CreditNotice {
                    text: OUT_OF_CREDIT.into(),
                    button: ADD_CREDIT.into(),
                    url: "https://run.cua.ai/billing".into(),
                }),
                "{raw}"
            );
        }
        for raw in [
            "fleet admission denied: sandbox size is over the Fleet limits",
            "You're out of Cua Cloud credit.",
            "out of Cua Cloud credit. Add credit at javascript:alert(1)",
            "no local runtime",
        ] {
            assert_eq!(credit_notice(raw), None, "{raw}");
        }
    }
}
