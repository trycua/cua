//! Trusted KWin window identity adapter.
//!
//! The optional in-process KWin effect exposes stable window tokens, process
//! identity, geometry, active/minimized state, and stacking metadata through a
//! deliberately read-only D-Bus surface. It does not expose activation or input
//! mutation methods. KWin's portal/libei input path is still focus-bound: libei
//! delivers to whichever surface is focused when KWin processes the event.
//!
//! A focus check followed by global input therefore has an unavoidable TOCTOU
//! race. Until Cua has a KWin input API that binds each mutation to the target
//! token/window, this module intentionally refuses raw target-addressed input.
//! Window discovery remains available; input authorization does not.

use std::collections::HashSet;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::time::Duration;

use crate::x11::WindowInfo;

const DEST: &str = "org.cua.KWinTarget";
const KWIN_DEST: &str = "org.kde.KWin";
const PATH: &str = "/org/cua/KWinTarget";
const IFACE: &str = "org.cua.KWinTarget";
const DBUS_DEST: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
pub const PROTOCOL_VERSION: u32 = 1;
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
const GEOMETRY_DELTA_PX: i32 = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KwinWindow {
    pub token: u64,
    pub pid: u32,
    pub title: String,
    pub app_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub active: bool,
    pub minimized: bool,
    pub stacking: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrelationError {
    NoMatch,
    Ambiguous,
    WrongActiveTarget,
}

impl std::fmt::Display for CorrelationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMatch => f.write_str("no exact KWin window matches the AT-SPI target"),
            Self::Ambiguous => f.write_str("multiple KWin windows match the AT-SPI target"),
            Self::WrongActiveTarget => f.write_str("KWin reports a different active target"),
        }
    }
}

impl std::error::Error for CorrelationError {}

/// Whether KWin currently offers a target-bound raw-input capability that can
/// safely authorize portal/libei foreground mutations.
///
/// This is intentionally distinct from helper presence: the read-only helper
/// may be installed and usable for discovery while target-bound raw input is
/// unavailable. Keep this false until the input operation itself is bound to
/// the KWin target token/window.
pub fn available() -> bool {
    false
}

pub fn list_windows() -> Option<Vec<KwinWindow>> {
    snapshot_for_owner(&helper_owner()?)
}

pub fn list_window_infos() -> Option<Vec<WindowInfo>> {
    Some(
        list_windows()?
            .into_iter()
            .map(|window| WindowInfo {
                xid: window.token,
                pid: Some(window.pid),
                app_name: window.app_name,
                title: window.title,
                is_on_screen: !window.minimized && window.width > 0 && window.height > 0,
                z_index: Some(window.stacking),
                x: window.x,
                y: window.y,
                width: window.width,
                height: window.height,
            })
            .collect(),
    )
}

pub fn trusted_window_for_id(pid: u32, token: u64) -> Option<KwinWindow> {
    snapshot_for_owner(&helper_owner()?)?
        .into_iter()
        .find(|window| window.pid == pid && window.token == token)
}

pub fn correlate_atspi_window(
    atspi: &WindowInfo,
    windows: &[KwinWindow],
) -> Result<KwinWindow, CorrelationError> {
    let pid = atspi.pid.ok_or(CorrelationError::NoMatch)?;
    let matches: Vec<_> = windows
        .iter()
        .filter(|window| window.pid == pid && !window.minimized)
        .filter(|window| geometry_matches(atspi, window))
        .cloned()
        .collect();
    match matches.as_slice() {
        [window] => Ok(window.clone()),
        [] => Err(CorrelationError::NoMatch),
        _ => Err(CorrelationError::Ambiguous),
    }
}

pub fn require_active_target(windows: &[KwinWindow], token: u64) -> Result<(), CorrelationError> {
    windows
        .iter()
        .find(|window| window.active)
        .filter(|window| window.token == token)
        .map(|_| ())
        .ok_or(CorrelationError::WrongActiveTarget)
}

