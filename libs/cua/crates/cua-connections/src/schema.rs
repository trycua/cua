//! The provider file format and its checks. Used by the generator and the
//! tests; the generated tables hold the result, so shipped code never parses.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::{ENDPOINT_HARNESSES, HARNESS_IDS, MethodKind, WIRES};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDef {
    pub path: String,
    pub auth: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    pub id: String,
    pub label: String,
    #[serde(default = "text")]
    pub kind: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<String>,
}

fn text() -> String {
    "text".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodDef {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub flow: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub help: String,
    /// `ok` (default) or `review`.
    #[serde(default)]
    pub terms: String,
    /// `required` (default) or `optional`.
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub field: Vec<FieldDef>,
}

/// One model a provider serves, as a person picks it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDef {
    /// The id the provider's API takes.
    pub id: String,
    /// The name a person reads.
    pub name: String,
    /// A quick, cheap model: the default of a new prompt picks the first one.
    #[serde(default)]
    pub fast: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionDef {
    pub id: String,
    pub name: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    pub tier: String,
    pub rank: u32,
    pub kind: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub hidden_reason: String,
    /// `ok` (default) or `review`.
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub wire: String,
    #[serde(default)]
    pub also_wires: Vec<String>,
    pub base_url: String,
    #[serde(default)]
    pub base_url_env: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub hermes_id: String,
    pub key_help: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub check: Option<CheckDef>,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub method: Vec<MethodDef>,
    #[serde(default)]
    pub region: Vec<RegionDef>,
    /// The models a person can pick, best known first. Empty: the provider
    /// has no list here and `default_model` (if any) stands alone.
    #[serde(default)]
    pub model: Vec<ModelDef>,
    /// Harness id to the names it receives: `NAME`, or `DEST<-SRC`.
    #[serde(default)]
    pub delivery: BTreeMap<String, Vec<String>>,
}

impl ProviderDef {
    pub fn terms_review(&self) -> bool {
        self.terms == "review"
    }

    /// `(dest, src)` of a delivery entry.
    pub fn entry(raw: &str) -> (&str, &str) {
        match raw.split_once("<-") {
            Some((d, s)) => (d, s),
            None => (raw, raw),
        }
    }
}

/// Every provider file under `dir`, sorted by rank then id, checked.
pub fn load_dir(dir: &Path) -> Result<Vec<ProviderDef>, String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut out = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let def: ProviderDef =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", f.display()))?;
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if stem != def.id {
            return Err(format!(
                "{}: the file is named for the id {:?}",
                f.display(),
                def.id
            ));
        }
        out.push(def);
    }
    out.sort_by(|a, b| (a.rank, &a.id).cmp(&(b.rank, &b.id)));
    check(&out)?;
    Ok(out)
}

fn env_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}

