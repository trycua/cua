// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `software`: the image's software manifest is what is installed.
//!
//! | id | claims |
//! |---|---|
//! | `software.apps` | every `claims.apps` entry answers its version command, matching `expect` when given |
//! | `software.tools` | the same for `claims.tools` (the dev tiers' toolchains) |
//! | `software.simulator_runtimes` | the available simulator runtimes are exactly `claims.simulator_runtimes` |
//!
//! A missing or wrong-version entry fails under `--strict` and warns
//! otherwise. The versions (and runtimes) land in the report's `fidelity`
//! block, which `scripts/images/record-software.py` turns into the docs'
//! per-image software table.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::manifest::SoftwareClaim;
use cua_spacesd_client::Command;

use crate::{Ctx, Recorder};

/// The version recorded for an entry whose command failed or is missing.
pub const UNAVAILABLE: &str = "unavailable";

/// Why one claimed entry does not hold, or `None` when it does.
pub fn problem(name: &str, claim: &SoftwareClaim, version: &str) -> Option<String> {
    if version == UNAVAILABLE || version.is_empty() {
        return Some(format!(
            "{name}: missing (`{}` failed)",
            claim.argv().join(" ")
        ));
    }
    let expect = claim.expect();
    if expect.is_empty() {
        return None;
    }
    match regex::Regex::new(expect) {
        Ok(re) if re.is_match(version) => None,
        Ok(_) => Some(format!("{name}: {version:?} does not match {expect:?}")),
        Err(error) => Some(format!(
            "{name}: bad expect {expect:?} in the manifest: {error}"
        )),
    }
}

/// Every problem of `claims` given the probed `versions` (name to first
/// output line, or [`UNAVAILABLE`]).
pub fn problems(
    claims: &BTreeMap<String, SoftwareClaim>,
    versions: &BTreeMap<String, String>,
) -> Vec<String> {
    claims
        .iter()
        .filter_map(|(name, claim)| {
            let version = versions
                .get(name)
                .map(String::as_str)
                .unwrap_or(UNAVAILABLE);
            problem(name, claim, version)
        })
        .collect()
}

/// Available runtime names from `xcrun simctl list runtimes -j`, sorted.
pub fn available_runtimes(json: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("simctl JSON does not parse: {e}"))?;
    let runtimes = value["runtimes"]
        .as_array()
        .ok_or("simctl JSON has no `runtimes` array")?;
    let mut names: Vec<String> = runtimes
        .iter()
        .filter(|r| r["isAvailable"].as_bool().unwrap_or(false))
        .filter_map(|r| r["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Runtimes missing from `got` and unexpected in it.
pub fn runtime_diff(expected: &[String], got: &[String]) -> (Vec<String>, Vec<String>) {
    let want: BTreeSet<&String> = expected.iter().collect();
    let have: BTreeSet<&String> = got.iter().collect();
    (
        want.difference(&have).map(|s| (*s).clone()).collect(),
        have.difference(&want).map(|s| (*s).clone()).collect(),
    )
}

async fn first_line(ctx: &Ctx, argv: &[String], timeout: Duration) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let out = ctx
        .client
        .run(
            Command::new(program)
                .args(args.iter().map(String::as_str))
                .timeout(timeout),
        )
        .await
        .ok()?;
    if !out.success() {
        return None;
    }
    // Some tools print their version on stderr (`java -version`).
    let text = out.stdout_str();
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        text
    };
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

/// Runs each entry's version command: name to first output line, or
/// [`UNAVAILABLE`].
pub async fn probe(
    ctx: &Ctx,
    claims: &BTreeMap<String, SoftwareClaim>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, claim) in claims {
        let version = first_line(ctx, claim.argv(), Duration::from_secs(30))
            .await
            .unwrap_or_else(|| UNAVAILABLE.into());
        out.insert(name.clone(), version);
    }
    out
}

fn verdict(ctx: &Ctx, problems: &[String]) -> Status {
    match (problems.is_empty(), ctx.options.strict) {
        (true, _) => Status::Pass,
        (false, true) => Status::Fail,
        (false, false) => Status::Warn,
    }
}

