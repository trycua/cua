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
        // The parent class stays `runtime`; the sub-kind rides along as
        // `runtime.<sub>` (see [`RUNTIME_KINDS`]).
        "runtime" => runtime_with_kind(&message.to_ascii_lowercase()),
        _ => variant,
    }
}

/// The `runtime.<sub>` word for a `Runtime` error no other rule named.
fn runtime_with_kind(l: &str) -> &'static str {
    let sub = runtime_kind_of_text(l);
    RUNTIME_KINDS
        .iter()
        .find(|(k, _)| *k == sub)
        .map(|(_, composite)| *composite)
        .unwrap_or("runtime.unclassified")
}

/// The sub-kind of a `runtime` failure, from the error text (lowercase).
/// Always one of [`RUNTIME_KINDS`]; `unclassified` when no real error path
/// names it, so a dashboard can still see how much is left.
pub fn runtime_kind_of_text(l: &str) -> &'static str {
    // The built-in Linux runtime (Colima/Lima VM that runs Docker on a Mac).
    if has(l, &["without gvisor", "gvisor", "runsc"]) {
        return "gvisor_unavailable";
    }
    if has(
        l,
        &[
            "download the built-in",
            "built-in lume download",
            "built-in linux runtime download",
            "did not match its checksum",
            "larger than its pin",
            "gave no pull token",
            "the built-in lume is not validly signed",
            "the built-in lume did not install",
            "the built-in linux runtime did not install",
        ],
    ) {
        return "runtime_download_failed";
    }
    if l.contains("built-in linux runtime cannot run from") || l.contains("unix sockets allow") {
        return "runtime_path_too_long";
    }
    if has(
        l,
        &[
            "docker engine does not answer",
            "did not answer ping",
            "did not start within 10 minutes",
            "is stopped (the built-in linux runtime is not running)",
        ],
    ) {
        return "linux_runtime_not_ready";
    }
    if has(l, &["the built-in linux runtime did not start", "colima "]) {
        return "linux_runtime_start_failed";
    }
    if has(
        l,
        &[
            "linux spaces can use at most",
            "give it less memory",
            "asks for ",
            "stop your running linux spaces",
        ],
    ) && l.contains("memory")
    {
        return "memory_limit";
    }
    if l.contains("need a mac with apple silicon") {
        return "unsupported_hardware";
    }
    if has(
        l,
        &[
            "too many open files",
            "os error 24",
            "emfile",
            "open-file limit",
        ],
    ) {
        return "open_file_limit";
    }
    if (l.contains("lume") && has(l, &["older than", "or newer"])) || l.contains("needs lume") {
        return "lume_outdated";
    }
    if l.contains("lume serve") || l.contains("lume request failed") || l.contains("lume api 5") {
        return "lume_unreachable";
    }
    if has(
        l,
        &[
            "stopped while booting",
            "never started (",
            "disappeared while booting",
            "exited while waiting for",
            "qemu started but vm status",
        ],
    ) {
        return "vm_stopped_early";
    }
    if (l.contains("qemu") && l.contains("failed (exit"))
        || l.contains("could not launch")
        || l.starts_with("qmp:")
        || l.contains("qmp: ")
    {
        return "qemu_start_failed";
    }
    if has(
        l,
        &[
            "exec format error",
            "does not match the detected host platform",
        ],
    ) {
        return "image_arch_mismatch";
    }
    if has(
        l,
        &[
            "oci runtime",
            "failed to create task",
            "failed to start container",
            "exited right after start",
        ],
    ) || (l.contains("container '") && l.contains("exited"))
    {
        return "container_start_failed";
    }
    if l.contains("container engine: ") || l.contains("sidecar ") {
        return "container_engine_error";
    }
    "unclassified"
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
        if has(&l, &["permission denied", "operation not permitted"]) {
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
                    // Docker Desktop on Windows: its named pipe is not there.
                    "pipe/docker_engine",
                    "pipe\\docker_engine",
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

/// Every `runtime_kind` word: the parent class `runtime` split by the real
/// error path behind it, as `(sub-kind, "runtime.<sub-kind>")`. The second
/// form is what [`create_failure_kind`] returns; `cua-telemetry` reads it
/// back into `error_kind=runtime` plus `runtime_kind=<sub-kind>`.
pub const RUNTIME_KINDS: &[(&str, &str)] = &[
    (
        "linux_runtime_start_failed",
        "runtime.linux_runtime_start_failed",
    ),
    ("linux_runtime_not_ready", "runtime.linux_runtime_not_ready"),
    ("gvisor_unavailable", "runtime.gvisor_unavailable"),
    ("runtime_download_failed", "runtime.runtime_download_failed"),
    ("runtime_path_too_long", "runtime.runtime_path_too_long"),
    ("memory_limit", "runtime.memory_limit"),
    ("unsupported_hardware", "runtime.unsupported_hardware"),
    ("open_file_limit", "runtime.open_file_limit"),
    ("lume_outdated", "runtime.lume_outdated"),
    ("lume_unreachable", "runtime.lume_unreachable"),
    ("vm_stopped_early", "runtime.vm_stopped_early"),
    ("qemu_start_failed", "runtime.qemu_start_failed"),
    ("image_arch_mismatch", "runtime.image_arch_mismatch"),
    ("container_start_failed", "runtime.container_start_failed"),
    ("container_engine_error", "runtime.container_engine_error"),
    ("unclassified", "runtime.unclassified"),
];

/// The sub-kind word in a `runtime.<sub-kind>` kind, if it is one.
pub fn runtime_sub_kind(kind: &str) -> Option<&'static str> {
    RUNTIME_KINDS
        .iter()
        .find(|(_, composite)| *composite == kind)
        .map(|(sub, _)| *sub)
}

