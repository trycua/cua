// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Spaces on the account's other machines, with what each is doing.
//!
//! The relay directory lists every Space a machine provides as its own
//! relay machine, under that machine (`host`), and says only whether its
//! driver is connected now (`online`). A Space being created, booting, off
//! or failed is "not connected" alike, so on its own the directory cannot
//! tell Starting from Stopped. The machine that runs the Space can:
//! [`Spaces::list_all`] asks each of the account's machines that provides
//! Spaces for its listing (`HostSpacesService.GetHostSpaces`, through the
//! relay, at most every [`LISTING_MAX_AGE`]), and [`Spaces::list`] gives each
//! of its Spaces the state that machine reports ([`apply_host_state`]):
//! `power_state` `running`, `starting`, `suspended`, `stopped` or `failed`,
//! with [`crate::SpaceInfo::state_reason`]. A machine that is offline, or
//! predates the states, leaves the directory's word.
//!
//! Every client reads the list the same way (the daemon's `ListSpaces`, the
//! SDK, the apps' rows, `cua sandbox ls`), so a Space reads the same on
//! every device. Nothing here needs more of the relay than it has.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cua_proto::env::v1 as pb;

use crate::relay::RelayMachine;
use crate::{SpaceInfo, Spaces};

/// How long a machine's listing is reused before it is asked again.
pub const LISTING_MAX_AGE: Duration = Duration::from_secs(5);

/// How long a listing still counts once it was read; older, its states are
/// not shown (the machine stopped answering).
const LISTING_STALE: Duration = Duration::from_secs(60);

/// How long a machine that did not answer is left alone before it is
/// asked again.
const FAILED_RETRY: Duration = Duration::from_secs(30);

/// How long one machine has to answer.
const LISTING_TIMEOUT: Duration = Duration::from_secs(4);

/// The last listing of each machine that provides Spaces.
#[derive(Default)]
pub(crate) struct HostListings {
    entries: std::sync::Mutex<HashMap<String, Listing>>,
    /// Held while the listings are read again (one read at a time).
    refreshing: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct Listing {
    read_at: Instant,
    /// `None`: the machine did not answer.
    answer: Option<pb::GetHostSpacesResponse>,
}

/// Whether `m` is one of the account's machines whose Spaces it lists:
/// a machine (not a Space) that is online, shares, may be used (not
/// view-only), and provides Spaces (it says so, or it has some listed).
pub(crate) fn lists_spaces(m: &RelayMachine, all: &[RelayMachine]) -> bool {
    m.host.as_deref().is_none_or(str::is_empty)
        && m.online
        && m.sharing
        && m.role != "viewer"
        && (m
            .meta
            .get(cua_host::META_PROVIDES_SPACES)
            .map(String::as_str)
            == Some("on")
            || all.iter().any(|s| s.host.as_deref() == Some(m.id.as_str())))
}

impl Spaces {
    /// Reads the listing of every machine of the account that provides
    /// Spaces and whose last listing is older than [`LISTING_MAX_AGE`],
    /// from the directory [`Spaces::relay_machines`] last read. Bounded:
    /// each machine has [`LISTING_TIMEOUT`], and one that did not answer is
    /// not asked again for [`FAILED_RETRY`]. A read already running is not
    /// waited for.
    pub(crate) async fn refresh_host_listings(&self) {
        let Ok(_one) = self.inner.host_listings.refreshing.try_lock() else {
            return;
        };
        let directory = self.inner.relay_cache.lock().expect("relay cache").clone();
        let this = self.this_relay_machine_id().unwrap_or_default();
        let hosts: Vec<RelayMachine> = directory
            .iter()
            .filter(|m| m.id != this && lists_spaces(m, &directory))
            .cloned()
            .collect();
        let due: Vec<RelayMachine> = {
            let mut entries = self.inner.host_listings.entries.lock().expect("listings");
            // Machines no longer listed (or offline) are forgotten.
            entries.retain(|id, _| hosts.iter().any(|h| &h.id == id));
            hosts
                .into_iter()
                .filter(|h| match entries.get(&h.id) {
                    None => true,
                    Some(l) if l.answer.is_some() => l.read_at.elapsed() >= LISTING_MAX_AGE,
                    Some(l) => l.read_at.elapsed() >= FAILED_RETRY,
                })
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let reads = due.iter().map(|h| async move {
            let answer = tokio::time::timeout(LISTING_TIMEOUT, self.read_host_listing(h)).await;
            let answer = match answer {
                Ok(Ok(a)) => Some(a),
                Ok(Err(e)) => {
                    tracing::debug!(host = h.id, "machine listing: {e}");
                    None
                }
                Err(_) => {
                    tracing::debug!(host = h.id, "machine listing: no answer");
                    None
                }
            };
            (h.id.clone(), answer)
        });
        let read = futures_util::future::join_all(reads).await;
        let mut entries = self.inner.host_listings.entries.lock().expect("listings");
        for (id, answer) in read {
            entries.insert(
                id,
                Listing {
                    read_at: Instant::now(),
                    answer,
                },
            );
        }
    }

    async fn read_host_listing(
        &self,
        host: &RelayMachine,
    ) -> crate::Result<pb::GetHostSpacesResponse> {
        let space = self.host_space(host).await?;
        space
            .spacesd()?
            .host_spaces()
            .get_host_spaces(pb::GetHostSpacesRequest {})
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| crate::host_spaces::from_host(cua_spacesd_client::Error::from(e)))
    }

