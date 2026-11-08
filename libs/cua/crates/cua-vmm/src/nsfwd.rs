//! The host's ports into a VM's pasta namespace, without pasta's splice.
//!
//! On Linux a VM runs inside a network namespace pasta creates (Secure
//! Spaces, [`crate::network`]). Its published ports (cua-spacesd's channel,
//! SSH, VNC, the user's forwards) used to cross into the namespace through
//! pasta's own forwards (`-t 127.0.0.1/H:N`). pasta carries those by
//! splicing between two sockets, and passt releases before 2025-01-06
//! (Ubuntu 24.04 ships 2024-02-20) set no `TCP_NODELAY` on them: a request
//! written in two parts (an HTTP/2 HEADERS frame, then DATA) waits for the
//! receiver's delayed ACK, a fixed ~40 ms per round trip on loopback (passt
//! commit 725acd1, "tcp_splice: Set (again) TCP_NODELAY on both sides").
//! Releases before 2026-10-02 can also hold a write back for 200 ms
//! (`SPLICE_F_MORE`, passt commit 8ba4132). The channel's RPCs went from
//! ~1 ms to ~42 ms.
//!
//! So cua forwards those ports itself, the way rootless container runtimes
//! do (rootlesskit's builtin port driver, Podman's rootlessport): a small
//! helper process ([`ARG`], run by the `cua` binary) binds the host's
//! `127.0.0.1:H` in the host's namespace, then joins the VM's user and
//! network namespaces (`setns`) and, for each connection, connects to
//! `127.0.0.1:N` inside. A socket keeps the namespace it was made in, so
//! the listeners stay on the host's loopback while every outbound connect
//! lands on the namespace's loopback (QEMU's `hostfwd`, the microVM's DNAT).
//! Both sockets get `TCP_NODELAY`; bytes are copied as they come.
//!
//! What it does not change: the host side listens on `127.0.0.1` only, as
//! pasta's forwards did, and the namespace side reaches only the namespace's
//! own loopback ports it was given. The guest's traffic still leaves through
//! pasta and the namespace's rules; the guest gains no path to the host
//! (the namespace still has no route to the host's loopback, `--no-map-gw`).
//!
//! When the helper can't run (not Linux, no `cua` binary to run it, an old
//! `cua`, or [`DISABLE_ENV`]`=0`) pasta's forwards are used as before. When
//! it starts but can't join the namespace, the launch fails with
//! [`JOIN_FAILED`], forwarding is turned off for this process
//! ([`is_join_failure`]) and the caller launches again with pasta's.
//!
//! The helper ends with the VM: it watches the namespace's first process
//! (a pidfd) and exits when it does. Stopping a VM also ends it at once
//! ([`stop`]), so a restart can bind the same ports.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;

use crate::error::{Result, VmmError};

/// The first argument that makes the `cua` binary the helper
/// ([`run_if_asked`]).
pub const ARG: &str = "__cua-nsfwd";
/// The helper's process id, in the VM's directory.
pub const PID_FILE: &str = "nsfwd.pid";
/// The helper's log, in the VM's directory.
pub const LOG_FILE: &str = "nsfwd.log";
/// `0` (or `false`, `off`, `no`): use pasta's forwards, not the helper.
/// `CUA_NSFWD_BIN` names the binary to run as the helper.
pub const DISABLE_ENV: &str = "CUA_NSFWD";
/// What a launch whose helper could not join the namespace fails with.
pub const JOIN_FAILED: &str = "the port forwarder could not join the VM's network namespace";

/// Set once the helper failed to join a namespace: this process uses
/// pasta's forwards from then on.
#[cfg_attr(not(unix), allow(dead_code))]
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Runs the helper and exits when this process was started as one (its
/// first argument is [`ARG`]). Call it first in `main`, before any thread
/// starts: joining a user namespace needs a single-threaded process.
pub fn run_if_asked() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != ARG) {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let spec = args
            .next()
            .and_then(|s| s.into_string().ok())
            .unwrap_or_default();
        std::process::exit(helper::main(&spec));
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("{ARG}: Linux only");
        std::process::exit(2);
    }
}

