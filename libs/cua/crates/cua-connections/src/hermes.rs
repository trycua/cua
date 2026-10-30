//! Hermes's `config.yaml`, generated from the Connections.
//!
//! Hermes's own `auto` picks a provider by its registry order and borrows the
//! logins of other CLIs it finds; neither is what the person connected. The
//! file written here names the provider (and model) explicitly, lists the
//! other connected providers as fallbacks in the person's order, and turns
//! `auth.adopt_external_logins` off so Hermes never takes another app's
//! sign-in. Credentials are not in it: Hermes reads them from the run's
//! environment under the names the registry delivers.

use crate::{Connected, Provider, provider, visible};

/// One provider Hermes may use, in the order of use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub provider: &'static Provider,
    pub region: Option<String>,
    pub model: String,
    /// A base URL to set (a plan or region, or a custom endpoint).
    pub base_url: Option<String>,
    /// The key is saved (a custom endpoint may have none).
    pub key: bool,
}

/// The providers Hermes can use from `connected`: only those that deliver a
/// variable to it (never a sign-in scoped to another app) and whose key is
/// saved (`present`), in rank order.
pub fn select(connected: &[Connected], present: &dyn Fn(&str) -> bool) -> Vec<Pick> {
    let mut picks = vec![];
    for p in visible() {
        let Some(c) = connected.iter().find(|c| c.provider == p.id) else {
            continue;
        };
        let entries = p.delivery_for("hermes");
        if entries.is_empty() || p.hermes_id.is_empty() {
            continue;
        }
        let key = entries
            .iter()
            .filter(|e| p.scope_of(e.src).is_none())
            .any(|e| present(e.src));
        let optional = p.key_method().is_some_and(|m| m.secret_optional);
        if !key && !optional {
            continue;
        }
        let model = c
            .fields
            .get("model")
            .cloned()
            .unwrap_or_else(|| p.default_model.to_string());
        let base_url = if p.kind == crate::Kind::Custom {
            c.fields.get("base_url").cloned()
        } else {
            c.region
                .as_deref()
                .map(|r| p.base_url_for(Some(r)))
                .filter(|u| *u != p.base_url)
                .map(str::to_string)
        };
        if p.kind == crate::Kind::Custom && base_url.is_none() {
            continue;
        }
        picks.push(Pick {
            provider: p,
            region: c.region.clone(),
            model,
            base_url,
            key,
        });
    }
    picks
}

fn q(s: &str) -> String {
    // A JSON string is a valid YAML double-quoted scalar.
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The `config.yaml` for `picks` (the first is the provider Hermes uses, the
/// rest its fallbacks), or `None` when there is nothing to name.
pub fn config_yaml(picks: &[Pick]) -> Option<String> {
    let first = picks.first()?;
    let mut y = String::from(
        "# Written by Cua from your Connections each time a run starts. Change the\n\
         # connections in Cua, not this file.\n",
    );
    y.push_str("model:\n");
    y.push_str(&format!("  provider: {}\n", q(first.provider.hermes_id)));
    if !first.model.is_empty() {
        y.push_str(&format!("  default: {}\n", q(&first.model)));
    }
    if let Some(b) = &first.base_url {
        y.push_str(&format!("  base_url: {}\n", q(b)));
    }
    if first.provider.kind == crate::Kind::Custom {
        let key = if first.key {
            "${CUSTOM_ENDPOINT_API_KEY}".to_string()
        } else {
            "no-key-required".to_string()
        };
        y.push_str(&format!("  api_key: {}\n", q(&key)));
        y.push_str("  context_length: 131072\n");
    }
    let fallbacks: Vec<&Pick> = picks[1..]
        .iter()
        .filter(|p| !p.model.is_empty() && p.provider.kind != crate::Kind::Custom)
        .collect();
    if fallbacks.is_empty() {
        y.push_str("fallback_providers: []\n");
    } else {
        y.push_str("fallback_providers:\n");
        for f in fallbacks {
            y.push_str(&format!("  - provider: {}\n", q(f.provider.hermes_id)));
            y.push_str(&format!("    model: {}\n", q(&f.model)));
            if let Some(b) = &f.base_url {
                y.push_str(&format!("    base_url: {}\n", q(b)));
            }
        }
    }
    y.push_str("auth:\n  adopt_external_logins: false\n");
    Some(y)
}

/// The provider ids in `connected` Hermes would use, first to last
/// (for the app's page and the CLI).
pub fn order(connected: &[Connected], present: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    select(connected, present)
        .into_iter()
        .map(|p| p.provider.id)
        .filter(|id| provider(id).is_some())
        .collect()
}
