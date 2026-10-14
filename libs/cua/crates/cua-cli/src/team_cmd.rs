//! `cua team …`: teams on the relay. A team shares machines with every
//! member (and their agents); owners and admins invite by email or link,
//! remove members, add machines they own (Mac, Linux or Windows), approve each member
//! device before it opens team machines, and read the team's audit log.
//!
//! Adding a Space is `cua team add-machine <team> <space>`, the same as
//! `cua spaces share <space> team:<id>` (it attaches the Space to the relay
//! first, and asks you to confirm); see `lib.rs`.

use crate::util::{self, line};
use clap::Subcommand;
use cua_host::{DeviceAuth, Team, TeamRole};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::Path;

#[derive(Subcommand, Debug, Clone)]
pub enum TeamCmd {
    /// Create a team; you are its owner.
    #[command(after_help = "Examples:
  cua team create \"AIGIS\"")]
    Create {
        /// Team name.
        name: String,
    },
    /// Your teams, and email invites waiting for you.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua team ls
  cua team ls --json"
    )]
    Ls,
    /// A team: members, machines, open invites and devices.
    #[command(after_help = "Examples:
  cua team show AIGIS")]
    Show {
        /// Team id (`team-…`) or name.
        team: String,
    },
    /// Invite someone by email, or make a link anyone can join with.
    #[command(after_help = "Examples:
  cua team invite AIGIS dillon@example.com
  cua team invite AIGIS --link
  cua team invite AIGIS ops@example.com --role admin")]
    Invite {
        /// Team id or name.
        team: String,
        /// Their email (they join by signing in with it).
        #[arg(required_unless_present = "link", conflicts_with = "link")]
        email: Option<String>,
        /// Make a link instead (up to 25 joins, 7 days).
        #[arg(long)]
        link: bool,
        /// `member` or `admin` (only the owner invites admins).
        #[arg(long, default_value = "member", value_parser = ["member", "admin"])]
        role: String,
    },
    /// Join a team with an invite link or code, or (with --team) the email
    /// invite for your account's email.
    #[command(after_help = "Examples:
  cua team join https://relay.cua.ai/join/team/team-0a1b….xyz
  cua team join --team team-0a1b2c3d4e5f6a7b")]
    Join {
        /// The invite link or code.
        #[arg(required_unless_present = "team")]
        code: Option<String>,
        /// The team of an email invite.
        #[arg(long, conflicts_with = "code")]
        team: Option<String>,
    },
    /// Leave a team.
    #[command(after_help = "Examples:
  cua team leave AIGIS")]
    Leave {
        /// Team id or name.
        team: String,
    },
    /// Remove a member (owner or admin; admins cannot remove admins).
    #[command(after_help = "Examples:
  cua team remove-member AIGIS bob@example.com")]
    RemoveMember {
        /// Team id or name.
        team: String,
        /// Their email or account id.
        who: String,
    },
    /// Make a member an admin, or an admin a member (owner).
    #[command(after_help = "Examples:
  cua team set-role AIGIS bob@example.com admin")]
    SetRole {
        /// Team id or name.
        team: String,
        /// Their email or account id.
        who: String,
        /// `member` or `admin`.
        #[arg(value_parser = ["member", "admin"])]
        role: String,
    },
    /// Member devices and whether they may open team machines.
    #[command(after_help = "Examples:
  cua team devices AIGIS")]
    Devices {
        /// Team id or name.
        team: String,
    },
    /// Approve a member device so it can open team machines.
    #[command(after_help = "Examples:
  cua team approve AIGIS dev_0123456789abcdef01234567")]
    Approve {
        /// Team id or name.
        team: String,
        /// Device id (`cua team devices`).
        device: String,
    },
    /// Decline a member device (it can no longer open team machines).
    #[command(after_help = "Examples:
  cua team deny AIGIS dev_0123456789abcdef01234567")]
    Deny {
        /// Team id or name.
        team: String,
        /// Device id.
        device: String,
    },
    /// Ask the team's admins to approve this device.
    #[command(after_help = "Examples:
  cua team request AIGIS")]
    Request {
        /// Team id or name.
        team: String,
    },
    /// Make a Space (or a machine you host) a team machine: every member
    /// and their agents see it and can open it. Macs, Linux and Windows machines all can.
    #[command(after_help = "Examples:
  cua team add-machine AIGIS local:build-box
  cua team add-machine AIGIS relay:0123abcd4567ef89 --yes")]
    AddMachine {
        /// Team id or name.
        team: String,
        /// Space id or name.
        space: String,
        /// Confirm without a prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Take a machine off the team.
    #[command(after_help = "Examples:
  cua team remove-machine AIGIS relay:0123abcd4567ef89")]
    RemoveMachine {
        /// Team id or name.
        team: String,
        /// The machine (`relay:<id>` or its id, as `cua team show` lists it).
        machine: String,
    },
    /// The team's audit log: joins, approvals, machines opened, changes.
    #[command(after_help = "Examples:
  cua team audit AIGIS --limit 50")]
    Audit {
        /// Team id or name.
        team: String,
        /// Newest events to show.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Delete a team (owner). Its machines stay yours.
    #[command(after_help = "Examples:
  cua team delete AIGIS --yes")]
    Delete {
        /// Team id or name.
        team: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

impl TeamCmd {
    /// The team argument, if the command has one.
    pub fn team_arg(&self) -> Option<&str> {
        match self {
            TeamCmd::Show { team }
            | TeamCmd::Invite { team, .. }
            | TeamCmd::Leave { team }
            | TeamCmd::RemoveMember { team, .. }
            | TeamCmd::SetRole { team, .. }
            | TeamCmd::Devices { team }
            | TeamCmd::Approve { team, .. }
            | TeamCmd::Deny { team, .. }
            | TeamCmd::Request { team }
            | TeamCmd::AddMachine { team, .. }
            | TeamCmd::RemoveMachine { team, .. }
            | TeamCmd::Audit { team, .. }
            | TeamCmd::Delete { team, .. } => Some(team),
            TeamCmd::Create { .. } | TeamCmd::Ls | TeamCmd::Join { .. } => None,
        }
    }
}

fn err(e: cua_host::Error) -> CuaError {
    crate::host::host_err(e)
}

/// The team id `query` names: an id as is, else the one team of yours
/// with that name (case-insensitive).
pub async fn resolve_team(auth: &DeviceAuth, query: &str) -> Result<String, CuaError> {
    let q = query.trim();
    let q = q.strip_prefix("team:").unwrap_or(q);
    if q.starts_with("team-") {
        return Ok(q.to_string());
    }
    let list = auth.teams().await.map_err(err)?;
    let matches: Vec<_> = list
        .teams
        .iter()
        .filter(|t| t.name.eq_ignore_ascii_case(q))
        .collect();
    match matches.as_slice() {
        [t] => Ok(t.id.clone()),
        [] => Err(CuaError::NotFound(format!(
            "no team of yours is called {q:?} (`cua team ls`)"
        ))),
        _ => Err(CuaError::InvalidArgument(format!(
            "several of your teams are called {q:?}: use the team id (`cua team ls`)"
        ))),
    }
}

fn when(ts: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts as i64, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "-".into())
}

fn emit_json(out: &mut dyn Write, v: &impl serde::Serialize) {
    line(out, serde_json::to_string(v).unwrap_or_default());
}

fn print_team(t: &Team, out: &mut dyn Write) {
    line(
        out,
        format!("{} ({}), you are {}", t.name, t.id, t.role.as_str()),
    );
    line(out, format!("Members ({}):", t.members.len()));
    for m in &t.members {
        line(out, format!("  {}  {}", m.label(), m.role.as_str()));
    }
    line(out, format!("Machines ({}):", t.machines.len()));
    if t.machines.is_empty() {
        line(
            out,
            "  none yet: `cua team add-machine <team> <space>` (owner or admin)",
        );
    }
    for m in &t.machines {
        line(
            out,
            format!(
                "  relay:{}  {}  (owner {}{})",
                m.id,
                if m.name.is_empty() { "-" } else { &m.name },
                m.owner,
                if m.online { ", online" } else { ", offline" }
            ),
        );
    }
    if !t.invites.is_empty() {
        line(out, "Open invites:");
        for i in &t.invites {
            line(
                out,
                format!(
                    "  {}  {}  as {}, expires {}",
                    i.id,
                    i.email.as_deref().unwrap_or("link"),
                    i.role.as_str(),
                    when(i.expires_at)
                ),
            );
        }
    }
    print_devices(t, out);
    if let Some(state) = &t.this_device {
        line(out, format!("This device: {state}"));
    }
}

fn print_devices(t: &Team, out: &mut dyn Write) {
    if t.devices.is_empty() {
        return;
    }
    line(out, "Devices:");
    for d in &t.devices {
        line(
            out,
            format!(
                "  {}  {}  {}{}",
                d.device,
                d.email.as_deref().unwrap_or(&d.account),
                d.state,
                if d.name.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", d.name)
                }
            ),
        );
    }
    if t.role.manages() && t.devices.iter().any(|d| d.state == "pending") {
        line(
            out,
            format!("Approve one with `cua team approve {} <device>`.", t.id),
        );
    }
}

/// Runs `cmd` (all but `add-machine`, which goes through Spaces) for this
/// device. `confirm` answers the delete prompt.
pub async fn run_with(
    cmd: TeamCmd,
    auth: &DeviceAuth,
    confirm: &dyn Fn(&str) -> bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let team = match cmd.team_arg() {
        Some(q) if !matches!(cmd, TeamCmd::AddMachine { .. }) => Some(resolve_team(auth, q).await?),
        _ => None,
    };
    let id = || team.clone().unwrap_or_default();
    match cmd {
        TeamCmd::Create { name } => {
            let t = auth.create_team(&name).await.map_err(err)?;
            if json {
                emit_json(out, &t);
            } else {
                line(out, format!("Created team {} ({}).", t.name, t.id));
                line(out, "Next:");
                line(
                    out,
                    format!("  cua team invite {} <email>   (or --link)", t.id),
                );
                line(out, format!("  cua team add-machine {} <space>", t.id));
            }
        }
        TeamCmd::Ls => {
            let l = auth.teams().await.map_err(err)?;
            if json {
                emit_json(out, &l);
                return Ok(0);
            }
            if l.teams.is_empty() {
                line(
                    out,
                    "You are in no team. Create one with `cua team create <name>`.",
                );
            }
            for t in &l.teams {
                line(
                    out,
                    format!(
                        "{}  {}  {}  {} members, {} machines{}",
                        t.id,
                        t.name,
                        t.role.as_str(),
                        t.members.len(),
                        t.machines.len(),
                        match t.this_device.as_deref() {
                            Some("approved") | None => String::new(),
                            Some(s) => format!(", this device {s}"),
                        }
                    ),
                );
            }
            for i in &l.invites {
                line(
                    out,
                    format!(
                        "Invited to {} ({}) as {}: `cua team join --team {}`",
                        i.team_name,
                        i.team,
                        i.invite.role.as_str(),
                        i.team
                    ),
                );
            }
        }
        TeamCmd::Show { .. } => {
            let t = auth.team(&id()).await.map_err(err)?;
            if json {
                emit_json(out, &t);
            } else {
                print_team(&t, out);
            }
        }
        TeamCmd::Invite {
            email, link, role, ..
        } => {
            let role = TeamRole::parse(&role).map_err(err)?;
            let email = if link { None } else { email };
            let n = auth
                .invite_to_team(&id(), email.as_deref(), role)
                .await
                .map_err(err)?;
            if json {
                emit_json(out, &n);
            } else {
                match &n.invite.email {
                    Some(e) => {
                        line(
                            out,
                            format!(
                                "Invited {e} as {}. Send them this link (it works only when they sign in as {e}), expires {}:",
                                role.as_str(),
                                when(n.invite.expires_at)
                            ),
                        );
                    }
                    None => line(
                        out,
                        format!(
                            "Invite link ({} joins, as {}, expires {}). Anyone with it can join; admins then approve their device:",
                            n.invite.uses_left,
                            role.as_str(),
                            when(n.invite.expires_at)
                        ),
                    ),
                }
                line(out, format!("  {}", n.link));
                line(out, format!("They run: cua team join {}", n.link));
            }
        }
        TeamCmd::Join { code, team } => {
            let t = auth
                .join_team(code.as_deref(), team.as_deref())
                .await
                .map_err(err)?;
            if json {
                emit_json(out, &t);
            } else {
                line(
                    out,
                    format!("You joined {} ({}) as {}.", t.name, t.id, t.role.as_str()),
                );
                match t.this_device.as_deref() {
                    Some("approved") => line(out, "This device can open the team's machines."),
                    Some(_) => line(
                        out,
                        "This device waits for a team admin's approval before it can open the team's machines.",
                    ),
                    None => line(
                        out,
                        format!(
                            "Enroll this device (`cua devices enroll`), then ask for approval: `cua team request {}`.",
                            t.id
                        ),
                    ),
                }
            }
        }
        TeamCmd::Leave { .. } => {
            auth.leave_team(&id()).await.map_err(err)?;
            if !json {
                line(out, format!("You left {}.", id()));
            }
        }
        TeamCmd::RemoveMember { who, .. } => {
            auth.remove_team_member(&id(), &who).await.map_err(err)?;
            if !json {
                line(
                    out,
                    format!("Removed {who}: their open sessions on team machines ended."),
                );
            }
        }
        TeamCmd::SetRole { who, role, .. } => {
            let role = TeamRole::parse(&role).map_err(err)?;
            let m = auth.set_team_role(&id(), &who, role).await.map_err(err)?;
            if json {
                emit_json(out, &m);
            } else {
                line(out, format!("{} is now {}.", m.label(), m.role.as_str()));
            }
        }
        TeamCmd::Devices { .. } => {
            let t = auth.team(&id()).await.map_err(err)?;
            if json {
                emit_json(out, &t.devices);
            } else if t.devices.is_empty() {
                line(out, "No member device has asked yet.");
            } else {
                print_devices(&t, out);
            }
        }
        TeamCmd::Approve { device, .. } => decide(auth, &id(), &device, true, json, out).await?,
        TeamCmd::Deny { device, .. } => decide(auth, &id(), &device, false, json, out).await?,
        TeamCmd::Request { .. } => {
            let d = auth.request_team_device(&id()).await.map_err(err)?;
            if json {
                emit_json(out, &d);
            } else {
                line(out, format!("This device ({}): {}.", d.device, d.state));
            }
        }
        TeamCmd::AddMachine { .. } => {
            return Err(CuaError::Internal("add-machine runs through Spaces".into()));
        }
        TeamCmd::RemoveMachine { machine, .. } => {
            let m = machine.trim();
            let m = m.strip_prefix("relay:").unwrap_or(m);
            let t = auth.remove_team_machine(&id(), m).await.map_err(err)?;
            if json {
                emit_json(out, &t);
            } else {
                line(
                    out,
                    format!("relay:{m} is no longer a machine of {}.", t.name),
                );
            }
        }
        TeamCmd::Audit { limit, .. } => {
            let events = auth.team_audit(&id(), limit).await.map_err(err)?;
            if json {
                emit_json(out, &events);
                return Ok(0);
            }
            for e in &events {
                let mut parts = vec![when(e.ts), e.kind.clone(), e.actor.clone()];
                parts.extend(e.subject.clone());
                parts.extend(e.machine.as_ref().map(|m| format!("relay:{m}")));
                parts.extend(e.detail.clone());
                parts.extend(e.device.as_ref().map(|d| format!("from {d}")));
                line(out, parts.join("  "));
            }
        }
        TeamCmd::Delete { yes, .. } => {
            if !yes && !confirm(&format!("Delete team {}? Members lose its machines.", id())) {
                return Err(CuaError::InvalidArgument(
                    "not deleted (pass --yes when not in a terminal)".into(),
                ));
            }
            auth.delete_team(&id()).await.map_err(err)?;
            if !json {
                line(out, format!("Deleted {}.", id()));
            }
        }
    }
    Ok(0)
}

async fn decide(
    auth: &DeviceAuth,
    team: &str,
    device: &str,
    approve: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), CuaError> {
    let d = auth
        .decide_team_device(team, device.trim(), approve)
        .await
        .map_err(err)?;
    if json {
        emit_json(out, &d);
    } else if approve {
        line(
            out,
            format!(
                "Approved {} ({}): it can open the team's machines.",
                d.device,
                d.email.as_deref().unwrap_or(&d.account)
            ),
        );
    } else {
        line(out, format!("Declined {}.", d.device));
    }
    Ok(())
}

/// `cua team …` (all but `add-machine`).
pub async fn run(
    cmd: TeamCmd,
    home: &Path,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let auth = crate::devices_cmd::device_auth(&crate::host::relay_url(None, home))?;
    run_with(
        cmd,
        &auth,
        &|q| util::interactive() && util::confirm(q, false),
        json,
        out,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_host::testing::FakeRelay;
    use cua_host::{MemoryKeySlot, StaticToken};
    use std::sync::Arc;

    async fn enrolled(relay: &FakeRelay, token: &str, account: &str) -> DeviceAuth {
        relay.fresh_sign_in(account);
        let auth = DeviceAuth::new(
            &relay.url,
            Arc::new(StaticToken(token.into())),
            Arc::new(MemoryKeySlot::default()),
            format!("{account} laptop"),
        )
        .unwrap()
        .with_machine_id(Some(format!("machine-of-{account}")));
        auth.enroll().await.unwrap();
        auth
    }

    async fn text(cmd: TeamCmd, auth: &DeviceAuth) -> Result<String, CuaError> {
        let mut out = Vec::new();
        run_with(cmd, auth, &|_| true, false, &mut out).await?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[tokio::test]
    async fn create_invite_join_approve_and_audit() {
        let relay = FakeRelay::start().await;
        relay.add_account("t-ada", "ada", Some("ada@aigis.test"));
        relay.add_account("t-bob", "bob", Some("bob@aigis.test"));
        relay.require_devices(true);
        let ada = enrolled(&relay, "t-ada", "ada").await;
        let bob = enrolled(&relay, "t-bob", "bob").await;

        let t = text(
            TeamCmd::Create {
                name: "AIGIS".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains("Created team AIGIS (team-"), "{t}");
        // Teams resolve by name.
        let t = text(
            TeamCmd::Invite {
                team: "aigis".into(),
                email: None,
                link: true,
                role: "member".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        let link = t
            .lines()
            .find_map(|l| l.trim().strip_prefix("They run: cua team join "))
            .expect("a join line")
            .to_string();
        assert!(link.contains("/join/team/"), "{link}");

        let t = text(
            TeamCmd::Join {
                code: Some(link),
                team: None,
            },
            &bob,
        )
        .await
        .unwrap();
        assert!(t.contains("You joined AIGIS"), "{t}");
        assert!(t.contains("waits for a team admin's approval"), "{t}");

        // A member cannot approve; the owner sees the device and approves.
        let bob_device = bob.device_id().unwrap().unwrap();
        let refused = text(
            TeamCmd::Approve {
                team: "AIGIS".into(),
                device: bob_device.clone(),
            },
            &bob,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(refused, CuaError::PermissionDenied(_)),
            "{refused:?}"
        );
        let t = text(
            TeamCmd::Devices {
                team: "AIGIS".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains(&bob_device) && t.contains("pending"), "{t}");
        let t = text(
            TeamCmd::Approve {
                team: "AIGIS".into(),
                device: bob_device.clone(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains("Approved"), "{t}");

        let t = text(TeamCmd::Ls, &bob).await.unwrap();
        assert!(t.contains("AIGIS  member"), "{t}");
        let t = text(
            TeamCmd::Audit {
                team: "AIGIS".into(),
                limit: 50,
            },
            &ada,
        )
        .await
        .unwrap();
        for k in [
            "team_created",
            "invite_created",
            "member_joined",
            "device_approved",
        ] {
            assert!(t.contains(k), "{k} in {t}");
        }
        let e = text(
            TeamCmd::Show {
                team: "nope".into(),
            },
            &ada,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, CuaError::NotFound(_)), "{e:?}");
    }
}
