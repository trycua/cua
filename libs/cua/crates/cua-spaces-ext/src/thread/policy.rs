// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who may do what in a Space's thread and with its agent members.
//!
//! Roles come from the authenticated caller ([`Caller`]), over the Access
//! roles:
//!
//! | Role | Who |
//! |---|---|
//! | owner | this machine's own user, or the owner's account on another device |
//! | admin | an admin (or the owner) of a team this machine's owner is in, from the Spaces API ([`crate::persistent::Persistent::set_team_admins`]); or relay role `admin` |
//! | member | an account the Space is shared with to edit (role `shared`) |
//! | guest | an account the Space is shared with to watch (role `viewer`), or any role this daemon does not know |
//! | agent | an agent member, through its bridge or (connected from a computer) its member key, in its own Space |
//!
//! Each agent member has three rights ([`Rights`], stored on the member as
//! `policy.rights`): `task` (whose @mention gives it a task), `control`
//! (interrupt, stop, pause, resume, take control) and `approve` (answer its
//! approval requests). Each is held by `owner`, `admins`, `members` or a
//! list of account ids ([`Who`]). The owner always holds every right;
//! guests hold none (they read the thread and nothing else); the author of
//! the task an agent works on may always stop that task.
//!
//! Every check here is made by the owner daemon at tool entry and again
//! where an entry is written, never from the arguments.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use cua_spaces::extension::Caller;
use cua_spaces::{Error, Result};

/// The error code of every refusal here.
pub const DENIED: &str = "permission_denied";

/// What a guest is told when they try to write.
pub const GUEST_READ_ONLY: &str =
    "view only: guests can read this thread but cannot post, answer or claim tasks";

/// A caller's role in this daemon's Spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Owner,
    Admin,
    Member,
    Guest,
    Agent,
}

impl Role {
    /// The role of `caller`.
    pub fn of(caller: &Caller) -> Role {
        match caller {
            Caller::Owner => Role::Owner,
            Caller::Agent { .. } | Caller::Member { .. } => Role::Agent,
            Caller::Relay { role, .. } => match role.as_str() {
                "owner" => Role::Owner,
                "admin" => Role::Admin,
                // `shared` is an editor of the machine (`HostCaller`).
                "shared" | "editor" | "member" => Role::Member,
                // `viewer`, and anything this daemon does not know: read
                // only (fail closed).
                _ => Role::Guest,
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Guest => "guest",
            Role::Agent => "agent",
        }
    }
}

/// One of an agent member's rights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Right {
    /// Give it a task (an @mention, `agent_send`, `agent_message`).
    Task,
    /// Interrupt, stop, pause, resume or take control of it.
    Control,
    /// Answer its approval requests.
    Approve,
}

impl Right {
    pub fn as_str(self) -> &'static str {
        match self {
            Right::Task => "task",
            Right::Control => "control",
            Right::Approve => "approve",
        }
    }
}

/// A group of people a right is given to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// The owner only.
    Owner,
    /// The owner and admins.
    Admins,
    /// The owner, admins, members (editors) and the Space's agent members.
    Members,
}

/// Who holds a right: a group, or these accounts (relay account ids, or
/// `agent:<name>`) besides the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Who {
    Group(Group),
    Accounts(Vec<String>),
}

impl Who {
    /// Parses `owner`, `admins`, `members` or a list of account ids.
    pub fn parse(right: &str, v: &Value) -> Result<Who> {
        let bad = || {
            Error::invalid(format!(
                "rights.{right}: use \"owner\", \"admins\", \"members\" or a list of account ids"
            ))
        };
        match v {
            Value::String(s) => match s.trim() {
                "owner" => Ok(Who::Group(Group::Owner)),
                "admins" => Ok(Who::Group(Group::Admins)),
                "members" => Ok(Who::Group(Group::Members)),
                _ => Err(bad()),
            },
            Value::Array(items) => {
                let mut ids = Vec::new();
                for i in items {
                    let id = i.as_str().map(str::trim).filter(|s| !s.is_empty());
                    let id = id.ok_or_else(bad)?;
                    if id.len() > 200 {
                        return Err(bad());
                    }
                    if !ids.iter().any(|x| x == id) {
                        ids.push(id.to_string());
                    }
                }
                if ids.len() > 256 {
                    return Err(Error::invalid(format!(
                        "rights.{right}: at most 256 accounts"
                    )));
                }
                Ok(Who::Accounts(ids))
            }
            _ => Err(bad()),
        }
    }

