//! What the installed `pasta` (passt) supports, probed once.
//!
//! Since passt's October 2024 change "fwd: Direct inbound spliced forwards
//! to the guest's external address", a connection that arrives at a
//! forwarded *host loopback* address (`-t 127.0.0.1/PORT:PORT`) is
//! delivered to the namespace's public address, not its loopback, unless
//! `--host-lo-to-ns-lo` is given. Older passt (Ubuntu 24.04's) delivered it
//! to the namespace loopback and rejects the option. Every forward cua makes targets a listener on the namespace
//! loopback (QEMU's `hostfwd`, the microVM's `OUTPUT` DNAT on `127.0.0.1`),
//! so with a newer passt the option is required, and with an older one it
//! must not be passed (pasta would refuse to start).

//!
//! Testing: the unit tests below pin the probe against the captured help of
//! both generations. The Linux jobs that boot a VM (space-egress-nightly)
//! install Ubuntu's passt, the older kind. A job for the newer behaviour
//! needs the same steps with a passt built from the passt.top sources (or a
//! distribution that ships a recent one, such as Fedora or Arch) in place of
//! the `passt` package; the existing egress and guest-control tests then
//! cover it unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The option that keeps a host-loopback forward on the namespace loopback.
pub const HOST_LO_TO_NS_LO: &str = "--host-lo-to-ns-lo";

/// Whether `help` (the output of `pasta --help`) lists
/// [`HOST_LO_TO_NS_LO`] as an option.
pub fn help_lists_host_lo(help: &str) -> bool {
    help.lines().any(|l| {
        l.trim_start()
            .strip_prefix(HOST_LO_TO_NS_LO)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

/// Whether the `pasta` at `bin` takes [`HOST_LO_TO_NS_LO`], from its
/// `--help` (asked once per binary and remembered). A binary that cannot be
/// asked is taken not to: the older behaviour is the one that needs nothing.
pub fn supports_host_lo(bin: &Path) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(bin) {
        return *v;
    }
    let v = std::process::Command::new(bin)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| {
            help_lists_host_lo(&String::from_utf8_lossy(&o.stdout))
                || help_lists_host_lo(&String::from_utf8_lossy(&o.stderr))
        })
        .unwrap_or(false);
    cache.lock().unwrap().insert(bin.to_path_buf(), v);
    v
}

/// The arguments that keep host-loopback forwards on the namespace
/// loopback, when `bin` supports them.
pub fn host_lo_args(bin: &Path) -> Vec<String> {
    host_lo_args_for(supports_host_lo(bin))
}

fn host_lo_args_for(supported: bool) -> Vec<String> {
    if supported {
        vec![HOST_LO_TO_NS_LO.to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pasta --help` of passt 2026_10_02 (Fedora), trimmed to the lines
    /// around the option.
    const NEW_HELP: &str = "    SPEC is as described above\n    default: auto\n  -U, --udp-ns SPEC\tUDP port forwarding to init namespace\n    SPEC is as described above\n    default: auto\n  --host-lo-to-ns-lo\tTranslate host-loopback forwards to\n\t\t\tnamespace loopback\n  --userns NSPATH \tTarget user namespace to join\n  --netns PATH|NAME\tTarget network namespace to join\n  --netns-only\t\tDon't join existing user namespace\n    implied if PATH or NAME are given without --userns\n";
    /// `pasta --help` of Ubuntu 24.04's passt (git 20240220), same lines.
    const OLD_HELP: &str = "  -U, --udp-ns SPEC\tUDP port forwarding to init namespace\n    SPEC is as described above\n    default: auto\n  --userns NSPATH \tTarget user namespace to join\n  --netns PATH|NAME\tTarget network namespace to join\n  --netns-only\t\tDon't join existing user namespace\n    implied if PATH or NAME are given without --userns\n  --no-netns-quit\tDon't quit if filesystem-bound target\n  \t\t\tnetwork namespace is deleted\n";

    #[test]
    fn probe_parses_both_helps() {
        assert!(help_lists_host_lo(NEW_HELP));
        assert!(!help_lists_host_lo(OLD_HELP));
        assert!(!help_lists_host_lo(""));
        // A mention in another option's text is not the option.
        assert!(!help_lists_host_lo("  --other  see --host-lo-to-ns-lo\n"));
        assert!(!help_lists_host_lo("  --host-lo-to-ns-lo-x\tnope\n"));
    }

    #[test]
    fn args_follow_support() {
        assert_eq!(host_lo_args_for(true), ["--host-lo-to-ns-lo"]);
        assert!(host_lo_args_for(false).is_empty());
    }

    #[test]
    fn missing_binary_is_unsupported() {
        assert!(!supports_host_lo(Path::new("/nonexistent/pasta")));
    }
}
