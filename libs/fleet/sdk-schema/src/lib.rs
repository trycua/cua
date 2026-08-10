mod claim;
mod common;
pub mod generate;
mod json;
mod sandbox;
mod warmpool;

pub use claim::{
    ClaimLifecycle, ClaimSpec, OSGymSandboxClaim, OSGymSandboxClaimCondition,
    OSGymSandboxClaimSandbox, OSGymSandboxClaimStatus,
};
pub use common::{
    Firmware, ImagePullPolicy, OidcConfig, RuntimeKind, SandboxService, SandboxServiceBuilder,
    ServiceProtocol, VmTemplate, VmTemplateBuilder,
};
pub use common::{SandboxTemplateRef, SandboxTemplateRefBuilder};
pub use json::{JsonValueError, PreservedJson};
pub use sandbox::{
    OSGymSandbox, OSGymSandboxSpec, OSGymSandboxStatus, OSGymSandboxTemplate,
    OSGymSandboxTemplateSpec, OSGymSandboxTemplateSpecBuilder,
};
pub use warmpool::{
    OSGymSandboxWarmPool, OSGymSandboxWarmPoolSpec, OSGymSandboxWarmPoolSpecBuilder,
    OSGymSandboxWarmPoolStatus, WarmPoolAutoscaling,
};

uniffi::setup_scaffolding!("cyclops_sdk_schema");

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SchemaBuildError {
    #[error("{record_type} is missing required field {field}")]
    MissingRequiredField { record_type: String, field: String },
}

impl SchemaBuildError {
    pub fn missing(record_type: &str, field: &str) -> Self {
        Self::MissingRequiredField {
            record_type: record_type.into(),
            field: field.into(),
        }
    }
}
