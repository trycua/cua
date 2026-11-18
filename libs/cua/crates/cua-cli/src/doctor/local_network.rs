//! `host.local_network`: whether the cua daemon can reach this Mac's own
//! macOS VMs.
//!
//! A macOS Space is a VM on Apple's shared network (192.168.64.x); the cua
//! daemon talks to it there. macOS blocks that until Local Network access
//! is on for the app macOS holds responsible for the daemon (the Spaces
//! app for the daemon it starts), and then a connect fails at once with
//! "No route to host" (`EHOSTUNREACH`). The grant is per app, so a connect
//! from this CLI would test the terminal's grant, not the daemon's: the
//! daemon probes (`cua_vmm::local_network::report`, in its runtime doctor
//! report) and names the app. A daemon that predates that report is tested
//! from this process instead, and the check says so. It is never required:
//! a blocked VM network warns; no VM to try, or not a Mac, skips with the
//! reason.

use cua_spacesd_client::diagnose::{Check, Severity, Status};
pub use cua_vmm::local_network::{Probe, ResponsibleApp};

const ID: &str = "host.local_network";

/// Where the setting is, as the user reads it in System Settings.
pub const SETTINGS_PATH: &str = "System Settings > Privacy & Security > Local Network";
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

/// Who probed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prober {
    /// The cua daemon (the process that talks to the VMs), as the app
    /// macOS holds responsible for it.
    Daemon(Option<ResponsibleApp>),
    /// This CLI process: the daemon predates the daemon-side probe.
    ThisProcess(Option<ResponsibleApp>),
}

impl Prober {
    fn app(&self) -> Option<&ResponsibleApp> {
        match self {
            Self::Daemon(a) | Self::ThisProcess(a) => a.as_ref(),
        }
    }

    /// "the cua daemon" / "this terminal (Terminal)".
    fn who(&self) -> String {
        match self {
            Self::Daemon(_) => "the cua daemon".into(),
            Self::ThisProcess(Some(a)) => format!("this process ({})", a.name),
            Self::ThisProcess(None) => "this process".into(),
        }
    }

    /// The app to allow, as the user finds it in System Settings.
    fn app_words(&self) -> String {
        match (self, self.app()) {
            (_, Some(a)) if a.bundle => format!("{} ({})", a.name, a.path.display()),
            (Self::Daemon(_), _) => "the app that started the cua daemon".into(),
            (Self::ThisProcess(_), _) => "the app you ran cua from (your terminal app)".into(),
        }
    }
}

/// The check for what this machine is and what the probe found (pure).
/// `macos_vms`: this Mac runs local macOS VMs (lume answers, or it
/// provides Spaces).
pub fn check(os: &str, macos_vms: bool, probe: Option<(Probe, Prober)>) -> Check {
    let skip = |message: String, reason: &str| {
        info(Check::new(ID, Status::Skip, message).skip_reason(reason))
    };
    if os != "macos" {
        return skip(
            "not a Mac: Local Network access is a macOS setting".into(),
            "not_applicable",
        );
    }
    if !macos_vms {
        return skip(
            "this Mac runs no local macOS VMs (lume is not set up), so it needs no Local Network access"
                .into(),
            "not_applicable",
        );
    }
    let generic_allow = format!(
        "open {SETTINGS_PATH} and turn on Cua Spaces (or cua-spacesd), which starts the cua \
         daemon, and the app you ran cua from; then run `cua doctor` again"
    );
    let Some((probe, by)) = probe else {
        return skip(
            "no macOS VM is running, so Local Network access can't be tested; start a macOS Space and run `cua doctor` again".into(),
            "no_running_vm",
        )
        .fix(format!("if macOS Spaces fail with \"No route to host\": {generic_allow}"));
    };
    let app = by.app_words();
    let allow = format!(
        "open {SETTINGS_PATH} and turn on {app}; if it isn't listed, quit and reopen it on \
         this Mac (macOS asks again); then run `cua doctor` again"
    );
    let mut check = match probe {
        Probe::NoRunningVm => skip(
            "no macOS VM is running, so Local Network access can't be tested; start a macOS Space and run `cua doctor` again".into(),
            "no_running_vm",
        )
        .fix(format!("if macOS Spaces fail with \"No route to host\": {allow}")),
        Probe::NoList { why } => skip(
            format!("couldn't list this Mac's VMs ({why})"),
            "no_vm_list",
        ),
        Probe::Reached { vm, ip } => info(Check::new(
            ID,
            Status::Pass,
            match &by {
                Prober::Daemon(_) => format!(
                    "the cua daemon reached the macOS VM {vm} at {ip}: Local Network access is on for {app}"
                ),
                Prober::ThisProcess(_) => format!(
                    "{} reached the macOS VM {vm} at {ip}; the cua daemon is too old to check its own access, \
                     so this does not show that the daemon has Local Network access",
                    by.who()
                ),
            },
        )),
        Probe::Blocked { vm, ip } => info(
            Check::new(
                ID,
                Status::Warn,
                format!(
                    "{} can't reach the macOS VM {vm} at {ip} (No route to host): Local Network access is off \
                     for {app}. macOS Spaces on this Mac fail until it is on",
                    by.who()
                ),
            )
            .fix(format!("{allow} ({SETTINGS_URL})")),
        ),
        Probe::Unclear { vm, ip, why } => skip(
            format!("couldn't tell whether the macOS VM {vm} at {ip} is reachable ({why})"),
            "inconclusive",
        )
        .fix(format!("if macOS Spaces fail with \"No route to host\": {allow}")),
    };
    check.facts.insert(
        "probed_by".into(),
        match by {
            Prober::Daemon(_) => "daemon".into(),
            Prober::ThisProcess(_) => "cli".into(),
        },
    );
    if let Some(a) = by.app() {
        check.facts.insert("responsible_app".into(), a.name.clone());
        check
            .facts
            .insert("responsible_app_path".into(), a.path.display().to_string());
    }
    check
}

