//! What an agent may do only with the user's approval.
//!
//! A [`Policy`] says, per [`Cap`] (capability), whether a call needs the
//! user's fingerprint (Touch ID, or the login password) before it runs. The
//! MCP server (`mcp::gate`) reads it on every call, so a change applies at
//! once, and asks an [`Approver`]. The Cua Spaces app edits it in Settings.
//!
//! Safe by default: the capabilities that can spend money or carry the
//! user's secrets out (the cloud, sensitive files, API keys) need approval
//! until the user turns them off; the rest start off. Changing the policy needs the user
//! too: [`Policy::set`] takes an [`Approver`] and writes nothing unless it
//! confirms, so there is no path from a tool call to a loosened setting.
//!
//! The policy file is sealed. `approvals.json` carries an HMAC-SHA256 over
//! the settings and a generation number, keyed by a secret the OS keeps for
//! the signed Cua apps and daemon (the macOS Keychain, an item whose access
//! list names only them; see [`PolicySeal`]). A process of the same user can
//! rewrite the file, but not the secret, so a file that does not verify, an
//! older sealed copy, a deleted file, or a file nothing vouches for is
//! ignored: every capability asks until the user reviews the settings in the
//! app ([`Loaded::notice`] says why). A build with no seal cannot vouch for
//! any file, so it runs the fresh-install defaults and cannot change them.
//!
//! Some gates are not settings: sharing a Space, volume grants, computer
//! access grants, teleport and site logins always need the user (they are
//! listed by [`always`] so the app can show them), and the tools that
//! approve a request or edit access are not offered to agents at all.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A capability an agent can be gated on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cap {
    /// Create or sweep Spaces in the Cua cloud or the user's cloud account
    /// (it costs money), and connect a cloud account.
    Cloud,
    /// Add a machine or attach a Space to the account relay.
    Machines,
    /// Run commands in, read files from, or write files to the user's own
    /// machines (a `relay:` or `direct:` Space).
    RemoteExec,
    /// Send sensitive files from this Mac into a Space (SSH keys, cloud
    /// credentials, browser profiles, the Cua home).
    HostFiles,
    /// Forward the user's provider API keys into a Space.
    ApiKeys,
    /// Schedule routines and start autonomous runs that outlive the session.
    Routines,
    /// Change where the Cua Volume is stored, or mount it.
    Storage,
    /// Share this Mac's network with a Space.
    Network,
    /// Draw on the user's screen (viewer, picture in picture, window).
    Display,
}

impl Cap {
    /// Every capability, in the order the app lists them.
    pub const ALL: [Cap; 9] = [
        Cap::Cloud,
        Cap::Machines,
        Cap::RemoteExec,
        Cap::HostFiles,
        Cap::ApiKeys,
        Cap::Routines,
        Cap::Storage,
        Cap::Network,
        Cap::Display,
    ];

