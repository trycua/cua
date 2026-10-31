//! Preflight: what to check before a local Space is created, and what to
//! tell the person when a check fails.
//!
//! This is the pure half. A host gathers [`Facts`] about the machine (free
//! disk space, the container engine, macOS Local Network access, hardware
//! virtualization; `cua_vmm::preflight` does it) and [`evaluate`] turns
//! them into [`Issue`]s: one plain sentence a parent would understand and,
//! where one exists, a single [`Fix`] for a button or link. Every shell
//! (the macOS, Windows and Linux apps, the CLI) shows the same words.
//!
//! No dependencies besides serde, so the Spaces app core's wasm build uses
//! it too. The sentences are fixed text on purpose: [`IssueKind::of_text`]
//! recognises them again, so a create that failed with one of them is
//! classified the same way as the preflight that predicted it.

use serde::{Deserialize, Serialize};

const GIB: u64 = 1 << 30;

/// The machine the Space is created on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Macos,
    Windows,
    Linux,
}

impl Host {
    /// This machine.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// How a sentence names the machine.
    fn subject(self) -> &'static str {
        match self {
            Self::Macos => "Your Mac",
            Self::Windows => "Your PC",
            Self::Linux => "Your computer",
        }
    }
}

/// The operating system of the Space to create.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Guest {
    Linux,
    Macos,
    Windows,
}

/// How the Space runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runs {
    /// A container, on a container engine (Docker).
    Container,
    /// A virtual machine.
    Vm,
}

/// What is about to be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub host: Host,
    pub guest: Guest,
    pub runs: Runs,
    /// Bytes the create writes (the image, the VM's disk).
    pub needed_bytes: u64,
    /// Bytes cua keeps free on the volume besides that.
    pub min_free_bytes: u64,
}

/// The container engine, as found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Answering.
    Ready,
    /// Not installed.
    Missing,
    /// Installed, not running.
    NotRunning,
    /// This user may not use it.
    PermissionDenied,
    /// Not checked (not needed, or this build cannot).
    #[default]
    NotChecked,
}

/// What stops this machine from running virtual machines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtGap {
    /// Windows: the Windows Hypervisor Platform feature is off.
    HypervisorPlatformOff,
    /// Linux: no `/dev/kvm` (virtualization off in the firmware, or none).
    KvmMissing,
    /// Linux: `/dev/kvm` exists but this user may not open it.
    KvmPermission,
}

/// What the host found, `None` / `NotChecked` for what it did not check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// Free bytes on the volume the Space is written to.
    pub free_bytes: Option<u64>,
    pub engine: Engine,
    /// macOS Local Network privacy blocked a connect to a running VM.
    pub local_network_blocked: bool,
    /// Local Network access is on, but a VPN or firewall keeps this Mac
    /// from its own VM network ([`crate::local_network`]).
    #[serde(default)]
    pub local_network_vpn: bool,
    pub virtualization: Option<VirtGap>,
}

/// What a failed check is about. Also the error kind a create that fails
/// the same way records ([`IssueKind::error_kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    InsufficientDisk,
    EngineMissing,
    EngineNotRunning,
    EnginePermission,
    /// macOS Local Network access is off (or not yet allowed) for the app.
    LocalNetwork,
    /// Local Network access is on, but a VPN or firewall blocks this Mac's
    /// own VM network.
    LocalNetworkVpn,
    VirtualizationUnavailable,
}

impl IssueKind {
    /// The telemetry word (`cua_telemetry::schema::ERROR_KINDS`).
    pub fn error_kind(self) -> &'static str {
        match self {
            Self::InsufficientDisk => "insufficient_disk",
            Self::EngineMissing => "runtime_missing",
            Self::EngineNotRunning => "runtime_not_running",
            Self::EnginePermission => "runtime_permission",
            Self::LocalNetwork | Self::LocalNetworkVpn => "local_network",
            Self::VirtualizationUnavailable => "virtualization_unavailable",
        }
    }

    /// The finer telemetry word under [`Self::error_kind`]
    /// (`cua_telemetry::schema::ERROR_SUBCLASSES`), where there is one.
    pub fn error_subclass(self) -> Option<&'static str> {
        match self {
            Self::LocalNetwork => Some(crate::local_network::DENIED),
            Self::LocalNetworkVpn => Some(crate::local_network::VPN),
            _ => None,
        }
    }

    /// The kind whose preflight sentence `text` is (compared lowercase), if
    /// it is one.
    pub fn of_text(text: &str) -> Option<Self> {
        let l = text.to_ascii_lowercase();
        let has = |s: &str| l.contains(s);
        if has("free to create this computer") {
            Some(Self::InsufficientDisk)
        } else if has("to run linux computers.") {
            Some(Self::EngineMissing)
        } else if has("isn't running. ") && has("docker") {
            Some(Self::EngineNotRunning)
        } else if has("isn't allowed to use docker") {
            Some(Self::EnginePermission)
        } else if has(VPN_MARK) {
            Some(Self::LocalNetworkVpn)
        } else if has(DENIED_MARK) || has("find devices on your network") {
            Some(Self::LocalNetwork)
        } else if has("hypervisor platform")
            || has("virtualization is turned off")
            || has("isn't allowed to use this computer's virtualization")
        {
            Some(Self::VirtualizationUnavailable)
        } else {
            None
        }
    }
}

