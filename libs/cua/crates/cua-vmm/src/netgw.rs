//! The Cua network gateway (`cua-netgw`, libs/cua/netgw): the user-mode
//! network of one VM on a host where the guest's traffic cannot be filtered
//! below the VMM without root (macOS microVMs today).
//!
//! The gateway is a build of gvisor-tap-vsock (Apache-2.0) with a policy
//! check on every TCP connection and UDP flow the guest opens, and its own
//! resolver, which drops DNS answers the rules refuse (rebinding). It runs
//! one process per VM, next to the monitor, and reads two files this module
//! writes in the VM's directory:
//!
//! * [`CONFIG_FILE`]: the network (subnet, addresses, the socket the VM's
//!   network device connects to), the rules file, the events file;
//! * [`RULES_FILE`]: the rules, in order, first match wins. The gateway
//!   reloads it when it changes, so [`write_rules`] applies a new policy to
//!   a running VM at once (open connections the new rules refuse are
//!   closed).
//!
//! Whatever the rules say, the gateway never dials the host's loopback, the
//! cloud metadata endpoints or its own network, and reaches the host only
//! through its host address and only for a rule naming that address
//! exactly. Refused connections are appended to [`EVENTS_FILE`] as JSON
//! lines (`{"t","event","reason","proto","dst"}`, never a name the guest
//! chose).
//!
//! A configuration's `tree` section ([`crate::tree::GatewayTree`]) makes the
//! gateway carry the guest's TCP to the other running VMs of its tree, and
//! theirs to it, through their ingress sockets. The same binary is the tree
//! relay inside a Linux VM's namespace (`cua-netgw -tree-relay`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::egress::{Rule, Rules};
use crate::error::{Result, VmmError};
use crate::host;

/// The gateway's executable.
pub const BIN: &str = if cfg!(windows) {
    "cua-netgw.exe"
} else {
    "cua-netgw"
};

/// The gateway's configuration, in the VM's directory.
pub const CONFIG_FILE: &str = "netgw.json";
/// The rules, in the VM's directory.
pub const RULES_FILE: &str = "netgw-rules.json";
/// Refused connections, one JSON line each, in the VM's directory.
pub const EVENTS_FILE: &str = "network-events.jsonl";
/// The gateway's process id, in the VM's directory.
pub const PID_FILE: &str = "netgw.pid";
/// The gateway's log (written by whoever starts it), in the VM's directory.
pub const LOG_FILE: &str = "netgw.log";

/// The point [`crate::egress::support`] names for backends behind the
/// gateway.
pub const POINT: &str = "the Cua network gateway, a user-mode network per VM on the host that \
     checks every connection the guest opens before making it";

/// `cua-netgw`: `$CUA_NETGW`, next to the running executable, next to
/// `cua-vz` (they ship together), `~/.cua/bin`, then `PATH`.
pub fn bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_NETGW").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let beside = |dir: Option<PathBuf>| dir.map(|d| d.join(BIN)).filter(|p| p.is_file());
    if let Some(p) = beside(
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    ) {
        return Some(p);
    }
    if let Some(p) = beside(
        std::env::var_os("CUA_VZ")
            .map(PathBuf::from)
            .and_then(|v| v.parent().map(Path::to_path_buf)),
    ) {
        return Some(p);
    }
    let own = host::cua_home().join("bin").join(BIN);
    if own.is_file() {
        return Some(own);
    }
    host::which(BIN)
}

/// The error when [`bin`] finds nothing: a VM with network never starts
/// without its gateway.
pub fn missing() -> VmmError {
    VmmError::missing(
        "cua-netgw",
        "the network gateway ships next to `cua`; set CUA_NETGW to its path or put it in \
         ~/.cua/bin (libs/cua/netgw/build.sh builds it)",
    )
}