    /// The stable id (the JSON key, and the row id in the app).
    pub fn id(self) -> &'static str {
        match self {
            Cap::Cloud => "cloud",
            Cap::Machines => "machines",
            Cap::RemoteExec => "remote_exec",
            Cap::HostFiles => "host_files",
            Cap::ApiKeys => "api_keys",
            Cap::Routines => "routines",
            Cap::Storage => "storage",
            Cap::Network => "network",
            Cap::Display => "display",
        }
    }

    /// The capability with this id.
    pub fn from_id(id: &str) -> Option<Cap> {
        Cap::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The row title.
    pub fn title(self) -> &'static str {
        match self {
            Cap::Cloud => "Use the cloud",
            Cap::Machines => "Add machines",
            Cap::RemoteExec => "Control your machines",
            Cap::HostFiles => "Send sensitive files",
            Cap::ApiKeys => "Share API keys",
            Cap::Routines => "Run on a schedule",
            Cap::Storage => "Change Volume storage",
            Cap::Network => "Share your network",
            Cap::Display => "Show things on your screen",
        }
    }

    /// One line under the title.
    pub fn detail(self) -> &'static str {
        match self {
            Cap::Cloud => "Create or delete cloud Spaces and connect cloud accounts.",
            Cap::Machines => "Add a machine by address or attach a Space to your account.",
            Cap::RemoteExec => {
                "Run commands and move files on a Mac mini or other machine of yours."
            }
            Cap::HostFiles => "SSH keys, cloud credentials, browser profiles and Cua settings.",
            Cap::ApiKeys => "Forward your provider keys into a Space.",
            Cap::Routines => "Schedule routines that keep running after the chat ends.",
            Cap::Storage => "Point the Volume at another bucket, or mount it.",
            Cap::Network => "Let a Space use this Mac's network.",
            Cap::Display => "Open the viewer or a floating window on this Mac.",
        }
    }

    /// Whether a call needs the user's fingerprint on a fresh install, until
    /// the user says otherwise. Only the capabilities that spend money or
    /// carry secrets out ask by default; the rest are the user's own machines
    /// and screen and are on by choice.
    pub fn default_required(self) -> bool {
        matches!(self, Cap::Cloud | Cap::HostFiles | Cap::ApiKeys)
    }
}

/// A gate that is not a setting: it always needs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Always {
    /// The row title.
    pub title: &'static str,
    /// One line under the title.
    pub detail: &'static str,
}

/// The gates the user cannot turn off, for the app to show.
pub fn always() -> Vec<Always> {
    vec![
        Always {
            title: "Sign in with saved passwords",
            detail: "Keyvault asks you each time.",
        },
        Always {
            title: "Copy a signed-in app or browser",
            detail: "Teleport asks you each time.",
        },
        Always {
            title: "Share a Space",
            detail: "Sharing asks you each time.",
        },
        Always {
            title: "Open the Volume to an agent",
            detail: "Only you approve access requests.",
        },
        Always {
            title: "Let an agent use a machine",
            detail: "Only you grant computer access.",
        },
    ]
}

/// Confirms an action with the user (Touch ID or the login password).
pub trait Approver: Send + Sync {
    /// `Ok` when the user confirmed; `Err(why)` when they declined, or
    /// nothing here can ask. `reason` is shown in the prompt.
    fn confirm(&self, reason: &str) -> Result<(), String>;
}

/// An approver that never confirms (no prompt is available).
pub struct NeverApprove;

impl Approver for NeverApprove {
    fn confirm(&self, _reason: &str) -> Result<(), String> {
        Err("nothing here can ask you to confirm; approve it in the Cua app".into())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    version: u32,
    /// Capability id to whether it needs approval.
    #[serde(default)]
    require: BTreeMap<String, bool>,
    /// Bumped on every change; the seal remembers the latest, so an older
    /// sealed copy of the file does not verify.
    #[serde(default)]
    generation: u64,
    /// Hex HMAC-SHA256 of the settings and the generation.
    #[serde(default)]
    mac: String,
}

/// The secret that vouches for the policy file of one cua home, and the
/// newest generation stored with it.
#[derive(Clone, PartialEq, Eq)]
pub struct SealState {
    /// The HMAC key.
    pub key: [u8; 32],
    /// The generation of the last stored policy.
    pub generation: u64,
}

impl std::fmt::Debug for SealState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealState")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Keeps the [`SealState`] where a process that only shares the user cannot
/// reach it. On macOS the Cua Spaces build stores it in a Keychain item whose
/// access list names the signed Cua apps and daemon; any other reader would
/// make the system ask the user.
pub trait PolicySeal: Send + Sync {
    /// The state for `home`: `None` when no policy was ever stored there.
    fn load(&self, home: &Path) -> Result<Option<SealState>, String>;
    /// Stores `state` for `home` (creating the secret the first time).
    fn save(&self, home: &Path, state: &SealState) -> Result<(), String>;
}

/// A seal that lives in memory: fixtures and tests only (nothing outside the
/// process vouches for anything).
#[derive(Default)]
pub struct MemorySeal(Mutex<BTreeMap<PathBuf, SealState>>);

impl PolicySeal for MemorySeal {
    fn load(&self, home: &Path) -> Result<Option<SealState>, String> {
        Ok(self.0.lock().unwrap().get(home).cloned())
    }
    fn save(&self, home: &Path, state: &SealState) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(home.to_path_buf(), state.clone());
        Ok(())
    }
}