/// Whether `e` is a launch that failed because the helper could not join
/// the namespace: launch again (pasta's forwards are used now).
pub fn is_join_failure(e: &VmmError) -> bool {
    e.to_string().contains(JOIN_FAILED)
}

/// `host:ns` pairs, comma separated: the helper's argument.
pub fn spec(forwards: &[(u16, u16)]) -> String {
    forwards
        .iter()
        .map(|(h, n)| format!("{h}:{n}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// [`spec`] read back.
pub fn parse_spec(s: &str) -> Option<Vec<(u16, u16)>> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (h, n) = p.split_once(':')?;
            Some((h.parse().ok()?, n.parse().ok()?))
        })
        .collect()
}

#[cfg_attr(not(unix), allow(dead_code))]
fn disabled_by(v: Option<&str>) -> bool {
    v.map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "0" | "false" | "off" | "no"))
}

/// The binary that runs the helper: `$CUA_NSFWD_BIN`, this executable when
/// it is `cua`, else `cua` in `~/.cua/bin` or on `PATH`.
#[cfg(unix)]
fn helper_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_NSFWD_BIN").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && exe.file_name().is_some_and(|n| n == "cua")
    {
        return Some(exe);
    }
    let own = crate::host::cua_home().join("bin").join("cua");
    if own.is_file() {
        return Some(own);
    }
    crate::host::which("cua")
}

/// A helper that holds the host's listeners and waits to join a namespace.
#[cfg(unix)]
pub struct Pending {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    dir: PathBuf,
}

/// Starts the helper for a VM in `dir`, binding `forwards` (`host`, `ns`
/// port pairs) on the host's loopback. `None`: forward with pasta instead
/// (no helper here, or it could not bind; see the module docs).
#[cfg(unix)]
pub async fn spawn(dir: &Path, forwards: &[(u16, u16)]) -> Option<Pending> {
    use tokio::io::AsyncBufReadExt;
    if !cfg!(target_os = "linux")
        || forwards.is_empty()
        || BROKEN.load(Ordering::Relaxed)
        || disabled_by(std::env::var(DISABLE_ENV).ok().as_deref())
    {
        return None;
    }
    let bin = helper_bin()?;
    // A helper from this VM's last run must free the ports first.
    stop(dir);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))
        .ok()?;
    let mut child = tokio::process::Command::new(&bin)
        .arg(ARG)
        .arg(spec(forwards))
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(log)
        // Its own group: a terminal's Ctrl-C to the CLI that started a VM
        // must not end the VM's ports.
        .process_group(0)
        .kill_on_drop(false)
        .spawn()
        .map_err(|e| tracing::debug!(error = %e, bin = %bin.display(), "no port forwarder"))
        .ok()?;
    let stdin = child.stdin.take()?;
    let mut stdout = tokio::io::BufReader::new(child.stdout.take()?).lines();
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), stdout.next_line()).await;
    match line {
        Ok(Ok(Some(l))) if l == "bound" => Some(Pending {
            child,
            stdin,
            stdout,
            dir: dir.to_path_buf(),
        }),
        other => {
            // An older `cua` (no helper), or a port it could not bind:
            // pasta's forwards, as before.
            tracing::warn!(
                ?other,
                "port forwarder did not start; using pasta's forwards"
            );
            let _ = child.start_kill();
            let _ = child.wait().await;
            None
        }
    }
}

#[cfg(unix)]
impl Pending {
    /// Joins the namespace of process `pid` (the namespace's first
    /// process) and starts forwarding. On failure the helper is ended,
    /// forwarding is off for this process, and the error says
    /// [`JOIN_FAILED`].
    pub async fn join(mut self, pid: u32) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let res = async {
            self.stdin.write_all(format!("{pid}\n").as_bytes()).await?;
            self.stdin.flush().await?;
            tokio::time::timeout(std::time::Duration::from_secs(5), self.stdout.next_line())
                .await
                .map_err(|_| std::io::Error::other("no answer within 5 s"))?
        }
        .await;
        match res {
            Ok(Some(l)) if l == "ready" => {
                if let Some(id) = self.child.id() {
                    let _ = std::fs::write(self.dir.join(PID_FILE), format!("{id}\n"));
                }
                // Reap it whenever it exits (the VM outlives this handle).
                let mut child = self.child;
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                Ok(())
            }
            other => {
                BROKEN.store(true, Ordering::Relaxed);
                let _ = self.child.start_kill();
                let _ = self.child.wait().await;
                let said = match other {
                    Ok(Some(l)) => l,
                    Ok(None) => "it exited".into(),
                    Err(e) => e.to_string(),
                };
                let log = std::fs::read_to_string(self.dir.join(LOG_FILE)).unwrap_or_default();
                Err(VmmError::other(format!(
                    "{JOIN_FAILED} ({said}; see {}): {}",
                    self.dir.join(LOG_FILE).display(),
                    crate::exec::tail(&log, 600).trim()
                )))
            }
        }
    }
}

