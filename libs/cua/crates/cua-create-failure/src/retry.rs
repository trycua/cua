//! Which failed creates are worth one automatic retry, and the finer cause
//! word a failed create sends as `error_subclass`.
//!
//! A create that fails for a passing reason (the built-in Linux runtime did
//! not come up this time, the network dropped during the image download, a
//! VM or its service did not answer in time) usually works when it runs
//! again: the image layers that finished downloading are kept. One that
//! fails for a lasting reason (no disk space, no runtime, no permission, an
//! image that is not there, a quota) fails the same way every time, so
//! running it again only repeats the error.
//!
//! [`decide`] is the one place that says which is which. The Spaces app
//! core retries a [`Retry::Once`] failure once, by itself, and never retries
//! a [`Retry::Never`] one.

/// What to do with a failed create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// A passing failure: run the create once more, by itself.
    Once,
    /// A lasting failure: running it again fails the same way.
    Never,
}

/// The finer cause of a failed create, where its `error_kind` alone is too
/// coarse (`runtime`, `local_network`). A fixed word, never the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subclass {
    /// Nothing finer to say (or the create did not fail).
    None,
    /// A local runtime did not start this time (the built-in Linux
    /// runtime's VM, its Docker engine, Apple's container services, a
    /// microVM).
    RuntimeStart,
    /// Cua's background service did not take the create in time.
    DaemonUnresponsive,
    /// The image download stopped partway; another try resumes it.
    PullInterrupted,
    /// macOS Local Network privacy is off for the app or its service.
    LocalNetworkDenied,
    /// A VPN routes the VM's private network away.
    LocalNetworkVpn,
}

/// Every [`Subclass`] word, in order (`cua_telemetry::schema::ERROR_SUBCLASSES`).
pub const SUBCLASSES: &[&str] = &[
    "none",
    "runtime_start",
    "daemon_unresponsive",
    "pull_interrupted",
    "local_network_denied",
    "local_network_vpn",
];

impl Subclass {
    /// Its word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RuntimeStart => "runtime_start",
            Self::DaemonUnresponsive => "daemon_unresponsive",
            Self::PullInterrupted => "pull_interrupted",
            Self::LocalNetworkDenied => "local_network_denied",
            Self::LocalNetworkVpn => "local_network_vpn",
        }
    }

    /// The subclass a word names (`none` for any other word).
    pub fn from_word(word: &str) -> Self {
        match word.trim() {
            "runtime_start" => Self::RuntimeStart,
            "daemon_unresponsive" => Self::DaemonUnresponsive,
            "pull_interrupted" => Self::PullInterrupted,
            "local_network_denied" => Self::LocalNetworkDenied,
            "local_network_vpn" => Self::LocalNetworkVpn,
            _ => Self::None,
        }
    }
}

