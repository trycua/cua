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

fn process_mode_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    ProcessMode::json_schema(generator)
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

pub(crate) fn env_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    HashMap::<String, String>::json_schema(generator)
}

/// Name prefix of a tenant-owned registry pull Secret. The /api/k8s gateway
/// lets a tenant create (and delete) only `kubernetes.io/dockerconfigjson`
/// Secrets with this prefix, and admits `vmTemplate.imagePullSecret` naming one
/// for any registry. The shared `ecr-credentials` Secret keeps its ECR
/// repository allowlist.
pub const REGISTRY_SECRET_NAME_PREFIX: &str = "cua-registry-";

/// The shared, operator-provisioned ECR pull Secret in every pool namespace.
pub const SHARED_ECR_PULL_SECRET: &str = "ecr-credentials";

pub(crate) fn default_runtime() -> Option<RuntimeKind> {
    Some(RuntimeKind::Kubevirt)
}

pub(crate) fn default_cpu_cores() -> Option<u32> {
    Some(4)
}

pub(crate) fn default_memory() -> Option<String> {
    Some("4Gi".into())
}

pub(crate) fn default_nested_virtualization() -> Option<bool> {
    Some(false)
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

/// How `vmTemplate.command`/`args`/`env` reach the sandbox
/// (`vmTemplate.processMode`). Absent means `Legacy`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
pub enum ProcessMode {
    /// What templates did before processMode existed: pod runtimes run
    /// command/args/env; KubeVirt ignores command and refuses args/env.
    Legacy,
    /// Every runtime runs command/args/env. Pod runtimes set them on the
    /// sandbox container; KubeVirt renders them into the sandbox's cloud-init.
    Run,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, uniffi::Enum)]
pub enum ServiceProtocol {
    TCP,
    UDP,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SchemaBuildError)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SchemaBuildError)]
#[serde(rename_all = "camelCase")]
pub struct VmTemplate {
    #[schemars(
        description = "KubeVirt containerDisk OCI image (runtime=kubevirt) or the sandbox pod image ref (runtime=macos/gvisor)."
    )]
    pub container_disk_image: String,
    #[schemars(
        description = "Entrypoint command (Kubernetes command semantics: replaces the image ENTRYPOINT). Pod runtimes (gvisor/macos) run it on the sandbox container. KubeVirt runs it only with processMode: Run (as /etc/cua/command.sh under cua-command.service, through the sandbox's cloud-init); without it KubeVirt ignores command, as it always has."
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
    #[schemars(
        description = "Pull Secret for containerDiskImage, in the pool namespace. Either the shared ecr-credentials Secret (only for the allowlisted ECR repositories) or a tenant-created kubernetes.io/dockerconfigjson Secret named cua-registry-<name> (any registry). Public images need none.",
        schema_with = "string_schema"
    )]
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
        default = "default_nested_virtualization",
        description = "Enable nested KVM by setting domain.cpu.model to host-passthrough"
    )]
    #[schemars(schema_with = "bool_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested_virtualization: Option<bool>,
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
    #[schemars(
        description = "Opt in to claim-scoped secret delivery (OSGymSandboxClaim spec.secretRef). The pool-operator gives every sandbox an operator-owned Secret, empty while the sandbox is warm, fills it when a claim binds and wipes it on release, so a warm sandbox receives its claimant's secrets without a restart. Pod runtimes mount it read-only as a directory (never subPath) at /run/cua, root-owned, mode 0600; the image keeps its default root user, and its root token-sync helper hands the token to a non-root driver. KubeVirt shares it over virtiofs as tag cua-claim-secrets (the EnableVirtioFsConfigVolumes feature gate is GA from KubeVirt v1.8; older KubeVirt needs it enabled); the guest image mounts that tag read-only at /run/cua. cua-env-driver images enable their await-token mode only when /run/cua is a mount point, so the key env-token becomes /run/cua/env-token. Claims with a secretRef fail on templates without this flag.",
        schema_with = "bool_schema"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    #[uniffi(default = None)]
    pub claim_secrets: Option<bool>,
    #[schemars(
        description = "Arguments (Kubernetes args semantics: they replace the image CMD and follow vmTemplate.command). Runs on pod runtimes (gvisor/macos). On runtime kubevirt it needs processMode: Run and a command (a VM image has no entrypoint to pass them to); without Run the gateway and the pool-operator refuse it."
    )]
    #[schemars(schema_with = "string_list_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    #[uniffi(default = None)]
    pub args: Option<Vec<String>>,
    #[schemars(
        description = "Plain environment variables for the sandbox's command (not for secrets: the values are stored in the template and visible to anyone who can read it). Names must match ^[A-Za-z_][A-Za-z0-9_]*$. $(NAME) references in command/args expand as in Kubernetes. Pod runtimes (gvisor/macos) set them on the sandbox container. On runtime kubevirt they need processMode: Run and go to /etc/cua/env (root, 0600) and the command's environment; values must be single-line there. Without Run the gateway and the pool-operator refuse env on kubevirt."
    )]
    #[schemars(schema_with = "env_schema")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    #[uniffi(default = None)]
    pub env: Option<HashMap<String, String>>,
    #[schemars(
        description = "How command, args and env reach the sandbox. Absent or Legacy: unchanged behavior (pod runtimes run them; KubeVirt ignores command and refuses args and env). Run: every runtime runs them with the same semantics. Pod runtimes set them on the sandbox container; KubeVirt writes /etc/cua/env (0600), /etc/cua/command.sh (0700) and cua-command.service into the sandbox's cloud-init Secret, which the guest re-reads on every boot, so warm VMs get it on return-to-pool too. KubeVirt Run needs a Linux guest with cloud-init and systemd.",
        schema_with = "process_mode_schema"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    #[uniffi(default = None)]
    pub process_mode: Option<ProcessMode>,
}

pub(crate) fn date_time_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "string", "format": "date-time"})
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SchemaBuildError)]
pub struct SandboxTemplateRef {
    pub name: String,
}
