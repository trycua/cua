//! gRPC errors in the shapes the Modal client maps to its exceptions
//! (`grpc_error_to_modal_exception`): NOT_FOUND -> `NotFoundError`,
//! INVALID_ARGUMENT -> `InvalidError`, ALREADY_EXISTS -> `AlreadyExistsError`,
//! FAILED_PRECONDITION and ABORTED -> `ConflictError`,
//! RESOURCE_EXHAUSTED -> `ResourceExhaustedError`, UNIMPLEMENTED ->
//! `UnimplementedError`, PERMISSION_DENIED -> `PermissionDeniedError`,
//! UNAUTHENTICATED -> `AuthError`; INTERNAL, UNKNOWN, UNAVAILABLE,
//! CANCELLED and DEADLINE_EXCEEDED are retried by the client.

use crate::backend::BackendError;
use tonic::Status;

pub fn invalid(msg: impl Into<String>) -> Status {
    Status::invalid_argument(msg.into())
}

pub fn not_found(msg: impl Into<String>) -> Status {
    Status::not_found(msg.into())
}

pub fn exists(msg: impl Into<String>) -> Status {
    Status::already_exists(msg.into())
}

/// `ConflictError` on the client.
pub fn conflict(msg: impl Into<String>) -> Status {
    Status::failed_precondition(msg.into())
}

pub fn exhausted(msg: impl Into<String>) -> Status {
    Status::resource_exhausted(msg.into())
}

pub fn denied(msg: impl Into<String>) -> Status {
    Status::permission_denied(msg.into())
}

/// A feature Cua Spaces cannot offer: `UnimplementedError` on the client,
/// with the reason.
pub fn unsupported(feature: &str, why: &str) -> Status {
    Status::unimplemented(format!("{feature} is not available on Cua Spaces: {why}"))
}

/// The client retries INTERNAL; use it only for the gateway's own faults.
pub fn internal(msg: impl Into<String>) -> Status {
    Status::internal(msg.into())
}

pub fn from_backend(e: BackendError) -> Status {
    match e {
        BackendError::NotFound(m) => not_found(m),
        BackendError::Invalid(m) => invalid(m),
        BackendError::Unsupported(m) => Status::unimplemented(m),
        // The client retries DEADLINE_EXCEEDED, so a timeout reaching the
        // caller is a capacity failure.
        BackendError::Timeout(m) | BackendError::RateLimited(m) => exhausted(m),
        BackendError::Other(m) => conflict(m),
    }
}

/// Why an RPC the gateway does not serve is unavailable, as the message the
/// client shows (`UnimplementedError`). Every RPC of the pinned client's API
/// maps to one reason here; a test checks none is left to the generic stub.
pub fn unserved_reason(rpc: &str) -> String {
    let (feature, why) = category(rpc);
    format!("{rpc} is not available on Cua Spaces ({feature}): {why}")
}

fn category(rpc: &str) -> (&'static str, &'static str) {
    const F: &str = "this Function RPC belongs to a Modal feature the gateway does not run (metrics time series, the input-plane protocol, managed endpoints, the Modal CLI's call listing or interactive shells); calling Functions (.remote, .map, .spawn), Classes, autoscaler settings and stats are served";
    const BILL: &str = "billing, usage and spend limits belong to a Modal workspace; Cua has no per-workspace billing in the gateway";
    const IAM: &str = "workspace members, roles and service users are a Modal organisation feature; access to Cua Spaces is the Cua account's (`cua login`, `cua team`)";
    const WEB: &str = "web endpoints and webhook tokens need Modal's public edge; expose a port of a Sandbox with encrypted_ports instead";
    const NET: &str =
        "Modal Proxies, custom domains and Flash networking are Modal network products";
    const FS: &str = "network file systems are deprecated by Modal; use a Volume";
    const LEGACY: &str =
        "this is an internal or legacy RPC the supported clients do not use for Sandboxes";
    let p = |pre: &str| rpc.starts_with(pre);
    if p("Metrics")
        || matches!(
            rpc,
            "FunctionGetMetrics"
                | "FunctionGetTimeRangeStats"
                | "ServerGetMetrics"
                | "ServerGetTimeRangeStats"
                | "ServerCountRequests"
        )
    {
        (
            "metrics",
            "metrics time series (utilization, request counts, latency) are a Modal observability product the gateway does not collect; Function.get_current_stats() (backlog and running inputs) is served",
        )
    } else if p("Function")
        || p("Attempt")
        || p("Map")
        || p("Class")
        || p("Endpoint")
        || p("Server")
        || p("TaskCurrentInputs")
        || p("TaskResult")
        || p("FlashSet")
    {
        ("Functions", F)
    } else if p("TaskContainer") {
        (
            "sidecar containers",
            "a Sandbox is one container on Cua Spaces; additional containers sharing its network namespace (sidecars) are not available",
        )
    } else if p("Container") || p("Task") && !p("TaskGetCommandRouterAccess") || p("Cluster") {
        ("container runtime", F)
    } else if p("Billing")
        || p("WorkspaceBilling")
        || p("EnvironmentBilling")
        || p("EnvironmentGetBudget")
        || p("EnvironmentSetBudget")
    {
        ("billing", BILL)
    } else if p("Workspace")
        || p("ServiceUser")
        || p("EnvironmentRole")
        || p("EnvironmentGetRoles")
        || p("EnvironmentSetDefaultMemberRole")
        || p("EnvironmentSetManaged")
        || p("EnvironmentGetManaged")
        || p("UserGroup")
        || p("TokenFlow")
        || p("AuditLog")
    {
        ("workspace administration", IAM)
    } else if p("Webhook") || p("Domain") || p("Template") {
        ("web endpoints", WEB)
    } else if p("Proxy") || p("Flash") || p("Tunnel") {
        ("networking", NET)
    } else if p("SharedVolume") {
        ("network file systems", FS)
    } else if p("Image") || p("App") {
        (
            "deployments",
            "deploying, publishing and rolling back Apps and Images is Modal's deployment registry; Sandboxes use App.lookup and Image builds, which the gateway serves",
        )
    } else if p("Sandbox") || p("PersistentSandbox") {
        (
            "Sandboxes",
            "this Sandbox call has no equivalent on Cua Spaces (resource usage metering, sidecar containers, persistent Sandboxes and async filesystem snapshots are Modal runtime features)",
        )
    } else {
        ("internal", LEGACY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_names_the_rpc_and_a_cause() {
        let r = unserved_reason("WorkspaceBillingReport");
        assert!(
            r.starts_with("WorkspaceBillingReport is not available on Cua Spaces ("),
            "{r}"
        );
        assert!(unserved_reason("FunctionGetMetrics").contains("metrics"));
        assert!(unserved_reason("TaskContainerCreate").contains("sidecar"));
        assert!(unserved_reason("ProxyCreate").contains("Proxies"));
    }
}
