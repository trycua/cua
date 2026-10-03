//! Build identities measured from the host, for guests whose cua-spacesd
//! predates the `build` checks (images published before them).
//!
//! Through the guest shell it hashes the standalone `cua-driver` on PATH,
//! asks it for its embedded build (`cua-driver doctor --json`), hashes every
//! process that runs a `cua-driver` or the daemon, and reads the overlay
//! records. The result is the same `build.identity` check the guest emits, so
//! `--expect` works the same way; the daemon's own git sha stays unknown
//! (its build cannot report it), so `cua-spacesd=git:` still fails.

use cua_spacesd_client::diagnose::{Check, Report, Status};
use cua_spacesd_client::expect::IDENTITY_CHECK;
use std::collections::BTreeMap;

/// Prints `key=value` lines. Bounded: /proc scan capped at 4096 entries.
const PROBE: &str = r#"
exe_of() { e=$(readlink "/proc/$1/exe" 2>/dev/null) || e=$(tr '\0' '\n' <"/proc/$1/cmdline" 2>/dev/null | head -n1); echo "${e% (deleted)}"; }
if p=$(command -v cua-driver 2>/dev/null); then
  echo "cua-driver.standalone.path=$p"
  echo "cua-driver.standalone.exe_sha256=$(sha256sum "$(readlink -f "$p")" | cut -d' ' -f1)"
  j=$(timeout 20 "$p" doctor --json 2>/dev/null | tr -d '\n')
  v=$(printf '%s' "$j" | sed -n 's/.*"build"[^}]*"version": *"\([^"]*\)".*/\1/p')
  g=$(printf '%s' "$j" | sed -n 's/.*"build"[^}]*"git_sha": *"\([^"]*\)".*/\1/p')
  [ -n "$v" ] || v=$("$p" --version 2>/dev/null | awk '{print $2}')
  echo "cua-driver.standalone.version=$v"
  echo "cua-driver.standalone.git_sha=$g"
fi
n=0
for d in /proc/[0-9]*; do
  n=$((n + 1)); [ $n -le 4096 ] || break
  pid=${d#/proc/}; e=$(exe_of "$pid"); b=${e##*/}
  case "$b" in
    cua-driver) echo "running.cua-driver.$pid=$(sha256sum "/proc/$pid/exe" 2>/dev/null | cut -d' ' -f1)" ;;
    cua-spacesd|cua-guestd|cua-env-driver) echo "cua-spacesd.exe_sha256=$(sha256sum "/proc/$pid/exe" 2>/dev/null | cut -d' ' -f1)" ;;
  esac
done
for f in /var/lib/cua/overlays/*.json; do
  [ -f "$f" ] || continue
  name=$(sed -n 's/.*"name":"\([^"]*\)".*/\1/p' "$f")
  echo "overlay.$name.path=$(sed -n 's/.*"path":"\([^"]*\)".*/\1/p' "$f")"
  echo "overlay.$name.sha256=$(sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p' "$f")"
done
"#;

/// Parses the probe's output into identity facts.
pub fn facts_from(output: &str) -> BTreeMap<String, String> {
    let mut facts = BTreeMap::new();
    let mut running = Vec::new();
    for line in output.lines().take(8192) {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if let Some(pid) = k.strip_prefix("running.cua-driver.") {
            running.push(format!("{pid}:{v}"));
        } else if !v.is_empty() {
            facts.insert(k.to_owned(), v.to_owned());
        }
    }
    if !running.is_empty() {
        facts.insert("cua-driver.running".into(), running.join(","));
    }
    facts
}

/// The host-measured `build.identity` check, plus a failure when a running
/// `cua-driver` process runs something other than the file on PATH.
pub fn checks(facts: BTreeMap<String, String>) -> Vec<Check> {
    let mut out = Vec::new();
    let disk = facts.get("cua-driver.standalone.exe_sha256").cloned();
    if let (Some(disk), Some(running)) = (&disk, facts.get("cua-driver.running")) {
        let stale: Vec<&str> = running
            .split(',')
            .filter(|e| !e.ends_with(&format!(":{disk}")))
            .collect();
        out.push(if stale.is_empty() {
            Check::new(
                "build.running.cua-driver",
                Status::Pass,
                "every running cua-driver runs the binary on PATH",
            )
        } else {
            Check::new(
                "build.running.cua-driver",
                Status::Fail,
                format!(
                    "cua-driver processes run another build: {}",
                    stale.join(", ")
                ),
            )
            .fix("restart them (`cua sb overlay` stops stale copies)")
        });
    }
    let mut identity = Check::new(
        IDENTITY_CHECK,
        Status::Pass,
        "measured from the host (the guest's cua-spacesd predates build identities)",
    )
    .fact("measured_by", "cua doctor");
    for (k, v) in facts {
        identity = identity.fact(k, v);
    }
    out.insert(0, identity);
    out
}

/// Adds the host-measured identity when the guest report has none.
pub async fn fill(cua: &std::sync::Arc<cua_sdk::Cua>, target: &str, report: &mut Report) {
    if report.checks.iter().any(|c| c.id == IDENTITY_CHECK) {
        return;
    }
    let Ok(env) = crate::sandbox::env_of(cua, target).await else {
        return;
    };
    let Ok(o) = env.sh(PROBE.to_string(), Some(90_000)).await else {
        return;
    };
    let facts = facts_from(&String::from_utf8_lossy(&o.stdout));
    report.checks.extend(checks(facts));
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn parses_and_flags_stale_processes() {
        let out = format!(
            "cua-driver.standalone.path=/usr/local/bin/cua-driver\n\
             cua-driver.standalone.exe_sha256={A}\n\
             cua-driver.standalone.git_sha=abcdef1234\n\
             cua-driver.standalone.version=0.28.2\n\
             running.cua-driver.42={A}\nrunning.cua-driver.43={B}\n\
             overlay.cua-driver.sha256={A}\nnoise\n"
        );
        let facts = facts_from(&out);
        assert_eq!(facts["cua-driver.running"], format!("42:{A},43:{B}"));
        let checks = checks(facts);
        assert_eq!(checks[0].id, IDENTITY_CHECK);
        let running = checks
            .iter()
            .find(|c| c.id == "build.running.cua-driver")
            .unwrap();
        assert_eq!(running.status, Status::Fail);
        assert!(running.message.contains("43"));

        // The measured identity satisfies a sha256 and git expectation.
        let mut report = Report {
            checks: checks
                .into_iter()
                .filter(|c| c.id == IDENTITY_CHECK)
                .collect(),
            ..Report::default()
        };
        let want = cua_spacesd_client::expect::parse_all(&[
            format!("cua-driver=sha256:{A}"),
            "cua-driver=git:abcdef1".into(),
            "cua-spacesd=git:abcdef1".into(),
        ])
        .unwrap();
        cua_spacesd_client::expect::apply(&mut report, &want);
        let status = |id: &str| report.checks.iter().find(|c| c.id == id).unwrap().status;
        assert_eq!(status("expect.cua-driver.sha256"), Status::Pass);
        assert_eq!(status("expect.cua-driver.git"), Status::Pass);
        // The old daemon cannot report its revision: still stale.
        assert_eq!(status("expect.cua-spacesd.git"), Status::Fail);
    }
}
