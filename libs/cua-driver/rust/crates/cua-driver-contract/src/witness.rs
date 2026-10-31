// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Metadata-only runtime conformance witness for cua-driver.
//!
//! This contract binds one concrete driver build/platform to read-only runtime
//! probes without retaining desktop content. It complements the static SDK
//! manifest: advertisement says what a build claims; a witness records what
//! was actually exercised on one runtime.

use crate::{Platform, CAPABILITY_VERSION, CONTRACT_VERSION, TOOLS_LIST_SCHEMA_VERSION};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const RUNTIME_WITNESS_SCHEMA_VERSION: &str = "0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WitnessRuntimeHost {
    Embedded,
    Daemon,
    Direct,
    Remote,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WitnessHealthOverall {
    Ok,
    Degraded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Pass,
    Fail,
    Skip,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProbeWitness {
    /// Public tool name exactly as advertised by cua-driver.
    pub tool: String,
    /// Capability tokens claimed by that tool at probe time.
    pub capabilities: Vec<String>,
    /// v0 is deliberately read-only. Acting probes require a future explicit
    /// opt-in contract rather than silently expanding this artifact.
    pub read_only: bool,
    /// Runtime outcome; timeout is distinct from ordinary tool failure.
    pub outcome: ProbeOutcome,
    /// Whether successful structuredContent passed the existing shared output
    /// validator. None for skips, timeouts, and error/refusal outcomes.
    pub output_schema_valid: Option<bool>,
    /// End-to-end elapsed time measured by the witness runner.
    pub elapsed_ms: u64,
    /// Stable machine-readable diagnostic class only. Do not place screenshots,
    /// window titles, application names, accessibility text, or tool payloads
    /// here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RuntimeWitnessV0 {
    pub schema_version: String,
    pub contract_version: String,
    pub tools_list_schema_version: String,
    pub capability_version: String,
    pub driver_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_git_sha: Option<String>,
    pub platform: Platform,
    pub runtime_host: WitnessRuntimeHost,
    /// True only when the runtime tools/list advertisement was observed for
    /// this exact build/host before probes were run.
    pub tools_list_observed: bool,
    pub health_overall: WitnessHealthOverall,
    /// True only when health_report itself was actually called for this
    /// witness. This prevents a producer from treating "not checked" as "ok".
    pub health_report_observed: bool,
    /// Read-only runtime probes. The artifact intentionally stores no desktop
    /// content or raw tool result payload.
    pub probes: Vec<ProbeWitness>,
}

impl RuntimeWitnessV0 {
    pub fn new(
        driver_version: impl Into<String>,
        driver_git_sha: Option<String>,
        platform: Platform,
        runtime_host: WitnessRuntimeHost,
        tools_list_observed: bool,
        health_overall: WitnessHealthOverall,
        health_report_observed: bool,
        probes: Vec<ProbeWitness>,
    ) -> Self {
        Self {
            schema_version: RUNTIME_WITNESS_SCHEMA_VERSION.into(),
            contract_version: CONTRACT_VERSION.into(),
            tools_list_schema_version: TOOLS_LIST_SCHEMA_VERSION.into(),
            capability_version: CAPABILITY_VERSION.into(),
            driver_version: driver_version.into(),
            driver_git_sha,
            platform,
            runtime_host,
            tools_list_observed,
            health_overall,
            health_report_observed,
            probes,
        }
    }

    /// Contract-level checks independent of any particular platform backend.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RUNTIME_WITNESS_SCHEMA_VERSION {
            return Err(format!(
                "unsupported runtime witness schema_version {}",
                self.schema_version
            ));
        }
        if self.contract_version != CONTRACT_VERSION {
            return Err(format!(
                "runtime witness contract_version {} does not match {}",
                self.contract_version, CONTRACT_VERSION
            ));
        }
        if self.tools_list_schema_version != TOOLS_LIST_SCHEMA_VERSION {
            return Err(format!(
                "runtime witness tools_list_schema_version {} does not match {}",
                self.tools_list_schema_version, TOOLS_LIST_SCHEMA_VERSION
            ));
        }
        if self.capability_version != CAPABILITY_VERSION {
            return Err(format!(
                "runtime witness capability_version {} does not match {}",
                self.capability_version, CAPABILITY_VERSION
            ));
        }
        if self.driver_version.trim().is_empty() {
            return Err("runtime witness driver_version must be non-empty".into());
        }
        if !self.tools_list_observed && !self.probes.is_empty() {
            return Err(
                "runtime witness cannot contain probes without observing tools/list".into(),
            );
        }
        if !self.health_report_observed && self.health_overall != WitnessHealthOverall::Unknown {
            return Err(
                "health_overall must be unknown when health_report was not observed".into(),
            );
        }
        let mut seen_tools = BTreeSet::new();
        let mut previous_tool: Option<&str> = None;
        for probe in &self.probes {
            if probe.tool.trim().is_empty() {
                return Err("runtime witness probe tool must be non-empty".into());
            }
            if !seen_tools.insert(probe.tool.as_str()) {
                return Err(format!("runtime witness contains duplicate probe {}", probe.tool));
            }
            if let Some(previous) = previous_tool {
                if previous > probe.tool.as_str() {
                    return Err("runtime witness probes must be sorted by tool name".into());
                }
            }
            previous_tool = Some(probe.tool.as_str());
            let mut sorted_capabilities = probe.capabilities.clone();
            sorted_capabilities.sort();
            sorted_capabilities.dedup();
            if sorted_capabilities != probe.capabilities {
                return Err(format!(
                    "runtime witness probe {} capabilities must be sorted and unique",
                    probe.tool
                ));
            }
            if !probe.read_only {
                return Err(format!(
                    "runtime witness v0 refuses acting probe {}",
                    probe.tool
                ));
            }
            match probe.outcome {
                ProbeOutcome::Pass => {
                    if probe.output_schema_valid != Some(true) {
                        return Err(format!(
                            "passing probe {} must have output_schema_valid=true",
                            probe.tool
                        ));
                    }
                }
                ProbeOutcome::Skip | ProbeOutcome::Timeout => {
                    if probe.output_schema_valid.is_some() {
                        return Err(format!(
                            "{:?} probe {} must not claim output schema validation",
                            probe.outcome, probe.tool
                        ));
                    }
                }
                ProbeOutcome::Fail => {}
            }
            if let Some(code) = &probe.diagnostic_code {
                if code.is_empty()
                    || code.len() > 128
                    || !code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
                {
                    return Err(format!(
                        "probe {} has invalid diagnostic_code",
                        probe.tool
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn runtime_witness_schema() -> serde_json::Value {
    let generator = crate::schema_settings().into_generator();
    serde_json::to_value(generator.into_root_schema_for::<RuntimeWitnessV0>())
        .expect("runtime witness schema must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_probe() -> ProbeWitness {
        ProbeWitness {
            tool: "get_screen_size".into(),
            capabilities: vec!["screen.size.read".into()],
            read_only: true,
            outcome: ProbeOutcome::Pass,
            output_schema_valid: Some(true),
            elapsed_ms: 12,
            diagnostic_code: None,
        }
    }

    #[test]
    fn metadata_only_read_only_witness_validates() {
        let witness = RuntimeWitnessV0::new(
            "0.14.0",
            Some("0123456789abcdef".into()),
            Platform::Macos,
            WitnessRuntimeHost::Daemon,
            true,
            WitnessHealthOverall::Ok,
            true,
            vec![passing_probe()],
        );
        assert_eq!(witness.validate(), Ok(()));
        let json = serde_json::to_value(&witness).expect("serialize witness");
        assert!(json.get("probes").is_some());
        assert!(json.to_string().find("screenshot").is_none());
        assert!(json.to_string().find("window_title").is_none());
    }

    #[test]
    fn unobserved_health_cannot_be_reported_ok() {
        let witness = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Linux,
            WitnessRuntimeHost::Direct,
            true,
            WitnessHealthOverall::Ok,
            false,
            vec![],
        );
        assert!(witness
            .validate()
            .unwrap_err()
            .contains("health_overall must be unknown"));
    }

    #[test]
    fn v0_refuses_acting_probes() {
        let mut probe = passing_probe();
        probe.read_only = false;
        probe.tool = "click".into();
        let witness = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Windows,
            WitnessRuntimeHost::Daemon,
            true,
            WitnessHealthOverall::Unknown,
            false,
            vec![probe],
        );
        assert!(witness.validate().unwrap_err().contains("refuses acting probe"));
    }

    #[test]
    fn timeout_is_not_schema_validated() {
        let mut probe = passing_probe();
        probe.outcome = ProbeOutcome::Timeout;
        probe.output_schema_valid = Some(true);
        let witness = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Macos,
            WitnessRuntimeHost::Daemon,
            true,
            WitnessHealthOverall::Unknown,
            false,
            vec![probe],
        );
        assert!(witness
            .validate()
            .unwrap_err()
            .contains("must not claim output schema validation"));
    }

    #[test]
    fn schema_is_closed_enough_to_name_core_versions() {
        let schema = runtime_witness_schema();
        let rendered = schema.to_string();
        for field in [
            "schema_version",
            "contract_version",
            "capability_version",
            "tools_list_schema_version",
            "tools_list_observed",
            "health_report_observed",
            "probes",
        ] {
            assert!(rendered.contains(field), "schema missing {field}");
        }
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;

    #[test]
    fn probes_require_observed_advertisement() {
        let witness = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Linux,
            WitnessRuntimeHost::Direct,
            false,
            WitnessHealthOverall::Unknown,
            false,
            vec![ProbeWitness {
                tool: "get_screen_size".into(),
                capabilities: vec!["screen.size.read".into()],
                read_only: true,
                outcome: ProbeOutcome::Pass,
                output_schema_valid: Some(true),
                elapsed_ms: 1,
                diagnostic_code: None,
            }],
        );
        assert!(witness
            .validate()
            .unwrap_err()
            .contains("without observing tools/list"));
    }

    #[test]
    fn probes_are_unique_sorted_and_capabilities_are_canonical() {
        let duplicate = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Linux,
            WitnessRuntimeHost::Direct,
            true,
            WitnessHealthOverall::Unknown,
            false,
            vec![
                ProbeWitness {
                    tool: "get_screen_size".into(),
                    capabilities: vec!["screen.size.read".into()],
                    read_only: true,
                    outcome: ProbeOutcome::Pass,
                    output_schema_valid: Some(true),
                    elapsed_ms: 1,
                    diagnostic_code: None,
                },
                ProbeWitness {
                    tool: "get_screen_size".into(),
                    capabilities: vec!["screen.size.read".into()],
                    read_only: true,
                    outcome: ProbeOutcome::Pass,
                    output_schema_valid: Some(true),
                    elapsed_ms: 1,
                    diagnostic_code: None,
                },
            ],
        );
        assert!(duplicate.validate().unwrap_err().contains("duplicate probe"));

        let unsorted_capabilities = RuntimeWitnessV0::new(
            "0.14.0",
            None,
            Platform::Linux,
            WitnessRuntimeHost::Direct,
            true,
            WitnessHealthOverall::Unknown,
            false,
            vec![ProbeWitness {
                tool: "get_screen_size".into(),
                capabilities: vec!["z".into(), "a".into()],
                read_only: true,
                outcome: ProbeOutcome::Pass,
                output_schema_valid: Some(true),
                elapsed_ms: 1,
                diagnostic_code: None,
            }],
        );
        assert!(unsorted_capabilities
            .validate()
            .unwrap_err()
            .contains("sorted and unique"));
    }
}
