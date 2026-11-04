// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A model-checked spec of how the pool's keys follow the account's devices
//! (`pool::keys`, `pool::seal`, the relay's `/v1/pool/keys` and its checks
//! on every append), checked exhaustively with Stateright.
//!
//! The fencing protocol (`pool_model.rs`) is unchanged by sealing: a
//! refused append (`rekey`) is, to it, an append that did not reach the log
//! and is sent again, which that model already covers (lost requests). What
//! is new is a second, independent protocol: who holds which key
//! generation, and what the relay lets through. This model checks it.
//!
//! | Model action | Real code |
//! |---|---|
//! | `Register` | a device certifies its pool key (`PoolKeys::member`, relay `POST /v1/pool/member`) |
//! | `Revoke` | the device is revoked (`DELETE /v1/devices/{id}`): it is no member any more and can make no relay call |
//! | `Fetch` | `PoolKeys::sync` reading the keyring and members (`GET /v1/pool/keys`) and opening its wrap |
//! | `First` / `Add` / `Rotate` | `PoolKeys::sync` writing a keyring from what it last read (a possibly stale view), named by the version it read (`PUT /v1/pool/keys`): the first generation; wraps for members that have none; the next generation when a device that holds the keys left |
//! | `Append` | a holder appending an entry sealed under the newest generation it has (`SealedLog::append`), which may be stale; the relay's key gate (`key_gate`: newest generation, keyring not stale) |
//!
//! Threat: a revoked device that colludes with the relay's storage. It
//! knows every key generation ever wrapped to it, and every one it held.
//!
//! Properties: nothing appended after a device was revoked is sealed under
//! a generation it knows; no generation made after the revocation is
//! wrapped to it; every entry is sealed under a generation the current
//! keyring's holders have; and once nothing more happens, every member has
//! the newest keys and every append landed. Each relay rule, turned off,
//! yields a counterexample (the `mutation_*` tests).

use stateright::{Checker, Model, Property};

const DEVICES: usize = 3;
/// Device 1 is revoked at some point; device 2 joins at some point.
const LEAVES: usize = 1;
const JOINS: usize = 2;
const MAX_GEN: u8 = 3;
const MAX_LOG: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Mutations {
    /// The relay stores entries sealed under any generation.
    skip_gate: bool,
    /// The relay stores a keyring wrapped to devices that are no members.
    skip_member_check: bool,
    /// Devices never make the next generation.
    no_rotation: bool,
}

