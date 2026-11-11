//! Preflight for a local create: gathers what the machine has and hands it
//! to [`cua_create_failure::preflight::evaluate`], which words the result.
//!
//! Read-only and quick (a free-space call, a socket connect, a file
//! check, and on a Mac one connect to a running macOS VM). Nothing here
//! installs, starts or changes anything.

use std::collections::BTreeMap;

use cua_create_failure::preflight::{
    self as pf, Engine, Facts, FixAction, Guest, Host, Issue, IssueKind, Request, Runs, VirtGap,
};

use crate::disk::{CONTAINER_PULL_ESTIMATE, GIB, LUME_PULL_ESTIMATE, VM_CREATE_ESTIMATE};
use crate::types::GuestOs;

/// What a Windows PC without QEMU is told. Cua never installs QEMU on
/// Windows, so a virtual computer cannot start until the person does.
pub const QEMU_MISSING_WINDOWS: &str = "Virtual computers on Windows need a free program called \
    QEMU, and it isn't installed on this PC. Install it from qemu.org (the Windows download), \
    then try again.";

/// Where the QEMU download for Windows is.
pub const QEMU_WINDOWS_URL: &str = "https://www.qemu.org/download/#windows";

/// A Windows VM image and its disk: bigger than a Linux VM's.
pub const WINDOWS_VM_ESTIMATE: u64 = 20 * GIB;

/// Bytes a create writes when the image's size is not known up front.
pub fn estimate(guest: Guest, runs: Runs) -> u64 {
    match (guest, runs) {
        (Guest::Macos, _) => LUME_PULL_ESTIMATE,
        (Guest::Windows, _) => WINDOWS_VM_ESTIMATE,
        (Guest::Linux, Runs::Container) => CONTAINER_PULL_ESTIMATE,
        (Guest::Linux, Runs::Vm) => VM_CREATE_ESTIMATE,
    }
}

/// The guest as the preflight words it.
pub fn guest_of(os: GuestOs) -> Guest {
    match os {
        GuestOs::Linux => Guest::Linux,
        GuestOs::Macos => Guest::Macos,
        GuestOs::Windows => Guest::Windows,
    }
}

/// Whether a missing or stopped container engine stops a Linux container
/// create here. On a Mac the built-in Linux runtime stands in, unless the
/// setting says to use only the Mac's own engine.
pub fn engine_required() -> bool {
    !cfg!(target_os = "macos")
        || crate::managed::LinuxSource::current() == crate::managed::LinuxSource::System
}

/// What is about to be created. `needed_bytes`: the image's size when known,
/// else [`estimate`].
pub fn request(guest: Guest, runs: Runs, needed_bytes: Option<u64>) -> Request {
    Request {
        host: Host::current(),
        guest,
        runs,
        needed_bytes: needed_bytes.unwrap_or_else(|| estimate(guest, runs)),
        min_free_bytes: crate::disk::CacheConfig::load().min_free,
    }
}

/// Finds what `req` needs to know about this machine.
pub async fn gather(req: &Request) -> Facts {
    gather_waiting(req, std::time::Duration::ZERO).await
}

/// [`gather`] for a create that is about to start: a Mac that cannot reach
/// its local network gets `patience` for someone to answer macOS's prompt
/// (the probe's own connect raises it) before it counts as blocked.
pub async fn gather_waiting(req: &Request, patience: std::time::Duration) -> Facts {
    let gap = if req.runs == Runs::Vm {
        virtualization(req.host).await
    } else {
        None
    };
    gather_with_virtualization(req, gap, Some(patience)).await
}

/// `patience`: how long a macOS guest's Local Network check waits for an
/// answer to macOS's prompt; `None` makes no local network connect at all
/// (one would make macOS ask).
async fn gather_with_virtualization(
    req: &Request,
    gap: Option<VirtGap>,
    patience: Option<std::time::Duration>,
) -> Facts {
    let mut facts = Facts {
        free_bytes: crate::disk::space(&crate::host::cua_home())
            .ok()
            .map(|s| s.available),
        ..Default::default()
    };
    if req.runs == Runs::Container && engine_required() {
        facts.engine = engine().await;
    }
    if req.runs == Runs::Vm {
        facts.virtualization = gap;
    } else if req.host == Host::Macos && mac_unsupported() && linux_needs_builtin_runtime().await {
        // A Linux container here runs in the built-in runtime's VM.
        facts.virtualization = Some(VirtGap::MacUnsupported);
    }
    if req.host == Host::Macos
        && req.guest == Guest::Macos
        && let Some(patience) = patience
    {
        facts.local_network_blocked = local_network_blocked(patience).await;
    }
    facts
}

/// The failed checks for creating `req` here, most important first. Empty:
/// go ahead.
pub async fn check(req: &Request) -> Vec<Issue> {
    evaluate(req, &gather(req).await)
}

/// [`pf::evaluate`] plus the checks that need the host's own tools: on a
/// Windows PC, whether QEMU is there to run a virtual computer.
pub fn evaluate(req: &Request, facts: &Facts) -> Vec<Issue> {
    let mut out = pf::evaluate(req, facts);
    if let Some(issue) = qemu_issue(req, qemu_installed) {
        out.insert(0, issue);
    }
    out
}

