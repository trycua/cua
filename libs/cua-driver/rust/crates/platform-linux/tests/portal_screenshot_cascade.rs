// SPDX-License-Identifier: MIT

//! The Wayland display-capture cascade against an xdg-desktop-portal served
//! on a private session bus. No compositor, X server or real portal is used:
//! `WAYLAND_DISPLAY` names a socket that does not exist and `DISPLAY` is
//! unset, so the cascade reaches the portal tier and any X11 fallback shows
//! up as an X11 error.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// The tests set process-wide environment, so they run one at a time.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// What the fake portal answers to a Screenshot request, as the
/// `org.freedesktop.portal.Request::Response` code.
#[derive(Clone, Copy)]
enum Answer {
    /// 1: the user cancelled or denied the request.
    Denied,
    /// 2: the interaction ended for some other reason.
    Other,
}

struct FakeScreenshotPortal {
    answer: Answer,
}

#[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
impl FakeScreenshotPortal {
    async fn screenshot(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _parent_window: &str,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header.sender().expect("request has a sender").to_string();
        let token = options
            .get("handle_token")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .expect("ashpd sends a handle_token");
        let path = OwnedObjectPath::try_from(format!(
            "/org/freedesktop/portal/desktop/request/{}/{token}",
            sender.trim_start_matches(':').replace('.', "_")
        ))
        .expect("request object path");
        let code: u32 = match self.answer {
            Answer::Denied => 1,
            Answer::Other => 2,
        };
        let connection = connection.clone();
        let signal_path = path.clone();
        tokio::spawn(async move {
            // Answer after the method reply, as a real portal does.
            tokio::time::sleep(Duration::from_millis(100)).await;
            connection
                .emit_signal(
                    None::<()>,
                    &signal_path,
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(code, HashMap::<String, OwnedValue>::new()),
                )
                .await
                .expect("emit Request::Response");
        });
        Ok(path)
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
}

/// A private `dbus-daemon`, killed on drop.
struct PrivateBus {
    child: Child,
    address: String,
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a private session bus in `directory`, or `None` (skip) when
/// `dbus-daemon` is not installed. Explicit config: a sandbox (Nix) has no
/// session.conf.
fn start_private_bus(directory: &Path) -> Option<PrivateBus> {
    let config = directory.join("session-bus.conf");
    std::fs::write(
        &config,
        format!(
            "<busconfig><type>session</type>\
             <listen>unix:path={}</listen><auth>EXTERNAL</auth>\
             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
            directory.join("session-bus.sock").display()
        ),
    )
    .expect("write private bus config");
    let mut child = match Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: dbus-daemon is not installed");
            return None;
        }
        Err(error) => panic!("private test bus: {error}"),
    };
    let mut address = String::new();
    std::io::BufReader::new(child.stdout.take().expect("dbus-daemon stdout"))
        .read_line(&mut address)
        .expect("read private bus address");
    let bus = PrivateBus {
        child,
        address: address.trim().to_owned(),
    };
    assert!(!bus.address.is_empty(), "dbus-daemon printed no address");
    Some(bus)
}

/// Capture the display through the public cascade while the fake portal on
/// a private bus gives `answer`. `None` when the test is skipped.
fn capture_with_portal_answer(answer: Answer) -> Option<anyhow::Result<Vec<u8>>> {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::Builder::new()
        .prefix("cua-portal-cascade-")
        .tempdir_in("/tmp")
        .expect("temporary test directory");
    let bus = start_private_bus(directory.path())?;
    let runtime = tokio::runtime::Runtime::new().expect("portal runtime");
    let _portal = runtime.block_on(async {
        zbus::connection::Builder::address(bus.address.as_str())
            .expect("private bus address")
            .name("org.freedesktop.portal.Desktop")
            .expect("portal name")
            .serve_at(
                "/org/freedesktop/portal/desktop",
                FakeScreenshotPortal { answer },
            )
            .expect("serve fake portal")
            .build()
            .await
            .expect("own org.freedesktop.portal.Desktop on the private bus")
    });
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    std::env::set_var("CUA_DRIVER_RS_ENABLE_WAYLAND", "1");
    std::env::set_var("WAYLAND_DISPLAY", directory.path().join("no-compositor"));
    for name in [
        "DISPLAY",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "CUA_INJECT_SOCKET",
        "XDG_CURRENT_DESKTOP",
    ] {
        std::env::remove_var(name);
    }
    Some(platform_linux::capture::screenshot_display_bytes())
}

#[test]
fn denied_portal_screenshot_reaches_the_caller_as_a_denied_error() {
    let Some(result) = capture_with_portal_answer(Answer::Denied) else {
        return;
    };
    let message = result
        .expect_err("a denied portal must not produce a capture")
        .to_string();
    assert!(
        message.starts_with("xdg-desktop-portal Screenshot denied"),
        "denial was not reported to the caller: {message}"
    );
}

#[test]
fn unspecified_portal_refusal_is_not_granted_and_not_called_denied() {
    let Some(result) = capture_with_portal_answer(Answer::Other) else {
        return;
    };
    let message = result
        .expect_err("a refused portal must not produce a capture")
        .to_string();
    assert!(
        message.starts_with("xdg-desktop-portal Screenshot not granted"),
        "refusal was not reported to the caller: {message}"
    );
    assert!(!message.contains("denied"), "{message}");
}
