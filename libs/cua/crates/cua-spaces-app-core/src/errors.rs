// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Error text the shells show. An action that fails because the local
//! `cua daemon` does not answer says so in plain words, never as a raw
//! transport error ("transport: Connection refused (os error 61)").

/// What a failed action shows when the Cua daemon is not running. The SDK's
/// `CuaError::DaemonNotRunning` carries the same sentence.
pub const DAEMON_NOT_RUNNING: &str =
    "The Cua daemon isn't running. Start Cua, or run `cua daemon start`.";

/// The words a shell shows for a failed action's `raw` error: a failure to
/// reach the local daemon becomes [`DAEMON_NOT_RUNNING`]; everything else
/// is `raw`, trimmed. Only the daemon's own transport errors match (the
/// SDK's `transport: <io error>` and `daemon transport: ...` forms and the
/// daemon client's messages); a Space's own `transport error: ...` or a
/// bare "connection refused" from a remote machine stays as it is.
pub fn plain_error(raw: &str) -> String {
    let t = raw.trim();
    if is_daemon_unreachable(t) {
        DAEMON_NOT_RUNNING.to_string()
    } else {
        t.to_string()
    }
}

fn is_daemon_unreachable(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    if l.contains("cua daemon isn't running")
        || l.contains("cannot reach the cua daemon")
        || l.contains("no cua daemon is running")
        || l.contains("the cua daemon is not running")
    {
        return true;
    }
    // The SDK's daemon transport forms: `transport: <io error>` (the
    // daemon client's connect failure) and `daemon transport: <...>`. A
    // Space's transport failure reads `transport: transport error: ...`.
    let rest = l
        .strip_prefix("daemon transport: ")
        .or_else(|| l.strip_prefix("transport: "));
    rest.is_some_and(|r| {
        !r.starts_with("transport error")
            && (r.contains("connection refused")
                || r.contains("os error 61")
                || r.contains("os error 111")
                || r.contains("no such file or directory"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_daemon_reads_in_plain_words() {
        for raw in [
            "transport: Connection refused (os error 61)",
            "transport: Connection refused (os error 111)",
            "transport: No such file or directory (os error 2)",
            "daemon transport: cannot reach the cua daemon: Connection refused (os error 61)",
            "cannot reach the cua daemon: tcp connect error",
            "the cua daemon is not running",
            DAEMON_NOT_RUNNING,
        ] {
            assert_eq!(plain_error(raw), DAEMON_NOT_RUNNING, "{raw}");
        }
    }

    #[test]
    fn other_errors_pass_through() {
        for raw in [
            "connection refused",
            "transport: transport error: Connection refused (os error 61)",
            "no macOS image is configured",
            "fleet: 503 Service Unavailable",
        ] {
            assert_eq!(plain_error(raw), raw, "{raw}");
        }
        assert_eq!(plain_error("  timed out \n"), "timed out");
    }
}
