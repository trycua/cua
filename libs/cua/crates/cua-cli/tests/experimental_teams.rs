//! Teams are experimental: `cua team`, `cua host pair --team` and sharing
//! with `team:<id>` stay out of help and are refused unless
//! CUA_EXPERIMENTAL_TEAMS is set.

use std::process::{Command, Output, Stdio};

const UNAVAILABLE: &str = "Teams isn't available yet.";

fn cua(teams: Option<&str>, args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cua"));
    cmd.args(args)
        .env("HOME", home.path())
        .env("CUA_HOME", home.path().join(".cua"))
        .env("CUA_TELEMETRY", "0")
        .env("CUA_RELAY_URL", "http://127.0.0.1:9")
        .env_remove("CUA_ENV")
        .env_remove("CUA_API_KEY")
        .stdin(Stdio::null());
    match teams {
        Some(v) => cmd.env("CUA_EXPERIMENTAL_TEAMS", v),
        None => cmd.env_remove("CUA_EXPERIMENTAL_TEAMS"),
    };
    cmd.output().expect("run cua")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn lists_team(help: &str) -> bool {
    help.lines().any(|l| l.trim_start().starts_with("team "))
}

#[test]
fn without_the_flag_teams_are_hidden_and_refused() {
    for off in [None, Some("0")] {
        let help = text(&cua(off, &["--help"]));
        assert!(help.contains("spaces"), "{help}");
        assert!(!lists_team(&help), "{help}");
        assert!(!text(&cua(off, &["host", "pair", "--help"])).contains("--team"));

        for args in [
            &["team", "ls"][..],
            &["team", "create", "AIGIS"][..],
            &["host", "pair", "--team", "team-aigis"][..],
            &[
                "spaces",
                "share",
                "local:studio",
                "team:team-aigis",
                "--yes",
            ][..],
            &["spaces", "unshare", "local:studio", "team:team-aigis"][..],
        ] {
            let out = cua(off, args);
            assert!(!out.status.success(), "{args:?}: {out:?}");
            assert!(text(&out).contains(UNAVAILABLE), "{args:?}: {}", text(&out));
        }
    }
}

#[test]
fn with_the_flag_teams_work_as_before() {
    let help = text(&cua(Some("1"), &["--help"]));
    assert!(lists_team(&help), "{help}");
    assert!(text(&cua(Some("1"), &["host", "pair", "--help"])).contains("--team"));
    // Past the gate: they fail here only for want of a sign-in or relay.
    for args in [
        &["team", "ls"][..],
        &["host", "pair", "--team", "team-aigis"][..],
        &[
            "spaces",
            "share",
            "local:studio",
            "team:team-aigis",
            "--yes",
        ][..],
    ] {
        let out = cua(Some("1"), args);
        assert!(
            !text(&out).contains(UNAVAILABLE),
            "{args:?}: {}",
            text(&out)
        );
    }
}
