// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Join codes and agents connected from a computer.
//!
//! "Connect your own agent" gives the user one line with a join code
//! (`7KQ2-9FXA`). The agent runs `cua join <code>` on the user's computer,
//! which redeems the code on this daemon ([`Persistent::redeem`]): the code
//! is single use, Space-scoped, expires (1 h by default, 15 min to 1 day),
//! is stored only as its SHA-256 (`<cua home>/persistent/join-codes.json`)
//! and needs the redeemer's Cua sign-in. Redeeming creates an external
//! agent member ([`super::identity::Origin::External`]) and its member key,
//! returned once to the CLI, which keeps it in the keychain.
//!
//! Every later call from that agent carries the key (and, when the
//! redeeming computer has a device key, a fresh signature by it), and the
//! daemon turns it into [`Caller::Member`] ([`Persistent::member_auth`]):
//! posts are stamped `agent:<handle>` with the owner and device from the
//! record. Removing the member drops the key's hash, so the key stops at
//! once.
//!
//! When the code asks for it (or the redeemer is someone other than this
//! machine's user and the Space's "Ask before members' agents join" setting
//! is on), the member waits as `pending` behind a `join_request` entry that
//! the owner or an admin answers like an approval.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::identity::{
    self, Device, HarnessInfo, Joined, Origin, Person, Stored, is_external, same_digest, sha256_hex,
};
use super::{AgentRecord, Persistent, locked_json, read_json};
use crate::thread::policy::{self, Group, Rights, Role, Who};
use crate::thread::{Author, Draft, EntryKind, JoinRequest, ThreadStore};
use cua_spaces::extension::Caller;
use cua_spaces::{Error, Result};

/// A code's default life.
pub const DEFAULT_TTL_S: u64 = 3600;
/// The shortest life a code may be given.
pub const MIN_TTL_S: u64 = 15 * 60;
/// The longest.
pub const MAX_TTL_S: u64 = 24 * 3600;
/// The characters of a code (no 0/O, 1/I/L, U).
pub const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters per code (shown as two groups of four).
pub const CODE_LEN: usize = 8;
/// Where a code's link points (the static skill doc for agents).
pub const JOIN_URL: &str = "https://cua.ai/join/";
/// Wrong codes one account may try per [`ACCOUNT_WINDOW_MS`].
pub const ACCOUNT_GUESSES: usize = 5;
pub const ACCOUNT_WINDOW_MS: u64 = 15 * 60 * 1000;
/// Wrong codes this daemon takes per [`HOST_WINDOW_MS`] before it cancels
/// every open code (each code is then out of reach of guessing).
pub const HOST_GUESSES: usize = 30;
pub const HOST_WINDOW_MS: u64 = 60 * 60 * 1000;
/// Posts per connected member.
pub const POSTS_PER_MINUTE: usize = 30;
pub const POSTS_PER_HOUR: usize = 600;
/// How far a member call's signed time may be from ours.
pub const SIGNATURE_SKEW_MS: u64 = 5 * 60 * 1000;
/// How often `last_seen_ms` is written.
const SEEN_EVERY_MS: u64 = 30_000;
/// Ended codes are kept this long (for the sheet's list), then dropped.
const KEEP_ENDED_MS: u64 = 7 * 24 * 3600 * 1000;

/// What every refused code is told (the daemon never says which it was).
pub const CODE_REFUSED: &str = "This code doesn't work: it was used, cancelled or has expired, or it is for a Space on another computer. Ask for a new link in Add an agent.";
/// A code for another account.
pub const OTHER_ACCOUNT: &str = "This code is for another Cua account's Space. Joining a teammate's Space from your computer isn't available yet; ask them to start an agent in the Space instead.";
/// Not signed in.
pub const SIGN_IN_FIRST: &str =
    "Sign in to Cua first (cua auth login), then run cua join <code> again.";
/// A key that no longer works.
pub const KEY_REVOKED: &str = "This agent's member key no longer works: it was removed from the Space. Ask for a new link in Add an agent.";

/// A join code's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeState {
    Open,
    PendingApproval,
    Redeemed,
    Denied,
    Cancelled,
}

/// One join code (only its hash).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinCode {
    pub id: String,
    /// The Space (an id).
    pub space: String,
    /// SHA-256 (hex) of the normalized code.
    pub sha256: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    pub created_by: Person,
    /// `owner` (its creator only) or `members`.
    pub can_task: String,
    pub ask_before_join: bool,
    pub state: CodeState,
    /// The member it made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
}

impl JoinCode {
    /// The state the sheet shows (`expired` for an open code past its time).
    pub fn shown_state(&self, now: u64) -> &'static str {
        match self.state {
            CodeState::Open if now >= self.expires_ms => "expired",
            CodeState::Open => "open",
            CodeState::PendingApproval => "pending_approval",
            CodeState::Redeemed => "redeemed",
            CodeState::Denied => "denied",
            CodeState::Cancelled => "cancelled",
        }
    }
}

/// A Space's thread settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSettings {
    pub space: String,
    /// "Ask before members' agents join" (default on).
    #[serde(default = "super::yes")]
    pub ask_before_join: bool,
}

