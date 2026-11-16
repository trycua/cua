// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Each account's default Space (account mode), kept here so the apps, CLI,
//! phone and agents share one answer:
//!
//! - `GET /v1/spaces/settings`: `{default_space, fallback, fallback_on,
//!   updated_at}` (defaults for an account that never set any).
//! - `PUT /v1/spaces/settings`: replaces the whole object. 400 for a bad id,
//!   a duplicate in `fallback` or more than [`MAX_FALLBACK`] entries; 403 for
//!   a computer the caller cannot see.
//! - `GET /v1/spaces/default?here=<machine-id>`: where a client should work
//!   now: the default if it is online, else the fallback list (or, when it
//!   is empty, every online computer the account can use, oldest first),
//!   else `here` (the caller's own computer).
//!
//! A Space id is `relay:<machine-id>` (a computer, or a Space with its own
//! record) or `relay:<machine-id>/<name>`. Ids that are no longer visible,
//! or whose computer was removed (detached, see [`crate::api`]), are skipped
//! but stay stored: they count again when they come back.
//!
//! Every call carries the account token (a computer token is refused) from
//! an enrolled device. The settings persist per account in
//! `<state>.spaces.json` (atomic replace, private file mode) and are erased
//! with the account ([`crate::account_erase`]).

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::{account_caller, error, visible_machines, LIVE_HEARTBEAT_SECS};
use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::device_api::{flag_grace, Gate};
use crate::directory::{MachineRecord, Role};
use crate::server::{Registrant, Relay};
use crate::storage::{store, Storage};

/// Most entries in `fallback`.
pub const MAX_FALLBACK: usize = 16;
/// Request bodies here are tiny.
const BODY_LIMIT: usize = 8 * 1024;

/// An account's default Space settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The Space to work in; `null` when unset.
    pub default_space: Option<String>,
    /// Where to go when the default is offline; empty means automatic.
    pub fallback: Vec<String>,
    /// False: only the default, then this device.
    pub fallback_on: bool,
    /// Unix seconds of the last change (0 for settings never set).
    pub updated_at: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_space: None,
            fallback: Vec::new(),
            fallback_on: true,
            updated_at: 0,
        }
    }
}

/// What a client may send to replace the settings; absent fields reset to
/// their defaults.
#[derive(Debug, Deserialize)]
struct Put {
    #[serde(default)]
    default_space: Option<String>,
    #[serde(default)]
    fallback: Vec<String>,
    #[serde(default = "yes")]
    fallback_on: bool,
}

fn yes() -> bool {
    true
}

/// The settings of every account, persisted as JSON (atomic replace) when a
/// file is configured.
#[derive(Debug, Default)]
pub struct SpacesSettings {
    path: Option<PathBuf>,
    accounts: Mutex<BTreeMap<String, Settings>>,
    storage: Arc<Storage>,
}

impl SpacesSettings {
    /// In memory only.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads (or starts) the store persisted at `path`, reporting writes to
    /// `storage`.
    pub fn open_with(path: PathBuf, storage: Arc<Storage>) -> std::io::Result<Self> {
        remove_stale_temps(&path)?;
        let accounts = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path),
            accounts: Mutex::new(accounts),
            storage,
        })
    }

    /// The account's settings (the defaults if it never set any).
    pub fn get(&self, account: &str) -> Settings {
        self.accounts
            .lock()
            .expect("spaces")
            .get(account)
            .cloned()
            .unwrap_or_default()
    }

    /// Writes `next` and swaps it in; on a failed write the settings stay as
    /// they were.
    fn commit(
        &self,
        accounts: &mut BTreeMap<String, Settings>,
        next: BTreeMap<String, Settings>,
    ) -> Result<(), String> {
        if let Some(path) = &self.path {
            let written = (|| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_private_with(path, |out| {
                    serde_json::to_writer(&mut *out, &next).map_err(std::io::Error::other)
                })
            })();
            self.storage.record(store::SPACES, &written);
            written.map_err(|e| e.to_string())?;
        }
        *accounts = next;
        Ok(())
    }

    /// Replaces the account's settings.
    fn set(&self, account: &str, put: Put) -> Result<Settings, String> {
        let mut accounts = self.accounts.lock().expect("spaces");
        let mut next = accounts.clone();
        let settings = Settings {
            default_space: put.default_space,
            fallback: put.fallback,
            fallback_on: put.fallback_on,
            updated_at: now_secs(),
        };
        next.insert(account.to_owned(), settings.clone());
        self.commit(&mut accounts, next)?;
        Ok(settings)
    }

    /// Account deletion: forgets the account's settings. Returns whether
    /// there were any. The file is rewritten every call, so a retry after a
    /// failed write finishes the job.
    pub fn erase_account(&self, account: &str) -> Result<bool, String> {
        let mut accounts = self.accounts.lock().expect("spaces");
        let mut next = accounts.clone();
        let had = next.remove(account).is_some();
        self.commit(&mut accounts, next)?;
        Ok(had)
    }
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/spaces/settings", get(get_settings).put(put_settings))
        .route("/v1/spaces/default", get(resolve))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// `relay:<machine-id>` or `relay:<machine-id>/<name>` as (machine id, name).
