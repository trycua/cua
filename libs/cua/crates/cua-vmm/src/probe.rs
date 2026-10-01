//! Daemon-agnostic readiness probes.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{Result, VmmError};
use crate::types::{Endpoints, Probe};

/// How long a freshly accepted TCP connection must stay open (or produce data)
/// to count as a real listener. QEMU slirp and Docker's userland proxy accept on
/// the host and then close straight away when nothing listens in the guest.
const HOLD_OPEN: Duration = Duration::from_millis(400);

/// How long every attempt of a probe may fail with "no route to host" (or
/// "network unreachable") before [`wait_all`] gives up with
/// [`unreachable_error`] instead of waiting out its whole budget. A guest
/// that just got its address can be briefly unroutable; one that stays so
/// is blocked from this process (on macOS: Local Network privacy) and
/// never becomes reachable by waiting.
pub const UNREACHABLE_GRACE: Duration = Duration::from_secs(20);

/// One probe attempt.
#[derive(Debug)]
pub enum Attempt {
    /// The probe passed.
    Passed,
    /// Not yet (refused, timed out, closed at once, bad status).
    NotYet,
    /// The connect failed because this host has no route to the guest
    /// ([`is_unreachable`]); `addr` is what was dialed.
    Unreachable { addr: String, error: std::io::Error },
}

/// Run one probe once. `Ok(true)` = passed.
pub async fn check(ep: &Endpoints, probe: &Probe) -> Result<bool> {
    Ok(matches!(attempt(ep, probe).await?, Attempt::Passed))
}

/// Run one probe once, telling "not yet" from "unreachable".
pub async fn attempt(ep: &Endpoints, probe: &Probe) -> Result<Attempt> {
    match probe {
        Probe::None => Ok(Attempt::Passed),
        Probe::Tcp { port } => {
            let addr = resolve(ep, *port)?;
            Ok(tcp_attempt(&addr).await)
        }
        Probe::Http { port, path } => {
            let addr = resolve(ep, *port)?;
            Ok(http_attempt(&addr, path).await)
        }
    }
}

