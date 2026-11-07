// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Command, Stdio};

use cua_driver_testkit::IsolatedStateRoot;

#[test]
fn cursor_theme_uses_the_real_executables_sibling_through_symlinks() {
    let home = IsolatedStateRoot::new().unwrap();
    let install = home.path().join("Cua Driver.app/Contents/MacOS");
    let bin_dir = home.path().join(".local/bin");
    fs::create_dir_all(&install).unwrap();
    fs::create_dir_all(&bin_dir).unwrap();
    let driver = install.join("cua-driver");
    fs::copy(env!("CARGO_BIN_EXE_cua-driver"), &driver).unwrap();
    let sidecar = install.join("cua-cursor-theme");
    fs::write(&sidecar, "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\nexit 17\n").unwrap();
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o755)).unwrap();

    let link = bin_dir.join("cua-driver");
    symlink(&driver, &link).unwrap();
    let chain = bin_dir.join("cua-driver-chain");
    symlink("cua-driver", &chain).unwrap();
    assert!(!bin_dir.join("cua-cursor-theme").exists());

    for executable in [&driver, &link, &chain] {
        let output = Command::new(executable)
            .args(["cursor-theme", "list", "--json", "argument with spaces"])
            .envs(home.env())
            .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .env("DO_NOT_TRACK", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(17),
            "{}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "<list>\n<--json>\n<argument with spaces>\n"
        );
    }

    fs::remove_file(&sidecar).unwrap();
    let output = Command::new(&chain)
        .args(["cursor-theme", "list", "--json"])
        .envs(home.env())
        .env("CUA_DRIVER_CLI_TELEMETRY_CHILD", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .env("DO_NOT_TRACK", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // macOS may expose /var as /private/var after resolving the executable.
    let expected = install.canonicalize().unwrap().join("cua-cursor-theme");
    assert!(
        stderr.contains(&format!(
            "cursor-theme compiler is unavailable at {}:",
            expected.display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("Reinstall Cua Driver"), "{stderr}");
}
