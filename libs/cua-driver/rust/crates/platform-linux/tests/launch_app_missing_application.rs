// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `launch_app` through the tool registry, against a private `PATH`, private
//! XDG application directories, the testkit's isolated home and a private
//! working directory.
//!
//! The tests set process-wide environment, so each holds `ENVIRONMENT` for its
//! whole run. No display variable is set, so the launch's window snapshot
//! reads nothing from a desktop, and every launch below fails before any
//! process starts.

#![cfg(target_os = "linux")]

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cua_driver_core::protocol::{Content, ToolResult};
use serde_json::{json, Value};

const MISSING_MESSAGE: &str = "is not an executable on PATH and matches no installed .desktop \
     application. Call list_apps and round-trip its launch_path.";

static ENVIRONMENT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn message_of(result: &ToolResult) -> String {
    match result.content.first() {
        Some(Content::Text { text, .. }) => text.clone(),
        other => panic!("the result must carry a text message, got {other:?}"),
    }
}

/// The error kind a direct spawn of `program` fails with. The cases below only
/// mean what they say if the filesystem produces these kinds, so each fixture
/// states its own.
fn spawn_error_kind(program: &Path, arguments: &[&str]) -> ErrorKind {
    match std::process::Command::new(program).args(arguments).spawn() {
        Ok(mut child) => {
            let _ = child.kill();
            panic!("{} must fail to spawn", program.display());
        }
        Err(error) => error.kind(),
    }
}

fn assert_spawn_error_kind(program: &Path, arguments: &[&str], expected: ErrorKind) {
    let kind = spawn_error_kind(program, arguments);
    assert_eq!(
        kind,
        expected,
        "spawning {} failed with {kind:?}, not {expected:?}; this filesystem (for \
         example one mounted noexec) cannot exercise the case",
        program.display()
    );
}

fn assert_plain_failure(result: &ToolResult, expected: &str, case: &str) {
    assert_eq!(result.is_error, Some(true), "{case}");
    assert_eq!(message_of(result), expected, "{case}");
    assert!(result.structured_content.is_none(), "{case}");
}

/// Private `PATH`, application directory, working directory and absent paths,
/// with the process environment pointed at them. Keep `ENVIRONMENT` locked
/// while it lives; dropping it restores the environment and the working
/// directory.
///
/// The search path, the XDG directories and every launcher command are
/// relative to the private working directory, so nothing the tests tokenize or
/// split on `:` depends on the text of `TMPDIR`.
struct Fixture {
    root: tempfile::TempDir,
    bin: PathBuf,
    absent: PathBuf,
    home: cua_driver_testkit::IsolatedStateRoot,
    previous_environment: Vec<(String, Option<std::ffi::OsString>)>,
    previous_directory: Option<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let data = root.path().join("data");
        let work = root.path().join("work");
        for dir in [&bin, &data.join("applications"), &work] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut fixture = Self {
            root,
            bin,
            // Absent: nothing creates it.
            absent: PathBuf::from("../absent"),
            home: cua_driver_testkit::IsolatedStateRoot::new().unwrap(),
            previous_environment: Vec::new(),
            previous_directory: std::env::current_dir().ok(),
        };

