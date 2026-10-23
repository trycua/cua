#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;

#[test]
fn video_uses_the_nested_session_created_after_registration() {
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
    std::env::remove_var("CUA_WAYLAND_NEST");
    let _registry = platform_linux::register_tools();
    let directory = tempfile::tempdir().unwrap();
    let compositor = directory.path().join("compositor");
    std::fs::write(
        &compositor,
        "#!/bin/sh\n: > \"$XDG_RUNTIME_DIR/wayland-1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&compositor, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("XDG_RUNTIME_DIR", directory.path());
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={}/missing-bus", directory.path().display()),
    );
    std::env::set_var("CUA_WAYLAND_NEST", "1");
    std::env::set_var("CUA_WAYLAND_NEST_COMPOSITOR", compositor);
    std::env::set_var(platform_linux::wayland::ENABLE_WAYLAND_ENV, "1");
    platform_linux::wayland::ensure_nested_session();
    assert_eq!(std::env::var("WAYLAND_DISPLAY").unwrap(), "wayland-1");
    std::env::set_var("PATH", directory.path());
    let error = match cua_driver_core::video::start_video(&directory.path().join("recording.mp4")) {
        Ok(video) => {
            let _ = video.stop();
            panic!("video cannot start without a recorder executable");
        }
        Err(error) => error.to_string(),
    };
    assert!(error.contains("wf-recorder not found"), "{error}");
}
