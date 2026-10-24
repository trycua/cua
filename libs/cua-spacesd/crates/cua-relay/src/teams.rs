// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teams (account mode): a named group of accounts with roles, that shares
//! machines with every member and gates each member device behind an
//! admin's approval.
//!
//! - **Roles.** `owner` (the creator; one per team), `admin`, `member`.
//!   Owners and admins invite and remove members, add and remove team
//!   machines, approve devices and read the team's audit log. Only the
//!   owner makes or unmakes admins, and admins cannot remove each other or
//!   the owner.
//! - **Invites.** By email (only an account whose verified email matches
//!   joins with it, once) or by link (anyone holding the code joins, up to
//!   its use limit). Both expire. Only the SHA-256 of an invite code is
//!   stored.
//! - **Team machines.** A machine its owner (a team owner or admin) adds to
//!   the team is reachable by every member, as an editor (`shared`), and
//!   listed for them and their agents, whatever OS it runs (Macs and
//!   macOS Spaces included).
//! - **Device approval.** A member's device reaches team machines only once
//!   an owner or admin approved it for the team. The first attempt from an
//!   unapproved device files a request the admins see. Owners' and admins'
//!   own enrolled devices are approved on first use (they could approve
//!   them themselves), and that is audited.
//! - **Audit.** Every team change, join, approval and machine open (once
//!   per member device and machine per [`OPEN_AUDIT_INTERVAL_SECS`]) is a
//!   line in the team's own log, bounded to [`TEAM_AUDIT_LIMIT`] entries.
//!
//! Persisted as one JSON file (atomic replace) next to the machine
//! directory; in memory without one.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::devices::scrub;
use crate::directory::MachineRecord;
use crate::oidc::Identity;
use crate::storage::{store, Storage};

/// Teams one account may own.
pub const MAX_OWNED_TEAMS: usize = 16;
/// Members per team.
pub const MAX_MEMBERS: usize = 200;
/// Machines per team.
pub const MAX_TEAM_MACHINES: usize = 100;
/// Open invites per team.
pub const MAX_INVITES: usize = 100;
/// Device records (pending + approved + denied) per team.
pub const MAX_TEAM_DEVICES: usize = 1000;
/// Audit entries kept per team (oldest dropped first).
pub const TEAM_AUDIT_LIMIT: usize = 5000;
/// An email invite lasts this long.
pub const EMAIL_INVITE_TTL_SECS: u64 = 14 * 24 * 3600;
/// A link invite lasts this long.
pub const LINK_INVITE_TTL_SECS: u64 = 7 * 24 * 3600;
/// Joins a link invite allows by default.
pub const LINK_INVITE_USES: u32 = 25;
/// A member device opening a team machine is audited at most this often
/// (per device and machine).
pub const OPEN_AUDIT_INTERVAL_SECS: u64 = 600;

/// Meta key a client sets on a machine (or declares when adding it to a
/// team) for the operating system of the Space it is: `linux`, `macos`,
/// `windows`.
pub const META_SPACE_OS: &str = "cua.space.os";
/// Meta key a host sets for its own operating system.
pub const META_HOST_OS: &str = "cua.host.os";

/// A member's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Member,
    Admin,
    Owner,
}

impl TeamRole {
    /// `owner` / `admin` / `member`.
    pub fn as_str(self) -> &'static str {
        match self {
            TeamRole::Owner => "owner",
            TeamRole::Admin => "admin",
            TeamRole::Member => "member",
        }
    }

    /// Owners and admins manage the team.
    pub fn manages(self) -> bool {
        matches!(self, TeamRole::Owner | TeamRole::Admin)
    }
}

/// One member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Account id.
    pub account: String,
    /// Verified email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub role: TeamRole,
    /// Unix seconds.
    pub joined_at: u64,
}

/// One open invite. The code itself is shown once, when it is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    /// `inv_<hex>`.
    pub id: String,
    /// Only this verified email may use it (an email invite); anyone with
    /// the code otherwise (a link invite).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The role joining gives.
    pub role: TeamRole,
    /// SHA-256 (hex) of the code.
    pub code_sha256: String,
    /// Account that created it.
    pub created_by: String,
    pub created_at: u64,
    pub expires_at: u64,
    /// Joins left.
    pub uses_left: u32,
}

/// `decided_by` of a device approved because its owner is an owner or admin.
const AUTO_APPROVER: &str = "auto";

/// Where a member device stands for a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceApproval {
    Pending,
    Approved,
    Denied,
}

/// A member device's approval for the team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDevice {
    /// The member's account.
    pub account: String,
    /// The member's email, when known (for the admins' list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Relay device id (`dev_…`).
    pub device: String,
    /// The device's name when it asked.
    #[serde(default)]
    pub name: String,
    pub state: DeviceApproval,
    pub requested_at: u64,
    /// Account that approved or denied it (`auto` for an owner's or
    /// admin's own device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<u64>,
}

/// One line of a team's audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAudit {
    pub ts: u64,
    /// `team_created`, `team_renamed`, `invite_created`, `invite_revoked`,
    /// `member_joined`, `member_left`, `member_removed`, `role_changed`,
    /// `device_requested`, `device_approved`, `device_auto_approved`,
    /// `device_denied`, `device_approval_reset`, `machine_added`, `machine_removed`,
    /// `machine_opened`, `access_refused`.
    pub kind: String,
    /// Acting account (email when known).
    pub actor: String,
    /// Acting device, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Other party (member, invite, device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Machine concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    /// `team-<hex>`.
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub members: Vec<Member>,
    #[serde(default)]
    pub invites: Vec<Invite>,
    /// Machine ids shared with the team.
    #[serde(default)]
    pub machines: Vec<String>,
    /// What each machine was added as: its owner then (a grant never
    /// carries over to someone else's machine that reuses the id) and the
    /// OS the adding client declared.
    #[serde(default)]
    pub grants: BTreeMap<String, Grant>,
    #[serde(default)]
    pub devices: Vec<TeamDevice>,
    #[serde(default)]
    pub audit: Vec<TeamAudit>,
}

impl Team {
    /// `account`'s membership.
    pub fn member(&self, account: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.account == account)
    }

    /// `account`'s role.
    pub fn role_of(&self, account: &str) -> Option<TeamRole> {
        self.member(account).map(|m| m.role)
    }

    fn device(&self, account: &str, device: &str) -> Option<&TeamDevice> {
        self.devices
            .iter()
            .find(|d| d.account == account && d.device == device)
    }

    fn log(&mut self, entry: TeamAudit) {
        self.audit.push(entry);
        let over = self.audit.len().saturating_sub(TEAM_AUDIT_LIMIT);
        if over > 0 {
            self.audit.drain(..over);
        }
    }

    /// Takes `machine` off the team (with its grant).
    fn drop_machine(&mut self, machine: &str) -> bool {
        let before = self.machines.len();
        self.machines.retain(|m| m != machine);
        self.grants.remove(machine);
        self.machines.len() != before
    }

    /// Takes the machines `account` contributed off the team (they left or
    /// were removed), logging each.
    fn drop_machines_of(&mut self, account: &str, actor: &Actor) {
        let theirs: Vec<String> = self
            .grants
            .iter()
            .filter(|(_, g)| g.owner == account)
            .map(|(m, _)| m.clone())
            .collect();
        for m in theirs {
            self.drop_machine(&m);
            let mut e = actor.entry("machine_removed");
            e.machine = Some(m);
            e.detail = Some("its owner is no longer in the team".into());
            self.log(e);
        }
    }

    fn prune_invites(&mut self, now: u64) {
        self.invites
            .retain(|i| i.expires_at > now && i.uses_left > 0);
    }
}

