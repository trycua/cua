// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who is on the other end of a keyvault socket.
//!
//! The identity comes from the kernel, never from what the client says:
//!
//! - macOS: `LOCAL_PEERTOKEN` yields the peer's audit token at connect time;
//!   `SecCodeCopyGuestWithAttributes(kSecGuestAttributeAudit)` turns it into
//!   a code object (the audit token carries the pid version, so a recycled
//!   pid or a post-connect `exec` does not match); `SecCodeCheckValidity`
//!   checks it against the Cua code requirement. Team id, signing id and
//!   cdhash come from the same code object.
//! - Linux: `SO_PEERCRED` pid and uid, then `/proc/<pid>/exe`. Same-user
//!   processes can ptrace each other there, so the identity is marked
//!   `os_verified: false` and third-party callers still need consent.
//! - Anything else: refused.
//!
//! What a client *claims* (`Hello.app_name`) is shown in the consent UI
//! only as an unverified hint next to the verified identity.

use serde::{Deserialize, Serialize};

use crate::crypto::sha256;

/// How the caller's code is signed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Signing {
    /// A valid signature from a certificate chain with a team id.
    Signed {
        /// Apple team id (`subject.OU` of the leaf).
        team_id: String,
        /// Signing identifier (`ai.cua.cli`).
        identifier: String,
        /// Code directory hash, hex.
        cdhash: String,
    },
    /// A valid signature without a team (ad hoc or self-signed).
    AdHoc {
        /// Signing identifier.
        identifier: String,
        /// Code directory hash, hex.
        cdhash: String,
    },
    /// Not signed, or the signature is invalid for the running code.
    Unsigned,
    /// Signature state unknown (platforms without code signing).
    Unknown,
}

/// A caller, as verified by the OS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerIdentity {
    /// Process id at connect time (display only; never an authority).
    pub pid: i32,
    /// Effective uid.
    pub uid: u32,
    /// Executable path, when known.
    pub path: Option<String>,
    /// Code signature.
    pub signing: Signing,
    /// The caller satisfied the first-party (Cua) code requirement.
    pub first_party: bool,
    /// The identity is backed by the OS code-signing root of trust
    /// (macOS audit token plus signature), not just a path.
    pub os_verified: bool,
    /// The parent process when the caller is the Cua CLI: whoever launched
    /// the CLI (a terminal, or another app driving it). Shown in consent
    /// prompts so a confused-deputy launch is visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_by: Option<String>,
    /// The legal entity Apple verified in the signing leaf certificate's
    /// subject common name (the Developer ID / notarization name, e.g.
    /// `Developer ID Application: Acme Inc (ABCDE12345)`). This, plus the team
    /// id, is the only attacker-*un*chosen identity string; the signing
    /// identifier and the claimed name are self-declared (red-team F4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_name: Option<String>,
}

/// Curated map of team ids to a human publisher name, for a "known publisher"
/// badge on the consent screen. A first-time or unknown team is shown as
/// unrecognized (red-team F4).
pub fn known_publisher(team_id: &str) -> Option<&'static str> {
    match team_id {
        CUA_TEAM_ID => Some("Cua (Trycua, Inc.)"),
        _ => None,
    }
}

impl CallerIdentity {
    /// A stable fingerprint of the verified identity. Grants and tokens bind
    /// to it.
    ///
    /// - Team-signed code: team id plus signing identifier, so an update of
    ///   the same app keeps its grants.
    /// - Ad hoc: identifier plus cdhash, so any rebuild loses its grants.
    /// - Unsigned or unknown: the executable path plus uid, hashed. Weak, so
    ///   such callers never get unattended rules by default.
    pub fn fingerprint(&self) -> String {
        let raw = match &self.signing {
            Signing::Signed {
                team_id,
                identifier,
                ..
            } => format!("team:{team_id}|id:{identifier}"),
            Signing::AdHoc { identifier, cdhash } => format!("adhoc:{identifier}|cd:{cdhash}"),
            Signing::Unsigned | Signing::Unknown => format!(
                "path:{}|uid:{}",
                self.path.as_deref().unwrap_or("?"),
                self.uid
            ),
        };
        format!("fp1-{}", &hex::encode(sha256(raw.as_bytes()))[..32])
    }

