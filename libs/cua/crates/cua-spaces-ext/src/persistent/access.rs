// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-agent access to the user's own computers.
//!
//! `cua host setup` makes a machine a Space for the whole account. A
//! persistent agent does not get it by being in the account: it reaches a
//! machine only through its bridge (`computer_list_tools`,
//! `computer_call`), and the bridge answers only for a machine the user
//! granted to that agent by name. Granting asks for presence, revoking does
//! not, and every grant, revocation, refusal and use is a line in a
//! hash-chained audit log.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cua_spaces::{Error, Result};

/// One grant: agent `agent` may use machine `machine` (a Space id such as
/// `relay:<machine-id>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGrant {
    pub id: String,
    pub agent: String,
    pub machine: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
}

impl AccessGrant {
    pub fn is_live(&self, now_ms: u64) -> bool {
        !self.revoked && self.expires_ms.is_none_or(|e| now_ms < e)
    }
}

/// The grants file and its audit log.
#[derive(Clone, Debug)]
pub struct Access {
    path: PathBuf,
    audit: cua_volume::audit::AuditLog,
}

impl Access {
    pub fn new(dir: &Path) -> Access {
        Access {
            path: dir.join("computer-access.json"),
            audit: cua_volume::audit::AuditLog::new(dir.join("computer-access-audit.jsonl")),
        }
    }

    pub fn audit(&self) -> &cua_volume::audit::AuditLog {
        &self.audit
    }

    /// Every grant (live, expired and revoked).
    pub fn grants(&self) -> Result<Vec<AccessGrant>> {
        super::read_json(&self.path)
    }

    /// The live grant letting `agent` use `machine`, if any.
    pub fn live(&self, agent: &str, machine: &str) -> Result<Option<AccessGrant>> {
        let now = cua_volume::now_ms();
        Ok(self
            .grants()?
            .into_iter()
            .find(|g| g.agent == agent && g.machine == machine && g.is_live(now)))
    }

    /// Lets `agent` use `machine` (asks `presence` first).
    pub fn grant(
        &self,
        presence: &dyn cua_volume::Presence,
        agent: &str,
        machine: &str,
        expires_ms: Option<u64>,
    ) -> Result<AccessGrant> {
        if !cua_volume::path::valid_agent_name(agent) {
            return Err(Error::invalid(format!("agent name {agent:?}")));
        }
        if machine.trim().is_empty() {
            return Err(Error::invalid("name the machine (a Space id)"));
        }
        if let Some(g) = self.live(agent, machine)? {
            return Ok(g);
        }
        presence
            .confirm(&format!("Let agent {agent} use {machine}"))
            .map_err(|e| crate::drive_err(cua_volume::Error::NotConfirmed(e)))?;
        let g = AccessGrant {
            id: cua_volume::new_id(),
            agent: agent.into(),
            machine: machine.into(),
            created_ms: cua_volume::now_ms(),
            expires_ms,
            revoked: false,
        };
        let out = g.clone();
        super::locked_json(&self.path, move |all: &mut Vec<AccessGrant>| all.push(g))?;
        self.log(
            "user",
            "grant",
            &out.machine,
            &format!("agent:{agent} id={}", out.id),
        );
        Ok(out)
    }

    /// Revokes every live grant of `agent` on `machine` (or on every
    /// machine when `machine` is `None`). Returns how many.
    pub fn revoke(&self, agent: &str, machine: Option<&str>) -> Result<usize> {
        let n = super::locked_json(&self.path, |all: &mut Vec<AccessGrant>| {
            let mut n = 0;
            for g in all.iter_mut() {
                if g.agent == agent && machine.is_none_or(|m| g.machine == m) && !g.revoked {
                    g.revoked = true;
                    n += 1;
                }
            }
            n
        })?;
        self.log(
            "user",
            "revoke",
            machine.unwrap_or("*"),
            &format!("agent:{agent} revoked={n}"),
        );
        Ok(n)
    }

    /// Checks (and audits) one use by `agent` of `machine`.
    pub fn check(&self, agent: &str, machine: &str, what: &str) -> Result<()> {
        if self.live(agent, machine)?.is_some() {
            self.log(&format!("agent:{agent}"), "use", machine, what);
            return Ok(());
        }
        self.log(&format!("agent:{agent}"), "denied", machine, what);
        Err(crate::drive_err(cua_volume::Error::Forbidden(format!(
            "agent {agent} has no access to {machine}; the user can allow it in Cua \
             (This machine > Agents) or with `cua agent allow-computer {agent} {machine}`"
        ))))
    }

    fn log(&self, principal: &str, action: &str, machine: &str, detail: &str) {
        if let Err(e) = self.audit.append(principal, action, machine, detail) {
            tracing::warn!("computer access audit: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Yes;
    impl cua_volume::Presence for Yes {
        fn confirm(&self, _: &str) -> std::result::Result<(), String> {
            Ok(())
        }
    }

    /// A Space one of your machines provides (`relay:<space machine>`) is a
    /// machine of its own: a grant on the host does not reach it, and a
    /// grant on it does not reach the host.
    #[test]
    fn a_grant_on_a_host_does_not_reach_the_spaces_it_provides() {
        let dir = tempfile::tempdir().unwrap();
        let a = Access::new(dir.path());
        let host = "relay:hostmachine1";
        let space = "relay:space-0123456789abcdef";
        a.grant(&Yes, "ada", host, None).unwrap();
        assert_eq!(
            a.check("ada", space, "call_tool click").unwrap_err().tag(),
            "forbidden"
        );
        a.revoke("ada", Some(host)).unwrap();
        a.grant(&Yes, "ada", space, None).unwrap();
        a.check("ada", space, "call_tool click").unwrap();
        assert!(a.check("ada", host, "call_tool click").is_err());
    }

    #[test]
    fn grants_are_per_agent_revocable_and_audited() {
        let dir = tempfile::tempdir().unwrap();
        let a = Access::new(dir.path());
        let mac = "relay:0123456789abcdef";
        assert_eq!(
            a.check("ada", mac, "call_tool click").unwrap_err().tag(),
            "forbidden"
        );
        assert_eq!(
            a.grant(&cua_volume::drive::NoPresence, "ada", mac, None)
                .unwrap_err()
                .tag(),
            "not_confirmed"
        );
        a.grant(&Yes, "ada", mac, None).unwrap();
        a.check("ada", mac, "call_tool click").unwrap();
        assert!(
            a.check("bob", mac, "x").is_err(),
            "per agent, not per account"
        );
        assert!(a.check("ada", "relay:other", "x").is_err(), "per machine");
        assert_eq!(a.revoke("ada", Some(mac)).unwrap(), 1);
        assert!(a.check("ada", mac, "x").is_err());
        let (events, ok) = a.audit().tail(20).unwrap();
        assert!(ok.is_ok());
        let actions: Vec<&str> = events.iter().rev().map(|e| e.action.as_str()).collect();
        assert_eq!(
            actions,
            [
                "denied", "grant", "use", "denied", "denied", "revoke", "denied"
            ]
        );
    }
}