/// A machine's grant to a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The machine's owner when it was added.
    pub owner: String,
    /// The OS the adding client declared, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub added_at: u64,
}

/// Team errors, mapped to HTTP statuses by the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamError {
    /// 400.
    Invalid(String),
    /// 403.
    Forbidden(String),
    /// 404 (also for teams the caller is not in).
    NotFound(String),
    /// 409.
    Conflict(String),
    /// 500.
    Storage(String),
}

impl std::fmt::Display for TeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m)
            | Self::Forbidden(m)
            | Self::NotFound(m)
            | Self::Conflict(m)
            | Self::Storage(m) => f.write_str(m),
        }
    }
}

fn no_team() -> TeamError {
    TeamError::NotFound("no such team".into())
}

/// Who is acting: an account, from which device (if known).
#[derive(Debug, Clone)]
pub struct Actor<'a> {
    pub who: &'a Identity,
    pub device: Option<&'a str>,
}

impl Actor<'_> {
    fn label(&self) -> String {
        self.who
            .email
            .clone()
            .unwrap_or_else(|| self.who.account.clone())
    }

    fn entry(&self, kind: &str) -> TeamAudit {
        TeamAudit {
            ts: now_secs(),
            kind: kind.into(),
            actor: self.label(),
            device: self.device.map(str::to_owned),
            subject: None,
            machine: None,
            detail: None,
        }
    }
}

fn member_label(m: &Member) -> String {
    m.email.clone().unwrap_or_else(|| m.account.clone())
}

fn sha256_hex(s: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref())
}

