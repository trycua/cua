//! CUA-1776: the CLI named `cua-local` is the Local channel's, and says so:
//! its usage lines and the examples in its help print `cua-local`, so a
//! copied example does not run Stable's `cua`. Stable's help is unchanged.
//!
//! The Local binary is a hard link of the built `cua` named `cua-local`
//! (the first run installs the bundled CLI under that name).
//!
//! It is Local at startup too, not only in its help: with no `CUA_CHANNEL`,
//! `CUA_ENV` or `CUA_HOME` (a plain shell) `cua-local` uses staging and
//! `~/.cua-local`, and never reads or writes Stable's `~/.cua`.

#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;

/// A directory with the built `cua` linked as `cua` and as `cua-local`.
struct Rig {
    dir: tempfile::TempDir,
}

/// The first start of a freshly built `cua` can take seconds on macOS (the
/// system checks the new 400 MB executable), and a dozen starting at once
/// on a busy machine can be killed while they wait. Start it once, alone,
/// before any test links or runs it.
fn warm_up() {
    static WARM: Once = Once::new();
    WARM.call_once(|| {
        let _ = Command::new(env!("CARGO_BIN_EXE_cua"))
            .arg("--version")
            .env("CUA_TELEMETRY", "0")
            .stdin(Stdio::null())
            .output();
    });
}

