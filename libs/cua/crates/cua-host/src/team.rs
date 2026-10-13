//! Teams on the relay: a named group of cua.ai accounts with roles
//! (`owner`, `admin`, `member`) that shares machines with every member.
//! Owners and admins invite (by email or link), remove members, add
//! machines they own, approve member devices (a member device opens team
//! machines only once approved) and read the team's audit log. macOS
//! Spaces cannot be team machines (one user per macOS copy).
//!
//! [`RelayClient`] has the raw calls (account token, from an enrolled
//! device's session); [`DeviceAuth`] wraps them with this device's session.

use crate::Result;
use crate::device::DeviceAuth;
use crate::relay::RelayClient;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A member's role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    #[default]
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

    /// Parses `member` / `admin`.
    pub fn parse(s: &str) -> Result<TeamRole> {
        match s.trim().to_ascii_lowercase().as_str() {
            "member" => Ok(TeamRole::Member),
            "admin" => Ok(TeamRole::Admin),
            other => Err(crate::Error::InvalidArgument(format!(
                "team role {other:?}: use member or admin"
            ))),
        }
    }

    /// Owners and admins manage the team.
    pub fn manages(self) -> bool {
        matches!(self, TeamRole::Owner | TeamRole::Admin)
    }
}

/// One member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub role: TeamRole,
    #[serde(default)]
    pub joined_at: u64,
}

impl TeamMember {
    /// Email when known, else the account id.
    pub fn label(&self) -> &str {
        self.email.as_deref().unwrap_or(&self.account)
    }
}

/// A team machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMachine {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The owner's email (or account id).
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub online: bool,
}

/// An open invite (never its code).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInvite {
    pub id: String,
    /// Only this email may use it; a link invite otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub role: TeamRole,
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub uses_left: u32,
}

/// A member device and its approval: `pending`, `approved`, `denied`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDevice {
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub device: String,
    #[serde(default)]
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub requested_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// A team as one of its members sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    /// The caller's role.
    pub role: TeamRole,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub members: Vec<TeamMember>,
    #[serde(default)]
    pub machines: Vec<TeamMachine>,
    /// Open invites (owner/admin view).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invites: Vec<TeamInvite>,
    /// Member devices (owner/admin: all; member: their own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<TeamDevice>,
    /// This device's approval (`pending`, `approved`, `denied`), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub this_device: Option<String>,
}

/// An email invite waiting for the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTeamInvite {
    pub team: String,
    #[serde(default)]
    pub team_name: String,
    pub invite: TeamInvite,
}

/// `GET /v1/teams`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamList {
    #[serde(default)]
    pub teams: Vec<Team>,
    /// Email invites for the caller's verified email.
    #[serde(default)]
    pub invites: Vec<PendingTeamInvite>,
}

/// A new invite: its code and link are shown once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTeamInvite {
    pub invite: TeamInvite,
    pub code: String,
    pub link: String,
}

/// One team audit line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAuditEvent {
    pub ts: u64,
    pub kind: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Deserialize)]
struct AuditList {
    #[serde(default)]
    events: Vec<TeamAuditEvent>,
}

/// The invite code in a pasted link (`…/join/team/<code>`) or the code
/// itself.
pub fn team_code_from_link(s: &str) -> String {
    let s = s.trim();
    s.rsplit_once("/join/team/")
        .map(|(_, c)| c)
        .unwrap_or(s)
        .trim_end_matches('/')
        .to_string()
}

fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

impl RelayClient {
    /// Your teams and the email invites waiting for you.
    pub async fn teams(&self, token: &str) -> Result<TeamList> {
        self.send(self.http.get(self.url("/v1/teams")), token).await
    }

    /// One team you are in.
    pub async fn team(&self, token: &str, id: &str) -> Result<Team> {
        self.send(
            self.http.get(self.url(&format!("/v1/teams/{}", seg(id)))),
            token,
        )
        .await
    }

    /// Creates a team; you are its owner.
    pub async fn create_team(&self, token: &str, name: &str) -> Result<Team> {
        self.send(
            self.http
                .post(self.url("/v1/teams"))
                .json(&json!({ "name": name })),
            token,
        )
        .await
    }

