//! Cua Cloud usage rates, read from Fleet.
//!
//! Fleet bills a sandbox for the vCPUs and memory its template reserves,
//! per hour (trycua/cloud `cyclops-cs/backend/metering`, Stripe meters
//! `cua_vcpu_hours` and `cua_gib_hours`). The rates a caller pays come from
//! `GET /api/config` (`usage_pricing`), which evaluates them per account.
//! Disk and GPU are not metered.
//!
//! Nothing here guesses a price: when Fleet does not answer with positive
//! rates, [`FleetClient::usage_pricing`] returns `None`.

use crate::{FleetClient, Result};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// How long an answer is reused. Rates change rarely; a wizard that redraws
/// on every slider move must not call Fleet each time.
pub const PRICING_TTL: Duration = Duration::from_secs(300);

/// One GiB in MiB (memory is reserved in MiB, billed in GiB-hours).
const MIB_PER_GIB: f64 = 1024.0;

/// Cua Cloud rates in USD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsagePricing {
    /// USD per reserved vCPU per hour.
    pub vcpu_hour_usd: f64,
    /// USD per reserved GiB of memory per hour.
    pub memory_gib_hour_usd: f64,
}

impl UsagePricing {
    /// Parses the `usage_pricing` object of `GET /api/config`. `None`
    /// unless both rates are positive finite numbers.
    pub fn from_config(config: &Value) -> Option<Self> {
        let p = &config["usage_pricing"];
        let rate = |key: &str| p[key].as_f64().filter(|v| v.is_finite() && *v > 0.0);
        Some(Self {
            vcpu_hour_usd: rate("vcpu_hour_usd")?,
            memory_gib_hour_usd: rate("memory_gib_hour_usd")?,
        })
    }

    /// USD per hour for a sandbox that reserves `cpus` vCPUs and
    /// `memory_mib` MiB.
    pub fn hourly_usd(&self, cpus: u32, memory_mib: u32) -> f64 {
        f64::from(cpus) * self.vcpu_hour_usd
            + f64::from(memory_mib) / MIB_PER_GIB * self.memory_gib_hour_usd
    }
}

fn cache() -> &'static Mutex<HashMap<String, (Instant, UsagePricing)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, UsagePricing)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

impl FleetClient {
    /// The caller's Cua Cloud rates (`GET /api/config`), reused for
    /// [`PRICING_TTL`] per Fleet base URL. `Ok(None)` when Fleet answers
    /// without usable rates; transport and auth failures are errors. Only
    /// answers with rates are cached.
    pub async fn usage_pricing(&self) -> Result<Option<UsagePricing>> {
        let base = self.config().base_url.trim_end_matches('/').to_string();
        if let Some((at, p)) = cache().lock().ok().and_then(|c| c.get(&base).copied())
            && at.elapsed() < PRICING_TTL
        {
            return Ok(Some(p));
        }
        let (status, body) = self.raw("GET", format!("{base}/api/config"), None).await?;
        if status != 200 {
            return Err(crate::SdkError::status(
                "read usage pricing",
                status,
                body.to_string().as_bytes(),
            )
            .into());
        }
        let pricing = UsagePricing::from_config(&body);
        if let (Some(p), Ok(mut c)) = (pricing, cache().lock()) {
            c.insert(base, (Instant::now(), p));
        }
        Ok(pricing)
    }
}

/// Forgets cached rates (sign-out, tests).
pub fn clear_pricing_cache() {
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_positive_rates_only() {
        let ok =
            json!({"usage_pricing": {"vcpu_hour_usd": 0.044625, "memory_gib_hour_usd": 0.0223125}});
        assert_eq!(
            UsagePricing::from_config(&ok),
            Some(UsagePricing {
                vcpu_hour_usd: 0.044625,
                memory_gib_hour_usd: 0.0223125
            })
        );
        for bad in [
            json!({}),
            json!({"usage_pricing": {"vcpu_hour_usd": 0.04}}),
            json!({"usage_pricing": {"vcpu_hour_usd": 0, "memory_gib_hour_usd": 0.02}}),
            json!({"usage_pricing": {"vcpu_hour_usd": -1, "memory_gib_hour_usd": 0.02}}),
            json!({"usage_pricing": {"vcpu_hour_usd": "0.04", "memory_gib_hour_usd": 0.02}}),
        ] {
            assert_eq!(UsagePricing::from_config(&bad), None, "{bad}");
        }
    }

    #[test]
    fn hourly_cost_is_vcpus_plus_gib() {
        let p = UsagePricing {
            vcpu_hour_usd: 0.044625,
            memory_gib_hour_usd: 0.0223125,
        };
        // 2 vCPUs + 4 GiB: 0.08925 + 0.08925.
        assert!((p.hourly_usd(2, 4096) - 0.1785).abs() < 1e-12);
        assert!((p.hourly_usd(1, 512) - (0.044625 + 0.0223125 / 2.0)).abs() < 1e-12);
    }
}
