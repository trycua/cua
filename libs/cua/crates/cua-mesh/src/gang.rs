// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Rank traffic of a job gang between one account's machines (CUA-1783).
//!
//! A gang member listens on a TCP port on its own machine (`127.0.0.1`),
//! or in a Space this machine runs (at the Space's own address, which the
//! registration names as its target). Its peers on other machines reach
//! that port through a [`Mesh::forward`] to the service
//! [`proto::port_service`] on its machine: direct when the mesh finds a
//! path (the LAN, or across NATs by hole punching), else through the
//! coordinator's relay, end to end encrypted either way. A member in a
//! Space reaches its peers' forwards at its gateway address on this
//! machine (the control request's `bind`).
//!
//! On a machine two processes take part, and this module is what they
//! share:
//!
//! - **The port registry** ([`PortRegistry`], `<cua home>/jobs/gang-ports/`):
//!   the job runner registers a gang member's port for as long as the
//!   member runs. The machine's mesh node lets a peer reach a port only
//!   while it is registered by a live process, and only for a peer the
//!   coordinator marked as another machine of this machine's own account
//!   ([`proto::ROLE_MACHINE`]).
//! - **The control socket** ([`serve_control`], [`ControlClient`],
//!   `<cua home>/host/mesh-control.sock`, `0600`, same user only): the job
//!   runner asks the machine's mesh node for outbound forwards, one JSON
//!   line per request:
//!
//!   ```text
//!   -> {"forward": {"machine": "<machine id>", "port": 47012}}
//!   -> {"forward": {"machine": "<machine id>", "port": 47012, "bind": "192.168.64.1"}}
//!   <- {"local": "127.0.0.1:53211", "path": "direct", "peer": "<name>"}
//!   <- {"error": "..."}
//!   ```
//!
//!   `bind` (a private or loopback address of this machine; default
//!   loopback) is where the forward listens.
//!
//!   A forward lives until the requester closes its control connection
//!   (or the node stops).

use std::io;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::Path;
use crate::node::Mesh;
use crate::proto;

/// The control socket's file name, in `<cua home>/host/`.
pub const CONTROL_SOCKET: &str = "mesh-control.sock";

/// The port registry's directory name, in `<cua home>/jobs/`.
pub const PORTS_DIR: &str = "gang-ports";

/// How long a new forward waits for the mesh to find a direct path before
/// it answers (traffic keeps looking afterwards).
pub const DIRECT_WAIT: Duration = Duration::from_secs(5);

/// How long the control socket waits for a machine to be in this node's
/// map (a gang's machines were just placed together, but one may have
/// joined the map a moment ago).
pub const MACHINE_WAIT: Duration = Duration::from_secs(15);

/// The control socket of the machine whose Cua home is `home`.
pub fn control_path(home: &FsPath) -> PathBuf {
    home.join("host").join(CONTROL_SOCKET)
}

/// The port registry of the machine whose Cua home is `home`.
pub fn ports_dir(home: &FsPath) -> PathBuf {
    home.join("jobs").join(PORTS_DIR)
}

/// Ports gang members on this machine listen on, while they run. One file
/// per port (`<dir>/<port>`, `0600`) naming the registering process and
/// what it is for.
#[derive(Debug, Clone)]
pub struct PortRegistry {
    dir: PathBuf,
}

/// Whether a forward may listen on `ip`, or a registered port's target be
/// `ip`: this machine's loopback, or a private network (a Space's NAT or
/// bridge); never a public or tailnet address.
pub fn private_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
        }
    }
}

/// A registered port; unregistered on drop.
#[derive(Debug)]
pub struct Registration {
    path: PathBuf,
    /// The port.
    pub port: u16,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl PortRegistry {
    /// The registry in `dir` ([`ports_dir`]).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn file(&self, port: u16) -> PathBuf {
        self.dir.join(port.to_string())
    }

    /// Registers `port` for this process (`label` says what for, e.g.
    /// `run-1 attempt 2 rank 0`). Fails with `AlreadyExists` while another
    /// live process holds it; a registration a dead process left is taken
    /// over.
    pub fn register(&self, port: u16, label: &str) -> io::Result<Registration> {
        self.register_to(port, label, None)
    }

