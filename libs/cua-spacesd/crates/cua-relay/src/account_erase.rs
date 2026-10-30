// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account erasure (`DELETE /internal/v1/accounts/{sub}?email=<email>`).
//!
//! The Spaces API calls this when a person deletes their account. The relay
//! then erases or revokes everything it holds for that account:
//!
//! - **machines** the account owns (record, live tunnel, team entries,
//!   mesh nodes), and its entries on other people's machines (allowlist and
//!   viewers, by account id and by each `email`);
//! - **access**: live streams and outstanding tickets are cut; the tickets
//!   stay refused for their lifetime by a hash, not by the id;
//! - **devices**, their sessions and the account's audit chain (other
//!   accounts' chains are separate and still verify), pairing codes;
//! - **teams** (leave, delete the ones solely owned, scrub invites and the
//!   audit log), who-reached-what rows (`grants`), usage and queued
//!   analytics events, the cached plan, cached team-access answers, mesh
//!   nodes and pairings, and the account's pool.
//!
//! It needs the internal token (like every `/internal/v1/*` route), is
//! idempotent (a second call, or an unknown account, answers 200 with zero
//! counts) and resumable (a failed write answers 500 with
//! `"complete": false`, and the same call again finishes the job: every
//! store is rewritten on each call). The answer holds counts only, plus
//! the PostHog distinct id the relay used for the account, so the caller
//! can delete that person in PostHog. Logs name the account by a short
//! keyed tag only ([`crate::ident`]).

use std::sync::Mutex;

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::server::Relay;

/// One erase at a time: two calls for one account would only repeat work.
static ERASING: Mutex<()> = Mutex::new(());

/// Longest account id accepted.
const MAX_SUB: usize = 256;
/// Most `email` parameters used.
const MAX_EMAILS: usize = 32;

use crate::ident::tag;

fn failed(step: &str, error: impl std::fmt::Display) -> String {
    format!("{step}: {error}")
}

/// `DELETE /internal/v1/accounts/{sub}?email=<email>` (email optional and
/// repeatable). See the module docs.
pub(crate) async fn erase(
    State(relay): State<Relay>,
    Path(sub): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = crate::hosted::require_internal(&relay, &headers) {
        return r;
    }
    let sub = sub.trim().to_owned();
    if sub.is_empty() || sub.len() > MAX_SUB || sub.chars().any(char::is_control) {
        return (StatusCode::BAD_REQUEST, "invalid account id").into_response();
    }
    let mut emails: Vec<String> = Vec::new();
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let v = v.trim().to_ascii_lowercase();
        if k == "email" && v.contains('@') && v.len() <= 254 && !emails.contains(&v) {
            emails.push(v);
        }
    }
    let tag = tag(&sub);
    let run = {
        let _one = ERASING.lock().unwrap_or_else(|p| p.into_inner());
        run(&relay, &sub, &emails)
    };
    match run {
        Ok(counts) => {
            tracing::info!(account = %tag, counts = %counts, "account erased");
            axum::Json(counts).into_response()
        }
        Err(e) => {
            tracing::warn!(account = %tag, error = %e, "account erase incomplete; retry");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "error": "erase_incomplete",
                    "detail": e,
                    "complete": false,
                })),
            )
                .into_response()
        }
    }
}