/// The rules every file set must meet. Each failure names the file.
pub fn check(all: &[ProviderDef]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut ranks = BTreeSet::new();
    // name -> provider that saves it
    let mut saved: BTreeMap<String, String> = BTreeMap::new();
    for p in all {
        let at = |m: &str| format!("providers/{}.toml: {m}", p.id);
        if !ids.insert(p.id.clone()) {
            return Err(at("duplicate id"));
        }
        if !ranks.insert(p.rank) {
            return Err(at(&format!("rank {} is used by another provider", p.rank)));
        }
        if !p
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(at("the id is lowercase letters, digits and -"));
        }
        if !matches!(p.tier.as_str(), "top" | "more") {
            return Err(at("tier is top or more"));
        }
        if !matches!(
            p.kind.as_str(),
            "hosted" | "aggregator" | "local" | "custom" | "cloud"
        ) {
            return Err(at("kind is hosted, aggregator, local, custom or cloud"));
        }
        if !matches!(p.terms.as_str(), "" | "ok" | "review") {
            return Err(at("terms is ok or review"));
        }
        if p.hidden && p.hidden_reason.trim().is_empty() {
            return Err(at("a hidden provider says why in hidden_reason"));
        }
        if p.tier == "top" && (p.hidden || p.terms_review()) {
            return Err(at("an up front provider cannot be hidden"));
        }
        if p.name.contains('\u{b7}') || p.key_help.contains('\u{b7}') {
            return Err(at("UI text has no middle dot"));
        }
        for t in std::iter::once(&p.name)
            .chain(std::iter::once(&p.key_help))
            .chain(p.method.iter().map(|m| &m.help))
            .chain(p.method.iter().map(|m| &m.label))
        {
            if t.to_ascii_lowercase().contains("device")
                && !t.to_ascii_lowercase().contains("device code")
            {
                return Err(at("UI text says machines, not devices"));
            }
        }
        for w in std::iter::once(&p.wire).chain(p.also_wires.iter()) {
            if !WIRES.contains(&w.as_str()) {
                return Err(at(&format!("unknown wire {w:?}")));
            }
        }
        if p.method.is_empty() {
            return Err(at("a provider has at least one method"));
        }
        let mut method_ids = BTreeSet::new();
        for m in &p.method {
            if !method_ids.insert(m.id.clone()) {
                return Err(at(&format!("duplicate method {}", m.id)));
            }
            let kind = MethodKind::parse(&m.kind)
                .ok_or_else(|| at(&format!("method {}: unknown kind {:?}", m.id, m.kind)))?;
            if !matches!(m.terms.as_str(), "" | "ok" | "review") {
                return Err(at(&format!("method {}: terms is ok or review", m.id)));
            }
            if !matches!(m.secret.as_str(), "" | "required" | "optional") {
                return Err(at(&format!(
                    "method {}: secret is required or optional",
                    m.id
                )));
            }
            let needs_env = matches!(
                kind,
                MethodKind::ApiKey | MethodKind::ImportLogin | MethodKind::CustomEndpoint
            );
            if needs_env && m.env.is_empty() {
                return Err(at(&format!("method {} saves a credential: set env", m.id)));
            }
            for e in std::iter::once(&m.env).chain(m.aliases.iter()) {
                if e.is_empty() {
                    continue;
                }
                if !env_ok(e) {
                    return Err(at(&format!("{e:?} is not an environment variable name")));
                }
                if let Some(prev) = saved.insert(e.clone(), p.id.clone())
                    && prev != p.id
                {
                    return Err(at(&format!("{e} is also saved by {prev}")));
                }
            }
            // A subscription or imported sign-in belongs to one app.
            let signin = matches!(
                kind,
                MethodKind::ImportLogin | MethodKind::SubscriptionLogin | MethodKind::DeviceCode
            );
            if signin && m.scope.is_empty() {
                return Err(at(&format!(
                    "method {}: a sign-in names the one app that may use it (scope)",
                    m.id
                )));
            }
            if !m.scope.is_empty() && !HARNESS_IDS.contains(&m.scope.as_str()) {
                return Err(at(&format!(
                    "method {}: scope {:?} is not an app",
                    m.id, m.scope
                )));
            }
            if kind == MethodKind::ImportLogin && m.app.is_empty() && m.terms != "review" {
                return Err(at(&format!(
                    "method {}: an import names the app it is read from",
                    m.id
                )));
            }
            if (m.kind == "device_code" || m.flow == "device_code") && m.label.is_empty() {
                return Err(at(&format!("method {}: a sign-in has a label", m.id)));
            }
            for f in &m.field {
                if !matches!(f.kind.as_str(), "text" | "url" | "choice") {
                    return Err(at(&format!("field {}: kind is text, url or choice", f.id)));
                }
                if f.kind == "choice" && f.options.is_empty() {
                    return Err(at(&format!("field {}: a choice lists its options", f.id)));
                }
            }
        }
        let mut region_ids = BTreeSet::new();
        for r in &p.region {
            if !region_ids.insert(r.id.clone()) {
                return Err(at(&format!("duplicate region {}", r.id)));
            }
        }
        let mut model_ids = BTreeSet::new();
        for m in &p.model {
            if m.id.trim().is_empty() || m.id.len() > 128 || m.id.chars().any(char::is_whitespace) {
                return Err(at(&format!("model {:?}: an id has no spaces", m.id)));
            }
            if !model_ids.insert(m.id.clone()) {
                return Err(at(&format!("duplicate model {}", m.id)));
            }
            if m.name.trim().is_empty() || m.name.contains('\u{b7}') {
                return Err(at(&format!("model {}: a name, without a middle dot", m.id)));
            }
        }
        if !p.model.is_empty()
            && !p.default_model.is_empty()
            && !model_ids.contains(&p.default_model)
        {
            return Err(at("default_model is one of the listed models"));
        }
        if let Some(c) = &p.check {
            if !matches!(c.auth.as_str(), "bearer" | "x-api-key" | "x-goog-api-key") {
                return Err(at("check.auth is bearer, x-api-key or x-goog-api-key"));
            }
            if !c.path.starts_with('/') || p.base_url.is_empty() {
                return Err(at("a check is a path on the provider's own base URL"));
            }
        }
        if !p.base_url.is_empty() && !p.base_url.starts_with("https://") && p.kind != "local" {
            return Err(at("a hosted base URL is https"));
        }
        // Delivery: known apps, names this provider saves, scoped names to their app only.
        for (harness, entries) in &p.delivery {
            if !HARNESS_IDS.contains(&harness.as_str()) {
                return Err(at(&format!("delivery to unknown app {harness:?}")));
            }
            for raw in entries {
                let (dest, src) = ProviderDef::entry(raw);
                if !env_ok(dest) || !env_ok(src) {
                    return Err(at(&format!(
                        "delivery entry {raw:?} is not a variable name"
                    )));
                }
                let method = p
                    .method
                    .iter()
                    .find(|m| m.env == src || m.aliases.iter().any(|a| a == src))
                    .ok_or_else(|| {
                        at(&format!(
                            "delivery entry {raw:?}: this provider saves no {src}"
                        ))
                    })?;
                if !method.scope.is_empty() && method.scope != *harness {
                    return Err(at(&format!(
                        "{src} is the {} sign-in: it may only go to {}, not {harness}",
                        method.id, method.scope
                    )));
                }
            }
        }
        // Endpoint harnesses are fed by wire: the file must name a model.
        let overlaps = ENDPOINT_HARNESSES
            .iter()
            .any(|(_, w)| *w == p.wire || p.also_wires.iter().any(|a| a == w));
        if overlaps && p.kind != "custom" && !p.default_model.is_empty() && p.base_url.is_empty() {
            return Err(at("serves an endpoint wire but has no base URL"));
        }
    }
    Ok(())
}
