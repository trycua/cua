use crate::{
    SandboxTemplateRef,
    common::{integer_schema, string_schema},
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_zero() -> Option<u32> {
    Some(0)
}

fn default_max_pool_size() -> Option<u32> {
    Some(50)
}

fn sandbox_template_ref_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": {
                "type": "string",
                "description": "Name of the OSGymSandboxTemplate to use."
            }
        }
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct WarmPoolAutoscaling {
    #[schemars(
        default = "default_zero",
        schema_with = "integer_schema",
        range(min = 0),
        description = "ScaledObject minReplicaCount — the durable warm\nfloor kept while the extension is enabled. 0 lets\nthe pool scale to zero when there are no claims.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_pool_size: Option<u32>,
    #[schemars(
        default = "default_zero",
        schema_with = "integer_schema",
        range(min = 0),
        description = "One-time warm head-start: the pool-operator seeds\nspec.replicas to this when the pool is created so\nthe first claims bind without a cold start. KEDA\nthen owns spec.replicas; the value decays toward\nthe live claim demand (floored at minPoolSize)\nonce KEDA begins polling.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_pool_size: Option<u32>,
    #[schemars(
        default = "default_max_pool_size",
        schema_with = "integer_schema",
        range(min = 1),
        description = "ScaledObject maxReplicaCount — hard ceiling."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_pool_size: Option<u32>,
}

#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[kube(
    group = "osgym.cua.ai",
    version = "v1alpha1",
    kind = "OSGymSandboxWarmPool",
    plural = "osgymsandboxwarmpools",
    namespaced,
    shortname = "oswp",
    status = "OSGymSandboxWarmPoolStatus",
    scale(
        spec_replicas_path = ".spec.replicas",
        status_replicas_path = ".status.replicas",
        label_selector_path = ".status.selector"
    ),
    printcolumn(name = "Replicas", type_ = "integer", json_path = ".spec.replicas"),
    printcolumn(name = "Ready", type_ = "integer", json_path = ".status.readyReplicas"),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    ),
    doc = ""
)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxWarmPoolSpec {
    #[schemars(
        schema_with = "integer_schema",
        range(min = 0),
        description = "Desired number of pre-warmed OSGymSandboxes."
    )]
    pub replicas: u32,
    #[schemars(schema_with = "sandbox_template_ref_schema")]
    pub sandbox_template_ref: SandboxTemplateRef,
    #[schemars(
        description = "OPTIONAL autoscaling extension. When present, the\npool-operator manages a KEDA ScaledObject that drives\nspec.replicas (via the /scale subresource) from the\npool's claim demand — the count of live\nOSGymSandboxClaims (Pending + Bound), published as\nosgym_pool_claim_demand. A Pending claim is unmet\ndemand so the pool grows; deleting claims shrinks it\n(drain-safe). Absent = a plain warm pool whose\nspec.replicas is set statically, with no autoscaling.\nSee docs/decisions/2026-06-15-pool-claim-autoscaling.md.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoscaling: Option<WarmPoolAutoscaling>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxWarmPoolStatus {
    #[schemars(default, schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<u32>,
    #[schemars(default, schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_replicas: Option<u32>,
    #[schemars(
        default,
        schema_with = "string_schema",
        description = "Label selector for the pool's OSGymSandboxes."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}
