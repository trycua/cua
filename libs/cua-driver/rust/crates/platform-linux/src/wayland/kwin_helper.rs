//! Client for the bundled `org.cua.KWinTarget` KWin effect.
//!
//! Portal/libei input follows compositor focus. This module only authorizes it
//! after a Cua-owned effect, running in `kwin_wayland`, proves the exact opaque
//! window token is active. The helper never exposes KWin's UUID directly: Cua's
//! public `window_id` is a u64, so the effect owns a KWin-lifetime token map.

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
const ACTIVATION_TIMEOUT: Duration = Duration::from_millis(500);
// Frame geometry can differ from AT-SPI client geometry by server decorations.
const GEOMETRY_DELTA_PX: i32 = 64;

#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum TransactionFailure {
    HelperUnavailable,
    HelperOwnerChanged,
    HelperProtocolMismatch,
    DbusTimeout,
    DbusCallFailed,
    SnapshotParseFailed,
    SnapshotInvalid,
    PreviousTargetMissing,
    PreviousTargetStale,
    RestoreActivationFailed,
    RestoreReadbackFailed,
    ForegroundTargetLost,
    TargetMinimized,
    ForegroundTargetChanged,
    LibeiDispatchFailed,
    PortalSessionFailed,
}

impl std::fmt::Display for TransactionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HelperUnavailable => "HelperUnavailable",
            Self::HelperOwnerChanged => "HelperOwnerChanged",
            Self::HelperProtocolMismatch => "HelperProtocolMismatch",
            Self::DbusTimeout => "DbusTimeout",
            Self::DbusCallFailed => "DbusCallFailed",
            Self::SnapshotParseFailed => "SnapshotParseFailed",
            Self::SnapshotInvalid => "SnapshotInvalid",
            Self::PreviousTargetMissing => "PreviousTargetMissing",
            Self::PreviousTargetStale => "PreviousTargetStale",
            Self::RestoreActivationFailed => "RestoreActivationFailed",
            Self::RestoreReadbackFailed => "RestoreReadbackFailed",
            Self::ForegroundTargetLost => "ForegroundTargetLost",
            Self::TargetMinimized => "TargetMinimized",
            Self::ForegroundTargetChanged => "ForegroundTargetChanged",
            Self::LibeiDispatchFailed => "LibeiDispatchFailed",
            Self::PortalSessionFailed => "PortalSessionFailed",
        })
    }
}

impl std::error::Error for TransactionFailure {}

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