/// The computer redeeming a code, as `cua join` reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct DeviceArg {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Its device key (base64url): every later call is signed with it.
    #[serde(default)]
    pub public_key: Option<String>,
}

/// What a connected agent sends with each call (`_member`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MemberCredential {
    pub member_id: String,
    pub key: String,
    #[serde(default)]
    pub ts_ms: Option<u64>,
    /// The device key's signature of
    /// [`cua_host::device::member_message`]`(member_id, ts_ms)`.
    #[serde(default)]
    pub signature: Option<String>,
}

/// The per-process counters: wrong codes and posts.
#[derive(Default)]
pub(crate) struct LiveJoin {
    guesses: HashMap<String, VecDeque<u64>>,
    host_guesses: VecDeque<u64>,
    posts: HashMap<String, VecDeque<u64>>,
    /// Members over their post limit (one note per episode).
    limited: HashSet<String>,
}

fn prune(q: &mut VecDeque<u64>, now: u64, window: u64) {
    while q.front().is_some_and(|t| now.saturating_sub(*t) >= window) {
        q.pop_front();
    }
}

/// A new code (`XXXX-XXXX`).
pub fn new_code() -> String {
    let bytes: [u8; CODE_LEN] = rand::random();
    let chars: String = bytes
        .iter()
        .map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// A code as typed or pasted (case, dashes, spaces, a whole link) in the
/// form that is hashed.
pub fn normalize_code(code: &str) -> String {
    let code = code.trim();
    let code = code.rsplit('/').next().unwrap_or(code);
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn denied(msg: impl Into<String>) -> Error {
    policy::denied(msg)
}

fn rate_limited(msg: impl Into<String>) -> Error {
    Error::extension("rate_limited", msg)
}

/// A person's possessive ("Ada's").
pub fn possessive(name: &str) -> String {
    if name.ends_with('s') {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}

impl Persistent {
    fn codes_path(&self) -> PathBuf {
        self.dir.join("join-codes.json")
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("thread-settings.json")
    }

    /// Every join code (hashes only).
    pub fn join_codes(&self) -> Result<Vec<JoinCode>> {
        read_json(&self.codes_path())
    }

    /// A Space's name as the user sees it (its id when unknown).
    pub fn space_name(&self, id: &str) -> String {
        self.spaces
            .list()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == id))
            .map(|s| s.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string())
    }

    /// `space`'s "Ask before members' agents join" setting.
    pub fn ask_before_join(&self, space: &str) -> bool {
        read_json::<SpaceSettings>(&self.settings_path())
            .ok()
            .and_then(|all| all.into_iter().find(|s| s.space == space))
            .is_none_or(|s| s.ask_before_join)
    }

    /// `thread_settings`: reads (everyone) or sets (owner and admins) a
    /// Space's thread settings.
    pub fn thread_settings(
        &self,
        caller: &Caller,
        space: &str,
        ask: Option<bool>,
    ) -> Result<Value> {
        let id = self.spaces.resolve(space)?.to_string();
        if let Some(ask) = ask {
            if !matches!(self.role_of(caller), Role::Owner | Role::Admin) {
                return Err(denied(
                    "only the owner and admins change this Space's settings",
                ));
            }
            locked_json(
                &self.settings_path(),
                |all: &mut Vec<SpaceSettings>| match all.iter_mut().find(|s| s.space == id) {
                    Some(s) => s.ask_before_join = ask,
                    None => all.push(SpaceSettings {
                        space: id.clone(),
                        ask_before_join: ask,
                    }),
                },
            )?;
        }
        Ok(json!({"space": id, "ask_before_join": self.ask_before_join(&id)}))
    }

    /// `thread_join_code_create`: a new code for `space`; its plaintext is
    /// returned only here.
    pub fn join_code_create(
        &self,
        caller: &Caller,
        space: &str,
        ttl_s: Option<u64>,
        can_task: Option<&str>,
        ask_before_join: Option<bool>,
    ) -> Result<Value> {
        if caller.agent_member().is_some() {
            return Err(denied("agents can't make join codes"));
        }
        policy::may_write(self.role_of(caller))?;
        let id = self.spaces.resolve(space)?.to_string();
        let ttl = ttl_s.unwrap_or(DEFAULT_TTL_S);
        if !(MIN_TTL_S..=MAX_TTL_S).contains(&ttl) {
            return Err(Error::invalid(format!(
                "ttl_s: {MIN_TTL_S} (15 minutes) to {MAX_TTL_S} (1 day)"
            )));
        }
        let can_task = can_task.unwrap_or("owner");
        if !matches!(can_task, "owner" | "members") {
            return Err(Error::invalid("can_task: use \"owner\" or \"members\""));
        }
        let code = new_code();
        let now = cua_volume::now_ms();
        let rec = JoinCode {
            id: format!("jc_{}", hex::encode(rand::random::<[u8; 6]>())),
            space: id.clone(),
            sha256: sha256_hex(&normalize_code(&code)),
            created_ms: now,
            expires_ms: now + ttl * 1000,
            created_by: Person::from(&self.author_for(caller)),
            can_task: can_task.into(),
            ask_before_join: ask_before_join.unwrap_or(false),
            state: CodeState::Open,
            member_id: None,
            ended_ms: None,
        };
        let out = rec.clone();
        locked_json(&self.codes_path(), |all: &mut Vec<JoinCode>| {
            all.retain(|c| {
                c.ended_ms
                    .or((c.state == CodeState::Open).then_some(c.expires_ms))
                    .is_none_or(|t| now.saturating_sub(t) < KEEP_ENDED_MS)
            });
            all.push(rec);
        })?;
        let url = format!("{JOIN_URL}{code}");
        Ok(json!({
            "id": out.id,
            "code": code,
            "url": url,
            "line": format!("Join my Cua Space \"{}\": {url}", self.space_name(&id)),
            "expires_at_ms": out.expires_ms,
        }))
    }

    /// `thread_join_code_list`: the owner and admins see every code of the
    /// Space, others their own; newest first.
    pub fn join_code_list(&self, caller: &Caller, space: &str) -> Result<Value> {
        let id = self.spaces.resolve(space)?.to_string();
        let me = self.author_for(caller).id;
        let all_codes = matches!(self.role_of(caller), Role::Owner | Role::Admin);
        let now = cua_volume::now_ms();
        let members = self.list()?;
        let mut codes: Vec<JoinCode> = self
            .join_codes()?
            .into_iter()
            .filter(|c| c.space == id && (all_codes || c.created_by.id == me))
            .collect();
        codes.sort_by_key(|b| std::cmp::Reverse(b.created_ms));
        let rows: Vec<Value> = codes
            .iter()
            .map(|c| {
                let mut row = json!({
                    "id": c.id,
                    "created_at_ms": c.created_ms,
                    "expires_at_ms": c.expires_ms,
                    "state": c.shown_state(now),
                    "can_task": c.can_task,
                    "ask_before_join": c.ask_before_join,
                    "created_by": c.created_by,
                });
                let handle = c.member_id.as_ref().and_then(|m| {
                    members
                        .iter()
                        .find(|r| r.identity.as_ref().is_some_and(|i| &i.member_id == m))
                        .map(|r| r.name.clone())
                });
                if let Some(h) = handle {
                    row["member_handle"] = json!(h);
                }
                row
            })
            .collect();
        Ok(json!({"codes": rows}))
    }

    /// `thread_join_code_cancel`: an open code stops working; a pending
    /// one's member is dropped and its join request closed. Its creator,
    /// the owner or an admin.
    pub fn join_code_cancel(&self, caller: &Caller, space: &str, id: &str) -> Result<Value> {
        let sid = self.spaces.resolve(space)?.to_string();
        let me = self.author_for(caller);
        let code = self
            .join_codes()?
            .into_iter()
            .find(|c| c.id == id && c.space == sid)
            .ok_or_else(|| Error::NotFound(format!("join code {id} in this Space")))?;
        if code.created_by.id != me.id && !matches!(self.role_of(caller), Role::Owner | Role::Admin)
        {
            return Err(denied(
                "only who made a code, the owner or an admin cancels it",
            ));
        }
        match code.state {
            CodeState::Open => {}
            CodeState::PendingApproval => {
                if let Some(rec) = self.external_by_member(code.member_id.as_deref()) {
                    self.close_join_request(&rec, &me, "the code was cancelled")?;
                }
            }
            CodeState::Cancelled => return Ok(json!({"id": id, "state": "cancelled"})),
            other => {
                return Err(Error::invalid(format!(
                    "join code {id} was already {}",
                    code.shown_state(0).replace('_', " ").to_lowercase()
                        + if other == CodeState::Redeemed {
                            "; remove the agent instead"
                        } else {
                            ""
                        }
                )));
            }
        }
        self.set_code_state(id, CodeState::Cancelled)?;
        Ok(json!({"id": id, "state": "cancelled"}))
    }

    fn set_code_state(&self, id: &str, state: CodeState) -> Result<()> {
        let now = cua_volume::now_ms();
        locked_json(&self.codes_path(), |all: &mut Vec<JoinCode>| {
            if let Some(c) = all.iter_mut().find(|c| c.id == id) {
                c.state = state;
                if state != CodeState::PendingApproval {
                    c.ended_ms = Some(now);
                }
            }
        })
    }

    fn external_by_member(&self, member_id: Option<&str>) -> Option<AgentRecord> {
        let m = member_id?;
        self.list()
            .ok()?
            .into_iter()
            .find(|r| is_external(r) && r.identity.as_ref().is_some_and(|i| i.member_id == m))
    }

    /// Counts a wrong code for `account` (and this daemon); past the
    /// daemon's limit every open code is cancelled.
    async fn wrong_guess(&self, account: &str) -> Result<()> {
        let now = cua_volume::now_ms();
        let flood = {
            let mut live = self.live.lock().await;
            let j = &mut live.join;
            j.guesses
                .entry(account.to_string())
                .or_default()
                .push_back(now);
            prune(&mut j.host_guesses, now, HOST_WINDOW_MS);
            j.host_guesses.push_back(now);
            j.host_guesses.len() >= HOST_GUESSES
        };
        if flood {
            let cancelled = locked_json(&self.codes_path(), |all: &mut Vec<JoinCode>| {
                let mut spaces = HashSet::new();
                for c in all
                    .iter_mut()
                    .filter(|c| c.state == CodeState::Open && c.expires_ms > now)
                {
                    c.state = CodeState::Cancelled;
                    c.ended_ms = Some(now);
                    spaces.insert(c.space.clone());
                }
                spaces
            })?;
            for space in cancelled {
                let store = self.thread_store(&space);
                self.rebuild_once(&store).await;
                store.append(Draft::note(
                    "join",
                    None,
                    "Open join links were cancelled after too many wrong codes. Make a new one in Add an agent.",
                ))?;
            }
            self.live.lock().await.join.host_guesses.clear();
        }
        Ok(())
    }

    /// `thread_join_redeem` (`cua join`): redeems `code` on this daemon for
    /// the signed-in caller on `device`, creating an external member. The
    /// member key is in the result, once.
    pub async fn redeem(&self, caller: &Caller, code: &str, device: DeviceArg) -> Result<Value> {
        if caller.agent_member().is_some() {
            return Err(denied("an agent can't redeem a join code"));
        }
        policy::may_write(self.role_of(caller))?;
        let me = self.author_for(caller);
        if me.id.is_empty() || (matches!(caller, Caller::Owner) && me.id == Author::owner().id) {
            return Err(Error::extension("not_signed_in", SIGN_IN_FIRST));
        }
        let now = cua_volume::now_ms();
        {
            let mut live = self.live.lock().await;
            let q = live.join.guesses.entry(me.id.clone()).or_default();
            prune(q, now, ACCOUNT_WINDOW_MS);
            if q.len() >= ACCOUNT_GUESSES {
                let wait = ACCOUNT_WINDOW_MS - now.saturating_sub(*q.front().unwrap_or(&now));
                return Err(rate_limited(format!(
                    "Too many wrong codes. Wait {} minutes, then check the link and try again.",
                    wait.div_ceil(60_000)
                )));
            }
        }
        let device = check_device(device)?;
        let digest = sha256_hex(&normalize_code(code));
        let found = self.join_codes()?.into_iter().find(|c| {
            same_digest(&c.sha256, &digest) && c.state == CodeState::Open && c.expires_ms > now
        });
        let Some(code) = found else {
            self.wrong_guess(&me.id).await?;
            return Err(Error::extension("join_code_refused", CODE_REFUSED));
        };
        if code.created_by.id != me.id {
            return Err(denied(OTHER_ACCOUNT));
        }
        let machine_owner = self.owner();
        let ask = code.ask_before_join
            || (me.id != machine_owner.id && self.ask_before_join(&code.space));
        // Claim the code before anything else can.
        let claimed = locked_json(&self.codes_path(), |all: &mut Vec<JoinCode>| {
            match all
                .iter_mut()
                .find(|c| c.id == code.id && c.state == CodeState::Open)
            {
                Some(c) => {
                    c.state = if ask {
                        CodeState::PendingApproval
                    } else {
                        CodeState::Redeemed
                    };
                    if !ask {
                        c.ended_ms = Some(now);
                    }
                    true
                }
                None => false,
            }
        })?;
        if !claimed {
            return Err(Error::extension("join_code_refused", CODE_REFUSED));
        }
        let key = identity::new_key();
        let member_id = format!("mem_{}", hex::encode(rand::random::<[u8; 8]>()));
        let owner = Person::from(&me);
        let rights = Rights {
            task: match code.can_task.as_str() {
                "members" => Who::Group(Group::Members),
                _ if code.created_by.id == machine_owner.id => Who::Group(Group::Owner),
                _ => Who::Accounts(vec![code.created_by.id.clone()]),
            },
            control: Who::Group(Group::Admins),
            // Its owner answers its approvals ([`Persistent::allows`]).
            approve: Who::Group(Group::Owner),
        };
        let stored = Stored {
            origin: Origin::External,
            member_id: member_id.clone(),
            display_name: "Agent".into(),
            icon: None,
            harness: HarnessInfo {
                id: "other".into(),
                version: None,
                reported: false,
            },
            owner: owner.clone(),
            device: Some(Device {
                id: device.id.clone(),
                name: device.name.clone(),
            }),
            joined: Joined {
                method: "prompt".into(),
                at_ms: now,
                by: owner.clone(),
            },
            pending: ask,
            provisional: true,
            last_seen_ms: Some(now),
            device_public_key: device.public_key.clone(),
            key_sha256: Some(sha256_hex(&key)),
            code_id: Some(code.id.clone()),
            request_seq: None,
        };
        let rec = locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            let handle = identity::free_handle("agent", |h| all.iter().any(|a| a.name == h));
            let rec = AgentRecord {
                name: handle,
                harness: "other".into(),
                space: code.space.clone(),
                model: None,
                base_url: None,
                env_from_host: vec![],
                env: Default::default(),
                created_ms: now,
                paused: false,
                space_state: Default::default(),
                recreate: None,
                run_id: None,
                cursor: 0,
                saved_turn: 0,
                notify: false,
                saved_ms: 0,
                last_error: None,
                notified_error: None,
                sessions: 1,
                policy: super::Policy {
                    rights: Some(rights),
                    ..Default::default()
                },
                added_by: Some(me.clone()),
                member_since_ms: now,
                task: None,
                held_by: None,
                identity: Some(stored),
                redrive: Default::default(),
                durable_space: None,
                setup: String::new(),
                setup_note: String::new(),
                relaunch: false,
                activity: None,
                epoch: 0,
                session_runs: vec![],
                session_k: 0,
                extra: Default::default(),
            };
            all.push(rec.clone());
            rec
        })?;
        locked_json(&self.codes_path(), |all: &mut Vec<JoinCode>| {
            if let Some(c) = all.iter_mut().find(|c| c.id == code.id) {
                c.member_id = Some(member_id.clone());
            }
        })?;
        let store = self.thread_store(&code.space);
        self.rebuild_once(&store).await;
        let ident = identity::identity_of(&rec, &machine_owner);
        let from = device_words(&device.name);
        if ask {
            let mut d = Draft::new(
                Author::system("join"),
                EntryKind::JoinRequest,
                format!(
                    "{} agent asks to join from {from}. The owner or an admin lets it in.",
                    possessive(&owner.name)
                ),
            );
            d.join_request = Some(JoinRequest {
                member: ident.clone(),
                can_task: code.can_task.clone(),
                state: "open".into(),
                code_id: code.id.clone(),
            });
            let e = store.append(d)?;
            self.update(&rec.name, |r| {
                if let Some(i) = r.identity.as_mut() {
                    i.request_seq = Some(e.seq);
                }
            })?;
        } else {
            self.note_joined(&store, &rec, &me)?;
        }
        Ok(json!({
            "space": code.space,
            "space_name": self.space_name(&code.space),
            "space_owner": Person::from(&machine_owner),
            "handle": rec.name,
            "member_id": member_id,
            "state": if ask { "pending" } else { "active" },
            "owner": owner,
            "device": {"id": device.id, "name": device.name},
            "key": key,
        }))
    }

    fn note_joined(&self, store: &ThreadStore, rec: &AgentRecord, by: &Author) -> Result<()> {
        let owner = self.owner();
        let ident = identity::identity_of(rec, &owner);
        let from = ident
            .device
            .as_ref()
            .map(|d| device_words(&d.name))
            .unwrap_or_else(|| "a computer".into());
        store.append(Draft::event(
            "join",
            format!(
                "{} agent joined from {from}.",
                possessive(&ident.owner.name)
            ),
            json!({"kind": "member_joined", "member": ident, "by": Person::from(by)}),
        ))?;
        Ok(())
    }

    /// The answer `approve` to join request `seq` (posted by `caller`):
    /// lets the member in, or drops it.
    pub(crate) fn answer_join_request(
        &self,
        caller: &Caller,
        store: &ThreadStore,
        seq: u64,
        approve: bool,
    ) -> Result<()> {
        let Some(rec) = self.list()?.into_iter().find(|r| {
            r.space == store.space()
                && r.identity
                    .as_ref()
                    .is_some_and(|i| i.pending && i.request_seq == Some(seq))
        }) else {
            return Ok(());
        };
        let by = self.author_for(caller);
        let code = rec.identity.as_ref().and_then(|i| i.code_id.clone());
        if approve {
            self.update(&rec.name, |r| {
                if let Some(i) = r.identity.as_mut() {
                    i.pending = false;
                }
            })?;
            if let Some(c) = &code {
                self.set_code_state(c, CodeState::Redeemed)?;
            }
            let rec = self.get(&rec.name)?;
            self.note_joined(store, &rec, &by)?;
        } else {
            self.drop_external(&rec)?;
            if let Some(c) = &code {
                self.set_code_state(c, CodeState::Denied)?;
            }
            store.append(Draft::note(
                "join",
                None,
                format!(
                    "{} didn't let {} agent join.",
                    by.name,
                    possessive(&identity::stored_of(&rec, &self.owner()).owner.name)
                ),
            ))?;
        }
        Ok(())
    }

    /// Closes `rec`'s open join request without an answer from a person
    /// (`why`), dropping the pending member.
    fn close_join_request(&self, rec: &AgentRecord, by: &Author, why: &str) -> Result<()> {
        let store = self.thread_store(&rec.space);
        if let Some(seq) = rec.identity.as_ref().and_then(|i| i.request_seq) {
            let mut d = Draft::new(Author::system("join"), EntryKind::Answer, String::new());
            d.reply_to = Some(seq);
            d.answer = Some(crate::thread::Answer {
                approval: seq,
                approve: false,
                note: why.into(),
            });
            store.append(d)?;
        }
        self.drop_external(rec)?;
        store.append(Draft::note(
            "join",
            None,
            format!("{} cancelled the join request: {why}.", by.name),
        ))?;
        Ok(())
    }

    /// Forgets an external member's record (its key stops at once).
    fn drop_external(&self, rec: &AgentRecord) -> Result<()> {
        let _ = self.fail_open_tasks(rec, &format!("{} was removed", rec.name));
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            all.retain(|a| a.name != rec.name)
        })?;
        Ok(())
    }

    /// A `member_removed` note for `rec`, removed by `by`.
    pub(crate) fn note_removed(&self, rec: &AgentRecord, by: &Author) -> Result<()> {
        let store = self.thread_store(&rec.space);
        if !store.exists() {
            return Ok(());
        }
        let ident = identity::identity_of(rec, &self.owner());
        let text = if is_external(rec) {
            format!(
                "{} removed {} ({} agent). Its key no longer works.",
                by.name,
                ident.display_name,
                possessive(&ident.owner.name)
            )
        } else {
            format!("{} removed {}.", by.name, ident.display_name)
        };
        store.append(Draft::event(
            "join",
            text,
            json!({"kind": "member_removed", "member": ident, "by": Person::from(by)}),
        ))?;
        Ok(())
    }

    /// Whether `caller` may remove `rec`: the owner, or for a connected
    /// agent also admins and its own owner.
    pub fn may_remove(&self, caller: &Caller, rec: &AgentRecord) -> Result<()> {
        if super::members::may_manage(caller).is_ok() {
            return Ok(());
        }
        if is_external(rec) && caller.agent_member().is_none() {
            let role = self.role_of(caller);
            let owner = identity::stored_of(rec, &self.owner()).owner.id;
            if role == Role::Admin || (role != Role::Guest && self.author_for(caller).id == owner) {
                return Ok(());
            }
        }
        Err(denied(
            "only the owner, an admin or the agent's owner removes it from the Space",
        ))
    }

    /// Removes every connected member bound to `device` (the device was
    /// removed from the account, or signed out): their keys stop at once.
    pub async fn revoke_device(&self, caller: &Caller, device: &str) -> Result<Value> {
        super::members::may_manage(caller)?;
        let by = self.author_for(caller);
        let recs: Vec<AgentRecord> = self
            .list()?
            .into_iter()
            .filter(|r| {
                is_external(r)
                    && r.identity
                        .as_ref()
                        .and_then(|i| i.device.as_ref())
                        .is_some_and(|d| d.id == device)
            })
            .collect();
        let mut removed = vec![];
        for r in recs {
            self.drop_external(&r)?;
            self.note_removed(&r, &by)?;
            removed.push(r.name);
        }
        Ok(json!({"removed": removed}))
    }

    /// Removes every connected member `account` owns (it signed out of
    /// this computer); returns their handles.
    pub async fn drop_members_of(&self, account: &Author) -> Result<Vec<String>> {
        let recs: Vec<AgentRecord> = self
            .list()?
            .into_iter()
            .filter(|r| {
                is_external(r) && identity::stored_of(r, &self.owner()).owner.id == account.id
            })
            .collect();
        let mut gone = vec![];
        for r in recs {
            self.drop_external(&r)?;
            let store = self.thread_store(&r.space);
            if store.exists() {
                let ident = identity::identity_of(&r, &self.owner());
                store.append(Draft::event(
                    "join",
                    format!(
                        "{} signed out of Cua, so {} ({} agent) was removed. Its key no longer works.",
                        account.name,
                        ident.display_name,
                        possessive(&account.name)
                    ),
                    json!({"kind": "member_removed", "member": ident, "by": Person::from(account)}),
                ))?;
            }
            gone.push(r.name);
        }
        Ok(gone)
    }

    /// The caller a member credential stands for ([`Caller::Member`]), or
    /// why it doesn't work. Updates the member's last seen time.
    pub async fn member_auth(&self, cred: &MemberCredential) -> Result<Caller> {
        let rec = self
            .list()?
            .into_iter()
            .find(|r| {
                is_external(r)
                    && r.identity
                        .as_ref()
                        .is_some_and(|i| !i.member_id.is_empty() && i.member_id == cred.member_id)
            })
            .ok_or_else(|| denied(KEY_REVOKED))?;
        let id = rec.identity.clone().expect("external");
        let good = id
            .key_sha256
            .as_deref()
            .is_some_and(|h| same_digest(h, &sha256_hex(&cred.key)));
        if !good {
            return Err(denied(KEY_REVOKED));
        }
        let now = cua_volume::now_ms();
        if let Some(pk) = id.device_public_key.as_deref() {
            let (Some(ts), Some(sig)) = (cred.ts_ms, cred.signature.as_deref()) else {
                return Err(denied(
                    "this member key works only from the computer that joined (its call wasn't signed)",
                ));
            };
            let fresh = now.abs_diff(ts) <= SIGNATURE_SKEW_MS;
            let msg = cua_host::device::member_message(&cred.member_id, ts);
            if !fresh || !cua_host::device::verify_device_signature(pk, &msg, sig) {
                return Err(denied(
                    "this member key works only from the computer that joined",
                ));
            }
        }
        if id.pending {
            return Err(denied(
                "waiting for the Space's owner or an admin to let this agent in",
            ));
        }
        // Its owner must still be the account signed in on this computer.
        let machine = self.owner();
        if machine.id != id.owner.id {
            return Err(denied(format!(
                "{} isn't signed in to Cua on this computer any more, so this agent's key no longer works. Sign in and ask for a new link in Add an agent.",
                id.owner.name
            )));
        }
        if id
            .last_seen_ms
            .is_none_or(|t| now.saturating_sub(t) >= SEEN_EVERY_MS)
        {
            let _ = self.update(&rec.name, |r| {
                if let Some(i) = r.identity.as_mut() {
                    i.last_seen_ms = Some(now);
                }
            });
        }
        Ok(Caller::Member {
            handle: rec.name,
            space: rec.space,
        })
    }

    /// A connected member's post budget: [`POSTS_PER_MINUTE`] and
    /// [`POSTS_PER_HOUR`]. Over it, the post is refused and the thread gets
    /// one note per episode.
    pub(crate) async fn post_allowed(&self, handle: &str, store: &ThreadStore) -> Result<()> {
        let now = cua_volume::now_ms();
        let (over, first) = {
            let mut live = self.live.lock().await;
            let j = &mut live.join;
            let q = j.posts.entry(handle.to_string()).or_default();
            prune(q, now, 3_600_000);
            let minute = q
                .iter()
                .filter(|t| now.saturating_sub(**t) < 60_000)
                .count();
            let over = if minute >= POSTS_PER_MINUTE {
                let oldest = q.iter().find(|t| now.saturating_sub(**t) < 60_000).copied();
                Some((60_000 - now.saturating_sub(oldest.unwrap_or(now))).div_ceil(1000))
            } else if q.len() >= POSTS_PER_HOUR {
                Some((3_600_000 - now.saturating_sub(*q.front().unwrap_or(&now))).div_ceil(1000))
            } else {
                None
            };
            match over {
                Some(_) => (over, j.limited.insert(handle.to_string())),
                None => {
                    q.push_back(now);
                    j.limited.remove(handle);
                    (None, false)
                }
            }
        };
        let Some(wait_s) = over else {
            return Ok(());
        };
        if first && let Ok(rec) = self.get(handle) {
            let ident = identity::identity_of(&rec, &self.owner());
            store.append(Draft::event(
                "limits",
                format!(
                    "{} is posting too fast; its posts are refused for a while.",
                    ident.display_name
                ),
                json!({"kind": "rate_limited", "member": ident, "by": null}),
            ))?;
        }
        Err(rate_limited(format!(
            "over {POSTS_PER_MINUTE} posts a minute or {POSTS_PER_HOUR} an hour; wait {wait_s} s"
        )))
    }

    /// `thread_join`: the calling agent member sets how it looks: its
    /// display name (within the name rules), icon and reported harness. A
    /// connected agent's first call also picks its handle from the name.
    pub async fn thread_join(
        &self,
        caller: &Caller,
        space: &str,
        name: &str,
        harness: Option<&str>,
        version: Option<&str>,
        icon: Option<&str>,
    ) -> Result<Value> {
        let Some((me, own)) = caller.agent_member() else {
            return Err(Error::invalid(
                "thread_join is for agents: people join a Space by being shared it",
            ));
        };
        let t = self.thread(caller, space).await?;
        let rec = self.get(me)?;
        if rec.space != own {
            return Err(denied("an agent joins only its own Space"));
        }
        let store = t.store();
        let owner = self.owner();
        let mut people: Vec<String> = vec![owner.name.clone()];
        people.extend(store.fold()?.people);
        let all = self.list()?;
        let handles: Vec<String> = all
            .iter()
            .filter(|r| r.name != rec.name)
            .map(|r| r.name.clone())
            .collect();
        let shown = match identity::check_name(name, &people, &handles) {
            Ok(n) => n,
            Err(why) => {
                let ident = identity::identity_of(&rec, &owner);
                store.append(Draft::event(
                    "join",
                    format!("{} asked for the name {:?}, which was refused: {why}.", ident.display_name, name.trim()),
                    json!({"kind": "name_refused", "member": ident, "by": null, "name": name.trim()}),
                ))?;
                return Err(Error::invalid(why));
            }
        };
        let icon = match icon.map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some("harness") => Some("harness".to_string()),
            Some(u)
                if u.starts_with("https://")
                    && u.len() <= 512
                    && !u.chars().any(char::is_whitespace) =>
            {
                Some(u.to_string())
            }
            Some(_) => {
                return Err(Error::invalid(
                    "icon: \"harness\" or an https:// image URL (at most 512 characters)",
                ));
            }
        };
        let version = match version.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) if v.len() > 32 || v.chars().any(char::is_control) => {
                return Err(Error::invalid("version: at most 32 characters"));
            }
            v => v.map(str::to_string),
        };
        let external = is_external(&rec);
        let reported = harness
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| identity::harness_id(h).to_string());
        let provisional = rec.identity.as_ref().is_some_and(|i| i.provisional);
        let mut handle = rec.name.clone();
        if provisional {
            let base = identity::slug(&shown);
            let new =
                identity::free_handle(&base, |h| h != rec.name && all.iter().any(|a| a.name == h));
            if new != rec.name {
                self.rename(&rec.name, &new)?;
                handle = new;
            }
        }
        let shown_name = shown.clone();
        self.update(&handle, |r| {
            let i = r
                .identity
                .get_or_insert_with(|| identity::stored_of(&rec, &owner));
            i.display_name = shown_name;
            i.icon = icon;
            i.provisional = false;
            if external {
                if let Some(h) = &reported {
                    i.harness = HarnessInfo {
                        id: h.clone(),
                        version: version.clone(),
                        reported: true,
                    };
                    r.harness = h.clone();
                }
            } else if version.is_some() {
                i.harness.version = version.clone();
            }
        })?;
        let rec = self.get(&handle)?;
        let mut row = super::members::member_row(&rec, &owner);
        let mut rows: Vec<Value> = self
            .members_of(&rec.space)?
            .iter()
            .map(|r| super::members::member_row(r, &owner))
            .collect();
        super::members::with_row_collisions(&mut rows);
        if let Some(me) = rows.iter().find(|r| r["name"] == handle.as_str()) {
            row["identity"]["collides"] = me["identity"]["collides"].clone();
        }
        let ident = &row["identity"];
        let device = ident["device"]["name"].as_str().map(str::to_string);
        Ok(json!({
            "member": row,
            "space": rec.space,
            "space_name": self.space_name(&rec.space),
            "handle": handle,
            "line": match device {
                Some(d) => format!(
                    "Joined {} as @{handle}: {} agent, posting from {d}.",
                    self.space_name(&rec.space),
                    possessive(ident["owner"]["name"].as_str().unwrap_or("Owner"))
                ),
                None => format!("Joined {} as @{handle}.", self.space_name(&rec.space)),
            },
        }))
    }

    /// Renames a member record (a connected agent's first `thread_join`).
    fn rename(&self, from: &str, to: &str) -> Result<()> {
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            if all.iter().any(|a| a.name == to) {
                return Err(Error::invalid(format!("the handle {to} is taken")));
            }
            if let Some(a) = all.iter_mut().find(|a| a.name == from) {
                a.name = to.to_string();
            }
            Ok(())
        })?
    }

    /// Refuses running `rec` in a Space: a connected agent runs on its
    /// owner's computer.
    pub(crate) fn not_external(&self, rec: &AgentRecord) -> Result<()> {
        if !is_external(rec) {
            return Ok(());
        }
        let ident = identity::identity_of(rec, &self.owner());
        Err(Error::invalid(format!(
            "{} is connected from {}; it runs there, not in the Space. @mention it in the thread and it picks the task up from its inbox.",
            rec.name,
            ident
                .device
                .map(|d| device_words(&d.name))
                .unwrap_or_else(|| "its owner's computer".into())
        )))
    }
}

