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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::egress::{Net, Rules};
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
    v
}

fn rule(net: &Net, accept: bool) -> Value {
    let mut r = json!({"net": net.cidr(), "accept": accept});
    if !accept {
        r["reason"] = json!("user:deny");
    }
    r
}

/// [`RULES_FILE`] for `rules`: every allow first (an exception wins), then
/// every deny, then accept.
pub fn rules(rules: &Rules) -> Value {
    json!({
        "version": 1,
        "rules": rules.ordered().map(|(n, accept)| rule(n, accept)).collect::<Vec<_>>(),
        "default_accept": true,
    })
}

/// Writes [`RULES_FILE`] into `dir` atomically (the gateway reloads it).
pub fn write_rules(dir: &Path, r: &Rules) -> Result<()> {
    let path = dir.join(RULES_FILE);
    let tmp = dir.join(format!("{RULES_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&rules(r))?)?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| VmmError::other(format!("write the network rules {}: {e}", path.display())))
}

/// Whether the process at `exe` is a network gateway (or the gvproxy of a VM an
/// older cua started), so only those are ever signalled.
pub fn is_gateway(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("cua-netgw") || s.contains("gvproxy")
}

#[cfg(test)]
mod tests {
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
                    {"net": "10.1.0.0/16", "accept": true},
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
        });
        assert_eq!(c["rules"], RULES_FILE);
        assert_eq!(c["events"], EVENTS_FILE);
        assert_eq!(c["pid_file"], PID_FILE);
        assert_eq!(c["listen"], json!({"vfkit": "unixgram:///r/gv.sock"}));
        assert_eq!(c["host_ip"], "172.16.0.6");
        assert!(c.get("dhcp").is_none());
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
    fn only_gateways_are_signalled() {
        assert!(is_gateway(Path::new(
            "/Applications/Cua.app/Contents/MacOS/cua-netgw"
        )));
        assert!(is_gateway(Path::new("/Users/me/.cua/bin/gvproxy-v0.9.0")));
        assert!(!is_gateway(Path::new("/usr/bin/ssh")));
    }
}
