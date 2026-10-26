//! Why a create failed, as a fixed word: the error text a failed create
//! shows is read here, inside the process, and only a word from
//! `cua_telemetry::schema::ERROR_KINDS` (or the caller's own enum case)
//! comes out. The text itself never leaves.
//!
//! One mapping serves `cua_space_create` (the Spaces app core, also in its
//! wasm build), `cua_sandbox_created` (the SDK) and
//! `cua_host_space_provided` (a host's daemon), so the three events name
//! the same cause the same way. Callers pass the result through
//! `cua_telemetry::events::error_kind`, which keeps only schema words.

pub mod local_network;
pub mod preflight;
pub mod retry;

/// Enum cases that say little on their own (compared lowercase, without
/// underscores): the error text decides instead.
const GENERIC: &[&str] = &[
    "", "none", "other", "runtime", "env", "sandbox", "internal", "io", "fleet", "relay",
];

/// The kind for a failed create: a cause word when the text or the enum
/// case names one, else the caller's enum case (`Runtime`, `Env`), else
/// `other`. Never empty and never `none`.
///
/// Local Network failures come out as their subclass
/// (`local_network_denied`, `local_network_vpn`; [`local_network`]), which
/// `cua_telemetry::events::error_kind` reads as the kind `local_network`
/// and `cua_telemetry::events::error_subclass` as the subclass.
///
/// - `message`: the error's text (read here, never returned).
/// - `phase`: the create phase it failed in (`preparing`, `pulling`,
///   `booting`, `waiting_for_services`, ...), or empty when not known.
/// - `variant`: the error enum's case name (`InsufficientDisk`, `Runtime`)
///   or its snake tag (`insufficient_disk`), or empty.
/// - `local`: the create ran on a machine's own runtime (Docker, Lume,
///   QEMU), so "X is not available" means that runtime is missing. False
///   for a cloud create, where it means the account's provider.
pub fn create_failure_kind<'v>(
    message: &str,
    phase: &str,
    variant: &'v str,
    local: bool,
) -> &'v str {
    let phase = phase.trim();
    let variant = variant.trim();
    let flat = flatten(variant);
    // Cua Cloud has ended: every cloud create is refused, whatever the error
    // case that carries it (`Unsupported`, `CloudEnded`). Its own word keeps a
    // known refusal out of the failures that need a fix.
    if flat == "cloudended" || message.to_ascii_lowercase().contains("cua cloud has ended") {
        return "cloud_ended";
    }
    match flat.as_str() {
        "cloudcreditexhausted" | "creditexhausted" | "limitexceeded" => return "quota_exceeded",
        // The app's own Local Network check (typed: `preflight::IssueKind`).
        "localnetworkdenied" => return local_network::DENIED,
        "localnetworkvpn" | "localnetworkblockedbyvpn" => return local_network::VPN,
        "localnetwork" => {
            return match preflight::IssueKind::of_text(message) {
                Some(preflight::IssueKind::LocalNetworkVpn) => local_network::VPN,
                _ => local_network::DENIED,
            };
        }
        // A specific case from the caller stays; a timeout is refined by
        // the text and the phase.
        f if f != "timeout" && !GENERIC.contains(&f) => return variant,
        _ => {}
    }
    if let Some(kind) = from_text(message, phase, local) {
        return kind;
    }
    match flat.as_str() {
        "timeout" => timeout_in(phase),
        "" | "none" => "other",
        _ => variant,
    }
}

fn flatten(variant: &str) -> String {
    variant
        .trim()
        .chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A timeout, by the phase it happened in.
fn timeout_in(phase: &str) -> &'static str {
    match phase {
        "pulling" => "image_pull_network",
        "booting" | "creating" => "vm_boot_timeout",
        "waiting_for_services" | "connecting" => "services_unreachable",
        _ => "timeout",
    }
}

fn has(l: &str, any: &[&str]) -> bool {
    any.iter().any(|n| l.contains(n))
}

