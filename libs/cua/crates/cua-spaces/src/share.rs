//! Sharing a Space with other cua.ai accounts, to watch or to edit.
//!
//! Every shared Space is a machine of the owner's account on the relay
//! (`relay:<machine-id>` to the people it is shared with):
//!
//! - a host (`relay:<id>`, from `cua host setup`) already is one;
//! - any other Space whose cua-spacesd supports `relay_attach` (local,
//!   cloud, or added by address) becomes one on its first share: the owner's
//!   account registers a machine for it and the Space's own driver joins
//!   the relay (`SystemService.AttachRelay`), with no restart and no inbound
//!   port.
//!
//! The relay's two lists decide who gets in: `allow` (editors: full use,
//! input as a human) and `viewers` (presence with their own cursor and a
//! view-only stream). Entries are account ids or verified emails, the same
//! allowlist `cua host share` uses, and every invitee's client device must
//! be enrolled for their account. The Space's driver enforces the role
//! itself: a viewer's input, shell, files and MCP are refused there.
//! Removing someone takes effect at once (the relay cuts their open
//! streams). Every share, removal, attach and detach is a line in the
//! hash-chained `<cua home>/shares-audit.jsonl`; joins and refusals are in
//! the relay's audit log and the Space's access log.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, SpaceId, Spaces};

/// The user's confirmation before a Space is shared: sharing hands a
/// desktop to another account, so an agent calling `share_space` must not
/// be able to do it on its own. `cua daemon` answers with user presence
/// (Touch ID or the login password, as the Keyvault does); the `cua` CLI
/// running without a daemon asks on the terminal.
pub trait ShareConsent: Send + Sync {
    /// `Ok` only when the user confirmed `reason` just now.
    fn confirm(&self, reason: &str) -> std::result::Result<(), String>;
}

/// The spacesd feature a Space needs to be shared through the relay
/// (hosts from `cua host setup` are already joined).
pub const RELAY_ATTACH_FEATURE: &str = "relay_attach";

/// What a person may do in a shared Space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRole {
    /// Presence with their own named cursor and a view-only stream.
    Viewer,
    /// Full use of the Space; their input is a human-origin session.
    Editor,
}

impl ShareRole {
    /// Parses `viewer` / `editor`.
    pub fn parse(s: &str) -> Result<ShareRole> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" | "view" | "watch" => Ok(ShareRole::Viewer),
            "editor" | "edit" => Ok(ShareRole::Editor),
            other => Err(Error::invalid(format!(
                "role {other:?}: use viewer or editor"
            ))),
        }
    }

    /// `viewer` / `editor`.
    pub fn as_str(self) -> &'static str {
        match self {
            ShareRole::Viewer => "viewer",
            ShareRole::Editor => "editor",
        }
    }
}

/// One person a Space is shared with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareEntry {
    /// An account id or a (verified) email, as shared.
    pub who: String,
    pub role: ShareRole,
    /// Connected through the relay right now.
    #[serde(default)]
    pub connected: bool,
}

/// A Space's shares.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceShares {
    /// The Space, as the owner names it.
    pub space: String,
    /// The relay machine it is shared as (empty when never shared).
    pub machine: String,
    /// What the people it is shared with open: `relay:<machine>`.
    pub invitee_space: String,
    /// The relay URL of the machine.
    pub url: String,
    /// The machine is connected to the relay now.
    pub online: bool,
    pub shares: Vec<ShareEntry>,
}

/// A Space published on the relay.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayRegistration {
    /// The Space, as the owner names it.
    pub space: String,
    /// How the account's other devices (and anyone it is shared with)
    /// reach it: `relay:<machine>`.
    pub relay_space: String,
    /// The relay machine id.
    pub machine: String,
    /// The relay URL of the machine.
    pub url: String,
    /// Connected to the relay now.
    pub online: bool,
}

/// One owner-side audit line (newest first in listings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareAudit {
    pub ts_ms: u64,
    /// `share`, `unshare`, `attach`, `detach`.
    pub action: String,
    pub space: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Attached {
    /// The relay machine id of an attached (non-host) Space.
    machine: String,
}

