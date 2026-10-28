//! Linux process-level behavior of the AT-SPI listener against private buses:
//! readiness when the bus accepts a connection but stalls, and liveness of the
//! process-lifetime accessibility connection while applications come and go.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::BufRead;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use cua_driver_testkit::{
    driver_binary_required, spawn_in_job, ChildReaper, IsolatedStateRoot, REQUIRE_DRIVER_BIN_ENV,
};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// `cua-driver serve` on the private session bus at `session_bus_address`,
/// with its per-user state under `state` (#4094). Keep `state` alive until the
/// child is reaped.
fn serve_command(
    daemon_socket: &Path,
    session_bus_address: &str,
    state: &IsolatedStateRoot,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .args([
            "serve",
            "--socket",
            daemon_socket.to_str().expect("UTF-8 daemon socket"),
            "--no-overlay",
            "--no-permissions-gate",
        ])
        .envs(state.env())
        .env("DBUS_SESSION_BUS_ADDRESS", session_bus_address)
        // A developer shell may export NO_AT_BRIDGE=1, which makes the driver
        // skip AT-SPI startup entirely; these tests exist to exercise it.
        .env_remove("NO_AT_BRIDGE")
        .env("CUA_DRIVER_RS_DISABLE_A11Y_ADVERTISE", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

#[test]
fn serve_binds_while_reachable_atspi_initialization_is_stalled() {
    let directory = tempfile::Builder::new()
        .prefix("cua-atspi-startup-")
        .tempdir_in("/tmp")
        .expect("temporary startup-test directory");
    let bus_path = directory.path().join("stalled-session-bus.sock");
    let daemon_socket = directory.path().join("driver.sock");
    let bus = UnixListener::bind(&bus_path).expect("bind reachable stalled bus");
    bus.set_nonblocking(true)
        .expect("make stalled bus listener nonblocking");
    let accepted = Arc::new(AtomicBool::new(false));
    let accepted_in_server = accepted.clone();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let bus_server = std::thread::spawn(move || {
        let accept_deadline = Instant::now() + Duration::from_secs(10);
        let connection = loop {
            match bus.accept() {
                Ok((connection, _)) => break Some(connection),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= accept_deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept driver bus connection: {error}"),
            }
        };
        if connection.is_some() {
            accepted_in_server.store(true, Ordering::SeqCst);
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        }
    });

    let state = IsolatedStateRoot::new().expect("isolated driver state root");
    let mut reaper = ChildReaper::new();
    let child = spawn_in_job(&mut serve_command(
        &daemon_socket,
        &format!("unix:path={}", bus_path.display()),
        &state,
    ))
    .expect("spawn cua-driver serve");
    reaper.push(child);

    let readiness_deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < readiness_deadline {
        if UnixStream::connect(&daemon_socket).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let ready = UnixStream::connect(&daemon_socket).is_ok();
    let _ = release_tx.send(());
    bus_server.join().expect("join stalled bus server");
    assert!(
        accepted.load(Ordering::SeqCst),
        "driver never reached the listening session-bus socket"
    );
    assert!(
        ready && daemon_socket.exists(),
        "serve did not bind after the bounded AT-SPI readiness wait"
    );
}

/// Start one private `dbus-daemon` in `directory`, owned by `reaper`, and
/// return its address. A missing binary skips the test, or panics under the
/// same `CUA_TEST_REQUIRE_DRIVER_BIN=1` strictness CI applies to the driver
/// binary. Explicit config: a sandbox (Nix) has no session.conf, and then the
/// daemon exits without printing an address.
fn start_private_bus(reaper: &mut ChildReaper, directory: &Path, name: &str) -> Option<String> {
    let config = directory.join(format!("{name}-bus.conf"));
    std::fs::write(
        &config,
        format!(
            "<busconfig><type>session</type>\
             <listen>unix:path={}</listen><auth>EXTERNAL</auth>\
             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
            directory.join(format!("{name}-bus.sock")).display()
        ),
    )
    .expect("write private bus config");
    let mut command = Command::new("dbus-daemon");
    command
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match spawn_in_job(&mut command) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert!(
                !driver_binary_required(),
                "dbus-daemon is not installed; {REQUIRE_DRIVER_BIN_ENV}=1 forbids skipping"
            );
            eprintln!("skipping: dbus-daemon is not installed");
            return None;
        }
        Err(error) => panic!("private test bus: {error}"),
    };
    let mut address = String::new();
    std::io::BufReader::new(child.stdout.take().expect("dbus-daemon stdout"))
        .read_line(&mut address)
        .expect("read private bus address");
    reaper.push(child);
    assert!(!address.trim().is_empty(), "dbus-daemon printed no address");
    Some(address.trim().to_owned())
}

/// `org.a11y.Bus` on the private session bus: tells the driver where the
/// private accessibility bus is.
struct AccessibilityBusLauncher {
    accessibility_bus_address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl AccessibilityBusLauncher {
    fn get_address(&self) -> String {
        self.accessibility_bus_address.clone()
    }
}

/// `org.a11y.atspi.Registry` on the private accessibility bus: remembers which
/// connection registered for which event.
struct Registry {
    registrations: Arc<Mutex<Vec<(String, String)>>>,
}

#[zbus::interface(name = "org.a11y.atspi.Registry")]
impl Registry {
    fn register_event(&self, event: String, #[zbus(header)] header: Header<'_>) {
        if let Some(sender) = header.sender() {
            self.registrations
                .lock()
                .unwrap()
                .push((sender.to_string(), event));
        }
    }
}

/// The registry's desktop root accessible, with no applications on it.
struct DesktopRoot;

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl DesktopRoot {
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        Vec::new()
    }
}

fn connect(address: &str) -> zbus::connection::Builder<'_> {
    zbus::connection::Builder::address(address).expect("private bus address")
}