    /// Who, in words ("the owner and admins").
    pub fn describe(&self) -> String {
        match self {
            Who::Group(Group::Owner) => "the owner".into(),
            Who::Group(Group::Admins) => "the owner and admins".into(),
            Who::Group(Group::Members) => "the owner and members".into(),
            Who::Accounts(ids) if ids.is_empty() => "the owner".into(),
            Who::Accounts(ids) => format!(
                "the owner and {} named {}",
                ids.len(),
                if ids.len() == 1 { "person" } else { "people" }
            ),
        }
    }

    /// Whether a caller with `role` and author id `id` holds it.
    pub fn holds(&self, role: Role, id: &str) -> bool {
        match role {
            Role::Owner => return true,
            Role::Guest => return false,
            _ => {}
        }
        match self {
            Who::Group(Group::Owner) => false,
            Who::Group(Group::Admins) => role == Role::Admin,
            Who::Group(Group::Members) => {
                matches!(role, Role::Admin | Role::Member | Role::Agent)
            }
            Who::Accounts(ids) => ids.iter().any(|i| i == id),
        }
    }
}

/// An agent member's rights (`policy.rights`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rights {
    /// Who can give it tasks. Default `members`.
    #[serde(default = "members")]
    pub task: Who,
    /// Who can interrupt, stop, pause, resume or take control of it (and
    /// the author of its current task, for that task). Default `admins`.
    #[serde(default = "admins")]
    pub control: Who,
    /// Who can answer its approval requests. Default `admins`.
    #[serde(default = "admins")]
    pub approve: Who,
}

fn members() -> Who {
    Who::Group(Group::Members)
}

fn admins() -> Who {
    Who::Group(Group::Admins)
}

impl Default for Rights {
    fn default() -> Self {
        Rights {
            task: members(),
            control: admins(),
            approve: admins(),
        }
    }
}

impl Rights {
    pub fn get(&self, right: Right) -> &Who {
        match right {
            Right::Task => &self.task,
            Right::Control => &self.control,
            Right::Approve => &self.approve,
        }
    }

    /// Whether a caller with `role` (author id `id`) holds `right`.
    /// Agents never answer approvals.
    pub fn allows(&self, right: Right, role: Role, id: &str) -> bool {
        if right == Right::Approve && role == Role::Agent {
            return false;
        }
        self.get(right).holds(role, id)
    }

    /// `{task, control, approve}` for a caller with `role` (author id
    /// `id`), as `agent_members` shows it.
    pub fn you_can(&self, role: Role, id: &str) -> Value {
        json!({
            "task": self.allows(Right::Task, role, id),
            "control": self.allows(Right::Control, role, id),
            "approve": self.allows(Right::Approve, role, id),
        })
    }

    /// These rights with `task`, `control` and `approve` from `v` (an
    /// object; absent keys keep their value).
    pub fn with(&self, v: &Value) -> Result<Rights> {
        let mut out = self.clone();
        let Some(o) = v.as_object() else {
            return Err(Error::invalid(
                "rights: an object with task, control or approve",
            ));
        };
        for (k, val) in o {
            if val.is_null() {
                continue;
            }
            let who = Who::parse(k, val)?;
            match k.as_str() {
                "task" => out.task = who,
                "control" => out.control = who,
                "approve" => out.approve = who,
                other => {
                    return Err(Error::invalid(format!(
                        "rights.{other}: use task, control or approve"
                    )));
                }
            }
        }
        Ok(out)
    }
}

/// A refusal with [`DENIED`].
pub fn denied(msg: impl Into<String>) -> Error {
    Error::extension(DENIED, msg)
}

/// Refuses `right` on `agent` for `caller`.
pub fn refuse(right: Right, agent: &str, who: &Who) -> Error {
    let holders = who.describe();
    denied(match right {
        Right::Task => format!("you can't give {agent} tasks: only {holders} can"),
        Right::Control => format!(
            "you can't stop or take control of {agent}: only {holders} can, and whoever asked for its current task"
        ),
        Right::Approve => {
            format!("you can't answer {agent}'s approval requests: only {holders} can")
        }
    })
}

