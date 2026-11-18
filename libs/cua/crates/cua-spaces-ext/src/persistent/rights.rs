// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-agent rights on the member records ([`crate::thread::policy`]),
//! taking control of an agent, and whose provider keys a task runs on.

use serde_json::Value;

use super::{AgentRecord, Persistent};
use crate::thread::policy::{self, Right, Rights};
use crate::thread::{Author, Draft, KeyOwner};
use cua_spaces::Result;
use cua_spaces::extension::Caller;

impl Persistent {
    /// `caller`'s role here: [`policy::Role::of`], with a member who is an
    /// admin of one of the owner's teams counted as an admin.
    pub fn role_of(&self, caller: &Caller) -> policy::Role {
        let role = policy::Role::of(caller);
        match caller {
            Caller::Relay { account, .. }
                if role == policy::Role::Member
                    && self
                        .admins
                        .read()
                        .is_ok_and(|a| a.contains(account.as_str())) =>
            {
                policy::Role::Admin
            }
            _ => role,
        }
    }

    /// The accounts that are admins (or the owner) of a team this
    /// machine's owner is in (the daemon reads them from the Spaces API).
    pub fn set_team_admins(&self, accounts: std::collections::HashSet<String>) {
        if let Ok(mut a) = self.admins.write() {
            *a = accounts;
        }
    }

    /// `rec`'s rights (the defaults when none were set).
    pub fn rights_of(rec: &AgentRecord) -> Rights {
        rec.policy.rights.clone().unwrap_or_default()
    }

    /// Whether `caller` holds `right` on `rec`.
    pub fn allows(&self, caller: &Caller, rec: &AgentRecord, right: Right) -> bool {
        let role = self.role_of(caller);
        let me = self.author_for(caller).id;
        if Self::rights_of(rec).allows(right, role, &me) {
            return true;
        }
        // A connected agent's owner controls it and answers its approvals.
        right != Right::Task
            && role != policy::Role::Guest
            && role != policy::Role::Agent
            && super::identity::is_external(rec)
            && super::identity::stored_of(rec, &self.owner()).owner.id == me
    }

    /// Refuses `right` on `rec` unless `caller` holds it.
    pub fn check_right(&self, caller: &Caller, rec: &AgentRecord, right: Right) -> Result<()> {
        policy::may_write(self.role_of(caller))?;
        if self.allows(caller, rec, right) {
            return Ok(());
        }
        Err(policy::refuse(
            right,
            &rec.name,
            Self::rights_of(rec).get(right),
        ))
    }

    /// Control of `rec`: the `control` right, or the author of the task its
    /// current turn works on.
    pub fn check_control(&self, caller: &Caller, rec: &AgentRecord) -> Result<()> {
        policy::may_write(self.role_of(caller))?;
        if self.allows(caller, rec, Right::Control) {
            return Ok(());
        }
        let me = self.author_for(caller).id;
        if let Some(t) = &rec.task
            && let Ok(fold) = self.thread_store(&rec.space).fold()
            && fold.tasks.get(&t.seq).is_some_and(|t| t.author.id == me)
        {
            return Ok(());
        }
        Err(policy::refuse(
            Right::Control,
            &rec.name,
            Self::rights_of(rec).get(Right::Control),
        ))
    }

    /// Sets who can task, control and approve `name` from `v` (an object
    /// with any of `task`, `control`, `approve`); the Space's owner and
    /// admins, and the agent's own owner. A system note in the thread says
    /// who changed it.
    pub fn set_rights(&self, caller: &Caller, name: &str, v: &Value) -> Result<AgentRecord> {
        let role = self.role_of(caller);
        let rec = self.get(name)?;
        let me = self.author_for(caller).id;
        let owns = !me.is_empty() && super::identity::stored_of(&rec, &self.owner()).owner.id == me;
        policy::may_set_rights(role, name, owns)?;
        let next = Self::rights_of(&rec).with(v)?;
        if Some(&next) == rec.policy.rights.as_ref()
            || (rec.policy.rights.is_none() && next == Rights::default())
        {
            return Ok(rec);
        }
        self.update(name, |r| r.policy.rights = Some(next.clone()))?;
        let me = self.author_for(caller);
        let mut changed = vec![];
        for right in [Right::Task, Right::Control, Right::Approve] {
            if Self::rights_of(&rec).get(right) != next.get(right) {
                changed.push(format!(
                    "{}: {}",
                    right.as_str(),
                    next.get(right).describe()
                ));
            }
        }
        self.thread_store(&rec.space).append(Draft::note(
            "rights",
            None,
            format!(
                "{} changed who can use {name}. {}",
                me.name,
                changed.join("; ")
            ),
        ))?;
        self.get(name)
    }

    /// Whether `rec`'s runs use this machine's provider keys: keys it
    /// forwards (`env_from_host`), or a key saved in Cua Spaces (or set in
    /// this daemon's environment) that its harness reads, when it has no
    /// endpoint of its own.
    pub fn uses_owner_keys(rec: &AgentRecord) -> bool {
        if !rec.env_from_host.is_empty() {
            return true;
        }
        if rec.base_url.is_some() {
            return false;
        }
        cua_spaces::agents::harness::harness(&rec.harness)
            .is_some_and(|h| cua_spaces::agents::host_key(h).is_some())
    }

    /// The owner whose keys a task from `caller` to `rec` runs on: set
    /// when someone other than the owner gives the task and the run uses
    /// the owner's keys.
    pub fn keys_of(&self, caller: &Caller, rec: &AgentRecord) -> Option<KeyOwner> {
        if !matches!(caller, Caller::Relay { .. }) || self.role_of(caller) == policy::Role::Owner {
            return None;
        }
        Self::uses_owner_keys(rec).then(|| {
            let o = self.owner();
            KeyOwner {
                id: o.id,
                name: o.name,
            }
        })
    }

    /// `caller` takes control of `name` (an interrupt or stop by name, or
    /// the app's Take control): the agent is held, its current task goes
    /// back to the queue, and nothing is delivered to it until someone
    /// gives control back ([`Persistent::give_back_control`]). A system
    /// note records who.
    pub fn take_control(&self, caller: &Caller, name: &str) -> Result<Author> {
        let rec = self.get(name)?;
        self.check_control(caller, &rec)?;
        let me = self.author_for(caller);
        let task = rec.task.as_ref().map(|t| t.seq);
        if task.is_some() {
            self.requeue(&rec, &format!("{} took control", me.name))?;
        }
        let first = rec.held_by.is_none();
        let held = me.clone();
        self.update(name, |r| r.held_by = Some(held))?;
        if first {
            self.thread_store(&rec.space).append(Draft::note(
                "control",
                task,
                format!(
                    "{} took control of {name}. {name} waits until control is given back.",
                    me.name
                ),
            ))?;
        }
        Ok(me)
    }

    /// Gives control of `name` back (`agent_resume`): its queued tasks are
    /// delivered again. `false` when it was not held.
    pub fn give_back_control(&self, caller: &Caller, name: &str) -> Result<bool> {
        let rec = self.get(name)?;
        if rec.held_by.is_none() {
            return Ok(false);
        }
        self.check_control(caller, &rec)?;
        self.update(name, |r| r.held_by = None)?;
        let me = self.author_for(caller);
        self.thread_store(&rec.space).append(Draft::note(
            "control",
            None,
            format!("{} gave control back to {name}.", me.name),
        ))?;
        Ok(true)
    }
}
