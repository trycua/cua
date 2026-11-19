// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Messages sent to a member with a key (`client_id`): the first answer is
//! kept, so the same message sent again returns it and delivers nothing
//! twice. A phone on a bad connection retries; the agent must not hear the
//! same words twice.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::{Persistent, locked_json, read_json};
use cua_spaces::Result;
use cua_spaces::extension::Caller;

/// Answers kept, and how long.
const KEEP: usize = 500;
const KEEP_MS: u64 = 24 * 3600 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Sent {
    account: String,
    member: String,
    client_id: String,
    at_ms: u64,
    answer: Value,
}

fn path(p: &Persistent) -> std::path::PathBuf {
    p.dir().join("phone-messages.json")
}

fn who(caller: &Caller) -> String {
    match caller {
        Caller::Relay { account, .. } => account.clone(),
        _ => "owner".into(),
    }
}

/// The answer to `client_id` sent to `member` before, if any, marked
/// `replayed`.
pub(crate) fn replay(
    p: &Persistent,
    caller: &Caller,
    member: &str,
    client_id: &str,
) -> Result<Option<Value>> {
    let account = who(caller);
    let all: Vec<Sent> = read_json(&path(p))?;
    Ok(all
        .into_iter()
        .find(|s| s.account == account && s.member == member && s.client_id == client_id)
        .map(|s| {
            let mut a = s.answer;
            if let Some(o) = a.as_object_mut() {
                o.insert("replayed".into(), Value::Bool(true));
            }
            a
        }))
}

/// Keeps `answer` for `client_id`.
pub(crate) fn remember(
    p: &Persistent,
    caller: &Caller,
    member: &str,
    client_id: &str,
    answer: &Value,
) -> Result<()> {
    let rec = Sent {
        account: who(caller),
        member: member.to_string(),
        client_id: client_id.to_string(),
        at_ms: cua_volume::now_ms(),
        answer: answer.clone(),
    };
    locked_json(&path(p), move |all: &mut Vec<Sent>| {
        let now = cua_volume::now_ms();
        all.retain(|s| now.saturating_sub(s.at_ms) < KEEP_MS);
        all.push(rec);
        let extra = all.len().saturating_sub(KEEP);
        all.drain(..extra);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_answer_is_kept_per_account_member_and_key() {
        use crate::persistent::SpacesPersistent as _;
        let dir = tempfile::tempdir().unwrap();
        let spaces = crate::register(
            cua_spaces::Spaces::builder().home(dir.path()),
            cua_volume::Drive::open_local(dir.path()),
            None,
        )
        .build();
        let p = spaces.persistent();
        let ada = Caller::Relay {
            account: "ada".into(),
            name: None,
            role: "owner".into(),
        };
        let bob = Caller::Relay {
            account: "bob".into(),
            name: None,
            role: "shared".into(),
        };
        assert!(replay(&p, &ada, "claude", "key-0001").unwrap().is_none());
        remember(
            &p,
            &ada,
            "claude",
            "key-0001",
            &serde_json::json!({"delivered": true, "mode": "queued"}),
        )
        .unwrap();
        let again = replay(&p, &ada, "claude", "key-0001").unwrap().unwrap();
        assert_eq!(again["mode"], "queued");
        assert_eq!(again["replayed"], true);
        // Another member, another key or another account is not a replay.
        assert!(replay(&p, &ada, "codex", "key-0001").unwrap().is_none());
        assert!(replay(&p, &ada, "claude", "key-0002").unwrap().is_none());
        assert!(replay(&p, &bob, "claude", "key-0001").unwrap().is_none());
    }
}
