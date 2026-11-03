//! The host-guest protocol of a Cua microVM, shared by the guest init
//! (`cua-microvm-init`, PID 1 in the VM) and the host runtime
//! (`cua_vmm::microvm`).
//!
//! The init listens on vsock port [`PORT`]. The host reaches it through
//! Firecracker's vsock Unix socket (`CONNECT <port>\n`, answered with
//! `OK <n>\n`). Each connection carries one [`Request`] as a JSON line; the
//! guest answers with JSON lines: one [`Reply`] for most requests, a stream
//! of [`ExecFrame`]s ending with [`ExecFrame::Exit`] for [`Request::Exec`].
//!
//! The listening socket survives a snapshot: a VM restored from a snapshot
//! answers on the same port, which is how a restored (or forked) VM gets its
//! own identity ([`Request::Identity`]: token, hostname, entropy, clock).
//!
//! Guest-side constants ([`GUEST_IP`], [`GATEWAY_IP`]) are identical in every
//! VM so snapshots restore anywhere: each VM has its own network namespace
//! on the host.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The vsock port the init listens on.
pub const PORT: u32 = 1024;

/// The guest's address on its point-to-point link (the same in every VM).
pub const GUEST_IP: &str = "172.16.0.2";
/// The host side of the link (the guest's gateway).
pub const GATEWAY_IP: &str = "172.16.0.1";
/// Prefix length of the link.
pub const PREFIX_LEN: u8 = 30;
/// The DNS address the guest uses; the host forwards it to its resolver.
pub const DNS_IP: &str = "172.16.0.1";
/// The guest's MAC address (the same in every VM, like its IP).
pub const GUEST_MAC: &str = "06:00:ac:10:00:02";

/// A file at the image's root listing extended attributes its files must
/// carry (`[{"path","name","value"(base64)}]`), for images built where the
/// squashfs writer keeps none (macOS). The init sets them at boot and
/// removes the file.
pub const XATTRS_FILE: &str = ".cua-microvm-xattrs.json";

/// One entry of [`XATTRS_FILE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Xattr {
    /// Path relative to the image's root.
    pub path: String,
    /// Attribute name (`security.capability`, ...).
    pub name: String,
    /// Base64 value.
    pub value: String,
}

/// Where the init writes the Spaces token (`ensure-env-token.sh`'s
/// location: cua-spacesd follows it in await-token-file mode).
pub const TOKEN_FILE: &str = "/run/cua/env-token";

/// A request from the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness. Answered with [`Reply::ok`].
    Ping,
    /// The workload to run, sent once after a cold boot. The init starts it
    /// and answers once it is spawned.
    Boot(BootConfig),
    /// This VM's identity, sent after a cold boot and after every restore
    /// from a snapshot (where the VM is a copy of another).
    Identity(Identity),
    /// Whether a TCP port is listening in the guest (`/proc/net/tcp*`).
    Listening {
        /// The port.
        port: u16,
    },
    /// Runs `/bin/sh -c script` and streams its output.
    Exec(Exec),
    /// Flushes filesystem buffers (before a snapshot or a disk copy).
    Sync,
    /// Connects to `127.0.0.1:port` in the guest and, once [`Reply::ok`]
    /// is sent, carries raw bytes both ways on this connection: a published
    /// port reached over vsock (the macOS microVM host forwards its
    /// loopback ports this way, so they work with or without a network).
    Forward {
        /// The guest port.
        port: u16,
    },
    /// Stops the workload (TERM, then KILL after `grace_secs`) and powers
    /// the VM off.
    Shutdown {
        /// Seconds the workload has to exit after SIGTERM.
        #[serde(default = "default_grace")]
        grace_secs: u64,
    },
}

fn default_grace() -> u64 {
    10
}

/// The workload of a cold boot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootConfig {
    /// argv (the image's ENTRYPOINT + CMD, or the Space's command).
    pub argv: Vec<String>,
    /// Environment (`KEY=value` pairs, in order).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Working directory (`/` when empty).
    #[serde(default)]
    pub workdir: Option<String>,
    /// `user`, `uid`, `user:group` or `uid:gid` (root when empty).
    #[serde(default)]
    pub user: Option<String>,
    /// Identity to apply before the workload starts.
    #[serde(default)]
    pub identity: Option<Identity>,
}