/// Ends the helper of the VM in `dir`, if one runs, and waits (up to 2 s)
/// until it has: its ports are free again.
#[cfg(unix)]
pub fn stop(dir: &Path) {
    let file = dir.join(PID_FILE);
    let pid = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    let _ = std::fs::remove_file(&file);
    let Some(pid) = pid.filter(|p| is_helper(*p)) else {
        return;
    };
    crate::host::signal(pid, "TERM");
    for _ in 0..100 {
        if !is_helper(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    crate::host::signal(pid, "KILL");
}

/// No helper off Unix: callers compile everywhere and forward with pasta
/// (or the platform's own forwards) as before.
#[cfg(not(unix))]
pub struct Pending;

#[cfg(not(unix))]
pub async fn spawn(_dir: &Path, _forwards: &[(u16, u16)]) -> Option<Pending> {
    None
}

#[cfg(not(unix))]
impl Pending {
    pub async fn join(self, _pid: u32) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(unix))]
pub fn stop(_dir: &Path) {}

/// Whether process `pid` is a port forwarder (by its command line, so a
/// reused pid is never signalled).
#[cfg(unix)]
fn is_helper(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .is_ok_and(|c| c.split(|b| *b == 0).any(|a| a == ARG.as_bytes()))
}

#[cfg(target_os = "linux")]
mod helper {
    //! The helper process itself: plain threads and blocking sockets, no
    //! runtime (it must be single-threaded until it has joined).

    use std::io::{BufRead, Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    pub fn main(spec: &str) -> i32 {
        let say = |l: &str| {
            let mut o = std::io::stdout().lock();
            let _ = writeln!(o, "{l}");
            let _ = o.flush();
        };
        let Some(forwards) = super::parse_spec(spec).filter(|f| !f.is_empty()) else {
            say("error: no forwards");
            return 2;
        };
        // The host's side, bound before joining: these stay on the host.
        let mut listeners = Vec::with_capacity(forwards.len());
        for (h, n) in &forwards {
            match bind(*h) {
                Ok(l) => listeners.push((l, *n)),
                Err(e) => {
                    eprintln!("127.0.0.1:{h}: {e}");
                    say(&format!("error: 127.0.0.1:{h}: {e}"));
                    return 1;
                }
            }
        }
        say("bound");
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_err() {
            return 1;
        }
        let Ok(pid) = line.trim().parse::<i32>() else {
            say("error: no namespace process");
            return 2;
        };
        // Watch the namespace's process before joining (pid numbers are
        // this pid namespace's, which joining does not change).
        let watch = pidfd(pid);
        if let Err(e) = join(pid) {
            eprintln!("joining process {pid}'s namespaces: {e}");
            say(&format!("error: joining process {pid}'s namespaces: {e}"));
            return 1;
        }
        say("ready");
        // Nothing more goes to the launcher, which may be gone.
        quiet_stdio();
        for (l, n) in listeners {
            std::thread::spawn(move || accept_loop(l, n));
        }
        wait_exit(pid, watch);
        0
    }

    /// Binds `127.0.0.1:port`, retrying for a moment while a stopping
    /// helper or VM still holds it.
    fn bind(port: u16) -> std::io::Result<TcpListener> {
        let mut tries = 0;
        loop {
            match TcpListener::bind(("127.0.0.1", port)) {
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                other => return other,
            }
        }
    }

    fn open_ns(pid: i32, kind: &str) -> std::io::Result<OwnedFd> {
        let f = std::fs::File::open(format!("/proc/{pid}/ns/{kind}"))?;
        Ok(OwnedFd::from(f))
    }

    fn same_ns(pid: i32, kind: &str) -> bool {
        use std::os::unix::fs::MetadataExt;
        let ino = |p: String| std::fs::metadata(p).map(|m| (m.dev(), m.ino())).ok();
        let mine = ino(format!("/proc/self/ns/{kind}"));
        mine.is_some() && mine == ino(format!("/proc/{pid}/ns/{kind}"))
    }

    /// Joins process `pid`'s user namespace (when it has its own: pasta
    /// makes one for an unprivileged user) and then its network namespace.
    fn join(pid: i32) -> std::io::Result<()> {
        let user = open_ns(pid, "user")?;
        let net = open_ns(pid, "net")?;
        if !same_ns(pid, "user") {
            // SAFETY: a plain syscall on an fd we own.
            if unsafe { libc::setns(user.as_raw_fd(), libc::CLONE_NEWUSER) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: as above.
        if unsafe { libc::setns(net.as_raw_fd(), libc::CLONE_NEWNET) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn quiet_stdio() {
        if let Ok(null) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
        {
            // SAFETY: dup2 onto the standard descriptors.
            unsafe {
                libc::dup2(null.as_raw_fd(), 0);
                libc::dup2(null.as_raw_fd(), 1);
            }
        }
    }

    fn pidfd(pid: i32) -> Option<OwnedFd> {
        // SAFETY: pidfd_open(2) returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        // SAFETY: a fresh fd this process owns.
        (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }

    /// Returns when process `pid` has exited.
    fn wait_exit(pid: i32, watch: Option<OwnedFd>) {
        if let Some(fd) = watch {
            let mut p = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                // SAFETY: one valid pollfd.
                let r = unsafe { libc::poll(&mut p, 1, -1) };
                if r > 0 {
                    return;
                }
                if r < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    break;
                }
            }
        }
        // No pidfd (a kernel before 5.3): look now and then.
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn accept_loop(l: TcpListener, port: u16) {
        for conn in l.incoming() {
            let Ok(inbound) = conn else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            std::thread::spawn(move || forward(inbound, port));
        }
    }

    /// Carries one connection to the namespace's `127.0.0.1:port`. Nothing
    /// listening there yet (the guest is booting) closes it, as pasta did.
    fn forward(inbound: TcpStream, port: u16) {
        let Ok(outbound) = TcpStream::connect(("127.0.0.1", port)) else {
            return;
        };
        let _ = inbound.set_nodelay(true);
        let _ = outbound.set_nodelay(true);
        let (Ok(in2), Ok(out2)) = (inbound.try_clone(), outbound.try_clone()) else {
            return;
        };
        let up = std::thread::spawn(move || pump(in2, out2));
        pump(outbound, inbound);
        let _ = up.join();
    }

    /// Copies `from` to `to` until `from` ends, then half-closes `to`.
    fn pump(mut from: TcpStream, mut to: TcpStream) {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match from.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if to.write_all(&buf[..n]).is_err() {
                        let _ = from.shutdown(Shutdown::Read);
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = to.shutdown(Shutdown::Write);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips() {
        let f = vec![(40001, 3211), (40002, 22)];
        assert_eq!(spec(&f), "40001:3211,40002:22");
        assert_eq!(parse_spec(&spec(&f)), Some(f));
        assert_eq!(parse_spec(""), Some(vec![]));
        assert_eq!(parse_spec("1:x"), None);
        assert_eq!(parse_spec("70000:1"), None);
    }

    #[test]
    fn only_an_explicit_off_disables_it() {
        for v in ["0", "false", "OFF", " no "] {
            assert!(disabled_by(Some(v)), "{v}");
        }
        for v in ["1", "on", ""] {
            assert!(!disabled_by(Some(v)), "{v}");
        }
        assert!(!disabled_by(None));
    }

    #[test]
    fn a_join_failure_is_recognised() {
        let e = VmmError::other(format!("{JOIN_FAILED} (error: EPERM)"));
        assert!(is_join_failure(&e));
        assert!(!is_join_failure(&VmmError::other("qemu exited")));
    }
}