/// One VM's network, for [`config`].
#[derive(Clone, Debug)]
pub struct Network<'a> {
    /// The virtual network (an IPv4 CIDR).
    pub subnet: &'a str,
    /// The gateway's address (and the guest's resolver).
    pub gateway_ip: &'a str,
    pub gateway_mac: &'a str,
    /// An address in `subnet` that stands for the host (its loopback),
    /// reachable only through a rule naming it exactly.
    pub host_ip: Option<&'a str>,
    /// Hand out addresses with DHCP (guests that ask for one).
    pub dhcp: bool,
    /// DHCP leases: address -> MAC address.
    pub static_leases: BTreeMap<String, String>,
    /// `unixgram://<path>` for a Virtualization.framework file-handle
    /// device.
    pub vfkit: Option<String>,
    /// `unix://<path>` or `tcp://127.0.0.1:<port>` for QEMU's
    /// `-netdev stream`.
    pub qemu: Option<String>,
    /// More addresses in `subnet` where the gateway answers DNS.
    pub dns_ips: Vec<String>,
    /// Published guest ports: `127.0.0.1:<host>` -> `<guest ip>:<port>`.
    pub forwards: Vec<(String, String)>,
    /// The readiness-probe endpoint ([`probe`]).
    pub control: Option<String>,
}

/// [`CONFIG_FILE`] for `net` (file names relative to the VM's directory).
pub fn config(net: &Network<'_>) -> Value {
    let mut listen = serde_json::Map::new();
    if let Some(v) = &net.vfkit {
        listen.insert("vfkit".into(), json!(v));
    }
    if let Some(q) = &net.qemu {
        listen.insert("qemu".into(), json!(q));
    }
    let mut v = json!({
        "subnet": net.subnet,
        "gateway_ip": net.gateway_ip,
        "gateway_mac": net.gateway_mac,
        "listen": listen,
        "rules": RULES_FILE,
        "events": EVENTS_FILE,
        "pid_file": PID_FILE,
        "log_level": "warning",
    });
    if let Some(h) = net.host_ip {
        v["host_ip"] = json!(h);
    }
    if net.dhcp {
        v["dhcp"] = json!(true);
    }
    if !net.static_leases.is_empty() {
        v["static_leases"] = json!(net.static_leases);
    }
    if !net.dns_ips.is_empty() {
        v["dns_ips"] = json!(net.dns_ips);
    }
    if !net.forwards.is_empty() {
        v["forwards"] = net
            .forwards
            .iter()
            .map(|(h, g)| json!({"host": h, "guest": g}))
            .collect();
    }
    if let Some(c) = &net.control {
        v["control"] = json!(c);
    }
    v
}