static SEAL: OnceLock<Arc<dyn PolicySeal>> = OnceLock::new();

/// Registers the process's seal (once, at startup, by the build that has
/// one: Cua Spaces).
pub fn register_seal(seal: Arc<dyn PolicySeal>) {
    let _ = SEAL.set(seal);
}

/// The registered seal, if any.
pub fn registered_seal() -> Option<Arc<dyn PolicySeal>> {
    SEAL.get().cloned()
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    k[..key.len().min(64)].copy_from_slice(&key[..key.len().min(64)]);
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What the MAC covers: every capability's setting, in a fixed order, and
/// the generation. Unknown keys in the file are not covered (and ignored).
fn mac_of(key: &[u8; 32], policy: &Policy, generation: u64) -> String {
    let mut msg = format!("cua-approvals-v2\ngeneration={generation}\n");
    for c in Cap::ALL {
        msg.push_str(&format!("{}={}\n", c.id(), u8::from(policy.requires(c))));
    }
    hex::encode(hmac_sha256(key, msg.as_bytes()))
}

/// A policy and, when the file could not be trusted, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// The policy to enforce.
    pub policy: Policy,
    /// Set when the settings were not trusted and everything asks; the app
    /// shows it.
    pub notice: Option<String>,
}

const TAMPERED: &str = "Your permission settings were changed outside Cua, or could not be verified, so every action asks until you review them here.";

/// The approval policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    require: BTreeMap<Cap, bool>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            require: Cap::ALL
                .into_iter()
                .map(|c| (c, c.default_required()))
                .collect(),
        }
    }
}

/// One row of the Settings list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    /// [`Cap::id`].
    pub id: &'static str,
    /// [`Cap::title`].
    pub title: &'static str,
    /// [`Cap::detail`].
    pub detail: &'static str,
    /// Whether a fingerprint is needed.
    pub require: bool,
    /// The default, so the app can offer "Restore defaults".
    pub default: bool,
}

/// Where the policy lives for the cua home `home`.
pub fn path_in(home: &Path) -> PathBuf {
    home.join("approvals.json")
}

impl Policy {
    /// The policy that asks for everything: what runs when the stored one
    /// cannot be trusted.
    pub fn strict() -> Policy {
        Policy {
            require: Cap::ALL.into_iter().map(|c| (c, true)).collect(),
        }
    }

    /// The policy stored in `home` and verified by the registered seal (see
    /// [`Policy::load_with`]), without the notice.
    pub fn load(home: &Path) -> Policy {
        Policy::load_with(home, registered_seal().as_deref()).policy
    }

