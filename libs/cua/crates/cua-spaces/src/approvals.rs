//! What an agent may do only with the user's approval.
//!
//! A [`Policy`] says, per [`Cap`] (capability), whether a call needs the
//! user's fingerprint (Touch ID, or the login password) before it runs. The
//! MCP server (`mcp::gate`) reads it on every call, so a change applies at
//! once, and asks an [`Approver`]. The Cua Spaces app edits it in Settings.
//!
//! Safe by default: every capability that can spend money, reach the
//! user's own machines, read their secrets, or outlive the session needs
//! approval until the user turns it off. A missing, unreadable or
//! unrecognised file means the defaults. Changing the policy needs the user
//! too: [`Policy::set`] takes an [`Approver`] and writes nothing unless it
//! confirms, so there is no path from a tool call to a loosened setting.
//!
//! Some gates are not settings: sharing a Space, volume grants, computer
//! access grants, teleport and site logins always need the user (they are
//! listed by [`always`] so the app can show them), and the tools that
//! approve a request or edit access are not offered to agents at all.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

    /// Whether a call needs the user's fingerprint until the user says
    /// otherwise.
    pub fn default_required(self) -> bool {
        !matches!(self, Cap::Display)
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
}

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
    /// The policy stored in `home` (the defaults when there is none, or it
    /// cannot be read: it never loosens by failing).
    pub fn load(home: &Path) -> Policy {
        let mut p = Policy::default();
        let Ok(text) = std::fs::read_to_string(path_in(home)) else {
            return p;
        };
        let Ok(file) = serde_json::from_str::<File>(&text) else {
            return p;
        };
        for (id, v) in file.require {
            if let Some(c) = Cap::from_id(&id) {
                p.require.insert(c, v);
            }
        }
        p
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
    pub fn set(
        home: &Path,
        cap: Cap,
        require: bool,
        approver: &dyn Approver,
    ) -> Result<Policy, String> {
        let mut p = Policy::load(home);
        if p.requires(cap) == require {
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
        p.store(home).map_err(|e| e.to_string())?;
        Ok(p)
    }

    fn store(&self, home: &Path) -> std::io::Result<()> {
        let file = File {
            version: 1,
            require: self
                .require
                .iter()
                .map(|(c, v)| (c.id().to_string(), *v))
                .collect(),
        };
        let mut text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
        text.push('\n');
        std::fs::create_dir_all(home)?;
        cua_home::guard_write(&path_in(home))?;
        cua_home::write_private(&path_in(home), text.as_bytes())
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

    #[test]
    fn defaults_gate_everything_but_display() {
        let h = home();
        let p = Policy::load(h.path());
        for c in Cap::ALL {
            assert_eq!(p.requires(c), c != Cap::Display, "{}", c.id());
        }
        assert_eq!(p.rows().len(), Cap::ALL.len());
    }

    #[test]
    fn unreadable_or_unknown_never_loosens() {
        let h = home();
        std::fs::write(path_in(h.path()), "{ not json").unwrap();
        assert_eq!(Policy::load(h.path()), Policy::default());
        std::fs::write(
            path_in(h.path()),
            r#"{"require":{"cloud":false,"bogus":false}}"#,
        )
        .unwrap();
        let p = Policy::load(h.path());
        assert!(!p.requires(Cap::Cloud));
        assert!(p.requires(Cap::Machines));
    }

    #[test]
    fn changing_a_setting_needs_the_user() {
        let h = home();
        let no = Count(AtomicUsize::new(0), false);
        assert!(Policy::set(h.path(), Cap::Cloud, false, &no).is_err());
        assert!(Policy::load(h.path()).requires(Cap::Cloud));
        assert!(Policy::set(h.path(), Cap::Cloud, false, &NeverApprove).is_err());
        let yes = Count(AtomicUsize::new(0), true);
        let p = Policy::set(h.path(), Cap::Cloud, false, &yes).unwrap();
        assert!(!p.requires(Cap::Cloud));
        assert!(!Policy::load(h.path()).requires(Cap::Cloud));
        // Tightening asks too; no change asks for nothing.
        assert!(Policy::set(h.path(), Cap::Cloud, true, &yes).is_ok());
        assert_eq!(yes.0.load(Ordering::SeqCst), 2);
        assert!(Policy::set(h.path(), Cap::Cloud, true, &no).is_ok());
        assert_eq!(no.0.load(Ordering::SeqCst), 1, "unchanged: not asked");
    }

    #[test]
    fn ids_round_trip() {
        for c in Cap::ALL {
            assert_eq!(Cap::from_id(c.id()), Some(c));
        }
    }
}