/// What [`IssueKind::of_text`] finds in [`local_network_denied`]'s sentence
/// (lowercase).
const DENIED_MARK: &str = "privacy & security \u{203a} local network";
/// What [`IssueKind::of_text`] finds in [`LOCAL_NETWORK_VPN`] (lowercase).
const VPN_MARK: &str = "a vpn or firewall is blocking this mac's own network";

/// The sentence for a Mac where `app` may not use the local network, so it
/// cannot reach the macOS Spaces on this Mac: what to allow, and where.
pub fn local_network_denied(app: &str) -> String {
    format!(
        "{app} can't connect to macOS Spaces on this Mac. Allow {app} in System Settings \u{203a} \
         Privacy & Security \u{203a} Local Network, then try again."
    )
}

/// The sentence for a Mac whose VPN or firewall blocks its own VM network
/// while Local Network access is on.
pub const LOCAL_NETWORK_VPN: &str = "A VPN or firewall is blocking this Mac's own network, so \
     Cua Spaces can't connect to macOS Spaces. Turn off the VPN and try again. If you can't turn \
     it off, ask whoever manages it (often your IT team) to let 192.168.64.0/24, the network \
     macOS Spaces use on this Mac, skip the VPN.";

/// A System Settings page or Windows dialog a fix opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    /// macOS: Privacy & Security > Local Network.
    LocalNetwork,
    /// Windows: Turn Windows features on or off.
    WindowsFeatures,
}

impl Pane {
    /// The URL that opens it, where there is one (`NSWorkspace.open`).
    pub fn url(self) -> Option<&'static str> {
        match self {
            Self::LocalNetwork => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")
            }
            Self::WindowsFeatures => None,
        }
    }
}

/// What the fix button does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FixAction {
    /// Open this web page.
    OpenUrl(String),
    /// Open this settings page.
    OpenSettings(Pane),
    /// Run this command (the CLI prints it; an app runs it for the user).
    RunCommand(String),
}

/// One button or link that fixes the problem.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub label: String,
    pub action: FixAction,
}

/// One failed check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub kind: IssueKind,
    /// One plain sentence or two: what is wrong and what to do.
    pub message: String,
    /// The single fix, where one is possible.
    pub fix: Option<Fix>,
}

impl Issue {
    fn new(kind: IssueKind, message: impl Into<String>, fix: Option<Fix>) -> Self {
        Self {
            kind,
            message: message.into(),
            fix,
        }
    }
}

fn fix(label: &str, action: FixAction) -> Option<Fix> {
    Some(Fix {
        label: label.into(),
        action,
    })
}

/// The issue for Local Network access that is off for `app`, with the
/// button that opens its settings.
pub fn local_network_issue(app: &str) -> Issue {
    Issue::new(
        IssueKind::LocalNetwork,
        local_network_denied(app),
        fix("Open Settings", FixAction::OpenSettings(Pane::LocalNetwork)),
    )
}

/// Whole gigabytes, rounded up, for a sentence.
fn gb(bytes: u64) -> u64 {
    bytes.div_ceil(GIB).max(1)
}