/// Refuse a focus-bound KWin foreground transaction.
///
/// `body` is deliberately never invoked. The current helper can identify an
/// exact KWin window, but its D-Bus API is read-only; even a separate activation
/// step would not bind a later libei event to that target. More pre-dispatch
/// focus checks therefore cannot make the operation target-safe.
pub fn with_focused_window<T>(
    pid: u32,
    token: u64,
    _body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    anyhow::bail!(
        "foreground_unavailable: KWin target pid={pid} token={token} is verified, but only \
         focus-bound portal/libei input is available; refusing raw input until a target-bound \
         KWin input path is implemented"
    )
}

fn geometry_matches(atspi: &WindowInfo, kwin: &KwinWindow) -> bool {
    let right = atspi
        .x
        .saturating_add(atspi.width.min(i32::MAX as u32) as i32);
    let bottom = atspi
        .y
        .saturating_add(atspi.height.min(i32::MAX as u32) as i32);
    let kwin_right = kwin
        .x
        .saturating_add(kwin.width.min(i32::MAX as u32) as i32);
    let kwin_bottom = kwin
        .y
        .saturating_add(kwin.height.min(i32::MAX as u32) as i32);
    (atspi.x - kwin.x).abs() <= GEOMETRY_DELTA_PX
        && (atspi.y - kwin.y).abs() <= GEOMETRY_DELTA_PX
        && (right - kwin_right).abs() <= GEOMETRY_DELTA_PX
        && (bottom - kwin_bottom).abs() <= GEOMETRY_DELTA_PX
}

fn helper_owner() -> Option<String> {
    let owner_raw = dbus_call("GetNameOwner", &[DEST.to_owned()]);
    let owner = parse_quoted_string(&owner_raw?)?;
    if !owner.starts_with(':') {
        return None;
    }

    let pid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixProcessID"),
        &[owner.clone()],
    );
    let uid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixUser"),
        &[owner.clone()],
    );
    let pid = parse_first_u32(&pid_raw?)?;
    let uid = parse_first_u32(&uid_raw?)?;
    if uid != current_uid() || !is_trusted_kwin(pid, &owner) {
        return None;
    }

    let version_raw = call_to(&owner, "GetVersion", &[]);
    let version = parse_first_u32(&version_raw?)?;
    (version == PROTOCOL_VERSION).then_some(owner)
}

fn snapshot_for_owner(owner: &str) -> Option<Vec<KwinWindow>> {
    // Keep this synchronous API safe to call from either sync or async driver
    // code by running the zbus/Tokio client on its own OS thread. In
    // particular, do not use zbus's blocking wrapper directly from a Tokio
    // task: that would create an async-sandwich nested-runtime hazard.
    let owner = owner.to_owned();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;

        runtime.block_on(async move {
            let connection = tokio::time::timeout(CALL_TIMEOUT, zbus::Connection::session())
                .await
                .ok()?
                .ok()?;
            let proxy = tokio::time::timeout(
                CALL_TIMEOUT,
                zbus::Proxy::new(&connection, owner.as_str(), PATH, IFACE),
            )
            .await
            .ok()?
            .ok()?;

            for attempt in 0..3 {
                let raw = tokio::time::timeout(
                    CALL_TIMEOUT,
                    proxy.call::<_, _, String>("GetWindows", &()),
                )
                .await
                .ok()
                .and_then(Result::ok);
                if let Some(snapshot) = raw.as_deref().and_then(parse_snapshot) {
                    return Some(snapshot);
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            None
        })
    })
    .join()
    .ok()
    .flatten()
}