    /// [`PortRegistry::register`] for a member that listens at `target`
    /// (`<its Space's address>:<port>`) instead of `127.0.0.1:<port>`:
    /// this machine's mesh node connects a peer's stream there. `target`
    /// must be a private address ([`private_address`]).
    pub fn register_to(
        &self,
        port: u16,
        label: &str,
        target: Option<SocketAddr>,
    ) -> io::Result<Registration> {
        if let Some(t) = target
            && !private_address(t.ip())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a gang port's target must be a private address, not {t}"),
            ));
        }
        create_private_dir(&self.dir)?;
        let path = self.file(port);
        if let Some(holder) = self.holder(port)
            && holder != std::process::id()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("port {port} is registered by process {holder}"),
            ));
        }
        let label: String = label
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        let mut text = format!("{} {label}\n", std::process::id());
        if let Some(t) = target {
            text.push_str(&format!("target {t}\n"));
        }
        write_private(&path, text.as_bytes())?;
        Ok(Registration { path, port })
    }

    /// Where a peer's stream to `port` goes, while a live process holds
    /// it: its registered target, else `127.0.0.1:<port>`.
    pub fn target(&self, port: u16) -> Option<SocketAddr> {
        self.holder(port)?;
        let text = std::fs::read_to_string(self.file(port)).ok()?;
        let named = text
            .lines()
            .skip(1)
            .find_map(|l| l.strip_prefix("target "))
            .and_then(|t| t.trim().parse::<SocketAddr>().ok())
            .filter(|t| private_address(t.ip()));
        Some(named.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], port))))
    }

    /// The live process holding `port`, if any.
    fn holder(&self, port: u16) -> Option<u32> {
        let text = std::fs::read_to_string(self.file(port)).ok()?;
        let pid: u32 = text.split_whitespace().next()?.parse().ok()?;
        alive(pid).then_some(pid)
    }

    /// What `port` is registered for, while a live process holds it.
    pub fn registered(&self, port: u16) -> Option<String> {
        self.holder(port)?;
        let text = std::fs::read_to_string(self.file(port)).ok()?;
        Some(
            text.lines()
                .next()
                .unwrap_or_default()
                .trim_end()
                .split_once(' ')
                .map(|(_, l)| l.to_owned())
                .unwrap_or_default(),
        )
    }

    /// Waits up to `within` for `port` to be registered (a gang member on
    /// this machine may start a moment after its peers).
    pub async fn wait_registered(&self, port: u16, within: Duration) -> Option<String> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(l) = self.registered(port) {
                return Some(l);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

fn create_private_dir(dir: &FsPath) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &FsPath, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// A control request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRequest {
    /// A forward to `port` on machine `machine`.
    Forward {
        /// The machine id.
        machine: String,
        /// Its local TCP port (a gang member's).
        port: u16,
        /// Where the forward listens (default loopback): a private
        /// address of this machine ([`private_address`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bind: Option<std::net::IpAddr>,
    },
}

/// A control reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlReply {
    /// Where to connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<SocketAddr>,
    /// `direct` or `relayed`, when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The peer's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// Why there is no forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `direct` or `relayed`.
pub fn path_word(p: &Path) -> &'static str {
    match p {
        Path::Direct(_) => "direct",
        Path::Relay => "relayed",
    }
}

/// A forward the control socket made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardInfo {
    /// Where to connect.
    pub local: SocketAddr,
    /// `direct` or `relayed`, when it answered.
    pub path: String,
    /// The peer's display name.
    pub peer: String,
}

/// The job runner's side of the control socket. Forwards live as long as
/// this connection.
#[cfg(unix)]
#[derive(Debug)]
pub struct ControlClient {
    stream: tokio::io::BufReader<tokio::net::UnixStream>,
}