pub fn available() -> bool {
    helper_owner().is_some()
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

fn exact_eligible_target(
    windows: &[KwinWindow],
    pid: u32,
    token: u64,
) -> Result<&KwinWindow, TransactionFailure> {
    let target = windows
        .iter()
        .find(|window| window.token == token && window.pid == pid)
        .ok_or(TransactionFailure::ForegroundTargetLost)?;
    (!target.minimized)
        .then_some(target)
        .ok_or(TransactionFailure::TargetMinimized)
}

fn next_transaction_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The final read-back is immediately before `body`, but portal/libei input is
/// still compositor-global. A compositor focus change after that read and
/// before EIS processing is a bounded TOCTOU limitation; callers must not
/// claim atomic target binding. The helper fails closed on every race it can
/// observe before dispatch and restores the previous focus afterward.
pub fn with_focused_window<T>(
    pid: u32,
    token: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let tx = next_transaction_id();
    tracing::debug!(
        tx,
        pid,
        token,
        stage = "resolve_target",
        "KWin foreground transaction"
    );
    let owner =
        helper_owner().ok_or_else(|| anyhow::anyhow!(TransactionFailure::HelperUnavailable))?;
    tracing::debug!(tx, owner, stage = "verify_helper", "KWin helper verified");
    let before = snapshot_for_owner_detailed(&owner)
        .map_err(|error| anyhow::anyhow!("{error}: initial KWin snapshot unavailable"))?;
    let target = exact_eligible_target(&before, pid, token)
        .map_err(|error| anyhow::anyhow!("{error}: exact target pid={pid} token={token}"))?;
    let previous = before
        .iter()
        .find(|window| window.active)
        .map(|window| window.token);
    tracing::debug!(tx, previous = ?previous, active = target.active, stage = "record_previous", "KWin focus recorded");

    if !target.active {
        tracing::debug!(tx, stage = "activate_target", "activating exact KWin token");
        activate_and_verify(&owner, token)?;
    }
    ensure_same_owner(&owner)
        .map_err(|error| anyhow::anyhow!(TransactionFailure::HelperOwnerChanged).context(error))?;
    let current = snapshot_for_owner_detailed(&owner)
        .map_err(|error| anyhow::anyhow!("{error}: target verification snapshot unavailable"))?;
    require_active_target(&current, token)
        .map_err(|_| anyhow::anyhow!(TransactionFailure::ForegroundTargetChanged))?;
    if !current
        .iter()
        .any(|window| window.token == token && window.pid == pid)
    {
        anyhow::bail!(
            "{}: target PID/token changed before dispatch",
            TransactionFailure::ForegroundTargetLost
        );
    }
    #[cfg(debug_assertions)]
    if let Some(steal_token) = std::env::var("CUA_KWIN_TEST_STEAL_TOKEN")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        tracing::debug!(
            tx,
            steal_token,
            stage = "test_focus_steal",
            "debug-only focus-steal hook"
        );
        activate_and_verify(&owner, steal_token)?;
    }

    ensure_same_owner(&owner)
        .map_err(|error| anyhow::anyhow!(TransactionFailure::HelperOwnerChanged).context(error))?;
    let final_current = snapshot_for_owner_detailed(&owner).map_err(|error| {
        anyhow::anyhow!("{error}: final target verification snapshot unavailable")
    })?;
    require_active_target(&final_current, token)
        .map_err(|_| anyhow::anyhow!(TransactionFailure::ForegroundTargetChanged))?;
    tracing::debug!(
        tx,
        active_before_dispatch = token,
        stage = "final_pre_dispatch_verify",
        "target verified immediately before dispatch"
    );

    #[cfg(debug_assertions)]
    if let Some(steal_token) = std::env::var("CUA_KWIN_TEST_POST_VERIFY_STEAL_TOKEN")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        tracing::debug!(
            tx,
            steal_token,
            stage = "test_post_verify_focus_steal",
            "debug-only post-verification focus-steal hook"
        );
        activate_and_verify(&owner, steal_token)?;
    }

    tracing::debug!(tx, stage = "dispatch_begin", "foreground operation begins");
    let body_result = body();
    tracing::debug!(
        tx,
        stage = "dispatch_end",
        success = body_result.is_ok(),
        "foreground operation ended"
    );

    let restore_result = match previous.filter(|previous| *previous != token) {
        Some(previous) => {
            tracing::debug!(
                tx,
                previous,
                stage = "restore_begin",
                "restoring previous KWin focus"
            );
            restore_if_present(&owner, previous, tx)
        }
        None => Ok(()),
    };
    match (body_result, restore_result) {
        (Ok(value), Ok(())) => {
            tracing::debug!(tx, stage = "complete", "foreground transaction complete");
            Ok(value)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(restore)) => Err(anyhow::anyhow!(
            "foreground operation succeeded; focus restoration failed: {restore}"
        )),
        (Err(error), Err(restore)) => {
            Err(error.context(format!("focus restoration also failed: {restore}")))
        }
    }
}

pub fn activate_window(pid: u32, token: u64) -> bool {
    let Some(owner) = helper_owner() else {
        return false;
    };
    let Some(before) = snapshot_for_owner(&owner) else {
        return false;
    };
    if !before
        .iter()
        .any(|window| window.pid == pid && window.token == token && !window.minimized)
    {
        return false;
    }
    activate_and_verify(&owner, token).is_ok()
        && ensure_same_owner(&owner).is_ok()
        && snapshot_for_owner(&owner).is_some_and(|windows| {
            require_active_target(&windows, token).is_ok()
                && windows
                    .iter()
                    .any(|window| window.pid == pid && window.token == token)
        })
}

