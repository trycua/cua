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
    /// This machine had no room, and Cua VMs it runs hold the capacity (a
    /// refusal that names them, so the person can stop them). A VM left
    /// running by an earlier attempt shows up here.
    CapacityVms,
    /// This machine had no room while no Cua VM runs: other programs use
    /// its memory or CPUs, or its limits are set too tight.
    CapacityOther,
    /// The built-in runtime (Lume) could not be set up: its download, its
    /// checksum or unpacking failed. A new, never-retried cause: until its
    /// rate is known a retry only repeats the download.
    RuntimeInstallFailed,
    /// The built-in runtime's signature did not verify.
    RuntimeNotSigned,
    /// `lume serve` started and never answered.
    RuntimeUnresponsive,
    /// This Mac's own Lume is older than macOS Spaces need.
    RuntimeTooOld,
    /// The runtime's API answered with an error (or not at all) outside a
    /// download.
    RuntimeApiError,
    /// A VM was created and never started, stopped while booting, or
    /// vanished while booting.
    VmNeverStarted,
    /// The VM's configuration was refused (`invalid request: ...`).
    VmConfigInvalid,
    /// A host command (`lume`, `codesign`, ...) exited with an error.
    HostCommandFailed,
}

/// Every [`Subclass`] word, in order (`cua_telemetry::schema::ERROR_SUBCLASSES`).
pub const SUBCLASSES: &[&str] = &[
    "none",
    "runtime_start",
    "daemon_unresponsive",
    "pull_interrupted",
    "local_network_denied",
    "local_network_vpn",
    "capacity_vms",
    "capacity_other",
    "runtime_install_failed",
    "runtime_not_signed",
    "runtime_unresponsive",
    "runtime_too_old",
    "runtime_api_error",
    "vm_never_started",
    "vm_config_invalid",
    "host_command_failed",
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
            Self::CapacityVms => "capacity_vms",
            Self::CapacityOther => "capacity_other",
            Self::RuntimeInstallFailed => "runtime_install_failed",
            Self::RuntimeNotSigned => "runtime_not_signed",
            Self::RuntimeUnresponsive => "runtime_unresponsive",
            Self::RuntimeTooOld => "runtime_too_old",
            Self::RuntimeApiError => "runtime_api_error",
            Self::VmNeverStarted => "vm_never_started",
            Self::VmConfigInvalid => "vm_config_invalid",
            Self::HostCommandFailed => "host_command_failed",
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
            "capacity_vms" => Self::CapacityVms,
            "capacity_other" => Self::CapacityOther,
            "runtime_install_failed" => Self::RuntimeInstallFailed,
            "runtime_not_signed" => Self::RuntimeNotSigned,
            "runtime_unresponsive" => Self::RuntimeUnresponsive,
            "runtime_too_old" => Self::RuntimeTooOld,
            "runtime_api_error" => Self::RuntimeApiError,
            "vm_never_started" => Self::VmNeverStarted,
            "vm_config_invalid" => Self::VmConfigInvalid,
            "host_command_failed" => Self::HostCommandFailed,
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
    "vm_runtime_missing",
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
    // No room on this machine: told apart by whether the refusal names Cua
    // VMs holding it (`cua_vmm::density::holders_note`). Both are lasting.
    if kind == "host_capacity" {
        if message.contains("Running now:") {
            return Subclass::CapacityVms;
        }
        if message.contains("No Cua VM is running") {
            return Subclass::CapacityOther;
        }
        return Subclass::None;
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
    // Causes that used to land in a bare `runtime`: the fixed sentences of
    // `cua_vmm::VmmError` and the built-in runtime's setup (the typed error
    // crosses the daemon and the SDK as its text, so this reads those
    // sentences, never free text, and only a word comes out).
    if has(&["is not validly signed", "is not signed by cua ai"]) {
        return Subclass::RuntimeNotSigned;
    }
    if has(&[
        "download the built-in lume",
        "built-in lume download",
        "the built-in lume did not install",
    ]) {
        return Subclass::RuntimeInstallFailed;
    }
    if has(&["started `lume serve` but"]) {
        return Subclass::RuntimeUnresponsive;
    }
    if has(&["macos spaces need lume", "older than the"]) && has(&["lume"]) {
        return Subclass::RuntimeTooOld;
    }
    if has(&["while booting", "never started ("]) && has(&["lume vm '"]) {
        return Subclass::VmNeverStarted;
    }
    if has(&["invalid request: "]) {
        return Subclass::VmConfigInvalid;
    }
    if has(&["lume api ", "lume request failed", "lume get "]) {
        return Subclass::RuntimeApiError;
    }
    if has(&["` failed (exit "]) {
        return Subclass::HostCommandFailed;
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
    fn no_room_says_whether_cua_vms_hold_the_capacity() {
        let held = "this host is at its VM memory limit: its VMs use 5.0 GiB. Running now: a (4.0 GiB, 2 vCPUs), b (1.0 GiB, 2 vCPUs). Stop or delete the ones you do not need";
        let (kind, sub, retry) = classify(held, "booting", "HostCapacity");
        assert_eq!(kind, "host_capacity");
        assert_eq!(sub, Subclass::CapacityVms);
        assert_eq!(retry, Retry::Never);
        let other = "this host has 1.0 GiB of memory available. No Cua VM is running on this machine, so other programs are using its memory or CPUs";
        let (kind, sub, retry) = classify(other, "booting", "HostCapacity");
        assert_eq!(kind, "host_capacity");
        assert_eq!(sub, Subclass::CapacityOther);
        assert_eq!(retry, Retry::Never);
        // Other refusals (file table, disk) say nothing finer.
        let (_, sub, _) = classify("out of open-file capacity", "booting", "HostCapacity");
        assert_eq!(sub, Subclass::None);
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
    fn unexplained_runtime_failures_get_a_subclass() {
        let cases = [
            (
                "the built-in Lume is not validly signed: `codesign` failed",
                Subclass::RuntimeNotSigned,
            ),
            (
                "the built-in Lume is not signed by Cua AI, Inc.",
                Subclass::RuntimeNotSigned,
            ),
            (
                "download the built-in Lume: error sending request",
                Subclass::RuntimeInstallFailed,
            ),
            (
                "the built-in Lume download did not match its checksum (got ab); try again",
                Subclass::RuntimeInstallFailed,
            ),
            (
                "started `lume serve` but http://127.0.0.1:7777 did not answer within 30s (see x)",
                Subclass::RuntimeUnresponsive,
            ),
            (
                "lume serve at http://x is Lume 0.1, and macOS Spaces need Lume 0.3 or newer",
                Subclass::RuntimeTooOld,
            ),
            (
                "lume VM 'a' never started (90s after `lume run`)",
                Subclass::VmNeverStarted,
            ),
            (
                "lume VM 'a' stopped while booting",
                Subclass::VmNeverStarted,
            ),
            ("invalid request: bad name", Subclass::VmConfigInvalid),
            ("lume API 500: boom", Subclass::RuntimeApiError),
            (
                "lume request failed: connection closed",
                Subclass::RuntimeApiError,
            ),
            (
                "`lume` failed (exit Some(1)): stderr",
                Subclass::HostCommandFailed,
            ),
        ];
        for (msg, want) in cases {
            let (kind, sub, retry) = classify(msg, "preparing", "Runtime");
            assert_eq!(sub, want, "{msg} ({kind})");
            // Not known to pass: no automatic retry.
            assert_eq!(retry, Retry::Never, "{msg}");
        }
        // A lasting cause keeps its kind and gets no subclass.
        assert_eq!(
            classify("permission denied: lume API 500", "preparing", "Runtime").1,
            Subclass::None
        );
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