/// Writing to a thread (post, answer, claim, reply): everyone but guests.
pub fn may_write(role: Role) -> Result<()> {
    if role == Role::Guest {
        return Err(denied(GUEST_READ_ONLY));
    }
    Ok(())
}

/// Changing an agent member's rights: the Space's owner and admins, and
/// the agent's own owner (`owns`: the member that added it or connected it).
pub fn may_set_rights(role: Role, agent: &str, owns: bool) -> Result<()> {
    match role {
        Role::Owner | Role::Admin => Ok(()),
        Role::Member if owns => Ok(()),
        _ => Err(denied(format!(
            "only the owner, admins and {agent}'s owner change who can task, stop and approve {agent}"
        ))),
    }
}

/// What a tool needs from a relay caller before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Reading: everyone, guests included.
    Read,
    /// Writing to the thread or steering an agent: not guests (the right
    /// itself is checked where the tool acts).
    Write,
    /// The owner and admins.
    Admin,
    /// The owner only.
    Owner,
}

/// What `tool` needs. Unknown tools need the owner.
pub fn need(tool: &str) -> Need {
    match tool {
        "thread_read"
        | "thread_inbox"
        | "agent_members"
        | "thread_join_code_list"
        | "thread_settings" => Need::Read,
        "thread_post"
        | "thread_claim"
        | "thread_reply"
        | "agent_send"
        | "agent_pause"
        | "agent_resume"
        | "thread_join"
        | "thread_join_code_create"
        | "thread_join_code_cancel"
        | "thread_join_redeem"
        // The agent's own owner may change its rights too: checked there.
        | "agent_set_rights"
        | "agent_remove" => Need::Write,
        _ => Need::Owner,
    }
}

/// The run tools that take a member's `name` ([`cua_spaces::agents::MEMBER_RUN_TOOLS`]).
pub fn need_member_run(tool: &str) -> Need {
    match tool {
        "agent_status" | "agent_events" => Need::Read,
        _ => Need::Write,
    }
}