/// The checks that fail, most important first. Empty: go ahead.
pub fn evaluate(req: &Request, facts: &Facts) -> Vec<Issue> {
    let mut out = Vec::new();

    if let Some(free) = facts.free_bytes {
        let need = req.needed_bytes.saturating_add(req.min_free_bytes);
        if free < need {
            out.push(Issue::new(
                IssueKind::InsufficientDisk,
                format!(
                    "{} needs about {} GB free to create this computer. Free up space and try \
                     again.",
                    req.host.subject(),
                    gb(need)
                ),
                fix(
                    "Free up space",
                    FixAction::RunCommand("cua cache prune".into()),
                ),
            ));
        }
    }

    if req.runs == Runs::Container {
        let desktop = req.host != Host::Linux;
        match facts.engine {
            Engine::Missing if desktop => out.push(Issue::new(
                IssueKind::EngineMissing,
                "Install Docker Desktop to run Linux computers.",
                fix(
                    "Get Docker",
                    FixAction::OpenUrl("https://www.docker.com/products/docker-desktop/".into()),
                ),
            )),
            Engine::Missing => out.push(Issue::new(
                IssueKind::EngineMissing,
                "Install Docker to run Linux computers.",
                fix(
                    "Get Docker",
                    FixAction::OpenUrl("https://docs.docker.com/engine/install/".into()),
                ),
            )),
            Engine::NotRunning if desktop => out.push(Issue::new(
                IssueKind::EngineNotRunning,
                "Docker Desktop isn't running. Open it, wait until it says it's running, then \
                 try again.",
                None,
            )),
            Engine::NotRunning => out.push(Issue::new(
                IssueKind::EngineNotRunning,
                "Docker isn't running. Start it, then try again.",
                None,
            )),
            Engine::PermissionDenied => out.push(Issue::new(
                IssueKind::EnginePermission,
                "Cua isn't allowed to use Docker on this computer. Give your account access to \
                 Docker, then try again.",
                fix(
                    "How to fix",
                    FixAction::OpenUrl(
                        "https://docs.docker.com/engine/install/linux-postinstall/".into(),
                    ),
                ),
            )),
            Engine::Ready | Engine::NotChecked => {}
        }
    }

    if req.host == Host::Macos && req.guest == Guest::Macos {
        if facts.local_network_vpn {
            out.push(Issue::new(
                IssueKind::LocalNetworkVpn,
                LOCAL_NETWORK_VPN,
                None,
            ));
        } else if facts.local_network_blocked {
            out.push(local_network_issue("Cua Spaces"));
        }
    }

    if req.runs == Runs::Vm
        && let Some(gap) = facts.virtualization
    {
        match (gap, req.host, req.guest) {
            (VirtGap::HypervisorPlatformOff, Host::Windows, _) => out.push(Issue::new(
                IssueKind::VirtualizationUnavailable,
                "Turn on the Windows Hypervisor Platform feature so this PC can run virtual \
                 computers, then restart it.",
                fix(
                    "Open Windows Features",
                    FixAction::OpenSettings(Pane::WindowsFeatures),
                ),
            )),
            // Without KVM a Linux guest still runs, slowly; Windows does not.
            (VirtGap::KvmMissing, Host::Linux, Guest::Windows) => out.push(Issue::new(
                IssueKind::VirtualizationUnavailable,
                "Virtualization is turned off on this computer, so it can't run Windows \
                 computers. Turn it on in the computer's BIOS settings.",
                None,
            )),
            // Like a missing KVM, a Linux guest still starts without it.
            (VirtGap::KvmPermission, Host::Linux, Guest::Windows) => out.push(Issue::new(
                IssueKind::VirtualizationUnavailable,
                "Cua isn't allowed to use this computer's virtualization. Add your account to \
                 the kvm group, then log out and back in.",
                None,
            )),
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(host: Host, guest: Guest, runs: Runs) -> Request {
        Request {
            host,
            guest,
            runs,
            needed_bytes: 7 * GIB,
            min_free_bytes: 5 * GIB,
        }
    }

    fn first(req: &Request, facts: &Facts) -> Issue {
        evaluate(req, facts).remove(0)
    }

    #[test]
    fn a_healthy_machine_has_no_issues() {
        let facts = Facts {
            free_bytes: Some(100 * GIB),
            engine: Engine::Ready,
            ..Default::default()
        };
        for r in [
            req(Host::Macos, Guest::Linux, Runs::Container),
            req(Host::Windows, Guest::Windows, Runs::Vm),
            req(Host::Linux, Guest::Linux, Runs::Container),
        ] {
            assert!(evaluate(&r, &facts).is_empty());
        }
        assert!(evaluate(&req(Host::Linux, Guest::Linux, Runs::Vm), &Facts::default()).is_empty());
    }

    #[test]
    fn a_full_disk_says_how_much_is_needed() {
        let facts = Facts {
            free_bytes: Some(3 * GIB),
            ..Default::default()
        };
        let i = first(&req(Host::Macos, Guest::Macos, Runs::Vm), &facts);
        assert_eq!(i.kind, IssueKind::InsufficientDisk);
        assert_eq!(
            i.message,
            "Your Mac needs about 12 GB free to create this computer. Free up space and try again."
        );
        assert_eq!(i.fix.unwrap().label, "Free up space");
        let w = first(&req(Host::Windows, Guest::Windows, Runs::Vm), &facts);
        assert!(w.message.starts_with("Your PC needs about 12 GB"));
        // Exactly enough is enough; an unknown size never blocks.
        let ok = Facts {
            free_bytes: Some(12 * GIB),
            ..Default::default()
        };
        assert!(evaluate(&req(Host::Macos, Guest::Macos, Runs::Vm), &ok).is_empty());
        assert!(evaluate(&req(Host::Macos, Guest::Macos, Runs::Vm), &Facts::default()).is_empty());
    }

    #[test]
    fn the_container_engine_has_three_ways_to_fail() {
        let r = req(Host::Macos, Guest::Linux, Runs::Container);
        let with = |engine| Facts {
            engine,
            ..Default::default()
        };
        let missing = first(&r, &with(Engine::Missing));
        assert_eq!(
            missing.message,
            "Install Docker Desktop to run Linux computers."
        );
        assert_eq!(
            missing.fix,
            Some(Fix {
                label: "Get Docker".into(),
                action: FixAction::OpenUrl(
                    "https://www.docker.com/products/docker-desktop/".into()
                )
            })
        );
        let linux = first(
            &req(Host::Linux, Guest::Linux, Runs::Container),
            &with(Engine::Missing),
        );
        assert_eq!(linux.message, "Install Docker to run Linux computers.");
        let down = first(&r, &with(Engine::NotRunning));
        assert_eq!(down.kind, IssueKind::EngineNotRunning);
        assert!(down.fix.is_none());
        let denied = first(&r, &with(Engine::PermissionDenied));
        assert_eq!(denied.kind, IssueKind::EnginePermission);
        // A VM does not need an engine.
        let vm = req(Host::Macos, Guest::Linux, Runs::Vm);
        assert!(evaluate(&vm, &with(Engine::Missing)).is_empty());
    }

    #[test]
    fn local_network_only_matters_for_a_macos_guest_on_a_mac() {
        let facts = Facts {
            local_network_blocked: true,
            ..Default::default()
        };
        let i = first(&req(Host::Macos, Guest::Macos, Runs::Vm), &facts);
        assert_eq!(i.kind, IssueKind::LocalNetwork);
        assert_eq!(
            i.fix.unwrap().action,
            FixAction::OpenSettings(Pane::LocalNetwork)
        );
        assert!(evaluate(&req(Host::Macos, Guest::Linux, Runs::Container), &facts).is_empty());
        assert!(evaluate(&req(Host::Windows, Guest::Windows, Runs::Vm), &facts).is_empty());
    }

    #[test]
    fn local_network_says_what_to_allow_and_where() {
        let i = local_network_issue("Cua Spaces");
        assert_eq!(
            i.message,
            "Cua Spaces can't connect to macOS Spaces on this Mac. Allow Cua Spaces in System \
             Settings \u{203a} Privacy & Security \u{203a} Local Network, then try again."
        );
        assert_eq!(i.fix.as_ref().unwrap().label, "Open Settings");
        assert_eq!(
            Pane::LocalNetwork.url(),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")
        );
        assert_eq!(IssueKind::LocalNetwork.error_kind(), "local_network");
        assert_eq!(
            IssueKind::LocalNetwork.error_subclass(),
            Some("local_network_denied")
        );
        // Another app's name is recognised the same way.
        assert_eq!(
            IssueKind::of_text(&local_network_denied("Cua Spaces Dev")),
            Some(IssueKind::LocalNetwork)
        );
    }

    #[test]
    fn a_vpn_is_told_apart_from_missing_permission() {
        let facts = Facts {
            local_network_blocked: true,
            local_network_vpn: true,
            ..Default::default()
        };
        let i = first(&req(Host::Macos, Guest::Macos, Runs::Vm), &facts);
        assert_eq!(i.kind, IssueKind::LocalNetworkVpn);
        assert!(i.message.contains("Turn off the VPN"), "{}", i.message);
        // A work VPN often can't be turned off: say what to ask for instead.
        assert!(i.message.contains("192.168.64.0/24"), "{}", i.message);
        assert!(i.fix.is_none());
        assert_eq!(i.kind.error_kind(), "local_network");
        assert_eq!(i.kind.error_subclass(), Some("local_network_vpn"));
        // Only one of the two is shown.
        assert_eq!(
            evaluate(&req(Host::Macos, Guest::Macos, Runs::Vm), &facts).len(),
            1
        );
        // A Facts from an older daemon (no `local_network_vpn`) still reads.
        let old: Facts = serde_json::from_str(
            r#"{"free_bytes":null,"engine":"not_checked","local_network_blocked":true,"virtualization":null}"#,
        )
        .unwrap();
        assert!(old.local_network_blocked && !old.local_network_vpn);
    }

    #[test]
    fn virtualization_prerequisites_by_host() {
        let gap = |g| Facts {
            virtualization: Some(g),
            ..Default::default()
        };
        let win = first(
            &req(Host::Windows, Guest::Windows, Runs::Vm),
            &gap(VirtGap::HypervisorPlatformOff),
        );
        assert_eq!(win.kind, IssueKind::VirtualizationUnavailable);
        assert_eq!(
            win.fix.unwrap().action,
            FixAction::OpenSettings(Pane::WindowsFeatures)
        );
        let linux_win = req(Host::Linux, Guest::Windows, Runs::Vm);
        assert!(first(&linux_win, &gap(VirtGap::KvmMissing)).fix.is_none());
        // A Linux guest without KVM still runs (slowly).
        let linux_linux = req(Host::Linux, Guest::Linux, Runs::Vm);
        assert!(evaluate(&linux_linux, &gap(VirtGap::KvmMissing)).is_empty());
        assert!(evaluate(&linux_linux, &gap(VirtGap::KvmPermission)).is_empty());
        assert_eq!(
            first(&linux_win, &gap(VirtGap::KvmPermission)).kind,
            IssueKind::VirtualizationUnavailable
        );
        // Containers do not need it.
        let c = req(Host::Windows, Guest::Linux, Runs::Container);
        assert!(evaluate(&c, &gap(VirtGap::HypervisorPlatformOff)).is_empty());
    }

    #[test]
    fn every_sentence_is_recognised_again() {
        let everything = Facts {
            free_bytes: Some(0),
            engine: Engine::Missing,
            local_network_blocked: true,
            local_network_vpn: false,
            virtualization: Some(VirtGap::HypervisorPlatformOff),
        };
        let mut seen = Vec::new();
        for (host, guest, runs, engine, gap) in [
            (Host::Macos, Guest::Macos, Runs::Vm, Engine::Ready, None),
            (
                Host::Macos,
                Guest::Linux,
                Runs::Container,
                Engine::Missing,
                None,
            ),
            (
                Host::Macos,
                Guest::Linux,
                Runs::Container,
                Engine::NotRunning,
                None,
            ),
            (
                Host::Linux,
                Guest::Linux,
                Runs::Container,
                Engine::NotRunning,
                None,
            ),
            (
                Host::Linux,
                Guest::Linux,
                Runs::Container,
                Engine::Missing,
                None,
            ),
            (
                Host::Linux,
                Guest::Linux,
                Runs::Container,
                Engine::PermissionDenied,
                None,
            ),
            (
                Host::Windows,
                Guest::Windows,
                Runs::Vm,
                Engine::Ready,
                Some(VirtGap::HypervisorPlatformOff),
            ),
            (
                Host::Linux,
                Guest::Windows,
                Runs::Vm,
                Engine::Ready,
                Some(VirtGap::KvmMissing),
            ),
            (
                Host::Linux,
                Guest::Windows,
                Runs::Vm,
                Engine::Ready,
                Some(VirtGap::KvmPermission),
            ),
        ] {
            let facts = Facts {
                engine,
                virtualization: gap,
                ..everything
            };
            for i in evaluate(&req(host, guest, runs), &facts) {
                assert_eq!(
                    IssueKind::of_text(&i.message),
                    Some(i.kind),
                    "{}",
                    i.message
                );
                seen.push(i.kind);
            }
        }
        let vpn = evaluate(
            &req(Host::Macos, Guest::Macos, Runs::Vm),
            &Facts {
                local_network_vpn: true,
                ..Default::default()
            },
        );
        assert_eq!(IssueKind::of_text(&vpn[0].message), Some(vpn[0].kind));
        seen.push(vpn[0].kind);
        for k in [
            IssueKind::LocalNetworkVpn,
            IssueKind::InsufficientDisk,
            IssueKind::EngineMissing,
            IssueKind::EngineNotRunning,
            IssueKind::EnginePermission,
            IssueKind::LocalNetwork,
            IssueKind::VirtualizationUnavailable,
        ] {
            assert!(seen.contains(&k), "{k:?} never produced");
        }
        assert_eq!(IssueKind::of_text("something else"), None);
    }

    #[test]
    fn issues_round_trip_as_json() {
        let i = first(
            &req(Host::Macos, Guest::Linux, Runs::Container),
            &Facts {
                engine: Engine::Missing,
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&i).unwrap();
        assert_eq!(serde_json::from_str::<Issue>(&json).unwrap(), i);
    }
}