/// Per-instance identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Hostname.
    #[serde(default)]
    pub hostname: Option<String>,
    /// The Spaces token, written to [`TOKEN_FILE`] (root:`token_group`
    /// 0640). `None` leaves the file alone.
    #[serde(default)]
    pub token: Option<String>,
    /// Group that may read the token file (default `cua`, else root).
    #[serde(default)]
    pub token_group: Option<String>,
    /// Hex bytes mixed into the guest's entropy pool (a restored VM has
    /// the same random state as its snapshot).
    #[serde(default)]
    pub entropy: Option<String>,
    /// Wall-clock time to set (Unix nanoseconds): a restored VM's clock
    /// is the snapshot's.
    #[serde(default)]
    pub unix_nanos: Option<u64>,
    /// `/etc/machine-id` (32 lowercase hex digits), also written to
    /// D-Bus's copy. An older host leaves it out; the init then derives
    /// one from `entropy`.
    #[serde(default)]
    pub machine_id: Option<String>,
    /// Extra environment for later `exec` requests.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A command for [`Request::Exec`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exec {
    /// Shell script for `/bin/sh -c`.
    pub script: String,
    /// Extra environment.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Base64 bytes fed on stdin (stdin is empty otherwise).
    #[serde(default)]
    pub stdin: Option<String>,
    /// Run as this user (root when empty).
    #[serde(default)]
    pub user: Option<String>,
    /// Kill the command after this many milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// The answer to every request but [`Request::Exec`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// Whether the request succeeded.
    pub ok: bool,
    /// Why it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// [`Request::Listening`]: the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<bool>,
    /// [`Request::Ping`]: the init's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// [`Request::Ping`]: whether the workload was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booted: Option<bool>,
}

impl Reply {
    /// Success.
    pub fn ok() -> Self {
        Self {
            ok: true,
            ..Default::default()
        }
    }

    /// Failure with a reason.
    pub fn err(e: impl std::fmt::Display) -> Self {
        Self {
            ok: false,
            error: Some(e.to_string()),
            ..Default::default()
        }
    }
}

/// One line of an [`Request::Exec`] answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecFrame {
    /// Base64 stdout bytes.
    Stdout(String),
    /// Base64 stderr bytes.
    Stderr(String),
    /// The exit code (128 + signal when killed); always the last frame.
    Exit(i64),
    /// The command could not run; always the last frame.
    Error(String),
}

/// The init's version, reported by [`Request::Ping`].
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Splits `user[:group]` into its parts.
pub fn split_user(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((u, g)) => (u, Some(g)),
        None => (spec, None),
    }
}

/// Looks up `name` (or a numeric id) in an `/etc/passwd`-format text:
/// `(uid, gid, home)`.
pub fn passwd_lookup(passwd: &str, name: &str) -> Option<(u32, u32, String)> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 {
            continue;
        }
        if f[0] == name || f[2] == name {
            return Some((f[2].parse().ok()?, f[3].parse().ok()?, f[5].to_string()));
        }
    }
    name.parse::<u32>().ok().map(|uid| (uid, uid, "/".into()))
}

/// Looks up a group (name or numeric id) in an `/etc/group`-format text.
pub fn group_lookup(group: &str, name: &str) -> Option<u32> {
    for line in group.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 3 && (f[0] == name || f[2] == name) {
            return f[2].parse().ok();
        }
    }
    name.parse().ok()
}

/// Whether `/proc/net/tcp`-format `table` has a socket listening on
/// `port` (state `0A`).
pub fn table_listens(table: &str, port: u16) -> bool {
    let want = format!(":{port:04X}");
    table.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 3 && f[1].ends_with(&want) && f[3] == "0A"
    })
}

/// `(blocks, block size)` of the ext4 filesystem whose superblock is `sb`
/// (the 1024 bytes at offset 1024 of the device), `None` when it is not
/// ext2/3/4.
pub fn ext4_size(sb: &[u8]) -> Option<(u64, u64)> {
    let u32_at =
        |o: usize| -> Option<u32> { Some(u32::from_le_bytes(sb.get(o..o + 4)?.try_into().ok()?)) };
    let magic = u16::from_le_bytes(sb.get(0x38..0x3a)?.try_into().ok()?);
    if magic != 0xEF53 {
        return None;
    }
    let log = u32_at(0x18)?;
    if log > 6 {
        return None;
    }
    let block = 1024u64 << log;
    let mut blocks = u64::from(u32_at(0x04)?);
    // INCOMPAT_64BIT: the count's high half is at 0x150.
    if u32_at(0x60)? & 0x80 != 0 {
        blocks |= u64::from(u32_at(0x150)?) << 32;
    }
    Some((blocks, block))
}