impl Rig {
    fn new() -> Self {
        warm_up();
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
    // A command named in the middle of a help text (in backticks) too.
    assert!(text.contains("`cua-local host pair`"), "{text}");
    assert!(!text.contains("`cua "), "{text}");
}

fn says_stable(text: &str) {
    assert!(text.contains("Usage: cua host setup"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.trim_start().starts_with("cua host setup")),
        "{text}"
    );
    assert!(!text.contains("cua-local"), "{text}");
    assert!(text.contains("`cua host pair`"), "{text}");
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
    // And a command named in a sentence, not only the examples.
    assert!(local.contains("Works with cua-local host setup"), "{local}");
    assert!(stable.contains("Works with cua host setup"), "{stable}");
    assert!(
        !local.lines().any(|l| l.trim_start().starts_with("cua ")),
        "{local}"
    );
}

// ---- Hints and errors name the CLI that printed them --------------------------------

/// `program args...` in a fresh fake `HOME` with a clean environment (and the
/// file credential store: never the developer's Keychain). Returns the exit
/// code, stdout and stderr.
fn run_fresh(rig: &Rig, program: &str, args: &[&str]) -> (Option<i32>, String, String) {
    let home = tempfile::tempdir_in(rig.dir.path()).unwrap().keep();
    let out = Command::new(rig.bin(program))
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
        .env("CUA_TELEMETRY", "0")
        .env("CUA_CREDENTIAL_STORE", "file")
        .stdin(Stdio::null())
        .output()
        .expect("run cua");
    (
        out.status.code(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A hint that tells the reader to run a `cua` command names the CLI the
/// reader ran: `cua-local` for Local, so pasting it does not run Stable's
/// `cua`. Stable's output is unchanged.
#[test]
fn hints_and_errors_name_the_cli_that_printed_them() {
    let rig = Rig::new();

    // A hint on stdout: the status of a Mac that is not set up. (The rest of
    // what it prints, and its exit status, depend on this machine's disk.)
    let (_, out, _) = run_fresh(&rig, "cua", &["host", "status"]);
    assert!(
        out.contains("This computer is not set up for access (run `cua host setup`)."),
        "{out}"
    );
    assert!(!out.contains("cua-local"), "{out}");
    let (_, out, _) = run_fresh(&rig, "cua-local", &["host", "status"]);
    assert!(
        out.contains("This computer is not set up for access (run `cua-local host setup`)."),
        "{out}"
    );
    assert!(!out.contains("`cua "), "{out}");

    // An error on stderr: a command that needs the sign-in. The error line
    // starts with the name the user ran.
    let says = |err: &str, line: &str| err.lines().any(|l| l == line);
    let (stable_code, _, err) = run_fresh(&rig, "cua", &["host", "pair"]);
    assert!(
        says(
            &err,
            "cua: unauthenticated: not signed in to cua.ai; run `cua auth login` first"
        ),
        "{err}"
    );
    let (local_code, _, err) = run_fresh(&rig, "cua-local", &["host", "pair"]);
    assert!(
        says(
            &err,
            "cua-local: unauthenticated: not signed in to cua.ai; run `cua-local auth login` first"
        ),
        "{err}"
    );
    assert_eq!(
        local_code, stable_code,
        "the exit code does not depend on the name"
    );

    // An error made in `cua-host` (it is shared with the app), not in the CLI.
    let (stable_code, _, err) = run_fresh(&rig, "cua", &["host", "start"]);
    assert!(
        says(
            &err,
            "cua: not found: this computer is not set up for access (run `cua host setup`)"
        ),
        "{err}"
    );
    let (local_code, _, err) = run_fresh(&rig, "cua-local", &["host", "start"]);
    assert!(
        says(
            &err,
            "cua-local: not found: this computer is not set up for access (run `cua-local host setup`)"
        ),
        "{err}"
    );
    assert_eq!(
        local_code, stable_code,
        "the exit code does not depend on the name"
    );
}

// ---- Startup: the environment and the home -----------------------------------

/// `cua auth status --json` of `program` in a fresh fake `HOME`, with the
/// whole environment cleared but `HOME`, `extra` and what keeps the run
/// hermetic (no `CUA_CHANNEL`, `CUA_ENV` or `CUA_HOME` unless `extra` sets
/// one). Returns the fake home and what the CLI reported.
fn status(
    rig: &Rig,
    program: &str,
    arg0: Option<&str>,
    extra: &[(&str, &str)],
) -> (PathBuf, serde_json::Value, String) {
    let home = tempfile::tempdir_in(rig.dir.path()).unwrap().keep();
    let mut cmd = Command::new(rig.bin(program));
    if let Some(a) = arg0 {
        cmd.arg0(a);
    }
    cmd.args(["auth", "status"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
        .env("CUA_TELEMETRY", "0")
        // The file store: never the developer's Keychain.
        .env("CUA_CREDENTIAL_STORE", "file")
        .envs(extra.iter().copied())
        .stdin(Stdio::null());
    let text = cmd.output().expect("run cua auth status");
    // Not logged in: exit 1, still reports.
    assert_eq!(text.status.code(), Some(1), "{text:?}");
    let text = String::from_utf8(text.stdout).unwrap();
    let json = cmd
        .arg("--json")
        .output()
        .expect("run cua auth status --json");
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    let json = serde_json::from_slice(&json.stdout).expect("json");
    (home, json, text)
}

/// The home the status line names (`home <path>).`), if it names one.
fn home_in(text: &str) -> Option<&str> {
    let rest = text.split("home ").nth(1)?;
    rest.split(").").next()
}

#[test]
fn a_plain_shell_cua_local_is_staging_with_its_own_home() {
    let rig = Rig::new();
    // No CUA_CHANNEL, CUA_ENV or CUA_HOME at all: the name alone decides.
    for arg0 in [None, Some("cua")] {
        let (home, json, text) = status(&rig, "cua-local", arg0, &[]);
        assert_eq!(json["env"], "staging", "{json}");
        assert!(
            json["issuer"].as_str().unwrap().contains("staging"),
            "{json}"
        );
        assert!(
            json["relay"].as_str().unwrap().contains("staging"),
            "{json}"
        );
        let want = home.join(".cua-local");
        assert_eq!(
            json["store"],
            format!("file {}", want.join("credentials.json").display()),
            "{json}"
        );
        assert!(text.contains("Environment: staging"), "{text}");
        assert_eq!(home_in(&text), want.to_str(), "{text}");
        // Stable's home is never created or read.
        assert!(!home.join(".cua").exists(), "{:?}", home.join(".cua"));
    }
}

#[test]
fn cua_local_with_the_prod_opt_in_keeps_its_own_home() {
    let rig = Rig::new();
    let (home, json, text) = status(&rig, "cua-local", None, &[("CUA_ENV", "prod")]);
    assert_eq!(json["env"], "prod", "{json}");
    assert_eq!(
        json["store"],
        format!(
            "file {}",
            home.join(".cua-local/envs/prod/credentials.json").display()
        ),
        "{json}"
    );
    // Production prints no "Environment:" line.
    assert!(!text.contains("Environment:"), "{text}");
    assert!(!home.join(".cua").exists());
}

#[test]
fn a_plain_shell_cua_is_still_prod_with_the_stable_home() {
    let rig = Rig::new();
    let (home, json, text) = status(&rig, "cua", None, &[]);
    assert_eq!(json["env"], "prod", "{json}");
    assert_eq!(
        json["store"],
        format!("file {}", home.join(".cua/credentials.json").display()),
        "{json}"
    );
    assert!(!text.contains("Environment:"), "{text}");
    assert!(!home.join(".cua-local").exists());
    // And staging on Stable keeps `~/.cua/envs/staging`, as before.
    let (home, json, _) = status(&rig, "cua", None, &[("CUA_ENV", "staging")]);
    assert_eq!(json["env"], "staging", "{json}");
    assert_eq!(
        json["store"],
        format!(
            "file {}",
            home.join(".cua/envs/staging/credentials.json").display()
        ),
        "{json}"
    );
    assert!(!home.join(".cua-local").exists());
}
