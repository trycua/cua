//! Drift guard against the Hermes release Cua installs.
//!
//! `hermes-pin.json` is a snapshot of the pinned release's provider set
//! (`tools/hermes_snapshot.py`). Provider files may only name variables and
//! base URLs Hermes uses, every Hermes provider is either listed or
//! deliberately not (`hermes-unlisted.txt`), and the snapshot is the pin in
//! `cua-agents/installables.json`. See `providers/README.md` to move the pin.

use std::collections::BTreeSet;

use serde_json::Value;

fn snapshot() -> Value {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hermes-pin.json"))
        .expect("hermes-pin.json");
    serde_json::from_str(&text).expect("json")
}

#[test]
fn the_snapshot_is_the_pinned_release() {
    let installables = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cua-agents/installables.json"
    ))
    .expect("installables.json");
    let v: Value = serde_json::from_str(&installables).expect("json");
    let hermes = &v["items"]["hermes"];
    let snap = snapshot();
    assert_eq!(
        snap["commit"], hermes["commit"],
        "pin moved: regenerate hermes-pin.json (providers/README.md)"
    );
    assert_eq!(snap["version"], hermes["version"]);
}

#[test]
fn provider_files_use_what_hermes_uses() {
    let snap = snapshot();
    let theirs = snap["providers"].as_object().expect("providers");
    for p in cua_connections::PROVIDERS {
        if p.hermes_id.is_empty() {
            continue;
        }
        let h = theirs.get(p.hermes_id).unwrap_or_else(|| {
            panic!(
                "{}: Hermes has no provider {:?} at the pin",
                p.id, p.hermes_id
            )
        });
        let env: BTreeSet<&str> = h["env"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        // Every name Hermes receives from us is one it reads.
        for d in p.delivery_for("hermes") {
            if p.kind == cua_connections::Kind::Custom {
                continue;
            }
            assert!(
                env.contains(d.dest),
                "{}: Hermes does not read {} (it reads {env:?})",
                p.id,
                d.dest
            );
        }
        // The base URL is Hermes's (empty on their side: computed or user set).
        let base = h["base_url"].as_str().unwrap_or_default();
        if !base.is_empty() && !p.base_url.is_empty() {
            assert_eq!(p.base_url, base, "{}: base URL differs from Hermes's", p.id);
        }
        // The variable that overrides the base URL, when both name one.
        let base_env = h["base_url_env"].as_str().unwrap_or_default();
        if !base_env.is_empty() && !p.base_url_env.is_empty() {
            assert_eq!(p.base_url_env, base_env, "{}", p.id);
        }
        // Regions are Hermes's endpoints for the same provider.
        for r in p.regions {
            assert!(r.base_url.starts_with("https://"), "{}: {}", p.id, r.id);
        }
    }
}

#[test]
fn every_hermes_provider_is_listed_or_left_out_on_purpose() {
    let snap = snapshot();
    let ours: BTreeSet<&str> = cua_connections::PROVIDERS
        .iter()
        .filter(|p| !p.hermes_id.is_empty())
        .map(|p| p.hermes_id)
        .collect();
    let allowed: BTreeSet<String> =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hermes-unlisted.txt"))
            .unwrap()
            .lines()
            .map(|l| l.split('#').next().unwrap().trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    let theirs: BTreeSet<String> = snap["providers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let unknown: Vec<&String> = theirs
        .iter()
        .filter(|id| !ours.contains(id.as_str()) && !allowed.contains(*id))
        .collect();
    assert!(
        unknown.is_empty(),
        "Hermes providers neither in a provider file nor in hermes-unlisted.txt: {unknown:?}"
    );
    // The allowlist does not keep ids that no longer exist or that now have a file.
    for id in &allowed {
        assert!(
            theirs.contains(id),
            "{id} is not a Hermes provider at the pin"
        );
        assert!(
            !ours.contains(id.as_str()),
            "{id} has a provider file: remove it from hermes-unlisted.txt"
        );
    }
}