async fn connection_stats(bus: &zbus::Connection, name: &str) -> HashMap<String, OwnedValue> {
    bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus.Debug.Stats"),
        "GetConnectionStats",
        &(name,),
    )
    .await
    .expect("GetConnectionStats (dbus-daemon built with statistics support)")
    .body()
    .deserialize()
    .expect("decode connection statistics")
}

fn queued_messages(stats: &HashMap<String, OwnedValue>) -> u32 {
    match stats.get("OutgoingMessages").map(|value| &**value) {
        Some(Value::U32(count)) => *count,
        other => panic!("OutgoingMessages missing from connection statistics: {other:?}"),
    }
}

async fn match_rules(bus: &zbus::Connection, name: &str) -> Vec<String> {
    let all: HashMap<String, Vec<String>> = bus
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus.Debug.Stats"),
            "GetAllMatchRules",
            &(),
        )
        .await
        .expect("GetAllMatchRules (dbus-daemon built with statistics support)")
        .body()
        .deserialize()
        .expect("decode match rules");
    all.get(name).cloned().unwrap_or_default()
}

/// `object:state-changed:focused` for one accessible, as a toolkit emits it.
fn focus_event(index: usize) -> zbus::Message {
    let properties: HashMap<&str, Value<'_>> = HashMap::new();
    zbus::Message::signal(
        format!("/org/a11y/atspi/accessible/probe_{index}"),
        "org.a11y.atspi.Event.Object",
        "StateChanged",
    )
    .expect("focus event header")
    .build(&("focused", 1_i32, 0_i32, Value::I32(0), properties))
    .expect("focus event body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accessibility_connection_keeps_draining_events_through_application_churn() {
    let directory = tempfile::Builder::new()
        .prefix("cua-atspi-churn-")
        .tempdir_in("/tmp")
        .expect("temporary churn-test directory");
    let state = IsolatedStateRoot::new().expect("isolated driver state root");
    let mut reaper = ChildReaper::new();
    let Some(session_bus_address) = start_private_bus(&mut reaper, directory.path(), "session")
    else {
        return;
    };
    let accessibility_bus_address = start_private_bus(&mut reaper, directory.path(), "a11y")
        .expect("second private bus starts once the first did");

    let _session_bus = connect(&session_bus_address)
        .name("org.a11y.Bus")
        .expect("launcher name")
        .serve_at(
            "/org/a11y/bus",
            AccessibilityBusLauncher {
                accessibility_bus_address: accessibility_bus_address.clone(),
            },
        )
        .expect("serve launcher")
        .build()
        .await
        .expect("own org.a11y.Bus on the private session bus");
    let registrations = Arc::new(Mutex::new(Vec::new()));
    let accessibility_bus = connect(&accessibility_bus_address)
        .name("org.a11y.atspi.Registry")
        .expect("registry name")
        .serve_at(
            "/org/a11y/atspi/registry",
            Registry {
                registrations: registrations.clone(),
            },
        )
        .expect("serve registry")
        .serve_at("/org/a11y/atspi/accessible/root", DesktopRoot)
        .expect("serve desktop root")
        .build()
        .await
        .expect("own org.a11y.atspi.Registry on the private accessibility bus");

    let daemon_socket = directory.path().join("driver.sock");
    let child = spawn_in_job(&mut serve_command(
        &daemon_socket,
        &session_bus_address,
        &state,
    ))
    .expect("spawn cua-driver serve");
    reaper.push(child);

    // The driver's unique name on the accessibility bus is the sender of its
    // startup event registrations; its StateChanged match rule follows them.
    let startup_deadline = Instant::now() + Duration::from_secs(15);
    let driver = loop {
        if let Some((sender, _)) = registrations.lock().unwrap().first().cloned() {
            break sender;
        }
        assert!(
            Instant::now() < startup_deadline,
            "cua-driver never registered an AT-SPI event listener on the private accessibility bus"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    loop {
        let rules = match_rules(&accessibility_bus, &driver).await;
        if rules
            .iter()
            .any(|rule| rule.contains("member='StateChanged'"))
        {
            break;
        }
        assert!(
            Instant::now() < startup_deadline,
            "cua-driver never subscribed to StateChanged on the accessibility bus: {rules:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // An application that joins the bus after the driver and never answers a
    // method call (it serves no objects), then 100 applications appearing and
    // leaving, then a burst of focus events for the driver's subscription.
    let _silent_application = connect(&accessibility_bus_address)
        .build()
        .await
        .expect("connect silent application");
    for _ in 0..100 {
        drop(
            connect(&accessibility_bus_address)
                .build()
                .await
                .expect("connect transient application"),
        );
    }
    for index in 0..3000 {
        accessibility_bus
            .send(&focus_event(index))
            .await
            .expect("emit focus event");
    }

    let drain_deadline = Instant::now() + Duration::from_secs(5);
    let queued = loop {
        let queued = queued_messages(&connection_stats(&accessibility_bus, &driver).await);
        if queued == 0 || Instant::now() >= drain_deadline {
            break queued;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        queued, 0,
        "{queued} messages stayed queued at the bus for cua-driver's accessibility connection: it stopped reading after application churn"
    );
}
