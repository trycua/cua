//! Explicitly opted-in, operator-granted Hyprland input experiment.
//! No approval or signing operation is available to Driver. A pending request
//! keeps its connection alive so an external operator can approve its challenge.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PACKET: usize = 2048;

pub fn enabled() -> bool {
    std::env::var("CUA_DRIVER_EXPERIMENTAL_HYPRLAND_INPUT").as_deref() == Ok("1")
        && super::wayland_input_enabled()
        && super::hyprland::is_session()
}

/// Coordinates are window-local logical units. Hyprland's existing capture()
/// normalizes physical buffers to logical dimensions; tool routes undo only
/// the later preview resize (or zoom), so no additional scale division belongs
/// here. TARGET bounds and revision remain authoritative at dispatch.
pub enum Action {
    Click {
        x: f64,
        y: f64,
        button: u32,
        count: usize,
    },
    Key {
        key: String,
        modifiers: Vec<String>,
    },
    Scroll {
        point: Option<(f64, f64)>,
        direction: String,
        amount: usize,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        duration_ms: u64,
    },
}

struct Client {
    socket: socket2::Socket,
    path: PathBuf,
    pid: u32,
    address: u64,
    epoch: String,
    challenge: String,
    sequence: u64,
}

fn socket_path(lane: usize) -> Result<PathBuf> {
    let runtime =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?);
    let signature =
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE").context("missing Hyprland instance")?;
    ensure!(runtime.is_absolute(), "XDG_RUNTIME_DIR must be absolute");
    ensure!(
        !signature.is_empty()
            && signature
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "invalid Hyprland instance"
    );
    Ok(runtime.join("hypr").join(signature).join(if lane == 0 {
        "cua-input-test.sock"
    } else {
        "cua-input-test-2.sock"
    }))
}

fn hex_field(value: &Value, name: &str) -> Result<String> {
    let field = value[name].as_str().context("missing protocol hex field")?;
    ensure!(
        field.len() == 32
            && field
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid {name}"
    );
    Ok(field.to_owned())
}

fn validate_reply(value: &Value) -> Result<()> {
    match value["ok"].as_bool() {
        Some(true) => Ok(()),
        Some(false) => {
            ensure!(
                value["code"].as_str().is_some() && value["detail"].as_str().is_some(),
                "malformed refusal"
            );
            Ok(())
        }
        None => bail!("missing protocol result"),
    }
}

fn wait(socket: &socket2::Socket, events: i16, deadline: Instant) -> Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "Hyprland input timeout; dispatch effect is unknown"
        );
        let mut fd = libc::pollfd {
            fd: socket.as_raw_fd(),
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().max(1) as i32) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        ensure!(
            result > 0,
            "Hyprland input timeout; dispatch effect is unknown"
        );
        ensure!(
            fd.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
            "Hyprland input connection failed"
        );
        ensure!(fd.revents & events != 0, "Hyprland input disconnected");
        return Ok(());
    }
}

impl Client {
    fn attest(&self) -> Result<()> {
        let wayland = super::hyprland::wayland_connection()?;
        super::hyprland::verify_capture_peer(&wayland)?;
        ensure!(
            super::hyprland::peer_pid(self.socket.as_raw_fd())?
                == super::hyprland::peer_pid(wayland.backend().poll_fd().as_raw_fd())?,
            "input plugin and Wayland name different compositor processes"
        );
        Ok(())
    }

