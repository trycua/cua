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
//!   `os_verified: false` and third-party callers still need consent. Cua
//!   itself is recognised by where it is installed: an executable inside a
//!   Cua install root (`/opt/Cua Spaces`, the `.deb`'s) that no one but root
//!   can change ([`TrustPolicy::linux_first_party_roots`]).
//! - Windows: the named pipe's client process id, then its image file's
//!   Authenticode signature ([`crate::winpeer`]). Cua is the code signed by
//!   the same publisher as this process ([`TrustPolicy::windows_publisher`]).
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
    /// Linux: install roots whose executables count as Cua, when every
    /// directory from the root up is owned by root and closed to everyone
    /// else (a same-user process cannot put its own binary there).
    pub linux_first_party_roots: Vec<String>,
    /// Windows: the publisher (the signing certificate's subject) that counts
    /// as Cua. `None`: the publisher that signed this process, which is how a
    /// signed daemon recognises the signed app and the other way round.
    pub windows_publisher: Option<String>,
    /// Test policies are marked so the daemon can refuse them in release.
    pub is_test_policy: bool,
}

/// The Apple team that signs Cua releases.
pub const CUA_TEAM_ID: &str = "YCK386LBJ7";
/// Where the Linux packages install Cua (the `.deb`'s `/opt/<product name>`,
/// and the distro-style locations): only executables inside one of these, in
/// a tree no one but root can change, count as Cua on Linux.
pub const LINUX_INSTALL_ROOTS: &[&str] = &[
    "/opt/Cua Spaces",
    "/usr/lib/cua-spaces",
    "/usr/local/lib/cua-spaces",
];
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
            linux_first_party_roots: LINUX_INSTALL_ROOTS.iter().map(|r| r.to_string()).collect(),
            windows_publisher: None,
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
            linux_first_party_roots: Vec::new(),
            windows_publisher: None,
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
    /// A Windows peer's process could not be inspected.
    #[error("peer process could not be inspected: {0}")]
    Inspect(String),
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

/// What the Authenticode check found out about a Windows executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsSigner {
    /// The signing certificate's subject as the system shows it: the
    /// publisher a UAC prompt names.
    pub subject: String,
    /// The signing certificate's SHA-1 thumbprint, hex (shown, never compared:
    /// short-lived signing certificates rotate under the same subject).
    pub thumbprint: String,
}

/// A Windows peer's signing, judged against `policy`: how to record it, and
/// whether it is Cua. `peer` is the signer of the peer's executable (`None`:
/// unsigned, or the signature does not chain to a trusted root); `own` the
/// signer of this process. Cua is the publisher the policy names or, with
/// none named, the one that signed this process, so a signed daemon and a
/// signed app recognise each other and an unsigned build recognises no one.
pub fn windows_signing(
    peer: Option<&WindowsSigner>,
    own: Option<&WindowsSigner>,
    file_name: &str,
    policy: &TrustPolicy,
) -> (Signing, bool) {
    let Some(peer) = peer else {
        return (Signing::Unsigned, false);
    };
    let first_party = match &policy.windows_publisher {
        Some(publisher) => *publisher == peer.subject,
        None => own.is_some_and(|own| own.subject == peer.subject),
    };
    let signing = Signing::Signed {
        team_id: peer.subject.clone(),
        identifier: file_name.to_string(),
        cdhash: peer.thumbprint.clone(),
    };
    (signing, first_party)
}

