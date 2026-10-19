//! `--expect COMPONENT=WANT`: fail a doctor report unless the build running
//! in the guest is the expected one.
//!
//! Both doctors use this: `cua-spacesd doctor --expect ...` in the guest and
//! `cua doctor REF --expect ...` on the host (against the guest's report).
//! CI passes the sha256 of every binary it injected (`cua sb create
//! --overlay`), so a test can never pass against a stale cua-driver or
//! cua-spacesd bundled in the image.
//!
//! The running identity comes from the guest report's `build.identity`
//! check (facts `<component>.version`, `.git_sha`, `.exe_sha256`, and
//! `cua-driver.standalone.*` for a separate `cua-driver` binary). A report
//! from a build that predates it has no identity, and every expectation
//! fails: a build that cannot say what it is counts as stale.
//!
//! WANT forms: `sha256:<64 hex>` (the exact executable), `git:<sha>` (the
//! source revision, 7 to 40 hex, prefix match), `version:<semver>`, or bare
//! (64 hex is a sha256, 7 to 40 hex a git sha, anything else a version).

use crate::diagnose::{Check, Report, Severity, Status};

/// The id of the guest check that carries the running build identity.
pub const IDENTITY_CHECK: &str = "build.identity";

/// What to compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// sha256 of the executable (lowercase hex).
    Sha256(String),
    /// Source revision (lowercase hex, 7 to 40 digits; prefix match).
    Git(String),
    /// Release version.
    Version(String),
}

impl Want {
    fn describe(&self) -> String {
        match self {
            Want::Sha256(s) => format!("sha256:{}", short(s)),
            Want::Git(s) => format!("git:{}", short(s)),
            Want::Version(v) => format!("version {v}"),
        }
    }
}

/// One `COMPONENT=WANT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    /// `cua-driver`, `cua-spacesd`, or an overlay name.
    pub component: String,
    /// The expected build.
    pub want: Want,
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

impl std::str::FromStr for Expectation {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, String> {
        let (component, want) = spec.split_once('=').ok_or_else(|| {
            format!("--expect {spec:?}: use COMPONENT=WANT, e.g. cua-driver=sha256:<hex>")
        })?;
        let component = component.trim();
        let want = want.trim();
        if component.is_empty() || want.is_empty() {
            return Err(format!("--expect {spec:?}: empty component or value"));
        }
        let lower = want.to_ascii_lowercase();
        let want = if let Some(hex) = lower.strip_prefix("sha256:") {
            if hex.len() != 64 || !is_hex(hex) {
                return Err(format!("--expect {spec:?}: sha256 needs 64 hex digits"));
            }
            Want::Sha256(hex.to_owned())
        } else if let Some(hex) = lower.strip_prefix("git:") {
            if !(7..=40).contains(&hex.len()) || !is_hex(hex) {
                return Err(format!("--expect {spec:?}: git needs 7 to 40 hex digits"));
            }
            Want::Git(hex.to_owned())
        } else if let Some(v) = want.strip_prefix("version:") {
            Want::Version(v.trim().trim_start_matches('v').to_owned())
        } else if lower.len() == 64 && is_hex(&lower) {
            Want::Sha256(lower)
        } else if (7..=40).contains(&lower.len()) && is_hex(&lower) {
            Want::Git(lower)
        } else {
            Want::Version(want.trim_start_matches('v').to_owned())
        };
        Ok(Expectation {
            component: component.to_owned(),
            want,
        })
    }
}

/// Parses every spec, failing on the first bad one.
pub fn parse_all(specs: &[String]) -> Result<Vec<Expectation>, String> {
    specs.iter().map(|s| s.parse()).collect()
}

/// One running build of a component, as the guest reported it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instance {
    /// Where it runs (`linked into cua-spacesd`, `/usr/local/bin/cua-driver`).
    pub label: String,
    pub version: String,
    pub git_sha: String,
    pub exe_sha256: String,
}

