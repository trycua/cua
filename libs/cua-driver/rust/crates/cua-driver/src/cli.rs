//! CLI compatibility adapter for PID-bound daemon shutdown.
//!
//! The historical CLI implementation stays byte-for-byte in `cli_base.rs`.
//! This adapter adds the release-uninstaller-only `--expected-pid` option to
//! the ordinary `Command::Stop` parse path without changing the daemon wire
//! protocol. The option deliberately supports `--expected-pid <pid> stop` so
//! older installed helpers fail closed instead of interpreting a plain stop.

#[path = "cli_base.rs"]
mod base;

pub use base::*;

use std::sync::atomic::{AtomicU32, Ordering};

static EXPECTED_STOP_PID: AtomicU32 = AtomicU32::new(0);

const VALUE_FLAGS: &[&str] = &[
    "--cursor-theme",
    "--cursor-reduced-motion",
    "--glide-ms",
    "--dwell-ms",
    "--idle-hide-ms",
    "--screenshot-out-file",
    "--client",
    "--socket",
    "--permission-mode",
    "--grant",
    "--session-policy",
    "--capability-manifest",
    "--pid-file",
    "--expected-pid",
    "--type",
    "--host-bundle-id",
    "--pid",
    "--strategy",
    "--window-id",
    "--session",
    "--profile-mode",
    "--profile-name",
    "--experimental-pip-geometry",
];

fn expected_pid_requested(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--expected-pid" || arg.starts_with("--expected-pid="))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(value) = arg
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value.to_owned());
        }
    }
    None
}

fn positional_args(args: &[String]) -> Vec<&str> {
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if VALUE_FLAGS.contains(&arg) {
            index += 2;
        } else if arg.starts_with('-') {
            index += 1;
        } else {
            positionals.push(arg);
            index += 1;
        }
    }
    positionals
}

fn expected_pid_stop_shape(args: &[String]) -> bool {
    expected_pid_requested(args) && positional_args(args).first().copied() == Some("stop")
}

pub fn expected_stop_pid() -> Option<u32> {
    match EXPECTED_STOP_PID.load(Ordering::Relaxed) {
        0 => None,
        pid => Some(pid),
    }
}

pub fn finite_command_name_from_argv() -> Option<&'static str> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return None;
    }
    if expected_pid_stop_shape(&args) {
        return Some("stop");
    }
    base::finite_command_name_from_argv()
}

pub fn finite_tool_name_from_argv() -> Option<String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if expected_pid_stop_shape(&args) {
        return None;
    }
    base::finite_tool_name_from_argv()
}

pub fn finite_computer_action_from_argv() -> bool {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if expected_pid_stop_shape(&args) {
        return false;
    }
    base::finite_computer_action_from_argv()
}

pub fn finite_operation_from_argv() -> &'static str {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if expected_pid_stop_shape(&args) {
        return "not_applicable";
    }
    base::finite_operation_from_argv()
}

pub fn finite_client_kind_from_argv() -> &'static str {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if expected_pid_stop_shape(&args) {
        return "not_applicable";
    }
    base::finite_client_kind_from_argv()
}

pub fn parse_command() -> Command {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    // Preserve the base parser's early help/version behavior even if someone
    // combines those flags with --expected-pid.
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return base::parse_command();
    }

    if !expected_pid_requested(&args) {
        return base::parse_command();
    }

    let raw_pid = flag_value(&args, "--expected-pid").unwrap_or_else(|| {
        eprintln!("--expected-pid requires a positive integer PID");
        std::process::exit(64);
    });
    let expected_pid = raw_pid
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .unwrap_or_else(|| {
            eprintln!("--expected-pid requires a positive integer PID");
            std::process::exit(64);
        });

    if positional_args(&args).first().copied() != Some("stop") {
        eprintln!("--expected-pid is valid only with `cua-driver stop`");
        std::process::exit(64);
    }

    EXPECTED_STOP_PID.store(expected_pid, Ordering::Relaxed);
    Command::Stop {
        socket: flag_value(&args, "--socket"),
    }
}

#[cfg(test)]
mod tests {
    use super::{expected_pid_stop_shape, positional_args};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn prefix_expected_pid_keeps_stop_as_the_command() {
        let argv = args(&["--expected-pid", "42", "stop"]);
        assert_eq!(positional_args(&argv), vec!["stop"]);
        assert!(expected_pid_stop_shape(&argv));
    }

    #[test]
    fn socket_and_expected_pid_can_precede_stop() {
        let argv = args(&[
            "--socket",
            "/tmp/cua.sock",
            "--expected-pid",
            "42",
            "stop",
        ]);
        assert_eq!(positional_args(&argv), vec!["stop"]);
        assert!(expected_pid_stop_shape(&argv));
    }

    #[test]
    fn expected_pid_is_not_a_general_global_option() {
        let argv = args(&["--expected-pid", "42", "status"]);
        assert_eq!(positional_args(&argv), vec!["status"]);
        assert!(!expected_pid_stop_shape(&argv));
    }
}
