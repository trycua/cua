// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Trusted KWin window identity adapter.
//!
//! The optional in-process KWin effect exposes stable window tokens, process
//! identity, geometry, active/minimized state, and stacking metadata over
//! D-Bus. It exposes no activation or input injection methods.
//!
//! KWin's portal/libei input path is focus-bound: libei delivers to whichever
//! surface is focused when KWin processes the event, so a focus check followed
//! by input has a TOCTOU race. Protocol 2 closes it with an input gate: an
//! input filter inside KWin that lets this driver's own libei events reach one
//! exact window and holds back any that would reach another, decided per
//! event on KWin's main thread (see [`super::kwin_gate`]). [`available`]
//! reports whether that gate is present. Protocol 1 helpers still serve window
//! discovery.

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
/// The newest helper protocol this driver speaks.
pub const PROTOCOL_VERSION: u32 = 2;
/// The first protocol with the input gate.
pub const GATE_PROTOCOL_VERSION: u32 = 2;
const CALL_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Whether KWin offers the input gate that binds this driver's libei input to
/// one exact window. A protocol 1 helper serves discovery only.
pub fn available() -> bool {
    gate_owner().is_some()
}

/// The trusted helper's bus name when it has the input gate.
pub(crate) fn gate_owner() -> Option<String> {
    helper_owner_from(GATE_PROTOCOL_VERSION)
}

/// The index of the keyboard layout KWin has active, or `None` when this isn't
/// KWin or it doesn't answer. KWin before 6.8 doesn't tell libei clients.
pub fn active_layout() -> Option<u32> {
    let reply = gdbus_call_to(
        "org.kde.keyboard",
        "/Layouts",
        "org.kde.KeyboardLayouts.getLayout",
        &[],
    )?;
    parse_first_u32(&reply)
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

fn helper_owner() -> Option<String> {
    helper_owner_from(1)
}

/// The helper's bus name, when KWin itself owns it and it speaks a protocol
/// from `oldest` up to [`PROTOCOL_VERSION`].
fn helper_owner_from(oldest: u32) -> Option<String> {
    let (owner, version) = loaded()?;
    (oldest..=PROTOCOL_VERSION)
        .contains(&version)
        .then_some(owner)
}

/// The bus name and protocol of the helper KWin itself has loaded.
fn loaded() -> Option<(String, u32)> {
    let owner_raw = dbus_call("GetNameOwner", &[DEST.to_owned()]);
    let owner = parse_quoted_string(&owner_raw?)?;
    if !owner.starts_with(':') {
        return None;
    }

    let pid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixProcessID"),
        std::slice::from_ref(&owner),
    );
    let uid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixUser"),
        std::slice::from_ref(&owner),
    );
    let pid = parse_first_u32(&pid_raw?)?;
    let uid = parse_first_u32(&uid_raw?)?;
    if uid != current_uid() || !is_trusted_kwin(pid, &owner) {
        return None;
    }

    let version_raw = call_to(&owner, "GetVersion", &[]);
    let version = parse_first_u32(&version_raw?)?;
    Some((owner, version))
}

/// KWin's name for the helper, as an effect and in `kwinrc`.
const EFFECT: &str = "cua_kwin_target_helper";
const EFFECTS_PATH: &str = "/Effects";
const EFFECTS_IFACE: &str = "org.kde.kwin.Effects";
/// How long KWin gets to bring a loaded helper onto the bus.
const START_TIMEOUT: Duration = Duration::from_secs(3);

/// Load the helper into the running KWin and have KWin start it with every
/// session. Does nothing when KWin has no such effect installed.
/// [`status_line`] tells what came of it.
pub(crate) fn turn_on() {
    if installed() != Some(true) {
        return;
    }
    write_autostart(true);
    if loaded().is_none() {
        effects_call("loadEffect");
        let deadline = std::time::Instant::now() + START_TIMEOUT;
        while loaded().is_none() && std::time::Instant::now() < deadline {
            crate::interaction::pause(Duration::from_millis(100));
        }
    }
}

/// Unload the helper and stop KWin from starting it.
pub(crate) fn turn_off() {
    if autostarts() {
        write_autostart(false);
    }
    if loaded().is_some() {
        effects_call("unloadEffect");
    }
}

/// The helper's line in `cua-driver kwin-capture status`, or `None` when KWin
/// doesn't answer.
pub(crate) fn status_line() -> Option<String> {
    let installed = installed()?;
    Some(describe(
        installed,
        loaded().map(|(_, version)| version),
        autostarts(),
    ))
}

