//! Opt-in, on an Apple silicon Mac: a macOS microVM (`vz`) reaches the
//! network only through the Cua network gateway (`cua_vmm::netgw`), which
//! checks every connection the guest opens. From inside the guest, as root:
//!
//! - the host's loopback is out of reach: the gateway's host address, a
//!   listener on 127.0.0.1, another Space's published port and the port a
//!   daemon would listen on;
//! - this Mac's own LAN address is out of reach;
//! - the cloud metadata address (169.254.169.254) is refused by the gateway
//!   itself (its event says so; no listener is needed);
//! - another Space (its own tree) is out of reach: every VM has its own
//!   gateway, and its published port is on the host's loopback;
//! - with the private ranges denied (the `internet` preset's default
//!   denies), a LAN address is refused and a name that resolves to a
//!   private or loopback address gets no answer (DNS rebinding);
//! - the internet is reachable, and DNS works;
//!
//! and a new policy applies to the running VM at once (`set_egress`).
//!
//! ```text
//! CUA_HOME=$(mktemp -d) CUA_VZ=... CUA_NETGW=... CUA_MICROVM_INIT=... \
//!   cargo test -p cua-daemon --test space_netgw -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Needs internet (1.1.1.1 on 443, public DNS for `localtest.me` and
//! `nip.io` names), `cua-vz`, `cua-netgw` and the static aarch64 guest init.
//! `CUA_NETGW_TEST_IMAGE` overrides the image (it needs python3; default
//! `python:3.13-alpine`, the smallest). Everything is created under the
//! test's `CUA_HOME` and deleted on every path.

#![cfg(target_os = "macos")]

use std::net::{IpAddr, TcpListener, UdpSocket};
use std::time::Duration;

use cua_vmm::egress::EgressPolicy;
use cua_vmm::exec::ExecRequest;
use cua_vmm::microvm::{MicroVmConfig, MicroVmRuntime};
use cua_vmm::{ImageSource, Runtime, StartSpec};

/// `python3 - <target>...` (this script on stdin): a target is `ip:port` (a TCP connect,
/// `open` or `blocked`) or `name` (its IPv4 answers, `-` for none).
const PROBE: &str = r#"
import socket, sys
out = []
for t in sys.argv[1:]:
    if ":" in t:
        ip, port = t.rsplit(":", 1)
        s = socket.socket(); s.settimeout(5)
        try:
            s.connect((ip, int(port))); r = "open"
        except Exception:
            r = "blocked"
        s.close()
    else:
        try:
            r = ",".join(sorted({a[4][0] for a in socket.getaddrinfo(t, 80, socket.AF_INET)})) or "-"
        except Exception:
            r = "-"
    out.append(t + "=" + r)
print(" ".join(out))
"#;

/// The Mac's LAN address (the one its default route leaves from).
fn lan_ip() -> IpAddr {
    let s = UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("1.1.1.1:53").unwrap();
    s.local_addr().unwrap().ip()
}

/// A listener that accepts (and holds) every connection.
fn listen(addr: &str) -> (TcpListener, u16) {
    let l = TcpListener::bind(addr).unwrap();
    let port = l.local_addr().unwrap().port();
    let c = l.try_clone().unwrap();
    std::thread::spawn(move || for _ in c.incoming() {});
    (l, port)
}

fn runtime() -> MicroVmRuntime {
    MicroVmRuntime::new(MicroVmConfig {
        resolver: Some(cua_image::RootfsLayerResolver::shared()),
        ..Default::default()
    })
}

fn spec(name: &str, egress: EgressPolicy) -> StartSpec {
    let image = std::env::var("CUA_NETGW_TEST_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/python:3.13-alpine".into());
    let mut s = StartSpec::new(name, ImageSource::oci(image))
        .cpus(1)
        .memory_mb(512)
        .command(["sleep", "86400"])
        .port(8000)
        .ready_timeout(Duration::from_secs(300));
    s.egress = egress;
    s
}

async fn probe(rt: &MicroVmRuntime, name: &str, targets: &[String]) -> String {
    let exec = rt.guest_exec(name).await.unwrap().expect("exec");
    let script = format!("python3 - {}", targets.join(" "));
    let out = exec
        .exec(
            ExecRequest::sh(script)
                .stdin(PROBE)
                .timeout(Duration::from_secs(120)),
        )
        .await
        .unwrap();
    let s = out.stdout_str();
    eprintln!("{name}: {s}");
    s
}

fn expect(out: &str, target: &str, want: &str) {
    let got = out
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{target}=")))
        .unwrap_or("(missing)");
    assert_eq!(got, want, "{target} in {out}");
}

