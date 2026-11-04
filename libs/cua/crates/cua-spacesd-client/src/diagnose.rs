//! `SystemService.Diagnose`: client calls, and the report's JSON form.
//!
//! The JSON form (schema version 1, `libs/cua/proto/diagnose-report.schema.json`)
//! uses the proto field names, with enum values as lowercase short names
//! ("pass", "required") instead of proto3 JSON's `CHECK_STATUS_PASS`, so CI
//! can filter it with `jq '.summary.status == "fail"'`. [`Report`] converts
//! losslessly to and from [`pb::DiagnoseReport`].
//!
//! [`Report::finalize`] applies the severity rules every producer shares
//! (cua-spacesd, the doctor shim, the host-side `cua doctor`), and
//! [`Report::to_human`] / [`Report::to_junit`] render it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use base64::Engine as _;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SpacesdClient, pb};

/// `DiagnoseReport.schema_version` this module reads and writes.
pub const SCHEMA_VERSION: u32 = cua_proto::DIAGNOSE_REPORT_SCHEMA_VERSION;

/// Largest artifact inlined into a report (bigger ones keep only `path`).
pub const MAX_INLINE_ARTIFACT_BYTES: usize = 1024 * 1024;

/// Most events a `Diagnose` stream may carry before the client gives up.
pub const MAX_EVENTS: usize = 10_000;

/// Skip reasons that `strict` accepts on a required check.
pub const STRICT_ALLOWED_SKIPS: &[&str] = &[
    // Hardware encoders are not exercised until GPU runners exist; only
    // `CUA_CODEC_TEST_<BACKEND>=1` runs them.
    "hw_encoder_deferred",
    // The image has no display (headless build).
    "no_display_headless_image",
    // The check does not apply to this OS, runtime or variant.
    "not_applicable",
    // A feature the manifest lists as optional is absent.
    "optional_unavailable",
];

/// Outcome of one check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Works as claimed.
    #[default]
    Pass,
    /// A caveat, or something unclaimed is off.
    Warn,
    /// Broken.
    Fail,
    /// Not run.
    Skip,
}

impl Status {
    /// Lowercase name, as in the JSON form.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }

    fn from_pb(value: i32) -> Self {
        match pb::CheckStatus::try_from(value).unwrap_or_default() {
            pb::CheckStatus::Warn => Status::Warn,
            pb::CheckStatus::Fail => Status::Fail,
            pb::CheckStatus::Skip => Status::Skip,
            pb::CheckStatus::Pass | pb::CheckStatus::Unspecified => Status::Pass,
        }
    }

    fn to_pb(self) -> pb::CheckStatus {
        match self {
            Status::Pass => pb::CheckStatus::Pass,
            Status::Warn => pb::CheckStatus::Warn,
            Status::Fail => pb::CheckStatus::Fail,
            Status::Skip => pb::CheckStatus::Skip,
        }
    }
}

/// How much a check matters for the image, from its manifest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Claimed by the image: a failure fails the report.
    Required,
    /// Optional in the manifest: a failure is a warning.
    Recommended,
    /// Unclaimed: a failure is a warning.
    #[default]
    Info,
}

impl Severity {
    /// Lowercase name, as in the JSON form.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Required => "required",
            Severity::Recommended => "recommended",
            Severity::Info => "info",
        }
    }

    fn from_pb(value: i32) -> Self {
        match pb::CheckSeverity::try_from(value).unwrap_or_default() {
            pb::CheckSeverity::Required => Severity::Required,
            pb::CheckSeverity::Recommended => Severity::Recommended,
            pb::CheckSeverity::Info | pb::CheckSeverity::Unspecified => Severity::Info,
        }
    }

    fn to_pb(self) -> pb::CheckSeverity {
        match self {
            Severity::Required => pb::CheckSeverity::Required,
            Severity::Recommended => pb::CheckSeverity::Recommended,
            Severity::Info => pb::CheckSeverity::Info,
        }
    }
}

/// Which checks may act on the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effects {
    /// Read-only.
    #[default]
    None,
    /// Only on fixture windows the doctor launched on a virtual display.
    VirtualOnly,
}

impl Effects {
    /// Parses `none` / `virtual` (also `virtual_only`).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Effects::None),
            "virtual" | "virtual_only" | "virtual-only" => Some(Effects::VirtualOnly),
            _ => None,
        }
    }

    /// The proto value.
    pub fn to_pb(self) -> pb::DiagnoseEffects {
        match self {
            Effects::None => pb::DiagnoseEffects::None,
            Effects::VirtualOnly => pb::DiagnoseEffects::VirtualOnly,
        }
    }

    /// From the proto value (unspecified is `None`).
    pub fn from_pb(value: i32) -> Self {
        match pb::DiagnoseEffects::try_from(value).unwrap_or_default() {
            pb::DiagnoseEffects::VirtualOnly => Effects::VirtualOnly,
            _ => Effects::None,
        }
    }
}