fn describe(installed: bool, protocol: Option<u32>, autostarts: bool) -> String {
    let state = match (protocol, installed) {
        (Some(version), _) if version < GATE_PROTOCOL_VERSION => format!(
            "loaded with protocol {version}; foreground input needs protocol \
             {GATE_PROTOCOL_VERSION}: build and install the current helper"
        ),
        (Some(version), _) if autostarts => {
            format!("loaded (protocol {version}), starts with KWin")
        }
        (Some(version), _) => format!(
            "loaded (protocol {version}) until KWin restarts; run `cua-driver kwin-capture \
             on` to start it with KWin"
        ),
        (None, true) => "installed but not loaded; run `cua-driver kwin-capture on`. If it \
                         stays unloaded, it was built for another KWin version: build and \
                         install it again"
            .to_owned(),
        (None, false) => "not installed; foreground input to KWin windows needs it (see \
                          libs/cua-driver/kwin-target-helper)"
            .to_owned(),
    };
    format!("kwin helper: {state}")
}

/// Whether KWin has the helper among the effects it can load, or `None` when
/// KWin doesn't answer.
fn installed() -> Option<bool> {
    let effects = gdbus_call_to(
        KWIN_DEST,
        EFFECTS_PATH,
        "org.freedesktop.DBus.Properties.Get",
        &[EFFECTS_IFACE.to_owned(), "listOfEffects".to_owned()],
    )?;
    Some(effects.contains(&format!("'{EFFECT}'")))
}

fn effects_call(method: &str) {
    gdbus_call_to(
        KWIN_DEST,
        EFFECTS_PATH,
        &format!("{EFFECTS_IFACE}.{method}"),
        &[EFFECT.to_owned()],
    );
}

/// Set or clear the `kwinrc` entry that makes KWin load the helper when it
/// starts. `kwriteconfig6` is KDE's own writer, so KWin's file stays intact.
fn write_autostart(on: bool) {
    let mut command = Command::new("kwriteconfig6");
    command.args(["--file", "kwinrc", "--group", "Plugins", "--key"]);
    command.arg(format!("{EFFECT}Enabled"));
    if on {
        command.args(["--type", "bool", "true"]);
    } else {
        command.arg("--delete");
    }
    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether `kwinrc` tells KWin to load the helper when it starts.
fn autostarts() -> bool {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        });
    config
        .and_then(|dir| std::fs::read_to_string(dir.join("kwinrc")).ok())
        .is_some_and(|text| autostart_set(&text))
}

fn autostart_set(kwinrc: &str) -> bool {
    let key = format!("{EFFECT}Enabled");
    let mut in_plugins = false;
    kwinrc.lines().map(str::trim).any(|line| {
        if line.starts_with('[') {
            in_plugins = line == "[Plugins]";
            return false;
        }
        in_plugins
            && line
                .split_once('=')
                .is_some_and(|(name, value)| name.trim() == key && value.trim() == "true")
    })
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

/// Where input would go right now, relative to one window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct Focus {
    /// KWin still has the window.
    pub known: bool,
    /// The keyboard focus is the window or one of its transients.
    pub keyboard: bool,
    /// The pointer focus is the window or one of its transients.
    pub pointer: bool,
}

/// [`Focus`] for the window with KWin internal id `uuid`.
pub(crate) fn focus(owner: &str, uuid: &str) -> anyhow::Result<Focus> {
    let raw = call_once(owner, "GetFocus", (uuid.to_owned(),))?;
    serde_json::from_str(&raw).map_err(|error| anyhow::anyhow!("KWin helper focus reply: {error}"))
}

/// The KWin internal id of the active window, if any.
pub(crate) fn active_uuid(owner: &str) -> anyhow::Result<Option<String>> {
    let raw = call_once(owner, "GetWindows", ())?;
    Ok(parse_active_uuid(&raw))
}

fn parse_active_uuid(raw: &str) -> Option<String> {
    let windows: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;
    windows
        .iter()
        .find(|window| window.get("active").and_then(serde_json::Value::as_bool) == Some(true))?
        .get("uuid")?
        .as_str()
        .map(str::to_owned)
}

