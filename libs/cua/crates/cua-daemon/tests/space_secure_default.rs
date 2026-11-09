//! Opt-in: the secure default (`cua_vmm::network`) holds from inside the
//! guest, on real runtimes. With no policy at all, a process that is root
//! in the guest
//!
//! - reaches the internet (1.1.1.1:443) and resolves names;
//! - reaches a Space of its own tree;
//! - can't reach: the host's loopback through any alias (the bridge
//!   gateway, `host.docker.internal`, `10.0.2.2`, ...), the host's own
//!   addresses, the host's LAN gateway, the cloud metadata endpoint
//!   (`169.254.169.254`, the real one on an AWS host), a Space of another
//!   tree, the daemon's port (also under `localhost` with every port), or a
//!   name that resolves to any of those (DNS rebinding).
//!
//! "Can't reach" means the connection times out or is unreachable: a
//! refused connection (RST) got through and fails the test.
//!
//! ```text
//! CUA_HOME=$(mktemp -d) cargo test -p cua-daemon --test space_secure_default -- \
//!   --ignored --nocapture --test-threads=1
//! ```
//!
//! Needs internet and, per test, what `space_egress.rs` says (a Docker
//! engine on Linux with nftables; gVisor: `runsc`; QEMU: KVM, pasta and
//! `CUA_EGRESS_TEST_DISK`; Firecracker: KVM, pasta and the microVM assets).
//! VMs reach the members of their tree through cua-mesh, not yet through
//! these tests' address probes. Image
//! `CUA_EGRESS_TEST_IMAGE` (default `ghcr.io/trycua/linux:24.04-slim`; it
//! needs python3). Everything is created under the test's `CUA_HOME` and
//! deleted on every path, and the host listeners close with the test.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use cua_vmm::exec::{ExecRequest, GuestExec};
use cua_vmm::network::{NetworkPreset, NetworkSettings};
use cua_vmm::{ImageSource, Runtime, StartSpec};

/// `python3 probe.py <host:port>...` prints `<host:port>=open|refused|blocked`
/// for a TCP connect to each (`blocked`: timed out or unreachable). A host
/// name is resolved in the guest first (`name` stays in the output).
const PROBE_PY: &str = r#"import socket, sys
def tcp(target):
    host, port = target.rsplit(":", 1)
    try:
        ip = socket.getaddrinfo(host, int(port), socket.AF_INET, socket.SOCK_STREAM)[0][4][0]
    except Exception:
        return "noname"
    s = socket.socket(); s.settimeout(5)
    try:
        s.connect((ip, int(port))); return "open"
    except ConnectionRefusedError:
        return "refused"
    except Exception:
        return "blocked"
print(" ".join(t + "=" + tcp(t) for t in sys.argv[1:]))
"#;

/// A guest to run shell commands in, as root.
struct Guest {
    exec: std::sync::Arc<dyn GuestExec>,
    root: &'static str,
}

impl Guest {
    async fn sh(&self, script: &str) -> String {
        let req = ExecRequest::sh(format!("{} sh -c {}", self.root, quote(script)))
            .timeout(Duration::from_secs(120));
        match self.exec.exec(req).await {
            Ok(o) => format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
            .trim()
            .to_string(),
            Err(e) => format!("exec failed: {e}"),
        }
    }

    async fn upload(&self) {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(PROBE_PY);
        let out = self
            .sh(&format!(
                "echo {b64} | base64 -d > /tmp/sd-probe.py && echo ok"
            ))
            .await;
        assert!(out.ends_with("ok"), "upload: {out}");
    }