fn run(relay: &Relay, sub: &str, given: &[String]) -> Result<serde_json::Value, String> {
    // The addresses the caller named plus every one the relay itself holds
    // for the account, found before anything is erased.
    let mut emails: Vec<String> = given.to_vec();
    let mut held: Vec<String> = Vec::new();
    held.extend(
        relay
            .directory
            .all()
            .into_iter()
            .filter(|r| r.owner.id == sub)
            .filter_map(|r| r.owner.email),
    );
    held.extend(
        relay
            .meter
            .all()
            .into_iter()
            .filter(|r| r.user == sub)
            .filter_map(|r| r.email),
    );
    held.extend(relay.teams.emails_of(sub));
    held.extend(relay.mesh.emails_of(sub));
    held.extend(relay.pairing.emails_of(sub));
    held.extend(relay.grants.emails_of(sub));
    for e in held {
        let e = e.trim().to_ascii_lowercase();
        if e.contains('@') && !emails.contains(&e) && emails.len() < MAX_EMAILS {
            emails.push(e);
        }
    }
    let emails = &emails[..];
    let mut people = vec![sub.to_owned()];
    people.extend(emails.iter().cloned());
    let me = |s: &str| people.iter().any(|p| p.eq_ignore_ascii_case(s));

    // 1. Cut what is live first: nothing new is authorized from here on, and
    //    the rest of the work races no open stream.
    let mut sessions_cut = 0;
    for id in relay.machine_ids() {
        let Some(machine) = relay.machine(&id) else {
            continue;
        };
        for (user, c) in machine.presence() {
            if me(&user) || c.email.as_deref().is_some_and(me) {
                machine.cut_user(&user);
                sessions_cut += 1;
            }
        }
    }
    relay.cut_account(sub, None);

    // 2. Machines the account owns.
    let mut machine_ids: Vec<String> = Vec::new();
    for record in relay.directory.all() {
        if record.owner.id != sub {
            continue;
        }
        match relay.directory.remove(&record.id) {
            Ok(_) | Err(crate::directory::DirectoryError::NotFound) => {}
            Err(e) => return Err(failed("machines", format!("{e:?}"))),
        }
        relay.drop_machine(&record.id);
        relay.teams.forget_machine(&record.id);
        machine_ids.push(record.id);
    }

    // 3. Its entries on other people's machines.
    let mut shares_removed = 0;
    let mut touched: Vec<String> = Vec::new();
    for record in relay.directory.all() {
        if record.owner.id == sub {
            continue;
        }
        let n = record
            .allow
            .iter()
            .chain(record.viewers.iter())
            .filter(|a| me(a))
            .count();
        if n == 0 {
            continue;
        }
        match relay.directory.remove_shares(&record.id, &people) {
            Ok(Some(_)) => {
                shares_removed += n;
                touched.push(record.id);
            }
            Ok(None) => {}
            Err(e) => return Err(failed("shares", format!("{e:?}"))),
        }
    }

    // 4. Teams (so team access to others' machines goes too).
    let teams = relay
        .teams
        .erase_account(sub, emails)
        .map_err(|e| failed("teams", e))?;

    // 5. Whatever the account still reaches loses it: cut again with the
    //    sharing and teams changes in, and re-check the machines it was
    //    removed from.
    relay.cut_account(sub, None);
    if !touched.is_empty() {
        relay.reauthorize(Some(&touched));
    }

    // 6. The stores.
    let devices = relay
        .devices
        .erase_account(sub, emails)
        .map_err(|e| failed("devices", e))?;
    let pairing_codes = relay
        .pairing
        .erase_account(sub)
        .map_err(|e| failed("pairing", e))?;
    let grants = relay
        .grants
        .erase_account(sub, emails)
        .map_err(|e| failed("grants", e))?;
    let usage = relay
        .meter
        .erase_user(sub)
        .map_err(|e| failed("usage", e))?;
    let plan_cache = match &relay.plans {
        Some(plans) => plans.forget(sub).map_err(|e| failed("plans", e))?,
        None => false,
    };
    relay.org.erase_account(sub, emails, &machine_ids);
    let mesh_nodes = relay
        .mesh
        .erase_account(sub, &machine_ids)
        .map_err(|e| failed("mesh", e))?;
    relay.mesh.changed();
    let pool = relay
        .pool
        .erase_account(sub)
        .map_err(|e| failed("pool", e))?;

    Ok(serde_json::json!({
        "machines": machine_ids.len(),
        "shares_removed": shares_removed,
        "devices": devices.devices,
        "pairing_codes": pairing_codes,
        "sessions_cut": sessions_cut + devices.sessions,
        "mesh_nodes": mesh_nodes,
        "usage": usage,
        "plan_cache": plan_cache,
        "pool": pool,
        "teams": teams,
        "grants": grants,
        "posthog_distinct_id": crate::meter::posthog_distinct_id(sub),
        "complete": true,
    }))
}
