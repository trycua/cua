//! `cua team …`: Teams. A team is an organization of your cua.ai account
//! (the Spaces API, `https://api.cua.ai`); its id is the organization id
//! `cua team ls` shows. A team shares machines with every member (and
//! their agents); owners and admins invite by email, remove members, add
//! machines they own (Mac, Linux or Windows) and approve each member
//! device before it opens team machines.
//!
//! Adding a Space is `cua team add-machine <team> <space>`, the same as
//! `cua spaces share <space> team:<id>` (it attaches the Space to the relay
//! first, and asks you to confirm); see `lib.rs`. The relay's old
//! `team-…` teams are not used: a server without Teams says so.

use crate::util::{self, hint_line, line};
use clap::Subcommand;
use cua_host::AccountTokens;
use cua_host::spaces_api::{
    ApiDeviceApproval, ApiOrg, FINISHING_SETUP, SETUP_RETRY_WINDOW, SpacesApiClient,
    is_teams_unavailable,
};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::Path;

#[derive(Subcommand, Debug, Clone)]
pub enum TeamCmd {
    /// Create a team; you are its owner. Buy Teams for it at app.cua.ai
    /// to add computers.
    #[command(after_help = "Examples:
  cua team create \"AIGIS\"")]
    Create {
        /// Team name.
        name: String,
    },
    /// Your teams.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua team ls
  cua team ls --json"
    )]
    Ls,
    /// A team: plan, members, computers, open invites and devices.
    #[command(after_help = "Examples:
  cua team show AIGIS")]
    Show {
        /// Team id, alias or name.
        team: String,
    },
    /// Invite someone by email: cua.ai emails them an invitation. Manage
    /// invites at app.cua.ai/team.
    #[command(after_help = "Examples:
  cua team invite AIGIS dillon@example.com
  cua team invite AIGIS ops@example.com --role admin")]
    Invite {
        /// Team id, alias or name.
        team: String,
        /// Their email.
        email: String,
        /// `member` or `admin` (only the owner invites admins).
        #[arg(long, default_value = "member", value_parser = ["member", "admin"])]
        role: String,
    },
    /// After accepting an emailed invitation: take the role it offered.
    /// Run `cua auth login` first so your session lists the team.
    #[command(after_help = "Examples:
  cua team join AIGIS")]
    Join {
        /// Team id, alias or name.
        team: String,
    },
    /// Leave a team.
    #[command(after_help = "Examples:
  cua team leave AIGIS")]
    Leave {
        /// Team id, alias or name.
        team: String,
    },
    /// Remove a member (owner or admin; admins cannot remove admins).
    #[command(after_help = "Examples:
  cua team remove-member AIGIS bob@example.com")]
    RemoveMember {
        /// Team id, alias or name.
        team: String,
        /// Their email or account id.
        who: String,
    },
    /// Make a member an admin, or an admin a member (owner).
    #[command(after_help = "Examples:
  cua team set-role AIGIS bob@example.com admin")]
    SetRole {
        /// Team id, alias or name.
        team: String,
        /// Their email or account id.
        who: String,
        /// `member` or `admin`.
        #[arg(value_parser = ["member", "admin"])]
        role: String,
    },
    /// Member devices and whether they may open team computers.
    #[command(after_help = "Examples:
  cua team devices AIGIS")]
    Devices {
        /// Team id, alias or name.
        team: String,
    },
    /// Approve a member device so it can open team computers.
    #[command(after_help = "Examples:
  cua team approve AIGIS dev_0123456789abcdef01234567")]
    Approve {
        /// Team id, alias or name.
        team: String,
        /// Device id (`cua team devices`).
        device: String,
    },
    /// Decline a member device (it can no longer open team computers).
    #[command(after_help = "Examples:
  cua team deny AIGIS dev_0123456789abcdef01234567")]
    Deny {
        /// Team id, alias or name.
        team: String,
        /// Device id.
        device: String,
    },
    /// Ask the team's admins to approve this device.
    #[command(after_help = "Examples:
  cua team request AIGIS")]
    Request {
        /// Team id, alias or name.
        team: String,
    },
    /// Make a Space (or a computer you host) a team computer: every member
    /// and their agents see it and can open it. Macs, Linux and Windows
    /// computers all can. Each Space is added on its own: adding a host does
    /// not add the Spaces it provides.
    #[command(after_help = "Examples:
  cua team add-machine AIGIS local:build-box
  cua team add-machine AIGIS relay:0123abcd4567ef89 --yes")]
    AddMachine {
        /// Team id, alias or name.
        team: String,
        /// Space id or name (`relay:<id>` for a computer you host).
        space: String,
        /// Confirm without a prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Take a computer off the team.
    #[command(after_help = "Examples:
  cua team remove-machine AIGIS relay:0123abcd4567ef89")]
    RemoveMachine {
        /// Team id, alias or name.
        team: String,
        /// The computer (`relay:<id>` or its id, as `cua team show` lists it).
        #[arg(value_name = "COMPUTER")]
        machine: String,
    },
    /// The team's audit log. Audit export is coming soon; for now see the
    /// account app (app.cua.ai).
    #[command(after_help = "Examples:
  cua team audit AIGIS")]
    Audit {
        /// Team id, alias or name.
        team: String,
        /// Newest events to show (when export arrives).
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Delete a team (owner; cancel its subscription first). Its computers
    /// stay yours.
    #[command(after_help = "Examples:
  cua team delete AIGIS --yes")]
    Delete {
        /// Team id, alias or name.
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
            | TeamCmd::Join { team }
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
            TeamCmd::Create { .. } | TeamCmd::Ls => None,
        }
    }
}

