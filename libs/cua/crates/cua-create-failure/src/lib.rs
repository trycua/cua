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

/// Enum cases that say little on their own (compared lowercase, without
/// underscores): the error text decides instead.
const GENERIC: &[&str] = &[
    "", "none", "other", "runtime", "env", "sandbox", "internal", "io", "fleet", "relay",
];

/// The kind for a failed create: a cause word when the text or the enum
/// case names one, else the caller's enum case (`Runtime`, `Env`), else
/// `other`. Never empty and never `none`.
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
    match flat.as_str() {
        "cloudcreditexhausted" | "creditexhausted" | "limitexceeded" => return "quota_exceeded",
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
    if l.contains("local network access")
        || (vmnet && has(&l, &["no route to host", "os error 65"]))
    {
        return Some("local_network");
    }
    if l.contains("virtualmachinelimitexceeded")
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
    "local_network",
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
        for k in TEXT_KINDS.iter().chain(&["quota_exceeded", "other"]) {
            assert!(
                cua_telemetry::schema::ERROR_KINDS.contains(k),
                "{k} missing from ERROR_KINDS"
            );
        }
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
            kind("registry: not found: ghcr.io/trycua/windows:11", p, ""),
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
            "local_network"
        );
        assert_eq!(
            kind(
                "connect 192.168.64.9:8000: os error 65",
                "waiting_for_services",
                ""
            ),
            "local_network"
        );
        assert_eq!(
            kind("VZErrorDomain virtualMachineLimitExceeded", "booting", ""),
            "vm_limit"
        );
        assert_eq!(
            kind("sandbox quota reached for this org", "", "Fleet"),
            "quota_exceeded"
        );
        assert_eq!(kind("", "", "cloud_credit_exhausted"), "quota_exceeded");
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