pub fn parse_space(id: &str) -> Option<(&str, Option<&str>)> {
    let rest = id.strip_prefix("relay:")?;
    let (machine, name) = match rest.split_once('/') {
        Some((m, n)) => (m, Some(n)),
        None => (rest, None),
    };
    if !crate::valid_machine_id(machine) {
        return None;
    }
    if let Some(n) = name {
        let ok = (1..=63).contains(&n.len())
            && n.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok {
            return None;
        }
    }
    Some((machine, name))
}

/// The account behind the request, from an enrolled device.
async fn account(
    relay: &Relay,
    headers: &HeaderMap,
) -> Result<(crate::oidc::Identity, Result<Gate, String>), Response> {
    let (who, gate) = account_caller(relay, headers).await?;
    if let Err(m) = &gate {
        return Err(error(StatusCode::FORBIDDEN, m.clone()));
    }
    Ok((who, gate))
}

fn finish(relay: &Relay, gate: &Result<Gate, String>, mut response: Response) -> Response {
    if matches!(gate, Ok(Gate::Grace)) {
        flag_grace(relay, response.headers_mut());
    }
    response
}

async fn get_settings(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let (who, gate) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    finish(
        &relay,
        &gate,
        Json(relay.spaces.get(&who.account)).into_response(),
    )
}