/// The cause the text names, if it names one.
fn from_text(message: &str, phase: &str, local: bool) -> Option<&'static str> {
    let l = message.to_ascii_lowercase();
    if l.trim().is_empty() {
        return None;
    }
    // Cancel first: a cancelled create may carry any earlier text.
    if has(
        &l,
        &[
            "cancelled by user",
            "canceled by user",
            "create was cancelled",
        ],
    ) {
        return Some("cancelled");
    }
    // A preflight sentence (`preflight::evaluate`) a create carried on.
    if let Some(kind) = preflight::IssueKind::of_text(&l) {
        return Some(kind.error_subclass().unwrap_or(kind.error_kind()));
    }
    if has(
        &l,
        &[
            "not enough disk space",
            "insufficient disk",
            "insufficient_disk",
            "no space left on device",
            "disk quota exceeded",
        ],
    ) {
        return Some("insufficient_disk");
    }
    let vmnet = l.contains("192.168.64.");
    // Older daemons' words for missing Local Network access.
    if l.contains("local network access")
        || (vmnet && has(&l, &["no route to host", "os error 65"]))
    {
        return Some(local_network::DENIED);
    }
    if l.contains("virtualmachinelimitexceeded")
        || l.contains("running macos guests reached")
        || (has(&l, &["virtual machine", "vms"])
            && l.contains("limit")
            && has(&l, &["exceed", "maximum", "reached"]))
    {
        return Some("vm_limit");
    }
    if has(
        &l,
        &[
            "quota",
            "credit exhausted",
            "credits exhausted",
            "out of credits",
            "payment required",
            "plan limit",
            "seat limit",
            "402 ",
        ],
    ) {
        return Some("quota_exceeded");
    }
    if has(
        &l,
        &[
            "virtualization is not available",
            "virtualization is not supported",
            "virtualization.framework",
            "vz is not available",
            "hypervisor framework",
            "hypervisor is not available",
            "/dev/kvm",
            "kvm is not available",
            "nested virtualization",
            "vt-x",
            "amd-v",
            "hyper-v is not",
            "whpx",
        ],
    ) {
        return Some("virtualization_unavailable");
    }
    // The local runtime (Docker, Podman, Lume, QEMU).
    if local {
        if l.contains("permission denied") {
            return Some("runtime_permission");
        }
        if l.contains(" is not running: ")
            || has(
                &l,
                &[
                    "cannot connect to the docker daemon",
                    "is the docker daemon running",
                    "docker desktop is not running",
                    "lume is not running",
                ],
            )
        {
            return Some("runtime_not_running");
        }
        if (l.contains(" is not available: ") && !l.contains("network route"))
            || (l.contains("qemu") && l.contains("not found on path"))
        {
            return Some("runtime_missing");
        }
    }
    if let Some(kind) = image_pull(&l, phase) {
        return Some(kind);
    }
    if l.contains("unauthenticated") {
        return Some("unauthenticated");
    }
    // Causes that used to land in `other`: they name the problem but not
    // the runtime, so they apply to every phase.
    if has(
        &l,
        &[
            "not enough memory",
            "out of memory",
            "cannot allocate memory",
            "failed to allocate memory",
        ],
    ) {
        return Some("out_of_memory");
    }
    if has(
        &l,
        &[
            "address already in use",
            "port is already allocated",
            "port is already in use",
            "only one usage of each socket address",
        ],
    ) {
        return Some("port_in_use");
    }
    if has(
        &l,
        &[
            "access is denied",
            "os error 5)",
            "being used by another process",
            "path too long",
            "file name too long",
            "filename too long",
            "read-only file system",
        ],
    ) {
        return Some("file_access");
    }
    if has(
        &l,
        &[
            "could not resolve host",
            "dns error",
            "name or service not known",
            "no such host",
            "temporary failure in name resolution",
            "network is unreachable",
            "no internet",
            "tls handshake",
            "certificate verify failed",
            "proxyconnect",
            "failed to lookup address",
        ],
    ) {
        return Some("network");
    }
    // The Space booted but its service (cua-spacesd, the computer server)
    // did not answer.
    if l.contains("cua-spacesd did not answer within ")
        || (phase == "waiting_for_services"
            && has(
                &l,
                &[
                    "connection refused",
                    "connection reset",
                    "unreachable",
                    "no route to host",
                    "did not answer",
                    "health",
                    "probe",
                ],
            ))
    {
        return Some("services_unreachable");
    }
    let timed_out = has(&l, &["timed out", "timeout", "not ready after", "deadline"]);
    if timed_out {
        return Some(timeout_in(phase));
    }
    None
}

