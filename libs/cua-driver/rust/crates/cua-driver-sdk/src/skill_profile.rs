//! Versioned, digest-bound agent guidance exposed through the SDK.
//!
//! Managed providers read these embedded resources from the accepted driver
//! generation. They never need arbitrary filesystem or shell access, and a
//! caller can pin the digest advertised by [`driver_skill_profile`] when it
//! reads an individual resource.

use crate::DriverError;
use sha2::{Digest, Sha256};

const PROFILE_ID: &str = "openclaw-mcp";
const PROFILE_SCHEMA_VERSION: &str = "1";
const TOOL_SCHEMA_PROFILE: &str = "cua-driver-provider-native-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmbeddedResource {
    name: &'static str,
    content: &'static str,
    platform: Option<&'static str>,
    capability: Option<&'static str>,
}

const RESOURCES: &[EmbeddedResource] = &[
    EmbeddedResource {
        name: "OPENCLAW.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW.md"),
        platform: None,
        capability: None,
    },
    EmbeddedResource {
        name: "OPENCLAW_MACOS.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW_MACOS.md"),
        platform: Some("macos"),
        capability: None,
    },
    EmbeddedResource {
        name: "OPENCLAW_WINDOWS.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW_WINDOWS.md"),
        platform: Some("windows"),
        capability: None,
    },
    EmbeddedResource {
        name: "OPENCLAW_LINUX.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW_LINUX.md"),
        platform: Some("linux"),
        capability: None,
    },
    EmbeddedResource {
        name: "OPENCLAW_BROWSER.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW_BROWSER.md"),
        platform: None,
        capability: Some("browser"),
    },
    EmbeddedResource {
        name: "OPENCLAW_RECORDING.md",
        content: include_str!("../../../Skills/cua-driver/OPENCLAW_RECORDING.md"),
        platform: None,
        capability: Some("recording"),
    },
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, uniffi::Record)]
pub struct DriverSkillResourceDescriptor {
    pub name: String,
    pub sha256: String,
    pub platform: Option<String>,
    pub capability: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, uniffi::Record)]
pub struct DriverSkillProfile {
    pub profile_id: String,
    pub schema_version: String,
    pub driver_version: String,
    pub compatible_driver_requirement: String,
    pub tool_schema_profile: String,
    pub bundle_sha256: String,
    pub resources: Vec<DriverSkillResourceDescriptor>,
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DriverSkillResource {
    pub profile_id: String,
    pub name: String,
    pub sha256: String,
    pub content: String,
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unsupported"
    }
}

fn digest(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn selected_resources() -> Vec<&'static EmbeddedResource> {
    RESOURCES
        .iter()
        .filter(|resource| {
            resource
                .platform
                .is_none_or(|platform| platform == current_platform())
        })
        .collect()
}

fn descriptors() -> Vec<DriverSkillResourceDescriptor> {
    selected_resources()
        .into_iter()
        .map(|resource| DriverSkillResourceDescriptor {
            name: resource.name.to_owned(),
            sha256: digest(resource.content),
            platform: resource.platform.map(str::to_owned),
            capability: resource.capability.map(str::to_owned),
        })
        .collect()
}

fn bundle_digest(resources: &[DriverSkillResourceDescriptor]) -> String {
    let mut hasher = Sha256::new();
    for resource in resources {
        hasher.update(resource.name.as_bytes());
        hasher.update([0]);
        hasher.update(resource.sha256.as_bytes());
        hasher.update([b'\n']);
    }
    format!("{:x}", hasher.finalize())
}

#[uniffi::export]
pub fn driver_skill_profile(profile_id: String) -> Result<DriverSkillProfile, DriverError> {
    if profile_id != PROFILE_ID {
        return Err(DriverError::Configuration {
            reason: format!("unsupported driver skill profile: {profile_id}"),
        });
    }
    let resources = descriptors();
    Ok(DriverSkillProfile {
        profile_id,
        schema_version: PROFILE_SCHEMA_VERSION.to_owned(),
        driver_version: env!("CARGO_PKG_VERSION").to_owned(),
        compatible_driver_requirement: format!("={}", env!("CARGO_PKG_VERSION")),
        tool_schema_profile: TOOL_SCHEMA_PROFILE.to_owned(),
        bundle_sha256: bundle_digest(&resources),
        resources,
        required_capabilities: vec![
            "embedded_runtime_v1".to_owned(),
            "private_worker_v1".to_owned(),
            "trusted_session_authorization_v1".to_owned(),
            "provider_native_tools_v1".to_owned(),
            "snapshot_action_verify_v1".to_owned(),
            "browser_exact_route_v1".to_owned(),
            "recording_v1".to_owned(),
            "trusted_host_resources_v1".to_owned(),
            "trusted_native_helpers_v1".to_owned(),
            "managed_file_output_suppression_v1".to_owned(),
            "trusted_existing_profile_authorization_v1".to_owned(),
        ],
    })
}