impl Instance {
    fn value(&self, want: &Want) -> &str {
        match want {
            Want::Sha256(_) => &self.exe_sha256,
            Want::Git(_) => &self.git_sha,
            Want::Version(_) => &self.version,
        }
    }

    fn matches(&self, want: &Want) -> bool {
        match want {
            Want::Sha256(w) => self.exe_sha256.eq_ignore_ascii_case(w),
            Want::Git(w) => {
                let have = self.git_sha.to_ascii_lowercase();
                !have.is_empty() && (have.starts_with(w.as_str()) || w.starts_with(&have))
            }
            Want::Version(w) => self.version.trim_start_matches('v') == w,
        }
    }
}

/// The running instances of `component` the report names.
pub fn instances(report: &Report, component: &str) -> Vec<Instance> {
    let Some(identity) = report.checks.iter().find(|c| c.id == IDENTITY_CHECK) else {
        // A guest that predates build identities: only versions are known.
        let mut out = Vec::new();
        if let Some(s) = &report.spacesd {
            match component {
                "cua-spacesd" if !s.version.is_empty() => out.push(Instance {
                    label: "cua-spacesd (no build identity)".into(),
                    version: s.version.clone(),
                    git_sha: s.git_sha.clone(),
                    ..Instance::default()
                }),
                "cua-driver" if !s.cua_driver_version.is_empty() => out.push(Instance {
                    label: "linked into cua-spacesd (no build identity)".into(),
                    version: s.cua_driver_version.clone(),
                    ..Instance::default()
                }),
                _ => {}
            }
        }
        return out;
    };
    let fact = |key: String| identity.facts.get(&key).cloned().unwrap_or_default();
    let read = |prefix: &str, label: String| Instance {
        label,
        version: fact(format!("{prefix}.version")),
        git_sha: fact(format!("{prefix}.git_sha")),
        exe_sha256: fact(format!("{prefix}.exe_sha256")),
    };
    let present = |prefix: &str| {
        ["version", "git_sha", "exe_sha256", "path"]
            .iter()
            .any(|k| identity.facts.contains_key(&format!("{prefix}.{k}")))
    };
    let mut out = Vec::new();
    match component {
        "cua-spacesd" => {
            if present("cua-spacesd") {
                out.push(read("cua-spacesd", "running cua-spacesd".into()));
            }
        }
        "cua-driver" => {
            if present("cua-driver") {
                out.push(read(
                    "cua-driver",
                    "cua-driver linked into cua-spacesd".into(),
                ));
            }
            if present("cua-driver.standalone") {
                let path = fact("cua-driver.standalone.path".into());
                out.push(read(
                    "cua-driver.standalone",
                    if path.is_empty() {
                        "standalone cua-driver".into()
                    } else {
                        format!("standalone {path}")
                    },
                ));
            }
        }
        other => {
            let prefix = format!("overlay.{other}");
            if present(&prefix) {
                let path = fact(format!("{prefix}.path"));
                let mut i = read(&prefix, format!("overlay {other} at {path}"));
                if i.exe_sha256.is_empty() {
                    i.exe_sha256 = fact(format!("{prefix}.sha256"));
                }
                out.push(i);
            }
        }
    }
    out
}

