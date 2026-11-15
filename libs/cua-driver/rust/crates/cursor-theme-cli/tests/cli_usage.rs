// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::process::{Command, Output};
use tempfile::TempDir;

fn run(args: &[&str]) -> Output {
    let store = TempDir::new().unwrap();
    Command::new(env!("CARGO_BIN_EXE_cua-cursor-theme"))
        .args(args)
        .env("CUA_DRIVER_CURSOR_THEME_DIR", store.path())
        .output()
        .expect("run cursor-theme CLI")
}

#[test]
fn rejects_unknown_arguments_before_reading_or_changing_themes() {
    let cases: &[(&[&str], &str)] = &[
        (&["--bogus"], "--bogus"),
        (&["unknown"], "unknown"),
        (&["list", "--bogus"], "--bogus"),
        (&["list", "--json", "--bogus"], "--bogus"),
        (&["list", "unexpected"], "unexpected"),
        (&["validate", "missing.lottie", "--bogus"], "--bogus"),
        (
            &[
                "build",
                "missing.lottie",
                "--output",
                "unused.cua-theme",
                "--bogus",
            ],
            "--bogus",
        ),
        (&["inspect", "missing.cua-theme", "--bogus"], "--bogus"),
        (
            &[
                "preview",
                "missing.cua-theme",
                "--output",
                "unused",
                "--bogus",
            ],
            "--bogus",
        ),
        (&["install", "missing.cua-theme", "--bogus"], "--bogus"),
        (&["uninstall", "cua.default", "--bogus"], "--bogus"),
    ];
    for (args, unknown) in cases {
        let output = run(args);
        assert_eq!(output.status.code(), Some(64), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(unknown));
    }
}

#[test]
fn missing_duplicate_or_conflicting_arguments_are_usage_errors() {
    let cases: &[&[&str]] = &[
        &[],
        &["validate"],
        &["build", "missing.lottie"],
        &["build", "missing.lottie", "--output"],
        &["inspect"],
        &["preview", "missing.cua-theme"],
        &["install"],
        &["uninstall"],
        &["list", "--json", "--json"],
        &["inspect", "missing.cua-theme", "extra"],
        &["list", "--output", "unused"],
    ];
    for args in cases {
        let output = run(args);
        assert_eq!(output.status.code(), Some(64), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn help_and_listing_still_succeed_and_file_errors_remain_runtime_errors() {
    for args in [vec!["--help"], vec!["list"], vec!["list", "--json"]] {
        let output = run(&args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty());
        if args == ["list", "--json"] {
            let themes: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(themes, ["cua.default"]);
        }
    }
    let cases: &[&[&str]] = &[
        &["validate", "missing.lottie", "--development"],
        &[
            "build",
            "missing.lottie",
            "--output",
            "unused.cua-theme",
            "--development",
        ],
        &["inspect", "missing.cua-theme", "--json"],
        &["preview", "missing.cua-theme", "--output", "unused"],
        &["install", "missing.cua-theme"],
    ];
    for args in cases {
        let output = run(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}
