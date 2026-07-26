use crate::PreservedJson;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

pub(crate) fn string_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    String::json_schema(generator)
}

pub(crate) fn bool_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    bool::json_schema(generator)
}

pub(crate) fn integer_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer"})
}

pub(crate) fn string_list_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Vec::<String>::json_schema(generator)
}

pub(crate) fn runtime_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    RuntimeKind::json_schema(generator)
}

pub(crate) fn firmware_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Firmware::json_schema(generator)
}

fn image_pull_policy_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    ImagePullPolicy::json_schema(generator)
}

fn service_protocol_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    ServiceProtocol::json_schema(generator)
}

pub(crate) fn node_selector_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    HashMap::<String, String>::json_schema(generator)
}

pub(crate) fn tolerations_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Vec::<Arc<PreservedJson>>::json_schema(generator)
}

pub(crate) fn preserved_json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    PreservedJson::json_schema(generator)
}

fn services_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Vec::<SandboxService>::json_schema(generator)
}

pub(crate) fn oidc_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    OidcConfig::json_schema(generator)
}

pub(crate) fn default_runtime() -> Option<RuntimeKind> {
    Some(RuntimeKind::Kubevirt)
}

pub(crate) fn default_cpu_cores() -> Option<u32> {
    Some(4)
}

pub(crate) fn default_memory() -> Option<String> {
    Some("4Gi".into())
}

pub(crate) fn default_firmware() -> Option<Firmware> {
    Some(Firmware::Bios)
}

fn default_protocol() -> Option<ServiceProtocol> {
    Some(ServiceProtocol::TCP)
}

fn default_aws_region() -> Option<String> {
    Some("us-west-2".into())
}

fn default_refresh_interval_seconds() -> Option<u32> {
    Some(1800)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Kubevirt,
    Macos,
    Gvisor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    Bios,
    Efi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
pub enum ImagePullPolicy {
    Always,
    IfNotPresent,
    Never,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
pub enum ServiceProtocol {
    TCP,
    UDP,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SandboxService {
    #[schemars(description = "Service name suffix (sandbox name is prepended).")]
    pub name: String,
    #[schemars(
        description = "Port on the VM pod to forward to.",
        range(min = 1, max = 65535)
    )]
    #[schemars(schema_with = "integer_schema")]
    pub target_port: u16,
    #[schemars(default = "default_protocol", schema_with = "service_protocol_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ServiceProtocol>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct OidcConfig {
    #[schemars(
        description = "Name of a Secret in the pool namespace holding the tenant's Keycloak client credentials under keys client_id and client_secret."
    )]
    pub credentials_secret: String,
    #[schemars(
        description = "Keycloak token endpoint for the workloads realm, e.g. https://auth.cua.ai/realms/workloads/protocol/openid-connect/token"
    )]
    pub token_url: String,
    #[schemars(
        description = "Optional. When set, the guest is configured (env + ~/.aws/config) so the AWS SDK assumes this role via the injected web-identity token with no extra setup.",
        schema_with = "string_schema"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub aws_role_arn: Option<String>,
    #[schemars(
        default = "default_aws_region",
        schema_with = "string_schema",
        description = "AWS region for in-guest AWS SDK calls."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    #[schemars(
        default = "default_refresh_interval_seconds",
        schema_with = "integer_schema",
        description = "How often the in-guest refresher re-mints the token (half the 1h token TTL by default).",
        range(min = 60)
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_interval_seconds: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct VmTemplate {
    #[schemars(
        description = "KubeVirt containerDisk OCI image (runtime=kubevirt) or the sandbox pod image ref (runtime=macos/gvisor)."
    )]
    pub container_disk_image: String,
    #[schemars(
        description = "Pod runtimes (macos/gvisor) only. Entrypoint command for the sandbox container (overrides the image default)."
    )]
    #[schemars(schema_with = "string_list_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub command: Option<Vec<String>>,
    #[schemars(
        default = "default_runtime",
        description = "Pool backend runtime. \"kubevirt\" (default) reconciles each sandbox into a KubeVirt VM. \"macos\" reconciles it into a macOS sandbox (an agent-sandbox Sandbox on a macOS node); \"gvisor\" into a gVisor (runsc) pod on the gVisor K3s workers (also an agent-sandbox Sandbox). For the pod runtimes containerDiskImage is the pod image ref and firmware/cpuCores/memory are advisory."
    )]
    #[schemars(schema_with = "runtime_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeKind>,
    #[schemars(
        description = "Pod runtimes (macos/gvisor) only. RuntimeClass for the sandbox pod (defaults: \"cua-macos-native\" for macos, \"gvisor\" for gvisor). Ignored for kubevirt."
    )]
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub runtime_class_name: Option<String>,
    #[schemars(
        description = "Pod runtimes (macos/gvisor) only. nodeSelector for the sandbox pod (defaults: cua.ai/macos=true for macos, cua.ai/gvisor=enabled for gvisor)."
    )]
    #[schemars(schema_with = "node_selector_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub node_selector: Option<HashMap<String, String>>,
    #[schemars(
        description = "Pod runtimes (macos/gvisor) only. Tolerations for the sandbox pod so it schedules onto tainted nodes (macos defaults tolerate the cua.ai/macos taint; the gVisor workers are untainted, so gvisor defaults to none)."
    )]
    #[schemars(schema_with = "tolerations_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub tolerations: Option<Vec<Arc<PreservedJson>>>,
    #[schemars(
        description = "Pod runtimes (macos/gvisor) only. Image pull policy (default IfNotPresent)."
    )]
    #[schemars(schema_with = "image_pull_policy_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub image_pull_policy: Option<ImagePullPolicy>,
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub image_pull_secret: Option<String>,
    #[schemars(default = "default_cpu_cores", range(min = 1))]
    #[schemars(schema_with = "integer_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<u32>,
    #[schemars(default = "default_memory")]
    #[schemars(schema_with = "string_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[schemars(
        default = "default_firmware",
        description = "VM firmware. Use \"efi\" for GPT/UEFI-only guest images (e.g. the dockur-built Windows desktop-workspace); \"bios\" is KubeVirt's default and what the Linux workspace images boot with."
    )]
    #[schemars(schema_with = "firmware_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<Firmware>,
    #[schemars(
        description = "Optional KubeVirt readinessProbe/livenessProbe for the VMI, gating ready on the guest actually serving."
    )]
    #[schemars(schema_with = "preserved_json_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub probes: Option<Arc<PreservedJson>>,
    #[schemars(description = "Extra K8s Services created per-sandbox.")]
    #[schemars(schema_with = "services_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub services: Option<Vec<SandboxService>>,
    #[schemars(
        description = "Optional Keycloak workload-OIDC token injection. When set, the pool-operator renders a cloud-init disk that drops the tenant-scoped Keycloak client credentials into the guest and runs an in-guest systemd refresher that mints + rotates an OIDC access token at /var/run/cua/oidc/token. Lets the workload federate into AWS (sts:AssumeRoleWithWebIdentity) and other OIDC-trust providers, scoped to the tenant that owns the pool. See docs/decisions/2026-06-25-osgym-pool-workload-oidc.md."
    )]
    #[schemars(schema_with = "oidc_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub oidc: Option<OidcConfig>,
}

pub(crate) fn date_time_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "string", "format": "date-time"})
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Record)]
pub struct SandboxTemplateRef {
    pub name: String,
}