/// A kind word in snake case (`InsufficientDisk` -> `insufficient_disk`),
/// as `create_failure_kind` may return the caller's enum case.
pub fn snake(kind: &str) -> String {
    let mut out = String::new();
    for (i, c) in kind.trim().chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Kinds whose cause lasts: the same create fails the same way again.
const LASTING: &[&str] = &[
    "cancelled",
    "cloud_ended",
    "insufficient_disk",
    "runtime_permission",
    "runtime_missing",
    "runtime_not_running",
    "unsupported",
    "invalid_argument",
    "invalid_placement",
    "local_network",
    "local_network_denied",
    "local_network_vpn",
    "out_of_memory",
    "quota_exceeded",
    "vm_limit",
    "virtualization_unavailable",
    "image_pull_not_found",
    "image_pull_auth",
    "image_pull_rate_limited",
    "unauthenticated",
    "port_in_use",
    "file_access",
    "host_capacity",
    "not_found",
    "provider_not_configured",
];

/// The finer cause of a failure of `kind` (a `create_failure_kind` word)
/// with the error text `message` (read here, never returned).
pub fn subclass(kind: &str, message: &str) -> Subclass {
    let kind = snake(kind);
    match kind.as_str() {
        "local_network_denied" => return Subclass::LocalNetworkDenied,
        "local_network_vpn" => return Subclass::LocalNetworkVpn,
        _ => {}
    }
    // A specific lasting cause stays itself: "permission denied" in a
    // runtime's start is a permission problem, not a passing one.
    if LASTING.contains(&kind.as_str()) {
        return Subclass::None;
    }
    let l = message.to_ascii_lowercase();
    let has = |any: &[&str]| any.iter().any(|n| l.contains(n));
    if has(&["the image download stopped before it finished"]) {
        return Subclass::PullInterrupted;
    }
    if has(&[
        "didn't start this create within",
        "did not start this create within",
        "did not take the create",
    ]) {
        return Subclass::DaemonUnresponsive;
    }
    if has(&[
        "built-in linux runtime did not start",
        "built-in linux runtime started but its docker engine does not answer",
        "container services did not start",
        "firecracker did not start",
        "did not start the vm in time",
        "runtime did not start",
    ]) {
        return Subclass::RuntimeStart;
    }
    Subclass::None
}

/// Whether a failed create of `kind` with subclass `sub` runs once more by
/// itself. Only causes known to pass: a runtime that did not start this
/// time, the service that did not take the create, a network failure or an
/// interruption during the image download, a VM or its service that did
/// not answer in time. Never a lasting cause ([`LASTING`]), nor a cause
/// nobody named (`other`, a bare `timeout`).
pub fn decide(kind: &str, sub: Subclass) -> Retry {
    let kind = snake(kind);
    if LASTING.contains(&kind.as_str()) {
        return Retry::Never;
    }
    match kind.as_str() {
        "image_pull_network" | "vm_boot_timeout" | "services_unreachable" => Retry::Once,
        _ if matches!(
            sub,
            Subclass::RuntimeStart | Subclass::DaemonUnresponsive | Subclass::PullInterrupted
        ) =>
        {
            Retry::Once
        }
        _ => Retry::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_failure_kind;

    /// The kind and subclass a local create's error text gets.
    fn classify(message: &str, phase: &str, variant: &str) -> (String, Subclass, Retry) {
        let kind = snake(create_failure_kind(message, phase, variant, true));
        let sub = subclass(&kind, message);
        let retry = decide(&kind, sub);
        (kind, sub, retry)
    }

    #[test]
    fn every_subclass_word_is_in_the_telemetry_schema() {
        assert_eq!(SUBCLASSES, cua_telemetry::schema::ERROR_SUBCLASSES);
        for w in SUBCLASSES {
            assert_eq!(Subclass::from_word(w).as_str(), *w);
        }
    }

    #[test]
    fn passing_failures_retry_once() {
        let cases = [
            (
                "the built-in Linux runtime did not start: exiting, status 1",
                "preparing",
                "Runtime",
                "runtime",
                Subclass::RuntimeStart,
            ),
            (
                "the built-in Linux runtime did not start within 10 minutes",
                "preparing",
                "",
                "other",
                Subclass::RuntimeStart,
            ),
            (
                "the built-in Linux runtime started but its Docker engine does not answer",
                "preparing",
                "Runtime",
                "runtime",
                Subclass::RuntimeStart,
            ),
            (
                "registry: error sending request: connection reset by peer",
                "pulling",
                "Runtime",
                "image_pull_network",
                Subclass::None,
            ),
            (
                "The image download stopped before it finished (lume stopped). Try again; it picks up where it stopped.",
                "pulling",
                "Runtime",
                "image_pull",
                Subclass::PullInterrupted,
            ),
            (
                "sandbox 'x' was not ready after 300s",
                "booting",
                "",
                "vm_boot_timeout",
                Subclass::None,
            ),
            (
                "cua-spacesd did not answer within 120s",
                "waiting_for_services",
                "",
                "services_unreachable",
                Subclass::None,
            ),
            (
                "Cua's background service didn't start this create within 30 s. Cua Spaces reconnected to it. Try again.",
                "preparing",
                "",
                "other",
                Subclass::DaemonUnresponsive,
            ),
        ];
        for (msg, phase, variant, want_kind, want_sub) in cases {
            let (kind, sub, retry) = classify(msg, phase, variant);
            assert_eq!(kind, want_kind, "{msg}");
            assert_eq!(sub, want_sub, "{msg}");
            assert_eq!(retry, Retry::Once, "{msg}");
        }
    }

    #[test]
    fn lasting_failures_never_retry() {
        let cases = [
            (
                "not enough disk space to pull x: it needs about 30.0 GB",
                "preparing",
                "",
            ),
            ("write: No space left on device", "pulling", ""),
            (
                "permission denied on unix:///var/run/docker.sock",
                "preparing",
                "Runtime",
            ),
            // A runtime that does not start for want of permission is a
            // permission problem.
            (
                "the built-in Linux runtime did not start: permission denied",
                "preparing",
                "Runtime",
            ),
            (
                "Docker is not running: start Docker Desktop",
                "preparing",
                "Runtime",
            ),
            (
                "QEMU (qemu-system-x86_64, qemu-img) is not available: install it",
                "pulling",
                "",
            ),
            ("no such feature", "", "Unsupported"),
            ("bad name", "", "InvalidArgument"),
            (
                "Local Network access is not available: cannot reach the VM at 192.168.64.3 (No route to host)",
                "waiting_for_services",
                "Runtime",
            ),
            ("Cannot allocate memory (os error 12)", "booting", "Runtime"),
            ("sandbox quota reached for this org", "", "Fleet"),
            (
                "container engine: pull ghcr.io/x/y:1: manifest unknown",
                "",
                "Runtime",
            ),
            (
                "registry: Not authorized: ghcr.io/me/private",
                "pulling",
                "",
            ),
            (
                "container engine: pull x: toomanyrequests: slow down",
                "pulling",
                "",
            ),
            ("VZErrorDomain virtualMachineLimitExceeded", "booting", ""),
            (
                "Virtualization is not available on this Mac",
                "preparing",
                "",
            ),
            ("unsupported: Cua Cloud has ended", "", "Unsupported"),
            ("create was cancelled by user", "pulling", ""),
            // Nothing named: not known to pass.
            ("something nobody has seen", "preparing", "Runtime"),
            ("boom", "", ""),
            ("timed out: waiting", "", "Timeout"),
        ];
        for (msg, phase, variant) in cases {
            let (kind, sub, retry) = classify(msg, phase, variant);
            assert_eq!(retry, Retry::Never, "{msg} ({kind}, {sub:?})");
        }
    }

    #[test]
    fn local_network_kinds_keep_their_subclass_and_never_retry() {
        for (kind, sub) in [
            ("local_network_denied", Subclass::LocalNetworkDenied),
            ("local_network_vpn", Subclass::LocalNetworkVpn),
        ] {
            assert_eq!(subclass(kind, ""), sub);
            assert_eq!(decide(kind, sub), Retry::Never);
        }
        assert_eq!(subclass("local_network", "whatever"), Subclass::None);
    }

    #[test]
    fn enum_cases_are_read_in_snake_case() {
        assert_eq!(snake("InsufficientDisk"), "insufficient_disk");
        assert_eq!(snake("insufficient_disk"), "insufficient_disk");
        assert_eq!(snake("Runtime"), "runtime");
        assert_eq!(decide("InsufficientDisk", Subclass::None), Retry::Never);
        assert_eq!(decide("VmBootTimeout", Subclass::None), Retry::Once);
    }
}