/// The block count an ext4 filesystem of `blocks` blocks of `block` bytes
/// grows to on a device of `device_bytes`, when the device is larger.
pub fn ext4_grow_to(blocks: u64, block: u64, device_bytes: u64) -> Option<u64> {
    let fits = device_bytes / block.max(1);
    (fits > blocks).then_some(fits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn superblock(blocks: u64, log: u32, bit64: bool) -> Vec<u8> {
        let mut sb = vec![0u8; 1024];
        sb[0x04..0x08].copy_from_slice(&(blocks as u32).to_le_bytes());
        sb[0x18..0x1c].copy_from_slice(&log.to_le_bytes());
        sb[0x38..0x3a].copy_from_slice(&0xEF53u16.to_le_bytes());
        if bit64 {
            sb[0x60..0x64].copy_from_slice(&0x80u32.to_le_bytes());
            sb[0x150..0x154].copy_from_slice(&((blocks >> 32) as u32).to_le_bytes());
        }
        sb
    }

    #[test]
    fn ext4_superblocks_give_the_size_and_growth() {
        assert_eq!(
            ext4_size(&superblock(262_144, 2, true)),
            Some((262_144, 4096))
        );
        assert_eq!(
            ext4_size(&superblock(5u64 << 32, 2, true)),
            Some((5u64 << 32, 4096))
        );
        // Without INCOMPAT_64BIT the high half is ignored.
        let mut sb = superblock(1000, 0, false);
        sb[0x150..0x154].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(ext4_size(&sb), Some((1000, 1024)));
        assert_eq!(ext4_size(&[0u8; 1024]), None, "no magic");
        assert_eq!(ext4_size(&[0u8; 16]), None, "too short");
        // A 1 GiB filesystem on a 16 GiB device grows to 16 GiB; never
        // shrinks.
        assert_eq!(ext4_grow_to(262_144, 4096, 16 << 30), Some(4_194_304));
        assert_eq!(ext4_grow_to(262_144, 4096, 1 << 30), None);
        assert_eq!(ext4_grow_to(262_144, 4096, 512 << 20), None);
    }

    #[test]
    fn forward_requests_are_tagged() {
        let r = Request::Forward { port: 3211 };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"op":"forward","port":3211}"#);
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
    }

    #[test]
    fn requests_are_tagged_json_lines() {
        let r = Request::Listening { port: 3211 };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"op":"listening","port":3211}"#);
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
        let s = r#"{"op":"shutdown"}"#;
        assert_eq!(
            serde_json::from_str::<Request>(s).unwrap(),
            Request::Shutdown { grace_secs: 10 }
        );
        let boot = Request::Boot(BootConfig {
            argv: vec!["/bin/true".into()],
            identity: Some(Identity {
                token: Some("t".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let s = serde_json::to_string(&boot).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), boot);
    }

    #[test]
    fn exec_frames_round_trip() {
        for f in [
            ExecFrame::Stdout("aGk=".into()),
            ExecFrame::Exit(3),
            ExecFrame::Error("no sh".into()),
        ] {
            let s = serde_json::to_string(&f).unwrap();
            assert_eq!(serde_json::from_str::<ExecFrame>(&s).unwrap(), f);
        }
        assert_eq!(
            serde_json::to_string(&ExecFrame::Exit(0)).unwrap(),
            r#"{"exit":0}"#
        );
    }

    #[test]
    fn users_and_groups_resolve_by_name_or_id() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\ncua:x:1000:1000::/home/cua:/bin/bash\n";
        assert_eq!(
            passwd_lookup(passwd, "cua"),
            Some((1000, 1000, "/home/cua".into()))
        );
        assert_eq!(passwd_lookup(passwd, "0"), Some((0, 0, "/root".into())));
        assert_eq!(
            passwd_lookup(passwd, "4242"),
            Some((4242, 4242, "/".into()))
        );
        assert_eq!(passwd_lookup(passwd, "nobody"), None);
        let group = "root:x:0:\ncua:x:1000:\n";
        assert_eq!(group_lookup(group, "cua"), Some(1000));
        assert_eq!(group_lookup(group, "missing"), None);
        assert_eq!(split_user("cua:audio"), ("cua", Some("audio")));
        assert_eq!(split_user("1000"), ("1000", None));
    }

    #[test]
    fn listening_sockets_are_found_in_proc_net_tcp() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue\n\
           0: 00000000:0C8B 00000000:0000 0A 00000000:00000000 00:00000000 00000000\n\
           1: 0100007F:1F90 0100007F:9C40 01 00000000:00000000 00:00000000 00000000\n";
        assert!(table_listens(table, 3211));
        assert!(!table_listens(table, 8080), "established, not listening");
        assert!(!table_listens(table, 22));
    }
}
