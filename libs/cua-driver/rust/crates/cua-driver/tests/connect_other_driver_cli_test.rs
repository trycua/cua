// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Spaces' Set up and `cua-driver connect` register the same server name,
//! `cua-driver`. When an agent's server launches the driver inside Cua Spaces,
//! `connect` must say so instead of reporting the agent as connected to this
//! driver, and must leave the entry alone.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const SPACES_DRIVER: &str = "/Applications/Cua Spaces.app/Contents/Helpers/cua-driver";

#[test]
fn connect_reports_an_agent_that_uses_the_driver_inside_cua_spaces() {
    let root = tempfile::tempdir().unwrap();
    let (home, bin) = (root.path().join("home"), root.path().join("bin"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = serde_json::json!({ "mcpServers": { "cua-driver": {
        "command": SPACES_DRIVER,
        "args": ["mcp", "--embedded", "--socket", "/tmp/cua-driver.sock"],
    } } })
    .to_string();
    std::fs::write(home.join(".claude.json"), &config).unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .args(args)
            .env("HOME", &home)
            .env("PATH", &bin)
            .env("CUA_DRIVER_RS_HOME", &home)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap()
    };

    let output = run(&["connect", "--client", "claude", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["agents"][0]["status"], "other_driver", "{report}");
    assert_eq!(report["agents"][0]["detail"], SPACES_DRIVER, "{report}");
    assert!(report["agents"][0]["skills"].is_null(), "{report}");

    let output = run(&["connect", "--client", "claude"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!(
            "Claude Code: uses another Cua Driver ({SPACES_DRIVER}); left as is. To use this one, \
             remove its `cua-driver` server and run `cua-driver connect` again."
        )
    );
    // Nothing but the config the test wrote: no skill pack, no links.
    let left: Vec<_> = std::fs::read_dir(&home)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != ".cua-driver")
        .collect();
    assert_eq!(left, [".claude.json"]);
    assert_eq!(
        std::fs::read_to_string(home.join(".claude.json")).unwrap(),
        config
    );
}
