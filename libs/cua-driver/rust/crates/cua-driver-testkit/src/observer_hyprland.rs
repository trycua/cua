//! Independent Hyprland observations, adapted from contributions in #3052/#3466.
//! This module never calls driver capture, geometry, or input implementations.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Deserialize};

use super::{DesktopJournal, DesktopSnapshot, FocusEvent, ObserverError, TargetWindow, TargetZ};

const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 1024 * 1024;

struct ReapedChild(Child);

impl Drop for ReapedChild {
    fn drop(&mut self) {
        // Always reap, including malformed output, timeout and read-error paths.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<(), ObserverError> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(ObserverError::new(format!(
            "hyprctl pipe setup: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn read_available(
    pipe: &mut impl Read,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, ObserverError> {
    let mut buffer = [0; 8192];
    // Limit work per iteration as well as retained output, so a writer cannot
    // starve deadline checks. A child inheriting the pipe cannot block EOF.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if bytes.len().saturating_add(count) > limit {
                    return Err(ObserverError::new("hyprctl output exceeded limit"));
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(ObserverError::new(format!("hyprctl pipe read: {error}"))),
        }
    }
    Ok(false)
}

fn bounded_output(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>, ObserverError> {
    let mut child = ReapedChild(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ObserverError::new(format!("hyprctl spawn: {error}")))?,
    );
    let mut stdout = child.0.stdout.take().expect("piped stdout");
    let mut stderr = child.0.stderr.take().expect("piped stderr");
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut err = Vec::new();
    loop {
        let out_done = read_available(&mut stdout, &mut out, limit)?;
        let err_done = read_available(&mut stderr, &mut err, limit)?;
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|error| ObserverError::new(format!("hyprctl wait: {error}")))?
        {
            if out_done && err_done {
                return if status.success() {
                    Ok(out)
                } else {
                    Err(ObserverError::new(format!(
                        "hyprctl exited with {status}: {}",
                        String::from_utf8_lossy(&err)
                    )))
                };
            }
        }
        if Instant::now() >= deadline {
            return Err(ObserverError::new("hyprctl query timed out"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn query<T: DeserializeOwned>(name: &str) -> Result<T, ObserverError> {
    let output = bounded_output(
        Command::new("hyprctl").args(["-j", name]),
        QUERY_TIMEOUT,
        OUTPUT_LIMIT,
    )
    .map_err(|error| ObserverError::new(format!("hyprctl {name}: {error}")))?;
    serde_json::from_slice(&output)
        .map_err(|error| ObserverError::new(format!("invalid hyprctl {name} JSON: {error}")))
}

#[derive(Clone, Debug, Deserialize)]
struct Workspace {
    id: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct Client {
    address: String,
    pid: u32,
    workspace: Workspace,
    monitor: i64,
    at: [i64; 2],
    size: [i64; 2],
    mapped: bool,
    hidden: bool,
    fullscreen: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct Monitor {
    id: i64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    transform: u8,
    #[serde(rename = "activeWorkspace")]
    active_workspace: Workspace,
    #[serde(rename = "specialWorkspace")]
    special_workspace: Workspace,
}

fn address(value: &str) -> Result<u64, ObserverError> {
    value
        .strip_prefix("0x")
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| ObserverError::new("invalid Hyprland client address"))
}

fn clients() -> Result<Vec<Client>, ObserverError> {
    let clients: Vec<Client> = query("clients")?;
    for client in &clients {
        address(&client.address)?;
    }
    Ok(clients)
}

fn select(clients: &[Client], target: TargetWindow) -> Result<Option<&Client>, ObserverError> {
    if target.native_id != 0 {
        let client = clients
            .iter()
            .find(|client| address(&client.address).ok() == Some(target.native_id));
        if client.is_some_and(|client| client.pid != target.pid) {
            return Err(ObserverError::new(
                "Hyprland target address belongs to a different pid",
            ));
        }
        return Ok(client);
    }
    let mut matches = clients
        .iter()
        .filter(|client| client.pid == target.pid && client.mapped);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(ObserverError::new(
            "Hyprland target pid has multiple mapped clients; native_id required",
        ));
    }
    Ok(first)
}

pub(super) fn client_address(target: TargetWindow) -> Result<String, ObserverError> {
    select(&clients()?, target)?
        .map(|client| client.address.clone())
        .ok_or_else(|| ObserverError::new("Hyprland target client not found"))
}

fn parse_focus(value: serde_json::Value) -> Result<Option<u64>, ObserverError> {
    if value.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(None);
    }
    let raw = value
        .get("address")
        .and_then(|address| address.as_str())
        .ok_or_else(|| ObserverError::new("Hyprland activewindow omitted address"))?;
    address(raw).map(Some)
}

pub(super) fn focus_identity() -> Result<Option<u64>, ObserverError> {
    parse_focus(query("activewindow")?)
}

#[derive(Deserialize)]
struct Cursor {
    x: f64,
    y: f64,
}

pub(super) fn cursor_position() -> Result<(f64, f64), ObserverError> {
    let cursor: Cursor = query("cursorpos")?;
    if !cursor.x.is_finite() || !cursor.y.is_finite() {
        return Err(ObserverError::new(
            "Hyprland cursor coordinates are not finite",
        ));
    }
    Ok((cursor.x, cursor.y))
}

fn monitor_region(monitor: &Monitor) -> Result<(f64, f64, f64, f64), ObserverError> {
    if ![
        monitor.x,
        monitor.y,
        monitor.width,
        monitor.height,
        monitor.scale,
    ]
    .iter()
    .all(|v| v.is_finite())
        || monitor.width <= 0.0
        || monitor.height <= 0.0
        || monitor.scale <= 0.0
        || monitor.transform > 7
    {
        return Err(ObserverError::new("invalid Hyprland monitor geometry"));
    }
    let (width, height) = if monitor.transform % 2 == 1 {
        (monitor.height, monitor.width)
    } else {
        (monitor.width, monitor.height)
    };
    let (width, height) = (width / monitor.scale, height / monitor.scale);
    if !width.is_finite() || !height.is_finite() {
        return Err(ObserverError::new(
            "invalid Hyprland logical monitor extent",
        ));
    }
    Ok((monitor.x, monitor.y, width, height))
}

pub(super) fn monitor_regions() -> Result<Vec<(f64, f64, f64, f64)>, ObserverError> {
    let monitors: Vec<Monitor> = query("monitors")?;
    if monitors.is_empty() {
        return Err(ObserverError::new("Hyprland has no active monitors"));
    }
    monitors.iter().map(monitor_region).collect()
}

#[derive(Deserialize)]
struct Instance {
    instance: String,
    wl_socket: String,
}

fn instance_matches(
    instances: &[Instance],
    signature: &str,
    display: &str,
    runtime: Option<&str>,
) -> bool {
    instances.iter().any(|instance| {
        instance.instance == signature
            && (instance.wl_socket == display
                || runtime.is_some_and(|runtime| {
                    std::path::Path::new(runtime).join(&instance.wl_socket)
                        == std::path::Path::new(display)
                }))
    })
}

pub(super) fn probe() -> Result<(), ObserverError> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| ObserverError::new("Hyprland observer requires an instance signature"))?;
    let display = std::env::var("WAYLAND_DISPLAY")
        .map_err(|_| ObserverError::new("Hyprland observer requires WAYLAND_DISPLAY"))?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    let instances = query::<Vec<Instance>>("instances")?;
    if !instance_matches(&instances, &signature, &display, runtime.as_deref()) {
        return Err(ObserverError::new(
            "Hyprland instance signature does not match WAYLAND_DISPLAY",
        ));
    }
    clients()?;
    focus_identity()?;
    monitor_regions()?;
    cursor_position()?;
    Ok(())
}

fn overlaps(a: &Client, b: &Client) -> bool {
    (0..2).all(|i| {
        a.at[i] < b.at[i].saturating_add(b.size[i]) && b.at[i] < a.at[i].saturating_add(a.size[i])
    })
}

fn contains(a: &Client, b: &Client) -> bool {
    (0..2).all(|i| {
        a.at[i] <= b.at[i] && a.at[i].saturating_add(a.size[i]) >= b.at[i].saturating_add(b.size[i])
    })
}

fn classify(
    clients: &[Client],
    active: Option<u64>,
    monitors: &[Monitor],
    target: TargetWindow,
) -> Result<TargetZ, ObserverError> {
    let Some(target) = select(clients, target)? else {
        return Ok(TargetZ::NotFound);
    };
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.id == target.monitor)
        .ok_or_else(|| ObserverError::new("Hyprland target monitor missing"))?;
    if !target.mapped
        || target.hidden
        || (target.workspace.id != monitor.active_workspace.id
            && target.workspace.id != monitor.special_workspace.id)
    {
        return Ok(TargetZ::Minimized);
    }
    if address(&target.address)? == active.unwrap_or(0) {
        return Ok(TargetZ::Foreground);
    }
    if target.size.iter().any(|size| *size <= 0) {
        return Err(ObserverError::new("Hyprland target has invalid extent"));
    }
    // An open special workspace overlays the regular workspace. Its coverage
    // and stacking cannot be inferred from the ordinary clients list.
    if monitor.special_workspace.id != 0 && target.workspace.id != monitor.special_workspace.id {
        return Err(ObserverError::new(
            "Hyprland special-workspace occlusion is ambiguous",
        ));
    }
    let others: Vec<_> = clients
        .iter()
        .filter(|client| {
            client.address != target.address
                && client.mapped
                && !client.hidden
                && client.workspace.id == target.workspace.id
                && overlaps(client, target)
        })
        .collect();
    if others.iter().any(|client| {
        address(&client.address).ok() == active
            && client.monitor == target.monitor
            && client.fullscreen == 2
            && contains(client, target)
    }) {
        return Ok(TargetZ::BackgroundOccluded);
    }
    if !others.is_empty() {
        return Err(ObserverError::new(
            "Hyprland overlapping clients have ambiguous z-order",
        ));
    }
    Ok(TargetZ::BackgroundVisible)
}

