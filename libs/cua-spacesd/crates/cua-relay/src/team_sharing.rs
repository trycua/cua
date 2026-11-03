// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sharing a computer only with people in the owner's team (CUA-1883).
//!
//! The relay stores no team data. The Spaces API says who is in the owner's
//! team (Keycloak organizations, with Stripe Entitlements for the plan) and
//! the relay enforces the answer:
//!
//! - when a share is created (`PATCH` of `allow` or `viewers`), through
//!   `POST /internal/v1/share-check` ([`OrgAccess::share_check`]);
//! - each time a shared person connects (same call, cached 60 s, fail closed
//!   for a person never asked about while the API is down);
//! - on start and daily, [`sweep`] sends every computer's shares to
//!   `POST /internal/v1/share-sweep`, removes the people it names from
//!   `allow` and `viewers` and cuts their open sessions. A repeat finds
//!   nobody left to remove.
//!
//! Switched on with `CUA_RELAY_TEAM_ONLY_SHARING`.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::devices::AuditEvent;
use crate::directory::{MachinePatch, MachineRecord};
use crate::oidc::Identity;
use crate::org_access::OrgAccess;
use crate::server::Relay;

/// What one clean-up run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Computers sent to the Spaces API.
    pub checked: usize,
    /// People removed from a computer's lists (a person on two computers
    /// counts twice).
    pub removed: usize,
}

/// The names the Spaces API may know a person by: account id and email.
pub fn who_keys(who: &Identity) -> Vec<String> {
    let mut keys = vec![who.user.clone()];
    if who.account != who.user {
        keys.push(who.account.clone());
    }
    if let Some(email) = who.email.as_deref().filter(|e| !e.is_empty()) {
        keys.push(email.to_owned());
    }
    keys
}

/// Sends every computer's shares to the Spaces API and removes the people
/// it returns, cutting their open sessions. Nobody is removed when the API
/// cannot be asked.
pub async fn sweep(relay: &Relay) -> Result<SweepReport, String> {
    let machines: Vec<serde_json::Value> = relay
        .directory
        .all()
        .into_iter()
        .filter(|r| !r.allow.is_empty() || !r.viewers.is_empty())
        .map(|r| {
            let mut who = r.allow.clone();
            who.extend(r.viewers.iter().cloned());
            who.dedup();
            serde_json::json!({
                "machine_id": r.id,
                "machine_name": r.name,
                "owner_sub": r.owner.id,
                "who": who,
            })
        })
        .collect();
    let mut report = SweepReport {
        checked: machines.len(),
        removed: 0,
    };
    if machines.is_empty() {
        return Ok(report);
    }
    // The API caps one call; send the computers in batches.
    let mut remove: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for batch in machines.chunks(1000) {
        let body = serde_json::json!({ "machines": batch });
        let (status, reply) = relay
            .org
            .post_json_reply("/internal/v1/share-sweep", &body)
            .await?;
        if !status.is_success() {
            return Err(format!("Spaces API: HTTP {status}"));
        }
        for item in reply
            .get("remove")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
        {
            let Some(id) = item.get("machine_id").and_then(|m| m.as_str()) else {
                continue;
            };
            let who = item
                .get("who")
                .and_then(|w| w.as_array())
                .into_iter()
                .flatten()
                .filter_map(|w| w.as_str().map(str::to_owned));
            remove.entry(id.to_owned()).or_default().extend(who);
        }
    }
    let mut touched = Vec::new();
    for (id, who) in &remove {
        match relay.directory.remove_shares(id, who) {
            Ok(Some(record)) => {
                report.removed += who.len();
                touched.push(id.clone());
                tracing::info!(machine = %id, people = who.len(), "outside shares removed (team-only sharing)");
                audit_sweep(relay, &record, who);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(machine = %id, error = %e, "share sweep: could not save"),
        }
    }
    if !touched.is_empty() {
        relay.org.forget_shares();
        relay.reauthorize(Some(&touched));
    }
    Ok(report)
}

/// Runs [`sweep`] once now and then every `sweep_every`, until the process
/// ends. Does nothing unless team-only sharing is on and an API is set.
pub fn spawn(relay: &Relay) {
    let org: &OrgAccess = &relay.org;
    if !org.team_only_sharing() || !org.has_api() {
        return;
    }
    let relay = relay.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(relay.org.sweep_every());
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match sweep(&relay).await {
                Ok(r) => {
                    tracing::info!(checked = r.checked, removed = r.removed, "share sweep done")
                }
                Err(e) => tracing::warn!(error = %e, "share sweep failed; nobody removed"),
            }
        }
    });
}

