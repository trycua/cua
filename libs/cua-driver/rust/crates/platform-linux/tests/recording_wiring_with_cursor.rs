#![cfg(target_os = "linux")]

mod recording_wiring;

#[tokio::test]
async fn cursor_registration_installs_recording_support() {
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
    std::env::remove_var("CUA_WAYLAND_NEST");
    let config = cursor_overlay::CursorConfig {
        enabled: false,
        ..Default::default()
    };
    recording_wiring::assert_recording_wired(platform_linux::register_tools_with_cursor(
        config, false,
    ))
    .await;
}