/// Starts the gateway of the VM whose directory is `dir` ([`CONFIG_FILE`]
/// and [`RULES_FILE`] written there), detached, its output in
/// [`LOG_FILE`]; returns its pid once it listens (its pid file appears).
/// It ends by itself when its VM disconnects, or when none connects within
/// two minutes.
pub async fn start(bin: &Path, dir: &Path, timeout: std::time::Duration) -> Result<u32> {
    let pid_file = dir.join(PID_FILE);
    let _ = std::fs::remove_file(&pid_file);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))?;
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-config")
        .arg(dir.join(CONFIG_FILE))
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(false);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| VmmError::other(format!("{}: {e}", bin.display())))?;
    let pid = child.id().unwrap_or_default();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if std::fs::read_to_string(&pid_file).is_ok_and(|p| !p.trim().is_empty()) {
            // Reaped by the runtime when it exits; never killed with us.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            return Ok(pid);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let log = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap_or_default();
            return Err(VmmError::other(format!(
                "the network gateway exited ({status}): {}",
                crate::exec::tail(&log, 2000)
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            return Err(VmmError::other(format!(
                "the network gateway did not listen within {timeout:?} (see {})",
                dir.join(LOG_FILE).display()
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Asks the gateway at `control` (`unix://<path>` or `tcp://<addr>`)
/// whether it can open a TCP connection to `guest` (`<ip>:<port>`): the
/// readiness of a published port, which the host side of a forward cannot
/// tell (the gateway accepts it before the guest does).
pub async fn probe(control: &str, guest: &str) -> Result<bool> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let line = format!("probe {guest}\n");
    let reply = async {
        let mut out = String::new();
        if let Some(path) = control.strip_prefix("unix://") {
            #[cfg(unix)]
            {
                let mut s = tokio::net::UnixStream::connect(path).await?;
                s.write_all(line.as_bytes()).await?;
                BufReader::new(s).read_line(&mut out).await?;
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(std::io::Error::other("unix sockets need a Unix host"));
            }
        } else {
            let addr = control.strip_prefix("tcp://").unwrap_or(control);
            let mut s = tokio::net::TcpStream::connect(addr).await?;
            s.write_all(line.as_bytes()).await?;
            BufReader::new(s).read_line(&mut out).await?;
        }
        Ok::<String, std::io::Error>(out)
    };
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
        .await
        .map_err(|_| VmmError::other("the network gateway did not answer a probe"))?
        .map_err(|e| VmmError::other(format!("the network gateway ({control}): {e}")))?;
    Ok(out.trim() == "ok")
}

fn rule(r: &Rule) -> Value {
    let mut v = json!({
        "net": r.net.cidr(),
        "accept": r.accept,
        "reason": serde_json::to_value(r.reason).unwrap_or(Value::Null),
    });
    if let Some(p) = &r.ports {
        v["ports"] = json!(p);
    }
    if let Some(p) = r.proto {
        v["proto"] = json!(p.as_str());
    }
    v
}

/// [`RULES_FILE`] for `rules`, in their order (first match wins), then
/// accept. A rule for "every address local to the host" is left out: the
/// gateway refuses the host's own addresses itself, and opens its host
/// address only for a rule naming it exactly.
pub fn rules(rules: &Rules) -> Value {
    json!({
        "version": 1,
        "rules": rules
            .rules
            .iter()
            .filter(|r| !r.host_local)
            .map(rule)
            .collect::<Vec<_>>(),
        "default_accept": true,
    })
}

/// [`rules`] with rules by name: the gateway is the guest's resolver, so
/// it refuses a denied name and lets an allowed name's answers through.
pub fn rules_named(r: &Rules, domains: &crate::egress::Domains) -> Value {
    let mut v = rules(r);
    if !domains.is_empty() {
        v["domains"] = json!(domains);
    }
    v
}

/// Writes [`RULES_FILE`] into `dir` atomically (the gateway reloads it).
pub fn write_rules(dir: &Path, r: &Rules) -> Result<()> {
    write_rules_named(dir, r, &Default::default())
}

/// [`write_rules`] with rules by name ([`rules_named`]).
pub fn write_rules_named(dir: &Path, r: &Rules, domains: &crate::egress::Domains) -> Result<()> {
    let path = dir.join(RULES_FILE);
    let tmp = dir.join(format!("{RULES_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&rules_named(r, domains))?)?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| VmmError::other(format!("write the network rules {}: {e}", path.display())))?;
    // The gateway also looks for a change every quarter second; telling it
    // makes the new rules hold for the very next connection.
    reload(dir);
    Ok(())
}

/// The gateway process of the VM in `dir`, from its pid file. Only Unix
/// signals it (Windows has no SIGHUP; its gateway polls the rules).
#[cfg(any(unix, test))]
fn gateway_pid(dir: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(dir.join(PID_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (pid > 1).then_some(pid)
}

/// Whether process `pid` is a `cua-netgw` (a stale pid file must never make
/// us signal a process that reused the number).
#[cfg(unix)]
fn is_gateway_pid(pid: i32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("cua-netgw"))
        .unwrap_or(false)
}

/// Whether the gateway of the VM in `dir` is running: its pid file names a
/// live `cua-netgw`. Always true where gateways are not signalled (Windows).
pub fn gateway_running(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        gateway_pid(dir).is_some_and(is_gateway_pid)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        true
    }
}

/// Asks the gateway of the VM in `dir` to read its rules now (SIGHUP). Best
/// effort: no gateway, or one already gone, is fine.
fn reload(dir: &Path) {
    #[cfg(unix)]
    if let Some(pid) = gateway_pid(dir).filter(|p| is_gateway_pid(*p)) {
        // SAFETY: kill(2) with a pid and SIGHUP touches no memory of ours.
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The connections the gateway of the VM in `dir` refused (and the DNS
/// answers it dropped), from the end of its [`EVENTS_FILE`] (at most the
/// last 256 KiB), merged by reason and destination, most recent first.
pub fn read_events(dir: &Path) -> Vec<crate::egress::NetworkEvent> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    const TAIL: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(dir.join(EVENTS_FILE)) else {
        return vec![];
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > TAIL {
        let _ = f.seek(SeekFrom::Start(len - TAIL));
    }
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let events = text
        .lines()
        // A first line cut by the tail does not parse and is skipped.
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| crate::egress::NetworkEvent {
            reason: v["reason"].as_str().unwrap_or("refused").to_string(),
            dst: v["dst"].as_str().unwrap_or_default().to_string(),
            proto: v["proto"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(str::to_string),
            count: 1 + v["repeats"].as_u64().unwrap_or(0),
            last_unix: v["t"].as_str().and_then(rfc3339_utc),
        })
        .collect();
    crate::egress::merge_events(events)
}

/// Unix seconds of a UTC RFC 3339 time (`2026-10-27T03:04:05.123Z`), as the
/// gateway writes them.
fn rfc3339_utc(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.split('.').next()?;
    let mut t = time.split(':').map(|x| x.parse::<i64>().ok());
    let (h, mi, sec) = (t.next()??, t.next()??, t.next()??);
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + mi * 60 + sec).ok()
}

/// Whether the process at `exe` is a network gateway (or the gvproxy of a VM an
/// older cua started), so only those are ever signalled.
pub fn is_gateway(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("cua-netgw") || s.contains("gvproxy")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_gateway_pid_comes_from_its_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(gateway_pid(dir.path()), None);
        std::fs::write(dir.path().join(PID_FILE), "4242\n").unwrap();
        assert_eq!(gateway_pid(dir.path()), Some(4242));
        // Never pid 0 or 1 (a signal to a group or to init).
        std::fs::write(dir.path().join(PID_FILE), "1").unwrap();
        assert_eq!(gateway_pid(dir.path()), None);
        std::fs::write(dir.path().join(PID_FILE), "not a pid").unwrap();
        assert_eq!(gateway_pid(dir.path()), None);
    }

    use super::*;
    use crate::egress::EgressPolicy;

    #[test]
    fn the_rules_file_keeps_the_policy_order() {
        let r = EgressPolicy {
            allow: vec!["10.1.0.0/16".into()],
            deny: vec!["10.0.0.0/8".into(), "all".into()],
        }
        .parse()
        .unwrap();
        assert_eq!(
            rules(&r),
            json!({
                "version": 1,
                "rules": [
                    {"net": "10.1.0.0/16", "accept": true, "reason": "user:allow"},
                    {"net": "10.0.0.0/8", "accept": false, "reason": "user:deny"},
                    {"net": "0.0.0.0/0", "accept": false, "reason": "user:deny"},
                    {"net": "::/0", "accept": false, "reason": "user:deny"},
                ],
                "default_accept": true,
            })
        );
        assert_eq!(
            rules(&Rules::default()),
            json!({"version": 1, "rules": [], "default_accept": true})
        );
    }

    #[test]
    fn the_config_names_the_files_beside_it() {
        let c = config(&Network {
            subnet: "172.16.0.0/29",
            gateway_ip: "172.16.0.1",
            gateway_mac: "5a:94:ef:e4:0c:dd",
            host_ip: Some("172.16.0.6"),
            dhcp: false,
            static_leases: BTreeMap::new(),
            vfkit: Some("unixgram:///r/gv.sock".into()),
            qemu: None,
            dns_ips: vec![],
            forwards: vec![],
            control: None,
        });
        assert_eq!(c["rules"], RULES_FILE);
        assert_eq!(c["events"], EVENTS_FILE);
        assert_eq!(c["pid_file"], PID_FILE);
        assert_eq!(c["listen"], json!({"vfkit": "unixgram:///r/gv.sock"}));
        assert_eq!(c["host_ip"], "172.16.0.6");
        assert!(c.get("dhcp").is_none());
        assert!(c.get("forwards").is_none() && c.get("control").is_none());
        let q = config(&Network {
            subnet: "10.0.2.0/24",
            gateway_ip: "10.0.2.2",
            gateway_mac: "5a:94:ef:e4:0c:dd",
            host_ip: Some("10.0.2.2"),
            dhcp: true,
            static_leases: [("10.0.2.15".to_string(), "52:54:00:12:34:56".to_string())].into(),
            vfkit: None,
            qemu: Some("unix:///t/q.sock".into()),
            dns_ips: vec!["10.0.2.3".into()],
            forwards: vec![("127.0.0.1:50001".into(), "10.0.2.15:3211".into())],
            control: Some("unix:///t/c.sock".into()),
        });
        assert_eq!(q["listen"], json!({"qemu": "unix:///t/q.sock"}));
        assert_eq!(q["dhcp"], true);
        assert_eq!(
            q["static_leases"],
            json!({"10.0.2.15": "52:54:00:12:34:56"})
        );
        assert_eq!(q["dns_ips"], json!(["10.0.2.3"]));
        assert_eq!(
            q["forwards"],
            json!([{"host": "127.0.0.1:50001", "guest": "10.0.2.15:3211"}])
        );
        assert_eq!(q["control"], "unix:///t/c.sock");
    }

    #[test]
    fn rules_are_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();
        write_rules(dir.path(), &Rules::default()).unwrap();
        let deny = EgressPolicy {
            allow: vec![],
            deny: vec!["192.168.0.0/16".into()],
        }
        .parse()
        .unwrap();
        write_rules(dir.path(), &deny).unwrap();
        let v: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(RULES_FILE)).unwrap()).unwrap();
        assert_eq!(v["rules"][0]["net"], "192.168.0.0/16");
        assert!(!dir.path().join(format!("{RULES_FILE}.tmp")).exists());
    }

    #[test]
    fn refused_connections_are_read_back_merged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(EVENTS_FILE),
            "{\"t\":\"2026-10-27T03:04:05.5Z\",\"event\":\"blocked\",\"reason\":\"platform:metadata\",\"proto\":\"tcp\",\"dst\":\"169.254.169.254:80\"}\n\
             {\"t\":\"2026-10-27T03:04:09Z\",\"event\":\"blocked\",\"reason\":\"platform:metadata\",\"proto\":\"tcp\",\"dst\":\"169.254.169.254:80\",\"repeats\":2}\n\
             not json\n",
        )
        .unwrap();
        let ev = read_events(dir.path());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].count, 4);
        assert_eq!(ev[0].reason, "platform:metadata");
        assert_eq!(ev[0].last_unix, rfc3339_utc("2026-10-27T03:04:09Z"));
        assert_eq!(rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert!(read_events(&dir.path().join("none")).is_empty());
    }

    #[test]
    fn only_gateways_are_signalled() {
        assert!(is_gateway(Path::new(
            "/Applications/Cua.app/Contents/MacOS/cua-netgw"
        )));
        assert!(is_gateway(Path::new("/Users/me/.cua/bin/gvproxy-v0.9.0")));
        assert!(!is_gateway(Path::new("/usr/bin/ssh")));
    }
}
