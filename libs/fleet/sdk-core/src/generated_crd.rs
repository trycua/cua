// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "bindDeadline")]
    pub bind_deadline: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lifecycle")]
    pub lifecycle: Option<ClaimSpecLifecycle>,
    #[serde(rename = "sandboxTemplateRef")]
    pub sandbox_template_ref: SandboxTemplateRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "warmpool")]
    pub warmpool: Option<String>,
}

impl ClaimSpec {
    pub fn builder() -> ClaimSpecBuilder {
        ClaimSpecBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaimSpecBuilder {
    bind_deadline: Option<u32>,
    lifecycle: Option<ClaimSpecLifecycle>,
    sandbox_template_ref: Option<SandboxTemplateRef>,
    warmpool: Option<String>,
}

impl ClaimSpecBuilder {
    pub fn bind_deadline(mut self, value: u32) -> Self {
        self.bind_deadline = Some(value);
        self
    }
    pub fn lifecycle(mut self, value: ClaimSpecLifecycle) -> Self {
        self.lifecycle = Some(value);
        self
    }
    pub fn sandbox_template_ref(mut self, value: SandboxTemplateRef) -> Self {
        self.sandbox_template_ref = Some(value);
        self
    }
    pub fn warmpool(mut self, value: String) -> Self {
        self.warmpool = Some(value);
        self
    }
    pub fn build(self) -> Result<ClaimSpec, String> {
        Ok(ClaimSpec {
            bind_deadline: self.bind_deadline,
            lifecycle: self.lifecycle,
            sandbox_template_ref: self
                .sandbox_template_ref
                .ok_or_else(|| "sandboxTemplateRef is required".to_string())?,
            warmpool: self.warmpool,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimSpecLifecycle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "autoRenew")]
    pub auto_renew: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "shutdownPolicy")]
    pub shutdown_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "shutdownTime")]
    pub shutdown_time: Option<String>,
}

impl ClaimSpecLifecycle {
    pub fn builder() -> ClaimSpecLifecycleBuilder {
        ClaimSpecLifecycleBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaimSpecLifecycleBuilder {
    auto_renew: Option<bool>,
    shutdown_policy: Option<String>,
    shutdown_time: Option<String>,
}

impl ClaimSpecLifecycleBuilder {
    pub fn auto_renew(mut self, value: bool) -> Self {
        self.auto_renew = Some(value);
        self
    }
    pub fn shutdown_policy(mut self, value: String) -> Self {
        self.shutdown_policy = Some(value);
        self
    }
    pub fn shutdown_time(mut self, value: String) -> Self {
        self.shutdown_time = Some(value);
        self
    }
    pub fn build(self) -> Result<ClaimSpecLifecycle, String> {
        Ok(ClaimSpecLifecycle {
            auto_renew: self.auto_renew,
            shutdown_policy: self.shutdown_policy,
            shutdown_time: self.shutdown_time,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "autoscaling")]
    pub autoscaling: Option<PoolSpecAutoscaling>,
    #[serde(rename = "replicas")]
    pub replicas: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "services")]
    pub services: Option<Vec<PoolSpecService>>,
    #[serde(rename = "template")]
    pub template: PoolTemplate,
}

impl PoolSpec {
    pub fn builder() -> PoolSpecBuilder {
        PoolSpecBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolSpecBuilder {
    autoscaling: Option<PoolSpecAutoscaling>,
    replicas: Option<u32>,
    services: Option<Vec<PoolSpecService>>,
    template: Option<PoolTemplate>,
}

impl PoolSpecBuilder {
    pub fn autoscaling(mut self, value: PoolSpecAutoscaling) -> Self {
        self.autoscaling = Some(value);
        self
    }
    pub fn replicas(mut self, value: u32) -> Self {
        self.replicas = Some(value);
        self
    }
    pub fn services(mut self, value: Vec<PoolSpecService>) -> Self {
        self.services = Some(value);
        self
    }
    pub fn template(mut self, value: PoolTemplate) -> Self {
        self.template = Some(value);
        self
    }
    pub fn build(self) -> Result<PoolSpec, String> {
        Ok(PoolSpec {
            autoscaling: self.autoscaling,
            replicas: self
                .replicas
                .ok_or_else(|| "replicas is required".to_string())?,
            services: self.services,
            template: self
                .template
                .ok_or_else(|| "template is required".to_string())?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolSpecAutoscaling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "initialPoolSize")]
    pub initial_pool_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "maxPoolSize")]
    pub max_pool_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "minPoolSize")]
    pub min_pool_size: Option<u32>,
}