/// A device name as a sentence part ("Ada's MacBook Pro", or "a computer").
fn device_words(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        "a computer".into()
    } else {
        n.to_string()
    }
}

/// Checks what the redeeming computer said about itself: a device key's id
/// must be the one its public key derives.
fn check_device(d: DeviceArg) -> Result<DeviceArg> {
    let id = d.id.trim().to_string();
    if id.is_empty() || id.len() > 128 || id.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::invalid("device.id: this computer's id is required"));
    }
    let name = d
        .name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>();
    if let Some(pk) = &d.public_key
        && cua_host::device::device_id_of(pk).as_deref() != Some(id.as_str())
    {
        return Err(Error::invalid("device.public_key: not this device's key"));
    }
    Ok(DeviceArg {
        id,
        name,
        public_key: d.public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_look_right_and_normalize() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), 9);
            assert_eq!(&c[4..5], "-");
            assert!(
                c.chars()
                    .filter(|c| *c != '-')
                    .all(|c| CODE_ALPHABET.contains(&(c as u8)))
            );
        }
        assert_eq!(normalize_code(" 7kq2-9fxa "), "7KQ29FXA");
        assert_eq!(normalize_code("https://cua.ai/join/7KQ2-9FXA"), "7KQ29FXA");
        assert_eq!(normalize_code("7KQ2 9FXA"), "7KQ29FXA");
        assert_ne!(new_code(), new_code());
    }

    #[test]
    fn shown_states() {
        let c = JoinCode {
            id: "jc".into(),
            space: "s".into(),
            sha256: String::new(),
            created_ms: 0,
            expires_ms: 100,
            created_by: Person {
                id: "a".into(),
                name: "A".into(),
            },
            can_task: "owner".into(),
            ask_before_join: false,
            state: CodeState::Open,
            member_id: None,
            ended_ms: None,
        };
        assert_eq!(c.shown_state(99), "open");
        assert_eq!(c.shown_state(100), "expired");
        assert_eq!(possessive("Ada"), "Ada's");
        assert_eq!(possessive("Linus"), "Linus'");
    }
}
