//! Opt-in: an egress policy is enforced by the host, outside the guest, for
//! every Linux kind (`cua_vmm::egress`). In each, a process that is root in
//! the guest
//!
//! - reaches an allowed address and fails to reach a denied one, from the
//!   guest's first instruction (the workload's own command probes both);
//! - flushes the guest's own firewall (`nft flush ruleset; iptables -F`)
//!   and still fails;
//! - fails with a hand-made TCP SYN over a raw packet socket, and from a
//!   new network namespace;
//!
//! and the policy follows `set_egress` while the guest runs, a restart and
//! a fork.
//!
//! ```text
//! CUA_HOME=$(mktemp -d) cargo test -p cua-daemon --test space_egress -- \
//!   --ignored --nocapture --test-threads=1
//! ```
//!
//! Needs internet (1.1.1.1 and 8.8.8.8 on 443) and, per test:
//!
//! - `container_*`: a Docker engine on Linux with nftables on its host
//!   (gVisor: `runsc` registered); image `CUA_EGRESS_TEST_IMAGE` (default
//!   `ghcr.io/trycua/linux:24.04-slim`; it needs python3);
//! - `qemu`: KVM, pasta (passt) with unprivileged user namespaces, and
//!   `CUA_EGRESS_TEST_DISK`, a Linux cloud image with cloud-init, sshd and
//!   python3 (Ubuntu's `noble-server-cloudimg-*.img`);
//! - `firecracker`: KVM, pasta, and the microVM assets
//!   (`cua runtime setup firecracker`; `CUA_MICROVM_INIT` for the init).
//!
//! Everything is created under the test's `CUA_HOME` and deleted on every
//! path.

use std::time::Duration;

use cua_vmm::egress::EgressPolicy;
use cua_vmm::exec::{ExecRequest, GuestExec};
use cua_vmm::{ImageSource, Probe, Runtime, StartSpec};

const ALLOWED: &str = "1.1.1.1";
const DENIED: &str = "8.8.8.8";

/// `python3 probe.py <ip>...` prints `<ip>=open|blocked` for a TCP connect
/// to each on 443.
const PROBE_PY: &str = r#"import socket, sys
def tcp(ip):
    s = socket.socket(); s.settimeout(6)
    try:
        s.connect((ip, 443)); return "open"
    except Exception:
        return "blocked"
print(" ".join(a + "=" + tcp(a) for a in sys.argv[1:]))
"#;

/// The workload's own command: its first instructions probe both
/// addresses, then it idles.
const FIRST_PY: &str = r#"import socket, time
def tcp(ip):
    s = socket.socket(); s.settimeout(6)
    try:
        s.connect((ip, 443)); return "open"
    except Exception:
        return "blocked"
r = " ".join(a + "=" + tcp(a) for a in ("1.1.1.1", "8.8.8.8"))
open("/tmp/egress-first", "w").write(r + "\n")
while True:
    time.sleep(3600)
"#;

/// `python3 raw.py <ip>` sends a TCP SYN to <ip>:443 over an AF_PACKET
/// socket (past the guest's IP stack and firewall, the qdisc too) and
/// prints whether a SYN-ACK came back.
const RAW_PY: &str = r#"import fcntl, random, socket, struct, sys, time
def csum(b):
    if len(b) % 2: b += b"\0"
    s = sum(struct.unpack("!%dH" % (len(b) // 2), b))
    while s >> 16: s = (s & 0xffff) + (s >> 16)
    return (~s) & 0xffff
dst = sys.argv[1]
route = [l.split() for l in open("/proc/net/route").read().splitlines()[1:] if l.split()]
ifc, gwhex = [(r[0], r[2]) for r in route if r[1] == "00000000"][0]
gwip = socket.inet_ntoa(struct.pack("<I", int(gwhex, 16)))
try:
    s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x0800))
except OSError as e:
    print("RAW: no raw socket:", e); sys.exit(0)
s.bind((ifc, 0))
try: s.setsockopt(263, 20, 1)
except OSError: pass
mac = s.getsockname()[4]
ip = fcntl.ioctl(socket.socket(), 0x8915, struct.pack("256s", ifc.encode()))[20:24]
socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(b"x", (gwip, 9))
time.sleep(0.5)
gwmac = None
for l in open("/proc/net/arp").read().splitlines()[1:]:
    f = l.split()
    if f and f[0] == gwip: gwmac = bytes.fromhex(f[3].replace(":", ""))
if gwmac is None:
    print("RAW: no gateway MAC"); sys.exit(0)
sport = random.randint(20000, 60000)
tcp = struct.pack("!HHIIBBHHH", sport, 443, 1234, 0, 5 << 4, 0x02, 64240, 0, 0)
pseudo = ip + socket.inet_aton(dst) + struct.pack("!BBH", 0, 6, len(tcp))
tcp = tcp[:16] + struct.pack("!H", csum(pseudo + tcp)) + tcp[18:]
iph = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(tcp), 1, 0, 64, 6, 0, ip, socket.inet_aton(dst))
iph = iph[:10] + struct.pack("!H", csum(iph)) + iph[12:]
s.settimeout(0.5)
for _ in range(3):
    s.send(gwmac + mac + b"\x08\x00" + iph + tcp)
    end = time.time() + 1.5
    while time.time() < end:
        try: f = s.recv(2048)
        except socket.timeout: continue
        if len(f) < 54 or f[12:14] != b"\x08\x00" or f[23] != 6: continue
        if socket.inet_ntoa(f[26:30]) == dst and struct.unpack("!H", f[36:38])[0] == sport and f[47] & 0x12 == 0x12:
            print("RAW: SYN-ACK from", dst); sys.exit(0)
