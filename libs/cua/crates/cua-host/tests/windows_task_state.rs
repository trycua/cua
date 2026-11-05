#![cfg(windows)]

use cua_host::service::{ServiceManager, WindowsTask};
use std::path::PathBuf;

#[test]
#[ignore = "read-only comparison with the existing host task; requires an installed Windows host"]
fn existing_host_task_matches_task_scheduler() {
    let observed = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; $s=New-Object -ComObject Schedule.Service; \
             $s.Connect(); [Console]::Out.Write([int]$s.GetFolder('\\Cua').GetTask('SpacesdHost').State)",
        ])
        .output()
        .expect("read-only Task Scheduler query");
    assert!(observed.status.success());
    let expected: i32 = String::from_utf8(observed.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let task = WindowsTask {
        xml_path: PathBuf::new(),
        user: String::new(),
    };
    let actual = task.state();
    assert!(actual.installed, "{actual:?}");
    assert_eq!(actual.running, expected == 4, "{actual:?}");
}