/// A Teams error as the CLI reports it: a server without Teams is
/// `unsupported`, with the words the apps use too.
pub fn err(e: cua_host::Error) -> CuaError {
    if is_teams_unavailable(&e) {
        return CuaError::Unsupported(e.to_string());
    }
    if cua_host::spaces_api::is_feature_required(&e) {
        return CuaError::PermissionDenied(format!(
            "this team's plan does not include that yet: finish the Teams checkout at app.cua.ai, then try again ({e})"
        ));
    }
    crate::host::host_err(e)
}

/// What a team command runs with: the Spaces API, the account token and
/// (for `request`) this device's relay id.
pub struct TeamCtx {
    pub api: SpacesApiClient,
    pub token: String,
    pub device: Option<String>,
}

impl TeamCtx {
    /// For the signed-in account (`cua auth login`) in this environment.
    pub async fn current(home: &Path, with_device: bool) -> Result<Self, CuaError> {
        let api = SpacesApiClient::current().map_err(err)?;
        let token = crate::host::SessionTokens
            .access_token()
            .await
            .map_err(err)?;
        let device = if with_device {
            let auth = crate::devices_cmd::device_auth(&crate::host::relay_url(None, home))?;
            auth.device_id().map_err(err)?
        } else {
            None
        };
        Ok(Self { api, token, device })
    }
}

/// The organization id `query` (an id, alias or name) names.
pub async fn resolve_team(home: &Path, query: &str) -> Result<String, CuaError> {
    let ctx = TeamCtx::current(home, false).await?;
    Ok(ctx
        .api
        .resolve_org(&ctx.token, query)
        .await
        .map_err(err)?
        .id)
}

fn emit_json(out: &mut dyn Write, v: &impl serde::Serialize) {
    line(out, serde_json::to_string(v).unwrap_or_default());
}

fn print_devices(org: &ApiOrg, devices: &[ApiDeviceApproval], out: &mut dyn Write) {
    if devices.is_empty() {
        return;
    }
    line(out, "Devices:");
    for d in devices {
        line(out, format!("  {}  {}  {}", d.device_id, d.sub, d.state));
    }
    if org.manages() && devices.iter().any(|d| d.state == "pending") {
        hint_line(
            out,
            format!("Approve one with `cua team approve {} <device>`.", org.id),
        );
    }
}

/// The account id (`sub`) `who` (an email or account id) names in `org`.
async fn member_sub(ctx: &TeamCtx, org: &str, who: &str) -> Result<String, CuaError> {
    let who = who.trim();
    let members = ctx.api.members(&ctx.token, org).await.map_err(err)?;
    members
        .iter()
        .find(|m| m.sub == who || m.email.eq_ignore_ascii_case(who))
        .map(|m| m.sub.clone())
        .ok_or_else(|| CuaError::NotFound(format!("{who} is not a member of this team")))
}

