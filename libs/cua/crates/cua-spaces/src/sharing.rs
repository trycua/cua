//! Sharing facts on every Space, and leaving or deleting several at once.
//!
//! A Space that is a machine on the relay carries who owns it, what this
//! account may do with it (`owner`, `editor`, `viewer`), the team it is
//! reached through and, in the owner's view, who else has it
//! ([`SpaceSharing`]). The relay directory already says most of that; the
//! team's display name and a team member's real role (a team share can be
//! watch only) come from the Spaces API (`GET /v1/me` orgs and
//! `GET /v1/orgs/{org}/machines`), read at most once a minute and kept
//! ([`TeamFacts`]).
//!
//! [`Spaces::leave_many`] takes this account off Spaces shared with it
//! (`POST /v1/machines/{id}/leave`); [`Spaces::delete_many`] deletes
//! several of your own, turning running ones off first. Both answer one
//! [`BulkResult`] per id and never stop at the first failure.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SpaceId, Spaces};

/// Who a Space belongs to and who else has it, as this account sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSharing {
    /// The owner's account id.
    #[serde(default)]
    pub owner_id: String,
    /// The owner's email, when the relay knows it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_email: String,
    /// The owner's display name, when the relay knows it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_name: String,
    /// What this account may do: `owner`, `editor` or `viewer`.
    pub role: String,
    /// The team this account reaches it through (its key), when it is a
    /// team's and not shared with this account directly.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub team: String,
    /// That team's display name (its key when the name is not known).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub team_name: String,
    /// Owner view: accounts (emails or ids) that may use it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<String>,
    /// Owner view: accounts that may only watch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewers: Vec<String>,
    /// Owner view: the teams it is a machine of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<TeamShare>,
}

/// A team a Space is shared with (owner view).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamShare {
    /// The team's key (org id).
    pub key: String,
    /// Its display name (the key when not known).
    pub name: String,
    /// `editor` or `viewer`.
    pub role: String,
}

impl From<cua_proto::daemon::v1::SpaceSharing> for SpaceSharing {
    fn from(s: cua_proto::daemon::v1::SpaceSharing) -> Self {
        Self {
            owner_id: s.owner_id,
            owner_email: s.owner_email,
            owner_name: s.owner_name,
            role: s.role,
            team: s.team,
            team_name: s.team_name,
            editors: s.editors,
            viewers: s.viewers,
            teams: s
                .teams
                .into_iter()
                .map(|t| TeamShare {
                    key: t.key,
                    name: t.name,
                    role: t.role,
                })
                .collect(),
        }
    }
}

impl From<&SpaceSharing> for cua_proto::daemon::v1::SpaceSharing {
    fn from(s: &SpaceSharing) -> Self {
        Self {
            owner_id: s.owner_id.clone(),
            owner_email: s.owner_email.clone(),
            owner_name: s.owner_name.clone(),
            role: s.role.clone(),
            team: s.team.clone(),
            team_name: s.team_name.clone(),
            editors: s.editors.clone(),
            viewers: s.viewers.clone(),
            teams: s
                .teams
                .iter()
                .map(|t| cua_proto::daemon::v1::SpaceTeamShare {
                    key: t.key.clone(),
                    name: t.name.clone(),
                    role: t.role.clone(),
                })
                .collect(),
        }
    }
}

impl From<&BulkResult> for cua_proto::daemon::v1::SpaceBulkResult {
    fn from(r: &BulkResult) -> Self {
        Self {
            space: r.id.clone(),
            ok: r.ok,
            error: r.error.clone().unwrap_or_default(),
            message: r.message.clone(),
        }
    }
}

impl From<cua_proto::daemon::v1::SpaceBulkResult> for BulkResult {
    fn from(r: cua_proto::daemon::v1::SpaceBulkResult) -> Self {
        Self {
            id: r.space,
            ok: r.ok,
            error: (!r.error.is_empty()).then_some(r.error),
            message: r.message,
        }
    }
}

/// The outcome for one Space of [`Spaces::delete_many`] or
/// [`Spaces::leave_many`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkResult {
    /// The id as asked.
    pub id: String,
    /// It went through.
    pub ok: bool,
    /// Why not, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What happened, for people (the single delete's message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

impl BulkResult {
    fn ok(id: &str, message: String) -> Self {
        Self {
            id: id.to_string(),
            ok: true,
            error: None,
            message,
        }
    }

    fn failed(id: &str, error: String) -> Self {
        Self {
            id: id.to_string(),
            ok: false,
            error: Some(error),
            message: String::new(),
        }
    }
}