/// Whether the executable `exe` (the target of `/proc/<pid>/exe`) counts as
/// Cua on Linux under `policy`: a path the policy lists, or a file inside one
/// of its install roots where `protected` says no one but root could have put
/// it (see [`TrustPolicy::linux_first_party_roots`]). A file that was deleted
/// or replaced since it started never counts.
pub fn linux_first_party(
    exe: &str,
    policy: &TrustPolicy,
    protected: impl Fn(&std::path::Path) -> bool,
) -> bool {
    use std::path::{Component, Path};
    if exe.ends_with(" (deleted)") {
        return false;
    }
    if policy.linux_first_party_exes.iter().any(|x| x == exe) {
        return true;
    }
    let path = Path::new(exe);
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    policy
        .linux_first_party_roots
        .iter()
        .any(|root| path.starts_with(root) && path != Path::new(root) && protected(path))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// Whether every part of `path` (the file and each directory above it) is a
    /// real file or directory owned by root that only root can write to. A path
    /// like that cannot hold a binary a same-user process put there.
    pub fn root_protected(path: &std::path::Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        path.ancestors().all(|p| {
            std::fs::symlink_metadata(p).is_ok_and(|md| {
                !md.file_type().is_symlink() && md.uid() == 0 && md.mode() & 0o022 == 0
            })
        })
    }

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
            .as_deref()
            .is_some_and(|p| linux_first_party(p, policy, root_protected));
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

    fn signer(subject: &str, thumbprint: &str) -> WindowsSigner {
        WindowsSigner {
            subject: subject.into(),
            thumbprint: thumbprint.into(),
        }
    }

    #[test]
    fn windows_recognises_the_publisher_that_signed_this_process() {
        let p = TrustPolicy::production();
        let cua = signer("Cua AI, Inc.", "aa");
        // The same publisher under a rotated certificate is still Cua.
        let (signing, first_party) = windows_signing(
            Some(&signer("Cua AI, Inc.", "bb")),
            Some(&cua),
            "Cua Spaces.exe",
            &p,
        );
        assert!(first_party);
        assert_eq!(
            signing,
            Signing::Signed {
                team_id: "Cua AI, Inc.".into(),
                identifier: "Cua Spaces.exe".into(),
                cdhash: "bb".into()
            }
        );
        // Another publisher, an unsigned peer, and an unsigned self recognise no one.
        assert!(!windows_signing(Some(&signer("Mallory LLC", "cc")), Some(&cua), "x.exe", &p).1);
        assert_eq!(
            windows_signing(None, Some(&cua), "x.exe", &p),
            (Signing::Unsigned, false)
        );
        assert!(!windows_signing(Some(&cua), None, "cua.exe", &p).1);
    }

    #[test]
    fn windows_can_pin_a_publisher_by_name() {
        let p = TrustPolicy {
            windows_publisher: Some("Cua AI, Inc.".into()),
            ..TrustPolicy::production()
        };
        // The name decides, whoever this process is signed by.
        assert!(windows_signing(Some(&signer("Cua AI, Inc.", "aa")), None, "cua.exe", &p).1);
        assert!(
            !windows_signing(
                Some(&signer("Mallory LLC", "bb")),
                Some(&signer("Mallory LLC", "bb")),
                "cua.exe",
                &p
            )
            .1
        );
    }

    #[test]
    fn a_windows_signer_is_a_publisher_not_a_team() {
        // Grants bind to the publisher and the file name, so an update keeps them.
        let (a, _) = windows_signing(
            Some(&signer("Cua AI, Inc.", "aa")),
            None,
            "Cua Spaces.exe",
            &TrustPolicy::production(),
        );
        let (b, _) = windows_signing(
            Some(&signer("Cua AI, Inc.", "zz")),
            None,
            "Cua Spaces.exe",
            &TrustPolicy::production(),
        );
        let id = |signing| CallerIdentity {
            signing,
            ..CallerIdentity::for_tests("x", false)
        };
        assert_eq!(id(a).fingerprint(), id(b).fingerprint());
    }

    fn linux_policy() -> TrustPolicy {
        TrustPolicy {
            linux_first_party_exes: vec!["/usr/bin/cua".into()],
            ..TrustPolicy::production()
        }
    }

    #[test]
    fn linux_trusts_the_install_roots_only_where_root_owns_the_tree() {
        let p = linux_policy();
        let root_only = |_: &std::path::Path| true;
        let user_writable = |_: &std::path::Path| false;
        // The deb's layout: the app and the daemon beside it.
        assert!(linux_first_party(
            "/opt/Cua Spaces/cua-spaces",
            &p,
            root_only
        ));
        assert!(linux_first_party(
            "/opt/Cua Spaces/resources/native/cua",
            &p,
            root_only
        ));
        // The same path in a tree a user could change is not Cua.
        assert!(!linux_first_party(
            "/opt/Cua Spaces/cua-spaces",
            &p,
            user_writable
        ));
        // Listed paths are trusted as listed.
        assert!(linux_first_party("/usr/bin/cua", &p, user_writable));
        // Anywhere else, never: a downloaded AppImage, a build, /tmp.
        for exe in [
            "/tmp/.mount_CuaSpa1/cua-spaces",
            "/home/ada/cua/target/debug/cua",
            "/opt/Cua Spaces",
            "/opt/Cua Spaces2/x",
            "/usr/bin/other",
        ] {
            assert!(!linux_first_party(exe, &p, root_only), "{exe}");
        }
    }

    #[test]
    fn linux_refuses_a_replaced_binary_and_a_path_that_climbs_out() {
        let p = linux_policy();
        let root_only = |_: &std::path::Path| true;
        assert!(!linux_first_party(
            "/opt/Cua Spaces/cua-spaces (deleted)",
            &p,
            root_only
        ));
        assert!(!linux_first_party(
            "/opt/Cua Spaces/../../tmp/evil",
            &p,
            root_only
        ));
        assert!(!linux_first_party(
            "opt/Cua Spaces/cua-spaces",
            &p,
            root_only
        ));
        // A test policy lists no roots: nothing but its exact paths count.
        assert!(!linux_first_party(
            "/opt/Cua Spaces/cua-spaces",
            &TrustPolicy::for_tests("x"),
            root_only
        ));
    }

    #[test]
    fn production_policy_names_the_linux_install_roots_and_leaves_windows_to_the_signer() {
        let p = TrustPolicy::production();
        assert_eq!(p.linux_first_party_roots, LINUX_INSTALL_ROOTS);
        assert!(p.linux_first_party_exes.is_empty());
        assert_eq!(p.windows_publisher, None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tree_the_user_owns_is_not_root_protected() {
        // SAFETY: geteuid has no preconditions. (Root owns its own files.)
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("cua");
        std::fs::write(&f, b"x").unwrap();
        assert!(
            !linux::root_protected(&f),
            "the test user owns its own temp files"
        );
        assert!(
            !linux::root_protected(std::path::Path::new("/proc/self/exe")),
            "a symlink is never protected"
        );
    }
}
