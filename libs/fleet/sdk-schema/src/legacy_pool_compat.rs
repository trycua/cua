use super::PoolStatus;

/// Canonical Rust name for the Kubernetes workspace-pool resource.
pub type Pool = super::workspace_pool::OSGymWorkspacePool;

#[deprecated(note = "use Pool")]
pub type OSGymWorkspacePool = Pool;

#[deprecated(note = "use PoolStatus")]
pub type OSGymWorkspacePoolStatus = PoolStatus;

#[cfg(test)]
mod tests {
    use super::{OSGymWorkspacePool, OSGymWorkspacePoolStatus, Pool, PoolStatus};

    #[allow(deprecated)]
    #[test]
    fn legacy_names_are_type_aliases() {
        fn pool(_: Pool) {}
        fn legacy_pool(_: OSGymWorkspacePool) {}
        fn status(_: Option<PoolStatus>) {}
        fn legacy_status(_: Option<OSGymWorkspacePoolStatus>) {}

        let _ = (pool, legacy_pool, status, legacy_status);
    }
}