fn entries_check(
    ctx: &Ctx,
    id: &str,
    kind: &str,
    claims: &BTreeMap<String, SoftwareClaim>,
    versions: &BTreeMap<String, String>,
) -> Check {
    let problems = problems(claims, versions);
    let listed = versions
        .iter()
        .map(|(n, v)| format!("{n} {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let message = if problems.is_empty() {
        format!("{} claimed {kind} present: {listed}", claims.len())
    } else {
        problems.join("; ")
    };
    let mut check = Check::new(id, verdict(ctx, &problems), message)
        .fact("claimed", claims.len())
        .fact("problems", problems.len());
    for (name, version) in versions {
        check = check.fact(format!("{kind}.{name}"), version);
    }
    if !problems.is_empty() {
        check = check.fix(format!(
            "install the missing {kind} (or fix their versions) in the image build, or drop the claim from image.json"
        ));
    }
    check
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("software") || !ctx.manifest.present() {
        return;
    }
    let manifest = &ctx.manifest.manifest;
    for (id, kind, claims) in [
        ("software.apps", "apps", &manifest.apps),
        ("software.tools", "tools", &manifest.tools),
    ] {
        if claims.is_empty() || !rec.wants(id) {
            continue;
        }
        let versions = probe(ctx, claims).await;
        {
            let mut f = ctx.fidelity.lock().await;
            if kind == "apps" {
                f.app_versions = versions.clone();
            } else {
                f.tool_versions = versions.clone();
            }
        }
        rec.push(
            entries_check(ctx, id, kind, claims, &versions),
            &["manifest:software"],
        )
        .await;
    }
    if manifest.simulator_runtimes.is_empty() {
        return;
    }
    let expected = manifest.simulator_runtimes.clone();
    rec.run(
        "software.simulator_runtimes",
        &["manifest:software"],
        Duration::from_secs(120),
        async {
            let out = ctx
                .client
                .run(
                    Command::new("xcrun")
                        .args(["simctl", "list", "runtimes", "-j"])
                        .timeout(Duration::from_secs(90)),
                )
                .await;
            let got = match out {
                Ok(out) if out.success() => available_runtimes(&out.stdout_str()),
                Ok(out) => Err(format!(
                    "xcrun simctl exited {:?}: {}",
                    out.status.code,
                    out.stderr_str().trim()
                )),
                Err(error) => Err(format!("xcrun simctl: {error}")),
            };
            let got = match got {
                Ok(got) => got,
                Err(error) => {
                    let status = verdict(ctx, std::slice::from_ref(&error));
                    return Check::new("software.simulator_runtimes", status, error)
                        .fix("install Xcode and its simulator runtimes, or drop claims.simulator_runtimes");
                }
            };
            ctx.fidelity.lock().await.simulator_runtimes = got.clone();
            let (missing, extra) = runtime_diff(&expected, &got);
            let mut problems = Vec::new();
            if !missing.is_empty() {
                problems.push(format!("missing {}", missing.join(", ")));
            }
            if !extra.is_empty() {
                problems.push(format!("unexpected {}", extra.join(", ")));
            }
            let message = if problems.is_empty() {
                format!("runtimes are exactly {}", got.join(", "))
            } else {
                format!("{} (expected {})", problems.join("; "), expected.join(", "))
            };
            let mut check = Check::new("software.simulator_runtimes", verdict(ctx, &problems), message)
                .fact("available", got.join(", "))
                .fact("expected", expected.join(", "));
            if !problems.is_empty() {
                check = check.fix("download exactly the claimed runtimes (xcodebuild -downloadPlatform) and delete the others (xcrun simctl runtime delete)");
            }
            check
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(json: &str) -> BTreeMap<String, SoftwareClaim> {
        serde_json::from_str(json).unwrap()
    }

    fn versions(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn present_entries_pass() {
        let c = claims(
            r#"{"python3":["python3","--version"],
                "node":{"argv":["node","--version"],"expect":"^v22\\."}}"#,
        );
        let v = versions(&[("python3", "Python 3.12.3"), ("node", "v22.20.0")]);
        assert!(problems(&c, &v).is_empty());
    }

    #[test]
    fn missing_and_wrong_versions_are_problems() {
        let c = claims(
            r#"{"firefox":["firefox","--version"],
                "go":{"argv":["go","version"],"expect":"go1\\.25\\."},
                "gh":{"argv":["gh","--version"]}}"#,
        );
        let v = versions(&[
            ("firefox", UNAVAILABLE),
            ("go", "go version go1.24.1 linux/arm64"),
        ]);
        let p = problems(&c, &v);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p.iter().any(|s| s.starts_with("firefox: missing")));
        assert!(p.iter().any(|s| s.starts_with("gh: missing")));
        assert!(p.iter().any(|s| s.contains("does not match")));
    }

    #[test]
    fn a_bad_regex_is_a_problem_not_a_panic() {
        let c = claims(r#"{"x":{"argv":["x"],"expect":"("}}"#);
        let p = problems(&c, &versions(&[("x", "x 1")]));
        assert!(p[0].contains("bad expect"), "{p:?}");
    }

    #[test]
    fn runtimes_are_parsed_and_diffed() {
        let json = r#"{"runtimes":[
            {"name":"iOS 26.0","isAvailable":true},
            {"name":"watchOS 26.0","isAvailable":false},
            {"name":"iOS 18.6","isAvailable":true}]}"#;
        let got = available_runtimes(json).unwrap();
        assert_eq!(got, ["iOS 18.6", "iOS 26.0"]);
        let (missing, extra) = runtime_diff(&["iOS 26.0".into(), "tvOS 26.0".into()], &got);
        assert_eq!(missing, ["tvOS 26.0"]);
        assert_eq!(extra, ["iOS 18.6"]);
        assert_eq!(
            runtime_diff(&got, &got),
            (Vec::<String>::new(), Vec::<String>::new())
        );
        assert!(available_runtimes("{}").is_err());
        assert!(available_runtimes("nope").is_err());
    }

    /// The claims the images make parse and name a command (the Linux
    /// image's generated manifest, and the macOS image.json).
    #[test]
    fn image_claims_parse() {
        let linux: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../images/common/tools/tests/fixtures/manifest.linux.json"
        ))
        .unwrap();
        let macos: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../images/macos/image.json")).unwrap();
        for apps in [&linux["apps"], &macos["claims"]["apps"]] {
            let apps: BTreeMap<String, SoftwareClaim> =
                serde_json::from_value(apps.clone()).unwrap();
            assert!(!apps.is_empty());
            assert!(apps.values().all(|c| !c.argv().is_empty()));
        }
    }
}