        // The testkit's isolated home stands in for the user's home and state
        // directories; app discovery and the search path are then pointed at
        // the private fixtures, so no host desktop entry or program is read.
        for (name, value) in fixture.home.env() {
            fixture.set(name, value);
        }
        fixture.set("XDG_DATA_HOME", "../data");
        fixture.set("XDG_DATA_DIRS", "../data");
        fixture.set("PATH", "../bin");
        std::env::set_current_dir(&work).unwrap();
        // No desktop, and no host authorization setting between the registry
        // and the tool.
        for variable in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            cua_driver_core::authorization::PERMISSION_MODE_ENV,
            cua_driver_core::authorization::DANGEROUS_BYPASS_ENV,
            cua_driver_core::authorization::DISABLE_UNRESTRICTED_ENV,
            cua_driver_core::session_manifest::CAPABILITY_MANIFEST_FILE_ENV,
            cua_driver_core::session_manifest::CAPABILITY_MANIFEST_APPROVED_ENV,
            cua_driver_core::session_manifest::SESSION_POLICY_FILE_ENV,
            cua_driver_core::session_manifest::SESSION_POLICY_APPROVED_ENV,
            cua_driver_core::policy::POLICY_FILE_ENV,
            cua_driver_core::policy::MANAGED_POLICY_FILE_ENV,
        ] {
            fixture.remember(variable);
            std::env::remove_var(variable);
        }
        fixture
    }

    fn remember(&mut self, name: &str) {
        self.previous_environment
            .push((name.to_owned(), std::env::var_os(name)));
    }

    fn set(&mut self, name: &str, value: impl AsRef<std::ffi::OsStr>) {
        self.remember(name);
        std::env::set_var(name, value);
    }

    fn desktop_entry(&self, file: &str, name: &str, exec: &Path) {
        std::fs::write(
            self.root.path().join("data/applications").join(file),
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nExec={}\n",
                exec.display()
            ),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(directory) = &self.previous_directory {
            let _ = std::env::set_current_dir(directory);
        }
        for (name, value) in self.previous_environment.iter().rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[tokio::test]
async fn a_missing_application_is_a_typed_refusal_and_other_failures_are_not() {
    let _environment = ENVIRONMENT.lock().await;
    let fixture = Fixture::new();

    // On PATH without execute permission: the spawn fails with EACCES.
    std::fs::write(fixture.bin.join("cua-not-executable"), "").unwrap();
    assert_spawn_error_kind(
        &fixture.bin.join("cua-not-executable"),
        &[],
        ErrorKind::PermissionDenied,
    );
    // An installed application whose launcher program does not exist.
    let missing_launcher = fixture.absent.join("cua-fixture-bin");
    fixture.desktop_entry("cua-fixture.desktop", "Cua Fixture", &missing_launcher);
    assert_spawn_error_kind(&missing_launcher, &[], ErrorKind::NotFound);
    // Installed applications that the cases below name ambiguously, or request
    // with other arguments; their launcher programs do not exist.
    fixture.desktop_entry(
        "cua-manager.desktop",
        "Cua Manager",
        &fixture.absent.join("m"),
    );
    fixture.desktop_entry(
        "cua-manager-settings.desktop",
        "Cua Manager Settings",
        &fixture.absent.join("ms"),
    );
    let with_mode = fixture.absent.join("cua-mode-bin");
    fixture.desktop_entry(
        "cua-mode.desktop",
        "Cua Mode",
        Path::new(&format!("{} --mode", with_mode.display())),
    );
    assert_spawn_error_kind(&with_mode, &["--mode", "--extra"], ErrorKind::NotFound);
    // No `xdg-open` on the private PATH.
    assert_spawn_error_kind(Path::new("xdg-open"), &[], ErrorKind::NotFound);
    // An absent program with an argument: the program is the first word.
    assert_spawn_error_kind(
        Path::new("cua-definitely-absent"),
        &["--flag"],
        ErrorKind::NotFound,
    );
    // Programs that are on no search path.
    let missing_path = fixture.absent.join("cua-launcher");
    assert_spawn_error_kind(&missing_path, &[], ErrorKind::NotFound);
    assert_spawn_error_kind(
        Path::new("cua-no-such-application"),
        &[],
        ErrorKind::NotFound,
    );
    // A NUL byte in an argument makes the spawn fail before any lookup.
    assert_spawn_error_kind(Path::new("cua-any"), &["a\0b"], ErrorKind::InvalidInput);

    let registry = platform_linux::tools::build_registry(false);
    let launch = |args: Value| registry.invoke("launch_app", args);

    // A `name` or `launch_path` that is not an executable and matches no
    // desktop entry is the typed refusal, with the text it always had.
    let missing_path = missing_path.to_str().unwrap();
    for (args, requested) in [
        (
            json!({ "name": "cua-no-such-application" }),
            "cua-no-such-application",
        ),
        (json!({ "launch_path": missing_path }), missing_path),
        (
            json!({ "launch_path": "cua-definitely-absent --flag" }),
            "cua-definitely-absent --flag",
        ),
    ] {
        let result = launch(args).await;
        assert_eq!(result.is_error, Some(true), "{requested}");
        assert_eq!(
            message_of(&result),
            format!("Failed to launch: '{requested}' {MISSING_MESSAGE}")
        );
        assert_eq!(
            result.structured_content,
            Some(json!({
                "code": "app_not_found",
                "effect": "refused",
                "name": requested,
                "pid": null,
                "running": null,
            })),
            "{requested}"
        );
    }

    // The failures below keep the plain text and carry no structured content.
    // The program exists on PATH but cannot be executed.
    let result = launch(json!({ "name": "cua-not-executable" })).await;
    assert_plain_failure(
        &result,
        &format!("Failed to launch: 'cua-not-executable' {MISSING_MESSAGE}"),
        "not executable",
    );
    // The program is absent but the spawn fails for another reason first.
    let result = launch(json!({
        "name": "cua-no-such-application",
        "additional_arguments": ["a\0b"],
    }))
    .await;
    assert_plain_failure(
        &result,
        &format!("Failed to launch: 'cua-no-such-application' {MISSING_MESSAGE}"),
        "NUL byte in an argument",
    );
    // A desktop entry matches, and its launcher program is missing.
    let result = launch(json!({ "name": "Cua Fixture" })).await;
    assert_plain_failure(
        &result,
        &format!(
            "Failed to launch: 'Cua Fixture' matched installed app 'Cua Fixture' but its \
             launcher `{}` failed to start: No such file or directory (os error 2)",
            missing_launcher.display()
        ),
        "missing desktop-entry launcher",
    );
    // A name that two installed entries match is ambiguous, not missing.
    let result = launch(json!({ "name": "cua man" })).await;
    assert_plain_failure(
        &result,
        &format!("Failed to launch: 'cua man' {MISSING_MESSAGE}"),
        "ambiguous installed name",
    );
    // An installed entry's launcher requested with other arguments.
    let requested = format!("{} --mode --extra", with_mode.display());
    let result = launch(json!({ "launch_path": requested })).await;
    assert_plain_failure(
        &result,
        &format!("Failed to launch: '{requested}' {MISSING_MESSAGE}"),
        "installed launcher with other arguments",
    );
    // A URL with no `xdg-open` to open it: a `NotFound` that is not the
    // direct spawn of the command.
    let result = launch(json!({ "urls": ["https://example.invalid/"] })).await;
    assert_plain_failure(
        &result,
        "Failed to launch: No such file or directory (os error 2)",
        "no xdg-open",
    );
    // The launch_path that list_apps reports for that entry: its launcher is
    // the missing program, and the command is not matched by name.
    let launcher = missing_launcher.to_str().unwrap();
    let result = launch(json!({ "launch_path": launcher })).await;
    assert_plain_failure(
        &result,
        &format!("Failed to launch: '{launcher}' {MISSING_MESSAGE}"),
        "launch_path of an installed entry",
    );
}

#[tokio::test]
async fn a_program_whose_interpreter_is_missing_keeps_the_plain_error() {
    let _environment = ENVIRONMENT.lock().await;
    let fixture = Fixture::new();

    // On PATH, executable, with a script interpreter that does not exist: the
    // spawn fails with ENOENT although the program exists. A filesystem that
    // does not allow execution fails this precheck, and only this test.
    let broken = fixture.bin.join("cua-broken-interpreter");
    std::fs::write(
        &broken,
        format!(
            "#!{}\n",
            fixture.root.path().join("absent/interpreter").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_spawn_error_kind(&broken, &[], ErrorKind::NotFound);
    // The same program by a path relative to the working directory, which
    // holds no whitespace whatever the temporary directory is called.
    let relative = "../bin/cua-broken-interpreter";
    assert_spawn_error_kind(Path::new(relative), &["--flag"], ErrorKind::NotFound);

    let registry = platform_linux::tools::build_registry(false);
    for (args, requested) in [
        (
            json!({ "name": "cua-broken-interpreter" }),
            "cua-broken-interpreter".to_owned(),
        ),
        (
            json!({ "launch_path": format!("{relative} --flag") }),
            format!("{relative} --flag"),
        ),
    ] {
        let result = registry.invoke("launch_app", args).await;
        assert_plain_failure(
            &result,
            &format!("Failed to launch: '{requested}' {MISSING_MESSAGE}"),
            &requested,
        );
    }
}