#[uniffi::export]
pub fn read_driver_skill_resource(
    profile_id: String,
    name: String,
    expected_sha256: String,
) -> Result<DriverSkillResource, DriverError> {
    let profile = driver_skill_profile(profile_id.clone())?;
    let descriptor = profile
        .resources
        .iter()
        .find(|resource| resource.name == name)
        .ok_or_else(|| DriverError::Configuration {
            reason: format!("resource {name} is not part of the selected {profile_id} bundle"),
        })?;
    if expected_sha256 != descriptor.sha256 {
        return Err(DriverError::Configuration {
            reason: format!("resource digest mismatch for {name}"),
        });
    }
    let resource = selected_resources()
        .into_iter()
        .find(|resource| resource.name == name)
        .expect("profile descriptor and embedded resource must stay in sync");
    Ok(DriverSkillResource {
        profile_id,
        name,
        sha256: descriptor.sha256.clone(),
        content: resource.content.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_exact_versioned_and_digest_bound() {
        let profile = driver_skill_profile(PROFILE_ID.to_owned()).unwrap();
        assert_eq!(profile.schema_version, "1");
        assert_eq!(
            profile.compatible_driver_requirement,
            format!("={}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(profile.bundle_sha256.len(), 64);
        for required in [
            "embedded_runtime_v1",
            "private_worker_v1",
            "trusted_session_authorization_v1",
            "browser_exact_route_v1",
            "recording_v1",
            "trusted_host_resources_v1",
            "trusted_native_helpers_v1",
            "managed_file_output_suppression_v1",
            "trusted_existing_profile_authorization_v1",
        ] {
            assert!(
                profile
                    .required_capabilities
                    .iter()
                    .any(|capability| capability == required),
                "profile omitted required capability {required}"
            );
        }
        assert!(profile
            .resources
            .iter()
            .any(|resource| resource.name == "OPENCLAW.md"));
        assert_eq!(
            profile
                .resources
                .iter()
                .filter(|resource| resource.platform.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn resource_reader_rejects_unknown_or_mismatched_content() {
        let profile = driver_skill_profile(PROFILE_ID.to_owned()).unwrap();
        let core = profile
            .resources
            .iter()
            .find(|resource| resource.name == "OPENCLAW.md")
            .unwrap();
        let read = read_driver_skill_resource(
            PROFILE_ID.to_owned(),
            core.name.clone(),
            core.sha256.clone(),
        )
        .unwrap();
        assert_eq!(read.sha256, core.sha256);
        assert!(read.content.contains("Snapshot, action, verification"));
        assert!(read_driver_skill_resource(
            PROFILE_ID.to_owned(),
            core.name.clone(),
            "0".repeat(64),
        )
        .is_err());
        assert!(read_driver_skill_resource(
            PROFILE_ID.to_owned(),
            "SKILL.md".to_owned(),
            "0".repeat(64),
        )
        .is_err());
    }

    #[test]
    fn managed_profile_never_bootstraps_or_selects_transport() {
        for resource in selected_resources() {
            for forbidden in [
                "cua-driver mcp",
                "cua-driver serve",
                "cua-driver stop",
                "cua-driver update",
                "--socket",
                "curl ",
                "install.sh",
            ] {
                assert!(
                    !resource.content.contains(forbidden),
                    "{} contains forbidden managed-provider instruction {forbidden}",
                    resource.name
                );
            }
        }
    }

    #[tokio::test]
    async fn documented_openclaw_routes_exist_in_the_live_tool_inventory() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = crate::CuaDriver::create(None).unwrap();
        let inventory: serde_json::Value =
            serde_json::from_str(&driver.list_tools_json().await.unwrap()).unwrap();
        let tools = inventory["tools"].as_array().unwrap();
        let names = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<std::collections::HashSet<_>>();
        driver.shutdown().await.unwrap();
        drop(_runtime_test);

        for required in [
            "start_session",
            "end_session",
            "get_window_state",
            "get_desktop_state",
            "click",
            "bring_to_front",
            "escalate_session",
            "get_browser_state",
            "browser_click",
            "browser_type",
            "browser_set_input_files",
            "browser_download",
            "start_recording",
            "stop_recording",
            "replay_trajectory",
        ] {
            assert!(
                names.contains(required),
                "OpenClaw profile documents missing live tool {required}"
            );
        }

        let schemas = tools
            .iter()
            .filter_map(|tool| {
                Some((
                    tool["name"].as_str()?,
                    tool["inputSchema"]["properties"].as_object()?,
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut managed_native_arguments = vec![
            ("start_recording", "output_dir"),
            ("browser_download", "destination_root"),
            ("browser_set_input_files", "files"),
            ("replay_trajectory", "dir"),
            ("get_desktop_state", "screenshot_out_file"),
            ("get_window_state", "screenshot_out_file"),
        ];
        #[cfg(target_os = "macos")]
        managed_native_arguments.push(("click", "debug_image_out"));
        let native_argument_names = managed_native_arguments
            .iter()
            .map(|(_, argument)| *argument)
            .collect::<std::collections::HashSet<_>>();
        for (tool, properties) in &schemas {
            for argument in properties.keys().map(String::as_str) {
                if !native_argument_names.contains(argument) {
                    continue;
                }
                assert!(
                    managed_native_arguments.contains(&(*tool, argument)),
                    "live native argument {tool}.{argument} is not covered by the managed resource policy audit"
                );
            }
        }
        for (tool, argument) in managed_native_arguments {
            assert!(
                schemas
                    .get(tool)
                    .is_some_and(|properties| properties.contains_key(argument)),
                "managed native argument policy is stale: {tool}.{argument} is absent from the live schema"
            );
        }
        for refused_tool in ["launch_app", "install_ffmpeg"] {
            assert!(
                schemas.contains_key(refused_tool),
                "managed resource policy expects to refuse missing live tool {refused_tool}"
            );
        }
        for argument in ["name", "urls", "additional_arguments"] {
            assert!(
                schemas
                    .get("launch_app")
                    .is_some_and(|properties| properties.contains_key(argument)),
                "managed launch refusal audit is stale: launch_app.{argument} is absent from the live schema"
            );
        }
    }
}