/// `cua team audit`: export is not there yet; say where to look.
fn audit_notice(json: bool, out: &mut dyn Write) -> i32 {
    let msg = format!(
        "Audit export is coming soon (CUA-1374); see {}",
        cua_home::env::account_app_url().trim_start_matches("https://")
    );
    if json {
        emit_json(out, &serde_json::json!({ "events": [], "note": msg }));
    } else {
        line(out, msg);
    }
    0
}

/// A line of what `team create` prints.
enum Created {
    Plain(String),
    Hint(String),
}

/// What `cua team create` tells the person (CUA-2791). The API makes the
/// team waiting for payment (`status: pending`): it stays hidden, and
/// `team ls`, `team show` and `team invite` do not know it, until Teams is
/// started at app.cua.ai. So it never prints those as next steps for it.
fn created_text(t: &cua_host::spaces_api::ApiOrg) -> Vec<Created> {
    use Created::{Hint, Plain};
    if t.status == "pending" {
        return vec![
            Plain(format!(
                "Reserved the team {}. It is not active yet: a team starts when you start Teams.",
                t.label()
            )),
            Plain("Next:".into()),
            Hint(format!(
                "  start Teams at {} (from 1 seat; the team appears once it is bought)",
                cua_host::spaces_api::BUY_TEAMS_URL
            )),
            Hint("  cua auth login   (your session lists the team after a new sign-in)".into()),
            Hint("  cua team invite <team> <email>".into()),
        ];
    }
    vec![
        Plain(format!("Created team {} ({}).", t.label(), t.id)),
        Plain("Next:".into()),
        Hint(
            "  cua auth login                 (your session lists the team after a new sign-in)"
                .into(),
        ),
        Plain(
            "  buy Teams for it at app.cua.ai (team computers and device approval need it)".into(),
        ),
        Hint(format!("  cua team invite {} <email>", t.id)),
        Hint(format!("  cua team add-machine {} <space>", t.id)),
    ]
}

