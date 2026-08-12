use crate::{
    SandboxTemplateRef,
    common::{bool_schema, date_time_schema, integer_schema, string_schema},
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_warmpool() -> Option<String> {
    Some("default".into())
}

fn default_shutdown_policy() -> Option<String> {
    Some("Retain".into())
}

fn default_auto_renew() -> Option<bool> {
    Some(false)
}

fn sandbox_template_ref_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": {
                "type": "string",
                "description": "OSGymSandboxTemplate the warm pool's OSGymSandboxes\nuse.\n"
            }
        }
    })
}

fn claim_lifecycle_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    ClaimLifecycle::json_schema(generator)
}

fn claim_conditions_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Vec::<OSGymSandboxClaimCondition>::json_schema(generator)
}

fn claim_sandbox_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    OSGymSandboxClaimSandbox::json_schema(generator)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ClaimLifecycle {
    #[schemars(
        default,
        schema_with = "date_time_schema",
        description = "Absolute UTC expiry. The claim reaper deletes the\nclaim (GC-cascading the bound OSGymSandbox) once\nnow > shutdownTime. /step PATCHes this forward.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_time: Option<String>,
    #[schemars(
        default = "default_shutdown_policy",
        schema_with = "string_schema",
        description = "Retain (default) — on claim deletion the bound\nOSGymSandbox is RETURNED to the warm pool and\nrestarted in place (VMI deleted -> KubeVirt\nrecreates it from spec.running:true with a fresh\ncontainerDisk overlay = clean desktop). Delete —\nthe escape hatch: destroy the OSGymSandbox CR\n(its finalizer tears down VM+Service,\nreconcile_pool respawns). The orchestrator never\nsets this field, so it picks up the new Retain\ndefault.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_policy: Option<String>,
    #[schemars(
        default = "default_auto_renew",
        schema_with = "bool_schema",
        description = "When true, the lane auto-renew timer in\npool-operator extends shutdownTime forward on\nevery renew tick. Set on lane-backed claims\n(managed by /api/runs/{name}/lanes) so a lane's\nVM is not reaped while the customer is between\n/step calls; left false for batch claims, which\nrenew through /step as before. The orchestrator\nnever touches this -- only the cyclops-cs runs\nmanager sets it on lane create.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_renew: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxClaimCondition {
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[schemars(default, schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxClaimSandbox {
    #[schemars(
        default,
        schema_with = "string_schema",
        description = "Adopted OSGymSandbox name (== vmName)."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[schemars(
        default,
        schema_with = "string_schema",
        description = "In-cluster DNS of the bound OSGymSandbox's Service.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxClaimStatus {
    #[schemars(
        default,
        schema_with = "string_schema",
        description = "Pending | Bound | Failed — a derived field written by\nthe claim controller alongside conditions, kept for\npool.py byte-compatibility.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[schemars(
        default,
        schema_with = "claim_conditions_schema",
        description = "Standard metav1.Condition list."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<OSGymSandboxClaimCondition>>,
    #[schemars(default, schema_with = "claim_sandbox_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<OSGymSandboxClaimSandbox>,
}

#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[kube(
    group = "osgym.cua.ai",
    version = "v1alpha1",
    kind = "OSGymSandboxClaim",
    plural = "osgymsandboxclaims",
    namespaced,
    shortname = "osbc",
    status = "OSGymSandboxClaimStatus",
    printcolumn(
        name = "Template",
        type_ = "string",
        json_path = ".spec.sandboxTemplateRef.name"
    ),
    printcolumn(name = "Phase", type_ = "string", json_path = ".status.phase"),
    printcolumn(name = "Sandbox", type_ = "string", json_path = ".status.sandbox.name"),
    printcolumn(
        name = "ShutdownTime",
        type_ = "string",
        json_path = ".spec.lifecycle.shutdownTime"
    ),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    ),
    doc = ""
)]
#[serde(rename_all = "camelCase")]
pub struct ClaimSpec {
    #[schemars(schema_with = "sandbox_template_ref_schema")]
    pub sandbox_template_ref: SandboxTemplateRef,
    #[schemars(
        default = "default_warmpool",
        schema_with = "string_schema",
        description = "\"none\" | \"default\" | <name> — which OSGymSandboxWarmPool\nto adopt an OSGymSandbox from.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmpool: Option<String>,
    #[schemars(
        default,
        schema_with = "integer_schema",
        range(min = 0),
        description = "Seconds the claim stays Pending waiting for a Sandbox to\nbind before it is marked Failed (default 300, set by the\noperator's OSGYM_CLAIM_BIND_DEADLINE). A Pending claim is\nthe autoscaler's demand signal, so the claim is kept\nPending across a cold VM boot + KEDA scale-up rather than\nfailing fast. Raise it for pools with slow cold starts.\n"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_deadline: Option<u32>,
    #[schemars(default, schema_with = "claim_lifecycle_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<ClaimLifecycle>,
}