/// How many Spaces [`Spaces::delete_many`] deletes at the same time.
pub const BULK_CONCURRENCY: usize = 3;

/// What the Spaces API says about the account's teams: display names and
/// the machines each team has with the team's role on them.
#[derive(Clone, Debug, Default)]
pub struct TeamFacts {
    read_at: Option<Instant>,
    /// Team key (org id, and alias) -> display name.
    names: BTreeMap<String, String>,
    /// Machine id -> (team key, `editor` | `viewer`).
    machines: BTreeMap<String, Vec<(String, String)>>,
}

impl TeamFacts {
    /// How long an answer is kept.
    pub const MAX_AGE: Duration = Duration::from_secs(60);

    /// Facts from the API's answers: `orgs` as (id, alias, display name)
    /// and each team's machines as (team id, machine id, role).
    pub fn from_answers(
        orgs: impl IntoIterator<Item = (String, String, String)>,
        machines: impl IntoIterator<Item = (String, String, String)>,
    ) -> Self {
        let mut f = Self::default();
        for (id, alias, label) in orgs {
            for key in [id, alias] {
                if !key.is_empty() && !label.is_empty() {
                    f.names.insert(key, label.clone());
                }
            }
        }
        for (team, machine, role) in machines {
            let role = if role == "viewer" { "viewer" } else { "editor" };
            f.machines
                .entry(machine)
                .or_default()
                .push((team, role.to_string()));
        }
        f
    }

