use crate::{
    VmTemplate,
    common::{bool_schema, date_time_schema, string_schema},
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[kube(
    group = "osgym.cua.ai",
    version = "v1alpha1",
    kind = "OSGymSandbox",
    plural = "osgymsandboxes",
    namespaced,
    shortname = "osbx",
    status = "OSGymSandboxStatus",
    printcolumn(name = "Phase", type_ = "string", json_path = ".status.phase"),
    printcolumn(name = "Ready", type_ = "boolean", json_path = ".status.ready"),
    printcolumn(name = "VM", type_ = "string", json_path = ".status.vmName"),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    ),
    doc = ""
)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxSpec {
    pub vm_template: VmTemplate,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxStatus {
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default, description = "Pending | Ready | Terminating.")]
    pub phase: Option<String>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(
        default,
        description = "Backend that provisioned this sandbox (\"macos\" for a native macOS Sandbox, \"gvisor\" for a gVisor pod, unset/kubevirt otherwise). Set by the operator."
    )]
    pub runtime: Option<String>,
    #[schemars(schema_with = "bool_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default, description = "Mirrored from the KubeVirt VM status.ready.")]
    pub ready: Option<bool>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default, description = "Name of the owned KubeVirt VirtualMachine.")]
    pub vm_name: Option<String>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default, description = "In-cluster DNS of the VM's Service.")]
    pub service: Option<String>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(
        default,
        description = "Human-readable detail, set when the OSGymSandbox is\nstuck (e.g. spec.vmTemplate.containerDiskImage missing).\n"
    )]
    pub message: Option<String>,
    #[schemars(schema_with = "date_time_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(
        default,
        description = "When the in-place restart (return-to-pool) was\nissued. Used only by the reset-timeout backstop in\nsandbox_mirror_vm_status \u{2014} if the Sandbox is still\nnot Ready RESET_TIMEOUT_SECS after this, the operator\ngives up the restart and deletes the OSGymSandbox CR\n(its finalizer tears down VM+Service, reconcile_pool\nrespawns). Cleared on the Resetting->Ready transition.\n"
    )]
    pub reset_issued_at: Option<String>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(
        default,
        description = "metadata.uid of the VirtualMachineInstance present\nwhen the in-place restart was issued. The mirror\ngates the Resetting->Ready transition on observing a\nVMI with a *different* uid \u{2014} i.e. KubeVirt actually\nrecreated the VMI rather than the old one lingering.\nCleared on the Resetting->Ready transition.\n"
    )]
    pub reset_vmi_uid: Option<String>,
}

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema, uniffi::Record)]
#[kube(
    group = "osgym.cua.ai",
    version = "v1alpha1",
    kind = "OSGymSandboxTemplate",
    plural = "osgymsandboxtemplates",
    namespaced,
    shortname = "osbt",
    printcolumn(
        name = "Image",
        type_ = "string",
        json_path = ".spec.vmTemplate.containerDiskImage"
    ),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    ),
    doc = ""
)]
#[serde(rename_all = "camelCase")]
pub struct OSGymSandboxTemplateSpec {
    pub vm_template: VmTemplate,
}