print("RAW: no answer from", dst)
"#;

fn policy(allow: &[&str], deny: &[&str]) -> EgressPolicy {
    EgressPolicy {
        allow: allow.iter().map(|s| s.to_string()).collect(),
        deny: deny.iter().map(|s| s.to_string()).collect(),
    }
}

/// A guest to run shell commands in, as root.
struct Guest {
    exec: std::sync::Arc<dyn GuestExec>,
    /// Prefix that makes a command root (`sudo -n` over SSH).
    root: &'static str,
}

impl Guest {
    async fn sh(&self, script: &str) -> String {
        let req = ExecRequest::sh(format!("{} sh -c {}", self.root, quote(script)))
            .timeout(Duration::from_secs(90));
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
        for (path, body) in [
            ("/tmp/egress-probe.py", PROBE_PY),
            ("/tmp/egress-raw.py", RAW_PY),
        ] {
            let b64 = base64::engine::general_purpose::STANDARD.encode(body);
            let out = self
                .sh(&format!("echo {b64} | base64 -d > {path} && echo ok"))
                .await;
            assert!(out.ends_with("ok"), "upload {path}: {out}");
        }
    }

    async fn probe(&self, ips: &[&str]) -> String {
        self.sh(&format!("python3 /tmp/egress-probe.py {}", ips.join(" ")))
            .await
    }

