use crate::Pool;
use serde::{Deserialize, Serialize};

const HEALTHY_INDICATOR: &str = "#2ea043";
const SCALED_TO_ZERO_INDICATOR: &str = "#a371f7";
const REMOVED_INDICATOR: &str = "#8b949e";
const OTHER_INDICATOR: &str = "#d29922";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum PoolDisplayStatusKind {
    Healthy,
    ScaledToZero,
    Removed,
    Terminating,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PoolDisplayStatus {
    pub kind: PoolDisplayStatusKind,
    pub label: String,
    pub indicator: String,
}

fn display_status(kind: PoolDisplayStatusKind, label: &str, indicator: &str) -> PoolDisplayStatus {
    PoolDisplayStatus {
        kind,
        label: label.into(),
        indicator: indicator.into(),
    }
}

#[uniffi::export]
pub fn pool_display_status(pool: Pool) -> PoolDisplayStatus {
    if pool.spec.replicas == 0 {
        return display_status(
            PoolDisplayStatusKind::ScaledToZero,
            "Scaled to zero",
            SCALED_TO_ZERO_INDICATOR,
        );
    }
    if pool.status.is_none() {
        return unknown_pool_display_status();
    }
    healthy_pool_display_status()
}

#[uniffi::export]
pub fn healthy_pool_display_status() -> PoolDisplayStatus {
    display_status(PoolDisplayStatusKind::Healthy, "Healthy", HEALTHY_INDICATOR)
}

#[uniffi::export]
pub fn removed_pool_display_status() -> PoolDisplayStatus {
    display_status(PoolDisplayStatusKind::Removed, "Removed", REMOVED_INDICATOR)
}

#[uniffi::export]
pub fn terminating_pool_display_status() -> PoolDisplayStatus {
    display_status(
        PoolDisplayStatusKind::Terminating,
        "Terminating",
        OTHER_INDICATOR,
    )
}

#[uniffi::export]
pub fn unknown_pool_display_status() -> PoolDisplayStatus {
    display_status(PoolDisplayStatusKind::Unknown, "Unknown", OTHER_INDICATOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceMetadata;
    use cyclops_sdk_schema::OSGymSandboxWarmPoolStatus;

    fn pool(replicas: u32, status: Option<OSGymSandboxWarmPoolStatus>) -> Pool {
        Pool {
            api_version: "osgym.cua.ai/v1alpha1".into(),
            kind: "OSGymSandboxWarmPool".into(),
            metadata: ResourceMetadata {
                namespace: "default".into(),
                name: "pool".into(),
                labels: None,
                creation_timestamp: None,
            },
            spec: serde_json::from_value(serde_json::json!({
                "replicas": replicas,
                "sandboxTemplateRef": { "name": "pool-template" },
            }))
            .unwrap(),
            status,
        }
    }

    #[test]
    fn fully_claimed_pool_is_healthy() {
        let status = OSGymSandboxWarmPoolStatus {
            replicas: Some(1),
            ready_replicas: Some(0),
            selector: None,
        };
        let display = pool_display_status(pool(1, Some(status)));
        assert_eq!(display.kind, PoolDisplayStatusKind::Healthy);
        assert_eq!(display.label, "Healthy");
        assert_eq!(display.indicator, HEALTHY_INDICATOR);
    }

    #[test]
    fn scaled_to_zero_and_unknown_have_sdk_owned_descriptors() {
        assert_eq!(
            pool_display_status(pool(0, Some(OSGymSandboxWarmPoolStatus::default()))).kind,
            PoolDisplayStatusKind::ScaledToZero
        );
        assert_eq!(
            pool_display_status(pool(1, None)).kind,
            PoolDisplayStatusKind::Unknown
        );
        assert_eq!(removed_pool_display_status().indicator, REMOVED_INDICATOR);
    }
}
