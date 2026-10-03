// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Source scan: cua-spacesd never injects input itself.
//!
//! Every pointer, keyboard, scroll, drag, text and interactive-batch
//! injection is delegated to cua-driver (its platform crates and tool
//! registry). This test fails when production code under
//! `libs/cua-spacesd/crates/*/src` names an X11, CoreGraphics, Win32,
//! uinput/libei or command-line input-injection API, or when a crate manifest
//! pulls in an input-injection dependency or x11rb input extension.
//!
//! `cua-spacesd-test-apps` is excluded: its fixtures *receive* input (and parse X
//! event types) so the desktop suite can check what arrived.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Identifiers that only appear in code that synthesizes input.
const FORBIDDEN_IDENTIFIERS: &[&str] = &[
    // X11 (XTest, XSendEvent input events, pointer warps).
    "xtest",
    "xtest_fake_input",
    "XTestFakeKeyEvent",
    "XTestFakeButtonEvent",
    "XTestFakeMotionEvent",
    "KeyPressEvent",
    "KeyReleaseEvent",
    "ButtonPressEvent",
    "ButtonReleaseEvent",
    "MotionNotifyEvent",
    "KEY_PRESS_EVENT",
    "KEY_RELEASE_EVENT",
    "BUTTON_PRESS_EVENT",
    "BUTTON_RELEASE_EVENT",
    "MOTION_NOTIFY_EVENT",
    "warp_pointer",
    "XWarpPointer",
    // CoreGraphics / Quartz event posting.
    "CGEventPost",
    "CGEventPostToPid",
    "CGEventCreateKeyboardEvent",
    "CGEventCreateMouseEvent",
    "CGEventCreateScrollWheelEvent",
    "CGEventTapLocation",
    "CGWarpMouseCursorPosition",
    "new_keyboard_event",
    "new_mouse_event",
    "new_scroll_event",
    "post_to_pid",
    // Win32.
    "SendInput",
    "keybd_event",
    "mouse_event",
    "KEYBDINPUT",
    "MOUSEINPUT",
    "INPUT_KEYBOARD",
    "INPUT_MOUSE",
    "SetCursorPos",
    "WM_LBUTTONDOWN",
    "WM_LBUTTONUP",
    "WM_RBUTTONDOWN",
    "WM_RBUTTONUP",
    "WM_MOUSEMOVE",
    "WM_MOUSEWHEEL",
    "WM_KEYDOWN",
    "WM_KEYUP",
    "WM_CHAR",
    // Linux kernel / Wayland / command-line injectors.
    "uinput",
    "VirtualDevice",
    "evdev",
    "libei",
    "reis",
    "ydotool",
    "xdotool",
    "wtype",
];

/// Files that name a forbidden identifier only to *delegate* to cua-driver:
/// the presence cursor-shape prober calls cua-driver's
/// `PointerShapeBackend::warp_pointer` (the platform crate moves the pointer)
/// and reports the driver's backend names (for example "xtest"). The tests
/// module implements that trait with fakes. Nothing here injects input.
const DELEGATING_FILES: &[(&str, &[&str])] = &[
    ("src/grpc/cursor_shape.rs", &["warp_pointer", "xtest"]),
    ("src/grpc/tests.rs", &["warp_pointer"]),
    // The doctor checks an image's claimed cursor-shape backend names
    // ("probe=xtest"); it only compares strings.
    ("cua-spacesd-doctor/src/checks/capabilities.rs", &["xtest"]),
];

fn delegated(file: &Path, violation: &str) -> bool {
    DELEGATING_FILES.iter().any(|(suffix, tokens)| {
        file.ends_with(suffix)
            && tokens
                .iter()
                .any(|token| violation.ends_with(&format!("`{token}`")))
    })
}

/// Crates that inject input; none may be a direct dependency.
const FORBIDDEN_DEPENDENCIES: &[&str] = &[
    "enigo",
    "rdev",
    "evdev",
    "uinput",
    "input-linux",
    "reis",
    "core-graphics",
    "autopilot",
    "inputbot",
];