fn restore_if_present(owner: &str, token: u64, tx: u64) -> anyhow::Result<()> {
    ensure_same_owner(owner)
        .map_err(|error| anyhow::anyhow!(TransactionFailure::HelperOwnerChanged).context(error))?;
    tracing::debug!(
        tx,
        previous = token,
        stage = "restore_snapshot",
        "reading fresh restoration snapshot"
    );
    let windows = snapshot_for_owner_detailed(owner)
        .map_err(|error| anyhow::anyhow!("{error}: restoration snapshot unavailable"))?;
    if !windows
        .iter()
        .any(|window| window.token == token && !window.minimized)
    {
        tracing::debug!(
            tx,
            previous = token,
            stage = "restore_skip",
            "previous target is missing or stale; skipping restoration"
        );
        return Ok(());
    }
    tracing::debug!(
        tx,
        previous = token,
        stage = "restore_activate",
        "restoring previous token"
    );
    activate_and_verify(owner, token).map_err(|error| {
        anyhow::anyhow!("{}: {error}", TransactionFailure::RestoreReadbackFailed)
    })?;
    tracing::debug!(
        tx,
        previous = token,
        stage = "restore_verify",
        "previous focus restored"
    );
    Ok(())
}

fn activate_and_verify(owner: &str, token: u64) -> anyhow::Result<()> {
    let accepted = call_to(owner, "Activate", &[token.to_string()])
        .is_some_and(|output| output.trim_start().starts_with("(true,"));
    if !accepted {
        anyhow::bail!("KWin did not accept activation of exact token {token}");
    }
    let deadline = std::time::Instant::now() + ACTIVATION_TIMEOUT;
    loop {
        ensure_same_owner(owner)?;
        if snapshot_for_owner(owner)
            .as_deref()
            .is_some_and(|windows| require_active_target(windows, token).is_ok())
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("KWin did not verify exact active token {token}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let trusted = uid == current_uid() && is_trusted_kwin(pid, &owner);
    if !trusted {
        return None;
    }
    let version_raw = call_to(&owner, "GetVersion", &[]);
    let version = parse_first_u32(&version_raw?)?;
    (version == PROTOCOL_VERSION).then_some(owner)
}

fn ensure_same_owner(owner: &str) -> anyhow::Result<()> {
    (helper_owner().as_deref() == Some(owner))
        .then_some(())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "foreground_unavailable: KWin helper owner changed during target transaction"
            )
        })
}

