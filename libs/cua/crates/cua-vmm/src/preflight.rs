//! Preflight for a local create: gathers what the machine has and hands it
//! to [`cua_create_failure::preflight::evaluate`], which words the result.
//!
//! Read-only and quick (a free-space call, a socket connect, a file
//! check, and on a Mac one connect to a running macOS VM). Nothing here
//! installs, starts or changes anything.

use std::collections::BTreeMap;

use cua_create_failure::preflight::{
    self as pf, Engine, Facts, Guest, Host, Issue, Request, Runs, VirtGap,
};

use crate::disk::{CONTAINER_PULL_ESTIMATE, GIB, LUME_PULL_ESTIMATE, VM_CREATE_ESTIMATE};
use crate::types::GuestOs;

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
    let gap = if req.runs == Runs::Vm {
        virtualization(req.host).await
    } else {
        None
    };
    gather_with_virtualization(req, gap).await
}

async fn gather_with_virtualization(req: &Request, gap: Option<VirtGap>) -> Facts {
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
    }
    if req.host == Host::Macos && req.guest == Guest::Macos {
        facts.local_network_blocked = local_network_blocked().await;
    }
    facts
}

/// The failed checks for creating `req` here, most important first. Empty:
/// go ahead.
pub async fn check(req: &Request) -> Vec<Issue> {
    pf::evaluate(req, &gather(req).await)
}

/// [`check`] for the Spaces a shell can offer, by name (`linux_container`,
/// `linux_vm`, `macos_vm`, `windows_vm`), with the image sizes estimated.
/// Hosts that cannot run a Space (a macOS guest off a Mac) are left out.
/// The daemon puts this in its runtime doctor report under `preflight`, so
/// every app and the CLI read it from one place.
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
        let mut facts = gather_with_virtualization(&req, gap).await;
        facts.free_bytes = None;
        out.insert(name.to_string(), pf::evaluate(&req, &facts));
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
        Host::Macos => None,
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

async fn local_network_blocked() -> bool {
    #[cfg(feature = "lume")]
    {
        matches!(
            crate::local_network::probe(&crate::local_network::lume_base()).await,
            crate::local_network::Probe::Blocked { .. }
        )
    }
    #[cfg(not(feature = "lume"))]
    {
        false
    }
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
            let mut container = req.clone();
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

    #[tokio::test]
    async fn the_report_names_every_space_this_host_can_run() {
        let r = report().await;
        assert!(r.contains_key("linux_container") && r.contains_key("windows_vm"));
        assert_eq!(r.contains_key("macos_vm"), cfg!(target_os = "macos"));
    }
}