fn qemu_installed() -> bool {
    let arch = crate::types::Arch::host();
    crate::host::which(&format!("qemu-system-{}", arch.qemu())).is_some()
        && crate::host::which("qemu-img").is_some()
}

/// Windows is the one host Cua never sets QEMU up on, so a VM there needs
/// it installed first. `installed` is only asked when it matters.
fn qemu_issue(req: &Request, installed: impl FnOnce() -> bool) -> Option<Issue> {
    if req.host != Host::Windows || req.runs != Runs::Vm || installed() {
        return None;
    }
    Some(Issue {
        kind: IssueKind::VirtualizationUnavailable,
        message: QEMU_MISSING_WINDOWS.to_string(),
        fix: Some(pf::Fix {
            label: "Get QEMU".into(),
            action: FixAction::OpenUrl(QEMU_WINDOWS_URL.into()),
        }),
    })
}

/// [`check`] for the Spaces a shell can offer, by name (`linux_container`,
/// `linux_vm`, `macos_vm`, `windows_vm`), with the image sizes estimated.
/// Hosts that cannot run a Space (a macOS guest off a Mac) are left out.
/// The daemon puts this in its runtime doctor report under `preflight`, so
/// every app and the CLI read it from one place. Local Network access is
/// not checked here: the app reads this report at launch, and a connect
/// then would make macOS ask before anything explained why. The create of a
/// macOS Space checks it ([`gather_waiting`]).
pub async fn report() -> BTreeMap<String, Vec<Issue>> {
    let mut out = BTreeMap::new();
    let gap = virtualization(Host::current()).await;
    for (name, guest, runs) in [
        ("linux_container", Guest::Linux, Runs::Container),
        ("linux_vm", Guest::Linux, Runs::Vm),
        ("macos_vm", Guest::Macos, Runs::Vm),
        ("windows_vm", Guest::Windows, Runs::Vm),
    ] {
        if guest == Guest::Macos && Host::current() != Host::Macos {
            continue;
        }
        let req = request(guest, runs, None);
        // The image's real size is unknown here and it may already be on
        // disk, so a guess must not block a create that would work: the
        // create's own disk guards decide.
        let mut facts = gather_with_virtualization(&req, gap, None).await;
        facts.free_bytes = None;
        out.insert(name.to_string(), evaluate(&req, &facts));
    }
    out
}

async fn engine() -> Engine {
    #[cfg(feature = "container")]
    {
        use crate::container::engine::{SocketCheck, check_socket, discover, stopped_context};
        let Some(ep) = discover() else {
            return if stopped_context().is_some() {
                Engine::NotRunning
            } else {
                Engine::Missing
            };
        };
        match check_socket(&ep.uri).await {
            Some(SocketCheck::Missing | SocketCheck::NotListening) => Engine::NotRunning,
            Some(SocketCheck::PermissionDenied) => Engine::PermissionDenied,
            Some(SocketCheck::Accepting | SocketCheck::Other(_)) | None => Engine::Ready,
        }
    }
    #[cfg(not(feature = "container"))]
    {
        Engine::NotChecked
    }
}

async fn virtualization(host: Host) -> Option<VirtGap> {
    match host {
        Host::Windows => windows_hypervisor_platform().await,
        Host::Linux => match crate::host::kvm_usable() {
            Ok(()) => None,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Some(VirtGap::KvmPermission)
            }
            Err(_) => Some(VirtGap::KvmMissing),
        },
        Host::Macos => mac_unsupported().then_some(VirtGap::MacUnsupported),
    }
}

/// This Mac cannot run virtual machines (it is one itself, or an Intel Mac
/// without hardware virtualization). A Mac that cannot tell counts as able.
fn mac_unsupported() -> bool {
    crate::host::mac_virtualization_supported() == Some(false)
}

/// Whether a Linux container create here would use the built-in runtime
/// (a VM) rather than a container engine already answering on this Mac.
async fn linux_needs_builtin_runtime() -> bool {
    use crate::managed::LinuxSource;
    match LinuxSource::current() {
        LinuxSource::System => false,
        LinuxSource::Builtin => true,
        LinuxSource::Auto => crate::managed::exists() || engine().await != Engine::Ready,
    }
}

async fn windows_hypervisor_platform() -> Option<VirtGap> {
    if !cfg!(windows) {
        return None;
    }
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$ErrorActionPreference = 'Stop'; Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\" | Select-Object -ExpandProperty InstallState | ConvertTo-Json -Compress",
    ]).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_hypervisor_platform(&String::from_utf8_lossy(&output.stdout))
}

fn parse_hypervisor_platform(stdout: &str) -> Option<VirtGap> {
    (serde_json::from_str::<u32>(stdout.trim()).ok() == Some(2))
        .then_some(VirtGap::HypervisorPlatformOff)
}