    /// A one-line label for consent prompts and the audit log. It leads with
    /// the OS-verified identity (the leaf certificate's legal name and the
    /// team id): the parts an attacker cannot choose. The signing identifier,
    /// which the signer picks freely, is shown parenthetically and never as
    /// the headline (red-team F4).
    pub fn display(&self) -> String {
        let who = match &self.signing {
            Signing::Signed {
                team_id,
                identifier,
                ..
            } => {
                let anchor = match (&self.verified_name, known_publisher(team_id)) {
                    (Some(name), _) => name.clone(),
                    (None, Some(pubname)) => pubname.to_string(),
                    (None, None) => format!("team {team_id}"),
                };
                format!("{anchor} (team {team_id}, signed id \"{identifier}\")")
            }
            Signing::AdHoc { identifier, .. } => {
                format!("{identifier} (ad hoc signature, UNVERIFIED)")
            }
            Signing::Unsigned => format!(
                "{} (UNSIGNED, UNTRUSTED)",
                self.path.as_deref().unwrap_or("unknown program")
            ),
            Signing::Unknown => format!(
                "{} (signature not checked on this OS, UNTRUSTED)",
                self.path.as_deref().unwrap_or("unknown program")
            ),
        };
        match (&self.launched_by, self.first_party) {
            (Some(parent), true) => format!("Cua: {who}, launched by {parent}"),
            (None, true) => format!("Cua: {who}"),
            (_, false) => who,
        }
    }

    /// The publisher name to badge, when the team is a known one (red-team F4).
    pub fn known_publisher(&self) -> Option<&'static str> {
        match &self.signing {
            Signing::Signed { team_id, .. } => known_publisher(team_id),
            _ => None,
        }
    }

    /// Whether this caller's identity is anchored in the OS root of trust with
    /// no self-declared string standing in for the verified one: a team-signed,
    /// OS-verified caller. Ad hoc, unsigned and `Unknown` callers are not, and
    /// the consent screen labels them untrusted (red-team F4).
    pub fn is_verified(&self) -> bool {
        self.os_verified && matches!(self.signing, Signing::Signed { .. })
    }

    /// The brand this caller's signing identifier impersonates, if it is a
    /// confusable of a known brand rather than the brand itself (red-team
    /// F4/F18). Warned about on the consent screen.
    pub fn impersonates(&self) -> Option<&'static str> {
        let id = match &self.signing {
            Signing::Signed { identifier, .. } | Signing::AdHoc { identifier, .. } => {
                identifier.as_str()
            }
            _ => return None,
        };
        crate::confusable::impersonated_brand(id)
    }

    /// A synthetic identity for tests and in-process use.
    pub fn for_tests(identifier: &str, first_party: bool) -> Self {
        Self {
            pid: 0,
            uid: 0,
            path: Some(format!("/test/{identifier}")),
            signing: Signing::Signed {
                team_id: if first_party {
                    "CUATEST01"
                } else {
                    "OTHER0001"
                }
                .into(),
                identifier: identifier.into(),
                cdhash: "00".repeat(20),
            },
            first_party,
            os_verified: true,
            launched_by: None,
            verified_name: None,
        }
    }
}

/// Which code counts as Cua.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustPolicy {
    /// macOS code requirement (csreq syntax) a first-party caller satisfies.
    pub macos_requirement: String,
    /// Require the hardened runtime flag on first-party callers (so they
    /// cannot be injected with `DYLD_INSERT_LIBRARIES` or a debugger).
    pub require_hardened_runtime: bool,
    /// Linux: executable paths that count as Cua.
    pub linux_first_party_exes: Vec<String>,
    /// Test policies are marked so the daemon can refuse them in release.
    pub is_test_policy: bool,
}

/// The Apple team that signs Cua releases.
pub const CUA_TEAM_ID: &str = "YCK386LBJ7";
/// Signing identifiers of first-party Cua binaries.
/// `com.trycua.cua` is the standalone CLI (and the Spaces app sidecar);
/// the Tauri Spaces app ships as `com.trycua.spaces.app` (pkg) with the
/// Tauri identifier `com.trycua.spaces.prototype`; the native SwiftUI
/// Spaces app (apps/cua-spaces-macos) is `com.trycua.spaces.macos`. Every
/// one of them is trusted only under [`TrustPolicy::production`]'s
/// requirement: Apple-anchored, team [`CUA_TEAM_ID`], hardened runtime.
pub const CUA_IDENTIFIERS: &[&str] = &[
    "com.trycua.cua",
    "com.trycua.spaces.app",
    "com.trycua.spaces.prototype",
    "com.trycua.spaces.macos",
];

impl TrustPolicy {
    /// Production: Apple-anchored Developer ID or App Store certificates
    /// from the Cua team, with a Cua signing identifier.
    pub fn production() -> Self {
        let ids = CUA_IDENTIFIERS
            .iter()
            .map(|i| format!("identifier \"{i}\""))
            .collect::<Vec<_>>()
            .join(" or ");
        Self {
            macos_requirement: format!(
                "anchor apple generic and certificate leaf[subject.OU] = \"{CUA_TEAM_ID}\" and ({ids})"
            ),
            require_hardened_runtime: true,
            linux_first_party_exes: Vec::new(),
            is_test_policy: false,
        }
    }

