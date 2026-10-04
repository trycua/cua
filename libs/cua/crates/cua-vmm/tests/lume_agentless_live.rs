//! Opt-in: agentless exec and screenshot against a running macOS Lume VM
//! the caller created (for example with `cua sb create macos --name
//! cua-e2e-mac`). Never creates, starts or stops a VM.
//!
//! `CUA_LUME_LIVE_VM=cua-e2e-mac cargo test -p cua-vmm --test lume_agentless_live`

#![cfg(all(feature = "lume", feature = "vnc"))]

use std::time::Duration;

use cua_vmm::{ExecRequest, Runtime, lume::LumeRuntime};

fn vm() -> Option<String> {
    std::env::var("CUA_LUME_LIVE_VM")
        .ok()
        .filter(|v| v.starts_with("cua-e2e-"))
}

#[tokio::test]
async fn exec_and_screenshot_without_spacesd() {
    let Some(name) = vm() else {
        eprintln!("skipped: set CUA_LUME_LIVE_VM=cua-e2e-<name>");
        return;
    };
    let rt = LumeRuntime::with_defaults();
    let exec = rt.guest_exec(&name).await.unwrap().expect("macOS guest");
    let out = exec
        .exec(ExecRequest::sh("sw_vers; echo oops >&2; exit 5").timeout(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(out.exit_code, 5);
    assert!(out.stdout_str().contains("ProductName"), "{out:?}");
    assert_eq!(out.stderr_str(), "oops\n");

    let ep = rt.endpoints(&name).await.unwrap();
    let vnc = ep.vnc.expect("lume reports a VNC endpoint");
    let fb = cua_vmm::vnc::capture_png(&vnc, Duration::from_secs(30))
        .await
        .unwrap();
    assert!(
        fb.width >= 640 && fb.height >= 480,
        "{}x{}",
        fb.width,
        fb.height
    );
    assert!(fb.png.starts_with(b"\x89PNG"));
    if let Ok(path) = std::env::var("CUA_LUME_LIVE_SHOT") {
        std::fs::write(path, &fb.png).unwrap();
    }
}
