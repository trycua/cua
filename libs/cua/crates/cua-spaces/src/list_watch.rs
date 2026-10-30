// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push for the Space list ([`Spaces::watch_list`]): a generation that moves
//! whenever what [`Spaces::list`] returns changes, so an app reads the list
//! again when something happened instead of on a timer.
//!
//! - This machine's Spaces (the registry, their power states) are compared
//!   every [`LOCAL_TICK`]: a local read.
//! - The account's other computers: the mesh coordinator pushes a new
//!   network map the moment one of them (or one of their Spaces) connects or
//!   goes away. Each map is followed, after [`MESH_SETTLE`], by one read of
//!   the relay directory (which has every computer's state), at most one per
//!   [`MESH_SETTLE`]. Computers that are not on the mesh are read again every
//!   [`REMOTE_FALLBACK`], the interval the apps polled at before.
//!
//! One watcher runs per [`Spaces`] however many subscribe, and stops with the
//! last subscriber.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::Spaces;

/// How often this machine's Spaces are compared.
pub const LOCAL_TICK: Duration = Duration::from_secs(1);
/// After a network map, how long to wait for the next one before reading
/// the directory (a computer coming up moves several of its Spaces at once).
pub const MESH_SETTLE: Duration = Duration::from_millis(200);
/// How often the directory is read without a push.
pub const REMOTE_FALLBACK: Duration = Duration::from_secs(10);

/// One subscription to [`Spaces::watch_list`].
pub struct ListWatch {
    rx: watch::Receiver<u64>,
    _watcher: Arc<Watcher>,
}

impl ListWatch {
    /// The generation now.
    pub fn generation(&self) -> u64 {
        *self.rx.borrow()
    }

    /// Waits for the list to change; the new generation (`None` once the
    /// watcher is gone).
    pub async fn changed(&mut self) -> Option<u64> {
        self.rx.changed().await.ok()?;
        Some(*self.rx.borrow_and_update())
    }
}

pub(crate) struct Watcher {
    tx: watch::Sender<u64>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(t) = self.task.lock().expect("watcher").take() {
            t.abort();
        }
    }
}

/// The running watcher of a [`Spaces`], while it has subscribers.
#[derive(Default)]
pub(crate) struct ListWatchSlot(Mutex<Weak<Watcher>>);

impl Spaces {
    /// Subscribes to changes of [`Spaces::list`]'s answer (see the module
    /// docs): this machine's Spaces within [`LOCAL_TICK`], the account's
    /// computers on the mesh within about [`MESH_SETTLE`] plus a directory
    /// read, the others within [`REMOTE_FALLBACK`].
    pub fn watch_list(&self) -> ListWatch {
        let mut slot = self.inner.list_watch.0.lock().expect("list watch");
        if let Some(w) = slot.upgrade() {
            return ListWatch {
                rx: w.tx.subscribe(),
                _watcher: w,
            };
        }
        let (tx, rx) = watch::channel(0u64);
        let w = Arc::new(Watcher {
            tx,
            task: Mutex::new(None),
        });
        let task = tokio::spawn(watch_loop(self.clone(), Arc::downgrade(&w)));
        *w.task.lock().expect("watcher") = Some(task);
        *slot = Arc::downgrade(&w);
        ListWatch { rx, _watcher: w }
    }
}

/// What the list says, to compare cheaply.
fn fingerprint(spaces: &Spaces) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match spaces.list() {
        Ok(rows) => format!("{rows:?}").hash(&mut h),
        Err(e) => e.to_string().hash(&mut h),
    }
    h.finish()
}