/// One helper call returning its string reply, on its own connection.
fn call_once<B>(owner: &str, method: &'static str, body: B) -> anyhow::Result<String>
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let owner = owner.to_owned();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let connection = zbus::Connection::session().await?;
                let proxy = zbus::Proxy::new(&connection, owner.as_str(), PATH, IFACE).await?;
                anyhow::Ok(proxy.call::<_, _, String>(method, &body).await?)
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("KWin helper call thread panicked"))?
    .map_err(|error| anyhow::anyhow!("KWin helper {method}: {error:#}"))
}

/// How many of the driver's events an armed gate let through and held back.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct GateCounts {
    pub passed: u64,
    pub dropped: u64,
}

/// An armed KWin input gate. KWin keeps it armed until [`InputGate::disarm`]
/// or until this connection closes, so a driver that dies mid-action leaves
/// nothing armed.
pub(crate) struct InputGate {
    done: Option<tokio::sync::oneshot::Sender<()>>,
    report: std::sync::mpsc::Receiver<anyhow::Result<GateCounts>>,
}

impl InputGate {
    /// Hold back the libei events of `client` that would reach any window but
    /// the one with KWin internal id `uuid`.
    pub(crate) fn arm(owner: &str, uuid: &str, client: &str) -> anyhow::Result<Self> {
        let (armed_tx, armed_rx) = std::sync::mpsc::channel();
        let (report_tx, report) = std::sync::mpsc::channel();
        let (done, done_rx) = tokio::sync::oneshot::channel::<()>();
        let (owner, uuid, client) = (owner.to_owned(), uuid.to_owned(), client.to_owned());
        std::thread::Builder::new()
            .name("cua-kwin-gate".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = armed_tx.send(Err(error.into()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let armed = async {
                        let connection = zbus::Connection::session().await?;
                        let proxy =
                            zbus::Proxy::new(&connection, owner.as_str(), PATH, IFACE).await?;
                        let raw: String = proxy.call("ArmInput", &(uuid, client)).await?;
                        let id = parse_arm(&raw)?;
                        anyhow::Ok((connection, proxy, id))
                    };
                    let (connection, proxy, id) =
                        match tokio::time::timeout(CALL_TIMEOUT, armed).await {
                            Ok(Ok(armed)) => armed,
                            Ok(Err(error)) => {
                                let _ = armed_tx.send(Err(error));
                                return;
                            }
                            Err(_) => {
                                let _ = armed_tx.send(Err(anyhow::anyhow!(
                                    "the KWin helper did not answer ArmInput"
                                )));
                                return;
                            }
                        };
                    if armed_tx.send(Ok(())).is_err() {
                        // The caller gave up waiting: closing the connection
                        // disarms the gate.
                        return;
                    }
                    let _ = done_rx.await;
                    let disarmed = async {
                        let raw: String = proxy.call("DisarmInput", &(id,)).await?;
                        parse_counts(&raw)
                    };
                    let counts = tokio::time::timeout(CALL_TIMEOUT, disarmed)
                        .await
                        .unwrap_or_else(|_| {
                            Err(anyhow::anyhow!(
                                "the KWin helper did not answer DisarmInput"
                            ))
                        });
                    let _ = report_tx.send(counts);
                    drop(connection);
                });
            })?;
        armed_rx
            .recv_timeout(CALL_TIMEOUT * 2)
            .map_err(|_| anyhow::anyhow!("the KWin helper did not answer ArmInput"))??;
        Ok(Self {
            done: Some(done),
            report,
        })
    }

    /// Stop gating and report what the gate did.
    pub(crate) fn disarm(mut self) -> anyhow::Result<GateCounts> {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
        self.report
            .recv_timeout(CALL_TIMEOUT * 2)
            .map_err(|_| anyhow::anyhow!("the KWin helper did not answer DisarmInput"))?
    }
}

impl Drop for InputGate {
    fn drop(&mut self) {
        // Unwinding past an armed gate still disarms it.
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
    }
}

fn parse_arm(raw: &str) -> anyhow::Result<u64> {
    let reply: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
        anyhow::bail!("the KWin helper refused the input gate: {error}");
    }
    reply
        .get("arm")
        .and_then(serde_json::Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow::anyhow!("the KWin helper sent no input gate id"))
}