    /// A test policy: `requirement` is usually a leaf-certificate hash of a
    /// throwaway signing identity.
    pub fn for_tests(requirement: impl Into<String>) -> Self {
        Self {
            macos_requirement: requirement.into(),
            require_hardened_runtime: false,
            linux_first_party_exes: Vec::new(),
            is_test_policy: true,
        }
    }
}

/// Why a peer could not be identified.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The OS call failed.
    #[error("peer credentials unavailable: {0}")]
    Unavailable(String),
    /// Another user.
    #[error("peer runs as uid {peer}, not {ours}")]
    WrongUser {
        /// The peer's uid.
        peer: u32,
        /// Ours.
        ours: u32,
    },
    /// No peer verification on this OS.
    #[error("keyvault IPC is not supported on this OS yet")]
    Unsupported,
}

/// Identifies the process on the other end of a connected Unix socket.
#[cfg(unix)]
pub fn identify_peer(
    fd: std::os::fd::RawFd,
    policy: &TrustPolicy,
) -> Result<CallerIdentity, PeerError> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::identify_peer(fd, policy)
    }
    #[cfg(target_os = "linux")]
    {
        linux::identify_peer(fd, policy)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (fd, policy);
        Err(PeerError::Unsupported)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub fn identify_peer(
        fd: std::os::fd::RawFd,
        policy: &TrustPolicy,
    ) -> Result<CallerIdentity, PeerError> {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a connected socket owned by the caller; cred and len
        // are valid for writes of the sizes given.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(PeerError::Unavailable(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // SAFETY: getuid has no preconditions.
        let ours = unsafe { libc::getuid() };
        if cred.uid != ours {
            return Err(PeerError::WrongUser {
                peer: cred.uid,
                ours,
            });
        }
        // `SO_PEERCRED` reports the peer's pid/uid as of `connect(2)`, so a
        // later fork/exec by the same user cannot change who we attribute the
        // connection to (red-team F14, connect-time credentials).
        //
        // Pin the process with a pidfd where the kernel supports it: it refers
        // to that exact process, so a pid recycled after connect is caught (the
        // open fails). Best effort: old kernels lack the syscall.
        let _pidfd = PidFd::open(cred.pid);
        // The pid->exe mapping is advisory: `/proc/<pid>/exe` can change if the
        // process execs. Read it twice and only trust a stable answer, so an
        // exec race between the reads drops the first-party claim rather than
        // silently trusting a swapped image (red-team F14, TOCTOU).
        let read_exe = || {
            std::fs::read_link(format!("/proc/{}/exe", cred.pid))
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        };
        let path = match (read_exe(), read_exe()) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
        let first_party = path
            .as_ref()
            .is_some_and(|p| policy.linux_first_party_exes.iter().any(|x| x == p));
        Ok(CallerIdentity {
            pid: cred.pid,
            uid: cred.uid,
            path,
            signing: Signing::Unknown,
            first_party,
            os_verified: false,
            launched_by: None,
            verified_name: None,
        })
    }

    /// An owned pidfd (`pidfd_open(2)`), closed on drop. Best effort: `open`
    /// returns `None` on kernels without the syscall.
    struct PidFd(std::os::fd::RawFd);

    impl PidFd {
        fn open(pid: i32) -> Option<Self> {
            // SAFETY: pidfd_open(2) with flags 0 takes a pid and returns a new
            // fd (or -1); it shares no memory with us.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                None
            } else {
                Some(PidFd(fd as std::os::fd::RawFd))
            }
        }
    }

    impl Drop for PidFd {
        fn drop(&mut self) {
            // SAFETY: self.0 is a fd we own from pidfd_open.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_follow_the_signing_model() {
        let a = CallerIdentity::for_tests("com.example.koalabot", false);
        let mut updated = a.clone();
        if let Signing::Signed { cdhash, .. } = &mut updated.signing {
            *cdhash = "11".repeat(20);
        }
        updated.pid = 999;
        assert_eq!(
            a.fingerprint(),
            updated.fingerprint(),
            "team-signed updates keep grants"
        );

        let adhoc = |cd: &str| CallerIdentity {
            signing: Signing::AdHoc {
                identifier: "x".into(),
                cdhash: cd.into(),
            },
            ..a.clone()
        };
        assert_ne!(
            adhoc("aa").fingerprint(),
            adhoc("bb").fingerprint(),
            "ad hoc rebuilds lose grants"
        );
        let unsigned = CallerIdentity {
            signing: Signing::Unsigned,
            ..a.clone()
        };
        assert!(unsigned.display().contains("UNSIGNED"));
        assert_ne!(unsigned.fingerprint(), a.fingerprint());
    }

    #[test]
    fn consent_display_leads_with_the_verified_identity_not_the_signing_id() {
        // Red-team F4: an attacker registers a team and signs an app whose
        // signing identifier is "com.google.Chrome". The identifier must never
        // be the headline; the OS-verified leaf name and team id are.
        let spoof = CallerIdentity {
            signing: Signing::Signed {
                team_id: "EVIL999999".into(),
                identifier: "com.google.Chrome".into(),
                cdhash: "00".repeat(20),
            },
            verified_name: Some("Developer ID Application: Mallory LLC (EVIL999999)".into()),
            ..CallerIdentity::for_tests("com.google.Chrome", false)
        };
        let shown = spoof.display();
        assert!(
            shown.starts_with("Developer ID Application: Mallory LLC"),
            "{shown}"
        );
        assert!(shown.contains("EVIL999999"), "{shown}");
        // The self-declared identifier appears only as a quoted, clearly-labeled
        // "signed id", never as the authority.
        assert!(shown.contains("signed id \"com.google.Chrome\""), "{shown}");
        assert!(!shown.starts_with("com.google.Chrome"), "{shown}");
        assert!(spoof.known_publisher().is_none());
        assert!(spoof.is_verified());

        // A known publisher is badged by its curated name.
        let cua = CallerIdentity::for_tests("com.trycua.cua", true);
        let cua = CallerIdentity {
            signing: Signing::Signed {
                team_id: CUA_TEAM_ID.into(),
                identifier: "com.trycua.cua".into(),
                cdhash: "00".repeat(20),
            },
            ..cua
        };
        assert_eq!(cua.known_publisher(), Some("Cua (Trycua, Inc.)"));
    }

    #[test]
    fn unverified_callers_are_labeled_untrusted_and_not_verified() {
        // Red-team F4/F14: unsigned, ad hoc and `Unknown` (Linux, other OS)
        // callers carry no OS-anchored identity, so they are untrusted.
        for signing in [
            Signing::Unsigned,
            Signing::Unknown,
            Signing::AdHoc {
                identifier: "x".into(),
                cdhash: "y".into(),
            },
        ] {
            let c = CallerIdentity {
                signing,
                ..CallerIdentity::for_tests("x", false)
            };
            assert!(!c.is_verified(), "{}", c.display());
            assert!(
                c.display().to_uppercase().contains("UNTRUSTED")
                    || c.display().contains("UNVERIFIED"),
                "{}",
                c.display()
            );
        }
    }

    #[test]
    fn a_homoglyph_signing_identifier_is_flagged_as_impersonation() {
        // Red-team F4/F18: a Cyrillic-i "g\u{0456}thub.com" identifier.
        let c = CallerIdentity {
            signing: Signing::Signed {
                team_id: "EVIL999999".into(),
                identifier: "g\u{0456}thub.com".into(),
                cdhash: "00".repeat(20),
            },
            ..CallerIdentity::for_tests("x", false)
        };
        assert_eq!(c.impersonates(), Some("github.com"));
        // The genuine identifier impersonates nothing.
        let ok = CallerIdentity::for_tests("com.trycua.cua", true);
        assert_eq!(ok.impersonates(), None);
    }

    #[test]
    fn production_policy_pins_team_and_identifiers() {
        let p = TrustPolicy::production();
        assert!(p.macos_requirement.starts_with("anchor apple generic"));
        assert!(p.macos_requirement.contains(CUA_TEAM_ID));
        assert!(
            p.macos_requirement
                .contains("identifier \"com.trycua.cua\"")
        );
        assert!(p.require_hardened_runtime && !p.is_test_policy);
    }

    #[test]
    fn production_policy_trusts_the_swiftui_app_under_the_same_team() {
        let p = TrustPolicy::production();
        let req = &p.macos_requirement;
        // One anchor and team clause guards every identifier, the SwiftUI
        // app's included: `anchor ... and OU = team and (id or id ...)`.
        let (prefix, ids) = req.split_once(" and (").expect("identifier group");
        assert_eq!(
            prefix,
            format!("anchor apple generic and certificate leaf[subject.OU] = \"{CUA_TEAM_ID}\"")
        );
        assert!(ids.ends_with(')') && !ids[..ids.len() - 1].contains(')'));
        for id in [
            "com.trycua.spaces.macos",
            "com.trycua.spaces.prototype",
            "com.trycua.cua",
        ] {
            assert!(ids.contains(&format!("identifier \"{id}\"")), "{id}: {req}");
        }
        assert!(p.require_hardened_runtime);
    }
}
