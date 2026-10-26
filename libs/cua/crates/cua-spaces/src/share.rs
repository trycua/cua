//! Sharing a Space with people in your team, to watch or to edit.
//!
//! Sharing is for teams (CUA-1883): only members of your own team, one at a
//! time or the whole team, and only from an account in a team on a plan with
//! sharing. The Spaces API decides (team membership comes from Keycloak
//! organisations); the relay enforces, when a share is made and each time one
//! is used. This crate asks first only to give a clear message, and turns the
//! refusals into plain words ([`NO_TEAM`], [`outside_team`]).
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

/// Why an account without a team cannot share, as the apps say it.
pub const NO_TEAM: &str = "Sharing is for teams. Start a team to share this Space.";

/// Where Teams starts, at one seat.
pub const TEAMS_URL: &str = "https://app.cua.ai/buy/teams?from=app_share";

/// Why `who` cannot be shared with.
pub fn outside_team(who: &str) -> String {
    format!("{who} isn't in your team. Invite them to your team first.")
}

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

/// Relay machine meta: the OS of the Space a machine is (`linux`, `macos`,
/// `windows`).
pub const META_SPACE_OS: &str = "cua.space.os";

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
        return Err(Error::invalid(if cua_home::experimental::teams_enabled() {
            "share with an account (email or account id), or with a team as team:<id> (`cua team ls`)"
        } else {
            "share with an account (email or account id)"
        }));
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

/// A Teams error as the user reads it: a team that is not theirs, a plan
/// that lacks team machines, a server without Teams.
fn team_err(e: cua_host::Error) -> Error {
    use cua_host::spaces_api::is_feature_required;
    match e {
        cua_host::Error::NotFound(m) if m.contains("org_not_found") => Error::NotFound(
            "that team (you are not in it, or it does not exist; a team joined a moment ago shows up after you sign in again)".into(),
        ),
        e if is_feature_required(&e) => Error::Relay(cua_host::Error::PermissionDenied(
            "this team's plan does not include team computers (yet): finish the Teams checkout at app.cua.ai, then try again".into(),
        )),
        cua_host::Error::PermissionDenied(m) if m.contains("machine_limit") => {
            Error::Relay(cua_host::Error::PermissionDenied(
                "the team has as many computers as its seats allow: add seats at app.cua.ai".into(),
            ))
        }
        other => Error::Relay(other),
    }
}

/// The team a `team:<id>` invitee names (`None` for anything else).
/// Sharing with a team makes the Space a team machine: every member (and
/// their agents) sees it and opens it as an editor from a device a team
/// admin approved.
/// With Teams turned off ([`cua_home::experimental`]) a
/// `team:<id>` invitee is refused.
pub fn team_of(who: &str) -> Option<Result<String>> {
    team_of_with(who, cua_home::experimental::teams_enabled())
}

