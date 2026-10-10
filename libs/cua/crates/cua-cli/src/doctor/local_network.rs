//! `host.local_network`: whether this Mac can reach its own macOS VMs.
//!
//! A macOS Space is a VM on Apple's shared network (192.168.64.x); the cua
//! daemon talks to it there. macOS blocks that until Local Network access
//! is on for the app responsible for the process, and then a connect fails
//! at once with "No route to host" (`EHOSTUNREACH`), the probe the SDK and
//! the app core read (`cua_vmm::probe::is_unreachable`). The check connects
//! once to a running VM `lume serve` lists, with short time limits, from
//! this process (the terminal app that ran `cua doctor`). It is never
//! required: a blocked VM network warns; no VM to try, or not a Mac, skips
//! with the reason.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Severity, Status};

const ID: &str = "host.local_network";
/// Listing the VMs (`lume serve` is local).
const LIST_TIMEOUT: Duration = Duration::from_secs(2);
/// One connect to the VM. A blocked network fails at once; a VM that is
/// there answers (accepts or refuses) well within this.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Any port works: a refusal comes back over the same network.
const PROBE_PORT: u16 = 22;

/// Where the setting is, as the user reads it in System Settings.
pub const SETTINGS_PATH: &str = "System Settings > Privacy & Security > Local Network";
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

/// What the probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// `lume serve` did not list its VMs in time (or answered an error).
    NoList(String),
    /// No macOS VM is running with an address.
    NoRunningVm,
    /// The VM answered (accepted or refused the connection).
    Reached { vm: String, ip: String },
    /// No route to the VM: Local Network access is off for this app.
    Blocked { vm: String, ip: String },
    /// No answer in time, or another error: inconclusive.
    Unclear { vm: String, ip: String, why: String },
}

/// The check for what this machine is and what the probe found (pure).
/// `macos_vms`: this Mac runs local macOS VMs (lume answers, or it
/// provides Spaces).
pub fn check(os: &str, macos_vms: bool, probe: Option<Probe>) -> Check {
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
    let allow = format!(
        "open {SETTINGS_PATH} and turn on the app you ran cua from (Terminal, or your terminal app), \
         and Cua Spaces or cua-spacesd, which start the cua daemon; then run `cua doctor` again"
    );
    match probe {
        None | Some(Probe::NoRunningVm) => skip(
            "no macOS VM is running, so Local Network access can't be tested; start a macOS Space and run `cua doctor` again".into(),
            "no_running_vm",
        )
        .fix(format!("if macOS Spaces fail with \"No route to host\": {allow}")),
        Some(Probe::NoList(why)) => skip(
            format!("couldn't list this Mac's VMs ({why})"),
            "no_vm_list",
        ),
        Some(Probe::Reached { vm, ip }) => info(Check::new(
            ID,
            Status::Pass,
            format!("reached the macOS VM {vm} at {ip}: Local Network access is on"),
        )),
        Some(Probe::Blocked { vm, ip }) => info(
            Check::new(
                ID,
                Status::Warn,
                format!(
                    "can't reach the macOS VM {vm} at {ip} (No route to host): Local Network access is off. \
                     macOS Spaces on this Mac fail until it is on"
                ),
            )
            .fix(format!("{allow} ({SETTINGS_URL})")),
        ),
        Some(Probe::Unclear { vm, ip, why }) => skip(
            format!("couldn't tell whether the macOS VM {vm} at {ip} is reachable ({why})"),
            "inconclusive",
        )
        .fix(format!("if macOS Spaces fail with \"No route to host\": {allow}")),
    }
}

fn info(mut c: Check) -> Check {
    c.severity = Severity::Info;
    c
}

/// What one connect says about the network (pure).
pub fn classify(vm: &str, ip: &str, result: Result<std::io::Result<()>, ()>) -> Probe {
    let (vm, ip) = (vm.to_owned(), ip.to_owned());
    match result {
        Ok(Ok(())) => Probe::Reached { vm, ip },
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            Probe::Reached { vm, ip }
        }
        Ok(Err(e)) if cua_vmm::probe::is_unreachable(&e) => Probe::Blocked { vm, ip },
        Ok(Err(e)) => Probe::Unclear {
            vm,
            ip,
            why: e.to_string(),
        },
        Err(()) => Probe::Unclear {
            vm,
            ip,
            why: format!("no answer in {} ms", CONNECT_TIMEOUT.as_millis()),
        },
    }
}