fn parse_counts(raw: &str) -> anyhow::Result<GateCounts> {
    let reply: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(error) = reply.get("error").and_then(serde_json::Value::as_str) {
        anyhow::bail!("the KWin input gate did not report: {error}");
    }
    Ok(serde_json::from_value(reply)?)
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

/// KWin's own support report, which names its version. No permission needed.
pub(crate) fn support_information() -> Option<String> {
    gdbus_call_to(KWIN_DEST, "/KWin", "org.kde.KWin.supportInformation", &[])
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
    let deadline = crate::interaction::operation_deadline(timeout);
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
            Ok(None) => crate::interaction::pause(Duration::from_millis(10)),
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

pub(super) fn current_uid() -> u32 {
    std::fs::metadata(format!("/proc/{}", std::process::id()))
        .map(|meta| meta.uid())
        .unwrap_or(u32::MAX)
}

fn is_trusted_kwin(pid: u32, helper_owner: &str) -> bool {
    let Some(owner_raw) = dbus_call("GetNameOwner", &[KWIN_DEST.to_owned()]) else {
        return false;
    };
    parse_quoted_string(&owner_raw).as_deref() == Some(helper_owner) && is_trusted_kwin_process(pid)
}

pub(super) fn is_trusted_kwin_process(pid: u32) -> bool {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok();
    if !matches!(
        comm.as_deref().map(str::trim),
        Some("kwin_wayland" | ".kwin_wayland-w")
    ) {
        return false;
    }

    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    match executable {
        Some(path) => {
            let metadata = std::fs::metadata(&path).ok();
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("kwin_wayland" | ".kwin_wayland-wrapped")
            ) && metadata
                .is_some_and(|meta| meta.uid() == 0 && meta.permissions().mode() & 0o022 == 0)
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kwinrc_starts_the_helper_only_with_its_own_key_set_true_under_plugins() {
        assert!(autostart_set(
            "[Plugins]\ncua_kwin_target_helperEnabled=true\n"
        ));
        assert!(autostart_set("[Desktops]\nRows=1\n\n[Plugins]\nblurEnabled=false\ncua_kwin_target_helperEnabled = true\n"));
        assert!(!autostart_set(
            "[Plugins]\ncua_kwin_target_helperEnabled=false\n"
        ));
        assert!(!autostart_set("[Plugins]\nblurEnabled=true\n"));
        // The same key under another group is not KWin's plugin switch.
        assert!(!autostart_set(
            "[Plugins]\n[Other]\ncua_kwin_target_helperEnabled=true\n"
        ));
        assert!(!autostart_set(""));
    }

    #[test]
    fn the_status_line_says_what_is_missing_and_how_to_get_it() {
        assert_eq!(
            describe(true, Some(2), true),
            "kwin helper: loaded (protocol 2), starts with KWin"
        );
        // Loaded by hand: gone after the next login unless turned on.
        assert!(describe(true, Some(2), false).contains("until KWin restarts"));
        assert!(describe(true, Some(1), true).contains("needs protocol 2"));
        let stopped = describe(true, None, true);
        assert!(stopped.contains("installed but not loaded"));
        assert!(stopped.contains("`cua-driver kwin-capture on`"));
        assert!(describe(false, None, false).contains("not installed"));
    }

    #[test]
    fn the_active_window_is_read_from_the_snapshot() {
        let raw = r#"[{"uuid":"aa","active":false},{"uuid":"bb","active":true}]"#;
        assert_eq!(parse_active_uuid(raw).as_deref(), Some("bb"));
        assert_eq!(parse_active_uuid(r#"[{"uuid":"aa","active":false}]"#), None);
        // A protocol 1 helper sends no uuid.
        assert_eq!(parse_active_uuid(r#"[{"token":1,"active":true}]"#), None);
    }

    #[test]
    fn an_arm_reply_is_an_id_or_the_helper_s_refusal() {
        assert_eq!(parse_arm(r#"{"arm":3}"#).unwrap(), 3);
        let refused = parse_arm(r#"{"error":"another input gate is armed"}"#).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the KWin helper refused the input gate: another input gate is armed"
        );
        assert!(parse_arm(r#"{"arm":0}"#).is_err());
        assert!(parse_arm("{}").is_err());
    }

    #[test]
    fn gate_counts_are_read_or_the_missing_gate_is_reported() {
        assert_eq!(
            parse_counts(r#"{"passed":4,"dropped":1}"#).unwrap(),
            GateCounts {
                passed: 4,
                dropped: 1
            }
        );
        let missing = parse_counts(r#"{"error":"this input gate is not armed"}"#).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "the KWin input gate did not report: this input gate is not armed"
        );
    }
}