fn snapshot_for_owner_detailed(owner: &str) -> Result<Vec<KwinWindow>, TransactionFailure> {
    let mut last_error = TransactionFailure::DbusCallFailed;
    for attempt in 0..3 {
        match gdbus_call_detailed(owner, PATH, &format!("{IFACE}.GetWindows"), &[]) {
            Ok(raw) => match parse_snapshot_detailed(&raw) {
                Ok(snapshot) => {
                    tracing::debug!(
                        owner,
                        count = snapshot.len(),
                        tokens = ?snapshot.iter().map(|window| (window.pid, window.token)).collect::<Vec<_>>(),
                        "KWin snapshot received"
                    );
                    return Ok(snapshot);
                }
                Err(error) => {
                    tracing::debug!(
                        owner,
                        attempt,
                        raw_len = raw.len(),
                        first_bracket = ?raw.find('['),
                        last_bracket = ?raw.rfind(']'),
                        error = %error,
                        "KWin snapshot invalid"
                    );
                    last_error = error;
                }
            },
            Err(error) => {
                tracing::debug!(owner, attempt, error = %error, "KWin snapshot D-Bus call failed");
                last_error = error;
            }
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Err(last_error)
}

fn gdbus_call_detailed(
    destination: &str,
    object_path: &str,
    method: &str,
    args: &[String],
) -> Result<String, TransactionFailure> {
    let mut command = Command::new("gdbus");
    command
        .args([
            "call",
            "--session",
            "--dest",
            destination,
            "--object-path",
            object_path,
            "--method",
            method,
        ])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let child = command
        .spawn()
        .map_err(|_| TransactionFailure::DbusCallFailed)?;
    wait_timeout_detailed(child)
}

fn wait_timeout_detailed(mut child: std::process::Child) -> Result<String, TransactionFailure> {
    use std::io::Read;

    let mut stdout = child
        .stdout
        .take()
        .ok_or(TransactionFailure::DbusCallFailed)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok();
        String::from_utf8(bytes).ok()
    });
    let deadline = std::time::Instant::now() + CALL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return reader
                    .join()
                    .ok()
                    .flatten()
                    .ok_or(TransactionFailure::DbusCallFailed);
            }
            Ok(Some(_)) | Err(_) => return Err(TransactionFailure::DbusCallFailed),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(TransactionFailure::DbusTimeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn snapshot_for_owner(owner: &str) -> Option<Vec<KwinWindow>> {
    for attempt in 0..3 {
        if let Some(snapshot) =
            call_to(owner, "GetWindows", &[]).and_then(|raw| parse_snapshot(&raw))
        {
            return Some(snapshot);
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    None
}

pub fn parse_snapshot(raw: &str) -> Option<Vec<KwinWindow>> {
    parse_snapshot_detailed(raw).ok()
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

fn parse_snapshot_detailed(raw: &str) -> Result<Vec<KwinWindow>, TransactionFailure> {
    let start = raw
        .find('[')
        .ok_or(TransactionFailure::SnapshotParseFailed)?;
    let end = raw
        .rfind(']')
        .ok_or(TransactionFailure::SnapshotParseFailed)?;
    let values: Vec<serde_json::Value> =
        serde_json::from_str(&raw[start..=end]).map_err(|error| {
            tracing::debug!(
                raw_len = raw.len(),
                start,
                end,
                error = %error,
                "KWin snapshot JSON decode failed"
            );
            TransactionFailure::SnapshotParseFailed
        })?;

    let mut tokens = HashSet::new();
    let mut active_count = 0usize;
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let token = value
                .get("token")
                .and_then(serde_json::Value::as_u64)
                .filter(|token| *token > 0)
                .ok_or(TransactionFailure::SnapshotInvalid)?;
            if !tokens.insert(token) {
                tracing::debug!(index, token, "duplicate KWin snapshot token");
                return Err(TransactionFailure::SnapshotInvalid);
            }
            if value.get("active").and_then(serde_json::Value::as_bool) == Some(true) {
                active_count += 1;
                if active_count > 1 {
                    tracing::debug!(index, token, "multiple active KWin snapshot records");
                    return Err(TransactionFailure::SnapshotInvalid);
                }
            }
            let parsed: Option<KwinWindow> = (|| {
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
                    active: value.get("active")?.as_bool()?,
                    minimized: value.get("minimized")?.as_bool()?,
                    stacking: usize::try_from(value.get("stacking")?.as_u64()?).ok()?,
                })
            })();
            parsed.ok_or_else(|| {
                let keys = value
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                tracing::debug!(
                    index,
                    ?keys,
                    pid = ?value.get("pid"),
                    token = ?value.get("token"),
                    x = ?value.get("x"),
                    y = ?value.get("y"),
                    w = ?value.get("w"),
                    h = ?value.get("h"),
                    active = ?value.get("active"),
                    minimized = ?value.get("minimized"),
                    stacking = ?value.get("stacking"),
                    "KWin snapshot record is invalid"
                );
                TransactionFailure::SnapshotInvalid
            })
        })
        .collect()
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
        .arg(method);
    command.args(args);
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
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
    // Yama/proc restrictions commonly deny reading another user's /proc/<pid>/exe
    // even when both processes share the same uid. Pinning the helper's unique
    // owner to KWin's own well-known D-Bus owner is the stronger identity check.
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
        // The D-Bus owner equality + exact comm is the fallback when procfs
        // hides exe metadata; an arbitrary process cannot also own org.kde.KWin.
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_minimized_target_has_its_own_failure_category() {
        let windows = vec![KwinWindow {
            token: 7,
            pid: 42,
            title: String::new(),
            app_name: String::new(),
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            active: false,
            minimized: true,
            stacking: 0,
        }];
        assert_eq!(
            exact_eligible_target(&windows, 42, 7),
            Err(TransactionFailure::TargetMinimized)
        );
    }
}