/// Evidence produced by a check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// File name, unique within the report.
    pub name: String,
    /// Media type.
    pub media_type: String,
    /// Inlined contents (base64 in JSON); empty when too large.
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "b64")]
    pub data: Vec<u8>,
    /// Where the full file was written in the guest.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

/// The result of one check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Stable id, `<group>.<name>[.<detail>]`.
    pub id: String,
    /// Group (the id's first segment).
    pub group: String,
    /// Outcome.
    pub status: Status,
    /// Severity from the manifest.
    pub severity: Severity,
    /// Manifest claims behind the severity.
    #[serde(default)]
    pub claimed_by: Vec<String>,
    /// One-line outcome.
    #[serde(default)]
    pub message: String,
    /// What to do about it.
    #[serde(default)]
    pub remediation: String,
    /// Wall time in milliseconds.
    #[serde(default)]
    pub duration_ms: u32,
    /// Measured values.
    #[serde(default)]
    pub facts: BTreeMap<String, String>,
    /// Evidence.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

impl Check {
    /// A check with `id` (group taken from the id).
    pub fn new(id: impl Into<String>, status: Status, message: impl Into<String>) -> Self {
        let id = id.into();
        Check {
            group: id.split('.').next().unwrap_or_default().to_owned(),
            id,
            status,
            message: message.into(),
            ..Check::default()
        }
    }

    /// Adds a fact.
    pub fn fact(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.facts.insert(key.into(), value.to_string());
        self
    }

    /// Sets the remediation.
    pub fn fix(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = remediation.into();
        self
    }

    /// Marks a skip with a machine-readable reason (`facts.skip_reason`).
    pub fn skip_reason(self, reason: &str) -> Self {
        self.fact("skip_reason", reason)
    }

    fn from_pb(check: pb::DiagnoseCheck) -> Self {
        Check {
            id: check.id,
            group: check.group,
            status: Status::from_pb(check.status),
            severity: Severity::from_pb(check.severity),
            claimed_by: check.claimed_by,
            message: check.message,
            remediation: check.remediation,
            duration_ms: check.duration_ms,
            facts: check.facts.into_iter().collect(),
            artifacts: check
                .artifacts
                .into_iter()
                .map(|a| Artifact {
                    name: a.name,
                    media_type: a.media_type,
                    data: a.data,
                    path: a.path,
                })
                .collect(),
        }
    }

    /// The proto form.
    pub fn to_pb(&self) -> pb::DiagnoseCheck {
        pb::DiagnoseCheck {
            id: self.id.clone(),
            group: self.group.clone(),
            status: self.status.to_pb() as i32,
            severity: self.severity.to_pb() as i32,
            claimed_by: self.claimed_by.clone(),
            message: self.message.clone(),
            remediation: self.remediation.clone(),
            duration_ms: self.duration_ms,
            facts: self
                .facts
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            artifacts: self
                .artifacts
                .iter()
                .map(|a| pb::DiagnoseArtifact {
                    name: a.name.clone(),
                    media_type: a.media_type.clone(),
                    data: a.data.clone(),
                    path: a.path.clone(),
                })
                .collect(),
        }
    }
}

/// The daemon that ran the checks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spacesd {
    /// cua-spacesd version.
    pub version: String,
    /// `cua.env.v1` protocol revision.
    pub protocol_revision: u32,
    /// Source revision, when known.
    #[serde(default)]
    pub git_sha: String,
    /// Linked cua-driver core version.
    #[serde(default)]
    pub cua_driver_version: String,
}

/// The image under test.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// Image name.
    #[serde(default)]
    pub name: String,
    /// Reference, when known.
    #[serde(default, rename = "ref")]
    pub reference: String,
    /// "rootfs" or "containerdisk".
    #[serde(default)]
    pub variant: String,
    /// "linux", "windows" or "macos".
    #[serde(default)]
    pub os: String,
    /// SHA-256 of the manifest checked against.
    #[serde(default)]
    pub manifest_sha256: String,
    /// Where that manifest came from.
    #[serde(default)]
    pub manifest_source: String,
}