/// An image download failure split by why: the image is not there, the
/// registry refused, the network failed, the registry rate-limited, or it
/// stopped partway.
fn image_pull(l: &str, phase: &str) -> Option<&'static str> {
    let about_pull = phase == "pulling"
        || has(
            l,
            &[
                "container engine: pull ",
                "registry: ",
                "manifest unknown",
                "pull access denied",
                "image download",
                "pull ghcr.io",
                "pull docker.io",
                "lume api 404",
                "error pulling image",
                "failed to pull",
            ],
        );
    if !about_pull {
        return None;
    }
    if has(
        l,
        &["toomanyrequests", "too many requests", "rate limit", " 429"],
    ) {
        return Some("image_pull_rate_limited");
    }
    if has(
        l,
        &[
            "pull access denied",
            "not authorized",
            "unauthorized",
            "authentication required",
            "denied: ",
            " 401",
            " 403",
            "forbidden",
        ],
    ) {
        return Some("image_pull_auth");
    }
    if has(
        l,
        &[
            "manifest unknown",
            "name unknown",
            "registry: not found",
            "not found: manifest",
            "repository does not exist",
            "no such image",
            "lume api 404",
            " 404",
            "no matching manifest",
            "manifest in image index",
        ],
    ) {
        return Some("image_pull_not_found");
    }
    if has(
        l,
        &[
            "dns error",
            "no such host",
            "failed to lookup address",
            "connection refused",
            "connection reset",
            "connection closed",
            "error sending request",
            "network is unreachable",
            "tls handshake",
            "certificate",
            "i/o timeout",
            "timed out",
            "timeout",
            "unexpected eof",
            "broken pipe",
            "temporary failure in name resolution",
        ],
    ) {
        return Some("image_pull_network");
    }
    if l.contains("the image download stopped before it finished") {
        return Some("image_pull");
    }
    // A pull that failed for a reason the text does not name.
    (phase == "pulling"
        || has(
            l,
            &["container engine: pull ", "registry: ", "failed to pull"],
        ))
    .then_some("image_pull")
}