    /// Deletes a team (owner).
    pub async fn delete_team(&self, token: &str, id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .delete(self.url(&format!("/v1/teams/{}", seg(id)))),
                token,
            )
            .await?;
        Ok(())
    }

    /// Joins with an invite `code` (or link), or, with `team` and no code,
    /// the email invite for your verified email.
    pub async fn join_team(
        &self,
        token: &str,
        code: Option<&str>,
        team: Option<&str>,
    ) -> Result<Team> {
        self.send(
            self.http.post(self.url("/v1/teams/join")).json(&json!({
                "code": code.map(team_code_from_link),
                "team": team,
            })),
            token,
        )
        .await
    }

    /// Leaves a team.
    pub async fn leave_team(&self, token: &str, id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .post(self.url(&format!("/v1/teams/{}/leave", seg(id))))
                    .json(&json!({})),
                token,
            )
            .await?;
        Ok(())
    }

    /// Invites `email` (or, without one, makes a link) as `role`.
    pub async fn invite_to_team(
        &self,
        token: &str,
        id: &str,
        email: Option<&str>,
        role: TeamRole,
    ) -> Result<NewTeamInvite> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/teams/{}/invites", seg(id))))
                .json(&json!({ "email": email, "role": role })),
            token,
        )
        .await
    }

    /// Revokes an open invite.
    pub async fn revoke_team_invite(&self, token: &str, id: &str, invite: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                self.http.delete(self.url(&format!(
                    "/v1/teams/{}/invites/{}",
                    seg(id),
                    seg(invite)
                ))),
                token,
            )
            .await?;
        Ok(())
    }

    /// Makes a member an admin or a member (owner).
    pub async fn set_team_role(
        &self,
        token: &str,
        id: &str,
        who: &str,
        role: TeamRole,
    ) -> Result<TeamMember> {
        self.send(
            self.http
                .patch(self.url(&format!("/v1/teams/{}/members/{}", seg(id), seg(who))))
                .json(&json!({ "role": role })),
            token,
        )
        .await
    }

    /// Removes a member (email or account id).
    pub async fn remove_team_member(&self, token: &str, id: &str, who: &str) -> Result<()> {
        let _: serde_json::Value = self
            .send(
                self.http
                    .delete(self.url(&format!("/v1/teams/{}/members/{}", seg(id), seg(who)))),
                token,
            )
            .await?;
        Ok(())
    }

    /// Asks the team's admins to approve this device.
    pub async fn request_team_device(&self, token: &str, id: &str) -> Result<TeamDevice> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/teams/{}/devices/request", seg(id))))
                .json(&json!({})),
            token,
        )
        .await
    }

    /// Approves (or denies) a member device (owner/admin).
    pub async fn decide_team_device(
        &self,
        token: &str,
        id: &str,
        device: &str,
        approve: bool,
    ) -> Result<TeamDevice> {
        self.send(
            self.http
                .post(self.url(&format!(
                    "/v1/teams/{}/devices/{}/{}",
                    seg(id),
                    seg(device),
                    if approve { "approve" } else { "deny" }
                )))
                .json(&json!({})),
            token,
        )
        .await
    }

    /// Adds a relay machine you own to the team (owner/admin). `os` is
    /// what the Space runs, when known (`linux`, `macos`, `windows`).
    pub async fn add_team_machine(
        &self,
        token: &str,
        id: &str,
        machine: &str,
        os: Option<&str>,
    ) -> Result<Team> {
        self.send(
            self.http
                .post(self.url(&format!("/v1/teams/{}/machines", seg(id))))
                .json(&json!({ "machine": machine, "os": os })),
            token,
        )
        .await
    }

    /// Takes a machine off the team.
    pub async fn remove_team_machine(&self, token: &str, id: &str, machine: &str) -> Result<Team> {
        self.send(
            self.http
                .delete(self.url(&format!("/v1/teams/{}/machines/{}", seg(id), seg(machine)))),
            token,
        )
        .await
    }

    /// The team's audit log, newest last (owner/admin).
    pub async fn team_audit(
        &self,
        token: &str,
        id: &str,
        limit: usize,
    ) -> Result<Vec<TeamAuditEvent>> {
        let l: AuditList = self
            .send(
                self.http
                    .get(self.url(&format!("/v1/teams/{}/audit?limit={limit}", seg(id)))),
                token,
            )
            .await?;
        Ok(l.events)
    }
}

/// Every team call from this enrolled device (its session, renewed once if
/// the relay forgot it).
macro_rules! with_device {
    ($self:ident, [$($v:ident),*], |$relay:ident, $token:ident| $call:expr) => {
        $self
            .with_session_retry(move |$relay: RelayClient, $token: String| {
                $(let $v = $v.clone();)*
                async move { $call }
            })
            .await
    };
}

impl DeviceAuth {
    /// Your teams and the email invites waiting for you.
    pub async fn teams(&self) -> Result<TeamList> {
        with_device!(self, [], |r, t| r.teams(&t).await)
    }