#[cfg(unix)]
impl ControlClient {
    /// Connects to the control socket at `path` ([`control_path`]).
    pub async fn connect(path: &FsPath) -> io::Result<Self> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Self {
            stream: tokio::io::BufReader::new(stream),
        })
    }

    /// Asks for a loopback forward to `port` on `machine`.
    pub async fn forward(&mut self, machine: &str, port: u16) -> Result<ForwardInfo, String> {
        self.forward_on(machine, port, None).await
    }

    /// Asks for a forward to `port` on `machine` listening on `bind` (an
    /// address of this machine a Space reaches it at; `None`: loopback).
    pub async fn forward_on(
        &mut self,
        machine: &str,
        port: u16,
        bind: Option<std::net::IpAddr>,
    ) -> Result<ForwardInfo, String> {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        let req = ControlRequest::Forward {
            machine: machine.to_owned(),
            port,
            bind,
        };
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stream
            .get_mut()
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("mesh control: {e}"))?;
        let mut reply = String::new();
        let wait = MACHINE_WAIT + DIRECT_WAIT + Duration::from_secs(10);
        let n = tokio::time::timeout(wait, self.stream.read_line(&mut reply))
            .await
            .map_err(|_| "mesh control: no answer".to_string())?
            .map_err(|e| format!("mesh control: {e}"))?;
        if n == 0 {
            return Err("mesh control: the mesh node closed the connection".into());
        }
        let reply: ControlReply =
            serde_json::from_str(reply.trim()).map_err(|e| format!("mesh control: {e}"))?;
        match (reply.error, reply.local) {
            (Some(e), _) => Err(e),
            (None, Some(local)) => Ok(ForwardInfo {
                local,
                path: reply.path.unwrap_or_default(),
                peer: reply.peer.unwrap_or_default(),
            }),
            (None, None) => Err("mesh control: an empty answer".into()),
        }
    }
}

/// Serves the control socket at `path` for `mesh` until the returned task
/// is aborted (the node stops). The socket is `0600` in a directory only
/// this user can write to, and each connection's peer must run as this
/// process's user.
#[cfg(unix)]
pub fn serve_control(mesh: Mesh, path: &FsPath) -> io::Result<tokio::task::JoinHandle<()>> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let path = path.to_owned();
    Ok(tokio::spawn(async move {
        let _cleanup = RemoveOnDrop(path);
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            match stream.peer_cred() {
                Ok(c) if c.uid() == me => {}
                other => {
                    tracing::warn!(
                        ?other,
                        "mesh control: refused a connection from another user"
                    );
                    continue;
                }
            }
            tokio::spawn(control_session(mesh.clone(), stream));
        }
    }))
}

#[cfg(unix)]
struct RemoveOnDrop(PathBuf);