/// Refuses a `PATCH` that shares `before` with someone outside the owner's
/// team, when team-only sharing is on: the Spaces API says who is in the
/// owner's team. Removing people is never refused.
#[allow(clippy::result_large_err)]
pub(crate) async fn check_added_shares(
    relay: &Relay,
    id: &str,
    before: &MachineRecord,
    patch: &MachinePatch,
) -> Result<(), Response> {
    if !relay.org.team_only_sharing() {
        return Ok(());
    }
    let added = added_shares(before, patch);
    if added.is_empty() {
        return Ok(());
    }
    match relay.org.share_check(&before.owner.id, &added).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(crate::org_access::ShareRefusal::NeedsTeam)) => Err(error_code(
            StatusCode::FORBIDDEN,
            "sharing_needs_team",
            "Sharing is for teams. Start a team to share this computer.",
        )),
        Ok(Err(crate::org_access::ShareRefusal::OutsideTeam(who))) => Err(error_code(
            StatusCode::FORBIDDEN,
            "share_outside_team",
            outside_team_message(&who),
        )),
        Err(e) => {
            tracing::warn!(machine = %id, error = %e, "share refused: the Spaces API is unavailable");
            Err(error_code(
                StatusCode::SERVICE_UNAVAILABLE,
                "share_check_unavailable",
                "Sharing can't be checked right now. Try again in a minute.",
            ))
        }
    }
}

/// The newly shared names in `patch`: not in `before`, any case.
/// What a share with people outside the team says (CUA-2836). Matches the
/// apps' words: "alex@x.test isn't in your team. Invite them to your team
/// first."
fn outside_team_message(who: &[String]) -> String {
    match who {
        [one] => format!("{one} isn't in your team. Invite them to your team first."),
        [] => "That person isn't in your team. Invite them to your team first.".into(),
        many => format!(
            "{} aren't in your team. Invite them to your team first.",
            many.join(", ")
        ),
    }
}

fn added_shares(before: &MachineRecord, patch: &MachinePatch) -> Vec<String> {
    let known = |n: &String| {
        before
            .allow
            .iter()
            .chain(before.viewers.iter())
            .any(|b| b.eq_ignore_ascii_case(n))
    };
    let mut added: Vec<String> = Vec::new();
    for n in patch
        .allow
        .iter()
        .flatten()
        .chain(patch.viewers.iter().flatten())
    {
        if !known(n) && !added.iter().any(|a| a.eq_ignore_ascii_case(n)) {
            added.push(n.clone());
        }
    }
    added
}

/// A refusal with a code the apps and the CLI turn into plain words.
fn error_code(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({"error": message.into(), "code": code})),
    )
        .into_response()
}

/// Audit lines for people the team-only clean-up removed.
fn audit_sweep(relay: &Relay, record: &MachineRecord, removed: &[String]) {
    for who in removed {
        relay.devices.audit(
            &record.owner.id,
            AuditEvent::new("share_removed")
                .machine(&record.id)
                .subject(who.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::outside_team_message;

    #[test]
    fn outside_team_message_has_no_double_negative() {
        assert_eq!(
            outside_team_message(&["alex@x.test".into()]),
            "alex@x.test isn't in your team. Invite them to your team first."
        );
        assert_eq!(
            outside_team_message(&["a@x.test".into(), "b@x.test".into()]),
            "a@x.test, b@x.test aren't in your team. Invite them to your team first."
        );
        assert!(!outside_team_message(&[]).contains("not in"));
    }
}