async fn watch_loop(spaces: Spaces, watcher: Weak<Watcher>) {
    let mut last = fingerprint(&spaces);
    let mut read_at = Instant::now();
    let mut pushed_at: Option<Instant> = None;
    loop {
        // The mesh nodes may come and go (the first relay Space starts
        // one): take their receivers afresh each round.
        let mut maps = spaces.inner.mesh.peer_changes().await;
        for m in &mut maps {
            m.borrow_and_update();
        }
        let pushed = async {
            if maps.is_empty() {
                return std::future::pending::<()>().await;
            }
            let changes = maps.iter_mut().map(|m| Box::pin(m.changed()));
            let _ = futures_util::future::select_all(changes).await;
        };
        let mut read_remote = false;
        tokio::select! {
            _ = tokio::time::sleep(LOCAL_TICK) => {}
            _ = pushed => {
                // Several maps in a row (a computer and its Spaces): one read.
                tokio::time::sleep(MESH_SETTLE).await;
                if pushed_at.is_none_or(|t| t.elapsed() >= MESH_SETTLE) {
                    read_remote = true;
                    pushed_at = Some(Instant::now());
                }
            }
        }
        if spaces.relay_account().is_some() && (read_remote || read_at.elapsed() >= REMOTE_FALLBACK)
        {
            read_at = Instant::now();
            if let Err(e) = spaces.relay_machines().await {
                tracing::debug!(error = %e, "list watch: directory not read");
            }
        }
        let now = fingerprint(&spaces);
        if now != last {
            last = now;
            let Some(w) = watcher.upgrade() else { return };
            w.tx.send_modify(|g| *g += 1);
        } else if watcher.strong_count() == 0 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{RelayAccount, StaticToken};
    use cua_host::testing::FakeRelay;

    /// A change on another computer reaches a subscriber within this.
    const PUSH_BUDGET: Duration = Duration::from_secs(2);

    async fn changed_within(w: &mut ListWatch, within: Duration) -> Option<Duration> {
        let t = Instant::now();
        tokio::time::timeout(within, w.changed())
            .await
            .ok()
            .flatten()
            .map(|_| t.elapsed())
    }

    /// A Space added on this machine is announced within a tick.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_local_change_is_pushed() {
        let home = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(home.path()).build();
        let mut w = spaces.watch_list();
        let env =
            cua_spacesd_client::testing::MockServer::start(cua_spacesd_client::testing::MockAuth {
                token: None,
                gateway: None,
                prefix: None,
            })
            .await;
        spaces
            .add(&env.url(), None, Some("new".into()))
            .await
            .unwrap();
        let took = changed_within(&mut w, LOCAL_TICK * 3)
            .await
            .expect("the new Space was announced");
        assert!(took <= LOCAL_TICK + Duration::from_millis(500), "{took:?}");
        // Nothing else moved: no further announcement.
        assert!(changed_within(&mut w, LOCAL_TICK * 2).await.is_none());
    }

    /// A computer that comes up (its node joins the mesh, the relay lists
    /// it online) is announced within the push budget, long before the
    /// fallback read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_computer_coming_up_is_pushed_by_the_mesh() {
        let relay = FakeRelay::start().await;
        relay.add_account("tok", "ada", None);
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        client
            .register(
                "tok",
                &cua_host::relay::RegisterRequest {
                    id: "aaaa000000000001".into(),
                    name: "far".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        relay.set_online("aaaa000000000001", false, "1.0.0");
        let home = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder()
            .home(home.path())
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(StaticToken("tok".into())),
            ))
            .build();
        spaces.list_all().await.unwrap();
        // The account's mesh node, as the first relay Space starts it.
        let coord = cua_mesh::testing::FakeCoordinator::start(cua_mesh::Mode::Account).await;
        let dialer = cua_mesh::dialer::Dialer::start(
            &coord.url,
            cua_mesh::static_auth(vec![]),
            None,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        spaces.inner.mesh.insert_for_test(&relay.url, dialer).await;
        let mut w = spaces.watch_list();
        // Let the watcher take the node's map receiver and settle.
        tokio::time::sleep(LOCAL_TICK * 2).await;
        while changed_within(&mut w, Duration::from_millis(100))
            .await
            .is_some()
        {}

        // The computer comes up: the relay lists it online, and its node
        // joins the mesh (the coordinator pushes a new map).
        let t = Instant::now();
        relay.set_online("aaaa000000000001", true, "1.0.0");
        let mut cfg = cua_mesh::MeshConfig::new(
            coord.url.clone(),
            cua_mesh::NodeKind::Machine,
            None,
            cua_mesh::static_auth(vec![("x-cua-machine-id".into(), "aaaa000000000001".into())]),
        );
        cfg.media_udp_port = 0;
        let machine = cua_mesh::Mesh::start(cfg).await.unwrap();
        let _ = changed_within(&mut w, PUSH_BUDGET + Duration::from_secs(1))
            .await
            .expect("the computer coming up was announced");
        let took = t.elapsed();
        assert!(took <= PUSH_BUDGET, "announced after {took:?}");
        let row = spaces
            .list()
            .unwrap()
            .into_iter()
            .find(|i| i.id == "relay:aaaa000000000001")
            .unwrap();
        assert_ne!(row.power_state, "offline", "{row:?}");

        // ... and going away again.
        let t = Instant::now();
        relay.set_online("aaaa000000000001", false, "1.0.0");
        machine.shutdown();
        changed_within(&mut w, PUSH_BUDGET + Duration::from_secs(1))
            .await
            .expect("the computer going away was announced");
        assert!(
            t.elapsed() <= PUSH_BUDGET,
            "announced after {:?}",
            t.elapsed()
        );
    }
}
