// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::Duration;

const MODE: &str = "CUA_RECORDING_ERROR_TEST_CHILD";
const LAUNCHED: &str = "CUA_RECORDING_TEST_LAUNCHED_ENCODER";

fn main() {
    if std::env::args().any(|arg| arg == "-version") {
        return;
    }
    let mode = std::env::var(MODE).unwrap_or_default();
    let output = std::env::args_os().last().unwrap();
    // A package-manager shim (Chocolatey's ffmpeg.exe) starts the real
    // encoder as a child sharing its stdin and stderr, then waits for it.
    if mode == "shim" && std::env::var_os(LAUNCHED).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(std::env::args_os().skip(1))
            .env(LAUNCHED, "1")
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }
    if mode == "shim" {
        let mut pid_file = output.clone();
        pid_file.push(".encoder-pid");
        std::fs::write(pid_file, std::process::id().to_string()).unwrap();
    }
    let mut byte = [0];
    while io::stdin().read_exact(&mut byte).is_ok() {
        if byte[0] == b'q' {
            match mode.as_str() {
                "timeout" | "shim" => std::thread::sleep(Duration::from_secs(60)),
                // A clean shutdown slower than three seconds, as when a
                // stalled desktop grab delays the encoder reading `q`.
                "slow" => {
                    std::thread::sleep(Duration::from_millis(4500));
                    std::fs::write(&output, b"finalized").unwrap();
                    std::process::exit(0);
                }
                _ => {}
            }
            std::process::exit(23);
        }
    }
    std::process::exit(24);
}