/// Runs `cmd` (all but `add-machine`, which goes through Spaces).
/// `confirm` answers the delete prompt.
pub async fn run_with(
    cmd: TeamCmd,
    ctx: &TeamCtx,
    confirm: &dyn Fn(&str) -> bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (api, token) = (&ctx.api, ctx.token.as_str());
    if let TeamCmd::Audit { .. } = cmd {
        return Ok(audit_notice(json, out));
    }
    let org = match cmd.team_arg() {
        Some(q) if !matches!(cmd, TeamCmd::AddMachine { .. }) => {
            Some(api.resolve_org(token, q).await.map_err(err)?)
        }
        _ => None,
    };
    let org = org.unwrap_or_default();
    let id = org.id.as_str();
    match cmd {
        TeamCmd::Create { name } => {
            let t = api.create_org(token, &name).await.map_err(err)?;
            if json {
                emit_json(out, &t);
            } else {
                for l in created_text(&t) {
                    match l {
                        Created::Plain(l) => line(out, l),
                        Created::Hint(l) => hint_line(out, l),
                    }
                }
            }
        }
        TeamCmd::Ls => {
            let me = api.me(token).await.map_err(err)?;
            if json {
                emit_json(out, &serde_json::json!({ "teams": me.orgs }));
                return Ok(0);
            }
            if me.orgs.is_empty() {
                hint_line(
                    out,
                    "You are in no team. Create one with `cua team create <name>` (or at app.cua.ai).",
                );
            }
            for o in &me.orgs {
                line(
                    out,
                    format!(
                        "{}  {}  {}  {}{}",
                        o.id,
                        o.label(),
                        o.role,
                        if o.plan.is_empty() { "free" } else { &o.plan },
                        if o.has("team_machines") {
                            ""
                        } else {
                            "  (no team computers: buy Teams at app.cua.ai)"
                        }
                    ),
                );
            }
        }
        TeamCmd::Show { .. } => {
            let detail = api.org(token, id).await.map_err(err)?;
            let members = api.members(token, id).await.map_err(err)?;
            let machines = api.org_machines(token, id).await.map_err(err)?;
            let invites = if org.manages() {
                api.invites(token, id).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let devices = api.org_devices(token, id).await.unwrap_or_default();
            if json {
                emit_json(
                    out,
                    &serde_json::json!({
                        "team": detail, "members": members, "machines": machines,
                        "invites": invites, "devices": devices,
                    }),
                );
                return Ok(0);
            }
            line(
                out,
                format!(
                    "{} ({}), you are {}; plan {}, {} of {} seats",
                    org.label(),
                    org.id,
                    detail.role,
                    if detail.plan.is_empty() {
                        "free"
                    } else {
                        &detail.plan
                    },
                    detail.seats_used,
                    detail.seats_paid
                ),
            );
            line(out, format!("Members ({}):", members.len()));
            for m in &members {
                line(
                    out,
                    format!(
                        "  {}  {}",
                        if m.email.is_empty() { &m.sub } else { &m.email },
                        m.role
                    ),
                );
            }
            line(out, format!("Computers ({}):", machines.len()));
            if machines.is_empty() {
                hint_line(
                    out,
                    "  none yet: `cua team add-machine <team> <space>` (owner or admin)",
                );
            }
            for m in &machines {
                line(
                    out,
                    format!(
                        "  relay:{}  {}  {}",
                        m.machine_id,
                        if m.label.is_empty() { "-" } else { &m.label },
                        if m.platform.is_empty() {
                            "-"
                        } else {
                            &m.platform
                        }
                    ),
                );
            }
            if !invites.is_empty() {
                line(out, "Open invites:");
                for i in &invites {
                    line(out, format!("  {}  {}", i.id, i.email));
                }
            }
            print_devices(&org, &devices, out);
        }
        TeamCmd::Invite { email, role, .. } => {
            api.invite(token, id, email.trim(), &role)
                .await
                .map_err(err)?;
            if json {
                emit_json(
                    out,
                    &serde_json::json!({ "team": id, "email": email.trim(), "role": role }),
                );
            } else {
                hint_line(
                    out,
                    format!(
                        "Invited {} to {} as {role}: cua.ai emails them an invitation. They accept it, sign in again (`cua auth login`), and run `cua team join {}`.",
                        email.trim(),
                        org.label(),
                        org.id
                    ),
                );
                line(
                    out,
                    format!(
                        "See and revoke invites at {}/team.",
                        cua_home::env::account_app_url()
                    ),
                );
            }
        }
        TeamCmd::Join { .. } => {
            let role = api.claim_invite(token, id).await.map_err(err)?;
            if json {
                emit_json(out, &serde_json::json!({ "team": id, "role": role }));
            } else {
                line(
                    out,
                    format!("You are in {} ({}) as {role}.", org.label(), org.id),
                );
                hint_line(
                    out,
                    format!(
                        "If the team approves devices, ask for this one: `cua team request {}`.",
                        org.id
                    ),
                );
            }
        }
        TeamCmd::Leave { .. } => {
            api.leave_org(token, id).await.map_err(err)?;
            if !json {
                line(out, format!("You left {}.", org.label()));
            }
        }
        TeamCmd::RemoveMember { who, .. } => {
            let sub = member_sub(ctx, id, &who).await?;
            api.remove_member(token, id, &sub).await.map_err(err)?;
            if !json {
                line(
                    out,
                    format!("Removed {who}: their open sessions on team computers end."),
                );
            }
        }
        TeamCmd::SetRole { who, role, .. } => {
            let sub = member_sub(ctx, id, &who).await?;
            api.set_member_role(token, id, &sub, &role)
                .await
                .map_err(err)?;
            if json {
                emit_json(out, &serde_json::json!({ "sub": sub, "role": role }));
            } else {
                line(out, format!("{who} is now {role}."));
            }
        }
        TeamCmd::Devices { .. } => {
            let devices = api.org_devices(token, id).await.map_err(err)?;
            if json {
                emit_json(out, &devices);
            } else if devices.is_empty() {
                line(out, "No member device has asked yet.");
            } else {
                print_devices(&org, &devices, out);
            }
        }
        TeamCmd::Approve { device, .. } => decide(ctx, &org, &device, true, json, out).await?,
        TeamCmd::Deny { device, .. } => decide(ctx, &org, &device, false, json, out).await?,
        TeamCmd::Request { .. } => {
            let device = ctx.device.clone().ok_or_else(|| {
                CuaError::InvalidArgument(
                    "this computer is not an enrolled device: run `cua devices enroll` first"
                        .into(),
                )
            })?;
            let d = api
                .request_org_device(token, id, &device)
                .await
                .map_err(err)?;
            if json {
                emit_json(out, &d);
            } else {
                line(out, format!("This device ({}): {}.", d.device_id, d.state));
            }
        }
        TeamCmd::AddMachine { .. } => {
            return Err(CuaError::Internal("add-machine runs through Spaces".into()));
        }
        TeamCmd::RemoveMachine { machine, .. } => {
            let m = machine.trim();
            let m = m.strip_prefix("relay:").unwrap_or(m);
            api.remove_org_machine(token, id, m).await.map_err(err)?;
            if json {
                emit_json(out, &serde_json::json!({ "team": id, "removed": m }));
            } else {
                line(
                    out,
                    format!("relay:{m} is no longer a computer of {}.", org.label()),
                );
            }
        }
        TeamCmd::Audit { .. } => unreachable!("answered above"),
        TeamCmd::Delete { yes, .. } => {
            if !yes
                && !confirm(&format!(
                    "Delete team {}? Members lose its computers.",
                    org.label()
                ))
            {
                return Err(CuaError::InvalidArgument(
                    "not deleted (pass --yes when not in a terminal)".into(),
                ));
            }
            api.delete_org(token, id).await.map_err(err)?;
            if !json {
                line(out, format!("Deleted {}.", org.label()));
            }
        }
    }
    Ok(0)
}

async fn decide(
    ctx: &TeamCtx,
    org: &ApiOrg,
    device: &str,
    approve: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), CuaError> {
    let device = device.trim();
    let requests = ctx
        .api
        .org_devices(&ctx.token, &org.id)
        .await
        .map_err(err)?;
    let sub = requests
        .iter()
        .find(|d| d.device_id == device)
        .map(|d| d.sub.clone())
        .ok_or_else(|| {
            CuaError::NotFound(format!(
                "no member of {} asked for device {device} (`cua team devices {}`)",
                org.label(),
                org.id
            ))
        })?;
    let mut said = false;
    let d = ctx
        .api
        .finishing_setup(
            &ctx.token,
            SETUP_RETRY_WINDOW,
            |_| {
                if !said && !json {
                    eprintln!("{FINISHING_SETUP} (the team's plan is on its way)");
                    said = true;
                }
            },
            || {
                ctx.api
                    .decide_org_device(&ctx.token, &org.id, device, &sub, approve)
            },
        )
        .await
        .map_err(err)?;
    if json {
        emit_json(out, &d);
    } else if approve {
        line(
            out,
            format!(
                "Approved {} ({}): it can open the team's computers.",
                d.device_id, d.sub
            ),
        );
    } else {
        line(out, format!("Declined {}.", d.device_id));
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
    if let TeamCmd::Audit { .. } = cmd {
        return Ok(audit_notice(json, out));
    }
    let ctx = TeamCtx::current(home, matches!(cmd, TeamCmd::Request { .. })).await?;
    run_with(
        cmd,
        &ctx,
        &|q| util::interactive() && util::confirm(q, false),
        json,
        out,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_host::testing::FakeSpacesApi;

    fn ctx(api: &FakeSpacesApi, token: &str, device: Option<&str>) -> TeamCtx {
        TeamCtx {
            api: SpacesApiClient::new(&api.url).unwrap(),
            token: token.into(),
            device: device.map(str::to_string),
        }
    }

    async fn text(cmd: TeamCmd, ctx: &TeamCtx) -> Result<String, CuaError> {
        let mut out = Vec::new();
        run_with(cmd, ctx, &|_| true, false, &mut out).await?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn a_team_waiting_for_payment_does_not_suggest_steps_that_fail() {
        let pending = cua_host::spaces_api::ApiOrg {
            id: "0f1e2d3c".into(),
            name: "Acme".into(),
            status: "pending".into(),
            ..Default::default()
        };
        let text: String = created_text(&pending)
            .into_iter()
            .map(|l| match l {
                Created::Plain(l) | Created::Hint(l) => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("not active yet"), "{text}");
        assert!(text.contains("https://app.cua.ai/buy/teams"), "{text}");
        assert!(!text.contains("add-machine"), "{text}");
        assert!(!text.contains("0f1e2d3c"), "{text}");
        let active = cua_host::spaces_api::ApiOrg {
            status: "active".into(),
            ..pending
        };
        let text: String = created_text(&active)
            .into_iter()
            .map(|l| match l {
                Created::Plain(l) | Created::Hint(l) => l,
            })
            .collect();
        assert!(text.contains("Created team Acme"), "{text}");
    }

    #[tokio::test]
    async fn teams_are_organizations_with_machines_and_approved_devices() {
        let api = FakeSpacesApi::start().await;
        api.add_account("t-ada", "ada", "ada@aigis.test");
        api.add_account("t-bob", "bob", "bob@aigis.test");
        let org = api.add_org(
            "aigis",
            "AIGIS",
            "ada",
            &["team_machines", "device_approval"],
        );
        api.add_member(&org, "bob", "member");
        let ada = ctx(&api, "t-ada", Some("dev_ada"));
        let bob = ctx(&api, "t-bob", Some("dev_bob"));

        // Listed by organization id; resolved by name.
        let t = text(TeamCmd::Ls, &bob).await.unwrap();
        assert!(t.contains(&format!("{org}  AIGIS  member  teams")), "{t}");
        let t = text(
            TeamCmd::Invite {
                team: "aigis".into(),
                email: "cy@aigis.test".into(),
                role: "member".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains("emails them an invitation"), "{t}");

        // Bob asks for his device; he cannot approve it himself; Ada does.
        let t = text(
            TeamCmd::Request {
                team: "AIGIS".into(),
            },
            &bob,
        )
        .await
        .unwrap();
        assert!(t.contains("dev_bob") && t.contains("pending"), "{t}");
        let refused = text(
            TeamCmd::Approve {
                team: "AIGIS".into(),
                device: "dev_bob".into(),
            },
            &bob,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                refused,
                CuaError::NotFound(_) | CuaError::PermissionDenied(_)
            ),
            "{refused:?}"
        );
        let t = text(TeamCmd::Devices { team: org.clone() }, &ada)
            .await
            .unwrap();
        assert!(t.contains("dev_bob") && t.contains("pending"), "{t}");
        let t = text(
            TeamCmd::Approve {
                team: org.clone(),
                device: "dev_bob".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains("Approved dev_bob"), "{t}");
        assert_eq!(
            api.approvals(&org),
            vec![("dev_bob".into(), "approved".into())]
        );

        // Machines: removing one the team does not have is not found.
        let e = text(
            TeamCmd::RemoveMachine {
                team: "AIGIS".into(),
                machine: "relay:nope".into(),
            },
            &ada,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, CuaError::NotFound(_)), "{e:?}");

        let t = text(
            TeamCmd::Show {
                team: "AIGIS".into(),
            },
            &ada,
        )
        .await
        .unwrap();
        assert!(t.contains("AIGIS") && t.contains("bob@aigis.test"), "{t}");
        let e = text(
            TeamCmd::Show {
                team: "nope".into(),
            },
            &ada,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, CuaError::NotFound(_)), "{e:?}");
        // The relay's old team ids are refused outright.
        let e = text(
            TeamCmd::Show {
                team: "team-0a1b2c3d4e5f6a7b".into(),
            },
            &ada,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, CuaError::InvalidArgument(_)), "{e:?}");
    }

    #[tokio::test]
    async fn audit_points_to_the_account_app() {
        let api = FakeSpacesApi::start().await;
        let t = text(
            TeamCmd::Audit {
                team: "AIGIS".into(),
                limit: 50,
            },
            &ctx(&api, "t-nobody", None),
        )
        .await
        .unwrap();
        assert!(
            t.starts_with("Audit export is coming soon (CUA-1374); see app."),
            "{t}"
        );
    }

    #[tokio::test]
    async fn a_server_without_teams_says_so() {
        let api = FakeSpacesApi::start().await;
        api.add_account("t-ada", "ada", "ada@aigis.test");
        api.without_teams();
        let e = text(TeamCmd::Ls, &ctx(&api, "t-ada", None))
            .await
            .unwrap_err();
        assert!(
            matches!(e, CuaError::Unsupported(ref m) if m.contains("Teams are not available on this server yet")),
            "{e:?}"
        );
    }
}