#[derive(Clone, Debug)]
struct Keys {
    mutations: Mutations,
    only: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Ring {
    generation: u8,
    /// Devices it is wrapped to (bits).
    to: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct View {
    ring: Option<Ring>,
    version: u8,
    members: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct State {
    ring: Option<Ring>,
    version: u8,
    registered: u8,
    revoked: u8,
    /// The generation of each entry.
    log: Vec<u8>,
    /// The log's length and the newest generation when the device left.
    revoked_at: Option<(usize, u8)>,
    /// The newest generation ever wrapped to each device.
    wrapped: [u8; DEVICES],
    /// The newest generation each device holds, and its last view.
    has: [u8; DEVICES],
    view: [Option<View>; DEVICES],
    refused: bool,
}

impl State {
    fn members(&self) -> u8 {
        self.registered & !self.revoked
    }

    fn snapshot(&self) -> View {
        View {
            ring: self.ring,
            version: self.version,
            members: self.members(),
        }
    }

    /// What the revoked device can read: every generation up to the
    /// newest it held or was ever wrapped.
    fn leaver_knows(&self) -> u8 {
        self.has[LEAVES].max(self.wrapped[LEAVES])
    }
}

fn bit(d: usize) -> u8 {
    1 << d
}

fn stale(r: &Ring, members: u8) -> bool {
    r.to & !members != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Action {
    Register(usize),
    Revoke(usize),
    Fetch(usize),
    First(usize),
    Add(usize),
    Rotate(usize),
    Append(usize),
}

impl Keys {
    /// The relay's rules for a keyring update from device `d`.
    fn put(&self, s: &mut State, d: usize, version: u8, new: Ring) -> bool {
        let members = s.members();
        if version != s.version || members & bit(d) == 0 {
            return false;
        }
        if !self.mutations.skip_member_check && new.to & !members != 0 {
            return false;
        }
        let ok = match s.ring {
            None => new.generation == 1,
            Some(old) if new.generation == old.generation => {
                old.to & !new.to == 0 && !stale(&old, members)
            }
            Some(old) => new.generation == old.generation + 1,
        };
        if !ok {
            return false;
        }
        s.ring = Some(new);
        s.version += 1;
        for (i, w) in s.wrapped.iter_mut().enumerate() {
            if new.to & bit(i) != 0 {
                *w = (*w).max(new.generation);
            }
        }
        true
    }
}

impl Model for Keys {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<State> {
        vec![State {
            ring: None,
            version: 0,
            registered: bit(0) | bit(LEAVES),
            revoked: 0,
            log: Vec::new(),
            revoked_at: None,
            wrapped: [0; DEVICES],
            has: [0; DEVICES],
            view: [None; DEVICES],
            refused: false,
        }]
    }

    fn actions(&self, s: &State, out: &mut Vec<Action>) {
        if s.registered & bit(JOINS) == 0 {
            out.push(Action::Register(JOINS));
        }
        if s.revoked == 0 {
            out.push(Action::Revoke(LEAVES));
        }
        for d in 0..DEVICES {
            if s.members() & bit(d) == 0 {
                continue;
            }
            let now = s.snapshot();
            let gets = now
                .ring
                .is_some_and(|r| r.to & bit(d) != 0 && s.has[d] < r.generation);
            if s.view[d] != Some(now) || gets {
                out.push(Action::Fetch(d));
            }
            let Some(v) = s.view[d] else { continue };
            match v.ring {
                None => out.push(Action::First(d)),
                Some(r) if s.has[d] == r.generation => {
                    if stale(&r, v.members) {
                        if !self.mutations.no_rotation && r.generation < MAX_GEN {
                            out.push(Action::Rotate(d));
                        }
                    } else if v.members & !r.to != 0 {
                        out.push(Action::Add(d));
                    }
                }
                Some(_) => {}
            }
            if s.has[d] > 0 && s.log.len() < MAX_LOG {
                out.push(Action::Append(d));
            }
        }
    }

    fn next_state(&self, s: &State, a: Action) -> Option<State> {
        let mut s = s.clone();
        match a {
            Action::Register(d) => s.registered |= bit(d),
            Action::Revoke(d) => {
                s.revoked |= bit(d);
                s.revoked_at = Some((s.log.len(), s.ring.map_or(0, |r| r.generation)));
            }
            Action::Fetch(d) => {
                let now = s.snapshot();
                s.view[d] = Some(now);
                if let Some(r) = now.ring
                    && r.to & bit(d) != 0
                {
                    s.has[d] = s.has[d].max(r.generation);
                }
            }
            Action::First(d) => {
                let v = s.view[d]?;
                let ring = Ring {
                    generation: 1,
                    to: v.members,
                };
                if !self.put(&mut s, d, v.version, ring) {
                    return None;
                }
                s.has[d] = 1;
            }
            Action::Add(d) => {
                let v = s.view[d]?;
                let r = v.ring?;
                let ring = Ring {
                    generation: r.generation,
                    to: r.to | v.members,
                };
                if !self.put(&mut s, d, v.version, ring) {
                    return None;
                }
            }
            Action::Rotate(d) => {
                let v = s.view[d]?;
                let r = v.ring?;
                let ring = Ring {
                    generation: r.generation + 1,
                    to: v.members,
                };
                if !self.put(&mut s, d, v.version, ring) {
                    return None;
                }
                s.has[d] = ring.generation;
            }
            Action::Append(d) => {
                let g = s.has[d];
                let members = s.members();
                let ok = self.mutations.skip_gate
                    || s.ring
                        .is_some_and(|r| r.generation == g && !stale(&r, members));
                if !ok {
                    // Refused (`rekey`): nothing changes but what the device
                    // learns, which `Fetch` models.
                    if s.refused {
                        return None;
                    }
                    s.refused = true;
                    return Some(s);
                }
                s.log.push(g);
            }
        }
        Some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        let all = vec![
            Property::always(
                "a revoked device reads nothing written after it left",
                |_, s: &State| {
                    s.revoked_at
                        .is_none_or(|(at, _)| s.log[at..].iter().all(|g| *g > s.leaver_knows()))
                },
            ),
            Property::always(
                "no keys made after a device left reach it",
                |_, s: &State| s.revoked_at.is_none_or(|(_, g)| s.wrapped[LEAVES] <= g),
            ),
            Property::always("keys go only to the account's devices", |_, s: &State| {
                s.ring.is_none_or(|r| r.to & !s.registered == 0)
            }),
            Property::always(
                "every entry is readable by the keys' holders",
                |_, s: &State| {
                    s.log
                        .iter()
                        .all(|g| s.ring.is_some_and(|r| *g >= 1 && *g <= r.generation))
                },
            ),
            Property::eventually("every member has the newest keys", |_, s: &State| {
                s.ring.is_some_and(|r| {
                    r.to == s.members()
                        && (0..DEVICES)
                            .filter(|d| s.members() & bit(*d) != 0)
                            .all(|d| s.has[d] == r.generation)
                })
            }),
            Property::eventually("every append lands", |_, s: &State| s.log.len() == MAX_LOG),
            Property::sometimes("new keys after a device left", |_, s: &State| {
                s.revoked_at
                    .is_some_and(|(_, g)| s.ring.is_some_and(|r| r.generation > g && g > 0))
                    && s.log.len() == MAX_LOG
            }),
            Property::sometimes("a stale writer refused", |_, s: &State| s.refused),
            Property::sometimes("a device that joined holds the keys", |_, s: &State| {
                s.has[JOINS] > 0 && s.revoked != 0
            }),
        ];
        match self.only {
            Some(name) => all.into_iter().filter(|p| p.name == name).collect(),
            None => all,
        }
    }
}

fn run(model: Keys) -> impl Checker<Keys> {
    let checker = model.checker().threads(2).spawn_dfs().join();
    eprintln!(
        "keys model: {} unique states ({} generated), max depth {}",
        checker.unique_state_count(),
        checker.state_count(),
        checker.max_depth()
    );
    checker
}

fn trace(checker: &impl Checker<Keys>, name: &'static str) -> String {
    checker
        .assert_any_discovery(name)
        .into_actions()
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// The protocol as built: every property holds over the whole space.
#[test]
fn pool_keys_follow_the_devices() {
    let checker = run(Keys {
        mutations: Mutations::default(),
        only: None,
    });
    for name in [
        "new keys after a device left",
        "a stale writer refused",
        "a device that joined holds the keys",
    ] {
        eprintln!("example of {name:?}: {}", trace(&checker, name));
    }
    checker.assert_properties();
}

fn find(mutations: Mutations, only: &'static str) {
    let checker = run(Keys {
        mutations,
        only: Some(only),
    });
    eprintln!("counterexample to {only:?}: {}", trace(&checker, only));
}

/// Without the relay's key gate, a holder that has not seen the new keys
/// yet seals under the ones the revoked device has.
#[test]
fn mutation_no_key_gate_lets_a_revoked_device_read() {
    find(
        Mutations {
            skip_gate: true,
            ..Mutations::default()
        },
        "a revoked device reads nothing written after it left",
    );
}

/// Without the relay's membership check, a device whose view predates the
/// revocation wraps the next generation to the revoked device too.
#[test]
fn mutation_no_member_check_wraps_new_keys_to_a_revoked_device() {
    find(
        Mutations {
            skip_member_check: true,
            ..Mutations::default()
        },
        "no keys made after a device left reach it",
    );
}

/// Without a new generation after a device leaves, the gate refuses every
/// append from then on: the pool stops.
#[test]
fn mutation_no_rotation_stops_the_pool() {
    find(
        Mutations {
            no_rotation: true,
            ..Mutations::default()
        },
        "every append lands",
    );
}
