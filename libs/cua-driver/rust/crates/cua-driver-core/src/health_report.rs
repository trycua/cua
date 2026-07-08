//! `health_report` — single-call end-to-end driver diagnostics.
//!
//! The point of this tool is to let downstream consumers ship one stable
//! diagnostic call and never have to know cua-driver internals: specific
//! MCP tool names, TCC field names, bundle IDs, per-platform check
//! matrix. cua-driver owns the health model entirely; consumers stay
//! thin and the driver evolves freely.
//!
//! ## Stability
//!
//! Output shape is the stable contract — documented inline in the
//! tool's `description`. `schema_version: "1"` is the commitment;
//! future breaking changes go to `"2"`. Adding new `name` values to
//! `checks[]` under the same `schema_version` is non-breaking;
//! consumers must tolerate unknown check names.
//!
//! ## Architecture
//!
//! This module ports the Swift `HealthReportTool` (closed PR #1905) to
//! the canonical Rust workspace. The check matrix differs per platform,
//! so the per-platform crates (`platform-macos`, `platform-windows`,
//! `platform-linux`) supply a [`HealthCheckProvider`] and a list of
//! check names; this module owns the cross-platform skeleton:
//! schema shape, status rollup, include/skip filtering, JSON-RPC tool
//! plumbing, and the text summary.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

// ── Canonical check names ────────────────────────────────────────────────────
//
// Constants live here (not on each platform) so the wire format stays
// consistent and downstream consumers can hard-code these strings
// against the documented `schema_version=1` contract.

pub const NAME_BINARY_VERSION: &str = "binary_version";
pub const NAME_PLATFORM_SUPPORTED: &str = "platform_supported";
pub const NAME_SESSION_ACTIVE: &str = "session_active";
pub const NAME_BUNDLE_IDENTITY: &str = "bundle_identity";
pub const NAME_TCC_ACCESSIBILITY: &str = "tcc_accessibility";
pub const NAME_TCC_SCREEN_RECORDING: &str = "tcc_screen_recording";
pub const NAME_AX_CAPABILITY: &str = "ax_capability";
pub const NAME_SCREEN_CAPTURE_CAPABILITY: &str = "screen_capture_capability";

/// Checks whose failure marks the whole report as `failed` (vs
/// `degraded`). Binary, platform, and the MCP session itself are
/// non-negotiable; everything else is degraded.
///
/// This list is fixed across platforms — a "core" check that doesn't
/// apply on a platform simply isn't present in that platform's `names`
/// vector, so the overall rollup ignores it.
pub fn core_check_names() -> &'static [&'static str] {
    &[NAME_BINARY_VERSION, NAME_PLATFORM_SUPPORTED, NAME_SESSION_ACTIVE]
}

// ── Output model ─────────────────────────────────────────────────────────────

/// Per-check status. Serialized lowercase to match the documented contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Fail,
    Skip,
}

/// Aggregate health verdict. Lowercase wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Overall {
    Ok,
    Degraded,
    Failed,
}

/// Per-check structured data. Kept as a fixed set of optional fields —
/// not a free-form map — so the JSON shape is statically known. Adding
/// new optional fields here is non-breaking under `schema_version="1"`;
/// removing any documented field would break consumers and gates a
/// version bump.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

impl CheckData {
    /// True if no field is set. Used by check authors so an empty
    /// `data` block is omitted from the wire format rather than
    /// serialized as `{}`.
    pub fn is_empty(&self) -> bool {
        self.bundle_identifier.is_none()
            && self.executable_path.is_none()
            && self.os_version.is_none()
            && self.architecture.is_none()
            && self.display_count.is_none()
            && self.error_detail.is_none()
    }
}

/// One entry in `report.checks[]`.
///
/// `hint` is required on `Fail` (the documented "fail entries carry a
/// remediation step" contract), optional on `Pass`/`Skip`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckEntry {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<CheckData>,
}

impl CheckEntry {
    /// Build a `Pass` entry with no hint and no structured data.
    pub fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Pass,
            message: message.into(),
            hint: None,
            data: None,
        }
    }

    /// Build a `Fail` entry. `hint` is required on the contract level —
    /// taking it as a non-`Option` argument makes the requirement
    /// explicit at the call site rather than catching it in a test.
    pub fn fail(
        name: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Fail,
            message: message.into(),
            hint: Some(hint.into()),
            data: None,
        }
    }

    /// Build a `Skip` entry — used when a check is filtered out by the
    /// caller's `include`/`skip` or doesn't apply to this platform.
    pub fn skip(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Skip,
            message: message.into(),
            hint: None,
            data: None,
        }
    }

    /// Attach structured `data`. Empty payloads are dropped so the wire
    /// format stays clean.
    pub fn with_data(mut self, data: CheckData) -> Self {
        if !data.is_empty() {
            self.data = Some(data);
        }
        self
    }
}