    fn connect(path: PathBuf, pid: u32, address: u64) -> Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::UNIX,
            socket2::Type::from(libc::SOCK_SEQPACKET),
            None,
        )?;
        socket.connect_timeout(&socket2::SockAddr::unix(&path)?, TIMEOUT)?;
        socket.set_nonblocking(true)?;
        let mut client = Self {
            socket,
            path,
            pid,
            address,
            epoch: String::new(),
            challenge: String::new(),
            sequence: 0,
        };
        client.attest()?;
        let hello = client.request("HELLO")?;
        ensure!(
            hello["ok"] == true && hello["protocol"].as_u64() == Some(0),
            "experimental protocol 0 unavailable"
        );
        client.epoch = hex_field(&hello, "epoch")?;
        client.challenge = hex_field(&hello, "challenge")?;
        Ok(client)
    }

    fn request(&self, packet: &str) -> Result<Value> {
        ensure!(
            packet.len() <= MAX_PACKET && packet.is_ascii(),
            "invalid input packet"
        );
        let deadline = Instant::now() + TIMEOUT;
        loop {
            wait(&self.socket, libc::POLLOUT, deadline)?;
            let count = unsafe {
                libc::send(
                    self.socket.as_raw_fd(),
                    packet.as_ptr().cast(),
                    packet.len(),
                    libc::MSG_NOSIGNAL,
                )
            };
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(count as usize == packet.len(), "partial input packet");
            break;
        }
        self.receive(deadline)
    }

    fn receive(&self, deadline: Instant) -> Result<Value> {
        let mut bytes = [0u8; MAX_PACKET];
        loop {
            wait(&self.socket, libc::POLLIN, deadline)?;
            let count = unsafe {
                libc::recv(
                    self.socket.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_TRUNC,
                )
            };
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(
                count > 0 && count as usize <= MAX_PACKET,
                "closed or oversized input reply"
            );
            let value: Value = serde_json::from_slice(&bytes[..count as usize])?;
            validate_reply(&value)?;
            return Ok(value);
        }
    }

    fn execute(
        &mut self,
        action: Action,
        started: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<Value> {
        self.attest()?;
        let target = self.request(&format!("TARGET {} {:x}", self.pid, self.address))?;
        if target["ok"] == false {
            return Ok(target);
        }
        let token = hex_field(&target, "target")?;
        let revision = target["revision"]
            .as_u64()
            .context("missing target revision")?;
        let width = target["width"].as_f64().context("missing target width")?;
        let height = target["height"].as_f64().context("missing target height")?;
        ensure!(
            width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
            "invalid target geometry"
        );
        self.sequence = self.sequence.checked_add(1).context("sequence exhausted")?;
        let is_drag = matches!(&action, Action::Drag { .. });
        let packet = action.packet(self.sequence, &token, revision, width, height)?;
        let mut reply = self.request(&packet)?;
        if reply["phase"] == "started" {
            ensure!(is_drag, "unexpected input start acknowledgement");
            if let Some(started) = started {
                let _ = started.send(());
            }
            reply = self.receive(Instant::now() + TIMEOUT)?;
        }
        if reply["ok"] == true {
            ensure!(
                reply["effect"] == "unverifiable" && reply["route"] == "synthetic_events",
                "invalid action acknowledgement"
            );
        } else {
            // Never report a pending operator grant as successful dispatch.
            reply["epoch"] = json!(self.epoch);
            reply["challenge"] = json!(self.challenge);
            reply["target"] = json!(token);
            reply["revision"] = json!(revision);
        }
        Ok(reply)
    }
}

impl Action {
    fn packet(
        self,
        sequence: u64,
        token: &str,
        revision: u64,
        width: f64,
        height: f64,
    ) -> Result<String> {
        let point = |x: f64, y: f64| -> Result<()> {
            ensure!(
                x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < width && y < height,
                "input point outside attested logical target geometry"
            );
            Ok(())
        };
        let prefix = format!("{sequence} {token} {revision}");
        Ok(match self {
            Self::Click {
                x,
                y,
                button,
                count,
            } => {
                point(x, y)?;
                ensure!(
                    (1..=2).contains(&count),
                    "experiment supports only single or double clicks"
                );
                let button = match button {
                    1 => 272,
                    2 => 274,
                    3 => 273,
                    _ => bail!("unsupported click button"),
                };
                format!("CLICK {prefix} {x} {y} {button} {count}")
            }
            Self::Key { key, modifiers } => {
                let keycode = super::key_to_evdev(&key)
                    .context("key has no evdev mapping; Unicode text is unsupported")?;
                let mut mask = 0;
                for modifier in modifiers {
                    mask |= match modifier.to_ascii_lowercase().as_str() {
                        "shift" => 1,
                        "ctrl" | "control" => 2,
                        "alt" | "option" => 4,
                        "super" | "meta" | "cmd" | "command" | "win" => 8,
                        _ => bail!("unsupported modifier"),
                    };
                }
                format!("KEY {prefix} {keycode} {mask}")
            }
            Self::Scroll {
                point: position,
                direction,
                amount,
            } => {
                let (x, y) = position.unwrap_or((width / 2.0, height / 2.0));
                point(x, y)?;
                ensure!((1..=100).contains(&amount), "unsupported scroll amount");
                let (axis, sign) = match direction.as_str() {
                    "up" => (0, -1),
                    "down" => (0, 1),
                    "left" => (1, -1),
                    "right" => (1, 1),
                    _ => bail!("unsupported scroll direction"),
                };
                let value = amount as i32 * 10 * sign;
                format!("SCROLL {prefix} {x} {y} {axis} {value}")
            }
            Self::Drag {
                from: (x1, y1),
                to: (x2, y2),
                duration_ms,
            } => {
                point(x1, y1)?;
                point(x2, y2)?;
                ensure!(
                    (50..=2000).contains(&duration_ms),
                    "experimental drag duration must be 50–2000 ms"
                );
                format!("DRAG {prefix} {x1} {y1} {x2} {y2} {duration_ms}")
            }
        })
    }
}

