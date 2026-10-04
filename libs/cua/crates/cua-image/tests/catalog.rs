//! The canonical constants agree with the shared image catalog,
//! `libs/images/sandbox-images.json` (the list the apps, the CLI and the docs
//! show). The docs generator checks the same from the other side; this keeps
//! `cargo test -p cua-image` honest when only Rust changes.

use cua_image::canonical::{self, CanonicalOs};
use serde_json::Value;

fn catalog() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../images/sandbox-images.json"
    );
    let text = std::fs::read_to_string(path).expect("read libs/images/sandbox-images.json");
    serde_json::from_str(&text).expect("catalog is JSON")
}

fn entries(group: &str) -> Vec<Value> {
    catalog()["images"]
        .as_array()
        .expect("images")
        .iter()
        .filter(|i| i["group"] == group)
        .cloned()
        .collect()
}

#[test]
fn every_default_is_a_published_catalog_entry() {
    let canonical = entries("canonical");
    let no_env = |_: &str| None;
    for os in [CanonicalOs::Linux, CanonicalOs::Windows, CanonicalOs::Macos] {
        let default = canonical::canonical_for_with(os, None, &no_env);
        let entry = canonical
            .iter()
            .find(|i| i["ref"] == default.as_str())
            .unwrap_or_else(|| panic!("{default} is not in the catalog"));
        assert_eq!(entry["os"], os.as_str(), "{default}");
        assert_eq!(entry["published"], true, "{default} must be published");
    }
    // `macos:sequoia` is the other published macOS version.
    let sequoia = canonical::canonical_for_with(CanonicalOs::Macos, Some("sequoia"), &no_env);
    assert!(canonical.iter().any(|i| i["ref"] == sequoia.as_str()));
}

#[test]
fn every_canonical_entry_is_in_a_canonical_repo() {
    for entry in entries("canonical") {
        let r = entry["ref"].as_str().expect("ref");
        assert!(
            canonical::is_canonical(r),
            "{r} is not in a canonical repository"
        );
    }
}

/// Every canonical entry names its tier, and its tag follows the tier
/// grammar `<os-version>[-<tier>][-disk]` that the tag policy floats.
#[test]
fn every_canonical_entry_has_a_tier_matching_its_tag() {
    for entry in entries("canonical") {
        let r = entry["ref"].as_str().expect("ref");
        let tag = r.rsplit_once(':').expect("tag").1;
        if entry["os"] == "macos" && tag == "15" {
            // Legacy macOS 15 (no cua-spacesd) has no tiers.
            assert!(entry.get("tier").is_none(), "{r}");
            continue;
        }
        let tier = entry["tier"]
            .as_str()
            .unwrap_or_else(|| panic!("{r} has no tier"));
        assert!(
            cua_image::publish::is_canonical_floating_tag(tag),
            "{r}: tag outside the tier grammar"
        );
        let base = tag.strip_suffix("-disk").unwrap_or(tag);
        let want = cua_image::canonical::Tier::parse(tier)
            .expect("tier")
            .suffix();
        assert!(base.ends_with(&want) || tier == "full", "{r}: tier {tier}");
        if tier == "full" {
            assert!(!base.contains("-slim") && !base.contains("-xcode"), "{r}");
        }
        // The embedded catalog agrees with the file.
        let e = cua_image::catalog::find(r).expect("embedded entry");
        assert_eq!(e.tier.as_deref(), Some(tier));
    }
}