/// Where the guest runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// Runtime ("container", "gvisor", "qemu", "kubevirt", "lume", ...).
    #[serde(default)]
    pub runtime: String,
    /// Runtime detail.
    #[serde(default)]
    pub runtime_detail: String,
    /// "x86_64" or "arm64".
    #[serde(default)]
    pub arch: String,
    /// Init system.
    #[serde(default)]
    pub init: String,
    /// Display server.
    #[serde(default)]
    pub display_server: String,
    /// OS name and version.
    #[serde(default)]
    pub os: String,
}

/// Totals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Worst outcome.
    pub status: Status,
    /// Passed.
    pub pass: u32,
    /// Warnings.
    pub warn: u32,
    /// Failures.
    pub fail: u32,
    /// Skipped.
    pub skip: u32,
    /// Wall time in milliseconds.
    pub duration_ms: u32,
    /// Whether `strict` applied.
    #[serde(default)]
    pub strict: bool,
}

/// Facts that change task behaviour between variants. Every key is always
/// present so two reports diff key by key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fidelity {
    /// Primary display, "<w>x<h>@<scale>".
    pub display: String,
    /// Hash of the installed font files.
    pub fonts_sha256: String,
    /// Time zone.
    pub tz: String,
    /// Locale.
    pub locale: String,
    /// Guest minus caller clock, ms.
    pub clock_skew_ms: i64,
    /// Stream encoder.
    pub encoder: String,
    /// Accessibility backend.
    pub a11y_backend: String,
    /// Audio backend.
    pub audio_backend: String,
    /// CPU model.
    pub cpu_model: String,
    /// Logical CPUs.
    pub cpu_count: u32,
    /// Memory, MiB.
    pub memory_mib: u64,
    /// GPU, or "none".
    pub gpu: String,
    /// Kernel release.
    pub kernel: String,
    /// Runtime.
    pub runtime: String,
    /// Init system.
    pub init: String,
    /// Hash of the installed package list.
    pub packages_sha256: String,
    /// Versions of manifest-listed apps.
    pub app_versions: BTreeMap<String, String>,
    /// Versions of manifest-listed tools (dev tiers).
    #[serde(default)]
    pub tool_versions: BTreeMap<String, String>,
    /// Available simulator runtimes (`xcrun simctl list runtimes`), sorted;
    /// empty unless the image claims some.
    #[serde(default)]
    pub simulator_runtimes: Vec<String>,
}

impl Fidelity {
    /// Every key as a flat string map (`app_versions.<app>` for apps), the
    /// form the parity diff compares.
    pub fn flatten(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let value = serde_json::to_value(self).unwrap_or_default();
        if let serde_json::Value::Object(map) = value {
            for (key, value) in map {
                match value {
                    serde_json::Value::Object(apps) => {
                        for (app, version) in apps {
                            out.insert(
                                format!("{key}.{app}"),
                                version.as_str().unwrap_or_default().to_owned(),
                            );
                        }
                    }
                    serde_json::Value::String(s) => {
                        out.insert(key, s);
                    }
                    other => {
                        out.insert(key, other.to_string());
                    }
                }
            }
        }
        out
    }
}

/// A whole report, JSON schema version 1.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Always [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Producer ("cua-spacesd", "cua-doctor-shim/<target>").
    #[serde(default)]
    pub producer: String,
    /// When the run started (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<pbjson_types::Timestamp>,
    /// The daemon; `null` for shim reports. Reports from images built
    /// before the cua-spacesd rename call it `guestd`.
    #[serde(alias = "guestd")]
    pub spacesd: Option<Spacesd>,
    /// The image.
    pub image: Image,
    /// Where it runs.
    pub environment: Environment,
    /// Totals.
    pub summary: Summary,
    /// Every check, in run order.
    pub checks: Vec<Check>,
    /// Variant-comparable facts.
    pub fidelity: Fidelity,
}