/// Lists the VMs `lume serve` runs and connects once to the first running
/// one with an address. Bounded: about 3.5 s at most.
pub async fn probe(lume_base: &str) -> Probe {
    let client = cua_vmm::lume::LumeClient::new(lume_base);
    let vms = match tokio::time::timeout(LIST_TIMEOUT, client.list()).await {
        Ok(Ok(vms)) => vms,
        Ok(Err(e)) => return Probe::NoList(e.to_string()),
        Err(_) => return Probe::NoList(format!("no answer in {} s", LIST_TIMEOUT.as_secs())),
    };
    let Some((vm, ip)) = vms.iter().find_map(|v| {
        (v.status == "running" && v.os.eq_ignore_ascii_case("macos"))
            .then(|| v.ip().map(|ip| (v.name.clone(), ip.to_owned())))
            .flatten()
    }) else {
        return Probe::NoRunningVm;
    };
    let addr = format!("{ip}:{PROBE_PORT}");
    let result = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&addr))
        .await
        .map(|r| r.map(drop))
        .map_err(drop);
    classify(&vm, &ip, result)
}

/// The `lume serve` the runtime doctor talks to.
pub fn lume_base() -> String {
    std::env::var("LUME_API").unwrap_or_else(|_| "http://127.0.0.1:7777".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn ok_vm() -> Option<Probe> {
        Some(Probe::Reached {
            vm: "m1".into(),
            ip: "192.168.64.5".into(),
        })
    }

    #[test]
    fn not_a_mac_or_no_vms_skips_with_the_reason() {
        let c = check("linux", true, ok_vm());
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

    #[test]
    fn a_blocked_vm_network_warns_and_says_where_to_turn_it_on() {
        let c = check(
            "macos",
            true,
            Some(Probe::Blocked {
                vm: "m1".into(),
                ip: "192.168.64.5".into(),
            }),
        );
        assert_eq!(c.status, Status::Warn);
        assert_eq!(c.severity, Severity::Info, "never required");
        assert!(c.message.contains("No route to host"), "{}", c.message);
        assert!(
            c.remediation
                .contains("System Settings > Privacy & Security > Local Network"),
            "{}",
            c.remediation
        );
        assert!(c.remediation.contains("turn on"), "{}", c.remediation);
        assert!(
            c.remediation.contains("Privacy_LocalNetwork"),
            "{}",
            c.remediation
        );
    }

    #[test]
    fn a_reached_vm_passes_and_no_vm_skips_with_the_fix() {
        let c = check("macos", true, ok_vm());
        assert_eq!(c.status, Status::Pass);
        assert!(c.message.contains("192.168.64.5"));
        let c = check("macos", true, Some(Probe::NoRunningVm));
        assert_eq!(c.status, Status::Skip);
        assert_eq!(
            c.facts.get("skip_reason").map(String::as_str),
            Some("no_running_vm")
        );
        assert!(c.remediation.contains("Local Network"), "{}", c.remediation);
        let c = check("macos", true, Some(Probe::NoList("not reachable".into())));
        assert_eq!(c.status, Status::Skip);
    }

    #[test]
    fn connect_results_classify() {
        let unreachable = Error::from(ErrorKind::HostUnreachable);
        assert!(matches!(
            classify("m", "1.2.3.4", Ok(Err(unreachable))),
            Probe::Blocked { .. }
        ));
        let net = Error::from(ErrorKind::NetworkUnreachable);
        assert!(matches!(
            classify("m", "1.2.3.4", Ok(Err(net))),
            Probe::Blocked { .. }
        ));
        // A refusal comes back over the network: reachable.
        let refused = Error::from(ErrorKind::ConnectionRefused);
        assert!(matches!(
            classify("m", "1.2.3.4", Ok(Err(refused))),
            Probe::Reached { .. }
        ));
        assert!(matches!(
            classify("m", "1.2.3.4", Ok(Ok(()))),
            Probe::Reached { .. }
        ));
        assert!(matches!(
            classify("m", "1.2.3.4", Err(())),
            Probe::Unclear { .. }
        ));
        // An Unclear probe skips (never a warning on a guess).
        assert_eq!(
            check("macos", true, Some(classify("m", "1.2.3.4", Err(())))).status,
            Status::Skip
        );
    }

    /// A fake `lume serve` listing one running macOS VM at 127.0.0.1: the
    /// probe lists it and reaches it (a refusal on port 22 counts).
    #[tokio::test]
    async fn probe_lists_lume_vms_and_connects_once_within_its_limits() {
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
        let p = probe(&base).await;
        assert!(started.elapsed() < Duration::from_secs(4));
        match p {
            Probe::Reached { vm, ip } | Probe::Unclear { vm, ip, .. } => {
                assert_eq!((vm.as_str(), ip.as_str()), ("m1", "127.0.0.1"));
            }
            other => panic!("{other:?}"),
        }
        // Nothing listening: the list fails fast and the check skips.
        let p = probe("http://127.0.0.1:9").await;
        assert!(matches!(p, Probe::NoList(_)), "{p:?}");
    }
}