struct SessionClient {
    lane: usize,
    client: Mutex<Option<Client>>,
}

static CLIENTS: OnceLock<Mutex<HashMap<String, Arc<SessionClient>>>> = OnceLock::new();

fn clients() -> &'static Mutex<HashMap<String, Arc<SessionClient>>> {
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The registry supplies a runtime-private lifecycle identity. Public labels
/// are already namespaced there; labels never grant input authority here.
fn session_client(owner: &str) -> Result<Arc<SessionClient>> {
    ensure!(
        !owner.is_empty() && owner != "default",
        "authenticated lifecycle required"
    );
    let mut clients = clients()
        .lock()
        .map_err(|_| anyhow::anyhow!("input pool poisoned"))?;
    if let Some(client) = clients.get(owner) {
        return Ok(client.clone());
    }
    let lane = (0..2)
        .find(|lane| clients.values().all(|client| client.lane != *lane))
        .context("both experimental input seats are in use; end an owning session first")?;
    let client = Arc::new(SessionClient {
        lane,
        client: Mutex::new(None),
    });
    clients.insert(owner.to_owned(), client.clone());
    Ok(client)
}

pub fn cleanup_session(owner: &str) {
    let client = clients().lock().unwrap().get(owner).cloned();
    if let Some(client) = client {
        // The normal lifecycle barrier waits for in-flight actions first.
        // Closing the exact connection revokes this lane, never another one.
        *client.client.lock().unwrap() = None;
        clients().lock().unwrap().remove(owner);
    }
}

/// Runtime teardown must also reclaim leases that never acquired an overlay.
/// The prefix comes from the trusted registry, never a public session label.
pub fn cleanup_runtime(prefix: &str) {
    let owners: Vec<_> = clients()
        .lock()
        .unwrap()
        .keys()
        .filter(|owner| owner.starts_with(prefix))
        .cloned()
        .collect();
    for owner in owners {
        cleanup_session(&owner);
    }
}

/// Only same-lifecycle actions serialize. Independent authenticated sessions
/// use independent sockets/seats and may overlap. Never retry unknown effects.
pub fn execute(owner: Option<String>, pid: u32, address: u64, action: Action) -> Result<Value> {
    execute_with_started(owner, pid, address, action, None)
}