/// Normalizes an invitee: an email (lowercased) or an account id. Teams and
/// organizations are not relay principals, so `team:` / `org:` entries are
/// refused with the reason.
pub fn normalize_who(who: &str) -> Result<String> {
    let w = who.trim();
    if w.is_empty() || w.len() > 254 || w.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::invalid(format!(
            "{who:?} is not an email or account id"
        )));
    }
    if w.starts_with("team:") || w.starts_with("org:") || w.starts_with("group:") {
        return Err(Error::invalid(
            "the relay shares with accounts (email or account id), not teams; share with each member",
        ));
    }
    if w.contains('@') {
        let (local, domain) = w.split_once('@').unwrap_or_default();
        if local.is_empty()
            || !domain.contains('.')
            || domain.starts_with('.')
            || domain.ends_with('.')
        {
            return Err(Error::invalid(format!("{who:?} is not an email address")));
        }
        return Ok(w.to_ascii_lowercase());
    }
    if !w
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(Error::invalid(format!(
            "{who:?} is not an email or account id"
        )));
    }
    Ok(w.to_string())
}

/// Moves `who` onto the list for `role` (or off both with `None`).
pub fn apply_share(
    allow: &mut Vec<String>,
    viewers: &mut Vec<String>,
    who: &str,
    role: Option<ShareRole>,
) {
    let same = |a: &String| a.eq_ignore_ascii_case(who);
    allow.retain(|a| !same(a));
    viewers.retain(|a| !same(a));
    match role {
        Some(ShareRole::Editor) => allow.push(who.to_string()),
        Some(ShareRole::Viewer) => viewers.push(who.to_string()),
        None => {}
    }
}

fn entries(m: &crate::relay::RelayMachine) -> Vec<ShareEntry> {
    let connected = |who: &str| {
        m.clients.iter().any(|c| {
            c.id.eq_ignore_ascii_case(who)
                || c.email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(who))
        })
    };
    m.allow
        .iter()
        .map(|w| ShareEntry {
            who: w.clone(),
            role: ShareRole::Editor,
            connected: connected(w),
        })
        .chain(m.viewers.iter().map(|w| ShareEntry {
            who: w.clone(),
            role: ShareRole::Viewer,
            connected: connected(w),
        }))
        .collect()
}

impl Spaces {
    fn shares_path(&self) -> PathBuf {
        self.home_dir().join("shared-spaces.json")
    }

    fn shares_audit(&self) -> crate::audit::AuditLog {
        crate::audit::AuditLog::new(self.home_dir().join("shares-audit.jsonl"))
    }

