// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Show the permission guide window with a fixed status, for UI work and
//! screenshots. Asks macOS for nothing and opens no System Settings pane.
//!
//! ```text
//! cargo run -p platform-macos --example permission_guide_preview -- accessibility
//! cargo run -p platform-macos --example permission_guide_preview -- screen-recording
//! cargo run -p platform-macos --example permission_guide_preview -- done
//! ```
//!
//! The drag tile shows the app icon only when the binary runs from inside an
//! `.app` bundle.

#[cfg(target_os = "macos")]
fn main() {
    use platform_macos::permissions::gate::MissingPermission;
    use platform_macos::permissions::panel::{self, PanelHooks, PanelOpts};
    use platform_macos::permissions::PermissionsStatus;

    let step = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "accessibility".to_owned());
    let status = PermissionsStatus {
        accessibility: step != "accessibility",
        screen_recording: step == "done",
    };
    let hooks = PanelHooks {
        status: Box::new(move || status),
        request: Box::new(|_| {}),
        open_settings: Box::new(|_| {}),
    };
    if step == "done" {
        panel::show_finished(hooks);
        return;
    }
    let outcome = panel::show_modal(PanelOpts {
        initial_status: status,
        already_requested: vec![
            MissingPermission::Accessibility,
            MissingPermission::ScreenRecording,
        ],
        hooks,
    });
    println!("{outcome:?}");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The permission guide is macOS only.");
}