/// Poll every probe until all pass or `timeout` elapses. A probe whose
/// every attempt fails with "no route to host" for [`UNREACHABLE_GRACE`]
/// fails at once with [`unreachable_error`].
pub async fn wait_all(
    name: &str,
    ep: &Endpoints,
    probes: &[Probe],
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    for probe in probes {
        let mut unreachable = UnreachableWatch::default();
        loop {
            let now = tokio::time::Instant::now();
            match attempt(ep, probe).await? {
                Attempt::Passed => {
                    tracing::debug!(sandbox = name, ?probe, "probe passed");
                    break;
                }
                Attempt::NotYet => unreachable.reset(),
                Attempt::Unreachable { addr, error } => {
                    if unreachable.observe(now) >= UNREACHABLE_GRACE {
                        return Err(unreachable_error(&addr, &error));
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::Timeout {
                    name: name.to_string(),
                    secs: timeout.as_secs(),
                    detail: match probe_addr(ep, probe) {
                        Some(addr) => {
                            format!("probe {probe:?} never passed (nothing answered on {addr})")
                        }
                        None => format!("probe {probe:?} never passed"),
                    },
                });
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }
    Ok(())
}

/// How long consecutive attempts have been unreachable.
#[derive(Debug, Default)]
pub struct UnreachableWatch {
    since: Option<tokio::time::Instant>,
}

impl UnreachableWatch {
    /// An attempt at `now` was unreachable: how long every attempt has been.
    pub fn observe(&mut self, now: tokio::time::Instant) -> Duration {
        now.saturating_duration_since(*self.since.get_or_insert(now))
    }

    /// An attempt got further (refused, timed out, passed).
    pub fn reset(&mut self) {
        self.since = None;
    }
}

/// Whether a connect error means this host has no route to the peer
/// (`EHOSTUNREACH`, `ENETUNREACH`), as opposed to nothing listening yet.
/// macOS reports a connection its Local Network privacy blocks this way.
pub fn is_unreachable(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable
    )
}

/// The error for a guest this process cannot reach: on macOS, what to allow
/// in Local Network privacy and for which app (the `.app` bundle this
/// process runs from, by name and path, since two builds can share a name).
pub fn unreachable_error(addr: &str, error: &std::io::Error) -> VmmError {
    unreachable_error_for(addr, error, std::env::current_exe().ok().as_deref())
}

fn unreachable_error_for(
    addr: &str,
    error: &std::io::Error,
    exe: Option<&std::path::Path>,
) -> VmmError {
    if cfg!(target_os = "macos") {
        let bundle = exe.and_then(app_bundle);
        let (who, allow) = match &bundle {
            Some((name, path)) => (
                format!("{name} ({})", path.display()),
                format!("turn on {name} there"),
            ),
            None => (
                "cua".to_string(),
                "turn on the app that started cua (for example your terminal) there".to_string(),
            ),
        };
        VmmError::Missing {
            what: "Local Network access".into(),
            hint: format!(
                "{who} cannot reach the VM at {addr} ({error}). macOS blocks an app's local \
                 network connections until they are allowed: open System Settings > Privacy & \
                 Security > Local Network, {allow}, then try again"
            ),
        }
    } else {
        VmmError::Missing {
            what: "a network route to the guest".into(),
            hint: format!(
                "{addr} is unreachable from this host ({error}); check the VM network and the \
                 host firewall"
            ),
        }
    }
}

/// The `.app` bundle `exe` runs from: its name (without `.app`) and path.
fn app_bundle(exe: &std::path::Path) -> Option<(String, std::path::PathBuf)> {
    exe.ancestors().find_map(|dir| {
        let name = dir.file_name()?.to_str()?;
        let stem = name.strip_suffix(".app")?;
        Some((stem.to_string(), dir.to_path_buf()))
    })
}

/// The address a probe dials, for messages.
fn probe_addr(ep: &Endpoints, probe: &Probe) -> Option<String> {
    match probe {
        Probe::None => None,
        Probe::Tcp { port } | Probe::Http { port, .. } => ep.addr(*port),
    }
}

fn resolve(ep: &Endpoints, guest_port: u16) -> Result<String> {
    ep.addr(guest_port).ok_or_else(|| {
        VmmError::invalid(format!(
            "probe port {guest_port} is not published by this sandbox"
        ))
    })
}

/// Connect, then require the peer to either send data or hold the connection
/// open for [`HOLD_OPEN`]. An immediate EOF/reset means "forwarder only".
pub async fn tcp_alive(addr: &str) -> bool {
    matches!(tcp_attempt(addr).await, Attempt::Passed)
}

/// Connects within 3 s; an unroutable peer is [`Attempt::Unreachable`].
async fn connect(addr: &str) -> std::result::Result<TcpStream, Attempt> {
    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(addr)).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(error)) if is_unreachable(&error) => Err(Attempt::Unreachable {
            addr: addr.to_string(),
            error,
        }),
        Ok(Err(_)) | Err(_) => Err(Attempt::NotYet),
    }
}

async fn tcp_attempt(addr: &str) -> Attempt {
    let mut s = match connect(addr).await {
        Ok(s) => s,
        Err(a) => return a,
    };
    let mut buf = [0u8; 64];
    let alive = match tokio::time::timeout(HOLD_OPEN, s.read(&mut buf)).await {
        Err(_) => true,      // still open, silent (e.g. HTTP server waiting)
        Ok(Ok(n)) => n > 0,  // banner (e.g. SSH) = alive; 0 = closed
        Ok(Err(_)) => false, // reset
    };
    if alive {
        Attempt::Passed
    } else {
        Attempt::NotYet
    }
}

