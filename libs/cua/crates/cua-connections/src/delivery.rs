//! Which saved credentials reach a run of which app: the one place that
//! decides, from the provider files.

use std::collections::BTreeMap;

use crate::{Connected, ENDPOINT_HARNESSES, HARNESS_IDS, Provider, provider, visible};

/// The variables a run of `harness` receives, given the saved names and
/// values (`lookup` answers for one saved name, `None` when not saved, empty
/// or no longer usable).
///
/// Providers are read in rank order and the first to fill a variable wins. A
/// name that only a scoped sign-in saves (the Claude Code sign-in, the ChatGPT
/// sign-in) is skipped for every app but its own, even if a provider file
/// listed it (the generator refuses such a file too).
pub fn deliver(harness: &str, lookup: &dyn Fn(&str) -> Option<String>) -> BTreeMap<String, String> {
    deliver_from(harness, None, lookup)
}

/// [`deliver`] from one provider only (`only`: the provider the person told the
/// app to use), or from every provider (`None`).
pub fn deliver_from(
    harness: &str,
    only: Option<&str>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for p in visible().filter(|p| only.is_none_or(|o| o == p.id)) {
        for e in p.delivery_for(harness) {
            if p.scope_of(e.src).is_some_and(|s| s != harness) {
                continue;
            }
            if out.contains_key(e.dest) {
                continue;
            }
            if let Some(v) = lookup(e.src).filter(|v| !v.is_empty()) {
                out.insert(e.dest.to_string(), v);
            }
        }
    }
    out
}

/// Whether `harness` can use a saved credential named `env`: a name it reads,
/// or the key of a provider it is pointed at by endpoint. A sign-in scoped to
/// another app (Claude Code's, Codex's) is never one.
pub fn can_read(harness: &str, env: &str) -> bool {
    if crate::reads(harness).contains(&env) {
        return true;
    }
    let Some(p) = crate::provider_of_env(env).filter(|p| p.visible()) else {
        return false;
    };
    p.scope_of(env).is_none()
        && p.delivery_for(harness).is_empty()
        && p.delivery.iter().all(|d| d.harness != harness)
        && endpoint_wire(p, harness).is_some()
        && p.key_method()
            .is_some_and(|m| m.env == env || m.aliases.contains(&env))
}

/// The apps that can use `p`, in the order of the Connections page: those it
/// delivers a variable to, then (for providers an app has no variable for)
/// the apps that can be pointed at its wire with a base URL and a model.
pub fn works_with(p: &Provider) -> Vec<&'static str> {
    HARNESS_IDS
        .iter()
        .copied()
        .filter(|h| {
            !p.delivery_for(h).is_empty()
                || (p.delivery.iter().all(|d| d.harness != *h) && endpoint_wire(p, h).is_some())
        })
        .collect()
}

/// The wire an endpoint harness would use for `p`, when it can.
fn endpoint_wire(p: &Provider, harness: &str) -> Option<&'static str> {
    let (_, wire) = ENDPOINT_HARNESSES.iter().find(|(h, _)| *h == harness)?;
    let serves = p.wire == *wire || p.also_wires.contains(wire);
    let usable =
        p.kind == crate::Kind::Custom || (!p.base_url.is_empty() && !p.default_model.is_empty());
    (serves && usable && p.visible()).then_some(*wire)
}

/// An endpoint a run is pointed at because a connection is for an app that
/// has no variable of its own for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointPlan {
    pub provider: &'static str,
    pub base_url: String,
    pub wire: &'static str,
    pub model: String,
    /// The saved name the key is read from (`None`: a server that needs none).
    pub key_src: Option<&'static str>,
}

/// The endpoint `harness` is pointed at from `connected` when none of its
/// own variables is available: the first connected provider (rank order)
/// that serves the harness's wire, has a base URL and a model, and whose key
/// is saved (`present`). `None` for a harness that is not an endpoint
/// harness, or when nothing qualifies.
pub fn endpoint_for(
    harness: &str,
    connected: &[Connected],
    present: &dyn Fn(&str) -> bool,
) -> Option<EndpointPlan> {
    let mut candidates: Vec<(&'static Provider, &Connected)> = connected
        .iter()
        .filter_map(|c| provider(&c.provider).map(|p| (p, c)))
        .filter(|(p, _)| p.visible() && p.delivery_for(harness).is_empty())
        .collect();
    candidates.sort_by_key(|(p, _)| p.rank);
    for (p, c) in candidates {
        let custom = p.kind == crate::Kind::Custom;
        let wire = if custom {
            let w = c.fields.get("wire").map(String::as_str).unwrap_or(p.wire);
            match endpoint_wire_of(harness) {
                Some(hw) if hw == w => hw,
                _ => continue,
            }
        } else {
            match endpoint_wire(p, harness) {
                Some(w) => w,
                None => continue,
            }
        };
        let Some(m) = p.key_method() else {
            continue;
        };
        let key_present = present(m.env) || m.aliases.iter().any(|a| present(a));
        let optional = m.secret_optional;
        if !key_present && !optional {
            continue;
        }
        let (base_url, model) = if custom {
            (
                c.fields.get("base_url").cloned().unwrap_or_default(),
                c.fields.get("model").cloned().unwrap_or_default(),
            )
        } else {
            (
                p.base_url_for(c.region.as_deref()).to_string(),
                c.fields
                    .get("model")
                    .cloned()
                    .unwrap_or_else(|| p.default_model.to_string()),
            )
        };
        if base_url.is_empty() || model.is_empty() {
            continue;
        }
        return Some(EndpointPlan {
            provider: p.id,
            base_url,
            wire,
            model,
            key_src: key_present.then_some(m.env),
        });
    }
    None
}

fn endpoint_wire_of(harness: &str) -> Option<&'static str> {
    ENDPOINT_HARNESSES
        .iter()
        .find(|(h, _)| *h == harness)
        .map(|(_, w)| *w)
}
