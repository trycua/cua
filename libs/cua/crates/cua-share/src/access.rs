//! Who may open a share: the decision, kept apart from the HTTP so every
//! rule has a plain test.

use serde::{Deserialize, Serialize};

use crate::model::{Access, Share};

/// The header the host's cua-spacesd sets from the relay's verified
/// identity (and removes from anything a client sent).
pub const CALLER_HEADER: &str = "x-cua-share-caller";

/// A verified account opening a share.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    /// Account id.
    #[serde(default)]
    pub account: String,
    /// Email, when the identity provider verified one.
    #[serde(default)]
    pub email: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// `owner` or `shared`: the caller's role on this machine.
    #[serde(default)]
    pub role: String,
    /// Team ids the relay admitted the caller through.
    #[serde(default)]
    pub teams: Vec<String>,
    /// How the caller got here (`relay`, `local`).
    #[serde(default)]
    pub via: String,
}

impl Caller {
    /// A name for the audit log.
    pub fn principal(&self) -> String {
        match self.email.as_deref().filter(|e| !e.is_empty()) {
            Some(e) => e.to_string(),
            None => format!("account:{}", self.account),
        }
    }
}

/// The verdict on one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Serve it.
    Allow,
    /// Nobody is signed in (401).
    Unauthenticated,
    /// Signed in, but not to open this (403).
    Forbidden,
}

/// Decides whether `caller` (none: anonymous) may open `share`.
///
/// Public shares are decided by the token alone. A private share admits only
/// its owner, as the owner of this machine: an account the machine was merely
/// shared with is refused even when the relay let it reach the machine. A
/// team share admits the owner or an account admitted through one of the
/// share's teams.
pub fn decide(share: &Share, caller: Option<&Caller>) -> Verdict {
    if share.access == Access::Public {
        return Verdict::Allow;
    }
    let Some(c) = caller.filter(|c| !c.account.is_empty()) else {
        return Verdict::Unauthenticated;
    };
    let is_owner = c.role == "owner" && c.account == share.owner.account;
    match share.access {
        Access::Private => {
            if is_owner {
                Verdict::Allow
            } else {
                Verdict::Forbidden
            }
        }
        Access::Team => {
            if is_owner || c.teams.iter().any(|t| share.teams.contains(t)) {
                Verdict::Allow
            } else {
                Verdict::Forbidden
            }
        }
        Access::Public => Verdict::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Owner, Target};

    fn share(access: Access) -> Share {
        Share {
            handle: "share-1".into(),
            key: "k".into(),
            name: None,
            target: Target::Port { port: 1 },
            access,
            owner: Owner {
                account: "alice".into(),
                email: None,
            },
            teams: vec!["t1".into()],
            created_ms: 0,
            expires_ms: None,
            url: String::new(),
        }
    }

    fn caller(account: &str, role: &str, teams: &[&str]) -> Caller {
        Caller {
            account: account.into(),
            role: role.into(),
            teams: teams.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn private_admits_only_the_owner() {
        let s = share(Access::Private);
        assert_eq!(decide(&s, None), Verdict::Unauthenticated);
        assert_eq!(
            decide(&s, Some(&Caller::default())),
            Verdict::Unauthenticated
        );
        assert_eq!(
            decide(&s, Some(&caller("alice", "owner", &[]))),
            Verdict::Allow
        );
        // The machine was shared with bob; that is not enough for a private share.
        assert_eq!(
            decide(&s, Some(&caller("bob", "shared", &[]))),
            Verdict::Forbidden
        );
        // Even a team member is refused on a private share.
        assert_eq!(
            decide(&s, Some(&caller("bob", "shared", &["t1"]))),
            Verdict::Forbidden
        );
        // The owner role of another account's machine is not this owner.
        assert_eq!(
            decide(&s, Some(&caller("mallory", "owner", &[]))),
            Verdict::Forbidden
        );
        // Shared role under the owner's own account id is not the owner.
        assert_eq!(
            decide(&s, Some(&caller("alice", "shared", &[]))),
            Verdict::Forbidden
        );
    }

    #[test]
    fn team_admits_owner_and_the_shares_teams() {
        let s = share(Access::Team);
        assert_eq!(decide(&s, None), Verdict::Unauthenticated);
        assert_eq!(
            decide(&s, Some(&caller("alice", "owner", &[]))),
            Verdict::Allow
        );
        assert_eq!(
            decide(&s, Some(&caller("bob", "shared", &["t1"]))),
            Verdict::Allow
        );
        assert_eq!(
            decide(&s, Some(&caller("bob", "shared", &["t2"]))),
            Verdict::Forbidden
        );
        assert_eq!(
            decide(&s, Some(&caller("bob", "shared", &[]))),
            Verdict::Forbidden
        );
    }

    #[test]
    fn public_needs_no_caller() {
        assert_eq!(decide(&share(Access::Public), None), Verdict::Allow);
    }
}