fn team_of_with(who: &str, teams: bool) -> Option<Result<String>> {
    let id = who.trim().strip_prefix("team:")?;
    if !teams {
        return Some(Err(Error::invalid(format!(
            "{} Share with people (email or account id).",
            cua_home::experimental::TEAMS_UNAVAILABLE
        ))));
    }
    // A team is an organization: its id (a UUID) or alias. The relay's
    // old `team-…` ids are refused, never sent anywhere.
    Some(if cua_host::spaces_api::is_legacy_team_id(id) {
        Err(Error::invalid(format!(
            "{who:?} is a team of the old Teams, which this version no longer uses: use team:<id> as `cua team ls` shows it"
        )))
    } else if cua_host::spaces_api::is_org_ref(id) {
        Ok(id.to_string())
    } else {
        Err(Error::invalid(format!(
            "{who:?} is not a team: use team:<id> as `cua team ls` shows it"
        )))
    })
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

/// A refusal from the relay or the Spaces API about who a Space is shared
/// with, in plain words; any other error as it is.
fn share_refused(e: cua_host::Error, who: &str) -> Error {
    let text = e.to_string();
    if text.contains("sharing_needs_team") {
        Error::Relay(cua_host::Error::PermissionDenied(NO_TEAM.into()))
    } else if text.contains("share_outside_team") {
        Error::Relay(cua_host::Error::PermissionDenied(outside_team(who)))
    } else {
        Error::Relay(e)
    }
}

impl Spaces {
    /// Refuses early, before asking the user to confirm, when this account
    /// is in no team that can share. Best effort: a server that predates
    /// team-only sharing, or one that cannot be reached, lets the share
    /// through to the relay, which decides.
    async fn require_team_to_share(&self) -> Result<()> {
        let relay = self.relay()?;
        let (Ok(api), Ok(token)) = (relay.spaces_api(), relay.tokens.access_token().await) else {
            return Ok(());
        };
        match api.sharing(&token).await {
            Ok(s) if !s.can_share => Err(Error::Relay(cua_host::Error::PermissionDenied(
                NO_TEAM.into(),
            ))),
            _ => Ok(()),
        }
    }

    /// People removed from this account's shared Spaces because they are not
    /// in its team, since the owner last acknowledged. For the one-line note
    /// in the apps; empty when there is nothing, or the server has nothing
    /// like it yet.
    pub async fn share_removals(&self) -> Result<Vec<cua_host::spaces_api::ApiShareRemoval>> {
        let relay = self.relay()?;
        let api = relay.spaces_api()?;
        let token = relay.tokens.access_token().await?;
        match api.share_removals(&token).await {
            Err(e) if cua_host::spaces_api::is_teams_unavailable(&e) => Ok(vec![]),
            other => Ok(other?),
        }
    }

    /// The owner has seen [`Self::share_removals`].
    pub async fn ack_share_removals(&self) -> Result<()> {
        let relay = self.relay()?;
        let api = relay.spaces_api()?;
        let token = relay.tokens.access_token().await?;
        match api.ack_share_removals(&token).await {
            Err(e) if cua_host::spaces_api::is_teams_unavailable(&e) => Ok(()),
            other => Ok(other?),
        }
    }

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
        let listed = self.list()?.into_iter().find(|i| i.id == canonical);
        let os = listed.as_ref().map(|i| i.os.clone()).unwrap_or_default();
        let name = listed
            .map(|i| i.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| canonical.clone());
        // What the Space runs, so the relay can keep macOS Spaces out of
        // teams on its own.
        let meta = if os.is_empty() {
            Default::default()
        } else {
            std::collections::BTreeMap::from([(META_SPACE_OS.to_string(), os)])
        };
        // Registering again keeps the lists and rotates the machine token.
        let reg = client
            .register(
                &token,
                &cua_host::relay::RegisterRequest {
                    id: machine.clone(),
                    name,
                    allow: vec![],
                    host: None,
                    meta,
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
        let client = relay.client().await?;
        let m = client.machine(&token, machine).await?;
        let mut shares = entries(&m);
        // Teams it is a machine of (best effort: a server without Teams,
        // or Teams turned off, lists none).
        if cua_home::experimental::teams_enabled()
            && let Ok(api) = relay.spaces_api()
            && let Ok(me) = api.me(&token).await
        {
            for org in me.orgs.iter().filter(|o| o.has("team_machines")) {
                if let Ok(ms) = api.org_machines(&token, &org.id).await
                    && ms.iter().any(|m| m.machine_id == machine)
                {
                    let role = ms
                        .iter()
                        .find(|m| m.machine_id == machine)
                        .map(|m| m.role.as_str())
                        .map_or(ShareRole::Editor, |r| {
                            if r == "viewer" {
                                ShareRole::Viewer
                            } else {
                                ShareRole::Editor
                            }
                        });
                    shares.push(ShareEntry {
                        who: format!("team:{}", org.id),
                        role,
                        connected: false,
                    });
                }
            }
        }
        Ok(SpaceShares {
            space: space.to_string(),
            machine: machine.to_string(),
            invitee_space: format!("relay:{machine}"),
            url: m.url.clone(),
            online: m.online,
            shares,
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
        if let Some(team) = team_of(who) {
            return self
                .share_with_team_as(space, &team?, role, &mut |_| {})
                .await;
        }
        let who = normalize_who(who)?;
        let canonical = self.resolve(space)?.to_string();
        self.require_team_to_share().await?;
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
            .await
            .map_err(|e| share_refused(e, &who))?;
        self.audit_share("share", &canonical, &format!("{who} {}", role.as_str()));
        self.shares_of(&canonical, &machine).await
    }

    /// Stops sharing `space` with `who`, at once (their open streams are
    /// cut). With `who = None` the Space stops being shared with anyone,
    /// and a Space that is not a host leaves the relay.
    pub async fn unshare_space(&self, space: &str, who: Option<&str>) -> Result<SpaceShares> {
        if let Some(team) = who.and_then(team_of) {
            return self.unshare_from_team(space, &team?).await;
        }
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let Some(machine) = self.share_machine(&canonical, &id) else {
            self.info(&id)?;
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

    /// The consent prompt, run off the async runtime.
    async fn confirm_share(&self, reason: String) -> Result<()> {
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
        tokio::task::spawn_blocking(move || consent.confirm(&reason))
            .await
            .map_err(|e| Error::invalid(format!("consent prompt: {e}")))?
            .map_err(|why| {
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "not shared: {why}"
                )))
            })
    }

    /// Makes `space` a machine of `team` (an organization of the Spaces
    /// API, by id or alias): every member (and their agents) lists it and
    /// opens it as an editor, from a device a team admin approved. Only a
    /// team owner or admin who owns the Space can. Macs, Linux and Windows
    /// machines can all be team machines. A Space of a host is its own
    /// relay record: sharing the host does not share its Spaces.
    ///
    /// Right after the team's plan was bought, the API may still answer
    /// `402 feature_required`: this retries for up to a minute
    /// ([`cua_host::spaces_api::SpacesApiClient::finishing_setup`]).
    pub async fn share_with_team(&self, space: &str, team: &str) -> Result<SpaceShares> {
        self.share_with_team_waiting(space, team, &mut |_| {}).await
    }

    /// [`Self::share_with_team`] with what the whole team may do: "Can view"
    /// (`Viewer`: watch only) or "Can edit" (`Editor`). Sharing again with
    /// the other role changes it.
    pub async fn share_with_team_as(
        &self,
        space: &str,
        team: &str,
        role: ShareRole,
        waiting: &mut (dyn FnMut(std::time::Duration) + Send),
    ) -> Result<SpaceShares> {
        self.share_with_team_role(space, team, role, waiting).await
    }

    /// [`Self::share_with_team`], calling `waiting` while a just-paid plan
    /// is on its way (show "Finishing setup…").
    pub async fn share_with_team_waiting(
        &self,
        space: &str,
        team: &str,
        waiting: &mut (dyn FnMut(std::time::Duration) + Send),
    ) -> Result<SpaceShares> {
        self.share_with_team_role(space, team, ShareRole::Editor, waiting)
            .await
    }

    async fn share_with_team_role(
        &self,
        space: &str,
        team: &str,
        role: ShareRole,
        waiting: &mut (dyn FnMut(std::time::Duration) + Send),
    ) -> Result<SpaceShares> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let relay = self.relay()?;
        let api = relay.spaces_api()?;
        let token = relay.tokens.access_token().await?;
        let org = api.resolve_org(&token, team).await.map_err(team_err)?;
        if !org.manages() {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "only an owner or admin of {} can add computers to it",
                org.label()
            ))));
        }
        // What the Space runs, as this device knows it (a relay machine's
        // own record speaks for itself on the relay).
        let listed = self.list()?.into_iter().find(|i| i.id == canonical);
        let os = listed
            .as_ref()
            .map(|i| i.os.clone())
            .filter(|o| !o.is_empty())
            .unwrap_or_default();
        let label = listed.map(|i| i.name).unwrap_or_default();
        self.confirm_share(match role {
            ShareRole::Viewer => {
                format!("Let every member of team {} watch {canonical}", org.label())
            }
            ShareRole::Editor => format!(
                "Let every member of team {} use {canonical} (see and control it)",
                org.label()
            ),
        })
        .await?;
        let machine = self.ensure_shared_machine(&canonical).await?;
        let token = relay.tokens.access_token().await?;
        if let Err(e) = api
            .finishing_setup_org(
                &token,
                &org.id,
                cua_host::spaces_api::SETUP_RETRY_WINDOW,
                waiting,
                || api.add_org_machine_as(&token, &org.id, &machine, &os, &label, role.as_str()),
            )
            .await
        {
            let e = api.explain_plan_error(&token, &org.id, e).await;
            return Err(team_err(e));
        }
        self.audit_share(
            "share",
            &canonical,
            &format!("team:{} {}", org.id, role.as_str()),
        );
        let mut shares = self.shares_of(&canonical, &machine).await?;
        let who = format!("team:{}", org.id);
        if !shares.shares.iter().any(|s| s.who == who) {
            shares.shares.push(ShareEntry {
                who,
                role,
                connected: false,
            });
        }
        Ok(shares)
    }

    /// Takes `space` off `team`.
    pub async fn unshare_from_team(&self, space: &str, team: &str) -> Result<SpaceShares> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let Some(machine) = self.share_machine(&canonical, &id) else {
            return Err(Error::NotFound(format!("{canonical} is not shared")));
        };
        let relay = self.relay()?;
        let api = relay.spaces_api()?;
        let token = relay.tokens.access_token().await?;
        let org = api.resolve_org(&token, team).await.map_err(team_err)?;
        match api.remove_org_machine(&token, &org.id, &machine).await {
            Ok(()) => {}
            Err(cua_host::Error::NotFound(m)) if m.contains("machine_not_found") => {}
            Err(e) => return Err(team_err(e)),
        }
        self.audit_share("unshare", &canonical, &format!("team:{}", org.id));
        let mut shares = self.shares_of(&canonical, &machine).await?;
        let who = format!("team:{}", org.id);
        shares.shares.retain(|s| s.who != who);
        Ok(shares)
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
    /// After a local Space booted again (a start of a stopped VM, a
    /// resize), attaches it to the relay again when it was on it: its
    /// guest forgot the attachment when it shut down. Best effort; a Space
    /// that was never on the relay is left alone.
    pub(crate) async fn reattach_relay_after_boot(&self, id: &SpaceId) {
        let canonical = id.to_string();
        if !self.attached().contains_key(&canonical) {
            return;
        }
        if let Err(e) = self.relay_register(&canonical).await {
            tracing::warn!(space = %canonical, error = %e, "re-attaching to the relay after a boot failed");
        }
    }

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
    /// removed (and with it every share). `relay:<machine>` removes that
    /// machine's record from the relay directory, which only its owner may
    /// do (the relay enforces it): a Space that registered itself (also
    /// after the Space is gone), or a machine that is no longer connected.
    /// A host that is connected now is taken off with `cua host remove` on
    /// it instead.
    pub async fn relay_unregister(&self, space: &str) -> Result<bool> {
        let id = match self.resolve(space) {
            // A bare relay machine id (`space-…`) is known once the
            // directory was listed.
            Err(Error::NotFound(_)) if self.relay_account().is_some() => {
                self.relay_directory(Some(space)).await?;
                self.resolve(space)?
            }
            other => other?,
        };
        let canonical = id.to_string();
        if let SpaceId::Relay { machine_id } = &id {
            let removed = self.forget_relay_machine(machine_id).await?;
            if removed.is_some() {
                self.audit_share("forget", &canonical, &format!("machine={machine_id}"));
            }
            return Ok(removed.is_some());
        }
        let Some(machine) = self.share_machine(&canonical, &id) else {
            return Ok(false);
        };
        self.detach(&canonical, &machine).await?;
        Ok(true)
    }

    /// The Space this device attached to the relay as `machine`.
    fn attached_space_of(&self, machine: &str) -> Option<String> {
        self.attached()
            .into_iter()
            .find(|(_, a)| a.machine == machine)
            .map(|(space, _)| space)
    }

    /// Asks the driver of the Space attached as `machine` (when it still
    /// answers, briefly) to leave the relay, so it does not keep dialing
    /// with a token the relay no longer knows.
    pub(crate) async fn detach_attached_driver(&self, machine: &str) {
        if let Some(space) = self.attached_space_of(machine) {
            self.detach_attached_space(&space).await;
        }
    }

    /// [`Self::detach_attached_driver`] by the Space's id (a Space that was
    /// never attached is left alone).
    pub(crate) async fn detach_attached_space(&self, space: &str) {
        if !self.attached().contains_key(space) {
            return;
        }
        let detach = async {
            if let Ok(s) = self.space(space).await
                && let Ok(d) = s.spacesd()
            {
                let _ = d
                    .system()
                    .detach_relay(cua_proto::env::v1::DetachRelayRequest {})
                    .await;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), detach).await;
    }

    /// Forgets that a Space was attached as `machine`.
    pub(crate) fn forget_attached_machine(&self, machine: &str) {
        let mut all = self.attached();
        let before = all.len();
        all.retain(|_, a| a.machine != machine);
        if all.len() != before
            && let Err(e) = self.save_attached(&all)
        {
            tracing::warn!("shared spaces: {e}");
        }
    }

    /// A Space being deleted that registered itself on the relay leaves it:
    /// its machine record is removed (as its owner), so no stale row stays
    /// behind. Best effort: a failure is logged and returned as a note for
    /// the delete's message.
    pub(crate) async fn release_attached(&self, canonical: &str) -> Option<String> {
        let machine = self.attached().get(canonical).map(|a| a.machine.clone())?;
        let removed = async {
            let relay = self.relay()?;
            let token = relay.tokens.access_token().await?;
            match relay.client().await?.delete(&token, &machine).await {
                Ok(()) | Err(cua_host::Error::NotFound(_)) => Ok(()),
                Err(e) => Err(Error::from(e)),
            }
        }
        .await;
        match removed {
            Ok(()) => {
                self.forget_attached_machine(&machine);
                self.relay_forget(&machine);
                self.audit_share("detach", canonical, &format!("machine={machine}"));
                None
            }
            Err(e) => {
                tracing::warn!(space = canonical, machine, "relay machine not removed: {e}");
                Some(format!(
                    "its relay computer relay:{machine} could not be removed ({e}); remove it with \
                     `cua spaces relay-unregister relay:{machine}`"
                ))
            }
        }
    }

    /// Who `space` is shared with, and who of them is connected now.
    pub async fn space_shares(&self, space: &str) -> Result<SpaceShares> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        match self.share_machine(&canonical, &id) {
            Some(machine) => self.shares_of(&canonical, &machine).await,
            None => {
                self.info(&id)?;
                Ok(SpaceShares {
                    space: canonical,
                    ..Default::default()
                })
            }
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
        assert!(normalize_who("team:design").is_err());
        // A team is an organization, by id or alias.
        let org = "0f1e2d3c-0000-4000-8000-000000000001";
        assert_eq!(
            team_of_with(&format!("team:{org}"), true).unwrap().unwrap(),
            org
        );
        assert_eq!(
            team_of_with("team:design", true).unwrap().unwrap(),
            "design"
        );
        assert!(team_of_with("team:Design Team", true).unwrap().is_err());
        // The old relay team ids are refused.
        let e = team_of_with("team:team-0a1b2c3d4e5f6a7b", true)
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(e.contains("old Teams"), "{e}");
        assert!(team_of_with("bob@x.io", true).is_none());
        assert!(team_of_with("bob@x.io", false).is_none());
        // With Teams off, a team is refused whatever its id.
        let e = team_of_with("team:team-0a1b", false)
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("Teams is turned off (CUA_EXPERIMENTAL_TEAMS=0)."),
            "{e}"
        );
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