/// The VM's blocked-connection events (the gateway's JSON lines).
fn events(name: &str) -> String {
    let dir = MicroVmConfig::default().root.join("instances").join(name);
    std::fs::read_to_string(dir.join(cua_vmm::netgw::EVENTS_FILE)).unwrap_or_default()
}

fn has_event(name: &str, reason: &str, dst: &str) -> bool {
    events(name).lines().any(|l| {
        let v: serde_json::Value = serde_json::from_str(l).unwrap_or_default();
        v["reason"] == reason && v["dst"] == dst
    })
}

/// The private ranges the `internet` preset denies by default.
fn private_denies() -> EgressPolicy {
    EgressPolicy {
        allow: vec![],
        deny: [
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "100.64.0.0/10",
            "169.254.0.0/16",
        ]
        .map(String::from)
        .to_vec(),
    }
}

#[tokio::test]
#[ignore = "boots macOS microVMs (cua-vz, cua-netgw, the guest init, internet); run with --ignored"]
async fn vz_guests_reach_the_internet_and_nothing_of_the_host() {
    let rt = std::sync::Arc::new(runtime());
    let id = format!("{:x}", std::process::id());
    let a = format!("netgw-a-{id}");
    let b = format!("netgw-b-{id}");
    let body = {
        let (rt, a, b) = (rt.clone(), a.clone(), b.clone());
        async move {
            let (_lo, lo_port) = listen("127.0.0.1:0");
            let lan = lan_ip();
            let (_lan_l, lan_port) = listen(&format!("{lan}:0"));
            // The daemon's port on the host loopback (any port there is one).
            let daemon_port = 4243;

            let ib = rt
                .start(&spec(&b, EgressPolicy::default()))
                .await
                .expect("start b");
            let b_port = *ib.endpoints.ports.get(&8000).expect("b publishes 8000");
            rt.start(&spec(&a, private_denies()))
                .await
                .expect("start a");

            let host = cua_vmm::microvm::vz::HOST_IP;
            let targets: Vec<String> = vec![
                format!("{host}:{lo_port}"),
                format!("{host}:{b_port}"),
                format!("{host}:{daemon_port}"),
                format!("{lan}:{lan_port}"),
                format!("{lan}:{b_port}"),
                "169.254.169.254:80".into(),
                "192.168.1.1:80".into(),
                "10.0.0.1:80".into(),
                "1.1.1.1:443".into(),
                "localtest.me".into(),
                "10.0.0.1.nip.io".into(),
                "192.168.1.1.nip.io".into(),
                "one.one.one.one".into(),
            ];
            let out = probe(&rt, &a, &targets).await;
            for t in &targets[..8] {
                expect(&out, t, "blocked");
            }
            expect(&out, "1.1.1.1:443", "open");
            expect(&out, "localtest.me", "-");
            expect(&out, "10.0.0.1.nip.io", "-");
            expect(&out, "192.168.1.1.nip.io", "-");
            assert!(!out.contains("one.one.one.one=-"), "public DNS: {out}");
            // Refused by the gateway, with the reason, not by a missing listener.
            assert!(has_event(
                &a,
                "default:host-loopback",
                &format!("{host}:{lo_port}")
            ));
            assert!(has_event(
                &a,
                "default:host-address",
                &format!("{lan}:{lan_port}")
            ));
            assert!(has_event(&a, "platform:metadata", "169.254.169.254:80"));
            assert!(has_event(&a, "user:deny", "10.0.0.1:80"));
            assert!(
                has_event(&a, "user:deny", "10.0.0.1"),
                "the rebinding answer was stripped"
            );
            assert!(has_event(&a, "platform:loopback", "127.0.0.1"));
            let ev = events(&a);
            assert!(
                !ev.contains("nip.io") && !ev.contains("localtest"),
                "names stay out: {ev}"
            );

            // b, without a policy, still never reaches the host or a.
            let out = probe(
                &rt,
                &b,
                &[format!("{host}:{lo_port}"), format!("{lan}:{lan_port}")],
            )
            .await;
            expect(&out, &format!("{host}:{lo_port}"), "blocked");
            expect(&out, &format!("{lan}:{lan_port}"), "blocked");

            // A new policy applies to the running VM at once.
            let deny_cf = EgressPolicy {
                allow: vec![],
                deny: vec!["1.1.1.1".into()],
            };
            rt.set_egress(&a, &deny_cf).await.expect("set_egress");
            tokio::time::sleep(Duration::from_secs(1)).await;
            let out = probe(&rt, &a, &["1.1.1.1:443".into(), "1.0.0.1:443".into()]).await;
            expect(&out, "1.1.1.1:443", "blocked");
            expect(&out, "1.0.0.1:443", "open");
        }
    };
    // The VMs go on every path, a failed assertion too.
    let result = tokio::spawn(body).await;
    for n in [&a, &b] {
        let _ = rt.delete(n).await;
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}