    fn attached(&self) -> BTreeMap<String, Attached> {
        std::fs::read(self.shares_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_attached(&self, all: &BTreeMap<String, Attached>) -> Result<()> {
        let path = self.shares_path();
        cua_home::guard_write(&path)?;
        cua_home::write_private(&path, &serde_json::to_vec_pretty(all)?)?;
        Ok(())
    }

    fn audit_share(&self, action: &str, space: &str, detail: &str) {
        if let Err(e) = self.shares_audit().append("user", action, space, detail) {
            tracing::warn!("share audit: {e}");
        }
    }

    /// The owner-side share audit, newest first (for one Space or all).
    pub fn share_audit(&self, space: Option<&str>, limit: usize) -> Result<Vec<ShareAudit>> {
        let (events, verdict) = self
            .shares_audit()
            .tail(limit.max(1) * 8)
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        verdict.map_err(|e| Error::invalid(format!("the share audit log does not verify: {e}")))?;
        Ok(events
            .into_iter()
            .filter(|e| space.is_none_or(|s| e.path == s))
            .take(limit)
            .map(|e| ShareAudit {
                ts_ms: e.ts_ms,
                action: e.action,
                space: e.path,
                detail: e.detail,
            })
            .collect())
    }

    /// The relay machine `space` is (or would be) shared as, when it is one
    /// of the account's machines or was attached before.
    fn share_machine(&self, space: &str, id: &SpaceId) -> Option<String> {
        match id {
            SpaceId::Relay { machine_id } => Some(machine_id.clone()),
            _ => self.attached().get(space).map(|a| a.machine.clone()),
        }
    }

    /// Makes sure `space` is a machine of the owner's account on the relay,
    /// online now, and returns its id: a host already is one; any other
    /// Space's driver is attached (registered on first share, re-attached
    /// after the Space restarted).
    async fn ensure_shared_machine(&self, space: &str) -> Result<String> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        if let SpaceId::Relay { machine_id } = &id {
            let m = client.machine(&token, machine_id).await?;
            if m.role != "owner" {
                return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{canonical} is shared with you; only its owner can share it"
                ))));
            }
            return Ok(machine_id.clone());
        }
        let s = self.space(&canonical).await?;
        s.require(RELAY_ATTACH_FEATURE)?;
        let mut all = self.attached();
        let machine = match all.get(&canonical) {
            Some(a) => a.machine.clone(),
            None => format!("space-{:016x}", rand::random::<u64>()),
        };
        if let Ok(m) = client.machine(&token, &machine).await
            && m.online
            && m.role == "owner"
        {
            return Ok(machine);
        }
        let name = self
            .list()?
            .into_iter()
            .find(|i| i.id == canonical)
            .map(|i| i.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| canonical.clone());
        // Registering again keeps the lists and rotates the machine token.
        let reg = client
            .register(
                &token,
                &cua_host::relay::RegisterRequest {
                    id: machine.clone(),
                    name,
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .map_err(|e| match e {
                cua_host::Error::InvalidArgument(m) => Error::invalid(m),
                other => Error::Relay(other),
            })?;
        s.spacesd()?
            .system()
            .attach_relay(cua_proto::env::v1::AttachRelayRequest {
                relay_url: relay.url.clone(),
                machine_token: reg.machine_token,
                machine_id: machine.clone(),
                relay_jwks_json: reg.jwks.to_string(),
                owner: reg.machine.owner.id.clone(),
                owner_email: reg.machine.owner.email.clone().unwrap_or_default(),
            })
            .await
            .map_err(|e| Error::Env(cua_spacesd_client::Error::from(e)))?;
        all.insert(
            canonical.clone(),
            Attached {
                machine: machine.clone(),
            },
        );
        self.save_attached(&all)?;
        self.audit_share("attach", &canonical, &format!("machine={machine}"));
        // The driver dials out on its own: wait (bounded) until the relay
        // lists it online.
        for _ in 0..100 {
            if client
                .machine(&token, &machine)
                .await
                .is_ok_and(|m| m.online)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Ok(machine)
    }

    async fn shares_of(&self, space: &str, machine: &str) -> Result<SpaceShares> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let m = relay.client().await?.machine(&token, machine).await?;
        Ok(SpaceShares {
            space: space.to_string(),
            machine: machine.to_string(),
            invitee_space: format!("relay:{machine}"),
            url: m.url.clone(),
            online: m.online,
            shares: entries(&m),
        })
    }

    /// Shares `space` with `who` (an email or account id) as `role`,
    /// replacing any earlier role. The first share of a Space that is not a
    /// host attaches its driver to the relay. Returns the Space's shares.
    pub async fn share_space(
        &self,
        space: &str,
        who: &str,
        role: ShareRole,
    ) -> Result<SpaceShares> {
        let who = normalize_who(who)?;
        let canonical = self.resolve(space)?.to_string();
        let consent = self
            .inner
            .share_consent
            .read()
            .expect("share consent")
            .clone()
            .ok_or_else(|| {
            Error::host(
                "share consent",
                "nothing here can ask you to confirm; share from the Cua app or run `cua spaces share` in a terminal",
            )
        })?;
        let reason = match role {
            ShareRole::Viewer => format!("Let {who} watch {canonical}"),
            ShareRole::Editor => format!("Let {who} use {canonical} (see and control it)"),
        };
        tokio::task::spawn_blocking(move || consent.confirm(&reason))
            .await
            .map_err(|e| Error::invalid(format!("consent prompt: {e}")))?
            .map_err(|why| {
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "not shared: {why}"
                )))
            })?;
        let machine = self.ensure_shared_machine(&canonical).await?;
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let m = client.machine(&token, &machine).await?;
        let (mut allow, mut viewers) = (m.allow.clone(), m.viewers.clone());
        apply_share(&mut allow, &mut viewers, &who, Some(role));
        client
            .patch(
                &token,
                &machine,
                &cua_host::MachinePatch {
                    allow: Some(allow),
                    viewers: Some(viewers),
                    ..Default::default()
                },
            )
            .await?;
        self.audit_share("share", &canonical, &format!("{who} {}", role.as_str()));
        self.shares_of(&canonical, &machine).await
    }

    /// Stops sharing `space` with `who`, at once (their open streams are
    /// cut). With `who = None` the Space stops being shared with anyone,
    /// and a Space that is not a host leaves the relay.
    pub async fn unshare_space(&self, space: &str, who: Option<&str>) -> Result<SpaceShares> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let Some(machine) = self.share_machine(&canonical, &id) else {
            return Ok(SpaceShares {
                space: canonical,
                ..Default::default()
            });
        };
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let m = client.machine(&token, &machine).await?;
        let (mut allow, mut viewers) = (m.allow.clone(), m.viewers.clone());
        match who {
            Some(w) => apply_share(&mut allow, &mut viewers, &normalize_who(w)?, None),
            None => {
                allow.clear();
                viewers.clear();
            }
        }
        client
            .patch(
                &token,
                &machine,
                &cua_host::MachinePatch {
                    allow: Some(allow),
                    viewers: Some(viewers),
                    ..Default::default()
                },
            )
            .await?;
        self.audit_share("unshare", &canonical, who.unwrap_or("everyone"));
        if who.is_none() && !matches!(id, SpaceId::Relay { .. }) {
            self.detach(&canonical, &machine).await?;
            return Ok(SpaceShares {
                space: canonical,
                ..Default::default()
            });
        }
        self.shares_of(&canonical, &machine).await
    }

    /// Detaches `space`'s driver from the relay and removes its machine.
    async fn detach(&self, canonical: &str, machine: &str) -> Result<()> {
        if let Ok(s) = self.space(canonical).await
            && let Ok(d) = s.spacesd()
        {
            let _ = d
                .system()
                .detach_relay(cua_proto::env::v1::DetachRelayRequest {})
                .await;
        }
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        match relay.client().await?.delete(&token, machine).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        let mut all = self.attached();
        all.remove(canonical);
        self.save_attached(&all)?;
        self.audit_share("detach", canonical, &format!("machine={machine}"));
        Ok(())
    }

    /// Publishes `space` as a machine of the signed-in account on the relay
    /// (what `cua host setup` does for this computer): its own driver joins
    /// the relay over outbound WSS with a machine token the owner's account
    /// registered, so the account's other devices (a phone off this
    /// network, the Cua app elsewhere) reach it as `relay:<machine>`, and
    /// nobody else until it is shared. Idempotent; re-attaches after the
    /// Space restarted. Returns the relay Space id.
    pub async fn relay_register(&self, space: &str) -> Result<RelayRegistration> {
        let canonical = self.resolve(space)?.to_string();
        let machine = self.ensure_shared_machine(&canonical).await?;
        let s = self.shares_of(&canonical, &machine).await?;
        Ok(RelayRegistration {
            space: canonical,
            relay_space: s.invitee_space,
            machine,
            url: s.url,
            online: s.online,
        })
    }

    /// Takes `space` off the relay: its driver leaves, its machine is
    /// removed (and with it every share). A host (`relay:<id>`) is removed
    /// with `cua host remove` on that machine instead.
    pub async fn relay_unregister(&self, space: &str) -> Result<bool> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        if matches!(id, SpaceId::Relay { .. }) {
            return Err(Error::invalid(format!(
                "{canonical} is a host; remove it with `cua host remove` on that machine"
            )));
        }
        let Some(machine) = self.share_machine(&canonical, &id) else {
            return Ok(false);
        };
        self.detach(&canonical, &machine).await?;
        Ok(true)
    }

    /// Who `space` is shared with, and who of them is connected now.
    pub async fn space_shares(&self, space: &str) -> Result<SpaceShares> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        match self.share_machine(&canonical, &id) {
            Some(machine) => self.shares_of(&canonical, &machine).await,
            None => Ok(SpaceShares {
                space: canonical,
                ..Default::default()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_is_an_email_or_an_account_and_never_a_team() {
        assert_eq!(
            normalize_who(" Bob@Example.com ").unwrap(),
            "bob@example.com"
        );
        assert_eq!(normalize_who("acct-123").unwrap(), "acct-123");
        for bad in ["", "two words", "bob@", "@x.com", "bob@localhost", "a/b"] {
            assert_eq!(
                normalize_who(bad).unwrap_err().tag(),
                "invalid_argument",
                "{bad}"
            );
        }
        let e = normalize_who("team:design").unwrap_err().to_string();
        assert!(e.contains("not teams"), "{e}");
    }

    #[test]
    fn a_share_moves_between_lists_and_a_removal_clears_both() {
        let (mut allow, mut viewers) = (vec!["ada@x.io".to_string()], vec![]);
        apply_share(
            &mut allow,
            &mut viewers,
            "bob@x.io",
            Some(ShareRole::Viewer),
        );
        assert_eq!(viewers, ["bob@x.io"]);
        apply_share(
            &mut allow,
            &mut viewers,
            "BOB@x.io",
            Some(ShareRole::Editor),
        );
        assert_eq!(allow, ["ada@x.io", "BOB@x.io"]);
        assert!(viewers.is_empty(), "never on both lists");
        apply_share(&mut allow, &mut viewers, "bob@x.io", None);
        assert_eq!(allow, ["ada@x.io"]);
        assert_eq!(ShareRole::parse("Editor").unwrap(), ShareRole::Editor);
        assert_eq!(ShareRole::parse("watch").unwrap(), ShareRole::Viewer);
        assert!(ShareRole::parse("admin").is_err());
    }
}
