#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::{exit, Command};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(driver) = args.next() else { exit(64) };
    let mut command = Command::new(driver);
    command.args(args);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    exit(
        command
            .status()
            .map_or(1, |status| status.code().unwrap_or(1)),
    );
}