impl Report {
    /// Parses and version-checks a JSON report.
    pub fn from_json(text: &str) -> std::result::Result<Self, String> {
        let report: Report = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if report.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "report schema_version {} is not {SCHEMA_VERSION}",
                report.schema_version
            ));
        }
        Ok(report)
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serializes")
    }

    /// From the proto form.
    pub fn from_pb(report: pb::DiagnoseReport) -> Self {
        let fidelity = report.fidelity.unwrap_or_default();
        let summary = report.summary.unwrap_or_default();
        let image = report.image.unwrap_or_default();
        let environment = report.environment.unwrap_or_default();
        Report {
            schema_version: report.schema_version,
            producer: report.producer,
            started_at: report.started_at,
            spacesd: report.spacesd.map(|g| Spacesd {
                version: g.version,
                protocol_revision: g.protocol_revision,
                git_sha: g.git_sha,
                cua_driver_version: g.cua_driver_version,
            }),
            image: Image {
                name: image.name,
                reference: image.r#ref,
                variant: image.variant,
                os: image.os,
                manifest_sha256: image.manifest_sha256,
                manifest_source: image.manifest_source,
            },
            environment: Environment {
                runtime: environment.runtime,
                runtime_detail: environment.runtime_detail,
                arch: environment.arch,
                init: environment.init,
                display_server: environment.display_server,
                os: environment.os,
            },
            summary: Summary {
                status: Status::from_pb(summary.status),
                pass: summary.pass,
                warn: summary.warn,
                fail: summary.fail,
                skip: summary.skip,
                duration_ms: summary.duration_ms,
                strict: summary.strict,
            },
            checks: report.checks.into_iter().map(Check::from_pb).collect(),
            fidelity: Fidelity {
                display: fidelity.display,
                fonts_sha256: fidelity.fonts_sha256,
                tz: fidelity.tz,
                locale: fidelity.locale,
                clock_skew_ms: fidelity.clock_skew_ms,
                encoder: fidelity.encoder,
                a11y_backend: fidelity.a11y_backend,
                audio_backend: fidelity.audio_backend,
                cpu_model: fidelity.cpu_model,
                cpu_count: fidelity.cpu_count,
                memory_mib: fidelity.memory_mib,
                gpu: fidelity.gpu,
                kernel: fidelity.kernel,
                runtime: fidelity.runtime,
                init: fidelity.init,
                packages_sha256: fidelity.packages_sha256,
                app_versions: fidelity.app_versions.into_iter().collect(),
                tool_versions: fidelity.tool_versions.into_iter().collect(),
                simulator_runtimes: fidelity.simulator_runtimes,
            },
        }
    }

    /// The proto form.
    pub fn to_pb(&self) -> pb::DiagnoseReport {
        let f = &self.fidelity;
        pb::DiagnoseReport {
            schema_version: self.schema_version,
            producer: self.producer.clone(),
            started_at: self.started_at,
            spacesd: self.spacesd.as_ref().map(|g| pb::DiagnoseSpacesd {
                version: g.version.clone(),
                protocol_revision: g.protocol_revision,
                git_sha: g.git_sha.clone(),
                cua_driver_version: g.cua_driver_version.clone(),
            }),
            image: Some(pb::DiagnoseImage {
                name: self.image.name.clone(),
                r#ref: self.image.reference.clone(),
                variant: self.image.variant.clone(),
                os: self.image.os.clone(),
                manifest_sha256: self.image.manifest_sha256.clone(),
                manifest_source: self.image.manifest_source.clone(),
            }),
            environment: Some(pb::DiagnoseEnvironment {
                runtime: self.environment.runtime.clone(),
                runtime_detail: self.environment.runtime_detail.clone(),
                arch: self.environment.arch.clone(),
                init: self.environment.init.clone(),
                display_server: self.environment.display_server.clone(),
                os: self.environment.os.clone(),
            }),
            summary: Some(pb::DiagnoseSummary {
                status: self.summary.status.to_pb() as i32,
                pass: self.summary.pass,
                warn: self.summary.warn,
                fail: self.summary.fail,
                skip: self.summary.skip,
                duration_ms: self.summary.duration_ms,
                strict: self.summary.strict,
            }),
            checks: self.checks.iter().map(Check::to_pb).collect(),
            fidelity: Some(pb::DiagnoseFidelity {
                display: f.display.clone(),
                fonts_sha256: f.fonts_sha256.clone(),
                tz: f.tz.clone(),
                locale: f.locale.clone(),
                clock_skew_ms: f.clock_skew_ms,
                encoder: f.encoder.clone(),
                a11y_backend: f.a11y_backend.clone(),
                audio_backend: f.audio_backend.clone(),
                cpu_model: f.cpu_model.clone(),
                cpu_count: f.cpu_count,
                memory_mib: f.memory_mib,
                gpu: f.gpu.clone(),
                kernel: f.kernel.clone(),
                runtime: f.runtime.clone(),
                init: f.init.clone(),
                packages_sha256: f.packages_sha256.clone(),
                app_versions: f
                    .app_versions
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                tool_versions: f
                    .tool_versions
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                simulator_runtimes: f.simulator_runtimes.clone(),
            }),
        }
    }

    /// Applies the shared rules and recomputes `summary`:
    ///
    /// - a failed check that is not `required` becomes a warning;
    /// - under `strict`, a skipped `required` check fails unless its
    ///   `facts.skip_reason` is in [`STRICT_ALLOWED_SKIPS`];
    /// - the summary is `fail` when any check failed, or under `strict`
    ///   when any warned; else `warn` when any warned; else `pass`;
    /// - a passing check carries no remediation.
    pub fn finalize(&mut self, strict: bool, duration: Duration) {
        for check in &mut self.checks {
            // Remediation is advice for something to fix.
            if check.status == Status::Pass {
                check.remediation.clear();
            }
            if check.status == Status::Fail && check.severity != Severity::Required {
                check.status = Status::Warn;
                check.facts.insert("downgraded_from".into(), "fail".into());
            }
            if strict && check.status == Status::Skip && check.severity == Severity::Required {
                let reason = check.facts.get("skip_reason").cloned().unwrap_or_default();
                if !STRICT_ALLOWED_SKIPS.contains(&reason.as_str()) {
                    check.status = Status::Fail;
                    check.message = format!("skipped a required check under --strict: {}", {
                        let m = check.message.clone();
                        if m.is_empty() { reason } else { m }
                    });
                }
            }
        }
        let count = |s: Status| self.checks.iter().filter(|c| c.status == s).count() as u32;
        let (pass, warn, fail, skip) = (
            count(Status::Pass),
            count(Status::Warn),
            count(Status::Fail),
            count(Status::Skip),
        );
        let status = if fail > 0 || (strict && warn > 0) {
            Status::Fail
        } else if warn > 0 {
            Status::Warn
        } else {
            Status::Pass
        };
        self.summary = Summary {
            status,
            pass,
            warn,
            fail,
            skip,
            duration_ms: duration.as_millis().min(u32::MAX as u128) as u32,
            strict,
        };
    }

    /// Exit code for a CLI: 0 on pass or warn (without strict), 1 on fail.
    pub fn exit_code(&self) -> i32 {
        if self.summary.status == Status::Fail {
            1
        } else {
            0
        }
    }

    /// `[ok] / [warn] / [fail] / [skip]` lines grouped by group.
    pub fn to_human(&self) -> String {
        let mut out = String::new();
        let who = match &self.spacesd {
            Some(g) => format!("cua-spacesd {}", g.version),
            None => self.producer.clone(),
        };
        let _ = writeln!(
            out,
            "{who}: image {} ({} {}) on {} {}{}",
            if self.image.name.is_empty() {
                "<no manifest>"
            } else {
                &self.image.name
            },
            self.image.os,
            self.image.variant,
            self.environment.runtime,
            self.environment.arch,
            if self.environment.init.is_empty() {
                String::new()
            } else {
                format!(", init {}", self.environment.init)
            }
        );
        let mut group = "";
        for check in &self.checks {
            if check.group != group {
                group = &check.group;
                let _ = writeln!(out, "{group}");
            }
            let tag = match check.status {
                Status::Pass => "[ok]  ",
                Status::Warn => "[warn]",
                Status::Fail => "[fail]",
                Status::Skip => "[skip]",
            };
            let _ = writeln!(out, "  {tag} {}: {}", check.id, check.message);
            if check.status != Status::Pass && !check.remediation.is_empty() {
                let _ = writeln!(out, "         fix: {}", check.remediation);
            }
        }
        let s = &self.summary;
        let _ = writeln!(
            out,
            "{}: {} passed, {} warnings, {} failed, {} skipped in {} ms{}",
            s.status.as_str(),
            s.pass,
            s.warn,
            s.fail,
            s.skip,
            s.duration_ms,
            if s.strict { " (strict)" } else { "" }
        );
        out
    }

    /// JUnit XML: one testsuite per group, one testcase per check.
    pub fn to_junit(&self) -> String {
        let mut groups: BTreeMap<&str, Vec<&Check>> = BTreeMap::new();
        for check in &self.checks {
            groups.entry(&check.group).or_default().push(check);
        }
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(
            out,
            "<testsuites name=\"cua doctor\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">",
            self.checks.len(),
            self.summary.fail
                + if self.summary.strict {
                    self.summary.warn
                } else {
                    0
                },
            self.summary.skip,
            self.summary.duration_ms as f64 / 1000.0
        );
        for (group, checks) in groups {
            let failures = checks
                .iter()
                .filter(|c| {
                    c.status == Status::Fail || (self.summary.strict && c.status == Status::Warn)
                })
                .count();
            let skipped = checks.iter().filter(|c| c.status == Status::Skip).count();
            let _ = writeln!(
                out,
                "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" skipped=\"{skipped}\">",
                xml_escape(group),
                checks.len()
            );
            for check in checks {
                let _ = write!(
                    out,
                    "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                    xml_escape(group),
                    xml_escape(&check.id),
                    check.duration_ms as f64 / 1000.0
                );
                let body = match check.status {
                    Status::Pass => None,
                    Status::Skip => Some(format!(
                        "<skipped message=\"{}\"/>",
                        xml_escape(&check.message)
                    )),
                    Status::Warn if !self.summary.strict => Some(format!(
                        "<system-out>warning: {}</system-out>",
                        xml_escape(&check.message)
                    )),
                    Status::Warn | Status::Fail => Some(format!(
                        "<failure message=\"{}\" type=\"{}\">{}</failure>",
                        xml_escape(&check.message),
                        check.severity.as_str(),
                        xml_escape(&check.remediation)
                    )),
                };
                match body {
                    None => out.push_str("/>\n"),
                    Some(body) => {
                        let _ = writeln!(out, ">{body}</testcase>");
                    }
                }
            }
            out.push_str("  </testsuite>\n");
        }
        out.push_str("</testsuites>\n");
        out
    }
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most control characters.
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// SHA-256 over the tool registry's schemas: one `name\tinput_schema\n`
/// line per tool, sorted by name, with each schema re-serialized from its
/// parsed JSON so formatting differences do not count. Pinned in the image
/// manifest by `cua-spacesd build-info` and re-checked by the doctor.
pub fn tools_sha256(tools: &[pb::ToolInfo]) -> String {
    use sha2::{Digest, Sha256};
    let mut lines: Vec<String> = tools
        .iter()
        .map(|t| {
            let schema = serde_json::from_str::<serde_json::Value>(&t.input_schema_json)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| t.input_schema_json.clone());
            format!("{}\t{schema}\n", t.name)
        })
        .collect();
    lines.sort();
    hex::encode(Sha256::digest(lines.concat().as_bytes()))
}

