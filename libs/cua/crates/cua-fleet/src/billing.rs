//! Cua Cloud billing from an app: the account's billing state.
//!
//! Fleet (trycua/cloud `cyclops-cs/backend/handlers/billing.go`) owns every
//! payment detail: the app never sees a card number. It reads
//! `GET /api/billing/status`, which carries the saved card, the plan, the
//! account's Cua Cloud credit (a signup grant, then its balance) and the
//! website billing page where cards are saved and plans are bought. Apps
//! open that page; Fleet refuses app tokens on its Stripe session routes
//! (trycua/cloud#7933).

use crate::{Error, FleetClient, Result, SdkError};
use serde::{Deserialize, Serialize};

/// The saved card, as Fleet describes it (never its number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCard {
    /// `visa`, `mastercard`, ...
    pub brand: String,
    /// Last four digits.
    pub last4: String,
    /// Expiry month (1-12).
    pub exp_month: u32,
    /// Expiry year.
    pub exp_year: u32,
}

/// The account's Cua Cloud credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCredit {
    /// What is left, US cents.
    pub balance_usd_cents: i64,
    /// The signup grant's amount, US cents (0: none).
    #[serde(default)]
    pub signup_grant_usd_cents: i64,
    /// A signup grant exists and none of it has been used.
    #[serde(default)]
    pub signup_grant_unused: bool,
}

/// `GET /api/billing/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingStatus {
    /// Billing is on for this account (off: nothing to set up).
    pub billing_enabled: bool,
    /// A default card is saved.
    pub payment_method_present: bool,
    /// That card.
    #[serde(default)]
    pub card: Option<BillingCard>,
    /// `none`, `payg`, or a plan key.
    #[serde(default = "none_plan")]
    pub plan: String,
    /// A card can be added for pay as you go.
    #[serde(default)]
    pub payg_available: bool,
    /// The account's credit (none when Fleet has no credit for it).
    #[serde(default)]
    pub credit: Option<BillingCredit>,
    /// The website billing page (plans, pay as you go, cards).
    #[serde(default)]
    pub billing_url: Option<String>,
}

fn none_plan() -> String {
    "none".into()
}

impl BillingStatus {
    /// What a Fleet without the status route means: billing off.
    pub fn disabled() -> Self {
        Self {
            billing_enabled: false,
            payment_method_present: false,
            card: None,
            plan: none_plan(),
            payg_available: false,
            credit: None,
            billing_url: None,
        }
    }
}

impl FleetClient {
    fn billing_url(&self, path: &str) -> String {
        format!(
            "{}/api/billing/{path}",
            self.config().base_url.trim_end_matches('/')
        )
    }

    /// The account's billing state. A Fleet without the route (404) or
    /// with billing off answers [`BillingStatus::disabled`].
    pub async fn billing_status(&self) -> Result<BillingStatus> {
        match self.raw("GET", self.billing_url("status"), None).await? {
            (200, body) => serde_json::from_value(body).map_err(|e| {
                Error::InvalidArgument(format!("unexpected billing status from Fleet: {e}"))
            }),
            (404, _) => Ok(BillingStatus::disabled()),
            (status, body) => {
                Err(
                    SdkError::status("read billing status", status, body.to_string().as_bytes())
                        .into(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_status_parses_with_defaults() {
        let s: BillingStatus = serde_json::from_value(json!({
            "billing_enabled": true, "payment_method_present": false
        }))
        .unwrap();
        assert_eq!((s.plan.as_str(), s.credit, s.card), ("none", None, None));
    }
}
