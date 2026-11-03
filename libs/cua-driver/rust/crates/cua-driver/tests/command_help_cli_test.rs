use std::process::{Command, Output};

fn run(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .args(args)
        .env("CUA_DRIVER_RS_HOME", home)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .output()
        .expect("run cua-driver")
}

fn help(home: &std::path::Path, args: &[&str]) -> String {
    let output = run(home, args);
    assert!(
        output.status.success(),
        "{args:?} exited {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 help")
}

/// `<command> --help` prints that command's reference from `cli_docs_json()`
/// instead of the top-level help (#4708).
#[test]
fn command_help_prints_the_named_commands_reference() {
    let home = tempfile::tempdir().expect("temp home");
    let top = help(home.path(), &["--help"]);

    let recording = help(home.path(), &["recording", "--help"]);
    assert!(
        recording != top,
        "recording --help printed the top-level help"
    );
    assert!(
        recording.contains("Usage: cua-driver recording [OPTIONS] [COMMAND]"),
        "{recording}"
    );
    assert!(
        recording.contains("Without a subcommand, runs cua-driver recording status."),
        "{recording}"
    );
    for sub in ["start", "stop", "status", "render"] {
        assert!(
            recording.contains(&format!("  {sub} ")),
            "missing subcommand {sub}:\n{recording}"
        );
    }

    // Nested subcommand, the short flag, and a global value option before the command.
    let render = help(
        home.path(),
        &["--socket", "/nonexistent.sock", "recording", "render", "-h"],
    );
    assert!(
        render.contains("Usage: cua-driver recording render [OPTIONS] <input-dir> <out-mp4>"),
        "{render}"
    );
    assert!(render.contains("--scale <number>"), "{render}");
    assert!(render.contains("--no-zoom"), "{render}");

    // A word that is not a subcommand (here the <output-dir> value) stops the walk.
    let start = help(home.path(), &["recording", "start", "mydir", "--help"]);
    assert!(
        start.contains("Usage: cua-driver recording start <output-dir>"),
        "{start}"
    );

    // An unknown command keeps the top-level help.
    assert_eq!(help(home.path(), &["bogus", "--help"]), top);

    // A long-lived command still only prints its help and exits.
    let serve = help(home.path(), &["serve", "--help"]);
    assert!(
        serve.contains("Usage: cua-driver serve [OPTIONS]"),
        "{serve}"
    );
}