    /// The policy stored in `home`, as `seal` vouches for it.
    ///
    /// - Nothing stored (no file, and `seal` has no secret for `home`): the
    ///   fresh-install defaults.
    /// - A file whose MAC verifies and whose generation is not older than the
    ///   seal's: its settings.
    /// - Anything else (a file that does not verify, an older copy, a file
    ///   that vanished after one was stored, an unreadable secret): every
    ///   capability asks, with a notice. It never loosens by failing.
    /// - No `seal` (a build that cannot vouch for a file): the defaults.
    pub fn load_with(home: &Path, seal: Option<&dyn PolicySeal>) -> Loaded {
        let ok = |policy| Loaded {
            policy,
            notice: None,
        };
        let tampered = || Loaded {
            policy: Policy::strict(),
            notice: Some(TAMPERED.to_string()),
        };
        let Some(seal) = seal else {
            return ok(Policy::default());
        };
        let Ok(state) = seal.load(home) else {
            return tampered();
        };
        let text = match std::fs::read_to_string(path_in(home)) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return tampered(),
        };
        let (Some(text), Some(state)) = (text.as_deref(), state.as_ref()) else {
            // Nothing stored anywhere is a fresh install; a file with no
            // secret behind it, or a secret with no file, is not.
            return if text.is_none() && state.is_none() {
                ok(Policy::default())
            } else {
                tampered()
            };
        };
        let Ok(file) = serde_json::from_str::<File>(&text) else {
            return tampered();
        };
        let mut p = Policy::default();
        for (id, v) in &file.require {
            if let Some(c) = Cap::from_id(id) {
                p.require.insert(c, *v);
            }
        }
        if file.generation < state.generation
            || !constant_time_eq(
                mac_of(&state.key, &p, file.generation).as_bytes(),
                file.mac.as_bytes(),
            )
        {
            return tampered();
        }
        ok(p)
    }

    /// Whether `cap` needs the user's fingerprint.
    pub fn requires(&self, cap: Cap) -> bool {
        self.require.get(&cap).copied().unwrap_or(true)
    }

    /// The rows for the Settings list.
    pub fn rows(&self) -> Vec<Row> {
        Cap::ALL
            .into_iter()
            .map(|c| Row {
                id: c.id(),
                title: c.title(),
                detail: c.detail(),
                require: self.requires(c),
                default: c.default_required(),
            })
            .collect()
    }

    /// Changes one setting and stores it in `home`, after `approver`
    /// confirms: turning a gate off and turning it on both need the user.
    /// Uses the registered seal.
    pub fn set(
        home: &Path,
        cap: Cap,
        require: bool,
        approver: &dyn Approver,
    ) -> Result<Policy, String> {
        Policy::set_with(home, cap, require, approver, registered_seal().as_deref())
    }

    /// [`Policy::set`] with an explicit seal. Storing needs one: a build
    /// without a seal cannot protect the file, so it refuses. A policy that
    /// could not be trusted is first replaced by the strict one, so the
    /// settings the user did not touch stay on.
    pub fn set_with(
        home: &Path,
        cap: Cap,
        require: bool,
        approver: &dyn Approver,
        seal: Option<&dyn PolicySeal>,
    ) -> Result<Policy, String> {
        let seal = seal.ok_or("this build cannot protect the permission settings")?;
        let loaded = Policy::load_with(home, Some(seal));
        let mut p = loaded.policy;
        if p.requires(cap) == require && loaded.notice.is_none() {
            return Ok(p);
        }
        approver.confirm(&format!(
            "Change \"{}\" to {}",
            cap.title(),
            if require {
                "ask for approval"
            } else {
                "run without asking"
            }
        ))?;
        p.require.insert(cap, require);
        p.store(home, seal)?;
        Ok(p)
    }

    /// Stores `overrides` on top of the defaults without asking anyone:
    /// fixtures and tests that need a sealed policy file.
    pub fn store_unprompted(
        home: &Path,
        overrides: &[(Cap, bool)],
        seal: &dyn PolicySeal,
    ) -> Result<Policy, String> {
        let mut p = Policy::load_with(home, Some(seal)).policy;
        for (c, v) in overrides {
            p.require.insert(*c, *v);
        }
        p.store(home, seal)?;
        Ok(p)
    }

    /// Stores the policy: bumps the generation in the seal first (so a
    /// failure after it leaves the file looking old, which is safe), then
    /// writes the file with its MAC.
    fn store(&self, home: &Path, seal: &dyn PolicySeal) -> Result<(), String> {
        let prev = seal.load(home)?;
        let mut state = match prev {
            Some(s) => s,
            None => {
                let mut key = [0u8; 32];
                rand::fill(&mut key);
                SealState { key, generation: 0 }
            }
        };
        state.generation += 1;
        seal.save(home, &state)?;
        let file = File {
            version: 2,
            require: self
                .require
                .iter()
                .map(|(c, v)| (c.id().to_string(), *v))
                .collect(),
            generation: state.generation,
            mac: mac_of(&state.key, self, state.generation),
        };
        let mut text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        text.push('\n');
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(home)?;
            cua_home::guard_write(&path_in(home))?;
            cua_home::write_private(&path_in(home), text.as_bytes())
        };
        write().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Count(AtomicUsize, bool);
    impl Approver for Count {
        fn confirm(&self, _: &str) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if self.1 {
                Ok(())
            } else {
                Err("declined".into())
            }
        }
    }

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn asks(c: Cap) -> bool {
        matches!(c, Cap::Cloud | Cap::HostFiles | Cap::ApiKeys)
    }

    #[test]
    fn defaults_gate_money_and_secrets_only() {
        let h = home();
        let seal = MemorySeal::default();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert!(l.notice.is_none());
        for c in Cap::ALL {
            assert_eq!(l.policy.requires(c), asks(c), "{}", c.id());
        }
        assert_eq!(l.policy.rows().len(), Cap::ALL.len());
        // A build with no seal cannot vouch for a file: the defaults too.
        assert_eq!(Policy::load_with(h.path(), None).policy, Policy::default());
    }

    #[test]
    fn a_stored_policy_verifies_and_applies() {
        let h = home();
        let seal = MemorySeal::default();
        Policy::store_unprompted(
            h.path(),
            &[(Cap::Cloud, false), (Cap::Machines, true)],
            &seal,
        )
        .unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert!(l.notice.is_none(), "{:?}", l.notice);
        assert!(!l.policy.requires(Cap::Cloud));
        assert!(l.policy.requires(Cap::Machines));
    }

    #[test]
    fn a_tampered_file_asks_for_everything_and_says_so() {
        let h = home();
        let seal = MemorySeal::default();
        Policy::store_unprompted(h.path(), &[], &seal).unwrap();
        // A same-user process loosens the file by hand: the MAC no longer fits.
        let path = path_in(h.path());
        let text = std::fs::read_to_string(&path).unwrap();
        let loosened = text.replace("\"cloud\": true", "\"cloud\": false");
        assert_ne!(text, loosened);
        std::fs::write(&path, loosened).unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert_eq!(l.policy, Policy::strict());
        assert!(l.notice.is_some());
        for c in Cap::ALL {
            assert!(l.policy.requires(c), "{}", c.id());
        }
    }

    #[test]
    fn a_forged_file_with_its_own_mac_does_not_verify() {
        let h = home();
        let seal = MemorySeal::default();
        Policy::store_unprompted(h.path(), &[], &seal).unwrap();
        // The attacker cannot read the key, so signs with one of its own.
        let evil = Policy::default();
        let mut file = File {
            version: 2,
            require: Cap::ALL
                .iter()
                .map(|c| (c.id().to_string(), false))
                .collect(),
            generation: 99,
            mac: String::new(),
        };
        let mut loose = evil.clone();
        for c in Cap::ALL {
            loose.require.insert(c, false);
        }
        file.mac = mac_of(&[7u8; 32], &loose, 99);
        std::fs::write(path_in(h.path()), serde_json::to_string(&file).unwrap()).unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert_eq!(l.policy, Policy::strict());
        assert!(l.notice.is_some());
    }

    #[test]
    fn junk_a_deleted_file_and_an_older_copy_all_fail_safe() {
        let h = home();
        let seal = MemorySeal::default();
        let yes = Count(AtomicUsize::new(0), true);
        Policy::set_with(h.path(), Cap::Cloud, false, &yes, Some(&seal)).unwrap();
        let old = std::fs::read_to_string(path_in(h.path())).unwrap();
        Policy::set_with(h.path(), Cap::Cloud, true, &yes, Some(&seal)).unwrap();
        // An older sealed copy (cloud off) put back: it is a valid MAC, but stale.
        std::fs::write(path_in(h.path()), &old).unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert_eq!(l.policy, Policy::strict(), "rollback");
        assert!(l.notice.is_some());
        // Not JSON.
        std::fs::write(path_in(h.path()), "{ not json").unwrap();
        assert_eq!(
            Policy::load_with(h.path(), Some(&seal)).policy,
            Policy::strict()
        );
        // Deleted after one was stored.
        std::fs::remove_file(path_in(h.path())).unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert_eq!(l.policy, Policy::strict());
        assert!(l.notice.is_some());
    }

    #[test]
    fn a_file_nothing_vouches_for_is_not_a_policy() {
        let h = home();
        let seal = MemorySeal::default();
        std::fs::write(
            path_in(h.path()),
            r#"{"require":{"cloud":false,"bogus":false}}"#,
        )
        .unwrap();
        let l = Policy::load_with(h.path(), Some(&seal));
        assert_eq!(l.policy, Policy::strict());
        assert!(l.notice.is_some());
    }

    #[test]
    fn the_user_can_review_after_tampering_and_the_rest_stays_strict() {
        let h = home();
        let seal = MemorySeal::default();
        std::fs::write(path_in(h.path()), "{}").unwrap();
        let yes = Count(AtomicUsize::new(0), true);
        let p = Policy::set_with(h.path(), Cap::Network, false, &yes, Some(&seal)).unwrap();
        assert!(!p.requires(Cap::Network));
        assert!(p.requires(Cap::Display), "untouched rows stay on");
        let l = Policy::load_with(h.path(), Some(&seal));
        assert!(l.notice.is_none(), "the settings are the user's again");
        assert!(!l.policy.requires(Cap::Network));
        // Even a change that equals what is shown needs the user while the notice stands.
        std::fs::write(path_in(h.path()), "{}").unwrap();
        let no = Count(AtomicUsize::new(0), false);
        assert!(Policy::set_with(h.path(), Cap::Display, true, &no, Some(&seal)).is_err());
        assert_eq!(no.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn changing_a_setting_needs_the_user() {
        let h = home();
        let seal = MemorySeal::default();
        let s = Some(&seal as &dyn PolicySeal);
        let no = Count(AtomicUsize::new(0), false);
        assert!(Policy::set_with(h.path(), Cap::Cloud, false, &no, s).is_err());
        assert!(Policy::load_with(h.path(), s).policy.requires(Cap::Cloud));
        assert!(Policy::set_with(h.path(), Cap::Cloud, false, &NeverApprove, s).is_err());
        let yes = Count(AtomicUsize::new(0), true);
        let p = Policy::set_with(h.path(), Cap::Cloud, false, &yes, s).unwrap();
        assert!(!p.requires(Cap::Cloud));
        assert!(!Policy::load_with(h.path(), s).policy.requires(Cap::Cloud));
        // Tightening asks too; no change asks for nothing.
        assert!(Policy::set_with(h.path(), Cap::Cloud, true, &yes, s).is_ok());
        assert_eq!(yes.0.load(Ordering::SeqCst), 2);
        assert!(Policy::set_with(h.path(), Cap::Cloud, true, &no, s).is_ok());
        assert_eq!(no.0.load(Ordering::SeqCst), 1, "unchanged: not asked");
    }

    #[test]
    fn a_build_without_a_seal_cannot_change_the_settings() {
        let h = home();
        let yes = Count(AtomicUsize::new(0), true);
        assert!(Policy::set_with(h.path(), Cap::Cloud, false, &yes, None).is_err());
        assert_eq!(yes.0.load(Ordering::SeqCst), 0);
        assert!(!path_in(h.path()).exists());
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        // Test case 2: key "Jefe", data "what do ya want for nothing?".
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn ids_round_trip() {
        for c in Cap::ALL {
            assert_eq!(Cap::from_id(c.id()), Some(c));
        }
    }
}