/// Encodes artifact bytes for display (base64).
pub fn encode_artifact(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// A server-streamed `Diagnose` run.
pub type DiagnoseStream = tonic::Streaming<pb::DiagnoseResponse>;

impl SpacesdClient {
    /// `SystemService.Diagnose`: the raw event stream.
    pub async fn diagnose(&self, options: pb::DiagnoseOptions) -> Result<DiagnoseStream> {
        Ok(self
            .system()
            .diagnose(pb::DiagnoseRequest {
                options: Some(options),
            })
            .await?
            .into_inner())
    }

    /// `SystemService.DiagnoseOnce`: only the final report.
    pub async fn diagnose_once(&self, options: pb::DiagnoseOptions) -> Result<Report> {
        let report = self
            .system()
            .diagnose_once(pb::DiagnoseOnceRequest {
                options: Some(options),
            })
            .await?
            .into_inner()
            .report
            .ok_or_else(|| Error::Protocol("DiagnoseOnce returned no report".into()))?;
        Ok(Report::from_pb(report))
    }

    /// Runs `Diagnose`, calling `on_check` for each finished check, and
    /// returns the report. Falls back to `DiagnoseOnce` when the stream
    /// breaks before its first event (proxies that buffer or drop server
    /// streams). Bounded: at most [`MAX_EVENTS`] events, and the options'
    /// timeout (default 120 s) plus 30 s overall.
    pub async fn diagnose_report(
        &self,
        options: pb::DiagnoseOptions,
        mut on_check: impl FnMut(&Check),
    ) -> Result<Report> {
        let budget = options
            .timeout
            .as_ref()
            .map(|d| Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32))
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(120))
            + Duration::from_secs(30);
        let run = async {
            let mut stream = match self.diagnose(options.clone()).await {
                Ok(stream) => stream,
                Err(error) if fallback_worthy(&error) => {
                    return self.diagnose_once(options.clone()).await;
                }
                Err(error) => return Err(error),
            };
            let mut seen = 0usize;
            loop {
                let event = match stream.next().await {
                    Some(Ok(event)) => event,
                    Some(Err(status)) => {
                        let error = Error::from(status);
                        if seen == 0 && fallback_worthy(&error) {
                            return self.diagnose_once(options.clone()).await;
                        }
                        return Err(error);
                    }
                    None => {
                        return Err(Error::Protocol(
                            "Diagnose stream ended without a report".into(),
                        ));
                    }
                };
                seen += 1;
                if seen > MAX_EVENTS {
                    return Err(Error::Protocol(format!(
                        "Diagnose sent more than {MAX_EVENTS} events"
                    )));
                }
                match event.event {
                    Some(pb::diagnose_response::Event::Check(check)) => {
                        on_check(&Check::from_pb(check));
                    }
                    Some(pb::diagnose_response::Event::Report(report)) => {
                        return Ok(Report::from_pb(report));
                    }
                    Some(pb::diagnose_response::Event::Started(_)) | None => {}
                }
            }
        };
        tokio::time::timeout(budget, run)
            .await
            .map_err(|_| Error::Timeout(budget))?
    }
}