fn random_hex(bytes: usize) -> String {
    use rand::RngCore as _;
    let mut b = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

fn new_secret() -> String {
    use rand::RngCore as _;
    let mut b = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// The team an invite code names: codes are `<team-id>.<secret>`.
pub fn team_of_code(code: &str) -> Option<&str> {
    let (team, secret) = code.trim().split_once('.')?;
    (team.starts_with("team-") && !secret.is_empty()).then_some(team)
}

fn clean_name(name: &str) -> Result<String, TeamError> {
    let n: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned();
    if n.is_empty() {
        return Err(TeamError::Invalid("a team needs a name".into()));
    }
    Ok(n)
}

fn clean_email(email: &str) -> Result<String, TeamError> {
    let e = email.trim().to_ascii_lowercase();
    let ok = e.len() <= 254
        && !e.chars().any(|c| c.is_control() || c.is_whitespace())
        && e.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if !ok {
        return Err(TeamError::Invalid(format!(
            "{email:?} is not an email address"
        )));
    }
    Ok(e)
}

/// What a team member's device may do with a team machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceCheck {
    /// Approved (now or before).
    Approved,
    /// Not approved: the reason to show.
    Refused(String),
}

/// A newly made invite: the record and its code (shown once).
#[derive(Debug, Clone)]
pub struct NewInvite {
    pub invite: Invite,
    pub code: String,
}

#[derive(Debug, Default)]
struct State {
    teams: BTreeMap<String, Team>,
}

/// The team store.
#[derive(Debug, Default)]
pub struct Teams {
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// Last `machine_opened` audit per (account, device, machine).
    opened: Mutex<HashMap<String, u64>>,
    /// Where write failures are counted and reported.
    storage: std::sync::Arc<Storage>,
}

impl Teams {
    /// An in-memory store.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads (or starts) the store persisted at `path`.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Self::open_with(path, Default::default())
    }

    /// [`Teams::open`], reporting writes to `storage`.
    pub fn open_with(path: PathBuf, storage: std::sync::Arc<Storage>) -> std::io::Result<Self> {
        remove_stale_temps(&path)?;
        let teams = match std::fs::read(&path) {
            Ok(raw) => {
                let list: Vec<Team> = serde_json::from_slice(&raw)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                list.into_iter().map(|t| (t.id.clone(), t)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path),
            state: Mutex::new(State { teams }),
            opened: Mutex::default(),
            storage,
        })
    }

    /// Whether the teams file is writable: no write failed lately without
    /// a successful one since (true for an in-memory store).
    pub fn writes_ok(&self) -> bool {
        self.storage.store_ok(store::TEAMS)
    }

    /// Writes the teams file: compact JSON streamed to a temp file that
    /// replaces it. Callers roll their change back when this fails.
    fn persist(&self, state: &State) -> Result<(), TeamError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let written = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_private_with(path, |out| {
                serde_json::to_writer(&mut *out, &state.teams.values().collect::<Vec<_>>())
                    .map_err(std::io::Error::other)
            })
        })();
        self.storage.record(store::TEAMS, &written);
        written.map_err(|e| TeamError::Storage(e.to_string()))
    }

    /// Runs `change` on team `id` and persists it; a change that fails
    /// leaves the team as it was.
    fn change<T>(
        &self,
        id: &str,
        change: impl FnOnce(&mut Team) -> Result<T, TeamError>,
    ) -> Result<T, TeamError> {
        let mut state = self.state.lock().expect("teams");
        let team = state.teams.get(id).ok_or_else(no_team)?;
        let mut next = team.clone();
        let out = change(&mut next)?;
        if &next == team {
            return Ok(out);
        }
        let before = state.teams.insert(id.to_owned(), next);
        if let Err(e) = self.persist(&state) {
            if let Some(before) = before {
                state.teams.insert(id.to_owned(), before);
            }
            return Err(e);
        }
        Ok(out)
    }

    /// Team `id`, when `account` is in it.
    pub fn get_for(&self, id: &str, account: &str) -> Result<(Team, TeamRole), TeamError> {
        let state = self.state.lock().expect("teams");
        let team = state.teams.get(id).ok_or_else(no_team)?;
        let role = team.role_of(account).ok_or_else(no_team)?;
        Ok((team.clone(), role))
    }

    fn require_manager(team: &Team, account: &str) -> Result<TeamRole, TeamError> {
        match team.role_of(account) {
            Some(r) if r.manages() => Ok(r),
            Some(_) => Err(TeamError::Forbidden(
                "only the team's owner or an admin can do this".into(),
            )),
            None => Err(no_team()),
        }
    }

    /// The teams `account` is in and their member counts (no clones).
    pub fn memberships(&self, account: &str) -> Vec<(String, u32)> {
        let state = self.state.lock().expect("teams");
        state
            .teams
            .values()
            .filter(|t| t.role_of(account).is_some())
            .map(|t| (t.id.clone(), t.members.len() as u32))
            .collect()
    }

    /// Every team `account` is in, with its role.
    pub fn teams_of(&self, account: &str) -> Vec<(Team, TeamRole)> {
        let state = self.state.lock().expect("teams");
        state
            .teams
            .values()
            .filter_map(|t| t.role_of(account).map(|r| (t.clone(), r)))
            .collect()
    }

    /// Open email invites for `email` (verified), across teams: the team
    /// and the invite.
    pub fn invites_for(&self, email: &str) -> Vec<(Team, Invite)> {
        let now = now_secs();
        let state = self.state.lock().expect("teams");
        state
            .teams
            .values()
            .flat_map(|t| {
                t.invites
                    .iter()
                    .filter(|i| {
                        i.expires_at > now
                            && i.uses_left > 0
                            && i.email
                                .as_deref()
                                .is_some_and(|e| e.eq_ignore_ascii_case(email))
                    })
                    .map(|i| (t.clone(), i.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Creates a team owned by the actor.
    pub fn create(&self, actor: &Actor, name: &str) -> Result<Team, TeamError> {
        let name = clean_name(name)?;
        let mut state = self.state.lock().expect("teams");
        let owned = state
            .teams
            .values()
            .filter(|t| t.role_of(&actor.who.account) == Some(TeamRole::Owner))
            .count();
        if owned >= MAX_OWNED_TEAMS {
            return Err(TeamError::Conflict(format!(
                "an account can own at most {MAX_OWNED_TEAMS} teams"
            )));
        }
        let id = format!("team-{}", random_hex(8));
        let now = now_secs();
        let mut team = Team {
            id: id.clone(),
            name,
            created_at: now,
            members: vec![Member {
                account: actor.who.account.clone(),
                email: actor.who.email.clone(),
                name: actor.who.name.clone(),
                role: TeamRole::Owner,
                joined_at: now,
            }],
            invites: Vec::new(),
            machines: Vec::new(),
            grants: BTreeMap::new(),
            devices: Vec::new(),
            audit: Vec::new(),
        };
        let mut e = actor.entry("team_created");
        e.detail = Some(team.name.clone());
        team.log(e);
        state.teams.insert(id.clone(), team.clone());
        if let Err(err) = self.persist(&state) {
            state.teams.remove(&id);
            return Err(err);
        }
        Ok(team)
    }

    /// Renames a team (owner or admin).
    pub fn rename(&self, id: &str, actor: &Actor, name: &str) -> Result<Team, TeamError> {
        let name = clean_name(name)?;
        self.change(id, |t| {
            Self::require_manager(t, &actor.who.account)?;
            let mut e = actor.entry("team_renamed");
            e.detail = Some(format!("{} -> {name}", t.name));
            t.name = name;
            t.log(e);
            Ok(t.clone())
        })
    }

    /// Deletes a team (owner only). Returns its machines (their open team
    /// clients should be cut).
    pub fn delete(&self, id: &str, actor: &Actor) -> Result<Team, TeamError> {
        let mut state = self.state.lock().expect("teams");
        let team = state.teams.get(id).ok_or_else(no_team)?;
        match team.role_of(&actor.who.account) {
            Some(TeamRole::Owner) => {}
            Some(_) => {
                return Err(TeamError::Forbidden(
                    "only the team's owner can delete it".into(),
                ))
            }
            None => return Err(no_team()),
        }
        let removed = state.teams.remove(id).ok_or_else(no_team)?;
        if let Err(e) = self.persist(&state) {
            state.teams.insert(id.to_owned(), removed);
            return Err(e);
        }
        Ok(removed)
    }

    /// Creates an invite: by `email` (only that verified email joins with
    /// it) or, without one, a link anyone holding the code can use.
    pub fn invite(
        &self,
        id: &str,
        actor: &Actor,
        email: Option<&str>,
        role: TeamRole,
        uses: Option<u32>,
    ) -> Result<NewInvite, TeamError> {
        let email = email.map(clean_email).transpose()?;
        self.change(id, |t| {
            let mine = Self::require_manager(t, &actor.who.account)?;
            match role {
                TeamRole::Owner => {
                    return Err(TeamError::Invalid(
                        "invite as member or admin; a team has one owner".into(),
                    ))
                }
                TeamRole::Admin if mine != TeamRole::Owner => {
                    return Err(TeamError::Forbidden(
                        "only the team's owner can invite admins".into(),
                    ))
                }
                _ => {}
            }
            let now = now_secs();
            t.prune_invites(now);
            if t.invites.len() >= MAX_INVITES {
                return Err(TeamError::Conflict(format!(
                    "a team can have at most {MAX_INVITES} open invites; revoke some"
                )));
            }
            if let Some(e) = &email {
                if t.members.iter().any(|m| {
                    m.email
                        .as_deref()
                        .is_some_and(|me| me.eq_ignore_ascii_case(e))
                }) {
                    return Err(TeamError::Conflict(format!("{e} is already in the team")));
                }
                // A new invite for the same email replaces the old one.
                t.invites
                    .retain(|i| !i.email.as_deref().is_some_and(|ie| ie == e));
            }
            let code = format!("{}.{}", t.id, new_secret());
            let invite = Invite {
                id: format!("inv_{}", random_hex(8)),
                email: email.clone(),
                role,
                code_sha256: sha256_hex(&code),
                created_by: actor.who.account.clone(),
                created_at: now,
                expires_at: now
                    + if email.is_some() {
                        EMAIL_INVITE_TTL_SECS
                    } else {
                        LINK_INVITE_TTL_SECS
                    },
                uses_left: if email.is_some() {
                    1
                } else {
                    uses.unwrap_or(LINK_INVITE_USES).clamp(1, 500)
                },
            };
            t.invites.push(invite.clone());
            let mut e = actor.entry("invite_created");
            e.subject = Some(email.clone().unwrap_or_else(|| "link".into()));
            e.detail = Some(format!("{} as {}", invite.id, role.as_str()));
            t.log(e);
            Ok(NewInvite { invite, code })
        })
    }

    /// Revokes an open invite.
    pub fn revoke_invite(&self, id: &str, actor: &Actor, invite: &str) -> Result<(), TeamError> {
        self.change(id, |t| {
            Self::require_manager(t, &actor.who.account)?;
            let before = t.invites.len();
            t.invites.retain(|i| i.id != invite);
            if t.invites.len() == before {
                return Err(TeamError::NotFound("no such invite".into()));
            }
            let mut e = actor.entry("invite_revoked");
            e.subject = Some(invite.to_owned());
            t.log(e);
            Ok(())
        })
    }

    /// Joins with an invite: `code` (a link or email invite's code), or
    /// without one, the open email invite for the caller's verified email
    /// in `team`. Joining from an enrolled device files that device's
    /// approval request. Returns the team and the role given.
    pub fn join(
        &self,
        actor: &Actor,
        team: Option<&str>,
        code: Option<&str>,
        device_name: Option<&str>,
    ) -> Result<(Team, TeamRole), TeamError> {
        let id = match (code.and_then(team_of_code), team) {
            (Some(t), _) => t.to_owned(),
            (None, Some(t)) if code.is_none() => t.to_owned(),
            _ => return Err(TeamError::Invalid("that is not a team invite code".into())),
        };
        let invalid =
            || TeamError::Forbidden("this invite is not valid (used, expired or revoked)".into());
        self.change(&id, |t| {
            let now = now_secs();
            t.prune_invites(now);
            if let Some(role) = t.role_of(&actor.who.account) {
                return Ok((t.clone(), role));
            }
            let email = actor.who.email.as_deref();
            let found = match code {
                Some(code) => {
                    let hash = sha256_hex(code.trim());
                    t.invites.iter().position(|i| i.code_sha256 == hash)
                }
                None => email.and_then(|e| {
                    t.invites.iter().position(|i| {
                        i.email.as_deref().is_some_and(|ie| ie.eq_ignore_ascii_case(e))
                    })
                }),
            };
            let Some(pos) = found else {
                return Err(if code.is_some() { invalid() } else { no_team() });
            };
            if let Some(for_email) = t.invites[pos].email.clone() {
                if !email.is_some_and(|e| e.eq_ignore_ascii_case(&for_email)) {
                    return Err(TeamError::Forbidden(format!(
                        "this invite is for {for_email}; sign in with that (verified) email to use it"
                    )));
                }
            }
            if t.members.len() >= MAX_MEMBERS {
                return Err(TeamError::Conflict(format!(
                    "a team can have at most {MAX_MEMBERS} members"
                )));
            }
            let invite = &mut t.invites[pos];
            invite.uses_left = invite.uses_left.saturating_sub(1);
            let (role, invite_id) = (invite.role, invite.id.clone());
            t.prune_invites(now);
            t.members.push(Member {
                account: actor.who.account.clone(),
                email: actor.who.email.clone(),
                name: actor.who.name.clone(),
                role,
                joined_at: now,
            });
            let mut e = actor.entry("member_joined");
            e.detail = Some(format!("as {} with {invite_id}", role.as_str()));
            t.log(e);
            if let Some(device) = actor.device {
                Self::request_device(t, actor, device, device_name.unwrap_or(""), role)?;
            }
            Ok((t.clone(), role))
        })
    }

    /// Accepts every open email invite for the actor's verified email (a
    /// signed-in, enrolled device listing its machines): the way an
    /// invited person joins with any client, including ones that predate
    /// teams. Returns the teams joined.
    pub fn accept_email_invites(&self, actor: &Actor, device_name: &str) -> Vec<String> {
        let Some(email) = actor.who.email.as_deref() else {
            return Vec::new();
        };
        let pending: Vec<String> = self
            .invites_for(email)
            .into_iter()
            .filter(|(t, _)| t.role_of(&actor.who.account).is_none())
            .map(|(t, _)| t.id)
            .collect();
        pending
            .into_iter()
            .filter(|id| {
                self.join(actor, Some(id), None, Some(device_name))
                    .map_err(
                        |e| tracing::warn!(team = %id, error = %e, "email invite not accepted"),
                    )
                    .is_ok()
            })
            .collect()
    }

    /// Account deletion: removes `account` (and anything addressed to
    /// `emails`) from every team. Where they are a member, they leave; a
    /// team they solely own is deleted; a team they own with other members
    /// passes to its longest-standing admin (else its longest-standing
    /// member). Machines they added leave the teams, invites addressed to
    /// their email or created by them are revoked, their device approvals
    /// are removed and their name or email in a team's audit log becomes
    /// "deleted account". Returns the number of teams changed. Idempotent.
    pub fn erase_account(&self, account: &str, emails: &[String]) -> Result<usize, TeamError> {
        const GONE: &str = "deleted account";
        let is_me = |s: &str| s == account || emails.iter().any(|e| e.eq_ignore_ascii_case(s));
        let mut state = self.state.lock().expect("teams");
        let mut next = state.teams.clone();
        let mut changed = 0;
        let ids: Vec<String> = next.keys().cloned().collect();
        for id in ids {
            let team = next.get_mut(&id).expect("team");
            let before = team.clone();
            let role = team.role_of(account);
            if role == Some(TeamRole::Owner) {
                let heir = team
                    .members
                    .iter()
                    .filter(|m| m.account != account)
                    .min_by_key(|m| (m.role != TeamRole::Admin, m.joined_at))
                    .map(|m| m.account.clone());
                match heir {
                    None => {
                        next.remove(&id);
                        changed += 1;
                        continue;
                    }
                    Some(heir) => {
                        if let Some(m) = team.members.iter_mut().find(|m| m.account == heir) {
                            m.role = TeamRole::Owner;
                        }
                        let e = TeamAudit {
                            ts: now_secs(),
                            kind: "role_changed".into(),
                            actor: GONE.into(),
                            device: None,
                            subject: None,
                            machine: None,
                            detail: Some(
                                "the owner's account was deleted; ownership passed on".into(),
                            ),
                        };
                        team.log(e);
                    }
                }
            }
            if role.is_some() {
                team.members.retain(|m| m.account != account);
                team.log(TeamAudit {
                    ts: now_secs(),
                    kind: "member_left".into(),
                    actor: GONE.into(),
                    device: None,
                    subject: None,
                    machine: None,
                    detail: Some("account deleted".into()),
                });
            }
            team.devices.retain(|d| d.account != account);
            // Machines it contributed (a deleted owner's machines are gone).
            let theirs: Vec<String> = team
                .grants
                .iter()
                .filter(|(_, g)| g.owner == account)
                .map(|(m, _)| m.clone())
                .collect();
            for m in theirs {
                team.drop_machine(&m);
            }
            team.invites
                .retain(|i| i.created_by != account && !i.email.as_deref().is_some_and(is_me));
            for e in &mut team.audit {
                let by_me = is_me(&e.actor);
                if by_me {
                    // What they did keeps its place in the log, without who
                    // or on what device (a device's name can be a person's).
                    e.actor = GONE.into();
                    e.device = None;
                    e.detail = None;
                }
                if e.subject.as_deref().is_some_and(is_me) {
                    e.subject = Some(GONE.into());
                    e.detail = None;
                }
                // Anywhere else their id or an address appears in a line.
                if let Some(d) = &mut e.detail {
                    *d = scrub(d, account, emails, GONE);
                }
                if let Some(d) = &mut e.subject {
                    *d = scrub(d, account, emails, GONE);
                }
                e.actor = scrub(&e.actor, account, emails, GONE);
            }
            if *team != before {
                changed += 1;
            }
        }
        if next != state.teams {
            let before = std::mem::replace(&mut state.teams, next);
            if let Err(e) = self.persist(&state) {
                state.teams = before;
                return Err(e);
            }
        }
        drop(state);
        self.opened
            .lock()
            .expect("opened")
            .retain(|k, _| !k.contains(account));
        Ok(changed)
    }

    /// Drops team audit entries older than `days` (0: keep until the count
    /// limit trims them). Returns how many were dropped; a failed write
    /// keeps the old log.
    pub fn trim_audit(&self, days: u64) -> usize {
        if days == 0 {
            return 0;
        }
        let cutoff = crate::assertion::now_secs().saturating_sub(days.saturating_mul(86_400));
        let mut state = self.state.lock().expect("teams");
        let mut next = state.teams.clone();
        let mut dropped = 0;
        for t in next.values_mut() {
            let n = t.audit.iter().take_while(|e| e.ts < cutoff).count();
            t.audit.drain(..n);
            dropped += n;
        }
        if dropped > 0 {
            let before = std::mem::replace(&mut state.teams, next);
            if let Err(e) = self.persist(&state) {
                tracing::warn!(error = %e, "team audit retention not applied; will retry");
                state.teams = before;
                return 0;
            }
        }
        dropped
    }

    /// Emails the teams hold for `account` (members and device requests).
    pub fn emails_of(&self, account: &str) -> Vec<String> {
        let state = self.state.lock().expect("teams");
        let mut out = Vec::new();
        for t in state.teams.values() {
            out.extend(
                t.members
                    .iter()
                    .filter(|m| m.account == account)
                    .filter_map(|m| m.email.clone()),
            );
            out.extend(
                t.devices
                    .iter()
                    .filter(|d| d.account == account)
                    .filter_map(|d| d.email.clone()),
            );
        }
        out
    }

    /// Leaves a team (not its owner).
    pub fn leave(&self, id: &str, actor: &Actor) -> Result<Team, TeamError> {
        self.change(id, |t| {
            match t.role_of(&actor.who.account) {
                None => return Err(no_team()),
                Some(TeamRole::Owner) => {
                    return Err(TeamError::Forbidden(
                        "the owner cannot leave the team; delete it instead".into(),
                    ))
                }
                Some(_) => {}
            }
            t.members.retain(|m| m.account != actor.who.account);
            t.devices.retain(|d| d.account != actor.who.account);
            t.log(actor.entry("member_left"));
            t.drop_machines_of(&actor.who.account, actor);
            Ok(t.clone())
        })
    }

    /// The member `who` names: an account id or an email.
    fn find_member<'t>(t: &'t Team, who: &str) -> Option<&'t Member> {
        t.members.iter().find(|m| {
            m.account == who
                || m.email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(who))
        })
    }

    /// Removes a member: the owner removes anyone else, an admin removes
    /// members. Returns the removed member.
    pub fn remove_member(&self, id: &str, actor: &Actor, who: &str) -> Result<Member, TeamError> {
        self.change(id, |t| {
            let mine = Self::require_manager(t, &actor.who.account)?;
            let target = Self::find_member(t, who)
                .cloned()
                .ok_or_else(|| TeamError::NotFound(format!("{who} is not in the team")))?;
            if target.account == actor.who.account {
                return Err(TeamError::Invalid("to leave the team, use leave".into()));
            }
            if target.role == TeamRole::Owner
                || (target.role == TeamRole::Admin && mine != TeamRole::Owner)
            {
                return Err(TeamError::Forbidden(format!(
                    "an {} cannot remove the team's {}",
                    mine.as_str(),
                    target.role.as_str()
                )));
            }
            t.members.retain(|m| m.account != target.account);
            t.devices.retain(|d| d.account != target.account);
            t.drop_machines_of(&target.account, actor);
            let mut e = actor.entry("member_removed");
            e.subject = Some(member_label(&target));
            t.log(e);
            Ok(target)
        })
    }

    /// Changes a member's role between `member` and `admin` (owner only).
    pub fn set_role(
        &self,
        id: &str,
        actor: &Actor,
        who: &str,
        role: TeamRole,
    ) -> Result<Member, TeamError> {
        self.change(id, |t| {
            match t.role_of(&actor.who.account) {
                Some(TeamRole::Owner) => {}
                Some(_) => {
                    return Err(TeamError::Forbidden(
                        "only the team's owner can change roles".into(),
                    ))
                }
                None => return Err(no_team()),
            }
            if role == TeamRole::Owner {
                return Err(TeamError::Invalid(
                    "a team has one owner; make them an admin".into(),
                ));
            }
            let target = Self::find_member(t, who)
                .cloned()
                .ok_or_else(|| TeamError::NotFound(format!("{who} is not in the team")))?;
            if target.role == TeamRole::Owner {
                return Err(TeamError::Invalid("the owner's role cannot change".into()));
            }
            let m = t
                .members
                .iter_mut()
                .find(|m| m.account == target.account)
                .expect("member");
            let before = m.role;
            m.role = role;
            let after = m.clone();
            let mut e = actor.entry("role_changed");
            e.subject = Some(member_label(&after));
            e.detail = Some(format!("{} -> {}", before.as_str(), role.as_str()));
            t.log(e);
            // A demoted admin's devices were approved only because of the
            // role: they need a fresh approval. Devices another admin
            // approved explicitly stay approved.
            if before.manages() && !role.manages() {
                let mut reset = Vec::new();
                for d in t.devices.iter_mut().filter(|d| {
                    d.account == after.account
                        && d.state == DeviceApproval::Approved
                        && d.decided_by.as_deref() == Some(AUTO_APPROVER)
                }) {
                    d.state = DeviceApproval::Pending;
                    d.decided_by = None;
                    d.decided_at = None;
                    reset.push(d.device.clone());
                }
                for device in reset {
                    let mut e = actor.entry("device_approval_reset");
                    e.subject = Some(device);
                    e.detail = Some("role change".to_owned());
                    t.log(e);
                }
            }
            Ok(after)
        })
    }

    /// Files (or keeps) `device`'s approval request; an owner's or admin's
    /// own device is approved at once.
    fn request_device(
        t: &mut Team,
        actor: &Actor,
        device: &str,
        name: &str,
        role: TeamRole,
    ) -> Result<(), TeamError> {
        if t.device(&actor.who.account, device).is_some() {
            return Ok(());
        }
        if t.devices.len() >= MAX_TEAM_DEVICES {
            // Make room: the oldest decided record, else the oldest request
            // (a member device asks again on its next attempt).
            let pos = t
                .devices
                .iter()
                .position(|d| d.state != DeviceApproval::Pending)
                .unwrap_or(0);
            t.devices.remove(pos);
        }
        let now = now_secs();
        let auto = role.manages();
        t.devices.push(TeamDevice {
            account: actor.who.account.clone(),
            email: actor.who.email.clone(),
            device: device.to_owned(),
            name: name.chars().filter(|c| !c.is_control()).take(64).collect(),
            state: if auto {
                DeviceApproval::Approved
            } else {
                DeviceApproval::Pending
            },
            requested_at: now,
            decided_by: auto.then(|| AUTO_APPROVER.to_owned()),
            decided_at: auto.then_some(now),
        });
        let mut e = actor.entry(if auto {
            "device_auto_approved"
        } else {
            "device_requested"
        });
        e.subject = Some(device.to_owned());
        if !name.is_empty() {
            e.detail = Some(name.to_owned());
        }
        t.log(e);
        Ok(())
    }

    /// A member asks for its device to be approved (idempotent).
    pub fn request_approval(
        &self,
        id: &str,
        actor: &Actor,
        device_name: &str,
    ) -> Result<TeamDevice, TeamError> {
        let device = actor.device.ok_or_else(|| {
            TeamError::Forbidden("ask from an enrolled device (`cua devices enroll`)".into())
        })?;
        self.change(id, |t| {
            let role = t.role_of(&actor.who.account).ok_or_else(no_team)?;
            Self::request_device(t, actor, device, device_name, role)?;
            t.device(&actor.who.account, device)
                .cloned()
                .ok_or_else(|| TeamError::Storage("device request not kept".into()))
        })
    }

    /// Approves or denies a member device (owner or admin). `device` is the
    /// relay device id; `account` disambiguates if two members share one.
    pub fn decide_device(
        &self,
        id: &str,
        actor: &Actor,
        device: &str,
        approve: bool,
    ) -> Result<TeamDevice, TeamError> {
        self.change(id, |t| {
            Self::require_manager(t, &actor.who.account)?;
            let d = t
                .devices
                .iter_mut()
                .find(|d| d.device == device)
                .ok_or_else(|| {
                    TeamError::NotFound(format!(
                        "no request from device {device} (it asks when it first opens a team machine, or with `cua team request <team>`)"
                    ))
                })?;
            d.state = if approve {
                DeviceApproval::Approved
            } else {
                DeviceApproval::Denied
            };
            d.decided_by = Some(actor.label());
            d.decided_at = Some(now_secs());
            let out = d.clone();
            let mut e = actor.entry(if approve {
                "device_approved"
            } else {
                "device_denied"
            });
            e.subject = Some(format!(
                "{} ({})",
                device,
                out.email.clone().unwrap_or_else(|| out.account.clone())
            ));
            t.log(e);
            Ok(out)
        })
    }

    /// Adds a machine the actor owns to the team (owner or admin).
    /// `host` is the record of the host providing it, if any; `declared_os`
    /// is what the client says the Space runs.
    pub fn add_machine(
        &self,
        id: &str,
        actor: &Actor,
        record: &MachineRecord,
        _host: Option<&MachineRecord>,
        declared_os: Option<&str>,
    ) -> Result<Team, TeamError> {
        self.change(id, |t| {
            Self::require_manager(t, &actor.who.account)?;
            if record.owner.id != actor.who.account {
                return Err(TeamError::Forbidden(
                    "add a machine you own: its owner adds it to the team".into(),
                ));
            }
            if t.machines.iter().any(|m| m == &record.id) {
                t.grants.insert(
                    record.id.clone(),
                    Grant {
                        owner: record.owner.id.clone(),
                        os: declared_os.map(str::to_owned),
                        added_at: now_secs(),
                    },
                );
                return Ok(t.clone());
            }
            if t.machines.len() >= MAX_TEAM_MACHINES {
                return Err(TeamError::Conflict(format!(
                    "a team can have at most {MAX_TEAM_MACHINES} machines"
                )));
            }
            t.machines.push(record.id.clone());
            t.grants.insert(
                record.id.clone(),
                Grant {
                    owner: record.owner.id.clone(),
                    os: declared_os.map(str::to_owned),
                    added_at: now_secs(),
                },
            );
            let mut e = actor.entry("machine_added");
            e.machine = Some(record.id.clone());
            e.detail = Some(record.name.clone());
            t.log(e);
            Ok(t.clone())
        })
    }

    /// Takes a machine off the team: an owner or admin, or the machine's
    /// own owner.
    pub fn remove_machine(
        &self,
        id: &str,
        actor: &Actor,
        machine: &str,
        machine_owner: Option<&str>,
    ) -> Result<Team, TeamError> {
        self.change(id, |t| {
            // The machine's owner may always withdraw it (also after leaving
            // the team); otherwise an owner or admin.
            let owns = machine_owner == Some(actor.who.account.as_str());
            let role = t.role_of(&actor.who.account);
            if role.is_none() && !owns {
                return Err(no_team());
            }
            if !role.is_some_and(TeamRole::manages) && !owns {
                return Err(TeamError::Forbidden(
                    "only the team's owner or an admin (or the machine's owner) can remove it"
                        .into(),
                ));
            }
            if !t.drop_machine(machine) {
                return Err(TeamError::NotFound(
                    "that machine is not in the team".into(),
                ));
            }
            let mut e = actor.entry("machine_removed");
            e.machine = Some(machine.to_owned());
            t.log(e);
            Ok(t.clone())
        })
    }

    /// Forgets a machine removed from the directory (every team).
    pub fn forget_machine(&self, machine: &str) {
        let mut state = self.state.lock().expect("teams");
        let mut changed = false;
        for t in state.teams.values_mut() {
            if t.drop_machine(machine) {
                t.log(TeamAudit {
                    ts: now_secs(),
                    kind: "machine_removed".into(),
                    actor: "relay".into(),
                    device: None,
                    subject: None,
                    machine: Some(machine.to_owned()),
                    detail: Some("the machine was removed from the relay".into()),
                });
                changed = true;
            }
        }
        // The machine is gone either way: the change stays in memory. A
        // failed write is counted (store `teams`); the next team write
        // saves it.
        if changed {
            if let Err(e) = self.persist(&state) {
                tracing::warn!(error = %e, "teams not persisted");
            }
        }
    }

    /// The teams of `account` that `record` is shared with and whose grant
    /// still holds: the machine has the owner it was added with (re-checked
    /// on every access). `host` is the providing host's record, if any
    /// (unused since Macs may be team machines). (A machine is in few teams; the store is small.)
    pub fn access(
        &self,
        record: &MachineRecord,
        _host: Option<&MachineRecord>,
        account: &str,
    ) -> Vec<(String, TeamRole)> {
        let state = self.state.lock().expect("teams");
        state
            .teams
            .values()
            .filter_map(|t| {
                let grant = t.grants.get(&record.id)?;
                if grant.owner != record.owner.id {
                    return None;
                }
                t.role_of(account).map(|r| (t.id.clone(), r))
            })
            .collect()
    }

    /// Whether `device` of `account` is approved in any of `teams` (a
    /// read: unlike [`Teams::check_device_any`], files no request).
    pub fn device_approved_any(&self, teams: &[String], account: &str, device: &str) -> bool {
        let state = self.state.lock().expect("teams");
        teams.iter().any(|id| {
            state
                .teams
                .get(id)
                .and_then(|t| t.device(account, device))
                .is_some_and(|d| d.state == DeviceApproval::Approved)
        })
    }

    /// [`Self::check_device`] across the teams a machine is reached
    /// through: the first that approved this device, else the first one's
    /// answer (which files the request there). Returns the team used.
    pub fn check_device_any(
        &self,
        teams: &[String],
        who: &Identity,
        device: Option<&str>,
        machine: &str,
    ) -> (String, DeviceCheck) {
        if let Some(device) = device {
            let state = self.state.lock().expect("teams");
            for id in teams {
                if state
                    .teams
                    .get(id)
                    .and_then(|t| t.device(&who.account, device))
                    .is_some_and(|d| d.state == DeviceApproval::Approved)
                {
                    return (id.clone(), DeviceCheck::Approved);
                }
            }
        }
        let first = teams.first().cloned().unwrap_or_default();
        let check = self.check_device(&first, who, device, machine);
        (first, check)
    }

    /// Every team machine `account` can reach through a team: machine id
    /// -> team id.
    pub fn machines_for(&self, account: &str) -> BTreeMap<String, String> {
        let state = self.state.lock().expect("teams");
        let mut out = BTreeMap::new();
        for t in state.teams.values() {
            if t.role_of(account).is_some() {
                for m in &t.machines {
                    out.entry(m.clone()).or_insert_with(|| t.id.clone());
                }
            }
        }
        out
    }

    /// Whether `device` of `who` may open team machines of `team`, filing a
    /// request (and approving an owner's or admin's own device) on first
    /// sight. Audits a refusal.
    pub fn check_device(
        &self,
        team: &str,
        who: &Identity,
        device: Option<&str>,
        machine: &str,
    ) -> DeviceCheck {
        let Some(device) = device else {
            return DeviceCheck::Refused(
                "team machines need an enrolled device: run `cua devices enroll` (or sign in again)".into(),
            );
        };
        // The hot path (every proxied request): an approved device needs no
        // write.
        {
            let state = self.state.lock().expect("teams");
            if state
                .teams
                .get(team)
                .and_then(|t| t.device(&who.account, device))
                .is_some_and(|d| d.state == DeviceApproval::Approved)
            {
                return DeviceCheck::Approved;
            }
        }
        let actor = Actor {
            who,
            device: Some(device),
        };
        let result = self.change(team, |t| {
            let role = t.role_of(&who.account).ok_or_else(no_team)?;
            Self::request_device(t, &actor, device, "", role)?;
            let d = t
                .device(&who.account, device)
                .cloned()
                .ok_or_else(|| TeamError::Storage("device request not kept".into()))?;
            let check = match d.state {
                DeviceApproval::Approved => DeviceCheck::Approved,
                DeviceApproval::Pending => DeviceCheck::Refused(format!(
                    "this device ({device}) is waiting for a team admin's approval before it can open {}'s machines: ask an owner or admin to run `cua team approve {} {device}` or approve it in the Cua Spaces app",
                    t.name, t.id
                )),
                DeviceApproval::Denied => DeviceCheck::Refused(format!(
                    "a team admin of {} declined this device ({device})",
                    t.name
                )),
            };
            if check != DeviceCheck::Approved {
                // At most one refusal line per device and machine per
                // interval (checked under the same lock below).
                let mut e = actor.entry("access_refused");
                e.machine = Some(machine.to_owned());
                e.subject = Some(device.to_owned());
                e.detail = Some("device not approved".into());
                if self.should_log_open(&format!("refused:{}:{device}:{machine}", who.account)) {
                    t.log(e);
                }
            }
            Ok(check)
        });
        result.unwrap_or_else(|e| DeviceCheck::Refused(e.to_string()))
    }

    fn should_log_open(&self, key: &str) -> bool {
        let now = now_secs();
        let mut opened = self.opened.lock().expect("opened");
        if opened
            .get(key)
            .is_some_and(|last| now.saturating_sub(*last) < OPEN_AUDIT_INTERVAL_SECS)
        {
            return false;
        }
        if opened.len() > 10_000 {
            opened.retain(|_, last| now.saturating_sub(*last) < OPEN_AUDIT_INTERVAL_SECS);
        }
        opened.insert(key.to_owned(), now);
        true
    }

    /// Records a member device opening a team machine (throttled).
    pub fn record_open(&self, team: &str, who: &Identity, device: Option<&str>, machine: &str) {
        let key = format!("open:{}:{}:{machine}", who.account, device.unwrap_or("-"));
        if !self.should_log_open(&key) {
            return;
        }
        let actor = Actor { who, device };
        let _ = self.change(team, |t| {
            let mut e = actor.entry("machine_opened");
            e.machine = Some(machine.to_owned());
            t.log(e);
            Ok(())
        });
    }

    /// The team's audit log, newest last (owner or admin).
    pub fn audit(
        &self,
        id: &str,
        account: &str,
        limit: usize,
    ) -> Result<Vec<TeamAudit>, TeamError> {
        let state = self.state.lock().expect("teams");
        let t = state.teams.get(id).ok_or_else(no_team)?;
        Self::require_manager(t, account)?;
        let n = t.audit.len();
        Ok(t.audit[n.saturating_sub(limit)..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::Owner;

    fn who(account: &str, email: &str) -> Identity {
        Identity {
            user: account.into(),
            account: account.into(),
            email: Some(email.into()),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    fn record(id: &str, owner: &str) -> MachineRecord {
        MachineRecord {
            id: id.into(),
            name: id.into(),
            owner: Owner {
                id: owner.into(),
                email: None,
                name: None,
            },
            allow: vec![],
            viewers: vec![],
            sharing: true,
            stopped_by: None,
            token_sha256: String::new(),
            created_at: 0,
            host: None,
            confirmed: true,
            meta: BTreeMap::new(),
        }
    }

    #[test]
    fn roles_invites_devices_and_machines() {
        let teams = Teams::in_memory();
        let ada = who("ada", "ada@x.io");
        let bob = who("bob", "bob@x.io");
        let eve = who("eve", "eve@x.io");
        let a = Actor {
            who: &ada,
            device: Some("dev_ada"),
        };
        let b = Actor {
            who: &bob,
            device: Some("dev_bob"),
        };
        let e = Actor {
            who: &eve,
            device: Some("dev_eve"),
        };
        let t = teams.create(&a, "AIGIS").unwrap();

        // Email invite: only that email joins, once.
        let inv = teams
            .invite(&t.id, &a, Some("Bob@x.io"), TeamRole::Member, None)
            .unwrap();
        assert_eq!(inv.invite.email.as_deref(), Some("bob@x.io"));
        assert!(matches!(
            teams.join(&e, None, Some(&inv.code), None),
            Err(TeamError::Forbidden(_))
        ));
        let (_, role) = teams
            .join(&b, None, Some(&inv.code), Some("bob mac"))
            .unwrap();
        assert_eq!(role, TeamRole::Member);
        // Bob's joining device waits for approval.
        let (team, _) = teams.get_for(&t.id, "bob").unwrap();
        assert_eq!(team.devices[0].state, DeviceApproval::Pending);
        assert!(team.invites.is_empty(), "an email invite is used once");

        // A member cannot invite, add machines, or approve.
        assert!(matches!(
            teams.invite(&t.id, &b, None, TeamRole::Member, None),
            Err(TeamError::Forbidden(_))
        ));
        assert!(matches!(
            teams.decide_device(&t.id, &b, "dev_bob", true),
            Err(TeamError::Forbidden(_))
        ));

        // Link invite: anyone with the code joins.
        let link = teams
            .invite(&t.id, &a, None, TeamRole::Member, Some(2))
            .unwrap();
        teams.join(&e, None, Some(&link.code), None).unwrap();
        assert!(teams.join(&e, None, Some("team-0.bad"), None).is_err());

        // Machines: the owner adds one it owns, whatever OS (Macs too).
        let linux = record("space-linux0001", "ada");
        teams
            .add_machine(&t.id, &a, &linux, None, Some("linux"))
            .unwrap();
        teams
            .add_machine(
                &t.id,
                &a,
                &record("space-mac00001", "ada"),
                None,
                Some("macos"),
            )
            .unwrap();
        // A Space a Mac provides, and the whole Mac (a host).
        let mut on_mac = record("space-onmac0001", "ada");
        on_mac.host = Some("host-mac".into());
        let mut mac_host = record("host-mac", "ada");
        mac_host.meta.insert(META_HOST_OS.into(), "macos".into());
        teams
            .add_machine(&t.id, &a, &on_mac, Some(&mac_host), None)
            .unwrap();
        teams.add_machine(&t.id, &a, &mac_host, None, None).unwrap();
        // Not the owner of the machine: refused.
        assert!(teams
            .add_machine(
                &t.id,
                &a,
                &record("space-bobs0001", "bob"),
                None,
                Some("linux")
            )
            .is_err());

        assert_eq!(teams.access(&linux, None, "bob")[0].1, TeamRole::Member);
        assert!(teams.access(&linux, None, "mallory").is_empty());
        // A grant does not carry over to another owner's machine reusing
        // the id; a macOS Space keeps its grant.
        let mut reused = linux.clone();
        reused.owner.id = "mallory".into();
        assert!(teams.access(&reused, None, "bob").is_empty());
        let mut now_mac = linux.clone();
        now_mac.meta.insert(META_SPACE_OS.into(), "macos".into());
        assert_eq!(teams.access(&now_mac, None, "bob").len(), 1);

        // Device gate: pending, then approved by the owner.
        assert!(matches!(
            teams.check_device(&t.id, &bob, Some("dev_bob"), "space-linux0001"),
            DeviceCheck::Refused(m) if m.contains("waiting for a team admin")
        ));
        // A device never seen files a request too.
        assert!(matches!(
            teams.check_device(&t.id, &bob, Some("dev_bob2"), "space-linux0001"),
            DeviceCheck::Refused(_)
        ));
        teams.decide_device(&t.id, &a, "dev_bob", true).unwrap();
        assert_eq!(
            teams.check_device(&t.id, &bob, Some("dev_bob"), "space-linux0001"),
            DeviceCheck::Approved
        );
        assert!(matches!(
            teams.check_device(&t.id, &bob, None, "space-linux0001"),
            DeviceCheck::Refused(_)
        ));
        // The owner's own device is approved on first use.
        assert_eq!(
            teams.check_device(&t.id, &ada, Some("dev_ada"), "space-linux0001"),
            DeviceCheck::Approved
        );

        // Roles: only the owner promotes; an admin cannot remove an admin.
        assert!(teams
            .set_role(&t.id, &b, "eve@x.io", TeamRole::Admin)
            .is_err());
        teams
            .set_role(&t.id, &a, "eve@x.io", TeamRole::Admin)
            .unwrap();
        teams.set_role(&t.id, &a, "bob", TeamRole::Admin).unwrap();
        assert!(teams.remove_member(&t.id, &e, "bob").is_err());
        teams.set_role(&t.id, &a, "bob", TeamRole::Member).unwrap();
        teams.remove_member(&t.id, &e, "bob@x.io").unwrap();
        assert!(teams.access(&linux, None, "bob").is_empty());
        assert!(teams.remove_member(&t.id, &e, "ada").is_err());
        assert!(teams.leave(&t.id, &a).is_err(), "the owner deletes instead");

        // The audit names every step.
        let kinds: Vec<_> = teams
            .audit(&t.id, "ada", 100)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        for k in [
            "team_created",
            "invite_created",
            "member_joined",
            "device_requested",
            "machine_added",
            "device_approved",
            "device_auto_approved",
            "role_changed",
            "member_removed",
            "access_refused",
        ] {
            assert!(kinds.iter().any(|x| x == k), "{k} in {kinds:?}");
        }
        assert!(
            teams.audit(&t.id, "eve", 10).is_ok(),
            "admins read the audit"
        );

        teams.forget_machine("space-linux0001");
        assert!(teams.access(&linux, None, "eve").is_empty());
        assert!(teams.delete(&t.id, &e).is_err());
        teams.delete(&t.id, &a).unwrap();
        assert!(teams.teams_of("ada").is_empty());
    }

    #[test]
    fn demoting_an_admin_resets_only_the_devices_their_role_approved() {
        let teams = Teams::in_memory();
        let ada = who("ada", "ada@x.io");
        let bob = who("bob", "bob@x.io");
        let a = Actor {
            who: &ada,
            device: Some("dev_ada"),
        };
        let t = teams.create(&a, "Demote").unwrap();
        teams.request_approval(&t.id, &a, "ada mac").unwrap();
        let inv = teams
            .invite(&t.id, &a, Some("bob@x.io"), TeamRole::Member, None)
            .unwrap();
        let b = |device| Actor {
            who: &bob,
            device: Some(device),
        };
        teams
            .join(&b("dev_bob_old"), None, Some(&inv.code), None)
            .unwrap();
        // The owner approves Bob's first device by hand while he is a member.
        teams.decide_device(&t.id, &a, "dev_bob_old", true).unwrap();
        // Promoted: his next device is approved by the role alone.
        teams.set_role(&t.id, &a, "bob", TeamRole::Admin).unwrap();
        teams
            .request_approval(&t.id, &b("dev_bob_role"), "laptop")
            .unwrap();
        let state_of = |id: &str| {
            let team = teams.get_for(&t.id, "ada").unwrap().0;
            team.devices
                .iter()
                .find(|d| d.device == id)
                .unwrap()
                .clone()
        };
        assert_eq!(state_of("dev_bob_role").state, DeviceApproval::Approved);
        assert_eq!(state_of("dev_bob_role").decided_by.as_deref(), Some("auto"));

        teams.set_role(&t.id, &a, "bob", TeamRole::Member).unwrap();

        let reset = state_of("dev_bob_role");
        assert_eq!(reset.state, DeviceApproval::Pending);
        assert!(reset.decided_by.is_none() && reset.decided_at.is_none());
        assert!(matches!(
            teams.check_device(&t.id, &bob, Some("dev_bob_role"), "x"),
            DeviceCheck::Refused(_)
        ));
        // Approved explicitly by the owner: untouched. The owner's own too.
        assert_eq!(state_of("dev_bob_old").state, DeviceApproval::Approved);
        assert_eq!(state_of("dev_ada").state, DeviceApproval::Approved);
        let kinds: Vec<_> = teams
            .audit(&t.id, "ada", 100)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert!(kinds.iter().any(|k| k == "device_approval_reset"));
        // An admin approving the pending device again makes it explicit.
        teams
            .decide_device(&t.id, &a, "dev_bob_role", true)
            .unwrap();
        teams.set_role(&t.id, &a, "bob", TeamRole::Admin).unwrap();
        teams.set_role(&t.id, &a, "bob", TeamRole::Member).unwrap();
        assert_eq!(state_of("dev_bob_role").state, DeviceApproval::Approved);
    }

    #[test]
    fn persists_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams.json");
        let ada = who("ada", "ada@x.io");
        let a = Actor {
            who: &ada,
            device: None,
        };
        let id = {
            let teams = Teams::open(path.clone()).unwrap();
            let t = teams.create(&a, "Persisted").unwrap();
            teams
                .invite(&t.id, &a, Some("bob@x.io"), TeamRole::Member, None)
                .unwrap();
            t.id
        };
        let teams = Teams::open(path).unwrap();
        let (t, role) = teams.get_for(&id, "ada").unwrap();
        assert_eq!(role, TeamRole::Owner);
        assert_eq!(t.invites.len(), 1);
        assert_eq!(teams.invites_for("BOB@x.io").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_is_counted_and_leaves_the_teams_as_they_were() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let storage = std::sync::Arc::new(Storage::new(Some(state.clone()), Default::default()));
        let path = state.join("relay.teams.json");
        let teams = Teams::open_with(path.clone(), storage.clone()).unwrap();
        let ada = who("ada", "ada@x.io");
        let a = Actor {
            who: &ada,
            device: None,
        };
        let t = teams.create(&a, "Kept").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains('\n'), "compact JSON: {raw}");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        let created = teams.create(&a, "Lost");
        let renamed = teams.rename(&t.id, &a, "Renamed");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Root ignores directory permissions; nothing to check then.
        if storage.failures(store::TEAMS) > 0 {
            assert!(matches!(created, Err(TeamError::Storage(_))), "{created:?}");
            assert!(matches!(renamed, Err(TeamError::Storage(_))), "{renamed:?}");
            assert_eq!(teams.get_for(&t.id, "ada").unwrap().0.name, "Kept");
            assert_eq!(teams.state.lock().unwrap().teams.len(), 1);
            assert!(!teams.writes_ok());
            let (ready, body) = storage.report();
            assert!(!ready);
            assert_eq!(body["failing"], serde_json::json!(["teams"]));
        }
        teams.rename(&t.id, &a, "Renamed").unwrap();
        assert!(teams.writes_ok());
        let again = Teams::open(path).unwrap();
        assert_eq!(again.get_for(&t.id, "ada").unwrap().0.name, "Renamed");
    }
}
