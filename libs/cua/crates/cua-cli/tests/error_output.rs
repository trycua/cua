//! How `cua` shows an error: `error:` and `fix:` lines, the layers only with
//! `-v`, `--json` errors with a stable code, NO_COLOR, and the verbosity
//! flags. Nothing here starts a VM or a container.

mod common;
use common::*;
use serde_json::Value;

fn home() -> Home {
    Home::new()
}

/// A recorded sandbox named `name` (a stopped QEMU one), so the name is taken.
fn taken(h: &Home, name: &str) {
    let state = cua_sandbox_core::StateStore::new(h.cua_home().join("sandboxes"));
    state
        .save(&cua_sandbox_core::SandboxState::Local(
            cua_sandbox_core::LocalState {
                name: name.into(),
                runtime_type: "qemu".into(),
                host: "127.0.0.1".into(),
                status: "stopped".into(),
                created_at: cua_sandbox_core::state::python_utc_now(),
                ..Default::default()
            },
        ))
        .unwrap();
}

/// Text that must never reach a person: a layer of our own code, a source
/// path or a Rust debug struct.
const INTERNAL: &[&str] = &[
    "local runtime:",
    "[local]",
    "container engine:",
    "libs/cua",
    "crates/",
    ".rs:",
    "DispatchFailure",
    "{ source",
    "cua: ",
];

fn assert_plain(text: &str) {
    for bad in INTERNAL {
        assert!(!text.contains(bad), "{bad:?} in:\n{text}");
    }
    assert!(!text.contains('\x1b'), "ANSI in:\n{text}");
}

#[tokio::test]
async fn a_taken_name_says_so_with_the_fix() {
    let h = home();
    taken(&h, "dup2");
    let o = h
        .run(&["--embedded", "sb", "create", "linux", "--name", "dup2"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert_eq!(
        o.stderr,
        "error: a sandbox named 'dup2' already exists\nfix: pick another name, or remove it with `cua sb rm dup2`\n"
    );
    assert!(o.stdout.is_empty(), "{o:?}");
    // It is not an authentication failure, and the old sandbox is untouched.
    assert!(!o.stderr.contains("bearer"), "{o:?}");
    assert!(
        h.cua_home().join("sandboxes/dup2.json").exists(),
        "the existing sandbox's state is kept"
    );
}

#[tokio::test]
async fn a_taken_name_in_json_is_a_json_error_on_stdout() {
    let h = home();
    taken(&h, "dup3");
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "create",
            "linux",
            "--name",
            "dup3",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.is_empty(), "{o:?}");
    assert_eq!(
        o.json(),
        serde_json::json!({"error": {
            "code": "already_exists",
            "message": "a sandbox named 'dup3' already exists",
            "hint": "pick another name, or remove it with `cua sb rm dup3`",
        }})
    );
}

#[tokio::test]
async fn not_found_is_an_error_line_a_fix_and_exit_3() {
    let h = home();
    let o = h
        .run(&["--embedded", "sb", "exec", "nope", "echo hi"])
        .await;
    assert_eq!(o.code, 3, "{o:?}");
    assert!(o.stderr.starts_with("error: "), "{o:?}");
    assert!(o.stderr.contains("\nfix: "), "{o:?}");
    assert_eq!(o.stderr.lines().count(), 2, "{o:?}");
    assert_plain(&o.stderr);
    assert!(o.stdout.is_empty());
}

#[tokio::test]
async fn json_errors_have_code_message_hint_and_the_same_exit_code() {
    let h = home();
    let o = h
        .run(&["--embedded", "--json", "sb", "exec", "nope", "echo hi"])
        .await;
    assert_eq!(o.code, 3, "{o:?}");
    let v = o.json();
    assert_eq!(v["error"]["code"], "not_found");
    assert!(v["error"]["message"].as_str().unwrap().contains("nope"));
    assert!(v["error"]["hint"].as_str().unwrap().contains("cua sb ls"));
    assert!(v["error"].get("detail").is_none(), "{v}");
    assert!(o.stderr.is_empty(), "{o:?}");
    // With -v the raw text comes along.
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "-v",
            "sb",
            "exec",
            "nope",
            "echo hi",
        ])
        .await;
    let v = o.json();
    assert!(
        v["error"]["detail"].as_str().unwrap().contains("not found"),
        "{v}"
    );
}

#[tokio::test]
async fn the_layers_come_with_v_and_never_without() {
    let h = home();
    let plain = h
        .run(&["--embedded", "sb", "exec", "nope", "echo hi"])
        .await;
    let verbose = h
        .run(&["--embedded", "-v", "sb", "exec", "nope", "echo hi"])
        .await;
    assert_eq!(plain.code, verbose.code);
    assert!(!plain.stderr.contains("detail:"), "{plain:?}");
    let first_two: Vec<_> = verbose.stderr.lines().take(2).collect();
    assert_eq!(first_two, plain.stderr.lines().collect::<Vec<_>>());
    assert!(
        verbose.stderr.contains("\ndetail: not found: "),
        "{verbose:?}"
    );
}