/// Top-level `health_report` payload.
///
/// JSON shape is the consumer-facing contract documented in the tool's
/// description. Field-rename in serde keeps the snake_case wire format
/// regardless of Rust naming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: String,
    pub platform: String,
    pub driver_version: String,
    pub overall: Overall,
    pub checks: Vec<CheckEntry>,
}

// ── Provider trait ───────────────────────────────────────────────────────────

/// Per-platform contract: list the canonical check names this platform
/// supports, and run a single check by name.
///
/// Implementations live in each `platform-*` crate so the
/// platform-specific TCC / UIA / AT-SPI / SCK plumbing stays out of the
/// core. Implementations must return entries for every name in their
/// `check_names()`; the dispatcher does not synthesize results.
#[async_trait]
pub trait HealthCheckProvider: Send + Sync {
    /// The platform tag for the `Report.platform` field — `"darwin"`,
    /// `"win32"`, or `"linux"`. The documented stable contract.
    fn platform(&self) -> &'static str;

    /// Canonical check names, in the run order the consumer should see
    /// reflected back in `report.checks[]`.
    fn check_names(&self) -> &'static [&'static str];

    /// Run a single check by name. The dispatcher only forwards names
    /// from `check_names()`; provider authors do not need to handle
    /// unknown names defensively.
    async fn run_check(&self, name: &str) -> CheckEntry;
}

// ── Filter / rollup logic ────────────────────────────────────────────────────

/// Decide which checks the call should actually run.
///
///   - `include` wins if non-empty: run exactly the canonical names
///     that intersect `include`. Unknown names in `include` are
///     silently ignored (forward-compat — a consumer may know a name
///     from a newer driver build).
///   - Otherwise `skip` filters out from the full canonical list.
///   - Empty filters → run everything.
pub fn select_checks(
    all_checks: &[&str],
    include: &BTreeSet<String>,
    skip: &BTreeSet<String>,
) -> BTreeSet<String> {
    let all: BTreeSet<String> = all_checks.iter().map(|s| (*s).to_owned()).collect();
    if !include.is_empty() {
        return all.intersection(include).cloned().collect();
    }
    if !skip.is_empty() {
        return all.difference(skip).cloned().collect();
    }
    all
}

/// Roll per-check statuses up into the documented overall verdict.
pub fn compute_overall(checks: &[CheckEntry]) -> Overall {
    let core: BTreeSet<&str> = core_check_names().iter().copied().collect();
    let mut any_fail = false;
    let mut any_core_fail = false;
    for entry in checks {
        if entry.status != CheckStatus::Fail {
            continue;
        }
        any_fail = true;
        if core.contains(entry.name.as_str()) {
            any_core_fail = true;
        }
    }
    if any_core_fail {
        return Overall::Failed;
    }
    if any_fail {
        return Overall::Degraded;
    }
    Overall::Ok
}

/// Pull a `BTreeSet<String>` out of a JSON value, treating non-array
/// inputs and non-string items as empty. Used to parse the optional
/// `include` / `skip` arguments.
pub fn parse_string_set(v: Option<&Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(Value::Array(items)) = v else {
        return out;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            out.insert(s.to_owned());
        }
    }
    out
}

// ── Text summary ─────────────────────────────────────────────────────────────

/// Compact human-readable summary of a report, for clients squinting at
/// MCP traces. `structuredContent` is the authoritative wire format;
/// this text is decorative.
pub fn text_summary(report: &Report) -> String {
    let overall_icon = match report.overall {
        Overall::Ok => "✅",
        Overall::Degraded => "⚠️",
        Overall::Failed => "❌",
    };
    let overall_word = match report.overall {
        Overall::Ok => "ok",
        Overall::Degraded => "degraded",
        Overall::Failed => "failed",
    };
    let mut lines = vec![format!(
        "{overall_icon} cua-driver {} on {} — {overall_word}",
        report.driver_version, report.platform
    )];
    for entry in &report.checks {
        let icon = match entry.status {
            CheckStatus::Pass => "✅",
            CheckStatus::Fail => "❌",
            CheckStatus::Skip => "⏭",
        };
        lines.push(format!("  {icon} {}: {}", entry.name, entry.message));
    }
    lines.join("\n")
}