/// Whether this process is blocked from the local network the macOS VMs
/// are on. A running VM is the best test; with none (a first create), a
/// connect to the router from this process says the same, and is what makes
/// macOS ask, in the daemon's name, before anything is pulled.
async fn local_network_blocked(patience: std::time::Duration) -> bool {
    use cua_create_failure::local_network::Access;
    #[cfg(test)]
    tests::LOCAL_NETWORK_CHECKS.with(|n| n.set(n.get() + 1));
    #[cfg(feature = "lume")]
    {
        use crate::local_network::Probe;
        if matches!(
            crate::local_network::probe(&crate::local_network::lume_base()).await,
            Probe::Reached { .. }
        ) {
            return false;
        }
    }
    crate::local_network::wait_for_lan_access(patience).await == Access::Blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_feature_state_only_blocks_vms_when_disabled() {
        use pf::{FixAction, IssueKind, Pane};
        let mut req = request(Guest::Windows, Runs::Vm, None);
        req.host = Host::Windows;
        for output in ["1", "2", "3", "4", "null", "", "invalid", "\"2\"", "[2]"] {
            let facts = Facts {
                virtualization: parse_hypervisor_platform(output),
                ..Default::default()
            };
            let issues = pf::evaluate(&req, &facts);
            if output == "2" {
                assert_eq!(issues.len(), 1);
                assert_eq!(issues[0].kind, IssueKind::VirtualizationUnavailable);
                assert_eq!(
                    issues[0].fix.as_ref().unwrap().action,
                    FixAction::OpenSettings(Pane::WindowsFeatures)
                );
                assert!(issues[0].message.contains("restart"));
            } else {
                assert!(issues.is_empty(), "{output}");
            }
            let mut container = req;
            container.guest = Guest::Linux;
            container.runs = Runs::Container;
            assert!(pf::evaluate(&container, &facts).is_empty());
        }
    }

    #[test]
    fn estimates_grow_with_the_guest() {
        assert!(estimate(Guest::Macos, Runs::Vm) > estimate(Guest::Windows, Runs::Vm));
        assert!(estimate(Guest::Windows, Runs::Vm) > estimate(Guest::Linux, Runs::Vm));
        assert!(estimate(Guest::Linux, Runs::Vm) > estimate(Guest::Linux, Runs::Container));
    }

    #[test]
    fn the_request_uses_the_images_size_when_known() {
        let r = request(Guest::Linux, Runs::Container, Some(3 * GIB));
        assert_eq!(r.needed_bytes, 3 * GIB);
        assert_eq!(
            request(Guest::Linux, Runs::Container, None).needed_bytes,
            CONTAINER_PULL_ESTIMATE
        );
    }

    #[test]
    fn a_windows_pc_without_qemu_is_told_before_a_vm_create() {
        let vm = request(Guest::Windows, Runs::Vm, None);
        let on_windows = Request {
            host: Host::Windows,
            ..vm
        };
        let issue = qemu_issue(&on_windows, || false).expect("QEMU is missing");
        assert_eq!(issue.message, QEMU_MISSING_WINDOWS);
        assert_eq!(
            issue.fix.unwrap().action,
            FixAction::OpenUrl(QEMU_WINDOWS_URL.into())
        );
        // QEMU there, a container, or another host: nothing to say, and the
        // lookup is not even made.
        assert!(qemu_issue(&on_windows, || true).is_none());
        let container = Request {
            runs: Runs::Container,
            ..on_windows
        };
        assert!(qemu_issue(&container, || unreachable!()).is_none());
        let on_linux = Request {
            host: Host::Linux,
            ..on_windows
        };
        assert!(qemu_issue(&on_linux, || unreachable!()).is_none());
    }

    #[test]
    fn the_qemu_sentence_is_plain() {
        let text = QEMU_MISSING_WINDOWS.to_lowercase();
        assert!(
            !text.contains('\u{2014}') && !text.contains("hypervisor") && !text.contains("path")
        );
    }

    thread_local! {
        /// `local_network_blocked` calls on this thread.
        pub(super) static LOCAL_NETWORK_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Regression (first run): the app reads the report at launch, and its
    /// connect to the router made macOS show the Local Network prompt before
    /// anything explained it. A macOS create still checks.
    #[tokio::test(flavor = "current_thread")]
    async fn only_a_macos_create_checks_local_network_access() {
        LOCAL_NETWORK_CHECKS.with(|n| n.set(0));
        let r = report().await;
        assert!(r.contains_key("macos_vm") || !cfg!(target_os = "macos"));
        assert_eq!(LOCAL_NETWORK_CHECKS.with(|n| n.get()), 0, "the report");
        let mut req = request(Guest::Macos, Runs::Vm, None);
        req.host = Host::Macos;
        gather_waiting(&req, std::time::Duration::ZERO).await;
        assert_eq!(LOCAL_NETWORK_CHECKS.with(|n| n.get()), 1, "a create");
    }

    #[tokio::test]
    async fn the_report_names_every_space_this_host_can_run() {
        let r = report().await;
        assert!(r.contains_key("linux_container") && r.contains_key("windows_vm"));
        assert_eq!(r.contains_key("macos_vm"), cfg!(target_os = "macos"));
    }
}
