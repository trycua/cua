//! Local Hyprland input after the common Driver registry admits the action.
//! Production v3 uses per-action target binding, not a second approval system.
//! Protocol 0 remains an explicitly selected, nonshipping test experiment.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(3);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);
const MAX_PACKET: usize = 2048;
const MAX_LANES: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryRoute {
    Background,
    Foreground,
}

impl DeliveryRoute {
    fn mode(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Foreground => "foreground",
        }
    }

    fn acknowledgement(self) -> &'static str {
        match self {
            Self::Background => "synthetic_events",
            Self::Foreground => "primary_foreground",
        }
    }
}

fn validate_target_route(target: &Value, route: DeliveryRoute) -> Result<()> {
    if route == DeliveryRoute::Foreground {
        ensure!(
            target["route"] == "primary_foreground",
            "foreground target route mismatch"
        );
    }
    Ok(())
}

/// Validate the whole string before dispatching any keys. Physical key delivery
/// uses the compositor's keyboard layout; these codes describe US ASCII keys.
pub(crate) fn foreground_text_actions(text: &str) -> Result<Vec<Action>> {
    ensure!(
        text.len() <= 4096 && text.is_ascii(),
        "foreground text requires at most 4096 ASCII bytes"
    );
    text.bytes()
        .map(|byte| {
            let (keycode, shift) = match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => (
                    super::key_to_evdev(&(byte as char).to_string())
                        .context("unsupported text key")?,
                    byte.is_ascii_uppercase(),
                ),
                b' ' => (57, false),
                b'\t' => (15, false),
                b'\n' => (28, false),
                b'-' => (12, false),
                b'_' => (12, true),
                b'=' => (13, false),
                b'+' => (13, true),
                b'[' => (26, false),
                b'{' => (26, true),
                b']' => (27, false),
                b'}' => (27, true),
                b';' => (39, false),
                b':' => (39, true),
                b'\'' => (40, false),
                b'"' => (40, true),
                b'`' => (41, false),
                b'~' => (41, true),
                b'\\' => (43, false),
                b'|' => (43, true),
                b',' => (51, false),
                b'<' => (51, true),
                b'.' => (52, false),
                b'>' => (52, true),
                b'/' => (53, false),
                b'?' => (53, true),
                b'!' => (2, true),
                b'@' => (3, true),
                b'#' => (4, true),
                b'$' => (5, true),
                b'%' => (6, true),
                b'^' => (7, true),
                b'&' => (8, true),
                b'*' => (9, true),
                b'(' => (10, true),
                b')' => (11, true),
                _ => bail!("unsupported foreground text control character"),
            };
            Ok(Action::TextKey { keycode, shift })
        })
        .collect()
}

/// Cancellation belongs to one invocation, never to its session's next call.
#[derive(Clone, Default)]
pub(crate) struct ActionCancellation(Arc<AtomicBool>);

pub(crate) struct CancelOnDrop(ActionCancellation);

impl ActionCancellation {
    pub(crate) fn invocation() -> (CancelOnDrop, Self) {
        let cancellation = Self::default();
        (CancelOnDrop(cancellation.clone()), cancellation)
    }

    fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            return Err(ActionCancelled.into());
        }
        Ok(())
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0 .0.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub(crate) struct ActionCancelled;

impl std::fmt::Display for ActionCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hyprland input invocation cancelled")
    }
}

impl std::error::Error for ActionCancelled {}

/// Both reservations are explicitly occupied; no target or action was sent.
#[derive(Debug)]
pub(crate) struct LaneBusy;

impl std::fmt::Display for LaneBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("both isolated input lanes are in use; end an owning session first")
    }
}

impl std::error::Error for LaneBusy {}

/// A sent action has no trustworthy final reply. The count describes only
/// acknowledged gesture phases (drag start or completed text keys), never a
/// total event count or proof that no later events landed.
#[derive(Debug)]
pub struct DispatchUnknown {
    pub acknowledged_phases: u32,
    detail: String,
}

impl std::fmt::Display for DispatchUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; final input delivery is unknown", self.detail)
    }
}

impl std::error::Error for DispatchUnknown {}

pub(crate) fn unknown_dispatch(error: anyhow::Error, acknowledged_phases: u32) -> anyhow::Error {
    DispatchUnknown {
        acknowledged_phases,
        detail: error.to_string(),
    }
    .into()
}

