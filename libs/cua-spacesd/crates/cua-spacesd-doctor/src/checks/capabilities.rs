// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `capabilities`: what `GetCapabilities` reports against what the image
//! claims.
//!
//! - every unsupported feature names a `limitation` (the conformance rule);
//! - every required feature is supported (its behavioural check follows in
//!   its own group);
//! - every optional feature missing is a warning;
//! - a supported feature the manifest does not list is a warning (the
//!   image may be under-documented);
//! - feature attributes are sane (`h264_hw.encoder` is a compiled backend);
//! - every attribute the manifest claims (`feature_attributes`, for example
//!   `presence.cursor_shape`'s `hit_test`/`system`/`probe` backends) is
//!   reported with that value.

use cua_spacesd_client::diagnose::{Check, Status};

use crate::{Ctx, Recorder};

/// `key=value` for every claimed attribute the feature reports otherwise.
fn attribute_mismatches<'a, I>(
    want: &std::collections::BTreeMap<String, String>,
    got: I,
) -> Vec<String>
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    let got: std::collections::BTreeMap<&str, &str> = got
        .into_iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    want.iter()
        .filter(|(k, v)| got.get(k.as_str()) != Some(&v.as_str()))
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    let features = &ctx.caps.features;
    let unnamed: Vec<&str> = features
        .iter()
        .filter(|f| !f.supported && f.limitation.trim().is_empty())
        .map(|f| f.name.as_str())
        .collect();
    rec.push(
        Check::new(
            "capabilities.limitations",
            super::verdict(unnamed.is_empty()),
            if unnamed.is_empty() {
                format!(
                    "{} features reported; every unsupported one names its limitation",
                    features.len()
                )
            } else {
                format!("unsupported without a limitation: {}", unnamed.join(", "))
            },
        )
        .fact(
            "supported",
            features
                .iter()
                .filter(|f| f.supported)
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
        .fix("a provider reports a feature as unsupported without saying why; fix its capabilities()"),
        &["core"],
    )
    .await;

    let manifest = &ctx.manifest.manifest;
    for name in &manifest.features_required {
        if name.ends_with('*') {
            continue;
        }
        let id = format!("capabilities.required.{name}");
        let claim = format!("feature:{name}");
        let check = if ctx.supports(name) {
            Check::new(&id, Status::Pass, format!("{name} supported"))
        } else {
            Check::new(
                &id,
                Status::Fail,
                format!(
                    "{name} is claimed but unsupported: {}",
                    ctx.limitation(name)
                ),
            )
            .fix("fix the image (or drop the claim from image.json `claims.features_required`)")
        };
        rec.push(check, &[&claim]).await;
    }
    for name in &manifest.features_optional {
        if name.ends_with('*') {
            continue;
        }
        let id = format!("capabilities.optional.{name}");
        let claim = format!("feature:{name}");
        let check = if ctx.supports(name) {
            Check::new(&id, Status::Pass, format!("{name} supported"))
        } else {
            // Optional: absent is not a defect (and never fails --strict).
            Check::new(
                &id,
                Status::Skip,
                format!("optional {name} unavailable: {}", ctx.limitation(name)),
            )
            .skip_reason("optional_unavailable")
        };
        rec.push(check, &[&claim]).await;
    }

    // Claimed attribute values: the backends the image was built for (for
    // example presence.cursor_shape's hit_test/system/probe).
    for (name, want) in &manifest.feature_attributes {
        let id = format!("capabilities.attributes.{name}");
        let claim = format!("feature:{name}");
        let reported = features.iter().find(|f| &f.name == name);
        let check = match reported {
            Some(f) if f.supported => {
                let mismatched = attribute_mismatches(want, &f.attributes);
                let seen = want
                    .keys()
                    .map(|k| {
                        format!(
                            "{k}={}",
                            f.attributes
                                .get(k)
                                .map(String::as_str)
                                .unwrap_or("<missing>")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if mismatched.is_empty() {
                    Check::new(&id, Status::Pass, format!("{name}: {seen}"))
                } else {
                    Check::new(
                        &id,
                        Status::Fail,
                        format!("{name} reports {seen}; claimed {}", mismatched.join(" ")),
                    )
                    .fix("the image's backend differs from its claim: fix the image, or the claim in image.json `claims.feature_attributes`")
                }
                .fact("attributes", seen)
            }
            _ => Check::new(
                &id,
                Status::Fail,
                format!(
                    "{name} attributes are claimed but it is unsupported: {}",
                    ctx.limitation(name)
                ),
            )
            .fix("fix the image (or drop the claim from image.json `claims.feature_attributes`)"),
        };
        // Attribute claims are required whenever they are made.
        rec.push(check, &[&claim, "manifest:feature_attributes"])
            .await;
    }

    if ctx.manifest.present() {
        let unclaimed: Vec<&str> = features
            .iter()
            .filter(|f| f.supported && !manifest.lists(&f.name))
            .map(|f| f.name.as_str())
            .collect();
        rec.push(
            Check::new(
                "capabilities.unclaimed",
                if unclaimed.is_empty() {
                    Status::Pass
                } else {
                    Status::Warn
                },
                if unclaimed.is_empty() {
                    "every supported feature is listed in the manifest".to_owned()
                } else {
                    format!(
                        "supported but not claimed (image may be under-documented): {}",
                        unclaimed.join(", ")
                    )
                },
            )
            .fix("add them to image.json `claims` (features_required or features_optional)"),
            &[],
        )
        .await;
    }

    // h264_hw.encoder must name a backend this build probes.
    let compiled: Vec<String> = cua_media_codec::probe::compiled_backends()
        .iter()
        .map(|b| format!("{b:?}").to_lowercase())
        .collect();
    let mut bad = Vec::new();
    for feature in features {
        for (key, value) in &feature.attributes {
            if feature.name == "h264_hw" && key == "encoder" && !compiled.contains(value) {
                bad.push(format!("h264_hw.encoder={value}"));
            }
        }
    }
    rec.push(
        Check::new(
            "capabilities.attributes",
            super::verdict(bad.is_empty()),
            if bad.is_empty() {
                format!(
                    "feature attributes consistent (compiled encoders: {})",
                    compiled.join(", ")
                )
            } else {
                format!(
                    "attributes name backends this build lacks: {}",
                    bad.join(", ")
                )
            },
        )
        .fact("codecs_compiled", compiled.join(",")),
        &["core"],
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::attribute_mismatches;

    #[test]
    fn claimed_attributes_must_match() {
        let want: BTreeMap<String, String> = [("hit_test", "atspi"), ("probe", "xtest")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let got = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let exact = got(&[
            ("hit_test", "atspi"),
            ("system", "xfixes"),
            ("probe", "xtest"),
        ]);
        assert!(attribute_mismatches(&want, &exact).is_empty());
        let off = got(&[("hit_test", "atspi"), ("probe", "off")]);
        assert_eq!(attribute_mismatches(&want, &off), vec!["probe=xtest"]);
        let missing = got(&[("probe", "xtest")]);
        assert_eq!(
            attribute_mismatches(&want, &missing),
            vec!["hit_test=atspi"]
        );
    }
}