/// A plain sentence for a kind (a [`TEXT_KINDS`] word or a `runtime.<sub>`
/// word) that says what happened and what to do next. No jargon, no raw
/// error text. `None` for kinds whose own message is already the best text.
pub fn user_text(kind: &str) -> Option<&'static str> {
    let sub = runtime_sub_kind(kind).unwrap_or(kind);
    Some(match sub {
        "linux_runtime_start_failed" => {
            "The Linux engine that Cua sets up on this Mac did not start. Quit Cua and open it again, then try again. If it keeps happening, open Settings and remove the host setup, then create the Space again."
        }
        "linux_runtime_not_ready" => {
            "The Linux engine started but did not become ready in time. Wait a minute, then try again. If it keeps happening, restart your Mac."
        }
        "gvisor_unavailable" => {
            "The safe sandbox layer for Linux Spaces (gVisor) is not working on this computer. Remove the host setup in Settings and try again."
        }
        "runtime_download_failed" => {
            "A file Cua needs to run Spaces did not download correctly. Check your internet connection, then try again."
        }
        "runtime_path_too_long" => {
            "Cua's folder is in a place with too long a name. Set the CUA_HOME setting to a shorter folder, such as /tmp/cua, then try again."
        }
        "memory_limit" => {
            "This Space asks for more memory than this computer can give it. Pick a smaller memory size, or stop other Linux Spaces first, then try again."
        }
        "unsupported_hardware" => {
            "macOS Spaces need a Mac with an Apple chip (M1 or newer). Create a Linux Space instead."
        }
        "open_file_limit" => {
            "This computer has too many files open at once. Close some apps, then try again. If it keeps happening, restart your computer."
        }
        "lume_outdated" => {
            "The tool that runs macOS Spaces on this Mac is too old. Update it, or choose the built-in one in Settings, then try again."
        }
        "lume_unreachable" => {
            "The tool that runs macOS Spaces on this Mac did not answer. Quit Cua and open it again, then try again."
        }
        "vm_stopped_early" => {
            "The Space's virtual computer stopped while it was starting. Try again. If it keeps happening, restart your computer."
        }
        "qemu_start_failed" => {
            "The virtual computer could not start. Try again. If it keeps happening, restart your computer and check that nothing else is using virtualization."
        }
        "image_arch_mismatch" => {
            "This image was built for a different kind of chip than this computer has. Pick an image that supports your computer."
        }
        "container_start_failed" => {
            "The Space started and then stopped right away. Try again. If it keeps happening, pick a different image."
        }
        "container_engine_error" => {
            "Docker reported a problem while setting up the Space. Make sure Docker is running and up to date, then try again."
        }
        "unclassified" => {
            "Something went wrong while starting the Space on this computer. Try again. If it keeps happening, send a diagnostic report from Settings so we can look into it."
        }
        "runtime_not_running" => {
            "Docker is not running. Open Docker, wait until it says it is running, then try again."
        }
        "runtime_permission" => {
            "Cua is not allowed to use Docker on this computer. Make sure your user can use Docker (on Linux, add yourself to the docker group and sign in again), then try again."
        }
        "port_in_use" => {
            "Another app is already using a port this Space needs. Close the other app, or stop the Space that uses it, then try again."
        }
        _ => return None,
    })
}

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
        assert_eq!(k("something nobody has seen"), "runtime.unclassified");
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
        assert_eq!(kind("boom", "preparing", "Runtime"), "runtime.unclassified");
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

    fn rk(message: &str) -> &'static str {
        // A `Runtime` error no other rule names, any phase.
        let k = kind(message, "preparing", "Runtime");
        runtime_sub_kind(k)
            .unwrap_or_else(|| panic!("{message:?} gave {k}, not a runtime sub-kind"))
    }

    #[test]
    fn runtime_splits_by_real_error_path() {
        // Built-in Linux runtime (Colima) on a Mac.
        assert_eq!(
            rk("the built-in Linux runtime did not start: fatal: vz driver failed"),
            "linux_runtime_start_failed"
        );
        assert_eq!(
            rk("colima start --profile cua: exit status 1"),
            "linux_runtime_start_failed"
        );
        assert_eq!(
            rk("the built-in Linux runtime started but its Docker engine does not answer"),
            "linux_runtime_not_ready"
        );
        assert_eq!(
            rk("the built-in Linux runtime did not start within 10 minutes"),
            "linux_runtime_not_ready"
        );
        assert_eq!(
            rk("container engine at unix:///x.sock did not answer ping: closed"),
            "linux_runtime_not_ready"
        );
        assert_eq!(
            rk(
                "the built-in Linux runtime booted without gVisor (no runsc); remove it and try again"
            ),
            "gvisor_unavailable"
        );
        assert_eq!(
            rk("download the built-in Linux runtime: error sending request"),
            "runtime_download_failed"
        );
        assert_eq!(
            rk("the built-in Lume download did not match its checksum (got abc); try again"),
            "runtime_download_failed"
        );
        assert_eq!(
            rk(
                "the built-in Linux runtime cannot run from /Users/x/long (its path is 120 bytes; Unix sockets allow 103): set CUA_HOME to a shorter directory"
            ),
            "runtime_path_too_long"
        );
        assert_eq!(
            rk(
                "This Linux Space asks for 16 GB of memory, but Linux Spaces can use at most 11 GB on this Mac. Give it less memory."
            ),
            "memory_limit"
        );
        assert_eq!(
            rk("lume: macOS Spaces need a Mac with Apple silicon"),
            "unsupported_hardware"
        );
        assert_eq!(
            rk("accept: Too many open files (os error 24)"),
            "open_file_limit"
        );
        assert_eq!(
            rk(
                "lume serve at http://127.0.0.1:7777 is Lume 0.2, and macOS Spaces need Lume 0.3 or newer is not usable"
            ),
            "lume_outdated"
        );
        assert_eq!(
            rk(
                "started `lume serve` but http://127.0.0.1:7777 did not answer within 30s (see serve.log)"
            ),
            "lume_unreachable"
        );
        assert_eq!(
            rk("lume request failed: connection closed"),
            "lume_unreachable"
        );
        assert_eq!(rk("lume VM 'a' stopped while booting"), "vm_stopped_early");
        assert_eq!(
            rk("lume VM 'a' never started (60s after `lume run`)"),
            "vm_stopped_early"
        );
        assert_eq!(
            rk("qemu for 'a' exited while waiting for Tcp(22) (see qemu.log)"),
            "vm_stopped_early"
        );
        assert_eq!(
            rk("`/usr/bin/qemu-system-x86_64 (see cmdline)` failed (exit None): bad drive"),
            "qemu_start_failed"
        );
        assert_eq!(
            rk("could not launch /usr/bin/qemu: nope"),
            "qemu_start_failed"
        );
        assert_eq!(rk("QMP: connection lost"), "qemu_start_failed");
        assert_eq!(rk("fork/exec: exec format error"), "image_arch_mismatch");
        assert_eq!(
            rk("container 'a' exited (code 1) before Tcp(8000) passed; see `docker logs a`"),
            "container_start_failed"
        );
        assert_eq!(
            rk("container 'a' exited right after start (status Stopped)"),
            "container_start_failed"
        );
        assert_eq!(
            rk("container engine: create container: OCI runtime create failed"),
            "container_start_failed"
        );
        assert_eq!(
            rk("container engine: inspect a: something odd"),
            "container_engine_error"
        );
        assert_eq!(rk("nobody has seen this"), "unclassified");
        assert_eq!(rk(""), "unclassified");
    }

    #[test]
    fn a_runtime_sub_kind_only_applies_to_the_runtime_class() {
        // Another enum case keeps its own word.
        assert_eq!(
            kind("lume VM 'a' stopped while booting", "booting", "Other"),
            "Other"
        );
        assert_eq!(kind("lume VM 'a' stopped while booting", "", ""), "other");
        // Text that names a precise cause still wins.
        assert_eq!(
            kind("write: No space left on device", "preparing", "Runtime"),
            "insufficient_disk"
        );
    }

    #[test]
    fn runtime_sub_kinds_are_consistent() {
        for (sub, composite) in RUNTIME_KINDS {
            assert_eq!(*composite, format!("runtime.{sub}"));
            assert_eq!(runtime_sub_kind(composite), Some(*sub));
            assert!(
                cua_telemetry::schema::RUNTIME_KINDS.contains(sub),
                "{sub} missing from the telemetry schema"
            );
            assert!(user_text(composite).is_some(), "{sub} has no user text");
        }
        assert_eq!(
            RUNTIME_KINDS.len(),
            // `none` is the schema's word for every other error kind.
            cua_telemetry::schema::RUNTIME_KINDS.len() - 1
        );
        assert_eq!(runtime_sub_kind("Runtime"), None);
        assert_eq!(runtime_sub_kind("runtime"), None);
    }

    #[test]
    fn user_text_is_plain() {
        let mut kinds: Vec<&str> = RUNTIME_KINDS.iter().map(|(_, c)| *c).collect();
        kinds.extend(["runtime_not_running", "runtime_permission", "port_in_use"]);
        for k in kinds {
            let t = user_text(k).expect(k);
            assert!(
                !t.contains('\u{2014}') && !t.contains('\u{2013}'),
                "{k}: dash"
            );
            assert!(!t.contains("--"), "{k}");
            for jargon in [
                "hypervisor",
                "daemon",
                "socket",
                "relay",
                "MCP",
                "exit code",
            ] {
                assert!(
                    !t.to_lowercase().contains(&jargon.to_lowercase()),
                    "{k}: {jargon}"
                );
            }
            assert!(t.ends_with('.'));
        }
        assert_eq!(user_text("insufficient_disk"), None);
    }

    #[test]
    fn docker_not_running_and_permission_variants() {
        assert_eq!(
            kind(
                "open //./pipe/docker_engine: The system cannot find the file specified.",
                "preparing",
                "Runtime"
            ),
            "runtime_not_running"
        );
        assert_eq!(
            kind(
                "operation not permitted (os error 1)",
                "preparing",
                "Runtime"
            ),
            "runtime_permission"
        );
    }

    #[test]
    fn a_runtime_sub_kind_never_carries_the_message() {
        let k = kind(
            "container 'maya-laptop' exited (code 1) at /Users/maya/x for maya@example.com",
            "booting",
            "Runtime",
        );
        assert_eq!(k, "runtime.container_start_failed");
    }
}