    /// Waits until `probe(ips)` says `want` (a policy change is immediate;
    /// a guest that just booted may need its network first).
    async fn expect(&self, ips: &[&str], want: &str) {
        let mut last = String::new();
        for _ in 0..10 {
            last = self.probe(ips).await;
            if last == want {
                eprintln!("  {ips:?}: {last}");
                return;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        panic!("{ips:?}: wanted {want:?}, got {last:?}");
    }

    /// The bypass attempts, as root: the guest's own firewall flushed, a
    /// raw SYN, a new network namespace. `DENIED` must stay unreachable.
    async fn bypass(&self) {
        let flush = self
            .sh(&format!(
                "nft flush ruleset; iptables -F; iptables -P OUTPUT ACCEPT; ip6tables -F; \
                 id -u; python3 /tmp/egress-probe.py {DENIED}"
            ))
            .await;
        eprintln!("  as root, nft flush ruleset; iptables -F; connect {DENIED}:\n{flush}");
        assert!(
            flush.contains(&format!("{DENIED}=blocked")) && !flush.contains("=open"),
            "flushing the guest's firewall opened the way: {flush}"
        );
        let raw = self
            .sh(&format!("python3 /tmp/egress-raw.py {DENIED}"))
            .await;
        eprintln!("  raw SYN over AF_PACKET to {DENIED}: {raw}");
        assert!(raw.contains("RAW:"), "{raw}");
        assert!(!raw.contains("SYN-ACK"), "a raw SYN got through: {raw}");
        let ns = self
            .sh(&format!(
                "unshare -n python3 /tmp/egress-probe.py {DENIED} 2>&1 || true"
            ))
            .await;
        eprintln!("  from a new network namespace: {ns}");
        assert!(!ns.contains("=open"), "{ns}");
    }

    /// The workload's first instructions saw the policy.
    async fn first(&self, want: &str) {
        let mut out = String::new();
        for _ in 0..20 {
            out = self.sh("cat /tmp/egress-first 2>/dev/null").await;
            if !out.is_empty() && !out.contains("No such file") {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        eprintln!("  the workload's first instructions: {out}");
        assert_eq!(out, want);
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

// ---------------------------------------------------------------- container

mod container {
    use super::*;
    use cua_vmm::container::{ContainerConfig, ContainerRuntime};

    fn image() -> String {
        std::env::var("CUA_EGRESS_TEST_IMAGE")
            .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into())
    }

    async fn guest(rt: &ContainerRuntime, name: &str) -> Guest {
        let g = Guest {
            exec: rt.guest_exec(name).await.unwrap().unwrap(),
            root: "",
        };
        g.upload().await;
        g
    }

    async fn run(runtime: &str) {
        let rt = ContainerRuntime::connect(ContainerConfig {
            warm: false,
            ..ContainerConfig::default()
        })
        .await
        .expect("a Docker engine");
        let name = format!("cua-egress-{runtime}-{}", suffix());
        let fork = format!("{name}-fork");
        let mut spec = StartSpec::new(&name, ImageSource::oci(image()))
            .cpus(1)
            .memory_mb(512)
            .command(["python3", "-c", FIRST_PY])
            .ready_timeout(Duration::from_secs(300));
        spec.container_runtime = Some(runtime.into());
        spec.egress = policy(&[ALLOWED], &["all"]);
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            let inst = rt.start(&spec).await.expect("start");
            eprintln!("{runtime}: started {name} ({:?})", inst.isolation);
            let g = guest(&rt, &name).await;
            g.first(&format!("{ALLOWED}=open {DENIED}=blocked")).await;
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=open {DENIED}=blocked"),
            )
            .await;
            g.bypass().await;

            eprintln!("{runtime}: set_egress while it runs (allow {DENIED} too)");
            rt.set_egress(&name, &policy(&[ALLOWED, DENIED], &["all"]))
                .await
                .unwrap();
            g.expect(&[ALLOWED, DENIED], &format!("{ALLOWED}=open {DENIED}=open"))
                .await;
            eprintln!("{runtime}: set_egress deny {ALLOWED} only");
            let deny_one = policy(&[], &[ALLOWED]);
            rt.set_egress(&name, &deny_one).await.unwrap();
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;
            assert_eq!(rt.egress(&name).await.unwrap(), deny_one);

            eprintln!("{runtime}: stop and start: the policy is applied again");
            rt.stop(&name).await.unwrap();
            rt.start(&spec).await.expect("restart");
            let g = guest(&rt, &name).await;
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;

            eprintln!("{runtime}: a fork inherits it");
            rt.stop(&name).await.unwrap();
            rt.fork(&name, &fork).await.unwrap();
            assert_eq!(rt.egress(&fork).await.unwrap(), deny_one);
            let mut fspec = spec.clone();
            fspec.name = fork.clone();
            fspec.image = ImageSource::Existing;
            fspec.egress = EgressPolicy::default();
            rt.start(&fspec).await.expect("start fork");
            let fg = guest(&rt, &fork).await;
            fg.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;
            fg.bypass_allowed_variant().await;

            eprintln!("{runtime}: an empty policy removes it");
            rt.set_egress(&fork, &EgressPolicy::default())
                .await
                .unwrap();
            fg.expect(&[ALLOWED, DENIED], &format!("{ALLOWED}=open {DENIED}=open"))
                .await;
        }))
        .await;
        let _ = rt.delete(&fork).await;
        let _ = rt.delete(&name).await;
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }

    #[tokio::test]
    #[ignore = "needs a Docker engine on Linux with nftables; run with --ignored"]
    async fn container_runc() {
        run("runc").await;
    }

    #[tokio::test]
    #[ignore = "needs a Docker engine on Linux with runsc and nftables; run with --ignored"]
    async fn container_gvisor() {
        run("gvisor").await;
    }
}

impl Guest {
    /// The bypass attempts against a policy that denies `ALLOWED`.
    #[allow(dead_code)]
    async fn bypass_allowed_variant(&self) {
        let out = self
            .sh(&format!(
                "nft flush ruleset; iptables -F; iptables -P OUTPUT ACCEPT; \
                 python3 /tmp/egress-probe.py {ALLOWED}"
            ))
            .await;
        eprintln!("  as root, flush then connect {ALLOWED}: {out}");
        assert!(
            out.contains(&format!("{ALLOWED}=blocked")) && !out.contains("=open"),
            "{out}"
        );
        let raw = self
            .sh(&format!("python3 /tmp/egress-raw.py {ALLOWED}"))
            .await;
        eprintln!("  raw SYN to {ALLOWED}: {raw}");
        assert!(!raw.contains("SYN-ACK"), "{raw}");
    }
}

// --------------------------------------------------------------------- QEMU

mod qemu {
    use super::*;
    use cua_vmm::exec::SshExec;
    use cua_vmm::qemu::{QemuConfig, QemuRuntime};