/// The `expect.<component>.<sha256|git|version>` check for one expectation
/// (always required).
pub fn evaluate_one(report: &Report, expectation: &Expectation) -> Check {
    let kind = match expectation.want {
        Want::Sha256(_) => "sha256",
        Want::Git(_) => "git",
        Want::Version(_) => "version",
    };
    let id = format!("expect.{}.{kind}", expectation.component);
    let mut all = instances(report, &expectation.component);
    // A cua-driver linked into cua-spacesd has cua-spacesd's executable: a
    // sha256 names the standalone binary only.
    if expectation.component == "cua-driver" && matches!(expectation.want, Want::Sha256(_)) {
        all.retain(|i| !i.label.starts_with("cua-driver linked"));
    }
    let want = expectation.want.describe();
    let mut check = if all.is_empty() {
        Check::new(
            id.clone(),
            Status::Fail,
            format!(
                "expected {} {want}, but the guest reports no running build of it",
                expectation.component
            ),
        )
        .fix("inject the build under test (`cua sb create --overlay NAME=PATH`) into an image whose cua-spacesd reports build identities")
    } else {
        let bad: Vec<String> = all
            .iter()
            .filter(|i| !i.matches(&expectation.want))
            .map(|i| {
                let have = i.value(&expectation.want);
                format!(
                    "{} is {}",
                    i.label,
                    if have.is_empty() {
                        "unknown (no build identity)".to_owned()
                    } else {
                        short(have)
                    }
                )
            })
            .collect();
        if bad.is_empty() {
            Check::new(
                id.clone(),
                Status::Pass,
                format!(
                    "{} {want}: {}",
                    expectation.component,
                    all.iter()
                        .map(|i| i.label.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        } else {
            Check::new(
                id.clone(),
                Status::Fail,
                format!("stale {}: expected {want}; {}", expectation.component, bad.join("; ")),
            )
            .fix("the guest runs a different build than the one under test: re-inject it (`cua sb overlay`) and restart the service")
        }
    };
    check.severity = Severity::Required;
    check.group = "expect".into();
    check = check.fact("want", want);
    for (n, i) in all.iter().enumerate() {
        check = check.fact(
            format!("instance.{n}"),
            format!(
                "{}: version={} git={} sha256={}",
                i.label,
                i.version,
                short(&i.git_sha),
                short(&i.exe_sha256)
            ),
        );
    }
    check
}

/// Appends one required `expect.*` check per expectation to `report`
/// (call before `Report::finalize`).
pub fn apply(report: &mut Report, expectations: &[Expectation]) {
    let checks: Vec<Check> = expectations
        .iter()
        .map(|e| evaluate_one(report, e))
        .collect();
    report.checks.extend(checks);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnose::Spacesd;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn report(facts: &[(&str, &str)]) -> Report {
        let mut identity = Check::new(IDENTITY_CHECK, Status::Pass, "identity");
        for (k, v) in facts {
            identity = identity.fact(*k, *v);
        }
        Report {
            checks: vec![identity],
            ..Report::default()
        }
    }

    fn eval(r: &Report, spec: &str) -> Check {
        evaluate_one(r, &spec.parse().unwrap())
    }

    #[test]
    fn parses_every_form() {
        let e: Expectation = format!("cua-driver=sha256:{A}").parse().unwrap();
        assert_eq!(e.want, Want::Sha256(A.into()));
        let e: Expectation = format!("cua-driver={}", A.to_uppercase()).parse().unwrap();
        assert_eq!(e.want, Want::Sha256(A.into()));
        let e: Expectation = "cua-spacesd=git:ABCDEF1".parse().unwrap();
        assert_eq!(e.want, Want::Git("abcdef1".into()));
        let e: Expectation = "cua-spacesd=0123abc".parse().unwrap();
        assert_eq!(e.want, Want::Git("0123abc".into()));
        let e: Expectation = "cua-driver=v0.3.1".parse().unwrap();
        assert_eq!(e.want, Want::Version("0.3.1".into()));
        let e: Expectation = "cua-driver=version:1.0.0".parse().unwrap();
        assert_eq!(e.want, Want::Version("1.0.0".into()));
        assert!("cua-driver".parse::<Expectation>().is_err());
        assert!("cua-driver=sha256:abc".parse::<Expectation>().is_err());
        assert!("=x".parse::<Expectation>().is_err());
        assert!("cua-driver=git:xyz1234".parse::<Expectation>().is_err());
    }

    #[test]
    fn spacesd_sha_git_and_version() {
        let r = report(&[
            ("cua-spacesd.version", "0.1.0"),
            (
                "cua-spacesd.git_sha",
                "0123456789abcdef0123456789abcdef01234567",
            ),
            ("cua-spacesd.exe_sha256", A),
        ]);
        assert_eq!(
            eval(&r, &format!("cua-spacesd=sha256:{A}")).status,
            Status::Pass
        );
        assert_eq!(
            eval(&r, &format!("cua-spacesd=sha256:{B}")).status,
            Status::Fail
        );
        assert_eq!(eval(&r, "cua-spacesd=git:0123456").status, Status::Pass);
        assert_eq!(eval(&r, "cua-spacesd=git:1123456").status, Status::Fail);
        assert_eq!(eval(&r, "cua-spacesd=0.1.0").status, Status::Pass);
        let c = eval(&r, "cua-spacesd=0.2.0");
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.severity, Severity::Required);
        assert!(c.message.contains("stale cua-spacesd"), "{}", c.message);
    }

    #[test]
    fn driver_sha_names_the_standalone_binary_only() {
        let linked_only = report(&[
            ("cua-driver.version", "0.9.0"),
            ("cua-driver.exe_sha256", A),
        ]);
        // The linked driver's executable is cua-spacesd: no standalone, fail.
        assert_eq!(
            eval(&linked_only, &format!("cua-driver=sha256:{A}")).status,
            Status::Fail
        );
        let both = report(&[
            ("cua-driver.version", "0.9.0"),
            ("cua-driver.git_sha", "abcdef1234"),
            ("cua-driver.exe_sha256", A),
            ("cua-driver.standalone.path", "/usr/local/bin/cua-driver"),
            ("cua-driver.standalone.version", "0.9.0"),
            ("cua-driver.standalone.git_sha", "abcdef1234"),
            ("cua-driver.standalone.exe_sha256", B),
        ]);
        assert_eq!(
            eval(&both, &format!("cua-driver=sha256:{B}")).status,
            Status::Pass
        );
        assert_eq!(
            eval(&both, &format!("cua-driver=sha256:{A}")).status,
            Status::Fail
        );
        // git and version must hold for every instance.
        assert_eq!(eval(&both, "cua-driver=git:abcdef1").status, Status::Pass);
        let skew = report(&[
            ("cua-driver.git_sha", "abcdef1234"),
            ("cua-driver.standalone.git_sha", "1111111111"),
        ]);
        let c = eval(&skew, "cua-driver=git:abcdef1");
        assert_eq!(c.status, Status::Fail);
        assert!(c.message.contains("standalone"), "{}", c.message);
    }

    #[test]
    fn a_guest_without_build_identity_is_stale() {
        let old = Report {
            spacesd: Some(Spacesd {
                version: "0.1.0".into(),
                cua_driver_version: "0.2.0".into(),
                ..Spacesd::default()
            }),
            ..Report::default()
        };
        assert_eq!(
            eval(&old, &format!("cua-spacesd=sha256:{A}")).status,
            Status::Fail
        );
        assert_eq!(eval(&old, "cua-spacesd=git:abcdef1").status, Status::Fail);
        // A version alone can still be checked.
        assert_eq!(eval(&old, "cua-spacesd=0.1.0").status, Status::Pass);
        assert_eq!(
            eval(&Report::default(), "cua-driver=0.2.0").status,
            Status::Fail
        );
    }

    #[test]
    fn overlays_by_name() {
        let r = report(&[
            ("overlay.mytool.path", "/opt/x"),
            ("overlay.mytool.sha256", A),
        ]);
        assert_eq!(eval(&r, &format!("mytool=sha256:{A}")).status, Status::Pass);
        assert_eq!(eval(&r, &format!("other=sha256:{A}")).status, Status::Fail);
    }

    #[test]
    fn apply_fails_the_finalized_report() {
        let mut r = report(&[("cua-spacesd.exe_sha256", A)]);
        apply(&mut r, &parse_all(&[format!("cua-spacesd={B}")]).unwrap());
        r.finalize(false, std::time::Duration::ZERO);
        assert_eq!(r.summary.status, Status::Fail);
        assert_eq!(r.exit_code(), 1);
    }
}