/// The tool-entry check: whether `caller` may call a tool needing `need`
/// at all. This machine's user and agents in their bridge pass (the bridge
/// serves its own short list); relay callers by role.
pub fn gate(caller: &Caller, role: Role, tool: &str, need: Need) -> Result<()> {
    // An agent connected with a member key: its Space's thread tools only.
    if matches!(caller, Caller::Member { .. }) {
        if cua_spaces::extension::MEMBER_TOOLS.contains(&tool) {
            return Ok(());
        }
        return Err(denied(format!(
            "{tool}: an agent connected to a Space reaches only that Space's thread"
        )));
    }
    if !matches!(caller, Caller::Relay { .. }) {
        return Ok(());
    }
    match (need, role) {
        (_, Role::Owner) | (Need::Read, _) => Ok(()),
        (_, Role::Guest) => Err(denied(GUEST_READ_ONLY)),
        (Need::Write, _) => Ok(()),
        (Need::Admin, Role::Admin) => Ok(()),
        (Need::Admin, _) => Err(denied(format!(
            "{tool}: only the owner and admins can do this"
        ))),
        (Need::Owner, _) => Err(denied(format!(
            "{tool}: only the owner of this machine can do this"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(role: &str, account: &str) -> Caller {
        Caller::Relay {
            account: account.into(),
            name: None,
            role: role.into(),
        }
    }

    #[test]
    fn roles_come_from_the_caller_and_unknown_roles_read_only() {
        assert_eq!(Role::of(&Caller::Owner), Role::Owner);
        assert_eq!(Role::of(&relay("owner", "a")), Role::Owner);
        assert_eq!(Role::of(&relay("admin", "a")), Role::Admin);
        assert_eq!(Role::of(&relay("shared", "a")), Role::Member);
        assert_eq!(Role::of(&relay("viewer", "a")), Role::Guest);
        assert_eq!(Role::of(&relay("", "a")), Role::Guest);
        assert_eq!(Role::of(&relay("superuser", "a")), Role::Guest);
        let agent = Caller::Agent {
            name: "x".into(),
            space: "local:s".into(),
        };
        assert_eq!(Role::of(&agent), Role::Agent);
    }

    #[test]
    fn default_rights_by_role() {
        let r = Rights::default();
        let agent = Caller::Agent {
            name: "x".into(),
            space: "local:s".into(),
        };
        let rows = [
            (Caller::Owner, "owner", [true, true, true]),
            (relay("owner", "o"), "o", [true, true, true]),
            (relay("admin", "ad"), "ad", [true, true, true]),
            (relay("shared", "m"), "m", [true, false, false]),
            (relay("viewer", "g"), "g", [false, false, false]),
            (agent, "agent:x", [true, false, false]),
        ];
        for (caller, id, want) in rows {
            let got = [Right::Task, Right::Control, Right::Approve]
                .map(|x| r.allows(x, Role::of(&caller), id));
            assert_eq!(got, want, "{caller:?}");
        }
    }

    #[test]
    fn named_accounts_and_owner_only() {
        let r = Rights::default()
            .with(&json!({"task": ["bob"], "control": "owner", "approve": "members"}))
            .unwrap();
        assert!(r.allows(Right::Task, Role::of(&relay("shared", "bob")), "bob"));
        assert!(!r.allows(Right::Task, Role::of(&relay("shared", "carol")), "carol"));
        // A guest listed by name still only reads.
        assert!(!r.allows(Right::Task, Role::of(&relay("viewer", "bob")), "bob"));
        assert!(!r.allows(Right::Control, Role::of(&relay("admin", "ad")), "ad"));
        assert!(r.allows(Right::Control, Role::Owner, "owner"));
        assert!(r.allows(Right::Approve, Role::of(&relay("shared", "m")), "m"));
        let agent = Caller::Agent {
            name: "x".into(),
            space: "s".into(),
        };
        assert!(!r.allows(Right::Approve, Role::of(&agent), "agent:x"));
    }

    #[test]
    fn parsing_refuses_unknown_values() {
        assert!(
            Rights::default()
                .with(&json!({"task": "everyone"}))
                .is_err()
        );
        assert!(Rights::default().with(&json!({"task": [1]})).is_err());
        assert!(Rights::default().with(&json!({"drive": "owner"})).is_err());
        let r = Rights::default()
            .with(&json!({"task": [" a ", "a", "b"]}))
            .unwrap();
        assert_eq!(r.task, Who::Accounts(vec!["a".into(), "b".into()]));
        // Stored as plain JSON: groups as strings, accounts as a list.
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"task": ["a", "b"], "control": "admins", "approve": "admins"})
        );
        let back: Rights = serde_json::from_value(json!({"task": "owner"})).unwrap();
        assert_eq!(back.task, Who::Group(Group::Owner));
        assert_eq!(back.control, admins());
    }

    #[test]
    fn the_gate_by_role() {
        let g =
            |c: &Caller, t: &str| gate(c, Role::of(c), t, need(t)).map_err(|e| e.tag().to_string());
        let guest = relay("viewer", "g");
        assert!(g(&guest, "thread_read").is_ok());
        assert!(g(&guest, "agent_members").is_ok());
        assert_eq!(g(&guest, "thread_post").unwrap_err(), DENIED);
        assert_eq!(g(&guest, "thread_claim").unwrap_err(), DENIED);
        let member = relay("shared", "m");
        assert!(g(&member, "thread_post").is_ok());
        // Past the gate: the agent's owner may; `set_rights` refuses the rest.
        assert!(g(&member, "agent_set_rights").is_ok());
        assert_eq!(g(&guest, "agent_set_rights").unwrap_err(), DENIED);
        assert!(may_set_rights(Role::Member, "ada", true).is_ok());
        assert_eq!(
            may_set_rights(Role::Member, "ada", false)
                .unwrap_err()
                .tag(),
            DENIED
        );
        assert_eq!(g(&member, "notifications_list").unwrap_err(), DENIED);
        assert_eq!(g(&member, "agent_add").unwrap_err(), DENIED);
        assert!(g(&relay("admin", "a"), "agent_set_rights").is_ok());
        assert!(g(&relay("owner", "o"), "agent_schedule_add").is_ok());
        assert!(g(&Caller::Owner, "notifications_list").is_ok());
    }
}