// ── The MCP tool ─────────────────────────────────────────────────────────────

/// Cross-platform `health_report` MCP tool. The platform-specific bits
/// (which checks to run, how to run each one) are supplied by the
/// `HealthCheckProvider` passed at construction time.
pub struct HealthReportTool {
    provider: Arc<dyn HealthCheckProvider>,
}

impl HealthReportTool {
    pub fn new(provider: Arc<dyn HealthCheckProvider>) -> Self {
        Self { provider }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "health_report".into(),
        // The description is part of the public contract — downstream
        // consumers depend on it spelling out `schema_version="1"` and the per-
        // platform check matrix. A test pins this commitment.
        description: r#"Single-call end-to-end driver diagnostics. Designed to let downstream consumers ship one stable call instead of stitching together check_permissions, doctor, version, bundle attribution, and a screenshot probe. cua-driver owns the health model; consumers stay thin.

Input — all optional:
  {
    "include": ["<check_name>", ...],   // run only these
    "skip":    ["<check_name>", ...]    // skip these
  }
If both are given, `include` wins.

Canonical check names:
  macOS  : binary_version, platform_supported, session_active,
           bundle_identity, tcc_accessibility, tcc_screen_recording,
           ax_capability, screen_capture_capability
  Windows: binary_version, platform_supported, session_active,
           ax_capability (via UIA), screen_capture_capability (via DXGI)
  Linux  : binary_version, platform_supported, session_active,
           ax_capability (via AT-SPI), screen_capture_capability (via X11)

Output — stable contract, schema_version="1":
  {
    "schema_version": "1",
    "platform": "darwin" | "win32" | "linux",
    "driver_version": "<semver>",
    "overall": "ok" | "degraded" | "failed",
    "checks": [
      {
        "name": "<one of the canonical names above>",
        "status": "pass" | "fail" | "skip",
        "message": "<one-line summary, always present>",
        "hint": "<remediation step, present when status=fail>",
        "data": { /* check-specific structured fields */ }
      },
      ...
    ]
  }

`overall` rules:
  - `ok`       — every non-skipped check passes
  - `degraded` — at least one non-core check fails (binary is still usable)
  - `failed`   — any core check fails (binary_version, platform_supported, session_active)

Stability: schema_version="1" is the contract. Future breaking changes will be `"2"`. Adding new check names under the same schema_version is non-breaking; consumers must tolerate unknown check names."#.into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "include": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description":
                        "Only run these checks (canonical names). Wins over `skip`."
                },
                "skip": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description":
                        "Skip these checks (canonical names). Ignored when `include` is set."
                }
            },
            "additionalProperties": false,
        }),
        // This is a diagnostic probe; it never mutates driver state.
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for HealthReportTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let include = parse_string_set(args.get("include"));
        let skip = parse_string_set(args.get("skip"));

        let all = self.provider.check_names();
        let to_run = select_checks(all, &include, &skip);

        // Every canonical check appears in the output, even if filtered
        // out — consumers get a complete map of what was considered.
        let mut checks: Vec<CheckEntry> = Vec::with_capacity(all.len());
        for name in all {
            if !to_run.contains(*name) {
                checks.push(CheckEntry::skip(*name, "Skipped by include/skip filter."));
                continue;
            }
            checks.push(self.provider.run_check(name).await);
        }

        let report = Report {
            schema_version: "1".to_owned(),
            platform: self.provider.platform().to_owned(),
            driver_version: env!("CARGO_PKG_VERSION").to_owned(),
            overall: compute_overall(&checks),
            checks,
        };

        let text = text_summary(&report);
        let structured = serde_json::to_value(&report)
            .unwrap_or_else(|_| json!({ "schema_version": "1", "error": "serialize_failed" }));

        // health_report is the consumer's "what's broken?" probe — it
        // must NEVER itself set isError, regardless of how degraded
        // the report is, or consumers can't tell apart "tool broken"
        // from "driver degraded".
        ToolResult::text(text).with_structured(structured)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A platform-agnostic fixture provider used to exercise the
    // dispatcher, filter, and rollup logic without touching any real
    // TCC / UIA / SCK plumbing.
    struct FixtureProvider {
        names: &'static [&'static str],
        platform: &'static str,
        // Per-name override: if Some, return this entry; if None, return
        // a vanilla Pass with the canonical name.
        fail_names: BTreeSet<String>,
    }

    #[async_trait]
    impl HealthCheckProvider for FixtureProvider {
        fn platform(&self) -> &'static str {
            self.platform
        }
        fn check_names(&self) -> &'static [&'static str] {
            self.names
        }
        async fn run_check(&self, name: &str) -> CheckEntry {
            if self.fail_names.contains(name) {
                CheckEntry::fail(name, format!("{name} failed in fixture"), "remediate")
            } else {
                CheckEntry::pass(name, format!("{name} ok"))
            }
        }
    }

    fn macos_names() -> &'static [&'static str] {
        &[
            NAME_BINARY_VERSION,
            NAME_PLATFORM_SUPPORTED,
            NAME_SESSION_ACTIVE,
            NAME_BUNDLE_IDENTITY,
            NAME_TCC_ACCESSIBILITY,
            NAME_TCC_SCREEN_RECORDING,
            NAME_AX_CAPABILITY,
            NAME_SCREEN_CAPTURE_CAPABILITY,
        ]
    }

    fn parse_args(v: Value) -> (BTreeSet<String>, BTreeSet<String>) {
        (
            parse_string_set(v.get("include")),
            parse_string_set(v.get("skip")),
        )
    }

    // ── select_checks ────────────────────────────────────────────────

    #[test]
    fn include_wins_over_skip() {
        let mut include = BTreeSet::new();
        include.insert(NAME_BINARY_VERSION.to_owned());
        let mut skip = BTreeSet::new();
        skip.insert(NAME_BINARY_VERSION.to_owned());
        let chosen = select_checks(macos_names(), &include, &skip);
        assert_eq!(
            chosen.into_iter().collect::<Vec<_>>(),
            vec![NAME_BINARY_VERSION.to_owned()]
        );
    }

    #[test]
    fn include_filters_out_unknowns_silently() {
        let mut include = BTreeSet::new();
        include.insert(NAME_BINARY_VERSION.to_owned());
        include.insert("tomorrows_check_name".to_owned());
        let chosen = select_checks(macos_names(), &include, &BTreeSet::new());
        assert_eq!(
            chosen.into_iter().collect::<Vec<_>>(),
            vec![NAME_BINARY_VERSION.to_owned()]
        );
    }

    #[test]
    fn empty_filters_runs_everything() {
        let chosen = select_checks(macos_names(), &BTreeSet::new(), &BTreeSet::new());
        let expected: BTreeSet<String> = macos_names().iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(chosen, expected);
    }

    #[test]
    fn skip_removes_named() {
        let mut skip = BTreeSet::new();
        skip.insert(NAME_TCC_ACCESSIBILITY.to_owned());
        let chosen = select_checks(macos_names(), &BTreeSet::new(), &skip);
        assert!(!chosen.contains(NAME_TCC_ACCESSIBILITY));
        assert!(chosen.contains(NAME_BINARY_VERSION));
    }

    // ── compute_overall ──────────────────────────────────────────────

    #[test]
    fn all_pass_is_ok() {
        let checks: Vec<_> = macos_names()
            .iter()
            .map(|n| CheckEntry::pass(*n, "ok"))
            .collect();
        assert_eq!(compute_overall(&checks), Overall::Ok);
    }

    #[test]
    fn all_skip_is_ok() {
        let checks: Vec<_> = macos_names()
            .iter()
            .map(|n| CheckEntry::skip(*n, "skipped"))
            .collect();
        assert_eq!(compute_overall(&checks), Overall::Ok);
    }

    #[test]
    fn non_core_fail_is_degraded() {
        // bundle_identity is non-core.
        let checks: Vec<_> = macos_names()
            .iter()
            .map(|n| {
                if *n == NAME_BUNDLE_IDENTITY {
                    CheckEntry::fail(*n, "no bundle", "relaunch in .app")
                } else {
                    CheckEntry::pass(*n, "ok")
                }
            })
            .collect();
        assert_eq!(compute_overall(&checks), Overall::Degraded);
    }

    #[test]
    fn core_fail_is_failed() {
        let checks: Vec<_> = macos_names()
            .iter()
            .map(|n| {
                if *n == NAME_BINARY_VERSION {
                    CheckEntry::fail(*n, "no version", "rebuild")
                } else {
                    CheckEntry::pass(*n, "ok")
                }
            })
            .collect();
        assert_eq!(compute_overall(&checks), Overall::Failed);
    }

    // ── parse_string_set ─────────────────────────────────────────────

    #[test]
    fn parse_string_set_ignores_non_string_items() {
        let v = json!(["a", 7, true, "b"]);
        let parsed = parse_string_set(Some(&v));
        let expected: BTreeSet<String> = ["a".to_owned(), "b".to_owned()].into_iter().collect();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_string_set_handles_nil_and_scalars() {
        assert!(parse_string_set(None).is_empty());
        assert!(parse_string_set(Some(&json!("x"))).is_empty());
        assert!(parse_string_set(Some(&json!(3))).is_empty());
    }

    // ── invoke end-to-end via the fixture provider ───────────────────

    #[tokio::test]
    async fn invoke_no_args_produces_documented_schema() {
        let provider = Arc::new(FixtureProvider {
            names: macos_names(),
            platform: "darwin",
            fail_names: BTreeSet::new(),
        });
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        // Top-level keys must match the documented schema verbatim.
        assert_eq!(structured["schema_version"], "1");
        assert_eq!(structured["platform"], "darwin");
        assert!(structured["driver_version"].is_string());
        assert!(structured["overall"].is_string());
        let checks = structured["checks"].as_array().expect("checks array");
        assert!(!checks.is_empty());
        for entry in checks {
            assert!(entry["name"].is_string());
            assert!(entry["message"].is_string());
            let status = entry["status"].as_str().unwrap();
            assert!(matches!(status, "pass" | "fail" | "skip"));
        }
    }

    #[tokio::test]
    async fn skip_filter_marks_check_as_skip_end_to_end() {
        let provider = Arc::new(FixtureProvider {
            names: macos_names(),
            platform: "darwin",
            fail_names: BTreeSet::new(),
        });
        let tool = HealthReportTool::new(provider);
        let result = tool
            .invoke(json!({ "skip": [NAME_TCC_ACCESSIBILITY] }))
            .await;
        let structured = result.structured_content.expect("structured");
        let checks = structured["checks"].as_array().unwrap();
        let entry = checks
            .iter()
            .find(|c| c["name"] == NAME_TCC_ACCESSIBILITY)
            .expect("tcc_accessibility appears even when skipped");
        assert_eq!(entry["status"], "skip");
    }

    #[tokio::test]
    async fn include_filter_runs_only_named_check() {
        let provider = Arc::new(FixtureProvider {
            names: macos_names(),
            platform: "darwin",
            fail_names: BTreeSet::new(),
        });
        let tool = HealthReportTool::new(provider);
        let result = tool
            .invoke(json!({ "include": [NAME_BINARY_VERSION] }))
            .await;
        let structured = result.structured_content.expect("structured");
        let checks = structured["checks"].as_array().unwrap();
        let ran: Vec<&str> = checks
            .iter()
            .filter(|c| c["status"] != "skip")
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(ran, vec![NAME_BINARY_VERSION]);
    }

    #[tokio::test]
    async fn fail_mode_does_not_set_is_error() {
        // Every check fails — overall should be Failed but the tool
        // result itself must NOT set isError.
        let mut fail = BTreeSet::new();
        for n in macos_names() {
            fail.insert((*n).to_owned());
        }
        let provider = Arc::new(FixtureProvider {
            names: macos_names(),
            platform: "darwin",
            fail_names: fail,
        });
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError even on total failure"
        );
        let structured = result.structured_content.expect("structured");
        assert_eq!(structured["overall"], "failed");
        // Every entry must carry hint + message.
        for entry in structured["checks"].as_array().unwrap() {
            assert_eq!(entry["status"], "fail");
            assert!(entry["hint"].as_str().unwrap_or("").len() > 0);
            assert!(entry["message"].as_str().unwrap_or("").len() > 0);
        }
    }

    // ── tool description contract ────────────────────────────────────

    #[test]
    fn description_commits_to_schema_version_1() {
        // The PR description and downstream consumers bake
        // `schema_version: "1"` into their expectations. A future
        // change that drops this commitment from the schema text
        // without bumping the version would silently break consumers
        // — this test fails loudly the moment that drift starts.
        let description = &def().description;
        assert!(
            description.contains(r#"schema_version="1""#)
                || description.contains(r#"schema_version: "1""#),
            "schema_version=1 must be documented in the tool description"
        );
    }

    // Belt-and-suspenders use of `parse_args` — keeps the helper
    // exercised in case future tests reach for it.
    #[test]
    fn parse_args_compiles() {
        let _ = parse_args(json!({ "include": ["x"], "skip": ["y"] }));
    }
}
