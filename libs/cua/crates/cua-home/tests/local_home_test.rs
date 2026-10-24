//! The Local channel's home and environment (CUA-1776).
//!
//! `cua_home()`, `CuaEnv::current()` and `pin_home()` read the process
//! environment, which a test in this process cannot change safely, so each
//! case re-runs this test binary as a child with the environment it needs
//! (a temporary `HOME`, no `CUA_*` variable but the ones given) and has the
//! child report what it resolves.

use std::path::Path;
use std::process::Command;

use cua_home::channel::CuaChannel;
use cua_home::env::{CuaEnv, pin_home};

const PROBE_VAR: &str = "CUA_HOME_PROBE";
const CUA_VARS: [&str; 3] = ["CUA_CHANNEL", "CUA_ENV", "CUA_HOME"];

/// The child: prints what this process resolves, then what `pin_home`
/// leaves in the environment. A no-op as an ordinary test.
#[test]
fn probe() {
    if std::env::var_os(PROBE_VAR).is_none() {
        return;
    }
    let show = |name: &str, v: String| println!("probe {name}={v}");
    show("channel", CuaChannel::current().name().to_string());
    show("env", CuaEnv::current().name().to_string());
    show(
        "try_current",
        match CuaEnv::try_current() {
            Ok(e) => e.name().to_string(),
            Err(e) => format!("error: {e}"),
        },
    );
    show("home", cua_home::cua_home().display().to_string());
    // SAFETY: the probe is its own process and runs one test, on the main
    // thread (`--test-threads=1`).
    let pinned = unsafe { pin_home() };
    show(
        "pinned",
        pinned.map_or("none".to_string(), |p| p.display().to_string()),
    );
    for var in CUA_VARS {
        show(
            &format!("after {var}"),
            std::env::var(var).unwrap_or_else(|_| "unset".into()),
        );
    }
}

/// What the child resolved, by name.
struct Probe(Vec<(String, String)>);

impl Probe {
    fn get(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("the probe printed no {name}: {:?}", self.0))
    }
}

/// Runs the probe with `home` as `$HOME` and `vars` as the only `CUA_*`
/// variables.
fn run(home: &Path, vars: &[(&str, &str)]) -> Probe {
    let exe = std::env::current_exe().expect("test binary");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", "probe", "--nocapture", "--test-threads=1"])
        .env(PROBE_VAR, "1")
        .env("HOME", home);
    for var in CUA_VARS {
        cmd.env_remove(var);
    }
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run the probe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "probe failed: {stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Probe(
        stdout
            .lines()
            // libtest prints `test probe ... ` before the first line.
            .filter_map(|l| l.rsplit_once("probe ").map(|(_, rest)| rest))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.trim().to_string()))
            .collect(),
    )
}

/// A build with a compiled-in default decides before the variables do.
fn built_with_defaults() -> bool {
    CuaChannel::build_default().is_some() || option_env!("CUA_DEFAULT_ENV").is_some()
}

fn display(home: &Path, rest: &str) -> String {
    home.join(rest).display().to_string()
}

#[test]
fn local_home_is_cua_local_with_no_environment_below_it() {
    if built_with_defaults() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let local_home = display(home, ".cua-local");

    // Local, `CUA_ENV` unset: staging, and `~/.cua-local` itself.
    let p = run(home, &[("CUA_CHANNEL", "local")]);
    assert_eq!(p.get("channel"), "local");
    assert_eq!(p.get("env"), "staging");
    assert_eq!(p.get("try_current"), "staging");
    assert_eq!(p.get("home"), local_home);
    assert!(!p.get("home").contains("envs"));

    // `CUA_ENV=staging` spelled out is the same.
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "staging")]);
    assert_eq!(p.get("env"), "staging");
    assert_eq!(p.get("home"), local_home);

    // Blank `CUA_ENV` means unset.
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", " ")]);
    assert_eq!(p.get("env"), "staging");
    assert_eq!(p.get("home"), local_home);

    // `CUA_ENV=prod` is the explicit opt-in; the home stays Local's.
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "prod")]);
    assert_eq!(p.get("env"), "prod");
    assert_eq!(p.get("home"), local_home);

    // A mistyped `CUA_ENV` is an error at startup and staging, not prod,
    // anywhere that has to answer anyway.
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "prd")]);
    assert_eq!(p.get("env"), "staging");
    assert!(p.get("try_current").contains("CUA_ENV"), "{:?}", p.0);

    // `CUA_HOME` still wins.
    let elsewhere = display(home, "elsewhere");
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_HOME", &elsewhere)]);
    assert_eq!(p.get("home"), elsewhere);
}

#[test]
fn stable_homes_are_unchanged() {
    if built_with_defaults() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    let p = run(home, &[]);
    assert_eq!(p.get("channel"), "stable");
    assert_eq!(p.get("env"), "prod");
    assert_eq!(p.get("home"), display(home, ".cua"));

    let p = run(home, &[("CUA_CHANNEL", "stable")]);
    assert_eq!(p.get("home"), display(home, ".cua"));

    let p = run(home, &[("CUA_ENV", "staging")]);
    assert_eq!(p.get("env"), "staging");
    assert_eq!(p.get("home"), display(home, ".cua/envs/staging"));

    // A blank `CUA_ENV` and a mistyped one are production, as before.
    let p = run(home, &[("CUA_ENV", "")]);
    assert_eq!(p.get("env"), "prod");
    let p = run(home, &[("CUA_ENV", "prd")]);
    assert_eq!(p.get("env"), "prod");
    assert!(p.get("try_current").contains("CUA_ENV"), "{:?}", p.0);

    // An unknown channel is an error at startup, and Stable elsewhere.
    let p = run(home, &[("CUA_CHANNEL", "dev")]);
    assert_eq!(p.get("channel"), "stable");
    assert!(p.get("try_current").contains("CUA_CHANNEL"), "{:?}", p.0);
}

#[test]
fn pin_home_pins_local_for_every_child_and_leaves_stable_alone() {
    if built_with_defaults() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let local_home = display(home, ".cua-local");

    // Local: channel, environment and home, all pinned.
    let p = run(home, &[("CUA_CHANNEL", "local")]);
    assert_eq!(p.get("pinned"), local_home);
    assert_eq!(p.get("after CUA_CHANNEL"), "local");
    assert_eq!(p.get("after CUA_ENV"), "staging");
    assert_eq!(p.get("after CUA_HOME"), local_home);

    // Local with the production opt-in: the opt-in is kept, the home is
    // still pinned (Stable production would not pin one).
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "prod")]);
    assert_eq!(p.get("after CUA_ENV"), "prod");
    assert_eq!(p.get("after CUA_HOME"), local_home);

    // A `CUA_HOME` that is set is never replaced.
    let elsewhere = display(home, "elsewhere");
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_HOME", &elsewhere)]);
    assert_eq!(p.get("pinned"), "none");
    assert_eq!(p.get("after CUA_HOME"), elsewhere);
    assert_eq!(p.get("after CUA_CHANNEL"), "local");

    // Stable production pins nothing; Stable staging pins its home only.
    let p = run(home, &[]);
    assert_eq!(p.get("pinned"), "none");
    assert_eq!(p.get("after CUA_CHANNEL"), "unset");
    assert_eq!(p.get("after CUA_ENV"), "unset");
    assert_eq!(p.get("after CUA_HOME"), "unset");
    let p = run(home, &[("CUA_ENV", "staging")]);
    assert_eq!(p.get("pinned"), display(home, ".cua/envs/staging"));
    assert_eq!(p.get("after CUA_CHANNEL"), "unset");
}