    /// The team's display name, else its key.
    pub fn name(&self, key: &str) -> String {
        self.names
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// The role team `key` has on `machine`, when known.
    pub fn role(&self, key: &str, machine: &str) -> Option<&str> {
        self.machines
            .get(machine)?
            .iter()
            .find(|(t, _)| t == key)
            .map(|(_, r)| r.as_str())
    }

    fn fresh(&self) -> bool {
        self.read_at.is_some_and(|at| at.elapsed() < Self::MAX_AGE)
    }
}

/// The sharing facts of relay machine `m` as this account sees it.
pub fn sharing_of(m: &crate::relay::RelayMachine, facts: &TeamFacts) -> SpaceSharing {
    let team = m.team.clone().unwrap_or_default();
    let role = match m.role.as_str() {
        "owner" => "owner",
        "viewer" => "viewer",
        // Reached through a team: the team's role on it (a team share can
        // be watch only); shared directly: an editor.
        _ if !team.is_empty() && facts.role(&team, &m.id) == Some("viewer") => "viewer",
        _ => "editor",
    };
    let teams = if role == "owner" {
        facts
            .machines
            .get(&m.id)
            .map(|ts| {
                ts.iter()
                    .map(|(key, role)| TeamShare {
                        key: key.clone(),
                        name: facts.name(key),
                        role: role.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    SpaceSharing {
        owner_id: m.owner.id.clone(),
        owner_email: m.owner.email.clone().unwrap_or_default(),
        owner_name: m.owner.name.clone().unwrap_or_default(),
        role: role.to_string(),
        team_name: if team.is_empty() {
            String::new()
        } else {
            facts.name(&team)
        },
        team,
        editors: m.allow.clone(),
        viewers: m.viewers.clone(),
        teams,
    }
}

/// `Couldn't leave <name>: <why>.`
fn leave_error(name: &str, why: &str) -> String {
    let why = why
        .trim()
        .trim_start_matches("relay: ")
        .trim_end_matches('.');
    format!("Couldn't leave {name}: {why}.")
}

impl Spaces {
    /// The team facts as last read.
    pub(crate) fn team_facts(&self) -> TeamFacts {
        self.inner.team_facts.lock().expect("team facts").clone()
    }

    /// Reads the team facts again when they are older than
    /// [`TeamFacts::MAX_AGE`] and something needs them: Teams are on, or
    /// the directory lists a machine reached through a team. Best effort
    /// and bounded: a failed read keeps the last answer (team keys stand
    /// in for names) until the next try.
    pub(crate) async fn refresh_team_facts(&self) {
        let Some(relay) = self.relay_account() else {
            return;
        };
        let teams_on = cua_home::experimental::teams_enabled();
        let via_team: Vec<String> = self
            .inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .iter()
            .filter_map(|m| m.team.clone())
            .collect();
        if !teams_on && via_team.is_empty() {
            return;
        }
        if self.inner.team_facts.lock().expect("team facts").fresh() {
            return;
        }
        let read = async {
            let token = relay.tokens.access_token().await?;
            let api = relay.spaces_api()?;
            let me = api.me(&token).await?;
            let mut machines = Vec::new();
            for org in &me.orgs {
                let referenced = via_team.iter().any(|t| t == &org.id || t == &org.alias);
                if !(referenced || (teams_on && org.has("team_machines"))) {
                    continue;
                }
                if let Ok(ms) = api.org_machines(&token, &org.id).await {
                    machines.extend(
                        ms.into_iter()
                            .map(|m| (org.id.clone(), m.machine_id, m.role)),
                    );
                }
            }
            let orgs = me
                .orgs
                .iter()
                .map(|o| (o.id.clone(), o.alias.clone(), o.label().to_string()));
            Ok::<_, Error>(TeamFacts::from_answers(orgs, machines))
        };
        let answer = tokio::time::timeout(Duration::from_secs(5), read).await;
        let mut facts = self.inner.team_facts.lock().expect("team facts");
        match answer {
            Ok(Ok(mut read)) => {
                read.read_at = Some(Instant::now());
                *facts = read;
            }
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "team facts not read");
                facts.read_at = Some(Instant::now());
            }
            Err(_) => {
                tracing::debug!("team facts read timed out");
                facts.read_at = Some(Instant::now());
            }
        }
    }

    /// Takes this account off each Space in `ids` that is shared with it
    /// (through the relay, `POST /v1/machines/{id}/leave`): a direct share
    /// ends, a team's Space leaves this account's list. Your own Spaces are
    /// refused (delete them instead). One result per id, in order; one
    /// failure never stops the rest. A relay that predates leaving answers
    /// "Couldn't leave <name>: the Cua relay needs an update."
    pub async fn leave_many(&self, ids: &[String]) -> Vec<BulkResult> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(match self.leave_one(id).await {
                Ok(message) => BulkResult::ok(id, message),
                Err(e) => BulkResult::failed(id, e),
            });
        }
        out
    }

    async fn leave_one(&self, space: &str) -> std::result::Result<String, String> {
        let name_of = |s: &Self| {
            s.list()
                .ok()
                .and_then(|l| l.into_iter().find(|i| i.id == space || i.name == space))
                .map(|i| i.name)
                .unwrap_or_else(|| space.to_string())
        };
        let id = match self.resolve(space) {
            Ok(id) => id,
            Err(e) => return Err(leave_error(space, &e.to_string())),
        };
        let SpaceId::Relay { machine_id } = &id else {
            return Err(leave_error(&name_of(self), "it's yours; delete it instead"));
        };
        let name = name_of(self);
        let relay = self
            .relay()
            .map_err(|e| leave_error(&name, &e.to_string()))?;
        let token = relay
            .tokens
            .access_token()
            .await
            .map_err(|e| leave_error(&name, &e.to_string()))?;
        let client = relay
            .client()
            .await
            .map_err(|e| leave_error(&name, &e.to_string()))?;
        match client.leave(&token, machine_id).await {
            Ok(_) => {}
            Err(e) if cua_host::relay::is_relay_outdated(&e) => {
                return Err(leave_error(&name, cua_host::relay::RELAY_NEEDS_UPDATE));
            }
            Err(cua_host::Error::PermissionDenied(m)) if m.contains("delete it instead") => {
                return Err(leave_error(&name, "it's yours; delete it instead"));
            }
            Err(cua_host::Error::NotFound(_)) => {
                // Not shared with this account any more: already left.
            }
            Err(e) => return Err(leave_error(&name, &e.to_string())),
        }
        // Gone from this device too: its row, connection and thumbnail.
        self.drop_connection(&id).await;
        self.relay_forget(machine_id);
        let _ = self.inner.registry.remove(&id.to_string());
        self.inner.thumbnails.remove(&id.to_string());
        Ok(format!("Left {name}."))
    }

    /// Deletes each Space in `ids` ([`Spaces::delete`]), at most
    /// [`BULK_CONCURRENCY`] at a time, turning a running one off first.
    /// One result per id, in order; one failure never stops the rest.
    pub async fn delete_many(&self, ids: &[String]) -> Vec<BulkResult> {
        let mut results: Vec<(usize, BulkResult)> =
            futures_util::stream::iter(ids.iter().cloned().enumerate())
                .map(|(i, id)| async move {
                    let r = match self.delete_one(&id).await {
                        Ok(message) => BulkResult::ok(&id, message),
                        Err(e) => BulkResult::failed(&id, e.to_string()),
                    };
                    (i, r)
                })
                .buffer_unordered(BULK_CONCURRENCY)
                .collect()
                .await;
        results.sort_by_key(|(i, _)| *i);
        results.into_iter().map(|(_, r)| r).collect()
    }

    async fn delete_one(&self, space: &str) -> Result<String> {
        let key = self.resolve(space)?.to_string();
        // Running and able to turn off (a local Space, one in your cloud,
        // one your machine provides): turned off first, as the apps say.
        let running = self
            .list()?
            .into_iter()
            .any(|i| i.id == key && !i.power.is_empty() && i.power_state == "running");
        if running && let Err(e) = self.stop(&key).await {
            // Deleting takes it down anyway; the turn-off is a courtesy.
            tracing::debug!(space = %key, error = %e, "not turned off before delete");
        }
        self.delete(&key).await
    }

    /// Fills in [`crate::SpaceInfo::sharing`] for a Space this device
    /// shared (attached to the relay as its own machine): its owner view.
    #[cfg(feature = "spaces-agents")]
    pub(crate) fn apply_attached_sharing(&self, infos: &mut [crate::SpaceInfo]) {
        let attached = self.attached_machines();
        if attached.is_empty() {
            return;
        }
        let facts = self.team_facts();
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        for info in infos.iter_mut().filter(|i| i.sharing.is_none()) {
            if let Some(m) = attached
                .get(&info.id)
                .and_then(|machine| cache.iter().find(|m| &m.id == machine))
            {
                info.sharing = Some(sharing_of(m, &facts));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::RelayMachine;

    fn machine(id: &str, role: &str, team: Option<&str>) -> RelayMachine {
        RelayMachine {
            id: id.into(),
            name: id.into(),
            owner: cua_host::Identity {
                id: "sam".into(),
                email: Some("sam@example.com".into()),
                name: Some("Sam Lee".into()),
            },
            role: role.into(),
            team: team.map(str::to_string),
            ..Default::default()
        }
    }

    fn facts() -> TeamFacts {
        TeamFacts::from_answers(
            [("org-qa".into(), "qa".into(), "QA".into())],
            [
                ("org-qa".into(), "space-viewonly".into(), "viewer".into()),
                ("org-qa".into(), "space-edit".into(), String::new()),
                ("org-qa".into(), "space-mine".into(), "editor".into()),
            ],
        )
    }

    #[test]
    fn a_direct_share_is_an_editor_or_a_viewer_with_the_owner_named() {
        let s = sharing_of(&machine("m1", "shared", None), &TeamFacts::default());
        assert_eq!(s.role, "editor");
        assert_eq!(
            (
                s.owner_id.as_str(),
                s.owner_email.as_str(),
                s.owner_name.as_str()
            ),
            ("sam", "sam@example.com", "Sam Lee")
        );
        assert!(s.team.is_empty() && s.team_name.is_empty());
        let s = sharing_of(&machine("m1", "viewer", None), &TeamFacts::default());
        assert_eq!(s.role, "viewer");
    }

    #[test]
    fn a_team_member_gets_the_teams_name_and_its_real_role() {
        let f = facts();
        let s = sharing_of(&machine("space-viewonly", "shared", Some("org-qa")), &f);
        assert_eq!((s.role.as_str(), s.team_name.as_str()), ("viewer", "QA"));
        // An empty role from an older API reads as editor.
        let s = sharing_of(&machine("space-edit", "shared", Some("org-qa")), &f);
        assert_eq!(s.role, "editor");
        // An alias works as the key too; an unknown team keeps its key.
        assert_eq!(f.name("qa"), "QA");
        let s = sharing_of(&machine("x", "shared", Some("team-abc")), &f);
        assert_eq!(
            (s.role.as_str(), s.team_name.as_str()),
            ("editor", "team-abc")
        );
    }

    #[test]
    fn the_owner_sees_who_has_it_including_teams() {
        let mut m = machine("space-mine", "owner", None);
        m.allow = vec!["priya@example.com".into()];
        m.viewers = vec!["sam@example.com".into()];
        let s = sharing_of(&m, &facts());
        assert_eq!(s.role, "owner");
        assert_eq!(s.editors, vec!["priya@example.com"]);
        assert_eq!(s.viewers, vec!["sam@example.com"]);
        assert_eq!(
            s.teams,
            vec![TeamShare {
                key: "org-qa".into(),
                name: "QA".into(),
                role: "editor".into()
            }]
        );
        // Someone it is shared with does not see the teams.
        assert!(
            sharing_of(&machine("space-mine", "shared", None), &facts())
                .teams
                .is_empty()
        );
    }

    #[test]
    fn leave_errors_read_as_one_sentence() {
        assert_eq!(
            leave_error("qa-shared", "relay: the Cua relay needs an update"),
            "Couldn't leave qa-shared: the Cua relay needs an update."
        );
        assert_eq!(
            leave_error("x", "no such computer."),
            "Couldn't leave x: no such computer."
        );
    }
}