/// Whether `/proc/net/tcp` + `/proc/net/tcp6` text (concatenated) shows a
/// socket in LISTEN state (`0A`) on `port`. `None` when the text has no
/// table header (not procfs output, so the caller cannot tell).
pub fn proc_net_listens(text: &str, port: u16) -> Option<bool> {
    let mut saw_header = false;
    let want = format!("{port:04X}");
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() == Some(&"sl") {
            saw_header = true;
            continue;
        }
        let (Some(local), Some(state)) = (cols.get(1), cols.get(3)) else {
            continue;
        };
        let local_port = local.rsplit_once(':').map(|(_, p)| p);
        if *state == "0A" && local_port.is_some_and(|p| p.eq_ignore_ascii_case(&want)) {
            return Some(true);
        }
    }
    saw_header.then_some(false)
}

/// Minimal HTTP/1.1 GET without pulling in a client: status 2xx/3xx passes.
pub async fn http_ok(addr: &str, path: &str) -> bool {
    matches!(http_attempt(addr, path).await, Attempt::Passed)
}

async fn http_attempt(addr: &str, path: &str) -> Attempt {
    let mut s = match connect(addr).await {
        Ok(s) => s,
        Err(a) => return a,
    };
    let fut = async {
        let host = addr.split(':').next().unwrap_or("localhost");
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut buf = vec![0u8; 256];
        let n = s.read(&mut buf).await.ok()?;
        parse_status(&buf[..n])
    };
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Some(code)) if (200..400).contains(&code) => Attempt::Passed,
        _ => Attempt::NotYet,
    }
}