pub fn parse_snapshot(raw: &str) -> Option<Vec<KwinWindow>> {
    // `raw` is the actual QString payload decoded by zbus, not gdbus's
    // human-readable GVariant rendering. Parsing the display representation is
    // unsafe because GLib may change its quoting/escaping for valid titles.
    let values: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;

    let mut tokens = HashSet::new();
    let mut active_count = 0usize;
    values
        .into_iter()
        .map(|value| {
            let token = value
                .get("token")
                .and_then(serde_json::Value::as_u64)
                .filter(|token| *token > 0)?;
            if !tokens.insert(token) {
                return None;
            }

            let active = value.get("active")?.as_bool()?;
            if active {
                active_count += 1;
                if active_count > 1 {
                    return None;
                }
            }

            Some(KwinWindow {
                token,
                pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
                title: value
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                app_name: value
                    .get("app_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                x: json_i32(&value, "x")?,
                y: json_i32(&value, "y")?,
                width: json_u32(&value, "w")?,
                height: json_u32(&value, "h")?,
                active,
                minimized: value.get("minimized")?.as_bool()?,
                stacking: usize::try_from(value.get("stacking")?.as_u64()?).ok()?,
            })
        })
        .collect()
}

fn json_i32(value: &serde_json::Value, key: &str) -> Option<i32> {
    let number = value.get(key)?.as_f64()?;
    (number.is_finite() && number >= i32::MIN as f64 && number <= i32::MAX as f64)
        .then(|| number.round() as i32)
}

fn json_u32(value: &serde_json::Value, key: &str) -> Option<u32> {
    let number = value.get(key)?.as_f64()?;
    (number.is_finite() && number >= 0.0 && number <= u32::MAX as f64)
        .then(|| number.round() as u32)
}

fn dbus_call(method: &str, args: &[String]) -> Option<String> {
    gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.{method}"),
        args,
    )
}

fn call_to(owner: &str, method: &str, args: &[String]) -> Option<String> {
    gdbus_call_to(owner, PATH, &format!("{IFACE}.{method}"), args)
}

fn gdbus_call_to(
    destination: &str,
    object_path: &str,
    method: &str,
    args: &[String],
) -> Option<String> {
    let mut command = Command::new("gdbus");
    command
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg(destination)
        .arg("--object-path")
        .arg(object_path)
        .arg("--method")
        .arg(method)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let child = command.spawn().ok()?;
    wait_timeout(child, CALL_TIMEOUT)
}

fn wait_timeout(mut child: std::process::Child, timeout: Duration) -> Option<String> {
    use std::io::Read;

    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok()?;
        String::from_utf8(bytes).ok()
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok().flatten(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn parse_quoted_string(raw: &str) -> Option<String> {
    let start = raw.find('\'')? + 1;
    let end = raw[start..].find('\'')? + start;
    (end > start).then(|| raw[start..end].to_owned())
}

fn parse_first_u32(raw: &str) -> Option<u32> {
    let payload = raw.split_once("uint32").map_or(raw, |(_, payload)| payload);
    payload
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn current_uid() -> u32 {
    std::fs::metadata(format!("/proc/{}", std::process::id()))
        .map(|meta| meta.uid())
        .unwrap_or(u32::MAX)
}

fn is_trusted_kwin(pid: u32, helper_owner: &str) -> bool {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok();
    if comm.as_deref().map(str::trim) != Some("kwin_wayland") {
        return false;
    }

    let Some(owner_raw) = dbus_call("GetNameOwner", &[KWIN_DEST.to_owned()]) else {
        return false;
    };
    let Some(kwin_owner) = parse_quoted_string(&owner_raw) else {
        return false;
    };
    if kwin_owner != helper_owner {
        return false;
    }

    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    match executable {
        Some(path) => {
            let metadata = std::fs::metadata(&path).ok();
            path.file_name().and_then(|name| name.to_str()) == Some("kwin_wayland")
                && metadata
                    .is_some_and(|meta| meta.uid() == 0 && meta.permissions().mode() & 0o022 == 0)
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_input_capability_is_fail_closed() {
        assert!(!available());

        let called = std::cell::Cell::new(false);
        let error = with_focused_window(42, 7, || {
            called.set(true);
            Ok(())
        })
        .expect_err("focus-bound KWin input must refuse");

        assert!(!called.get());
        assert!(error.to_string().contains("target-bound KWin input path"));
    }
}