async fn put_settings(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(put): Json<Put>,
) -> Response {
    let (who, gate) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if put.fallback.len() > MAX_FALLBACK {
        return error(
            StatusCode::BAD_REQUEST,
            format!("at most {MAX_FALLBACK} fallback Spaces"),
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for id in &put.fallback {
        if !seen.insert(id.as_str()) {
            return error(StatusCode::BAD_REQUEST, format!("duplicate Space id: {id}"));
        }
    }
    for id in put.default_space.iter().chain(put.fallback.iter()) {
        let Some((machine, _)) = parse_space(id) else {
            return error(StatusCode::BAD_REQUEST, format!("invalid Space id: {id}"));
        };
        let usable = relay
            .directory
            .get(machine)
            .is_some_and(|r| relay.machine_role(&r, &who).is_some());
        if !usable {
            return error(
                StatusCode::FORBIDDEN,
                format!("{id} is not a computer you can use"),
            );
        }
    }
    match relay.spaces.set(&who.account, put) {
        Ok(settings) => finish(&relay, &gate, Json(settings).into_response()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// A computer is online when it is connected for its owner with a live
/// heartbeat and is sharing.
fn online(relay: &Relay, record: &MachineRecord) -> bool {
    record.sharing
        && relay
            .machine(&record.id)
            .filter(|m| matches!(&m.registrant, Registrant::Account(o) if o == &record.owner.id))
            .is_some_and(|m| m.heartbeat_age().as_secs() <= LIVE_HEARTBEAT_SECS)
}

#[derive(Serialize)]
struct Entry {
    space: String,
    name: String,
    online: bool,
    role: &'static str,
}

async fn resolve(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (who, gate) = match account(&relay, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let settings = relay.spaces.get(&who.account);
    // What the caller can use now: detached Spaces are out.
    let usable: Vec<(MachineRecord, Role)> = visible_machines(&relay, &who, relay.org.source())
        .await
        .into_iter()
        .filter(|(r, ..)| !relay.directory.is_detached(r))
        .map(|(r, role, _)| (r, role))
        .collect();
    let find = |machine: &str| usable.iter().find(|(r, _)| r.id == machine);
    // An id as an order entry, if it can be used now.
    let entry = |space: &str, role: &'static str| -> Option<Entry> {
        let (machine, child) = parse_space(space)?;
        let (record, _) = find(machine)?;
        Some(Entry {
            space: space.to_owned(),
            name: match child {
                Some(n) => format!("{} / {n}", record.name),
                None => record.name.clone(),
            },
            online: online(&relay, record),
            role,
        })
    };

    let here = query
        .get("here")
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
        .map(|h| format!("relay:{h}"))
        .and_then(|s| entry(&s, "here"));

    let Some(default) = settings.default_space.as_deref() else {
        let space = here.as_ref().map(|e| e.space.clone());
        return finish(
            &relay,
            &gate,
            Json(serde_json::json!({
                "space": space,
                "reason": "unset",
                "default_space": null,
                "default_online": false,
                "order": here.into_iter().collect::<Vec<_>>(),
            }))
            .into_response(),
        );
    };

    let mut order: Vec<Entry> = Vec::new();
    let default_entry = entry(default, "default");
    let default_online = default_entry.as_ref().is_some_and(|e| e.online);
    order.extend(default_entry);
    if settings.fallback_on {
        let here_id = here.as_ref().map(|e| e.space.as_str());
        let skip = |space: &str| space == default || Some(space) == here_id;
        if settings.fallback.is_empty() {
            let mut auto: Vec<&MachineRecord> = usable
                .iter()
                .filter(|(r, role)| r.host.is_none() && *role != Role::Viewer && online(&relay, r))
                .map(|(r, _)| r)
                .filter(|r| !skip(&format!("relay:{}", r.id)))
                .collect();
            auto.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
            order.extend(auto.into_iter().map(|r| Entry {
                space: format!("relay:{}", r.id),
                name: r.name.clone(),
                online: true,
                role: "fallback",
            }));
        } else {
            order.extend(
                settings
                    .fallback
                    .iter()
                    .filter(|s| !skip(s))
                    .filter_map(|s| entry(s, "fallback")),
            );
        }
    }
    let first = order.iter().find(|e| e.online);
    let (space, reason) = match first {
        Some(e) if e.role == "default" => (Some(e.space.clone()), "default"),
        Some(e) => (Some(e.space.clone()), "fallback"),
        None => (here.as_ref().map(|e| e.space.clone()), "here"),
    };
    order.extend(here);
    finish(
        &relay,
        &gate,
        Json(serde_json::json!({
            "space": space,
            "reason": reason,
            "default_space": default,
            "default_online": default_online,
            "order": order,
        }))
        .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_ids() {
        assert_eq!(parse_space("relay:abcd1234"), Some(("abcd1234", None)));
        assert_eq!(
            parse_space("relay:abcd1234/dev-1"),
            Some(("abcd1234", Some("dev-1")))
        );
        for bad in [
            "abcd1234",
            "relay:",
            "relay:short",
            "relay:abcd1234/",
            "relay:abcd1234/UP",
            "relay:abcd1234/a/b",
            "relay:ABCD1234",
            "local:abcd1234",
        ] {
            assert_eq!(parse_space(bad), None, "{bad}");
        }
        assert_eq!(
            parse_space(&format!("relay:abcd1234/{}", "a".repeat(64))),
            None
        );
    }

    #[test]
    fn settings_persist_and_erase() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.spaces.json");
        let store = SpacesSettings::open_with(path.clone(), Default::default()).unwrap();
        assert_eq!(store.get("ada"), Settings::default());
        store
            .set(
                "ada",
                Put {
                    default_space: Some("relay:abcd1234".into()),
                    fallback: vec![],
                    fallback_on: false,
                },
            )
            .unwrap();
        let again = SpacesSettings::open_with(path.clone(), Default::default()).unwrap();
        assert_eq!(
            again.get("ada").default_space.as_deref(),
            Some("relay:abcd1234")
        );
        assert!(!again.get("ada").fallback_on);
        assert!(again.erase_account("ada").unwrap());
        assert!(!again.erase_account("ada").unwrap());
        let last = SpacesSettings::open_with(path, Default::default()).unwrap();
        assert_eq!(last.get("ada"), Settings::default());
    }
}