/// Transport-level failures (not a verdict from the guest) that justify
/// retrying as `DiagnoseOnce`.
fn fallback_worthy(error: &Error) -> bool {
    matches!(
        error.code(),
        Some(tonic::Code::Unavailable | tonic::Code::Internal | tonic::Code::Unknown)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Report {
        let mut report = Report {
            schema_version: SCHEMA_VERSION,
            producer: "cua-spacesd".into(),
            started_at: Some(pbjson_types::Timestamp {
                seconds: 1_790_000_000,
                nanos: 0,
            }),
            spacesd: Some(Spacesd {
                version: "0.1.0".into(),
                protocol_revision: 3,
                git_sha: "abc".into(),
                cua_driver_version: "0.3.0".into(),
            }),
            image: Image {
                name: "linux".into(),
                reference: "ghcr.io/trycua/linux@sha256:00".into(),
                variant: "rootfs".into(),
                os: "linux".into(),
                manifest_sha256: "ff".into(),
                manifest_source: "/etc/cua-image/manifest.json".into(),
            },
            environment: Environment {
                runtime: "gvisor".into(),
                runtime_detail: "runsc".into(),
                arch: "arm64".into(),
                init: "supervisord".into(),
                display_server: "x11".into(),
                os: "Ubuntu 24.04".into(),
            },
            checks: vec![
                Check {
                    severity: Severity::Required,
                    claimed_by: vec!["feature:desktop_stream".into()],
                    artifacts: vec![Artifact {
                        name: "a.png".into(),
                        media_type: "image/png".into(),
                        data: vec![1, 2, 3],
                        path: "/var/tmp/a.png".into(),
                    }],
                    ..Check::new("stream.encode.openh264", Status::Pass, "ok")
                        .fact("psnr_db", "41.2")
                },
                Check {
                    severity: Severity::Info,
                    ..Check::new("network.egress", Status::Fail, "no route")
                },
                Check {
                    severity: Severity::Required,
                    ..Check::new("stream.encode.vaapi", Status::Skip, "deferred")
                        .skip_reason("hw_encoder_deferred")
                },
            ],
            fidelity: Fidelity {
                display: "1280x800@1".into(),
                app_versions: [("firefox".to_owned(), "130.0".to_owned())].into(),
                ..Fidelity::default()
            },
            ..Report::default()
        };
        report.finalize(false, Duration::from_millis(1234));
        report
    }

    #[test]
    fn severity_rules_and_summary() {
        let report = sample();
        // An unclaimed failure is downgraded to a warning.
        assert_eq!(report.checks[1].status, Status::Warn);
        assert_eq!(report.checks[1].facts["downgraded_from"], "fail");
        assert_eq!(
            (
                report.summary.pass,
                report.summary.warn,
                report.summary.fail
            ),
            (1, 1, 0)
        );
        assert_eq!(report.summary.status, Status::Warn);
        assert_eq!(report.exit_code(), 0);
        // Strict: warnings fail; an allowlisted skip stays a skip.
        let mut strict = report.clone();
        strict.finalize(true, Duration::ZERO);
        assert_eq!(strict.summary.status, Status::Fail);
        assert_eq!(strict.checks[2].status, Status::Skip);
        assert_eq!(strict.exit_code(), 1);
        // A required skip without an allowlisted reason fails under strict.
        let mut skipped = Report {
            checks: vec![Check {
                severity: Severity::Required,
                ..Check::new("a11y.tree", Status::Skip, "no bus")
            }],
            ..Report::default()
        };
        skipped.finalize(true, Duration::ZERO);
        assert_eq!(skipped.checks[0].status, Status::Fail);
        assert!(skipped.checks[0].message.contains("--strict"));
    }

    #[test]
    fn json_uses_proto_field_names_and_short_enums() {
        let json: serde_json::Value = serde_json::from_str(&sample().to_json()).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["summary"]["status"], "warn");
        assert_eq!(json["checks"][0]["severity"], "required");
        assert_eq!(json["checks"][0]["artifacts"][0]["data"], "AQID");
        assert_eq!(json["image"]["ref"], "ghcr.io/trycua/linux@sha256:00");
        assert_eq!(json["started_at"], "2026-09-21T14:13:20+00:00");
        assert_eq!(json["fidelity"]["app_versions"]["firefox"], "130.0");
    }

    #[test]
    fn proto_and_json_round_trip() {
        let report = sample();
        assert_eq!(Report::from_pb(report.to_pb()), report);
        assert_eq!(Report::from_json(&report.to_json()).unwrap(), report);
        // Reports from images built before the rename say `guestd`.
        let older = report.to_json().replacen("\"spacesd\":", "\"guestd\":", 1);
        assert!(older.contains("\"guestd\":"));
        assert_eq!(Report::from_json(&older).unwrap(), report);
        let mut wrong = serde_json::to_value(&report).unwrap();
        wrong["schema_version"] = 2.into();
        assert!(Report::from_json(&wrong.to_string()).is_err());
    }

    /// The checked-in JSON Schema and this model name the same keys, so the
    /// schema cannot drift from what producers write.
    #[test]
    fn schema_file_matches_the_model() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../proto/diagnose-report.schema.json"))
                .unwrap();
        fn keys(v: &serde_json::Value) -> Vec<String> {
            let mut k: Vec<String> = v
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            k.sort();
            k
        }
        let json = serde_json::to_value(sample()).unwrap();
        let props = &schema["properties"];
        assert_eq!(keys(props), keys(&json), "top-level keys");
        let defs = &schema["$defs"];
        for (field, def) in [
            ("spacesd", "spacesd"),
            ("image", "image"),
            ("environment", "environment"),
            ("summary", "summary"),
            ("fidelity", "fidelity"),
        ] {
            assert_eq!(
                keys(&defs[def]["properties"]),
                keys(&json[field]),
                "{field} keys"
            );
            let mut required: Vec<String> = defs[def]["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            required.sort();
            if field == "fidelity" {
                assert_eq!(
                    required,
                    keys(&json[field]),
                    "fidelity keys are all required"
                );
            }
        }
        assert_eq!(
            keys(&defs["check"]["properties"]),
            keys(&json["checks"][0]),
            "check keys"
        );
        assert_eq!(
            keys(&defs["artifact"]["properties"]),
            keys(&json["checks"][0]["artifacts"][0]),
            "artifact keys"
        );
        let statuses: Vec<&str> = defs["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(statuses, ["pass", "warn", "fail", "skip"]);
    }

    #[test]
    fn junit_and_human_render() {
        let report = sample();
        let junit = report.to_junit();
        assert!(
            junit.contains("<testsuite name=\"stream\" tests=\"2\" failures=\"0\" skipped=\"1\">")
        );
        assert!(junit.contains("<skipped message=\"deferred\"/>"));
        assert!(junit.contains("warning: no route"));
        let human = report.to_human();
        assert!(human.contains("[ok]   stream.encode.openh264: ok"));
        assert!(human.contains("[warn] network.egress: no route"));
        assert!(human.ends_with("in 1234 ms\n"), "{human}");
        assert_eq!(xml_escape("<a&\"b\u{1}>"), "&lt;a&amp;&quot;b &gt;");
    }

    #[test]
    fn tools_hash_ignores_order_and_formatting() {
        let tool = |name: &str, schema: &str| pb::ToolInfo {
            name: name.into(),
            input_schema_json: schema.into(),
            ..Default::default()
        };
        let a = tools_sha256(&[tool("b", r#"{"type": "object"}"#), tool("a", "{}")]);
        let b = tools_sha256(&[tool("a", "{ }"), tool("b", r#"{"type":"object"}"#)]);
        assert_eq!(a, b);
        assert_ne!(a, tools_sha256(&[tool("a", "{}")]));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn fidelity_flattens_every_key() {
        let flat = sample().fidelity.flatten();
        assert_eq!(flat["display"], "1280x800@1");
        assert_eq!(flat["app_versions.firefox"], "130.0");
        assert_eq!(flat["clock_skew_ms"], "0");
        assert!(flat.contains_key("gpu"));
    }

    #[tokio::test]
    async fn diagnose_report_streams_and_falls_back() {
        use crate::testing::MockServer;
        let server = MockServer::start(Default::default()).await;
        let client = SpacesdClient::connect_url(&server.url(), None)
            .await
            .unwrap();
        server.state.set_diagnose_report(sample().to_pb());
        let mut seen = Vec::new();
        let report = client
            .diagnose_report(Default::default(), |c| seen.push(c.id.clone()))
            .await
            .unwrap();
        assert_eq!(report, sample());
        assert_eq!(seen.len(), 3);
        // DiagnoseOnce answers the same report.
        assert_eq!(
            client.diagnose_once(Default::default()).await.unwrap(),
            sample()
        );
    }
}