impl PoolSpecAutoscaling {
    pub fn builder() -> PoolSpecAutoscalingBuilder {
        PoolSpecAutoscalingBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolSpecAutoscalingBuilder {
    initial_pool_size: Option<u32>,
    max_pool_size: Option<u32>,
    min_pool_size: Option<u32>,
}

impl PoolSpecAutoscalingBuilder {
    pub fn initial_pool_size(mut self, value: u32) -> Self {
        self.initial_pool_size = Some(value);
        self
    }
    pub fn max_pool_size(mut self, value: u32) -> Self {
        self.max_pool_size = Some(value);
        self
    }
    pub fn min_pool_size(mut self, value: u32) -> Self {
        self.min_pool_size = Some(value);
        self
    }
    pub fn build(self) -> Result<PoolSpecAutoscaling, String> {
        Ok(PoolSpecAutoscaling {
            initial_pool_size: self.initial_pool_size,
            max_pool_size: self.max_pool_size,
            min_pool_size: self.min_pool_size,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolSpecService {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "protocol")]
    pub protocol: Option<PoolSpecServiceProtocol>,
    #[serde(rename = "targetPort")]
    pub target_port: u32,
}

impl PoolSpecService {
    pub fn builder() -> PoolSpecServiceBuilder {
        PoolSpecServiceBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolSpecServiceBuilder {
    name: Option<String>,
    protocol: Option<PoolSpecServiceProtocol>,
    target_port: Option<u32>,
}

impl PoolSpecServiceBuilder {
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }
    pub fn protocol(mut self, value: PoolSpecServiceProtocol) -> Self {
        self.protocol = Some(value);
        self
    }
    pub fn target_port(mut self, value: u32) -> Self {
        self.target_port = Some(value);
        self
    }
    pub fn build(self) -> Result<PoolSpecService, String> {
        Ok(PoolSpecService {
            name: self.name.ok_or_else(|| "name is required".to_string())?,
            protocol: self.protocol,
            target_port: self
                .target_port
                .ok_or_else(|| "targetPort is required".to_string())?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolSpecServiceProtocol {
    #[serde(rename = "TCP")]
    TCP,
    #[serde(rename = "UDP")]
    UDP,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "command")]
    pub command: Option<Vec<String>>,
    #[serde(rename = "containerDiskImage")]
    pub container_disk_image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "cpuCores")]
    pub cpu_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "firmware")]
    pub firmware: Option<PoolTemplateFirmware>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "imagePullSecret")]
    pub image_pull_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "memory")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nodeSelector")]
    pub node_selector: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "oidc")]
    pub oidc: Option<PoolTemplateOidc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "probes")]
    pub probes: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "runtime")]
    pub runtime: Option<PoolTemplateRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "runtimeClassName")]
    pub runtime_class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tolerations")]
    pub tolerations: Option<Vec<serde_json::Value>>,
}