#[tokio::test]
async fn colour_follows_the_terminal_rules_and_the_flag() {
    let h = home();
    // Piped: never colour, whatever NO_COLOR says.
    let o = h
        .run(&["--embedded", "sb", "exec", "nope", "echo hi"])
        .await;
    assert!(!o.stderr.contains('\x1b'), "{o:?}");
    // --color always forces it, NO_COLOR or not.
    let mut h2 = home();
    h2.set("NO_COLOR", "1");
    let o = h2
        .run(&[
            "--embedded",
            "--color",
            "always",
            "sb",
            "exec",
            "nope",
            "echo hi",
        ])
        .await;
    assert!(o.stderr.starts_with("\x1b[1;31merror:\x1b[0m "), "{o:?}");
    let o = h2
        .run(&[
            "--embedded",
            "--color",
            "never",
            "sb",
            "exec",
            "nope",
            "echo hi",
        ])
        .await;
    assert!(!o.stderr.contains('\x1b'), "{o:?}");
}

#[tokio::test]
async fn the_version_flag_is_capital_v_and_a_lone_small_v_still_works_for_now() {
    let h = home();
    let want = format!("cua {}\n", cua_sdk::VERSION);
    for flag in ["-V", "--version"] {
        let o = h.run(&[flag]).await;
        assert_eq!(
            (o.code, o.stdout.as_str()),
            (0, want.as_str()),
            "{flag}: {o:?}"
        );
        assert!(o.stderr.is_empty(), "{o:?}");
    }
    let o = h.run(&["-v"]).await;
    assert_eq!((o.code, o.stdout.as_str()), (0, want.as_str()), "{o:?}");
    assert!(o.stderr.contains("`cua -V`"), "{o:?}");
    // With anything else, -v is verbose: a command that succeeds is the same.
    let o = h.run(&["-v", "--version"]).await;
    assert_eq!(o.stdout, want, "{o:?}");
}

#[tokio::test]
async fn quiet_and_verbose_conflict_and_log_levels_parse() {
    let h = home();
    let o = h.run(&["-q", "-v", "sb", "ls"]).await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("cannot be used with"), "{o:?}");
    for args in [
        &["--log-level", "debug", "sb", "ls", "--local"][..],
        &["-vv", "sb", "ls", "--local"],
        &["-q", "sb", "ls", "--local"],
        &["--color", "never", "sb", "ls", "--local"],
    ] {
        let mut full = vec!["--embedded"];
        full.extend_from_slice(args);
        let o = h.run(&full).await;
        assert_eq!(o.code, 0, "{args:?}: {o:?}");
    }
    let o = h.run(&["--log-level", "loud", "sb", "ls"]).await;
    assert_eq!(o.code, 2);
    assert!(
        o.stderr
            .contains("[possible values: error, warn, info, debug, trace]"),
        "{o:?}"
    );
    // The environment variable is the same flag.
    let mut h = home();
    h.set("CUA_LOG_LEVEL", "debug");
    let o = h.run(&["--embedded", "sb", "ls", "--local"]).await;
    assert_eq!(o.code, 0, "{o:?}");
    h.set("CUA_LOG_LEVEL", "loud");
    let o = h.run(&["--embedded", "sb", "ls", "--local"]).await;
    assert_eq!(o.code, 2, "{o:?}");
}

/// The expert override still wins over every flag, and log lines go to
/// stderr without ANSI when piped.
#[tokio::test]
async fn cua_log_is_the_expert_override_and_logs_are_plain_when_piped() {
    let mut h = home();
    h.set("CUA_LOG", "cua_sandbox_core=trace");
    let o = h.run(&["--embedded", "-q", "sb", "ls", "--local"]).await;
    assert_eq!(o.code, 0, "{o:?}");
    assert!(!o.stderr.contains('\x1b'), "ANSI in piped logs: {o:?}");
    assert!(
        !o.stdout.contains("TRACE"),
        "logs never reach stdout: {o:?}"
    );
}

#[tokio::test]
async fn json_error_codes_are_documented_and_stable() {
    // A value the CLI refuses (exit 2) is an invalid_argument in JSON too.
    let h = home();
    let o = h
        .run(&[
            "--embedded",
            "--json",
            "sb",
            "create",
            "linux",
            "--memory",
            "lots",
        ])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    let v: Value = o.json();
    assert_eq!(v["error"]["code"], "invalid_argument", "{v}");
    assert!(v["error"]["hint"].is_string(), "{v}");
}

#[tokio::test]
async fn impossible_resources_are_refused_before_anything_starts() {
    let h = home();
    let o = h
        .run(&["--embedded", "sb", "create", "linux", "--cpu", "100000"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(
        o.stderr
            .starts_with("error: --cpu 100000 is more than this computer has ("),
        "{o:?}"
    );
    let o = h
        .run(&["--embedded", "sb", "create", "linux", "--memory", "4000GB"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.starts_with("error: "), "{o:?}");
}