/// Every cause word [`create_failure_kind`] can return from the text or
/// the phase (the rest are the caller's enum cases).
pub const TEXT_KINDS: &[&str] = &[
    "cancelled",
    "insufficient_disk",
    "local_network_denied",
    "local_network_vpn",
    "vm_limit",
    "quota_exceeded",
    "virtualization_unavailable",
    "image_pull_rate_limited",
    "image_pull_auth",
    "image_pull_not_found",
    "image_pull_network",
    "image_pull",
    "runtime_permission",
    "runtime_not_running",
    "runtime_missing",
    "unauthenticated",
    "out_of_memory",
    "port_in_use",
    "file_access",
    "network",
    "services_unreachable",
    "vm_boot_timeout",
    "timeout",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn kind<'a>(message: &'a str, phase: &'a str, variant: &'a str) -> &'a str {
        create_failure_kind(message, phase, variant, true)
    }

    #[test]
    fn every_cause_word_is_in_the_telemetry_schema() {
        for k in TEXT_KINDS
            .iter()
            .chain(&["quota_exceeded", "cloud_ended", "other"])
        {
            // A subclass word is sent as its kind plus the subclass.
            let kind = cua_telemetry::events::error_kind(k);
            assert!(
                kind != "other" || *k == "other",
                "{k} missing from ERROR_KINDS"
            );
            assert!(
                cua_telemetry::schema::ERROR_KINDS.contains(&kind),
                "{k} missing from ERROR_KINDS"
            );
        }
        assert_eq!(
            cua_telemetry::events::error_kind(local_network::DENIED),
            "local_network"
        );
        assert_eq!(
            retry::subclass(local_network::VPN, ""),
            retry::Subclass::LocalNetworkVpn
        );
        assert_eq!(
            retry::subclass(local_network::DENIED, ""),
            retry::Subclass::LocalNetworkDenied
        );
    }

    #[test]
    fn local_network_failures_come_from_the_typed_check_or_the_text() {
        // The app's typed check, by its variant, whatever the text.
        assert_eq!(
            kind("whatever", "preparing", "LocalNetworkDenied"),
            "local_network_denied"
        );
        assert_eq!(
            kind("whatever", "", "local_network_vpn"),
            "local_network_vpn"
        );
        // The daemon's fixed sentences.
        let denied = preflight::local_network_denied("Cua Spaces");
        assert_eq!(
            kind(&denied, "waiting_for_services", "Runtime"),
            "local_network_denied"
        );
        assert_eq!(
            kind(
                preflight::LOCAL_NETWORK_VPN,
                "waiting_for_services",
                "Runtime"
            ),
            "local_network_vpn"
        );
        let wrapped = format!(
            "runtime: Local Network access is not available: {} (the VM at 192.168.64.3:3211: No route to host)",
            preflight::LOCAL_NETWORK_VPN
        );
        assert_eq!(
            kind(&wrapped, "waiting_for_services", "Runtime"),
            "local_network_vpn"
        );
        assert_eq!(kind(&wrapped, "", "LocalNetwork"), "local_network_vpn");
        // Older daemons' text stays missing access.
        assert_eq!(
            kind(
                "cua-spacesd did not answer within 120s (nothing answered at http://192.168.64.234:3211/ \
                 (it may still be starting; retry in a few seconds): gRPC: tcp connect error \
                 (No route to host (os error 65)))",
                "waiting_for_services",
                "Timeout"
            ),
            "local_network_denied"
        );
    }

    #[test]
    fn former_other_texts_get_a_kind() {
        fn k(m: &str) -> &str {
            kind(m, "preparing", "Runtime")
        }
        assert_eq!(k("Cannot allocate memory (os error 12)"), "out_of_memory");
        assert_eq!(k("QEMU: not enough memory for the guest"), "out_of_memory");
        assert_eq!(
            k("bind: address already in use on 127.0.0.1:5900"),
            "port_in_use"
        );
        assert_eq!(
            k("Bind for 0.0.0.0:8080 failed: port is already allocated"),
            "port_in_use"
        );
        assert_eq!(k("Access is denied. (os error 5)"), "file_access");
        assert_eq!(
            k("the file is being used by another process"),
            "file_access"
        );
        assert_eq!(
            k("error sending request: dns error: failed to lookup address information"),
            "network"
        );
        assert_eq!(k("x509: certificate verify failed"), "network");
        // A pull keeps its own, finer kinds.
        assert_eq!(
            kind("dns error: no such host", "pulling", "Runtime"),
            "image_pull_network"
        );
        assert_eq!(k("something nobody has seen"), "Runtime");
        assert_eq!(kind("something nobody has seen", "", ""), "other");
    }

    #[test]
    fn image_pulls_split_by_cause() {
        let p = "pulling";
        assert_eq!(
            kind(
                "container engine: pull ghcr.io/x/y:1: manifest unknown",
                "",
                "Runtime"
            ),
            "image_pull_not_found"
        );
        assert_eq!(
            kind("registry: not found: ghcr.io/example/missing:1", p, ""),
            "image_pull_not_found"
        );
        assert_eq!(
            kind("container engine: pull x: pull access denied for x", "", ""),
            "image_pull_auth"
        );
        assert_eq!(
            kind("registry: Not authorized: ghcr.io/me/private", p, "Other"),
            "image_pull_auth"
        );
        assert_eq!(
            kind(
                "container engine: pull x: toomanyrequests: slow down",
                p,
                ""
            ),
            "image_pull_rate_limited"
        );
        assert_eq!(
            kind(
                "registry: error sending request: dns error: no such host",
                p,
                ""
            ),
            "image_pull_network"
        );
        assert_eq!(kind("operation timed out", p, ""), "image_pull_network");
        assert_eq!(
            kind(
                "The image download stopped before it finished (lume stopped). Try again; it picks up where it stopped.",
                p,
                "Runtime"
            ),
            "image_pull"
        );
        assert_eq!(kind("boom", p, ""), "image_pull");
    }

    #[test]
    fn disk_network_limits_and_quota() {
        assert_eq!(
            kind(
                "not enough disk space to pull x: it needs about 30.0 GB",
                "preparing",
                ""
            ),
            "insufficient_disk"
        );
        assert_eq!(
            kind("write: No space left on device", "pulling", ""),
            "insufficient_disk"
        );
        assert_eq!(
            kind(
                "Local Network access is not available: cannot reach the VM at 192.168.64.3 (No route to host)",
                "waiting_for_services",
                "Runtime"
            ),
            "local_network_denied"
        );
        assert_eq!(
            kind(
                "connect 192.168.64.9:8000: os error 65",
                "waiting_for_services",
                ""
            ),
            "local_network_denied"
        );
        assert_eq!(
            kind("VZErrorDomain virtualMachineLimitExceeded", "booting", ""),
            "vm_limit"
        );
        assert_eq!(
            kind(
                "lume API 409: VM start rejected: host limit of 2 running macOS guests reached",
                "booting",
                ""
            ),
            "vm_limit"
        );
        assert_eq!(
            kind("sandbox quota reached for this org", "", "Fleet"),
            "quota_exceeded"
        );
        assert_eq!(kind("", "", "cloud_credit_exhausted"), "quota_exceeded");
    }

    /// Cua Cloud ended: a cloud create is refused with its own word, not the
    /// generic `unsupported` that hid a known refusal among real failures.
    #[test]
    fn a_cloud_create_after_the_end_of_cua_cloud_is_cloud_ended() {
        let msg = "unsupported: Cua Cloud has ended; use a local sandbox or your own computer. \
                   See https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended";
        assert_eq!(
            create_failure_kind(msg, "", "Unsupported", false),
            "cloud_ended"
        );
        assert_eq!(create_failure_kind(msg, "", "", false), "cloud_ended");
        assert_eq!(
            create_failure_kind("", "", "CloudEnded", false),
            "cloud_ended"
        );
        // Other unsupported errors keep their case.
        assert_eq!(
            create_failure_kind("no such feature", "", "Unsupported", true),
            "Unsupported"
        );
    }

    #[test]
    fn runtime_causes() {
        assert_eq!(
            kind(
                "local runtime: [local] a container engine is not available: no socket",
                "preparing",
                "Runtime"
            ),
            "runtime_missing"
        );
        assert_eq!(
            kind(
                "Docker is not running: start Docker Desktop",
                "preparing",
                "Runtime"
            ),
            "runtime_not_running"
        );
        assert_eq!(
            kind(
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
                "preparing",
                ""
            ),
            "runtime_not_running"
        );
        assert_eq!(
            kind(
                "permission denied on unix:///var/run/docker.sock",
                "preparing",
                ""
            ),
            "runtime_permission"
        );
        assert_eq!(
            kind(
                "QEMU (qemu-system-x86_64, qemu-img) is not available: install it",
                "pulling",
                ""
            ),
            "runtime_missing"
        );
        assert_eq!(
            kind(
                "/dev/kvm is missing, so same-architecture guests run slowly",
                "booting",
                ""
            ),
            "virtualization_unavailable"
        );
        assert_eq!(
            kind(
                "Virtualization is not available on this Mac",
                "preparing",
                ""
            ),
            "virtualization_unavailable"
        );
    }

    #[test]
    fn a_cloud_provider_that_is_not_available_is_not_a_runtime() {
        let k = create_failure_kind(
            "cloud is not available: sign in",
            "",
            "ProviderNotConfigured",
            false,
        );
        assert_eq!(k, "ProviderNotConfigured");
    }

    #[test]
    fn timeouts_split_by_phase() {
        assert_eq!(
            kind(
                "sandbox 'x' was not ready after 300s: no answer",
                "booting",
                ""
            ),
            "vm_boot_timeout"
        );
        assert_eq!(
            kind(
                "cua-spacesd did not answer within 120s",
                "waiting_for_services",
                ""
            ),
            "services_unreachable"
        );
        assert_eq!(
            kind(
                "probe http://10.0.0.2:8000/health: connection refused",
                "waiting_for_services",
                ""
            ),
            "services_unreachable"
        );
        assert_eq!(kind("timed out: waiting", "", "Timeout"), "timeout");
        assert_eq!(kind("", "booting", "Timeout"), "vm_boot_timeout");
    }

    #[test]
    fn enum_cases_and_fallbacks() {
        assert_eq!(
            kind("whatever", "preparing", "InsufficientDisk"),
            "InsufficientDisk"
        );
        assert_eq!(kind("whatever", "", "LimitExceeded"), "quota_exceeded");
        assert_eq!(
            kind("the token was rejected: unauthenticated", "", ""),
            "unauthenticated"
        );
        assert_eq!(
            kind("create was cancelled by user", "pulling", ""),
            "cancelled"
        );
        assert_eq!(kind("boom", "preparing", "Runtime"), "Runtime");
        assert_eq!(kind("boom", "preparing", "Env"), "Env");
        assert_eq!(kind("boom", "preparing", ""), "other");
        assert_eq!(kind("", "", ""), "other");
        // Never `none` on a failure.
        assert_eq!(kind("boom", "", "none"), "other");
    }

    #[test]
    fn the_message_never_comes_out() {
        let secret =
            "pull ghcr.io/me/private: token ghp_abc123 for maya@example.com at /Users/maya";
        let k = kind(secret, "pulling", "");
        assert!(cua_telemetry::schema::ERROR_KINDS.contains(&k));
        assert!(!k.contains("maya"));
    }
}
