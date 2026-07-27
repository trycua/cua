mod claim;
mod common;
pub mod generate;
mod json;
mod sandbox;
mod warmpool;
mod workspace_pool;
mod legacy_pool_compat;

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
pub use workspace_pool::{PoolSpec, PoolStatus, PoolTemplate, WorkspacePoolAutoscaling};
pub use legacy_pool_compat::*;

uniffi::setup_scaffolding!("cyclops_sdk_schema");
