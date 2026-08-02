use crate::{
    Firmware, OidcConfig, PreservedJson, RuntimeKind, SandboxService, WarmPoolAutoscaling,
    common::{
        default_cpu_cores, default_firmware, default_memory, default_runtime, firmware_schema,
        integer_schema, node_selector_schema, oidc_schema, preserved_json_schema, runtime_schema,
        string_list_schema, string_schema, tolerations_schema,
    },
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};

pub type WorkspacePoolAutoscaling = WarmPoolAutoscaling;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PoolTemplate {
    #[schemars(
        default = "default_runtime",
        schema_with = "runtime_schema",
        description = "Pool backend. \"kubevirt\" (default) provisions a KubeVirt\nVM. \"macos\" provisions a macOS sandbox (ADMIN-ONLY\n\u{2014} the cyclops-cs backend rejects macos pool writes from\nnon-admins; the SPA hides the option). Flows verbatim\nthrough the compat shim into the OSGymSandboxTemplate\nvmTemplate, where macos_backend dispatches on it.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeKind>,
    #[schemars(
        default,
        schema_with = "string_schema",
        description = "runtime=macos only. Sandbox pod RuntimeClass (default cua-macos-native)."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_class_name: Option<String>,
    #[schemars(
        default,
        schema_with = "node_selector_schema",
        description = "runtime=macos only. nodeSelector to pin onto macOS nodes."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<HashMap<String, String>>,
    #[schemars(
        default,
        schema_with = "tolerations_schema",
        description = "runtime=macos only. Tolerations for tainted macOS nodes."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerations: Option<Vec<Arc<PreservedJson>>>,
    #[schemars(
        default,
        schema_with = "string_list_schema",
        description = "runtime=macos only. Sandbox container entrypoint."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    pub container_disk_image: String,
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_pull_secret: Option<String>,
    #[schemars(
        default = "default_cpu_cores",
        schema_with = "integer_schema",
        range(min = 1)
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<u32>,
    #[schemars(default = "default_memory", schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[schemars(
        default = "default_firmware",
        schema_with = "firmware_schema",
        description = "VM firmware. Use \"efi\" for GPT/UEFI-only guest images\n(e.g. the dockur-built Windows desktop-workspace);\n\"bios\" is KubeVirt's default and what the Linux\nworkspace images boot with.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<Firmware>,
    #[schemars(
        default,
        schema_with = "preserved_json_schema",
        description = "Optional KubeVirt readinessProbe/livenessProbe for the\nVMI, gating ready on the guest actually serving. (Was\npreviously sent by the dashboard but pruned by this\nschema \u{2014} adding it makes the field actually persist.)\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probes: Option<Arc<PreservedJson>>,
    #[schemars(
        default,
        schema_with = "oidc_schema",
        description = "Optional Keycloak workload-OIDC token injection,\npropagated verbatim by the compat shim onto the generated\nOSGymSandboxTemplate.spec.vmTemplate.oidc. When set, the\npool-operator renders a cloud-init disk that drops the\ntenant-scoped Keycloak client credentials into the guest\nand runs an in-guest refresher that mints + rotates an\nOIDC access token at /var/run/cua/oidc/token. See\ndocs/decisions/2026-06-25-osgym-pool-workload-oidc.md.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oidc: Option<OidcConfig>,
}

#[allow(clippy::duplicated_attributes)]
#[uniffi::export(Eq, Hash)]
#[derive(
    CustomResource, Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record,
)]
#[kube(
    group = "cua.ai",
    version = "v1",
    kind = "OSGymWorkspacePool",
    plural = "osgymworkspacepools",
    namespaced,
    shortname = "owsp",
    status = "OSGymWorkspacePoolStatus",
    printcolumn(name = "Replicas", type_ = "integer", json_path = ".spec.replicas"),
    printcolumn(
        name = "Available",
        type_ = "integer",
        json_path = ".status.availableCount"
    ),
    printcolumn(name = "Phase", type_ = "string", json_path = ".status.phase"),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    ),
    doc = ""
)]
#[serde(rename_all = "camelCase")]
pub struct PoolSpec {
    #[schemars(schema_with = "integer_schema", range(min = 0))]
    pub replicas: u32,
    pub template: PoolTemplate,
    #[schemars(
        description = "OPTIONAL autoscaling extension. Propagated verbatim by\nthe compat shim onto the generated\nOSGymSandboxWarmPool.spec.autoscaling, which opts the\npool into KEDA autoscaling on claim demand. Absent = a\nplain pool. See\ndocs/decisions/2026-06-15-pool-claim-autoscaling.md.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoscaling: Option<WorkspacePoolAutoscaling>,
    #[schemars(description = "Extra K8s Services created per-sandbox.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<SandboxService>>,
}

impl Hash for PoolSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let value =
            serde_json::to_value(self).expect("PoolSpec serialization should be infallible");
        hash_json_value(&value, state);
    }
}

fn hash_json_value<H: Hasher>(value: &serde_json::Value, state: &mut H) {
    match value {
        serde_json::Value::Null => 0_u8.hash(state),
        serde_json::Value::Bool(value) => {
            1_u8.hash(state);
            value.hash(state);
        }
        serde_json::Value::Number(value) => {
            2_u8.hash(state);
            value.hash(state);
        }
        serde_json::Value::String(value) => {
            3_u8.hash(state);
            value.hash(state);
        }
        serde_json::Value::Array(values) => {
            4_u8.hash(state);
            values.len().hash(state);
            for value in values {
                hash_json_value(value, state);
            }
        }
        serde_json::Value::Object(values) => {
            5_u8.hash(state);
            values.len().hash(state);
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            for key in keys {
                key.hash(state);
                hash_json_value(&values[key], state);
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymWorkspacePoolStatus {
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[schemars(default, schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u32>,
    #[schemars(default, schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_count: Option<u32>,
    #[schemars(default, schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{HashMap, hash_map::DefaultHasher},
        hash::{Hash, Hasher},
    };

    #[test]
    fn canonical_json_hash_matches_equal_zero_representations() {
        let positive = serde_json::json!(0.0);
        let negative = serde_json::json!(-0.0);
        assert_eq!(positive, negative);

        let mut positive_hasher = DefaultHasher::new();
        hash_json_value(&positive, &mut positive_hasher);
        let mut negative_hasher = DefaultHasher::new();
        hash_json_value(&negative, &mut negative_hasher);
        assert_eq!(positive_hasher.finish(), negative_hasher.finish());
    }

    #[test]
    fn pool_spec_hash_is_stable_across_node_selector_insertion_order() {
        let mut left: PoolSpec = serde_json::from_value(serde_json::json!({
            "replicas": 1,
            "template": { "containerDiskImage": "example.invalid/workspace:latest" }
        }))
        .expect("pool spec fixture should deserialize");
        let mut right = left.clone();

        left.template.node_selector = Some(HashMap::from([
            ("kubernetes.io/arch".to_string(), "amd64".to_string()),
            (
                "topology.kubernetes.io/zone".to_string(),
                "us-east-1a".to_string(),
            ),
        ]));
        right.template.node_selector = Some(HashMap::from([
            (
                "topology.kubernetes.io/zone".to_string(),
                "us-east-1a".to_string(),
            ),
            ("kubernetes.io/arch".to_string(), "amd64".to_string()),
        ]));

        assert_eq!(left, right);

        let mut left_hasher = DefaultHasher::new();
        left.hash(&mut left_hasher);
        let mut right_hasher = DefaultHasher::new();
        right.hash(&mut right_hasher);
        assert_eq!(left_hasher.finish(), right_hasher.finish());
    }
}
