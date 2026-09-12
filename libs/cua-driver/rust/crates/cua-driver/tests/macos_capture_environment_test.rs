#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{driver_binary, Driver, McpDriver};
use serde_json::json;

struct StopDaemon<'a> {
    binary: &'a Path,
    socket: &'a Path,
}

impl Drop for StopDaemon<'_> {
    fn drop(&mut self) {
        let _ = Command::new(self.binary)
            .args(["stop", "--socket"])
            .arg(self.socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
#[ignore = "requires an unlocked macOS desktop and a TCC-authorized installed driver"]
fn desktop_capture_does_not_require_sbin_on_path() {
    let binary = driver_binary().canonicalize().expect("installed driver");
    let app = binary.ancestors().nth(3).expect("installed app bundle");
    assert_eq!(app.extension().and_then(|s| s.to_str()), Some("app"));
    for path in ["/usr/bin:/bin:/usr/sbin:/sbin", "/usr/bin:/bin"] {
        eprintln!("desktop capture with daemon PATH={path}");
        let output = tempfile::tempdir_in("/tmp").unwrap();
        let socket = output.path().join("d.sock");
        let _stop = StopDaemon {
            binary: &binary,
            socket: &socket,
        };
        assert!(Command::new("/usr/bin/open")
            .args([
                "-n",
                "-g",
                "--env",
                &format!("PATH={path}"),
                "--env",
                "CUA_DRIVER_RS_TELEMETRY_ENABLED=false"
            ])
            .arg(app)
            .args(["--args", "serve", "--socket"])
            .arg(&socket)
            .args([
                "--permission-mode",
                "unrestricted",
                "--dangerously-bypass-approvals",
                "--no-overlay"
            ])
            .status()
            .expect("launch the installed app")
            .success());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(
                Instant::now() < deadline,
                "owned daemon did not create its socket"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut driver = McpDriver::spawn_daemon_proxy_unrecorded(socket.to_str().unwrap())
            .expect("connect to the owned daemon");
        let session = driver.call(
            "start_session",
            json!({"session": "capture-path", "capture_scope": "desktop"}),
        );
        assert!(!session.is_error(), "{}", session.text());
        let png = output.path().join("desktop.png");
        let result = driver.call(
            "get_desktop_state",
            json!({"session": "capture-path", "screenshot_out_file": png}),
        );
        assert!(!result.is_error(), "{}", result.text());
        let image = image::open(&png).expect("decode the captured desktop PNG");
        let metadata = result.structured();
        assert_eq!(metadata["screenshot_mime_type"], "image/png");
        assert_eq!(metadata["screenshot_width"], image.width());
        assert_eq!(metadata["screenshot_height"], image.height());
        eprintln!("decoded desktop PNG: {}x{}", image.width(), image.height());
    }
}