    /// One team.
    pub async fn team(&self, id: &str) -> Result<Team> {
        let id = id.to_owned();
        with_device!(self, [id], |r, t| r.team(&t, &id).await)
    }

    /// Creates a team.
    pub async fn create_team(&self, name: &str) -> Result<Team> {
        let name = name.to_owned();
        with_device!(self, [name], |r, t| r.create_team(&t, &name).await)
    }

    /// Deletes a team (owner).
    pub async fn delete_team(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        with_device!(self, [id], |r, t| r.delete_team(&t, &id).await)
    }

    /// Joins with a code or link, or the email invite of `team`.
    pub async fn join_team(&self, code: Option<&str>, team: Option<&str>) -> Result<Team> {
        let (code, team) = (code.map(str::to_owned), team.map(str::to_owned));
        with_device!(self, [code, team], |r, t| r
            .join_team(&t, code.as_deref(), team.as_deref())
            .await)
    }

    /// Leaves a team.
    pub async fn leave_team(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        with_device!(self, [id], |r, t| r.leave_team(&t, &id).await)
    }

    /// Invites by email, or makes a link without one.
    pub async fn invite_to_team(
        &self,
        id: &str,
        email: Option<&str>,
        role: TeamRole,
    ) -> Result<NewTeamInvite> {
        let (id, email) = (id.to_owned(), email.map(str::to_owned));
        with_device!(self, [id, email], |r, t| r
            .invite_to_team(&t, &id, email.as_deref(), role)
            .await)
    }

    /// Revokes an invite.
    pub async fn revoke_team_invite(&self, id: &str, invite: &str) -> Result<()> {
        let (id, invite) = (id.to_owned(), invite.to_owned());
        with_device!(self, [id, invite], |r, t| r
            .revoke_team_invite(&t, &id, &invite)
            .await)
    }

    /// Changes a member's role (owner).
    pub async fn set_team_role(&self, id: &str, who: &str, role: TeamRole) -> Result<TeamMember> {
        let (id, who) = (id.to_owned(), who.to_owned());
        with_device!(self, [id, who], |r, t| r
            .set_team_role(&t, &id, &who, role)
            .await)
    }

    /// Removes a member.
    pub async fn remove_team_member(&self, id: &str, who: &str) -> Result<()> {
        let (id, who) = (id.to_owned(), who.to_owned());
        with_device!(self, [id, who], |r, t| r
            .remove_team_member(&t, &id, &who)
            .await)
    }

    /// Asks for this device's approval.
    pub async fn request_team_device(&self, id: &str) -> Result<TeamDevice> {
        let id = id.to_owned();
        with_device!(self, [id], |r, t| r.request_team_device(&t, &id).await)
    }

    /// Approves or denies a member device.
    pub async fn decide_team_device(
        &self,
        id: &str,
        device: &str,
        approve: bool,
    ) -> Result<TeamDevice> {
        let (id, device) = (id.to_owned(), device.to_owned());
        with_device!(self, [id, device], |r, t| r
            .decide_team_device(&t, &id, &device, approve)
            .await)
    }

    /// Adds a relay machine you own (see [`RelayClient::add_team_machine`]).
    pub async fn add_team_machine(
        &self,
        id: &str,
        machine: &str,
        os: Option<&str>,
    ) -> Result<Team> {
        let (id, machine, os) = (id.to_owned(), machine.to_owned(), os.map(str::to_owned));
        with_device!(self, [id, machine, os], |r, t| r
            .add_team_machine(&t, &id, &machine, os.as_deref())
            .await)
    }

    /// Takes a machine off the team.
    pub async fn remove_team_machine(&self, id: &str, machine: &str) -> Result<Team> {
        let (id, machine) = (id.to_owned(), machine.to_owned());
        with_device!(self, [id, machine], |r, t| r
            .remove_team_machine(&t, &id, &machine)
            .await)
    }

    /// The team's audit log.
    pub async fn team_audit(&self, id: &str, limit: usize) -> Result<Vec<TeamAuditEvent>> {
        let id = id.to_owned();
        with_device!(self, [id], |r, t| r.team_audit(&t, &id, limit).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_roles() {
        assert_eq!(
            team_code_from_link(" https://relay.cua.ai/join/team/team-ab.cd_EF/ "),
            "team-ab.cd_EF"
        );
        assert_eq!(team_code_from_link("team-ab.cd"), "team-ab.cd");
        assert_eq!(TeamRole::parse("Admin").unwrap(), TeamRole::Admin);
        assert!(TeamRole::parse("owner").is_err());
        assert_eq!(seg("bob+x@ex.com"), "bob%2Bx%40ex.com");
    }
}
