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
pub use common::SandboxTemplateRef;
pub use common::{
    Firmware, ImagePullPolicy, OidcConfig, RuntimeKind, SandboxService, ServiceProtocol, VmTemplate,
};
pub use json::{JsonValueError, PreservedJson};
pub use sandbox::{
    OSGymSandbox, OSGymSandboxSpec, OSGymSandboxStatus, OSGymSandboxTemplate,
    OSGymSandboxTemplateSpec,
};
pub use warmpool::{
    OSGymSandboxWarmPool, OSGymSandboxWarmPoolSpec, OSGymSandboxWarmPoolStatus, WarmPoolAutoscaling,
};

uniffi::setup_scaffolding!("cyclops_sdk_schema");