pub fn execute_with_started(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    started: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<Value> {
    ensure!(enabled(), "experimental Hyprland input is disabled");
    ensure!(
        pid > 0 && address > 0,
        "exact pid and window address required"
    );
    let owner = owner.context("authenticated lifecycle required")?;
    let client = session_client(&owner)?;
    let mut slot = client
        .client
        .lock()
        .map_err(|_| anyhow::anyhow!("input connection poisoned"))?;
    let path = socket_path(client.lane)?;
    if slot
        .as_ref()
        .is_some_and(|client| client.pid != pid || client.address != address || client.path != path)
    {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(Client::connect(path, pid, address)?);
    }
    let mut result = slot
        .as_mut()
        .context("missing input connection")?
        .execute(action, started);
    if result.is_err() {
        *slot = None;
    }
    if let Ok(value) = &mut result {
        // A bounded slot number lets the external test operator select the
        // correct endpoint. It is not a credential or a caller-selected seat.
        value["lane"] = json!(client.lane);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn independent_lifecycles_have_bounded_distinct_seats_and_cleanup() {
        assert!(session_client("").is_err());
        assert!(session_client("default").is_err());
        let a = session_client("input-pool-test-a").unwrap();
        let b = session_client("input-pool-test-b").unwrap();
        assert_ne!(a.lane, b.lane);
        assert!(Arc::ptr_eq(
            &a,
            &session_client("input-pool-test-a").unwrap()
        ));
        assert!(session_client("input-pool-test-c").is_err());
        let a_lock = a.client.lock().unwrap();
        // No global action mutex: another seat remains independently usable.
        assert!(b.client.try_lock().is_ok());
        drop(a_lock);
        cleanup_session("input-pool-test-a");
        let c = session_client("input-pool-test-c").unwrap();
        assert_eq!(c.lane, a.lane);
        assert!(Arc::ptr_eq(
            &b,
            &session_client("input-pool-test-b").unwrap()
        ));
        cleanup_session("input-pool-test-b");
        cleanup_session("input-pool-test-c");
        let runtime = session_client("__cua_runtime_test:pending").unwrap();
        let other = session_client("__cua_runtime_other:pending").unwrap();
        cleanup_runtime("__cua_runtime_test:");
        assert!(Arc::ptr_eq(
            &other,
            &session_client("__cua_runtime_other:pending").unwrap()
        ));
        let reused = session_client("input-pool-test-reused").unwrap();
        assert_eq!(reused.lane, runtime.lane);
        cleanup_session("__cua_runtime_other:pending");
        cleanup_session("input-pool-test-reused");
    }

    #[test]
    fn click_is_exact_and_maps_right_button() {
        assert_eq!(
            Action::Click {
                x: 2.5,
                y: 3.0,
                button: 3,
                count: 2
            }
            .packet(7, TOKEN, 4, 100.0, 80.0)
            .unwrap(),
            format!("CLICK 7 {TOKEN} 4 2.5 3 273 2")
        );
    }

    #[test]
    fn unsafe_geometry_and_unsupported_actions_refuse() {
        for x in [f64::NAN, f64::INFINITY, -1.0, 100.0] {
            assert!(Action::Click {
                x,
                y: 1.0,
                button: 1,
                count: 1
            }
            .packet(1, TOKEN, 1, 100.0, 80.0)
            .is_err());
        }
        assert!(Action::Key {
            key: "🙂".into(),
            modifiers: vec![]
        }
        .packet(1, TOKEN, 1, 100.0, 80.0)
        .is_err());
        assert!(Action::Drag {
            from: (1.0, 1.0),
            to: (2.0, 2.0),
            duration_ms: 2001
        }
        .packet(1, TOKEN, 1, 100.0, 80.0)
        .is_err());
    }

    #[test]
    fn hotkey_and_scroll_are_bounded_complete_packets() {
        assert_eq!(
            Action::Key {
                key: "a".into(),
                modifiers: vec!["ctrl".into(), "shift".into()]
            }
            .packet(2, TOKEN, 8, 100.0, 80.0)
            .unwrap(),
            format!("KEY 2 {TOKEN} 8 30 3")
        );
        assert_eq!(
            Action::Scroll {
                point: None,
                direction: "up".into(),
                amount: 3
            }
            .packet(3, TOKEN, 8, 100.0, 80.0)
            .unwrap(),
            format!("SCROLL 3 {TOKEN} 8 50 40 0 -30")
        );
    }

    #[test]
    fn malformed_responses_and_tokens_refuse() {
        assert!(validate_reply(&json!({"ok": false})).is_err());
        assert!(validate_reply(
            &json!({"ok":false,"code":"permission_required","detail":"pending"})
        )
        .is_ok());
        assert!(hex_field(&json!({"target":"../invalid"}), "target").is_err());
        assert_eq!(
            hex_field(&json!({"target":TOKEN}), "target").unwrap(),
            TOKEN
        );
    }

    fn test_connection() -> (Client, socket2::Socket) {
        let mut fds = [-1; 2];
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    fds.as_mut_ptr(),
                )
            },
            0
        );
        let socket = unsafe { socket2::Socket::from_raw_fd(fds[0]) };
        let peer = unsafe { socket2::Socket::from_raw_fd(fds[1]) };
        socket.set_nonblocking(true).unwrap();
        peer.set_read_timeout(Some(TIMEOUT)).unwrap();
        (
            Client {
                socket,
                path: PathBuf::new(),
                pid: 1,
                address: 1,
                epoch: TOKEN.into(),
                challenge: TOKEN.into(),
                sequence: 0,
            },
            peer,
        )
    }

    #[test]
    fn pending_refusal_keeps_seqpacket_connection_usable() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut packet = [0u8; 64];
                assert!(
                    unsafe {
                        libc::recv(
                            peer.as_raw_fd(),
                            packet.as_mut_ptr().cast(),
                            packet.len(),
                            0,
                        )
                    } > 0
                );
                peer.send(br#"{"ok":false,"code":"permission_required","detail":"pending"}"#)
                    .unwrap();
            }
        });
        for _ in 0..2 {
            assert_eq!(
                client.request("HELLO").unwrap()["code"],
                "permission_required"
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn oversized_seqpacket_reply_refuses_without_truncation() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            let mut packet = [0u8; 64];
            assert!(
                unsafe {
                    libc::recv(
                        peer.as_raw_fd(),
                        packet.as_mut_ptr().cast(),
                        packet.len(),
                        0,
                    )
                } > 0
            );
            peer.send(&vec![b' '; MAX_PACKET + 1]).unwrap();
        });
        assert!(client
            .request("HELLO")
            .unwrap_err()
            .to_string()
            .contains("oversized"));
        server.join().unwrap();
    }
}