    /// The probe's verdict per target (retried while the guest's network
    /// comes up: the internet must answer first).
    async fn probe(&self, targets: &[String]) -> Vec<(String, String)> {
        let out = self
            .sh(&format!("python3 /tmp/sd-probe.py {}", targets.join(" ")))
            .await;
        out.split_whitespace()
            .filter_map(|kv| kv.rsplit_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    async fn wait_internet(&self) {
        for _ in 0..30 {
            if self.probe(&["1.1.1.1:443".into()]).await == [("1.1.1.1:443".into(), "open".into())]
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        panic!("the guest never reached the internet (1.1.1.1:443)");
    }

    /// Every target must be `want` (`blocked` for the negatives).
    async fn expect_all(&self, what: &str, targets: &[String], want: &str) {
        let got = self.probe(targets).await;
        eprintln!("  {what}: {got:?}");
        assert_eq!(got.len(), targets.len(), "{what}: {got:?}");
        for (t, v) in &got {
            assert_eq!(v, want, "{what}: {t} is {v}, wanted {want}");
        }
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            % 100000
    )
}

/// Listeners on the host for the guest to (fail to) reach: one on every
/// address, one on loopback only. Accept and hold, so `open` is real.
struct HostListeners {
    any: u16,
    loopback: u16,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn listen(addr: &str) -> (u16, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind(addr).await.expect("bind");
    let port = l.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    (port, task)
}

impl HostListeners {
    async fn start() -> HostListeners {
        let (any, a) = listen("0.0.0.0:0").await;
        let (loopback, b) = listen("127.0.0.1:0").await;
        HostListeners {
            any,
            loopback,
            _tasks: vec![a, b],
        }
    }
}

/// This host's own non-loopback IPv4 addresses and its default gateway.
fn host_facts() -> (Vec<Ipv4Addr>, Option<Ipv4Addr>) {
    let out = std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let addrs = out
        .lines()
        .filter_map(|l| l.split_whitespace().nth(3))
        .filter_map(|a| a.split('/').next()?.parse::<Ipv4Addr>().ok())
        .filter(|a| !a.is_loopback())
        .collect();
    let route = std::process::Command::new("ip")
        .args(["-4", "route", "show", "default"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let gw = route
        .split_whitespace()
        .skip_while(|w| *w != "via")
        .nth(1)
        .and_then(|g| g.parse().ok());
    (addrs, gw)
}

/// The names that resolve to `ip` (a public wildcard DNS service): DNS
/// rebinding must not get the guest past the rules.
fn rebinding(ip: IpAddr, port: u16) -> String {
    format!("{ip}.nip.io:{port}")
}

// ---------------------------------------------------------------- container

mod container {
    use super::*;
    use cua_vmm::container::{ContainerConfig, ContainerRuntime};

    fn image() -> String {
        std::env::var("CUA_EGRESS_TEST_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into())
    }

    fn spec(name: &str, runtime: &str, tree: &str, network: NetworkSettings) -> StartSpec {
        let mut s = StartSpec::new(name, ImageSource::oci(image()))
            .cpus(1)
            .memory_mb(512)
            .command([
                "python3",
                "-c",
                "import socket,time\ns=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind(('0.0.0.0',7777));s.listen(16)\nh=[]\nwhile True:\n  c,_=s.accept();h.append(c)",
            ])
            .ready_timeout(Duration::from_secs(300));
        s.container_runtime = Some(runtime.into());
        s.network = Some(network);
        s.tree = Some(tree.into());
        s
    }

    async fn guest(rt: &ContainerRuntime, name: &str) -> Guest {
        let g = Guest {
            exec: rt.guest_exec(name).await.unwrap().unwrap(),
            root: "",
        };
        g.upload().await;
        g
    }

    async fn ip_of(name: &str) -> String {
        let out = tokio::process::Command::new("docker")
            .args([
                "inspect",
                "-f",
                "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{.Gateway}}{{end}}",
                name,
            ])
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn run(runtime: &str) {
        let rt = ContainerRuntime::connect(ContainerConfig {
            warm: false,
            ..ContainerConfig::default()
        })
        .await
        .expect("a Docker engine");
        let host = HostListeners::start().await;
        let (addrs, lan_gw) = host_facts();
        let tag = suffix();
        let a = format!("cua-sd-a-{runtime}-{tag}");
        let child = format!("cua-sd-c-{runtime}-{tag}");
        let other = format!("cua-sd-o-{runtime}-{tag}");
        let open_host = format!("cua-sd-l-{runtime}-{tag}");
        let names = [a.clone(), child.clone(), other.clone(), open_host.clone()];
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            let tree_a = format!("local:{a}");
            rt.start(&spec(&a, runtime, &tree_a, NetworkSettings::internet()))
                .await
                .expect("start a");
            rt.start(&spec(&child, runtime, &tree_a, NetworkSettings::internet()))
                .await
                .expect("start its child");
            rt.start(&spec(
                &other,
                runtime,
                &format!("local:{other}"),
                NetworkSettings::internet(),
            ))
            .await
            .expect("start another tree");
            let g = guest(&rt, &a).await;
            g.wait_internet().await;

            let a_net = ip_of(&a).await;
            let gateway = a_net.split_whitespace().nth(1).unwrap_or_default().to_string();
            let child_ip = ip_of(&child).await.split_whitespace().next().unwrap_or_default().to_string();
            let other_ip = ip_of(&other).await.split_whitespace().next().unwrap_or_default().to_string();
            eprintln!("{runtime}: a {a_net}, child {child_ip}, other tree {other_ip}");

            // Positives: the internet, names, its own child.
            let dns = g.sh("getent hosts example.com >/dev/null && echo resolved").await;
            assert_eq!(dns, "resolved", "DNS: {dns}");
            g.expect_all(
                "the internet and its own child",
                &["1.1.1.1:443".into(), format!("{child_ip}:7777")],
                "open",
            )
            .await;

            // Negatives.
            let mut no: Vec<String> = vec![
                format!("{gateway}:{}", host.any),
                format!("{gateway}:{}", host.loopback),
                format!("host.docker.internal:{}", host.any),
                format!("169.254.169.254:80"),
                format!("{other_ip}:7777"),
                rebinding("169.254.169.254".parse().unwrap(), 80),
            ];
            for addr in &addrs {
                no.push(format!("{addr}:{}", host.any));
                no.push(rebinding(IpAddr::V4(*addr), host.any));
            }
            if let Some(gw) = lan_gw {
                no.push(format!("{gw}:53"));
                no.push(format!("{gw}:80"));
            }
            // A missing alias name resolves to nothing: drop it rather than
            // count `noname` as reached.
            let got = g.probe(&no).await;
            eprintln!("  {runtime}: host, LAN, metadata, other tree, rebinding: {got:?}");
            for (t, v) in &got {
                assert!(
                    v == "blocked" || (v == "noname" && t.starts_with("host.docker.internal")),
                    "{runtime}: reached {t} ({v})"
                );
            }
            // Root in the guest has no raw sockets to forge with.
            let raw = g
                .sh("python3 -c 'import socket;socket.socket(socket.AF_PACKET,socket.SOCK_RAW)' 2>&1 && echo RAW_OK || echo RAW_DENIED")
                .await;
            assert!(raw.ends_with("RAW_DENIED"), "{runtime}: raw socket: {raw}");

            // `localhost` with every port: the host's listener through the
            // gateway, never the daemon's port.
            eprintln!("{runtime}: localhost (all ports) reaches the host, not the daemon");
            let daemon = std::env::var("CUA_HOME").map(std::path::PathBuf::from).unwrap();
            std::fs::create_dir_all(&daemon).unwrap();
            std::fs::write(
                daemon.join("daemon.json"),
                format!(
                    r#"{{"pid":1,"loopback_url":"http://127.0.0.1:{}","version":"test"}}"#,
                    host.loopback
                ),
            )
            .unwrap();
            let mut lh = NetworkSettings::preset(NetworkPreset::Localhost);
            lh.all_localhost_ports = true;
            rt.start(&spec(&open_host, runtime, &format!("local:{open_host}"), lh))
                .await
                .expect("start localhost");
            let lg = guest(&rt, &open_host).await;
            lg.wait_internet().await;
            let lgw = ip_of(&open_host).await.split_whitespace().nth(1).unwrap_or_default().to_string();
            // A listener on every address answers through the gateway.
            lg.expect_all("the host via localhost", &[format!("{lgw}:{}", host.any)], "open")
                .await;
            let mut daemon_targets = vec![format!("{lgw}:{}", host.loopback)];
            for addr in &addrs {
                daemon_targets.push(format!("{addr}:{}", host.loopback));
            }
            lg.expect_all("the daemon's port", &daemon_targets, "blocked").await;
            lg.expect_all("metadata", &["169.254.169.254:80".into()], "blocked")
                .await;
            let _ = std::fs::remove_file(daemon.join("daemon.json"));
        }))
        .await;
        for n in names.iter().rev() {
            let _ = rt.delete(n).await;
        }
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }

    #[tokio::test]
    #[ignore = "needs a Docker engine on Linux with nftables; run with --ignored"]
    async fn container_runc_secure_default() {
        run("runc").await;
    }

    #[tokio::test]
    #[ignore = "needs a Docker engine on Linux with runsc and nftables; run with --ignored"]
    async fn container_gvisor_secure_default() {
        run("gvisor").await;
    }
}

/// The negatives every VM kind shares: the host's own addresses, the
/// host's loopback (`alias`, the guest's address for it), its LAN gateway,
/// metadata, and rebinding names for them.
fn vm_negatives(host: &HostListeners, alias: &str) -> Vec<String> {
    let (addrs, lan_gw) = host_facts();
    let mut no: Vec<String> = vec![
        format!("{alias}:{}", host.any),
        format!("{alias}:{}", host.loopback),
        "169.254.169.254:80".into(),
        rebinding("169.254.169.254".parse().unwrap(), 80),
    ];
    for addr in &addrs {
        no.push(format!("{addr}:{}", host.any));
        no.push(format!("{addr}:{}", host.loopback));
        no.push(rebinding(IpAddr::V4(*addr), host.any));
    }
    if let Some(gw) = lan_gw {
        no.push(format!("{gw}:53"));
        no.push(format!("{gw}:80"));
    }
    no
}

async fn vm_checks(g: &Guest, what: &str, host: &HostListeners, alias: &str) {
    g.wait_internet().await;
    let dns = g
        .sh("getent hosts example.com >/dev/null && echo resolved")
        .await;
    assert_eq!(dns, "resolved", "{what}: DNS: {dns}");
    let got = g.probe(&vm_negatives(host, alias)).await;
    eprintln!("  {what}: host, LAN, metadata, rebinding: {got:?}");
    for (t, v) in &got {
        assert_eq!(v, "blocked", "{what}: reached {t} ({v})");
    }
}

// --------------------------------------------------------------------- QEMU

mod qemu {
    use super::*;
    use cua_vmm::Probe;
    use cua_vmm::exec::SshExec;
    use cua_vmm::qemu::{QemuConfig, QemuRuntime};

    async fn guest(rt: &QemuRuntime, name: &str) -> Guest {
        let ep = rt.endpoints(name).await.unwrap();
        assert!(ep.qmp.is_none(), "QMP must not be a TCP port: {:?}", ep.qmp);
        let ssh = ep.ssh.expect("SSH endpoint");
        let exec =
            SshExec::new(&ssh.host, ssh.port, &ssh.user).key(ssh.private_key.expect("managed key"));
        let g = Guest {
            exec: std::sync::Arc::new(exec),
            root: "sudo -n",
        };
        for _ in 0..60 {
            if g.sh("id -u").await == "0" {
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        g.upload().await;
        g
    }

    #[tokio::test]
    #[ignore = "boots a QEMU VM under KVM in a pasta namespace; run with --ignored"]
    async fn qemu_secure_default() {
        let disk = std::env::var("CUA_EGRESS_TEST_DISK").expect("CUA_EGRESS_TEST_DISK");
        let root = tempfile::tempdir().unwrap();
        let rt = QemuRuntime::new(QemuConfig {
            root: root.path().to_path_buf(),
            vnc: false,
            warm: false,
            graceful_stop: Duration::from_secs(5),
            ..QemuConfig::default()
        });
        let host = HostListeners::start().await;
        let name = format!("cua-sd-qemu-{}", suffix());
        let mut spec = StartSpec::new(&name, ImageSource::Disk { path: disk.into() })
            .cpus(2)
            .memory_mb(2048)
            .probe(Probe::Tcp { port: 22 })
            .ready_timeout(Duration::from_secs(600));
        spec.network = Some(NetworkSettings::internet());
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            rt.start(&spec).await.expect("start");
            let g = guest(&rt, &name).await;
            vm_checks(&g, "qemu", &host, "10.0.2.2").await;
            // `localhost` with one port: that one answers through 10.0.2.2,
            // nothing else of the host's does.
            eprintln!("qemu: localhost (port {})", host.loopback);
            rt.stop(&name).await.unwrap();
            let mut lh = NetworkSettings::preset(NetworkPreset::Localhost);
            lh.localhost_ports = vec![host.loopback];
            // A kept setting wins; set it through the lists' owner.
            let st = root.path().join(&name).join("state.json");
            let mut v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&st).unwrap()).unwrap();
            v["network"] = serde_json::to_value(&lh).unwrap();
            std::fs::write(&st, serde_json::to_vec(&v).unwrap()).unwrap();
            let mut again = spec.clone();
            again.image = ImageSource::Existing;
            rt.start(&again).await.expect("restart");
            let g = guest(&rt, &name).await;
            g.wait_internet().await;
            g.expect_all(
                "the listed localhost port",
                &[format!("10.0.2.2:{}", host.loopback)],
                "open",
            )
            .await;
            g.expect_all(
                "another host port",
                &[format!("10.0.2.2:{}", host.any)],
                "blocked",
            )
            .await;
        }))
        .await;
        let _ = rt.delete(&name).await;
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}

// -------------------------------------------------------------- Firecracker

mod firecracker {
    use super::*;
    use cua_vmm::microvm::{MicroVmConfig, MicroVmRuntime};

    #[tokio::test]
    #[ignore = "boots a Firecracker microVM under KVM in a pasta namespace; run with --ignored"]
    async fn firecracker_secure_default() {
        let image = std::env::var("CUA_EGRESS_TEST_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
        if cua_vmm::microvm::assets::firecracker_bin().is_none() {
            cua_vmm::microvm::assets::install_firecracker()
                .await
                .expect("install firecracker");
        }
        let rt = MicroVmRuntime::new(MicroVmConfig {
            resolver: Some(cua_image::RootfsLayerResolver::shared()),
            ..MicroVmConfig::default()
        });
        let host = HostListeners::start().await;
        let name = format!("cua-sd-fc-{}", suffix());
        let mut spec = StartSpec::new(&name, ImageSource::oci(image))
            .cpus(1)
            .memory_mb(1024)
            .command(["sleep", "infinity"])
            .ready_timeout(Duration::from_secs(300));
        spec.network = Some(NetworkSettings::internet());
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            rt.start(&spec).await.expect("start");
            let g = Guest {
                exec: rt.guest_exec(&name).await.unwrap().unwrap(),
                root: "",
            };
            g.upload().await;
            vm_checks(&g, "firecracker", &host, "172.16.0.1").await;
        }))
        .await;
        let _ = rt.delete(&name).await;
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}
