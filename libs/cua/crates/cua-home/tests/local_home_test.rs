//! The cua home, environment and Keychain service of every channel and
//! environment (CUA-1776): Stable and Local, each on prod and staging.
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
    show(
        "keychain",
        CuaChannel::current_keychain_service().to_string(),
    );
    show(
        "legacy keychain",
        CuaChannel::current_legacy_keychain_service()
            .unwrap_or("none")
            .to_string(),
    );
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
fn local_home_is_cua_local_on_staging_and_envs_prod_below_it_on_prod() {
    if built_with_defaults() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let local_home = display(home, ".cua-local");
    let local_prod_home = display(home, ".cua-local/envs/prod");

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

    // `CUA_ENV=prod` is the explicit opt-in, and it has a home of its own
    // below Local's, so it never shares one with Local's staging state.
    for prod in ["prod", "Production"] {
        let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", prod)]);
        assert_eq!(p.get("env"), "prod");
        assert_eq!(p.get("home"), local_prod_home, "CUA_ENV={prod}");
    }

    // A mistyped `CUA_ENV` is an error at startup and staging, not prod,
    // anywhere that has to answer anyway.
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "prd")]);
    assert_eq!(p.get("env"), "staging");
    assert_eq!(p.get("home"), local_home);
    assert!(p.get("try_current").contains("CUA_ENV"), "{:?}", p.0);

    // `CUA_HOME` still wins, on both environments.
    let elsewhere = display(home, "elsewhere");
    for env in ["staging", "prod"] {
        let p = run(
            home,
            &[
                ("CUA_CHANNEL", "local"),
                ("CUA_ENV", env),
                ("CUA_HOME", &elsewhere),
            ],
        );
        assert_eq!(p.get("home"), elsewhere);
    }
}

/// All four (channel, environment) pairs, as one process sees them: each
/// has a home and a Keychain service of its own, and none is shared.
#[test]
fn every_channel_and_environment_has_its_own_home_and_keychain_service() {
    if built_with_defaults() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    // (CUA_CHANNEL, CUA_ENV, resolved env, home below $HOME, Keychain
    // service, legacy Keychain service).
    let cases: [(&str, &str, &str, &str, &str, &str); 4] = [
        (
            "stable",
            "prod",
            "prod",
            ".cua",
            "ai.cua.session",
            "run.cua.ai",
        ),
        (
            "stable",
            "staging",
            "staging",
            ".cua/envs/staging",
            "ai.cua.session.staging",
            "staging.run.cua.ai",
        ),
        (
            "local",
            "staging",
            "staging",
            ".cua-local",
            "ai.cua.session.local.staging",
            "none",
        ),
        (
            "local",
            "prod",
            "prod",
            ".cua-local/envs/prod",
            "ai.cua.session.local",
            "none",
        ),
    ];
    let mut homes = Vec::new();
    let mut services = Vec::new();
    for (channel, env, want_env, want_home, want_service, want_legacy) in cases {
        let p = run(home, &[("CUA_CHANNEL", channel), ("CUA_ENV", env)]);
        let what = format!("{channel} on {env}");
        assert_eq!(p.get("channel"), channel, "{what}");
        assert_eq!(p.get("env"), want_env, "{what}");
        assert_eq!(p.get("home"), display(home, want_home), "{what}");
        assert_eq!(p.get("keychain"), want_service, "{what}");
        assert_eq!(p.get("legacy keychain"), want_legacy, "{what}");
        homes.push(p.get("home").to_string());
        services.push(p.get("keychain").to_string());
    }
    // Local on staging is also what Local with nothing set resolves to, and
    // Stable with nothing set is Stable on prod.
    let p = run(home, &[("CUA_CHANNEL", "local")]);
    assert_eq!(p.get("keychain"), "ai.cua.session.local.staging");
    assert_eq!(p.get("legacy keychain"), "none");
    let p = run(home, &[]);
    assert_eq!(p.get("keychain"), "ai.cua.session");
    assert_eq!(p.get("legacy keychain"), "run.cua.ai");

    for list in [&homes, &services] {
        let mut sorted = list.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "a home or service is shared: {list:?}");
    }
    // No Local home is inside a Stable home, or the other way round.
    let (stable_homes, local_homes) = homes.split_at(2);
    for s in stable_homes {
        for l in local_homes {
            let (s, l) = (Path::new(s), Path::new(l));
            assert!(
                !s.starts_with(l) && !l.starts_with(s),
                "{s:?} and {l:?} overlap"
            );
        }
    }
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

    // Local with the production opt-in: the opt-in is kept, and the home
    // pinned is Local's production one (Stable production would not pin one).
    let p = run(home, &[("CUA_CHANNEL", "local"), ("CUA_ENV", "prod")]);
    assert_eq!(p.get("pinned"), display(home, ".cua-local/envs/prod"));
    assert_eq!(p.get("after CUA_ENV"), "prod");
    assert_eq!(
        p.get("after CUA_HOME"),
        display(home, ".cua-local/envs/prod")
    );

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