impl PoolTemplate {
    pub fn builder() -> PoolTemplateBuilder {
        PoolTemplateBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolTemplateBuilder {
    command: Option<Vec<String>>,
    container_disk_image: Option<String>,
    cpu_cores: Option<u32>,
    firmware: Option<PoolTemplateFirmware>,
    image_pull_secret: Option<String>,
    memory: Option<String>,
    node_selector: Option<std::collections::BTreeMap<String, String>>,
    oidc: Option<PoolTemplateOidc>,
    probes: Option<serde_json::Value>,
    runtime: Option<PoolTemplateRuntime>,
    runtime_class_name: Option<String>,
    tolerations: Option<Vec<serde_json::Value>>,
}

impl PoolTemplateBuilder {
    pub fn command(mut self, value: Vec<String>) -> Self {
        self.command = Some(value);
        self
    }
    pub fn container_disk_image(mut self, value: String) -> Self {
        self.container_disk_image = Some(value);
        self
    }
    pub fn cpu_cores(mut self, value: u32) -> Self {
        self.cpu_cores = Some(value);
        self
    }
    pub fn firmware(mut self, value: PoolTemplateFirmware) -> Self {
        self.firmware = Some(value);
        self
    }
    pub fn image_pull_secret(mut self, value: String) -> Self {
        self.image_pull_secret = Some(value);
        self
    }
    pub fn memory(mut self, value: String) -> Self {
        self.memory = Some(value);
        self
    }
    pub fn node_selector(mut self, value: std::collections::BTreeMap<String, String>) -> Self {
        self.node_selector = Some(value);
        self
    }
    pub fn oidc(mut self, value: PoolTemplateOidc) -> Self {
        self.oidc = Some(value);
        self
    }
    pub fn probes(mut self, value: serde_json::Value) -> Self {
        self.probes = Some(value);
        self
    }
    pub fn runtime(mut self, value: PoolTemplateRuntime) -> Self {
        self.runtime = Some(value);
        self
    }
    pub fn runtime_class_name(mut self, value: String) -> Self {
        self.runtime_class_name = Some(value);
        self
    }
    pub fn tolerations(mut self, value: Vec<serde_json::Value>) -> Self {
        self.tolerations = Some(value);
        self
    }
    pub fn build(self) -> Result<PoolTemplate, String> {
        Ok(PoolTemplate {
            command: self.command,
            container_disk_image: self
                .container_disk_image
                .ok_or_else(|| "containerDiskImage is required".to_string())?,
            cpu_cores: self.cpu_cores,
            firmware: self.firmware,
            image_pull_secret: self.image_pull_secret,
            memory: self.memory,
            node_selector: self.node_selector,
            oidc: self.oidc,
            probes: self.probes,
            runtime: self.runtime,
            runtime_class_name: self.runtime_class_name,
            tolerations: self.tolerations,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolTemplateFirmware {
    #[serde(rename = "bios")]
    Bios,
    #[serde(rename = "efi")]
    Efi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolTemplateOidc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "awsRegion")]
    pub aws_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "awsRoleArn")]
    pub aws_role_arn: Option<String>,
    #[serde(rename = "credentialsSecret")]
    pub credentials_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "refreshIntervalSeconds")]
    pub refresh_interval_seconds: Option<u32>,
    #[serde(rename = "tokenUrl")]
    pub token_url: String,
}

impl PoolTemplateOidc {
    pub fn builder() -> PoolTemplateOidcBuilder {
        PoolTemplateOidcBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PoolTemplateOidcBuilder {
    aws_region: Option<String>,
    aws_role_arn: Option<String>,
    credentials_secret: Option<String>,
    refresh_interval_seconds: Option<u32>,
    token_url: Option<String>,
}

impl PoolTemplateOidcBuilder {
    pub fn aws_region(mut self, value: String) -> Self {
        self.aws_region = Some(value);
        self
    }
    pub fn aws_role_arn(mut self, value: String) -> Self {
        self.aws_role_arn = Some(value);
        self
    }
    pub fn credentials_secret(mut self, value: String) -> Self {
        self.credentials_secret = Some(value);
        self
    }
    pub fn refresh_interval_seconds(mut self, value: u32) -> Self {
        self.refresh_interval_seconds = Some(value);
        self
    }
    pub fn token_url(mut self, value: String) -> Self {
        self.token_url = Some(value);
        self
    }
    pub fn build(self) -> Result<PoolTemplateOidc, String> {
        Ok(PoolTemplateOidc {
            aws_region: self.aws_region,
            aws_role_arn: self.aws_role_arn,
            credentials_secret: self
                .credentials_secret
                .ok_or_else(|| "credentialsSecret is required".to_string())?,
            refresh_interval_seconds: self.refresh_interval_seconds,
            token_url: self
                .token_url
                .ok_or_else(|| "tokenUrl is required".to_string())?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PoolTemplateRuntime {
    #[serde(rename = "kubevirt")]
    Kubevirt,
    #[serde(rename = "macos")]
    Macos,
    #[serde(rename = "gvisor")]
    Gvisor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SandboxTemplateRef {
    #[serde(rename = "name")]
    pub name: String,
}

impl SandboxTemplateRef {
    pub fn builder() -> SandboxTemplateRefBuilder {
        SandboxTemplateRefBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SandboxTemplateRefBuilder {
    name: Option<String>,
}

impl SandboxTemplateRefBuilder {
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }
    pub fn build(self) -> Result<SandboxTemplateRef, String> {
        Ok(SandboxTemplateRef {
            name: self.name.ok_or_else(|| "name is required".to_string())?,
        })
    }
}