pub(super) fn snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
    let clients = clients()?;
    let active = focus_identity()?;
    let monitors = query::<Vec<Monitor>>("monitors")?;
    for monitor in &monitors {
        monitor_region(monitor)?;
    }
    let target_z = classify(&clients, active, &monitors, target)?;
    Ok(DesktopSnapshot {
        foreground: active,
        input_focus: active,
        target_z,
        cursor_pos: Some(cursor_position()?),
    })
}

pub(super) struct Journal {
    stop: Arc<AtomicBool>,
    sampler: Option<JoinHandle<Result<DesktopJournal, ObserverError>>>,
}

impl Journal {
    pub(super) fn start() -> Result<Self, ObserverError> {
        Self::with_query(focus_identity)
    }

    fn with_query(
        query: impl Fn() -> Result<Option<u64>, ObserverError> + Send + 'static,
    ) -> Result<Self, ObserverError> {
        // Establish baseline before returning to the action under observation.
        let mut previous = query()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let sampler = std::thread::spawn(move || {
            let mut journal = DesktopJournal::default();
            while !thread_stop.load(Ordering::Acquire) {
                let current = query()?;
                if current != previous {
                    if journal.focus_events.len() >= 10_000 {
                        return Err(ObserverError::new(
                            "Hyprland focus journal exceeded event limit",
                        ));
                    }
                    journal.focus_events.push(FocusEvent {
                        from: previous,
                        to: current,
                    });
                    previous = current;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(journal)
        });
        Ok(Self {
            stop,
            sampler: Some(sampler),
        })
    }

    pub(super) fn drain(mut self) -> Result<DesktopJournal, ObserverError> {
        self.stop.store(true, Ordering::Release);
        // Each iteration contains one bounded query and a 10ms sleep. Thus
        // shutdown is bounded by QUERY_TIMEOUT plus scheduling and child reap.
        self.sampler
            .take()
            .expect("active journal")
            .join()
            .map_err(|_| ObserverError::new("Hyprland focus journal panicked"))?
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(sampler) = self.sampler.take() {
            let _ = sampler.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    fn client(id: u64, pid: u32) -> Client {
        serde_json::from_value(json!({"address": format!("0x{id:x}"), "pid": pid,
            "workspace": {"id": 1}, "monitor": 0, "at": [10,20], "size": [100,80],
            "mapped": true, "hidden": false, "fullscreen": 0}))
        .unwrap()
    }

    fn monitor() -> Monitor {
        serde_json::from_value(json!({"id": 0, "x": -100, "y": 0, "width": 1920,
            "height": 1080, "scale": 1.5, "transform": 1,
            "activeWorkspace": {"id": 1}, "specialWorkspace": {"id": 0}}))
        .unwrap()
    }

    fn target() -> TargetWindow {
        TargetWindow {
            pid: 100,
            native_id: 0x10,
        }
    }

    #[test]
    fn explicit_identity_never_selects_larger_sibling_or_missing_id_fallback() {
        let small = client(0x10, 100);
        let mut large = client(0x11, 100);
        large.size = [1000, 1000];
        let clients = [large, small];
        assert_eq!(select(&clients, target()).unwrap().unwrap().address, "0x10");
        assert!(select(
            &clients,
            TargetWindow {
                native_id: 0x12,
                ..target()
            }
        )
        .unwrap()
        .is_none());
        assert!(select(
            &clients,
            TargetWindow {
                native_id: 0,
                ..target()
            }
        )
        .is_err());
        assert!(select(
            &clients,
            TargetWindow {
                pid: 200,
                ..target()
            }
        )
        .is_err());
    }

    #[test]
    fn pid_only_identity_requires_unique_mapped_client() {
        let clients = [client(0x10, 100)];
        assert!(select(
            &clients,
            TargetWindow {
                native_id: 0,
                ..target()
            }
        )
        .unwrap()
        .is_some());
        let mut unmapped = client(0x11, 100);
        unmapped.mapped = false;
        assert!(select(
            &[clients[0].clone(), unmapped],
            TargetWindow {
                native_id: 0,
                ..target()
            }
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn fullscreen_requires_actual_cover_and_maximized_is_ambiguous() {
        let mut cover = client(0x20, 200);
        cover.fullscreen = 2;
        cover.at = [0, 0];
        cover.size = [1920, 1080];
        let mut clients = [client(0x10, 100), cover];
        assert_eq!(
            classify(&clients, Some(0x20), &[monitor()], target()).unwrap(),
            TargetZ::BackgroundOccluded
        );
        clients[1].fullscreen = 1;
        assert!(classify(&clients, Some(0x20), &[monitor()], target()).is_err());
        clients[1].fullscreen = 2;
        clients[1].size = [30, 30];
        assert!(classify(&clients, Some(0x20), &[monitor()], target()).is_err());
        clients[1].at = [1000, 1000];
        assert_eq!(
            classify(&clients, Some(0x20), &[monitor()], target()).unwrap(),
            TargetZ::BackgroundVisible
        );
    }

    #[test]
    fn hidden_and_off_workspace_precede_foreground_and_special_overlay_is_unknown() {
        let mut clients = [client(0x10, 100)];
        clients[0].workspace.id = 9;
        assert_eq!(
            classify(&clients, Some(0x10), &[monitor()], target()).unwrap(),
            TargetZ::Minimized
        );
        clients[0].workspace.id = 1;
        clients[0].hidden = true;
        assert_eq!(
            classify(&clients, Some(0x10), &[monitor()], target()).unwrap(),
            TargetZ::Minimized
        );
        clients[0].hidden = false;
        let mut monitor = monitor();
        monitor.special_workspace.id = -99;
        assert!(classify(&clients, None, &[monitor], target()).is_err());
    }

    #[test]
    fn monitor_regions_preserve_negative_origins_rotation_and_scale() {
        let mut monitor = monitor();
        assert_eq!(
            monitor_region(&monitor).unwrap(),
            (-100.0, 0.0, 720.0, 1280.0)
        );
        monitor.scale = f64::NAN;
        assert!(monitor_region(&monitor).is_err());
        monitor.scale = 0.0;
        assert!(monitor_region(&monitor).is_err());
        monitor.scale = 1.0;
        monitor.transform = 8;
        assert!(monitor_region(&monitor).is_err());
    }

    #[test]
    fn malformed_focus_is_not_silently_treated_as_unfocused() {
        assert_eq!(parse_focus(json!({})).unwrap(), None);
        assert_eq!(
            parse_focus(json!({"address": "0x123"})).unwrap(),
            Some(0x123)
        );
        for value in [
            json!(null),
            json!([]),
            json!({"address": "invalid"}),
            json!({"pid": 10}),
            json!({"address": "0x0"}),
        ] {
            assert!(parse_focus(value).is_err());
        }
        assert!(serde_json::from_value::<Client>(json!({"address": "0x10"})).is_err());
        assert!(serde_json::from_slice::<Cursor>(br#"{"x":2}"#).is_err());
        let cursor: Cursor = serde_json::from_slice(br#"{"x":4367,"y":-964}"#).unwrap();
        assert_eq!((cursor.x, cursor.y), (4367.0, -964.0));
    }

    #[test]
    fn stale_instance_signature_cannot_probe_a_different_display() {
        let instances = [Instance {
            instance: "session-a".into(),
            wl_socket: "wayland-1".into(),
        }];
        assert!(instance_matches(&instances, "session-a", "wayland-1", None));
        assert!(instance_matches(
            &instances,
            "session-a",
            "/run/example/wayland-1",
            Some("/run/example")
        ));
        assert!(!instance_matches(
            &instances,
            "session-a",
            "wayland-2",
            None
        ));
        assert!(!instance_matches(
            &instances,
            "session-b",
            "wayland-1",
            None
        ));
    }

    #[test]
    fn bounded_process_rejects_timeout_and_reaps_child() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let start = Instant::now();
        let error = bounded_output(
            Command::new("sh")
                .args(["-c", "echo $$ > \"$1\"; exec sleep 10", "observer-test"])
                .arg(&pid_file),
            Duration::from_millis(100),
            1024,
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn bounded_process_limits_both_streams_and_reports_exit_failure() {
        for script in [
            "while :; do printf '0123456789'; done",
            "while :; do printf '0123456789' >&2; done",
        ] {
            let error = bounded_output(
                Command::new("sh").args(["-c", script]),
                Duration::from_secs(1),
                64,
            )
            .unwrap_err();
            assert!(error.to_string().contains("output exceeded limit"));
        }
        let error = bounded_output(
            Command::new("sh").args(["-c", "printf 'query rejected' >&2; exit 7"]),
            Duration::from_secs(1),
            64,
        )
        .unwrap_err();
        assert!(error.to_string().contains("query rejected"));
        let output = bounded_output(
            Command::new("sh").args(["-c", "printf '{}' "]),
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert_eq!(output, b"{}");
    }

    #[test]
    fn journal_reports_baseline_and_sampler_errors() {
        assert!(Journal::with_query(|| Err(ObserverError::new("baseline failure"))).is_err());
        let count = AtomicUsize::new(0);
        let journal = Journal::with_query(move || {
            if count.fetch_add(1, Ordering::Relaxed) == 0 {
                Ok(Some(1))
            } else {
                Err(ObserverError::new("sampler failure"))
            }
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !journal.sampler.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(journal.drain().unwrap_err().to_string(), "sampler failure");
    }

    #[test]
    fn journal_stop_waits_only_for_bounded_inflight_query() {
        let count = AtomicUsize::new(0);
        let (started, ready) = std::sync::mpsc::channel();
        let journal = Journal::with_query(move || {
            if count.fetch_add(1, Ordering::Relaxed) == 0 {
                return Ok(Some(1));
            }
            started.send(()).unwrap();
            bounded_output(
                Command::new("sleep").arg("10"),
                Duration::from_millis(100),
                64,
            )?;
            Ok(Some(1))
        })
        .unwrap();
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        assert!(journal
            .drain()
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