#[cfg(unix)]
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(unix)]
async fn control_session(mesh: Mesh, stream: tokio::net::UnixStream) {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let (r, mut w) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(r).lines();
    // Held until the requester hangs up.
    let mut held = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > 4096 {
            return;
        }
        let reply = match serde_json::from_str::<ControlRequest>(&line) {
            Ok(ControlRequest::Forward {
                machine,
                port,
                bind,
            }) => match forward_on(&mesh, &machine, port, bind).await {
                Ok((fwd, reply)) => {
                    held.push(fwd);
                    reply
                }
                Err(e) => ControlReply {
                    error: Some(e),
                    ..Default::default()
                },
            },
            Err(e) => ControlReply {
                error: Some(format!("bad request: {e}")),
                ..Default::default()
            },
        };
        let mut out = serde_json::to_string(&reply).expect("json");
        out.push('\n');
        if w.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// A forward to `port` on `machine`, once the mesh had a moment to find a
/// direct path.
pub async fn forward(
    mesh: &Mesh,
    machine: &str,
    port: u16,
) -> Result<(crate::Forward, ControlReply), String> {
    forward_on(mesh, machine, port, None).await
}

/// [`forward`] listening on `bind` (a private address of this machine;
/// `None`: loopback).
pub async fn forward_on(
    mesh: &Mesh,
    machine: &str,
    port: u16,
    bind: Option<std::net::IpAddr>,
) -> Result<(crate::Forward, ControlReply), String> {
    if port == 0 {
        return Err("port 0".into());
    }
    let bind = bind.unwrap_or(std::net::Ipv4Addr::LOCALHOST.into());
    if !private_address(bind) {
        return Err(format!(
            "a forward listens on a private address of this machine, not {bind}"
        ));
    }
    let peer = mesh
        .wait_machine(machine, MACHINE_WAIT)
        .await
        .ok_or_else(|| format!("machine {machine} is not on this machine's mesh"))?;
    if peer.kind != proto::NodeKind::Machine {
        return Err(format!("{machine} is not a machine"));
    }
    let fwd = mesh
        .forward_on(peer.node_key, &proto::port_service(port), None, bind)
        .await
        .map_err(|e| e.to_string())?;
    let path = mesh.warm(&peer.node_key, DIRECT_WAIT).await;
    let reply = ControlReply {
        local: Some(fwd.local),
        path: path.as_ref().map(|p| path_word(p).to_owned()),
        peer: Some(peer.name.clone()),
        error: None,
    };
    Ok((fwd, reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_registered_while_its_registration_lives() {
        let dir = tempfile::tempdir().unwrap();
        let reg = PortRegistry::new(dir.path().join("gang-ports"));
        assert_eq!(reg.registered(47012), None);
        let r = reg.register(47012, "run-1 attempt 1 rank 0").unwrap();
        assert_eq!(
            reg.registered(47012).as_deref(),
            Some("run-1 attempt 1 rank 0")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &FsPath| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("gang-ports")), 0o700);
            assert_eq!(mode(&dir.path().join("gang-ports/47012")), 0o600);
        }
        assert_eq!(reg.registered(47013), None);
        drop(r);
        assert_eq!(reg.registered(47012), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_dead_processs_registration_does_not_count_and_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let reg = PortRegistry::new(dir.path());
        // A process that has exited.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(dir.path().join("47012"), format!("{pid} crashed run\n")).unwrap();
        assert_eq!(reg.registered(47012), None);
        let _r = reg.register(47012, "new run").unwrap();
        assert_eq!(reg.registered(47012).as_deref(), Some("new run"));
    }

    #[cfg(unix)]
    #[test]
    fn a_live_processs_registration_is_not_taken() {
        let dir = tempfile::tempdir().unwrap();
        let reg = PortRegistry::new(dir.path());
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        std::fs::write(
            dir.path().join("47012"),
            format!("{} other run\n", child.id()),
        )
        .unwrap();
        let err = reg.register(47012, "mine").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(reg.registered(47012).as_deref(), Some("other run"));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_member_in_a_space_registers_its_spaces_address_as_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let reg = PortRegistry::new(dir.path());
        assert_eq!(reg.target(47012), None);
        let plain = reg.register(47012, "run-1 attempt 1 rank 0").unwrap();
        assert_eq!(reg.target(47012), Some("127.0.0.1:47012".parse().unwrap()));
        drop(plain);
        let r = reg
            .register_to(
                47012,
                "run-1 attempt 1 rank 0",
                Some("192.168.64.7:47012".parse().unwrap()),
            )
            .unwrap();
        assert_eq!(
            reg.target(47012),
            Some("192.168.64.7:47012".parse().unwrap())
        );
        assert_eq!(
            reg.registered(47012).as_deref(),
            Some("run-1 attempt 1 rank 0")
        );
        drop(r);
        assert_eq!(reg.target(47012), None);
        // Never a public or tailnet address.
        for bad in ["8.8.8.8:47012", "100.64.1.2:47012"] {
            let e = reg
                .register_to(47013, "x", Some(bad.parse().unwrap()))
                .unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
        assert!(private_address("172.17.0.1".parse().unwrap()));
        assert!(private_address("127.0.0.1".parse().unwrap()));
        assert!(!private_address("100.100.1.1".parse().unwrap()));
    }

    #[test]
    fn control_messages_have_a_stable_shape() {
        let req = ControlRequest::Forward {
            machine: "m-1".into(),
            port: 47012,
            bind: None,
        };
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"forward":{"machine":"m-1","port":47012}}"#
        );
        let on: ControlRequest = serde_json::from_str(
            r#"{"forward":{"machine":"m-1","port":47012,"bind":"192.168.64.1"}}"#,
        )
        .unwrap();
        assert_eq!(
            on,
            ControlRequest::Forward {
                machine: "m-1".into(),
                port: 47012,
                bind: Some("192.168.64.1".parse().unwrap()),
            }
        );
        let reply: ControlReply =
            serde_json::from_str(r#"{"local":"127.0.0.1:53211","path":"direct","peer":"mac"}"#)
                .unwrap();
        assert_eq!(reply.local, Some("127.0.0.1:53211".parse().unwrap()));
        assert_eq!(
            serde_json::to_string(&ControlReply {
                error: Some("no".into()),
                ..Default::default()
            })
            .unwrap(),
            r#"{"error":"no"}"#
        );
    }
}
