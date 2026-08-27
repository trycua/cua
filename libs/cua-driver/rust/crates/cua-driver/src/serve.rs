//! Daemon transport adapter.
//!
//! All ordinary serve/client behavior is re-exported unchanged from
//! `serve_base.rs`. Only the public stop command is specialized when the CLI
//! parser supplied `--expected-pid`: metadata validation and shutdown then run
//! on one connection through `stop.rs`, closing the socket-replacement race.

#[path = "serve_base.rs"]
mod base;

pub use base::*;

#[path = "stop.rs"]
mod pid_bound_stop;

pub fn run_stop_cmd(socket_path: &str) {
    if let Some(expected_pid) = crate::cli::expected_stop_pid() {
        pid_bound_stop::run_pid_bound_stop_cmd(socket_path, expected_pid);
    } else {
        base::run_stop_cmd(socket_path);
    }
}