pub fn enabled() -> bool {
    super::wayland_input_enabled()
        && super::hyprland::is_session()
        && (protocol() == InputProtocol::Experiment
            || (0..MAX_LANES).any(|lane| {
                socket_path(lane).ok().is_some_and(|path| {
                    std::fs::symlink_metadata(path)
                        .ok()
                        .is_some_and(|metadata| {
                            metadata.file_type().is_socket()
                                && metadata.uid() == unsafe { libc::geteuid() }
                        })
                })
            }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputProtocol {
    Production,
    Experiment,
}

fn protocol() -> InputProtocol {
    if std::env::var("CUA_DRIVER_EXPERIMENTAL_HYPRLAND_INPUT").as_deref() == Ok("1") {
        InputProtocol::Experiment
    } else {
        InputProtocol::Production
    }
}

impl InputProtocol {
    fn version(self) -> u64 {
        match self {
            Self::Production => 3,
            Self::Experiment => 0,
        }
    }

    fn socket_name(self, lane: usize) -> Result<&'static str> {
        Ok(match (self, lane) {
            (Self::Production, 0) => "cua-input-v3.sock",
            (Self::Production, 1) => "cua-input-v3-2.sock",
            (Self::Experiment, 0) => "cua-input-test.sock",
            (Self::Experiment, 1) => "cua-input-test-2.sock",
            _ => bail!("invalid isolated input lane"),
        })
    }
}

/// Coordinates are window-local logical units. Hyprland's existing capture()
/// normalizes physical buffers to logical dimensions; tool routes undo only
/// the later preview resize (or zoom), so no additional scale division belongs
/// here. TARGET bounds and revision remain authoritative at dispatch.
pub enum Action {
    Activate,
    TextKey {
        keycode: u32,
        shift: bool,
    },
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
    cancellation: ActionCancellation,
    protocol: InputProtocol,
    foreground_supported: bool,
    delivery_route: Option<DeliveryRoute>,
    // Assigned by the compositor to this connection, never by the local pool.
    lane: Option<usize>,
    owner: String,
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
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            && signature != "."
            && signature != "..",
        "invalid Hyprland instance"
    );
    Ok(runtime
        .join("hypr")
        .join(signature)
        .join(protocol().socket_name(lane)?))
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

fn wait(
    socket: &socket2::Socket,
    owner: &str,
    cancellation: &ActionCancellation,
    events: i16,
    deadline: Instant,
) -> Result<()> {
    loop {
        cancellation.check()?;
        ensure!(
            !cua_driver_core::session::is_session_ending(owner),
            "Hyprland input session ending; dispatch effect is unknown"
        );
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
        let interval = remaining.min(CANCELLATION_POLL);
        let result = unsafe { libc::poll(&mut fd, 1, interval.as_millis().max(1) as i32) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if result == 0 {
            continue;
        }
        cancellation.check()?;
        ensure!(
            !cua_driver_core::session::is_session_ending(owner),
            "Hyprland input session ending; dispatch effect is unknown"
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
    fn peer_closed(&self) -> bool {
        let mut fd = libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: libc::POLLRDHUP,
            revents: 0,
        };
        // A zero-length SEQPACKET is valid, so recv/peek returning zero is
        // not proof of EOF. Poll only for hangup, without reading any queued
        // replies or waiting while the pool mutex is held. Uncertain results
        // (including an interrupted poll) keep the existing reservation.
        let result = unsafe { libc::poll(&mut fd, 1, 0) };
        result > 0 && fd.revents & (libc::POLLHUP | libc::POLLRDHUP) != 0
    }

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

    fn connect(
        path: PathBuf,
        owner: String,
        pid: u32,
        address: u64,
        lane: usize,
        cancellation: ActionCancellation,
    ) -> Result<Option<Self>> {
        cancellation.check()?;
        let socket = socket2::Socket::new(
            socket2::Domain::UNIX,
            socket2::Type::from(libc::SOCK_SEQPACKET),
            None,
        )?;
        socket.connect_timeout(&socket2::SockAddr::unix(&path)?, TIMEOUT)?;
        socket.set_nonblocking(true)?;
        let mut client = Self {
            socket,
            cancellation,
            protocol: protocol(),
            foreground_supported: false,
            delivery_route: None,
            lane: None,
            owner,
            path,
            pid,
            address,
            epoch: String::new(),
            challenge: String::new(),
            sequence: 0,
        };
        client.attest()?;
        if client.handshake(lane)? {
            Ok(Some(client))
        } else {
            Ok(None)
        }
    }

    /// Only an explicit busy reservation permits another endpoint attempt.
    /// Malformed replies and connection errors have unknown outcomes.
    fn handshake(&mut self, expected_lane: usize) -> Result<bool> {
        ensure!(expected_lane < MAX_LANES, "invalid isolated input lane");
        let hello = self.request("HELLO")?;
        ensure!(
            hello["ok"] == true && hello["protocol"].as_u64() == Some(self.protocol.version()),
            "isolated input protocol mismatch; no downgrade is permitted"
        );
        self.epoch = hex_field(&hello, "epoch")?;
        self.foreground_supported =
            self.protocol == InputProtocol::Production && hello["foreground_target"] == true;
        if self.protocol == InputProtocol::Experiment {
            self.challenge = hex_field(&hello, "challenge")?;
        } else {
            ensure!(
                hello.get("challenge").is_none(),
                "production input unexpectedly requires a signer"
            );
        }
        let claim = self.request("CLAIM")?;
        if claim["ok"] == false {
            if claim["code"] == "lane_busy" && claim["detail"] == "lane_busy" {
                return Ok(false);
            }
            bail!("isolated input lane claim refused: {claim}");
        }
        ensure!(
            claim["lane"].as_u64() == Some(expected_lane as u64),
            "invalid isolated input lane claim"
        );
        self.lane = Some(expected_lane);
        Ok(true)
    }

    fn request(&self, packet: &str) -> Result<Value> {
        ensure!(
            packet.len() <= MAX_PACKET && packet.is_ascii(),
            "invalid input packet"
        );
        let deadline = Instant::now() + TIMEOUT;
        loop {
            wait(
                &self.socket,
                &self.owner,
                &self.cancellation,
                libc::POLLOUT,
                deadline,
            )?;
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
            wait(
                &self.socket,
                &self.owner,
                &self.cancellation,
                libc::POLLIN,
                deadline,
            )?;
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

    fn execute_routed(
        &mut self,
        action: Action,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        route: DeliveryRoute,
    ) -> Result<Value> {
        self.cancellation.check()?;
        ensure!(self.lane.is_some(), "isolated input lane is not claimed");
        self.check_route(route)?;
        ensure!(
            route == DeliveryRoute::Foreground
                || !matches!(&action, Action::Activate | Action::TextKey { .. }),
            "action requires foreground input"
        );
        ensure!(
            self.delivery_route.is_none_or(|bound| bound == route),
            "input route is immutable"
        );
        self.delivery_route = Some(route);
        self.attest()?;
        let target = self.bind_target(&action, route)?;
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
        let mut reply = self.dispatch_routed(&packet, is_drag, started, route)?;
        if reply["ok"] == false && self.protocol == InputProtocol::Experiment {
            // Never report a pending operator grant as successful dispatch.
            reply["epoch"] = json!(self.epoch);
            reply["challenge"] = json!(self.challenge);
            reply["target"] = json!(token);
            reply["revision"] = json!(revision);
        }
        Ok(reply)
    }

    #[cfg(test)]
    fn dispatch(
        &self,
        packet: &str,
        is_drag: bool,
        started: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<Value> {
        self.dispatch_routed(packet, is_drag, started, DeliveryRoute::Background)
    }

    fn dispatch_routed(
        &self,
        packet: &str,
        is_drag: bool,
        started: Option<tokio::sync::oneshot::Sender<()>>,
        route: DeliveryRoute,
    ) -> Result<Value> {
        let mut reply = self
            .request(packet)
            .map_err(|error| unknown_dispatch(error, 0))?;
        let acknowledged = reply["ok"] == true && reply["phase"] == "started";
        if acknowledged {
            if !is_drag {
                return Err(unknown_dispatch(
                    anyhow::anyhow!("unexpected input start acknowledgement"),
                    0,
                ));
            }
            if let Some(started) = started {
                let _ = started.send(());
            }
            reply = self
                .receive(Instant::now() + TIMEOUT)
                .map_err(|error| unknown_dispatch(error, 1))?;
        }
        if route == DeliveryRoute::Foreground
            && reply["ok"] == false
            && reply["code"] == "foreground_partial_unknown"
        {
            return Err(unknown_dispatch(
                anyhow::anyhow!("foreground activation or input may have started"),
                u32::from(acknowledged),
            ));
        }
        if reply["ok"] == true {
            if reply["effect"] != "unverifiable" || reply["route"] != route.acknowledgement() {
                return Err(unknown_dispatch(
                    anyhow::anyhow!("invalid action acknowledgement"),
                    u32::from(acknowledged),
                ));
            }
        } else if acknowledged {
            reply["effect"] = json!("partial");
            reply["delivery"] = json!({"mode":route.mode(),"delivered_count":1});
        }
        Ok(reply)
    }

    #[cfg(test)]
    fn target_packet(&self, action: &Action) -> String {
        self.target_packet_routed(action, DeliveryRoute::Background)
    }

    fn check_route(&self, route: DeliveryRoute) -> Result<()> {
        ensure!(
            route == DeliveryRoute::Background
                || (self.protocol == InputProtocol::Production && self.foreground_supported),
            "Hyprland foreground input is unavailable; no fallback or downgrade is permitted"
        );
        Ok(())
    }

    fn bind_target(&self, action: &Action, route: DeliveryRoute) -> Result<Value> {
        self.check_route(route)?;
        let target = self.request(&self.target_packet_routed(action, route))?;
        if target["ok"] == true {
            validate_target_route(&target, route)?;
        }
        Ok(target)
    }

    fn target_packet_routed(&self, action: &Action, route: DeliveryRoute) -> String {
        let command = match route {
            DeliveryRoute::Background => "TARGET",
            DeliveryRoute::Foreground => "FOREGROUND_TARGET",
        };
        let target = format!("{command} {} {:x}", self.pid, self.address);
        if self.protocol == InputProtocol::Production {
            format!("{target} {}", action.capability())
        } else {
            target
        }
    }
}

impl Action {
    fn capability(&self) -> u8 {
        match self {
            Self::Activate => 16,
            Self::Click { .. } => 1,
            Self::Key { .. } | Self::TextKey { .. } => 2,
            Self::Scroll { .. } => 4,
            Self::Drag { .. } => 8,
        }
    }

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
            Self::Activate => format!("ACTIVATE {prefix}"),
            Self::TextKey { keycode, shift } => {
                ensure!((1..=57).contains(&keycode), "unsupported text key");
                format!("KEY {prefix} {keycode} {}", u8::from(shift))
            }
            Self::Click {
                x,
                y,
                button,
                count,
            } => {
                point(x, y)?;
                ensure!(
                    (1..=2).contains(&count),
                    "isolated input supports only single or double clicks"
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
                    "isolated drag duration must be 50–2000 ms"
                );
                format!("DRAG {prefix} {x1} {y1} {x2} {y2} {duration_ms}")
            }
        })
    }
}

struct SessionClient {
    client: Mutex<Option<Client>>,
}

impl SessionClient {
    fn lock(&self, cancellation: &ActionCancellation) -> Result<MutexGuard<'_, Option<Client>>> {
        loop {
            cancellation.check()?;
            match self.client.try_lock() {
                Ok(mut slot) => {
                    // Cancellation may race with the previous owner releasing.
                    // Refuse before touching its connection or sending packets.
                    cancellation.check()?;
                    // The compositor can expire an idle connection while its
                    // owner remains active. Reconnect before any new dispatch,
                    // keeping queued callers on this same serialization mutex.
                    if slot.as_ref().is_some_and(Client::peer_closed) {
                        *slot = None;
                    }
                    return Ok(slot);
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(CANCELLATION_POLL),
                Err(TryLockError::Poisoned(_)) => bail!("input connection poisoned"),
            }
        }
    }
}

/// The compositor owns the cross-process allocation. No target or action has
/// been sent while this bounded search is running, and errors never fall back.
fn claim_available(mut connect: impl FnMut(usize) -> Result<Option<Client>>) -> Result<Client> {
    for lane in 0..MAX_LANES {
        if let Some(client) = connect(lane)? {
            return Ok(client);
        }
    }
    Err(LaneBusy.into())
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
    // Failed claims leave empty slots; the compositor may also close idle
    // connections without their owning sessions making another call.
    // Reclaim only idle entries: queued callers must keep the same mutex.
    // Never wait for a per-session mutex while holding the pool lock.
    clients.retain(|_, client| {
        Arc::strong_count(client) > 1
            || client.client.try_lock().map_or(true, |slot| {
                slot.as_ref().is_some_and(|client| !client.peer_closed())
            })
    });
    if clients.len() >= MAX_LANES {
        return Err(LaneBusy.into());
    }
    let client = Arc::new(SessionClient {
        client: Mutex::new(None),
    });
    clients.insert(owner.to_owned(), client.clone());
    Ok(client)
}

pub fn cleanup_session(owner: &str) {
    let client = clients().lock().unwrap().get(owner).cloned();
    if let Some(client) = client {
        // In-flight waits observe the common termination signal and drop
        // their owned socket promptly. Final cleanup still uses the barrier.
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
pub(crate) fn execute(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    cancellation: ActionCancellation,
) -> Result<Value> {
    execute_with_started(owner, pid, address, action, None, cancellation)
}

pub(crate) fn execute_with_started(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    cancellation: ActionCancellation,
) -> Result<Value> {
    execute_routed(
        owner,
        pid,
        address,
        action,
        started,
        cancellation,
        DeliveryRoute::Background,
    )
}

pub(crate) fn execute_foreground(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    cancellation: ActionCancellation,
) -> Result<Value> {
    execute_foreground_with_started(owner, pid, address, action, None, cancellation)
}

pub(crate) fn execute_foreground_with_started(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    cancellation: ActionCancellation,
) -> Result<Value> {
    execute_routed(
        owner,
        pid,
        address,
        action,
        started,
        cancellation,
        DeliveryRoute::Foreground,
    )
}

fn execute_routed(
    owner: Option<String>,
    pid: u32,
    address: u64,
    action: Action,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    cancellation: ActionCancellation,
    route: DeliveryRoute,
) -> Result<Value> {
    execute_actions_routed(
        owner,
        pid,
        address,
        vec![action],
        started,
        cancellation,
        route,
        false,
    )
}

pub(crate) fn execute_foreground_text(
    owner: Option<String>,
    pid: u32,
    address: u64,
    text: &str,
    cancellation: ActionCancellation,
) -> Result<Value> {
    let actions = foreground_text_actions(text)?;
    ensure!(!actions.is_empty(), "foreground text must not be empty");
    execute_actions_routed(
        owner,
        pid,
        address,
        actions,
        None,
        cancellation,
        DeliveryRoute::Foreground,
        true,
    )
}

fn execute_actions_routed(
    owner: Option<String>,
    pid: u32,
    address: u64,
    actions: Vec<Action>,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    cancellation: ActionCancellation,
    route: DeliveryRoute,
    text: bool,
) -> Result<Value> {
    cancellation.check()?;
    ensure!(enabled(), "Hyprland isolated input is unavailable");
    ensure!(
        pid > 0 && address > 0,
        "exact pid and window address required"
    );
    let owner = owner.context("authenticated lifecycle required")?;
    ensure!(
        route == DeliveryRoute::Background || protocol() == InputProtocol::Production,
        "foreground input requires production protocol; no downgrade is permitted"
    );
    if route == DeliveryRoute::Background && protocol() == InputProtocol::Production {
        if let Err(reason) = super::hyprland_compatibility::qualify(pid) {
            return Ok(json!({"ok":false,"code":reason,"detail":reason}));
        }
    }
    let client = session_client(&owner)?;
    let mut slot = client.lock(&cancellation)?;
    if let Some(client) = slot.as_ref() {
        let lane = client.lane.context("missing claimed input lane")?;
        if client.pid != pid
            || client.address != address
            || client.path != socket_path(lane)?
            || client.protocol != protocol()
            || client.delivery_route.is_some_and(|bound| bound != route)
        {
            *slot = None;
        }
    }
    if slot.is_none() {
        *slot = Some(claim_available(|lane| {
            Client::connect(
                socket_path(lane)?,
                owner.clone(),
                pid,
                address,
                lane,
                cancellation.clone(),
            )
        })?);
    }
    slot.as_mut()
        .context("missing input connection")?
        .cancellation = cancellation;
    dispatch_in_slot(&mut slot, |client| {
        if !text {
            return client.execute_routed(
                actions.into_iter().next().context("missing input action")?,
                started,
                route,
            );
        }
        execute_text_actions(actions, |action| client.execute_routed(action, None, route))
    })
}

fn execute_text_actions(
    actions: Vec<Action>,
    mut dispatch: impl FnMut(Action) -> Result<Value>,
) -> Result<Value> {
    let route = DeliveryRoute::Foreground;
    let mut delivered = 0u32;
    for action in actions {
        match dispatch(action) {
            Ok(mut reply) if reply["ok"] == false => {
                reply["effect"] = json!(if delivered > 0 { "partial" } else { "none" });
                reply["delivery"] = json!({"mode":route.mode(),"delivered_count":delivered});
                return Ok(reply);
            }
            Ok(_) => delivered += 1,
            Err(error) => {
                if let Some(unknown) = error.downcast_ref::<DispatchUnknown>() {
                    return Err(unknown_dispatch(
                        anyhow::anyhow!(unknown.detail.clone()),
                        delivered + unknown.acknowledged_phases,
                    ));
                }
                if delivered == 0 {
                    return Err(error);
                }
                return Ok(
                    json!({"ok":false,"code":"text_interrupted","detail":error.to_string(),
                        "effect":"partial","delivery":{"mode":route.mode(),"delivered_count":delivered}}),
                );
            }
        }
    }
    Ok(
        json!({"ok":true,"effect":"unverifiable","route":route.acknowledgement(),
            "delivery":{"mode":route.mode(),"delivered_count":delivered}}),
    )
}

fn dispatch_in_slot(
    slot: &mut Option<Client>,
    dispatch: impl FnOnce(&mut Client) -> Result<Value>,
) -> Result<Value> {
    let lane = slot
        .as_ref()
        .and_then(|client| client.lane)
        .context("missing claimed input lane")?;
    let mut result = dispatch(slot.as_mut().context("missing input connection")?);
    if result.as_ref().map_or(true, terminal_connection_result) {
        *slot = None;
    }
    if let Ok(value) = &mut result {
        // Report the compositor-assigned lane for diagnostics. It is not a
        // credential or a caller-selected seat.
        value["lane"] = json!(lane);
    }
    result
}

fn terminal_connection_result(value: &Value) -> bool {
    value["ok"] == false
        && matches!(
            value["code"].as_str(),
            Some(
                "desktop_changed"
                    | "plugin_disabled"
                    | "plugin_shutdown"
                    | "generation_exhausted"
                    | "text_interrupted"
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    static POOL_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn foreground_requires_advertisement_and_production_without_sending_target() {
        for protocol in [InputProtocol::Production, InputProtocol::Experiment] {
            let (mut client, peer) = test_connection();
            client.protocol = protocol;
            client.foreground_supported = protocol == InputProtocol::Experiment;
            assert!(client
                .bind_target(&Action::Activate, DeliveryRoute::Foreground)
                .is_err());
            drop(client);
            let mut byte = [0u8];
            assert_eq!(
                unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                0
            );
        }
    }

    #[test]
    fn foreground_target_checks_route_and_never_retries_background() {
        for reply in [
            json!({"ok":true,"route":"primary_foreground"}),
            json!({"ok":true,"route":"synthetic_events"}),
            json!({"ok":true}),
            json!({"ok":false,"code":"unsupported","detail":"unsupported"}),
        ] {
            let (mut client, peer) = test_connection();
            client.protocol = InputProtocol::Production;
            client.foreground_supported = true;
            let expected_ok = reply["ok"] == false || reply["route"] == "primary_foreground";
            let server = std::thread::spawn(move || {
                assert_eq!(read_packet(&peer), "FOREGROUND_TARGET 1 1 16");
                peer.send(reply.to_string().as_bytes()).unwrap();
                let mut byte = [0u8];
                assert_eq!(
                    unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                    0
                );
            });
            assert_eq!(
                client
                    .bind_target(&Action::Activate, DeliveryRoute::Foreground)
                    .is_ok(),
                expected_ok
            );
            drop(client);
            server.join().unwrap();
        }
    }

    #[test]
    fn foreground_handshake_records_capability_and_exact_claim() {
        let (mut client, peer) = test_connection();
        client.protocol = InputProtocol::Production;
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "HELLO");
            peer.send(
                json!({"ok":true,"protocol":3,"epoch":TOKEN,"foreground_target":true})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
            assert_eq!(read_packet(&peer), "CLAIM");
            peer.send(br#"{"ok":true,"lane":0}"#).unwrap();
        });
        assert!(client.handshake(0).unwrap());
        assert!(client.foreground_supported);
        server.join().unwrap();
    }

    #[test]
    fn foreground_drag_refusal_preserves_started_phase_and_foreground_count() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "DRAG synthetic");
            peer.send(br#"{"ok":true,"phase":"started"}"#).unwrap();
            peer.send(br#"{"ok":false,"code":"cancelled","detail":"cancelled"}"#)
                .unwrap();
        });
        let (started, acknowledgement) = tokio::sync::oneshot::channel();
        let reply = client
            .dispatch_routed(
                "DRAG synthetic",
                true,
                Some(started),
                DeliveryRoute::Foreground,
            )
            .unwrap();
        assert!(acknowledgement.blocking_recv().is_ok());
        assert_eq!(reply["effect"], "partial");
        assert_eq!(
            reply["delivery"],
            json!({"mode":"foreground","delivered_count":1})
        );
        server.join().unwrap();
    }

    #[test]
    fn foreground_action_cannot_acknowledge_background_delivery() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "KEY synthetic");
            peer.send(br#"{"ok":true,"effect":"unverifiable","route":"synthetic_events"}"#)
                .unwrap();
        });
        assert!(client
            .dispatch_routed("KEY synthetic", false, None, DeliveryRoute::Foreground)
            .unwrap_err()
            .is::<DispatchUnknown>());
        server.join().unwrap();
    }

    #[test]
    fn foreground_partial_unknown_keeps_only_proven_acknowledgements() {
        for started in [false, true] {
            let (client, peer) = test_connection();
            let packet = if started {
                "DRAG synthetic"
            } else {
                "KEY synthetic"
            };
            let server = std::thread::spawn(move || {
                assert_eq!(read_packet(&peer), packet);
                if started {
                    peer.send(br#"{"ok":true,"phase":"started"}"#).unwrap();
                }
                peer.send(br#"{"ok":false,"code":"foreground_partial_unknown","detail":"foreground_partial_unknown"}"#).unwrap();
                let mut byte = [0u8];
                assert_eq!(
                    unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                    0
                );
            });
            let error = client
                .dispatch_routed(packet, started, None, DeliveryRoute::Foreground)
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<DispatchUnknown>()
                    .unwrap()
                    .acknowledged_phases,
                u32::from(started)
            );
            drop(client);
            server.join().unwrap();
        }
    }

    #[test]
    fn foreground_text_partial_unknown_counts_prior_completed_keys() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "KEY synthetic");
            peer.send(br#"{"ok":true,"effect":"unverifiable","route":"primary_foreground"}"#)
                .unwrap();
            assert_eq!(read_packet(&peer), "KEY synthetic");
            peer.send(br#"{"ok":false,"code":"foreground_partial_unknown","detail":"foreground_partial_unknown"}"#).unwrap();
            let mut byte = [0u8];
            assert_eq!(
                unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                0
            );
        });
        let error = execute_text_actions(foreground_text_actions("abc").unwrap(), |_| {
            client.dispatch_routed("KEY synthetic", false, None, DeliveryRoute::Foreground)
        })
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<DispatchUnknown>()
                .unwrap()
                .acknowledged_phases,
            1
        );
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn foreground_unknown_code_does_not_change_background_refusals() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "KEY synthetic");
            peer.send(br#"{"ok":false,"code":"foreground_partial_unknown","detail":"foreground_partial_unknown"}"#).unwrap();
        });
        let reply = client.dispatch("KEY synthetic", false, None).unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["code"], "foreground_partial_unknown");
        server.join().unwrap();
    }

    #[test]
    fn foreground_text_is_bounded_and_fully_prevalidated() {
        for text in ["valid prefix\u{00e9}", "valid prefix\0", "\r", "\u{007f}"] {
            assert!(foreground_text_actions(text).is_err());
        }
        assert!(foreground_text_actions(&"a".repeat(4097)).is_err());
        assert_eq!(
            foreground_text_actions(&"a".repeat(4096)).unwrap().len(),
            4096
        );
        let actions = foreground_text_actions("Aa!? \t\n").unwrap();
        let packets: Vec<_> = actions
            .into_iter()
            .map(|action| action.packet(1, TOKEN, 1, 1.0, 1.0).unwrap())
            .collect();
        for (packet, suffix) in packets
            .iter()
            .zip(["30 1", "30 0", "2 1", "53 1", "57 0", "15 0", "28 0"])
        {
            assert_eq!(*packet, format!("KEY 1 {TOKEN} 1 {suffix}"));
        }
        assert_eq!(
            foreground_text_actions(&(32u8..127).map(char::from).collect::<String>())
                .unwrap()
                .len(),
            95
        );
    }

    #[test]
    fn foreground_text_stops_after_refusal_or_cancellation_without_replay() {
        for cancel in [false, true] {
            let mut calls = 0;
            let reply = execute_text_actions(foreground_text_actions("abc").unwrap(), |_| {
                calls += 1;
                match calls {
                    1 => Ok(json!({"ok":true})),
                    2 if cancel => Err(ActionCancelled.into()),
                    2 => Ok(json!({"ok":false,"code":"target_changed","detail":"target_changed"})),
                    _ => panic!("replayed text after interrupted dispatch"),
                }
            })
            .unwrap();
            assert_eq!(calls, 2);
            assert_eq!(reply["ok"], false);
            assert_eq!(reply["effect"], "partial");
            assert_eq!(
                reply["delivery"],
                json!({"mode":"foreground","delivered_count":1})
            );
            if cancel {
                assert!(terminal_connection_result(&reply));
            }
        }
    }

    #[test]
    fn foreground_text_unknown_delivery_keeps_only_acknowledged_key_count() {
        let mut calls = 0;
        let error = execute_text_actions(foreground_text_actions("abc").unwrap(), |_| {
            calls += 1;
            if calls == 1 {
                Ok(json!({"ok":true}))
            } else {
                Err(unknown_dispatch(anyhow::anyhow!("peer closed"), 0))
            }
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(
            error
                .downcast_ref::<DispatchUnknown>()
                .unwrap()
                .acknowledged_phases,
            1
        );
    }

    #[test]
    fn cancelled_foreground_target_sends_no_input() {
        let (mut client, peer) = test_connection();
        client.protocol = InputProtocol::Production;
        client.foreground_supported = true;
        let (guard, cancellation) = ActionCancellation::invocation();
        client.cancellation = cancellation;
        drop(guard);
        assert!(client
            .bind_target(&Action::Activate, DeliveryRoute::Foreground)
            .unwrap_err()
            .is::<ActionCancelled>());
        drop(client);
        let mut byte = [0u8];
        assert_eq!(
            unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
            0
        );
    }

    #[test]
    fn revoked_connections_are_dropped_without_replaying_the_action() {
        for code in [
            "desktop_changed",
            "plugin_disabled",
            "plugin_shutdown",
            "generation_exhausted",
        ] {
            assert!(terminal_connection_result(
                &json!({"ok": false, "code": code})
            ));
        }
        for value in [
            json!({"ok": true}),
            json!({"ok": false, "code": "pending_operator_approval"}),
            json!({"ok": false, "code": "primary_target_busy"}),
        ] {
            assert!(!terminal_connection_result(&value));
        }
    }

    #[test]
    fn independent_lifecycles_have_bounded_serialization_slots_and_cleanup() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        assert!(session_client("").is_err());
        assert!(session_client("default").is_err());
        let a = session_client("input-pool-test-a").unwrap();
        let b = session_client("input-pool-test-b").unwrap();
        let pool = Arc::new(Mutex::new([None, None]));
        *a.client.lock().unwrap() = Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        *b.client.lock().unwrap() = Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        assert!(!Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(
            &a,
            &session_client("input-pool-test-a").unwrap()
        ));
        assert!(session_client("input-pool-test-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        let a_lock = a.client.lock().unwrap();
        // No global action mutex: another seat remains independently usable.
        assert!(b.client.try_lock().is_ok());
        drop(a_lock);
        cleanup_session("input-pool-test-a");
        let c = session_client("input-pool-test-c").unwrap();
        assert!(!Arc::ptr_eq(&c, &a));
        let reclaimed = claim_available(|lane| fake_claim(lane, &pool)).unwrap();
        assert_eq!(reclaimed.lane, Some(0));
        *c.client.lock().unwrap() = Some(reclaimed);
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
        assert!(!Arc::ptr_eq(&reused, &runtime));
        cleanup_session("__cua_runtime_other:pending");
        cleanup_session("input-pool-test-reused");
    }

    #[test]
    fn failed_owners_do_not_exhaust_local_lanes() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        for failure in ["connect", "HELLO", "CLAIM"] {
            for owner in ["input-failed-a", "input-failed-b"] {
                let client = session_client(owner).unwrap();
                let mut slot = client.client.lock().unwrap();
                let mut attempts = Vec::new();
                let result = claim_available(|lane| {
                    attempts.push(lane);
                    if failure == "connect" {
                        bail!("connection unavailable");
                    }
                    let (mut client, peer) = test_connection();
                    let server = std::thread::spawn(move || {
                        assert_eq!(read_packet(&peer), "HELLO");
                        if failure == "CLAIM" {
                            peer.send(hello_reply().to_string().as_bytes()).unwrap();
                            assert_eq!(read_packet(&peer), "CLAIM");
                        }
                        peer.send(b"not-json").unwrap();
                    });
                    let result = client.handshake(lane);
                    server.join().unwrap();
                    result.map(|claimed| claimed.then_some(client))
                });
                assert!(result.is_err());
                assert_eq!(attempts, [0]);
                *slot = result.ok();
            }
            let pool = Arc::new(Mutex::new([None, None]));
            let third = session_client("input-failed-third").unwrap();
            *third.client.lock().unwrap() =
                Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
            assert_eq!(third.client.lock().unwrap().as_ref().unwrap().lane, Some(0));
            cleanup_session("input-failed-third");
            cleanup_session("input-failed-a");
            cleanup_session("input-failed-b");
        }
    }

    #[test]
    fn failed_reconnect_reclaims_only_the_empty_owner() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        let pool = Arc::new(Mutex::new([None, None]));
        for owner in ["input-reconnect-a", "input-reconnect-b"] {
            let client = session_client(owner).unwrap();
            *client.client.lock().unwrap() =
                Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        }
        assert!(session_client("input-reconnect-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        {
            let client = session_client("input-reconnect-a").unwrap();
            let mut slot = client.client.lock().unwrap();
            *slot = None;
            assert!(claim_available(|_| bail!("reconnect unavailable")).is_err());
        }
        let c = session_client("input-reconnect-c").unwrap();
        *c.client.lock().unwrap() = Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        assert_eq!(c.client.lock().unwrap().as_ref().unwrap().lane, Some(0));
        let b = session_client("input-reconnect-b").unwrap();
        assert_eq!(b.client.lock().unwrap().as_ref().unwrap().lane, Some(1));
        for owner in [
            "input-reconnect-a",
            "input-reconnect-b",
            "input-reconnect-c",
        ] {
            cleanup_session(owner);
        }
    }

    #[test]
    fn closed_idle_peers_do_not_exhaust_local_lanes() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        let pool = Arc::new(Mutex::new([None, None]));
        for owner in ["input-idle-a", "input-idle-b"] {
            let client = session_client(owner).unwrap();
            *client.client.lock().unwrap() =
                Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        }
        assert!(session_client("input-idle-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        // The compositor revokes both idle connections; neither owner calls
        // again to discover EOF or explicitly clean up its cached client.
        *pool.lock().unwrap() = [None, None];
        let c = session_client("input-idle-c").unwrap();
        *c.client.lock().unwrap() = Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        assert_eq!(c.client.lock().unwrap().as_ref().unwrap().lane, Some(0));
        let clients = clients().lock().unwrap();
        assert!(!clients.contains_key("input-idle-a"));
        assert!(!clients.contains_key("input-idle-b"));
        drop(clients);
        cleanup_session("input-idle-c");
    }

    #[test]
    fn same_owner_resumes_after_idle_eof_under_the_existing_mutex() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        let owner = "input-idle-resume";
        let session = session_client(owner).unwrap();
        let (mut client, peer) = test_connection();
        client.lane = Some(0);
        let (previous_guard, previous_cancellation) = ActionCancellation::invocation();
        client.cancellation = previous_cancellation;
        *session.client.lock().unwrap() = Some(client);
        drop(previous_guard);
        drop(peer);

        let resumed = session_client(owner).unwrap();
        assert!(Arc::ptr_eq(&session, &resumed));
        let (_guard, cancellation) = ActionCancellation::invocation();
        let mut slot = resumed.lock(&cancellation).unwrap();
        assert!(
            slot.is_none(),
            "closed idle peer must be discarded before dispatch"
        );
        assert!(session.client.try_lock().is_err());
        let (mut replacement, peer) = test_connection();
        replacement.cancellation = cancellation;
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "HELLO");
            peer.send(hello_reply().to_string().as_bytes()).unwrap();
            assert_eq!(read_packet(&peer), "CLAIM");
            peer.send(br#"{"ok":true,"lane":0}"#).unwrap();
            assert_eq!(read_packet(&peer), "TARGET synthetic");
            peer.send(br#"{"ok":true}"#).unwrap();
            assert_eq!(read_packet(&peer), "CLICK synthetic");
            peer.send(br#"{"ok":true,"effect":"unverifiable","route":"synthetic_events"}"#)
                .unwrap();
            // Keep the peer open until the reply has been consumed.
            let mut byte = [0u8];
            assert_eq!(
                unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                0
            );
        });
        let mut replacement = Some(replacement);
        let mut claims = 0;
        *slot = Some(
            claim_available(|lane| {
                claims += 1;
                let mut client = replacement.take().unwrap();
                assert!(client.handshake(lane)?);
                Ok(Some(client))
            })
            .unwrap(),
        );
        let reply = dispatch_in_slot(&mut slot, |client| {
            client.request("TARGET synthetic")?;
            client.dispatch("CLICK synthetic", false, None)
        })
        .unwrap();
        assert_eq!(claims, 1);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["lane"], 0);
        drop(slot);
        cleanup_session(owner);
        server.join().unwrap();
    }

    #[test]
    fn uncertain_io_after_reuse_is_an_error_without_replay() {
        for malformed_reply in [false, true] {
            let (mut client, peer) = test_connection();
            client.lane = Some(0);
            let session = SessionClient {
                client: Mutex::new(Some(client)),
            };
            let mut slot = session.lock(&ActionCancellation::default()).unwrap();
            assert!(slot.is_some(), "a live peer must remain reusable");
            let server = std::thread::spawn(move || {
                assert_eq!(read_packet(&peer), "CLICK synthetic");
                if malformed_reply {
                    peer.send(b"not-json").unwrap();
                } else {
                    peer.shutdown(std::net::Shutdown::Write).unwrap();
                }
                // The uncertain connection must close without another packet.
                let mut byte = [0u8];
                assert_eq!(
                    unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
                    0
                );
            });
            let mut dispatches = 0;
            let error = dispatch_in_slot(&mut slot, |client| {
                dispatches += 1;
                client.dispatch("CLICK synthetic", false, None)
            })
            .unwrap_err();
            assert!(error.is::<DispatchUnknown>());
            assert_eq!(dispatches, 1);
            assert!(slot.is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn live_idle_peers_with_pending_packets_keep_their_lanes() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        let pool = Arc::new(Mutex::new([None, None]));
        for owner in ["input-live-a", "input-live-b"] {
            let client = session_client(owner).unwrap();
            *client.client.lock().unwrap() =
                Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        }
        {
            let pool = pool.lock().unwrap();
            pool[0].as_ref().unwrap().send(b"").unwrap();
            pool[1].as_ref().unwrap().send(b"pending reply").unwrap();
        }
        assert!(session_client("input-live-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        // Reclaim only the closed peer, preserving the other idle client.
        pool.lock().unwrap()[0] = None;
        let c = session_client("input-live-c").unwrap();
        *c.client.lock().unwrap() = Some(claim_available(|lane| fake_claim(lane, &pool)).unwrap());
        assert_eq!(c.client.lock().unwrap().as_ref().unwrap().lane, Some(0));
        let b = session_client("input-live-b").unwrap();
        let slot = b.client.lock().unwrap();
        assert_eq!(slot.as_ref().unwrap().lane, Some(1));
        assert_eq!(read_packet(&slot.as_ref().unwrap().socket), "pending reply");
        drop(slot);
        cleanup_session("input-live-b");
        cleanup_session("input-live-c");
    }

    #[test]
    fn pending_same_owner_callers_keep_their_serialization_slot() {
        let _pool_test = POOL_TEST_LOCK.lock().unwrap();
        let owner = "input-pending-a";
        let active = session_client(owner).unwrap();
        let (client, peer) = test_connection();
        *active.client.lock().unwrap() = Some(client);
        // Even a conclusively closed socket must not replace the mutex while
        // active or queued callers still hold this owner's serialization slot.
        drop(peer);
        let slot = active.client.lock().unwrap();
        let queued = session_client(owner).unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            ready.send(()).unwrap();
            let _slot = queued.client.lock().unwrap();
            released.recv().unwrap();
        });
        waiting.recv().unwrap();
        let b = session_client("input-pending-b").unwrap();
        assert!(session_client("input-pending-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        assert!(Arc::ptr_eq(&active, &session_client(owner).unwrap()));
        drop(slot);
        drop(active);
        assert!(session_client("input-pending-c")
            .err()
            .unwrap()
            .is::<LaneBusy>());
        release.send(()).unwrap();
        waiter.join().unwrap();
        let c = session_client("input-pending-c").unwrap();
        assert!(Arc::ptr_eq(&b, &session_client("input-pending-b").unwrap()));
        drop(c);
        for owner in [owner, "input-pending-b", "input-pending-c"] {
            cleanup_session(owner);
        }
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
                cancellation: ActionCancellation::default(),
                protocol: InputProtocol::Experiment,
                foreground_supported: false,
                delivery_route: None,
                lane: None,
                owner: "input-unregistered-test-client".into(),
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

    fn read_packet(peer: &socket2::Socket) -> String {
        let mut bytes = [0u8; MAX_PACKET];
        let count =
            unsafe { libc::recv(peer.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0) };
        assert!(count > 0);
        String::from_utf8(bytes[..count as usize].to_vec()).unwrap()
    }

    #[test]
    fn peer_closure_probe_is_nonblocking_and_does_not_consume_packets() {
        let (client, peer) = test_connection();
        let started = Instant::now();
        assert!(!client.peer_closed());
        assert!(started.elapsed() < TIMEOUT);

        peer.send(b"").unwrap();
        peer.send(b"pending reply").unwrap();
        assert!(!client.peer_closed());
        assert!(!client.peer_closed());
        let mut byte = [0u8];
        assert_eq!(
            unsafe {
                libc::recv(
                    client.socket.as_raw_fd(),
                    byte.as_mut_ptr().cast(),
                    byte.len(),
                    libc::MSG_DONTWAIT,
                )
            },
            0
        );
        assert_eq!(read_packet(&client.socket), "pending reply");
        assert!(!client.peer_closed());

        peer.send(b"last reply").unwrap();
        drop(peer);
        assert!(client.peer_closed());
        assert_eq!(read_packet(&client.socket), "last reply");
        assert!(client.peer_closed());
    }

    #[test]
    fn peer_write_shutdown_is_conclusive_even_with_a_pending_empty_packet() {
        let (client, peer) = test_connection();
        peer.send(b"").unwrap();
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(client.peer_closed());
    }

    #[tokio::test]
    async fn aborted_queued_invocation_never_targets_or_changes_the_active_connection() {
        let (client, peer) = test_connection();
        let session = Arc::new(SessionClient {
            client: Mutex::new(Some(client)),
        });
        let first = session.client.lock().unwrap();
        let queued = session.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let call = tokio::spawn(async move {
            let (_guard, cancellation) = ActionCancellation::invocation();
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                let result = queued
                    .lock(&cancellation)
                    .and_then(|slot| slot.as_ref().unwrap().request("TARGET synthetic"));
                finished.send(result).unwrap();
            })
            .await
            .unwrap();
        });
        waiting.await.unwrap();
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        // The canceled queue entry exits even while the first owns the lane.
        assert!(tokio::time::timeout(Duration::from_secs(1), done)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .is::<ActionCancelled>());
        assert!(first.is_some());
        drop(first);
        let mut byte = [0u8];
        assert_eq!(
            unsafe {
                libc::recv(
                    peer.as_raw_fd(),
                    byte.as_mut_ptr().cast(),
                    1,
                    libc::MSG_DONTWAIT,
                )
            },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
        // The first connection is still usable; cancellation didn't clear it.
        peer.send(br#"{"ok":true}"#).unwrap();
        assert_eq!(
            session
                .client
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .receive(Instant::now() + TIMEOUT)
                .unwrap()["ok"],
            true
        );
    }

    #[tokio::test]
    async fn aborted_acknowledged_drag_disconnects_only_its_own_lane_without_replay() {
        let (mut client, peer) = test_connection();
        client.lane = Some(0);
        let (other, other_peer) = test_connection();
        let (started, acknowledged) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "DRAG synthetic");
            peer.send(br#"{"ok":true,"phase":"started"}"#).unwrap();
            let mut bytes = [0u8; MAX_PACKET];
            // EOF is the plugin's connection-scoped revoke/release signal.
            // There must be no replay or unrelated release packet.
            assert_eq!(
                unsafe { libc::recv(peer.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0) },
                0
            );
        });
        let call = tokio::spawn(async move {
            let (_guard, cancellation) = ActionCancellation::invocation();
            tokio::task::spawn_blocking(move || {
                client.cancellation = cancellation;
                let mut slot = Some(client);
                let result = dispatch_in_slot(&mut slot, |client| {
                    client.dispatch("DRAG synthetic", true, Some(started))
                });
                assert!(slot.is_none());
                finished.send(result).unwrap();
            })
            .await
            .unwrap();
        });
        acknowledged.await.unwrap();
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        let error = tokio::time::timeout(Duration::from_secs(1), done)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<DispatchUnknown>()
                .unwrap()
                .acknowledged_phases,
            1
        );
        server.join().unwrap();
        other_peer.send(br#"{"ok":true}"#).unwrap();
        assert_eq!(other.receive(Instant::now() + TIMEOUT).unwrap()["ok"], true);
    }

    fn hello_reply() -> Value {
        json!({"ok":true,"protocol":0,"epoch":TOKEN,"challenge":TOKEN})
    }

    #[test]
    fn canceled_invocation_cannot_send_on_a_ready_socket() {
        let (mut client, peer) = test_connection();
        let (guard, cancellation) = ActionCancellation::invocation();
        client.cancellation = cancellation;
        drop(guard);
        for packet in [
            "HELLO",
            "CLAIM",
            "TARGET synthetic",
            "CLICK synthetic",
            "DRAG synthetic",
        ] {
            assert!(client.request(packet).unwrap_err().is::<ActionCancelled>());
        }
        let mut byte = [0u8];
        assert_eq!(
            unsafe {
                libc::recv(
                    peer.as_raw_fd(),
                    byte.as_mut_ptr().cast(),
                    1,
                    libc::MSG_DONTWAIT,
                )
            },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn interrupted_drag_preserves_acknowledged_progress_and_never_replays() {
        for final_reply in [
            Some(json!({"ok":false,"code":"cancelled","detail":"cancelled"})),
            None,
            Some(json!({"ok":true,"effect":"invalid"})),
        ] {
            let (client, peer) = test_connection();
            let expected_cancel = final_reply
                .as_ref()
                .is_some_and(|value| value["ok"] == false);
            let server = std::thread::spawn(move || {
                assert_eq!(read_packet(&peer), "DRAG synthetic");
                peer.send(br#"{"ok":true,"phase":"started"}"#).unwrap();
                if let Some(reply) = final_reply {
                    peer.send(reply.to_string().as_bytes()).unwrap();
                }
            });
            let (started, acknowledged) = tokio::sync::oneshot::channel();
            let result = client.dispatch("DRAG synthetic", true, Some(started));
            assert!(acknowledged.blocking_recv().is_ok());
            if expected_cancel {
                let reply = result.unwrap();
                assert_eq!(reply["effect"], "partial");
                assert_eq!(
                    reply["delivery"],
                    json!({"mode":"background","delivered_count":1})
                );
            } else {
                assert_eq!(
                    result
                        .unwrap_err()
                        .downcast_ref::<DispatchUnknown>()
                        .unwrap()
                        .acknowledged_phases,
                    1
                );
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn missing_initial_action_reply_cannot_claim_acknowledged_progress() {
        let (client, peer) = test_connection();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "CLICK synthetic");
        });
        let result = client.dispatch("CLICK synthetic", false, None).unwrap_err();
        assert_eq!(
            result
                .downcast_ref::<DispatchUnknown>()
                .unwrap()
                .acknowledged_phases,
            0
        );
        server.join().unwrap();
    }

    #[test]
    fn production_handshake_has_no_signer_and_claims_exact_lane() {
        let (mut client, peer) = test_connection();
        client.protocol = InputProtocol::Production;
        client.challenge.clear();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "HELLO");
            peer.send(
                json!({"ok":true,"protocol":3,"epoch":TOKEN})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
            assert_eq!(read_packet(&peer), "CLAIM");
            peer.send(br#"{"ok":true,"lane":1}"#).unwrap();
        });
        assert!(client.handshake(1).unwrap());
        server.join().unwrap();
        assert_eq!(client.lane, Some(1));
        assert!(client.challenge.is_empty());
    }

    #[test]
    fn production_refuses_experiment_and_unexpected_signer_without_claiming() {
        for hello in [
            hello_reply(),
            json!({"ok":true,"protocol":3,"epoch":TOKEN,"challenge":TOKEN}),
        ] {
            let (mut client, peer) = test_connection();
            client.protocol = InputProtocol::Production;
            let server = std::thread::spawn(move || {
                assert_eq!(read_packet(&peer), "HELLO");
                peer.send(hello.to_string().as_bytes()).unwrap();
                let mut bytes = [0u8; MAX_PACKET];
                // Closing after the refused HELLO must be the next event:
                // no CLAIM, TARGET, APPROVE, or input packet is sent.
                let count = unsafe {
                    libc::recv(peer.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0)
                };
                assert_eq!(count, 0);
            });
            assert!(client.handshake(0).is_err());
            assert!(client.lane.is_none());
            drop(client);
            server.join().unwrap();
        }
    }

    #[test]
    fn production_targets_bind_one_exact_operation_and_use_distinct_endpoints() {
        let (mut client, _peer) = test_connection();
        client.protocol = InputProtocol::Production;
        for (action, capability) in [
            (
                Action::Click {
                    x: 1.0,
                    y: 2.0,
                    button: 1,
                    count: 1,
                },
                1,
            ),
            (
                Action::Key {
                    key: "a".into(),
                    modifiers: vec![],
                },
                2,
            ),
            (
                Action::Scroll {
                    point: None,
                    direction: "up".into(),
                    amount: 1,
                },
                4,
            ),
            (
                Action::Drag {
                    from: (1.0, 2.0),
                    to: (3.0, 4.0),
                    duration_ms: 100,
                },
                8,
            ),
        ] {
            assert_eq!(
                client.target_packet(&action),
                format!("TARGET 1 1 {capability}")
            );
        }
        for lane in 0..MAX_LANES {
            assert_ne!(
                InputProtocol::Production.socket_name(lane).unwrap(),
                InputProtocol::Experiment.socket_name(lane).unwrap()
            );
        }
        assert!(InputProtocol::Production.socket_name(MAX_LANES).is_err());
    }

    // A compositor-side pool, deliberately independent of Driver's CLIENTS.
    // Retaining the peer models a reservation until the claimant disconnects.
    fn fake_claim(
        lane: usize,
        pool: &Arc<Mutex<[Option<socket2::Socket>; MAX_LANES]>>,
    ) -> Result<Option<Client>> {
        let (mut client, peer) = test_connection();
        let pool = pool.clone();
        let server = std::thread::spawn(move || {
            assert_eq!(read_packet(&peer), "HELLO");
            peer.send(hello_reply().to_string().as_bytes()).unwrap();
            assert_eq!(read_packet(&peer), "CLAIM");
            let mut pool = pool.lock().unwrap();
            if let Some(occupied) = pool[lane].as_ref() {
                let mut byte = [0u8];
                let count = unsafe {
                    libc::recv(
                        occupied.as_raw_fd(),
                        byte.as_mut_ptr().cast(),
                        1,
                        libc::MSG_DONTWAIT,
                    )
                };
                if count == 0 {
                    pool[lane] = None;
                } else {
                    assert_eq!(count, -1);
                    assert_eq!(
                        std::io::Error::last_os_error().kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                }
            }
            if pool[lane].is_some() {
                peer.send(br#"{"ok":false,"code":"lane_busy","detail":"lane_busy"}"#)
                    .unwrap();
            } else {
                peer.send(json!({"ok":true,"lane":lane}).to_string().as_bytes())
                    .unwrap();
                pool[lane] = Some(peer);
            }
        });
        let result = client.handshake(lane);
        server.join().unwrap();
        result.map(|claimed| claimed.then_some(client))
    }

    #[test]
    fn independent_claimants_use_compositor_reservations_and_disconnect_reuses_lane() {
        let pool = Arc::new(Mutex::new([None, None]));
        let mut attempts = Vec::new();
        let a = claim_available(|lane| {
            attempts.push(lane);
            fake_claim(lane, &pool)
        })
        .unwrap();
        assert_eq!(a.lane, Some(0));
        assert_eq!(attempts, [0]);
        attempts.clear();
        let b = claim_available(|lane| {
            attempts.push(lane);
            fake_claim(lane, &pool)
        })
        .unwrap();
        assert_eq!(b.lane, Some(1));
        assert_eq!(attempts, [0, 1]);
        attempts.clear();
        assert!(claim_available(|lane| {
            attempts.push(lane);
            fake_claim(lane, &pool)
        })
        .err()
        .unwrap()
        .is::<LaneBusy>());
        assert_eq!(attempts, [0, 1]);
        drop(a);
        let c = claim_available(|lane| fake_claim(lane, &pool)).unwrap();
        assert_eq!(c.lane, Some(0));
        assert_eq!(b.lane, Some(1));
        drop(b);
        let d = claim_available(|lane| fake_claim(lane, &pool)).unwrap();
        assert_eq!(d.lane, Some(1));
    }

    #[test]
    fn malformed_or_unexpected_handshake_never_tries_another_lane() {
        let cases = [
            (
                json!({"ok":true,"protocol":1,"epoch":TOKEN,"challenge":TOKEN}),
                None,
            ),
            (
                json!({"ok":true,"protocol":0,"epoch":"bad","challenge":TOKEN}),
                None,
            ),
            (
                json!({"ok":false,"code":"lane_busy","detail":"lane_busy"}),
                None,
            ),
            (hello_reply(), Some(json!({"ok":true}))),
            (hello_reply(), Some(json!({"ok":true,"lane":1}))),
            (hello_reply(), Some(json!({"ok":true,"lane":2}))),
            (hello_reply(), Some(json!({"ok":true,"lane":"0"}))),
            (hello_reply(), Some(json!({"ok":false,"code":"lane_busy"}))),
            (
                hello_reply(),
                Some(json!({"ok":false,"code":"lane_busy","detail":"unknown"})),
            ),
            (
                hello_reply(),
                Some(json!({"ok":false,"code":"stopped","detail":"stopped"})),
            ),
        ];
        for (hello, claim) in cases {
            let mut attempts = Vec::new();
            assert!(claim_available(|lane| {
                attempts.push(lane);
                let (mut client, peer) = test_connection();
                let hello = hello.clone();
                let claim = claim.clone();
                let server = std::thread::spawn(move || {
                    assert_eq!(read_packet(&peer), "HELLO");
                    peer.send(hello.to_string().as_bytes()).unwrap();
                    if let Some(claim) = claim {
                        assert_eq!(read_packet(&peer), "CLAIM");
                        peer.send(claim.to_string().as_bytes()).unwrap();
                    }
                });
                let result = client.handshake(lane);
                server.join().unwrap();
                assert!(client.lane.is_none());
                result.map(|claimed| claimed.then_some(client))
            })
            .is_err());
            assert_eq!(attempts, [0]);
        }
    }

    #[test]
    fn unknown_claim_outcome_and_connection_errors_never_try_another_lane() {
        for reply in [None, Some("not-json")] {
            let mut attempts = Vec::new();
            assert!(claim_available(|lane| {
                attempts.push(lane);
                let (mut client, peer) = test_connection();
                let server = std::thread::spawn(move || {
                    assert_eq!(read_packet(&peer), "HELLO");
                    peer.send(hello_reply().to_string().as_bytes()).unwrap();
                    assert_eq!(read_packet(&peer), "CLAIM");
                    if let Some(reply) = reply {
                        peer.send(reply.as_bytes()).unwrap();
                    }
                    // The claim may have succeeded before connection loss.
                });
                let result = client.handshake(lane);
                server.join().unwrap();
                result.map(|claimed| claimed.then_some(client))
            })
            .is_err());
            assert_eq!(attempts, [0]);
        }
        let mut attempts = Vec::new();
        assert!(claim_available(|lane| {
            attempts.push(lane);
            bail!("connection unavailable")
        })
        .is_err());
        assert_eq!(attempts, [0]);
    }

    #[test]
    fn pending_termination_interrupts_receive_before_cleanup_barrier() {
        use cua_driver_core::session::{self, SessionClientKind, SessionTransport};
        let owner = "input-cancellation-owner";
        let sid = "input-cancellation-runtime-id";
        let guard = session::begin_session_dispatch(
            sid,
            None,
            owner,
            true,
            SessionTransport::McpStdio,
            SessionClientKind::Mcp,
        )
        .unwrap();
        let (mut client, peer) = test_connection();
        client.owner = sid.into();
        let terminator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            assert!(session::end_session_for_owner(sid, owner));
        });
        let start = Instant::now();
        assert!(client
            .receive(start + TIMEOUT)
            .unwrap_err()
            .to_string()
            .contains("session ending"));
        assert!(start.elapsed() < Duration::from_millis(750));
        terminator.join().unwrap();
        assert!(
            !session::is_session_ended(sid),
            "cleanup still waits for the guard"
        );
        drop(client); // execute_with_started drops exactly this connection on error.
        let mut byte = [0u8];
        assert_eq!(
            unsafe { libc::recv(peer.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) },
            0
        );
        let (other, other_peer) = test_connection();
        other_peer.send(br#"{"ok":true}"#).unwrap();
        assert_eq!(other.receive(Instant::now() + TIMEOUT).unwrap()["ok"], true);
        drop(guard);
        assert!(session::is_session_ended(sid));
    }

    #[test]
    fn terminated_session_never_sends_even_when_socket_is_ready() {
        let (mut client, peer) = test_connection();
        client.owner = "input-terminated-runtime-id".into();
        cua_driver_core::session::end_session(&client.owner);
        assert!(client
            .request("HELLO")
            .unwrap_err()
            .to_string()
            .contains("session ending"));
        let mut byte = [0u8];
        assert_eq!(
            unsafe {
                libc::recv(
                    peer.as_raw_fd(),
                    byte.as_mut_ptr().cast(),
                    1,
                    libc::MSG_DONTWAIT,
                )
            },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
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