fn info(mut c: Check) -> Check {
    c.severity = Severity::Info;
    c
}

/// The daemon's own probe from its runtime doctor report (`report_json`'s
/// `local_network`), when it has one.
pub fn from_daemon(report_json: &serde_json::Value) -> Option<(Probe, Prober)> {
    let r: cua_vmm::local_network::Report =
        serde_json::from_value(report_json.get("local_network")?.clone()).ok()?;
    Some((r.probe, Prober::Daemon(r.app)))
}

/// Probes from this process (a daemon that predates its own probe).
pub async fn from_this_process() -> (Probe, Prober) {
    let probe = cua_vmm::local_network::probe(&lume_base()).await;
    (
        probe,
        Prober::ThisProcess(ResponsibleApp::of_this_process()),
    )
}

/// The `lume serve` the runtime doctor talks to.
pub fn lume_base() -> String {
    cua_vmm::local_network::lume_base()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn app(name: &str) -> ResponsibleApp {
        ResponsibleApp {
            name: name.into(),
            path: PathBuf::from(format!("/Users/x/apps/{name}.app")),
            bundle: true,
        }
    }

    fn daemon(probe: Probe) -> Option<(Probe, Prober)> {
        Some((probe, Prober::Daemon(Some(app("Cua1807 Main")))))
    }

    fn blocked() -> Probe {
        Probe::Blocked {
            vm: "m1".into(),
            ip: "192.168.64.5".into(),
        }
    }

    fn reached() -> Probe {
        Probe::Reached {
            vm: "m1".into(),
            ip: "192.168.64.5".into(),
        }
    }

    #[test]
    fn not_a_mac_or_no_vms_skips_with_the_reason() {
        let c = check("linux", true, daemon(reached()));
        assert_eq!(c.status, Status::Skip);
        assert_eq!(
            c.facts.get("skip_reason").map(String::as_str),
            Some("not_applicable")
        );
        assert!(c.message.contains("not a Mac"), "{}", c.message);
        let c = check("macos", false, None);
        assert_eq!(c.status, Status::Skip);
        assert!(c.message.contains("no local macOS VMs"), "{}", c.message);
        assert_eq!(c.severity, Severity::Info);
    }

    /// CUA-1807: the daemon is blocked while the terminal is allowed. The
    /// daemon's probe decides, and names the app to turn on.
    #[test]
    fn a_blocked_daemon_warns_and_names_its_app() {
        let c = check("macos", true, daemon(blocked()));
        assert_eq!(c.status, Status::Warn);
        assert_eq!(c.severity, Severity::Info, "never required");
        assert!(
            c.message.contains("the cua daemon can't reach"),
            "{}",
            c.message
        );
        assert!(c.message.contains("No route to host"), "{}", c.message);
        assert!(
            c.message
                .contains("Cua1807 Main (/Users/x/apps/Cua1807 Main.app)"),
            "{}",
            c.message
        );
        assert!(
            c.remediation.contains(
                "System Settings > Privacy & Security > Local Network and turn on Cua1807 Main"
            ),
            "{}",
            c.remediation
        );
        assert!(
            c.remediation.contains("Privacy_LocalNetwork"),
            "{}",
            c.remediation
        );
        assert_eq!(c.facts["probed_by"], "daemon");
        assert_eq!(c.facts["responsible_app"], "Cua1807 Main");
    }

    #[test]
    fn a_reached_vm_passes_and_says_for_which_app() {
        let c = check("macos", true, daemon(reached()));
        assert_eq!(c.status, Status::Pass);
        assert!(
            c.message.contains("the cua daemon reached"),
            "{}",
            c.message
        );
        assert!(c.message.contains("on for Cua1807 Main"), "{}", c.message);
        let c = check("macos", true, daemon(Probe::NoRunningVm));
        assert_eq!(c.status, Status::Skip);
        assert_eq!(
            c.facts.get("skip_reason").map(String::as_str),
            Some("no_running_vm")
        );
        assert!(c.remediation.contains("Local Network"), "{}", c.remediation);
        let c = check("macos", true, daemon(Probe::NoList { why: "x".into() }));
        assert_eq!(c.status, Status::Skip);
        assert!(
            check("macos", true, None)
                .remediation
                .contains("Cua Spaces")
        );
    }

    /// An older daemon: the CLI's own pass must not read as the daemon's.
    #[test]
    fn a_pass_from_this_process_says_the_daemon_was_not_checked() {
        let by = Prober::ThisProcess(Some(app("Terminal")));
        let c = check("macos", true, Some((reached(), by.clone())));
        assert_eq!(c.status, Status::Pass);
        assert!(
            c.message.contains("this process (Terminal)"),
            "{}",
            c.message
        );
        assert!(
            c.message.contains("does not show that the daemon"),
            "{}",
            c.message
        );
        assert_eq!(c.facts["probed_by"], "cli");
        let c = check("macos", true, Some((blocked(), by)));
        assert_eq!(c.status, Status::Warn);
        assert!(
            c.remediation.contains("turn on Terminal"),
            "{}",
            c.remediation
        );
    }

    #[test]
    fn a_bare_daemon_names_the_app_that_started_it() {
        let bare = ResponsibleApp {
            name: "cua".into(),
            path: PathBuf::from("/usr/local/bin/cua"),
            bundle: false,
        };
        let c = check("macos", true, Some((blocked(), Prober::Daemon(Some(bare)))));
        assert!(
            c.remediation
                .contains("turn on the app that started the cua daemon"),
            "{}",
            c.remediation
        );
    }

    #[test]
    fn the_daemon_report_is_read_from_its_json() {
        let json = serde_json::json!({
            "backends": [],
            "local_network": {
                "probe": {"result": "blocked", "vm": "m1", "ip": "192.168.64.5"},
                "app": {"name": "Cua Spaces", "path": "/Applications/Cua Spaces.app", "bundle": true},
                "pid": 42
            }
        });
        let (probe, by) = from_daemon(&json).unwrap();
        assert_eq!(probe, blocked());
        assert!(matches!(by, Prober::Daemon(Some(a)) if a.name == "Cua Spaces"));
        // An older daemon has none.
        assert!(from_daemon(&serde_json::json!({"backends": []})).is_none());
    }

    /// A fake `lume serve` listing one running macOS VM at 127.0.0.1: the
    /// probe lists it and reaches it (a refusal on port 22 counts).
    #[tokio::test]
    async fn probe_lists_lume_vms_and_connects_once_within_its_limits() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let body = r#"[{"name":"stopped","os":"macOS","status":"stopped","ipAddress":"192.168.64.9"},
                    {"name":"linux","os":"linux","status":"running","ipAddress":"192.168.64.8"},
                    {"name":"m1","os":"macOS","status":"running","ipAddress":"127.0.0.1"}]"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            }
        });
        let started = std::time::Instant::now();
        let p = cua_vmm::local_network::probe(&base).await;
        assert!(started.elapsed() < Duration::from_secs(4));
        match p {
            Probe::Reached { vm, ip } | Probe::Unclear { vm, ip, .. } => {
                assert_eq!((vm.as_str(), ip.as_str()), ("m1", "127.0.0.1"));
            }
            other => panic!("{other:?}"),
        }
        // Nothing listening: the list fails fast and the check skips.
        let p = cua_vmm::local_network::probe("http://127.0.0.1:9").await;
        assert!(matches!(p, Probe::NoList { .. }), "{p:?}");
    }
}