/// A forbidden dependency one crate may use for something other than input,
/// with why. The identifier scan above still fails on any event-posting call
/// (`CGEventPost`, `new_mouse_event`, ...) in every file of that crate.
const ALLOWED_DEPENDENCIES: &[(&str, &str, &str)] = &[(
    "cua-spacesd-desktop",
    "core-graphics",
    "display enumeration (src/macos_display.rs)",
)];

fn allowed(krate: &Path, dependency: &str) -> bool {
    ALLOWED_DEPENDENCIES
        .iter()
        .any(|(name, dep, _)| *dep == dependency && krate.ends_with(name))
}

/// x11rb extensions that exist to synthesize input.
const FORBIDDEN_X11RB_FEATURES: &[&str] = &["xtest", "xinput"];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

fn production_crates() -> Vec<PathBuf> {
    let mut crates: Vec<PathBuf> = std::fs::read_dir(crates_dir())
        .expect("read crates directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name != "cua-spacesd-test-apps")
        })
        .collect();
    crates.sort();
    crates
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>, budget: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        assert!(*budget > 0, "source scan visited too many entries");
        *budget -= 1;
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out, budget);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Code with `//` comments removed (a string containing `//` is cut short,
/// which only ever hides text, never adds a false positive).
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

fn identifiers(code: &str) -> impl Iterator<Item = &str> {
    code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
}

fn violations_in(source: &str) -> Vec<String> {
    let forbidden: HashSet<&str> = FORBIDDEN_IDENTIFIERS.iter().copied().collect();
    let code = strip_line_comments(source);
    let mut found = Vec::new();
    for (number, line) in code.lines().enumerate() {
        for token in identifiers(line) {
            if forbidden.contains(token) {
                found.push(format!("line {}: `{token}`", number + 1));
            }
        }
    }
    found
}

#[test]
fn production_sources_name_no_input_injection_api() {
    let mut budget = 20_000;
    let mut violations = Vec::new();
    let mut scanned = 0;
    for krate in production_crates() {
        let mut files = Vec::new();
        rust_files(&krate.join("src"), &mut files, &mut budget);
        for file in files {
            let source = std::fs::read_to_string(&file).expect("read source");
            scanned += 1;
            for violation in violations_in(&source) {
                if delegated(&file, &violation) {
                    continue;
                }
                violations.push(format!("{}: {violation}", file.display()));
            }
        }
    }
    assert!(
        scanned > 20,
        "scanned only {scanned} files; is the path right?"
    );
    assert!(
        violations.is_empty(),
        "cua-spacesd must delegate input injection to cua-driver; found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn manifests_pull_in_no_input_injection_dependency() {
    let mut violations = Vec::new();
    let workspace = crates_dir().parent().expect("workspace").to_path_buf();
    for krate in production_crates().into_iter().chain([workspace]) {
        let manifest_path = krate.join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
        let code = manifest
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default())
            .collect::<Vec<_>>();
        for line in &code {
            let key = line.split('=').next().unwrap_or_default().trim();
            if FORBIDDEN_DEPENDENCIES.contains(&key) && !allowed(&krate, key) {
                violations.push(format!("{}: dependency `{key}`", manifest_path.display()));
            }
            if key == "x11rb" {
                for feature in FORBIDDEN_X11RB_FEATURES {
                    if line.contains(&format!("\"{feature}\"")) {
                        violations.push(format!(
                            "{}: x11rb feature `{feature}`",
                            manifest_path.display()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "cua-spacesd must delegate input injection to cua-driver; found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_catches_what_it_should() {
    assert_eq!(
        violations_in("conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x, y, 0)?;").len(),
        2
    );
    assert_eq!(
        violations_in("event.post(CGEventTapLocation::HID);").len(),
        1
    );
    assert_eq!(
        violations_in("unsafe { SendInput(&inputs, size) };").len(),
        1
    );
    // Comments, look-alike identifiers and prose do not count.
    assert!(violations_in("// XTest and XSendEvent are cua-driver's job").is_empty());
    assert!(violations_in("let r: SendInputResponse = send_input_request();").is_empty());
    assert!(violations_in("fn adapter_newtypes_are_send_and_sync() {}").is_empty());
}