    async fn guest(rt: &QemuRuntime, name: &str) -> Guest {
        let ep = rt.endpoints(name).await.unwrap();
        let ssh = ep.ssh.expect("SSH endpoint");
        let exec =
            SshExec::new(&ssh.host, ssh.port, &ssh.user).key(ssh.private_key.expect("managed key"));
        let g = Guest {
            exec: std::sync::Arc::new(exec),
            root: "sudo -n",
        };
        // cloud-init may still be finishing.
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
    async fn qemu() {
        let disk = std::env::var("CUA_EGRESS_TEST_DISK").expect("CUA_EGRESS_TEST_DISK");
        let root = tempfile::tempdir().unwrap();
        let rt = QemuRuntime::new(QemuConfig {
            root: root.path().to_path_buf(),
            vnc: false,
            warm: false,
            graceful_stop: Duration::from_secs(5),
            ..QemuConfig::default()
        });
        let name = format!("cua-egress-qemu-{}", suffix());
        let spec = StartSpec::new(&name, ImageSource::Disk { path: disk.into() })
            .cpus(2)
            .memory_mb(2048)
            .probe(Probe::Tcp { port: 22 })
            .ready_timeout(Duration::from_secs(600));
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            rt.start(&spec).await.expect("start");
            let g = guest(&rt, &name).await;
            eprintln!("qemu: no policy");
            g.expect(&[ALLOWED, DENIED], &format!("{ALLOWED}=open {DENIED}=open"))
                .await;
            // Memory survives the move into the namespace.
            g.sh("echo carried > /dev/shm/egress-marker").await;

            eprintln!("qemu: set_egress on the running VM (moves it into a namespace)");
            rt.set_egress(&name, &policy(&[ALLOWED], &["all"]))
                .await
                .unwrap();
            let g = guest(&rt, &name).await;
            assert_eq!(g.sh("cat /dev/shm/egress-marker").await, "carried");
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=open {DENIED}=blocked"),
            )
            .await;
            g.bypass().await;
            let dns = g
                .sh("getent hosts one.one.one.one >/dev/null && echo resolved || echo no-dns")
                .await;
            eprintln!("  DNS through the VM's resolver (10.0.2.3, not allowed): {dns}");
            assert_eq!(dns, "no-dns");

            eprintln!("qemu: set_egress in the namespace (allow 10.0.2.3 and {DENIED})");
            rt.set_egress(&name, &policy(&[ALLOWED, DENIED, "10.0.2.3"], &["all"]))
                .await
                .unwrap();
            g.expect(&[ALLOWED, DENIED], &format!("{ALLOWED}=open {DENIED}=open"))
                .await;
            let dns = g
                .sh("getent hosts one.one.one.one >/dev/null && echo resolved || echo no-dns")
                .await;
            assert_eq!(dns, "resolved");

            eprintln!("qemu: stop and start: in its namespace from the first instruction");
            rt.set_egress(&name, &policy(&[], &[DENIED])).await.unwrap();
            rt.stop(&name).await.unwrap();
            rt.start(&spec).await.expect("restart");
            let g = guest(&rt, &name).await;
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=open {DENIED}=blocked"),
            )
            .await;
            g.bypass().await;
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
    async fn firecracker() {
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
        let name = format!("cua-egress-fc-{}", suffix());
        let fork = format!("{name}-fork");
        let mut spec = StartSpec::new(&name, ImageSource::oci(image))
            .cpus(1)
            .memory_mb(1024)
            .command(["python3", "-c", FIRST_PY])
            .ready_timeout(Duration::from_secs(300));
        spec.egress = policy(&[ALLOWED], &["all"]);
        let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            rt.start(&spec).await.expect("start");
            let g = Guest {
                exec: rt.guest_exec(&name).await.unwrap().unwrap(),
                root: "",
            };
            g.upload().await;
            g.first(&format!("{ALLOWED}=open {DENIED}=blocked")).await;
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=open {DENIED}=blocked"),
            )
            .await;
            g.bypass().await;

            eprintln!("firecracker: set_egress while it runs");
            rt.set_egress(&name, &policy(&[], &[ALLOWED]))
                .await
                .unwrap();
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;

            eprintln!("firecracker: a fork of the running VM inherits it");
            rt.fork(&name, &fork).await.unwrap();
            let mut fspec = spec.clone();
            fspec.name = fork.clone();
            fspec.image = ImageSource::Existing;
            fspec.egress = EgressPolicy::default();
            rt.start(&fspec).await.expect("start fork");
            let fg = Guest {
                exec: rt.guest_exec(&fork).await.unwrap().unwrap(),
                root: "",
            };
            fg.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;
            fg.bypass_allowed_variant().await;

            eprintln!("firecracker: stop and start");
            rt.stop(&name).await.unwrap();
            rt.start(&spec).await.expect("restart");
            let g = Guest {
                exec: rt.guest_exec(&name).await.unwrap().unwrap(),
                root: "",
            };
            g.upload().await;
            g.expect(
                &[ALLOWED, DENIED],
                &format!("{ALLOWED}=blocked {DENIED}=open"),
            )
            .await;
        }))
        .await;
        let _ = rt.delete(&fork).await;
        let _ = rt.delete(&name).await;
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}