    /// The last listing the machine `host` answered, when it is recent
    /// enough to show ([`LISTING_STALE`]).
    pub(crate) fn host_listing(&self, host: &str) -> Option<pb::GetHostSpacesResponse> {
        let entries = self.inner.host_listings.entries.lock().expect("listings");
        let l = entries.get(host)?;
        if l.read_at.elapsed() >= LISTING_STALE {
            return None;
        }
        l.answer.clone()
    }

    /// Forgets the listing of `host`, so the next list asks it again (after
    /// a create, a power change or a delete there).
    pub(crate) fn forget_host_listing(&self, host: &str) {
        self.inner
            .host_listings
            .entries
            .lock()
            .expect("listings")
            .remove(host);
    }
}

/// Gives `info`, the row of the relay machine `m` (a Space the machine
/// `host` provides), what that machine says the Space is doing:
///
/// - its driver is connected: `running`;
/// - the machine lists it: its state (`starting`, `stopped`, `suspended`,
///   `failed` with why; `running` while its driver is not connected yet
///   reads `starting`);
/// - the machine is offline: its state stays unknown, and
///   [`SpaceInfo::state_reason`] says the machine is offline.
///
/// A machine that predates the states lists no state: the row is left as
/// the directory says.
pub(crate) fn apply_host_state(
    info: &mut SpaceInfo,
    m: &RelayMachine,
    host: Option<&RelayMachine>,
    listing: Option<&pb::GetHostSpacesResponse>,
) {
    if m.online {
        if m.sharing {
            info.power_state = "running".into();
        }
        return;
    }
    let host_name = || {
        host.map(|h| {
            if h.name.is_empty() {
                h.id.clone()
            } else {
                h.name.clone()
            }
        })
        .unwrap_or_else(|| info.host.clone())
    };
    match host {
        None => {
            info.state_reason = format!("{} is no longer on the relay", host_name());
            return;
        }
        Some(h) if !h.online => {
            info.state_reason = format!("{} is offline", host_name());
            return;
        }
        Some(_) => {}
    }
    let Some(listed) = listing.and_then(|l| l.spaces.iter().find(|s| s.relay_machine == m.id))
    else {
        // A create the machine is still running for it (CUA-1635's
        // listing, keyed by the Space's relay machine): starting, with the
        // step it is on.
        if let Some(c) = listing.and_then(|l| l.creating.iter().find(|c| c.key == m.id)) {
            info.power_state = "starting".into();
            info.state_reason = creating_step(c);
        }
        return;
    };
    match listed.state.as_str() {
        "running" | "starting" => {
            info.power_state = "starting".into();
        }
        "stopped" | "suspended" => {
            info.power_state = listed.state.clone();
        }
        "failed" => {
            info.power_state = "failed".into();
            info.state_reason = listed.state_reason.clone();
        }
        _ => {}
    }
}

/// The step a create on another machine is on, in words ("Downloading
/// the image, 41%").
fn creating_step(c: &pb::HostSpaceCreating) -> String {
    let step = match c.phase.as_str() {
        "pulling" => "Downloading the image",
        "creating" => "Preparing the disk",
        "booting" => "Booting",
        "waiting_for_services" => "Starting Cua inside the Space",
        "connecting" => "Connecting",
        _ => "Starting",
    };
    let fraction = match (c.bytes_done, c.bytes_total) {
        (Some(done), Some(total)) if total > 0 => Some(done as f64 / total as f64),
        _ => c.fraction,
    };
    match fraction {
        Some(f) if (0.0..=1.0).contains(&f) => format!("{step}, {}%", (f * 100.0).floor()),
        _ => step.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(id: &str, host: Option<&str>, online: bool) -> RelayMachine {
        RelayMachine {
            id: id.into(),
            name: id.into(),
            online,
            sharing: true,
            role: "owner".into(),
            host: host.map(Into::into),
            ..Default::default()
        }
    }

    fn listing(machine: &str, state: &str, reason: &str) -> pb::GetHostSpacesResponse {
        pb::GetHostSpacesResponse {
            spaces: vec![pb::HostSpace {
                relay_machine: machine.into(),
                local_space: "local:s".into(),
                state: state.into(),
                state_reason: reason.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn state(
        m: &RelayMachine,
        host: Option<&RelayMachine>,
        l: Option<&pb::GetHostSpacesResponse>,
    ) -> (String, String) {
        let mut info = crate::spaces::relay_info(m);
        apply_host_state(&mut info, m, host, l);
        (info.power_state, info.state_reason)
    }

    #[test]
    fn a_space_reads_what_its_machine_says() {
        let host = machine("mini-0001", None, true);
        let off = machine("space-0000000000000001", Some("mini-0001"), false);
        let on = machine("space-0000000000000001", Some("mini-0001"), true);
        let s = |st: &str, why: &str| (st.to_string(), why.to_string());

        // Connected: running, whatever an older listing said.
        let l = listing(&on.id, "stopped", "");
        assert_eq!(state(&on, Some(&host), Some(&l)), s("running", ""));
        // Being created or booting: starting, not "Stopped".
        let l = listing(&off.id, "starting", "");
        assert_eq!(state(&off, Some(&host), Some(&l)), s("starting", ""));
        // Up, its driver not connected yet.
        let l = listing(&off.id, "running", "");
        assert_eq!(state(&off, Some(&host), Some(&l)), s("starting", ""));
        // Off, as the machine says.
        for word in ["stopped", "suspended"] {
            let l = listing(&off.id, word, "");
            assert_eq!(state(&off, Some(&host), Some(&l)), s(word, ""));
        }
        // Failed, with why.
        let l = listing(&off.id, "failed", "its sandbox is gone");
        assert_eq!(
            state(&off, Some(&host), Some(&l)),
            s("failed", "its sandbox is gone")
        );
        // Still being created there (CUA-1635's listing): starting, with
        // the step it is on.
        let mut l = listing("space-other", "running", "");
        l.creating.push(pb::HostSpaceCreating {
            key: off.id.clone(),
            phase: "pulling".into(),
            bytes_done: Some(41),
            bytes_total: Some(100),
            ..Default::default()
        });
        assert_eq!(
            state(&off, Some(&host), Some(&l)),
            s("starting", "Downloading the image, 41%")
        );
        // A machine that predates the states, or does not list it: as the
        // directory says.
        let l = listing(&off.id, "", "");
        assert_eq!(state(&off, Some(&host), Some(&l)), s("", ""));
        assert_eq!(state(&off, Some(&host), None), s("", ""));
        // The machine is offline (or gone): why it cannot be reached.
        let down = machine("mini-0001", None, false);
        assert_eq!(
            state(&off, Some(&down), None),
            s("", "mini-0001 is offline")
        );
        assert_eq!(
            state(&off, None, None),
            s("", "mini-0001 is no longer on the relay")
        );
    }

    #[test]
    fn only_machines_that_provide_spaces_are_asked() {
        let mut provides = machine("mini-0001", None, true);
        provides
            .meta
            .insert(cua_host::META_PROVIDES_SPACES.into(), "on".into());
        let desktop = machine("laptop-0001", None, true);
        let hosting = machine("studio-0001", None, true);
        let space = machine("space-0000000000000001", Some("studio-0001"), true);
        let mut viewer = provides.clone();
        viewer.role = "viewer".into();
        let mut offline = provides.clone();
        offline.online = false;
        let all = vec![
            provides.clone(),
            desktop.clone(),
            hosting.clone(),
            space.clone(),
        ];
        assert!(lists_spaces(&provides, &all));
        assert!(lists_spaces(&hosting, &all), "it has Spaces listed");
        assert!(!lists_spaces(&desktop, &all));
        assert!(!lists_spaces(&space, &all), "a Space is not a machine");
        assert!(!lists_spaces(&viewer, &all));
        assert!(!lists_spaces(&offline, &all));
    }
}
