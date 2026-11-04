//! `cua --version` reports the released cua SDK version.

use std::process::{Command, Stdio};

#[test]
fn version_flag_prints_the_sdk_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_cua"))
        .arg("--version")
        .env("HOME", env!("CARGO_TARGET_TMPDIR"))
        .stdin(Stdio::null())
        .output()
        .expect("run cua --version");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim(), format!("cua {}", cua_sdk::VERSION));
    assert_eq!(cua_sdk::VERSION, cua_sdk::cua_sdk_version());
}