fn parse_status(head: &[u8]) -> Option<u16> {
    let line = std::str::from_utf8(head).ok()?.lines().next()?;
    let mut parts = line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tokio::net::TcpListener;

    fn ep(guest: u16, host: u16) -> Endpoints {
        Endpoints {
            host: "127.0.0.1".into(),
            ports: BTreeMap::from([(guest, host)]),
            ..Default::default()
        }
    }

    #[test]
    fn parses_proc_net_tcp() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1 0 100 0 0 10 0\n\
   1: 0100007F:0CDB 0100007F:9C40 01 00000000:00000000 00:00000000 00000000     0        0 2 1 0 20 4 30 10 -1\n";
        let tcp6 = "  sl  local_address                         remote_address                        st tx_queue\n\
   0: 00000000000000000000000000000000:0C8B 00000000000000000000000000000000:0000 0A 00000000:00000000\n";
        let all = format!("{tcp}{tcp6}");
        assert_eq!(proc_net_listens(&all, 22), Some(true));
        assert_eq!(proc_net_listens(&all, 3211), Some(true));
        // 0x0CDB = 3291 is ESTABLISHED, not LISTEN.
        assert_eq!(proc_net_listens(&all, 3291), Some(false));
        assert_eq!(proc_net_listens(tcp, 3211), Some(false));
        assert_eq!(proc_net_listens("cat: not found", 22), None);
    }

    #[test]
    fn only_no_route_errors_are_unreachable() {
        use std::io::{Error, ErrorKind};
        assert!(is_unreachable(&Error::from(ErrorKind::HostUnreachable)));
        assert!(is_unreachable(&Error::from(ErrorKind::NetworkUnreachable)));
        #[cfg(unix)]
        {
            // What connect(2) returns: EHOSTUNREACH / ENETUNREACH.
            assert!(is_unreachable(&Error::from_raw_os_error(
                libc_ehostunreach()
            )));
        }
        assert!(!is_unreachable(&Error::from(ErrorKind::ConnectionRefused)));
        assert!(!is_unreachable(&Error::from(ErrorKind::TimedOut)));
        assert!(!is_unreachable(&Error::from(ErrorKind::ConnectionReset)));
    }

    #[cfg(unix)]
    fn libc_ehostunreach() -> i32 {
        if cfg!(target_os = "linux") { 113 } else { 65 }
    }

    #[test]
    fn unreachable_watch_measures_only_consecutive_failures() {
        let t0 = tokio::time::Instant::now();
        let mut w = UnreachableWatch::default();
        assert_eq!(w.observe(t0), Duration::ZERO);
        assert_eq!(
            w.observe(t0 + Duration::from_secs(5)),
            Duration::from_secs(5)
        );
        // An attempt that got further (refused, timed out) starts over.
        w.reset();
        assert_eq!(w.observe(t0 + Duration::from_secs(30)), Duration::ZERO);
        assert_eq!(
            w.observe(t0 + Duration::from_secs(30) + UNREACHABLE_GRACE),
            UNREACHABLE_GRACE
        );
    }

    #[test]
    fn unreachable_error_names_the_app_bundle_to_allow() {
        let e = std::io::Error::from(std::io::ErrorKind::HostUnreachable);
        let exe = std::path::Path::new("/Applications/Cua Spaces.app/Contents/MacOS/cua");
        let err = unreachable_error_for("192.168.64.140:3211", &e, Some(exe));
        let text = err.to_string();
        assert!(matches!(err, VmmError::Missing { .. }), "{text}");
        assert!(text.contains("192.168.64.140:3211"), "{text}");
        if cfg!(target_os = "macos") {
            assert!(
                text.starts_with("Local Network access is not available: Cua Spaces (/Applications/Cua Spaces.app) cannot reach"),
                "{text}"
            );
            assert!(
                text.contains("Privacy & Security > Local Network, turn on Cua Spaces there"),
                "{text}"
            );
            // Outside a bundle (a terminal's `cua`) it says what to allow.
            let bare = unreachable_error_for(
                "192.168.64.140:3211",
                &e,
                Some(std::path::Path::new("/usr/local/bin/cua")),
            )
            .to_string();
            assert!(bare.contains("the app that started cua"), "{bare}");
        }
        assert_eq!(
            app_bundle(exe),
            Some(("Cua Spaces".into(), "/Applications/Cua Spaces.app".into()))
        );
        assert_eq!(app_bundle(std::path::Path::new("/usr/local/bin/cua")), None);
    }

    #[test]
    fn parses_status_lines() {
        assert_eq!(parse_status(b"HTTP/1.1 204 No Content\r\n"), Some(204));
        assert_eq!(parse_status(b"SSH-2.0-OpenSSH\r\n"), None);
    }

    #[tokio::test]
    async fn tcp_probe_distinguishes_listener_from_accept_and_close() {
        // A real listener that holds connections open.
        let real = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let real_port = real.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = real.accept().await.unwrap();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    drop(s);
                });
            }
        });
        // A forwarder that accepts and closes immediately (slirp with no guest listener).
        let fake = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake_port = fake.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = fake.accept().await.unwrap();
                drop(s);
            }
        });
        assert!(check(&ep(22, real_port), &Probe::tcp(22)).await.unwrap());
        assert!(!check(&ep(22, fake_port), &Probe::tcp(22)).await.unwrap());
        assert!(check(&ep(22, real_port), &Probe::tcp(23)).await.is_err());
    }

    #[tokio::test]
    async fn http_probe_requires_success_status() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = [0u8; 512];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let resp = if req.starts_with("GET /ok ") {
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
                } else {
                    "HTTP/1.1 503 Nope\r\nContent-Length: 0\r\n\r\n"
                };
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        assert!(check(&ep(80, port), &Probe::http(80, "/ok")).await.unwrap());
        assert!(
            !check(&ep(80, port), &Probe::http(80, "/bad"))
                .await
                .unwrap()
        );
        wait_all(
            "t",
            &ep(80, port),
            &[Probe::http(80, "/ok"), Probe::None],
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let err = wait_all(
            "t",
            &ep(80, port),
            &[Probe::http(80, "/bad")],
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }));
    }
}
