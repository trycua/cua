//! CUA-1776: the CLI named `cua-local` is the Local channel's, and says so:
//! its usage lines and the examples in its help print `cua-local`, so a
//! copied example does not run Stable's `cua`. Stable's help is unchanged.
//!
//! The Local binary is a hard link of the built `cua` named `cua-local`
//! (the first run installs the bundled CLI under that name).

#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A directory with the built `cua` linked as `cua` and as `cua-local`.
struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        for name in ["cua", "cua-local"] {
            let to = dir.path().join(name);
            let from = Path::new(env!("CARGO_BIN_EXE_cua"));
            std::fs::hard_link(from, &to)
                .or_else(|_| std::fs::copy(from, &to).map(|_| ()))
                .unwrap();
        }
        Rig { dir }
    }

    fn bin(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Runs `program` as `arg0` with `CUA_CHANNEL` set to `channel`.
    fn run(
        &self,
        program: &str,
        arg0: Option<&str>,
        channel: Option<&str>,
        args: &[&str],
    ) -> String {
        let mut cmd = Command::new(self.bin(program));
        if let Some(a) = arg0 {
            cmd.arg0(a);
        }
        cmd.args(args)
            .env("HOME", self.dir.path())
            .env("CUA_HOME", self.dir.path().join(".cua"))
            .env("CUA_TELEMETRY", "0")
            .env("CUA_RELAY_URL", "http://127.0.0.1:9")
            // An empty PATH: a bare `argv[0]` is not looked up in the
            // developer's own `~/.local/bin`.
            .env("PATH", "")
            .env_remove("CUA_ENV")
            .env_remove("CUA_API_KEY")
            .env_remove("CUA_EXPERIMENTAL_TEAMS")
            .stdin(Stdio::null());
        match channel {
            Some(c) => cmd.env("CUA_CHANNEL", c),
            None => cmd.env_remove("CUA_CHANNEL"),
        };
        let out = cmd.output().expect("run cua");
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }
}

/// The help of `cua host setup`, which has `cua ...` examples.
const HELP: &[&str] = &["host", "setup", "--help"];

fn says_local(text: &str) {
    assert!(text.contains("Usage: cua-local host setup"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.trim_start().starts_with("cua-local host setup")),
        "{text}"
    );
    assert!(
        !text.lines().any(|l| l.trim_start().starts_with("cua ")),
        "{text}"
    );
}

fn says_stable(text: &str) {
    assert!(text.contains("Usage: cua host setup"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.trim_start().starts_with("cua host setup")),
        "{text}"
    );
    assert!(!text.contains("cua-local"), "{text}");
}

#[test]
fn stable_cua_is_unchanged() {
    let rig = Rig::new();
    says_stable(&rig.run("cua", None, None, HELP));
    says_stable(&rig.run("cua", None, Some("stable"), HELP));
}

#[test]
fn the_file_named_cua_local_is_the_local_cli() {
    let rig = Rig::new();
    says_local(&rig.run("cua-local", None, None, HELP));
    // Run as `cua` (argv[0] says one thing, the file another): the file
    // path alone says Local.
    says_local(&rig.run("cua-local", Some("cua"), None, HELP));
}

#[test]
fn the_name_it_was_started_under_says_local_too() {
    let rig = Rig::new();
    // A `cua` file started as `cua-local` (a link made later, a shell alias):
    // argv[0] alone says Local.
    says_local(&rig.run("cua", Some("cua-local"), None, HELP));
}

#[test]
fn cua_channel_wins_over_the_name() {
    let rig = Rig::new();
    says_local(&rig.run("cua", None, Some("local"), HELP));
    says_local(&rig.run("cua", Some("cua"), Some("LOCAL"), HELP));
    says_stable(&rig.run("cua-local", Some("cua"), Some("stable"), HELP));
}

#[test]
fn the_version_line_is_cua_in_both_channels() {
    let rig = Rig::new();
    let want = format!("cua {}", cua_sdk::VERSION);
    for (program, arg0, channel) in [
        ("cua", None, None),
        ("cua-local", None, None),
        ("cua-local", Some("cua"), None),
        ("cua", None, Some("local")),
    ] {
        let v = rig.run(program, arg0, channel, &["--version"]);
        assert_eq!(v.trim(), want, "{program} {arg0:?} {channel:?}");
    }
}

#[test]
fn help_advanced_follows_the_channel() {
    let rig = Rig::new();
    let stable = rig.run("cua", None, None, &["help", "advanced"]);
    assert!(stable.contains("cua host setup --relay"), "{stable}");
    assert!(!stable.contains("cua-local"), "{stable}");
    let local = rig.run("cua-local", None, None, &["help", "advanced"]);
    assert!(local.contains("cua-local host setup --relay"), "{local}");
    assert!(
        !local.lines().any(|l| l.trim_start().starts_with("cua ")),
        "{local}"
    );
}
