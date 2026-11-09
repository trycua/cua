// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Error text the shells show. An action that fails because the local
//! `cua daemon` does not answer says so in plain words, never as a raw
//! transport error ("transport: Connection refused (os error 61)"), and
//! the SDK's internal error tags ("local runtime: ", "[local] ") never reach
//! a person.

/// What a failed action shows when the Cua daemon is not running. The SDK's
/// `CuaError::DaemonNotRunning` carries the same sentence.
pub const DAEMON_NOT_RUNNING: &str =
    "The Cua daemon isn't running. Start Cua, or run `cua daemon start`.";

/// The words a shell shows for a failed action's `raw` error: a failure to
/// reach the local daemon becomes [`DAEMON_NOT_RUNNING`]; everything else
/// is `raw`, trimmed. Only the daemon's own transport errors match (the
/// SDK's `transport: <io error>` and `daemon transport: ...` forms and the
/// daemon client's messages); a Space's own `transport error: ...` or a
/// bare "connection refused" from a remote machine stays as it is. The
/// SDK's error tags in front ([`INTERNAL_TAGS`]) are dropped and the
/// sentence starts with a capital, so "local runtime: This Mac can't run
/// Spaces itself, ..." reads as the sentence it carries (CUA-3193).
pub fn plain_error(raw: &str) -> String {
    let t = raw.trim();
    if is_daemon_unreachable(t) {
        return DAEMON_NOT_RUNNING.to_string();
    }
    let rest = without_tags(t);
    if rest.len() == t.len() {
        return t.to_string();
    }
    let mut chars = rest.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => t.to_string(),
    }
}

/// The SDK's error tags: `CuaError::Runtime`'s `local runtime: ` and a local
/// backend's `[local] `. Internal words, never what a person reads.
pub const INTERNAL_TAGS: &[&str] = &["local runtime: ", "[local] "];

/// `t` without the error tags in front of it (in any order, each once or more).
fn without_tags(t: &str) -> &str {
    let mut rest = t;
    loop {
        let before = rest.len();
        for tag in INTERNAL_TAGS {
            if rest
                .get(..tag.len())
                .is_some_and(|h| h.eq_ignore_ascii_case(tag))
            {
                rest = rest[tag.len()..].trim_start();
            }
        }
        if rest.len() == before {
            return rest;
        }
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
            "upstream: 503 Service Unavailable",
        ] {
            assert_eq!(plain_error(raw), raw, "{raw}");
        }
        assert_eq!(plain_error("  timed out \n"), "timed out");
    }

    /// CUA-3193: the "can't run Spaces" step read "local runtime: This Mac ...".
    #[test]
    fn the_sdk_error_tags_never_reach_a_person() {
        let mac = cua_create_failure::preflight::MAC_UNSUPPORTED;
        for raw in [
            format!("local runtime: {mac}"),
            format!("local runtime: [local] {mac}"),
            format!("  [local] {mac}"),
        ] {
            assert_eq!(plain_error(&raw), mac, "{raw}");
        }
        assert_eq!(
            plain_error(
                "local runtime: the built-in Linux runtime did not start: exiting, status 1"
            ),
            "The built-in Linux runtime did not start: exiting, status 1"
        );
        // A tag with nothing after it keeps its words rather than showing nothing.
        assert_eq!(plain_error("local runtime: "), "local runtime:");
        for tag in INTERNAL_TAGS {
            assert!(!plain_error(&format!("{tag}{tag}no socket")).contains(tag.trim()));
        }
    }
}
