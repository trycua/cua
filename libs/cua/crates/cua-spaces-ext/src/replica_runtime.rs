// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Volume across your machines: starts it when it should run, and only
//! then.
//!
//! Nothing here runs unless the user has signed in (this machine's mesh node
//! has a machines list), has another machine on it, and has not pointed the
//! Volume at a bucket ([`cua_volume::replica::migrate::decide`]). With one
//! machine, signed out, or a bucket, the Volume is served exactly as every
//! earlier release served it. When the machines list gains a machine the
//! local store simply starts to be replicated (it is the same directory:
//! nothing is copied or converted first except a backup of its heads); when
//! the list drops to one machine, replication stops and the same directory
//! carries on as a plain local Volume.
//!
//! Peers reach each other through the mesh the account already has: this
//! machine registers a loopback listener as a mesh port
//! ([`cua_mesh::gang::PortRegistry`]), its mesh node connects peers' streams
//! to it, and it asks the same node for a forward to a peer's registered
//! port ([`cua_mesh::gang::ControlClient`]). WireGuard between the two nodes
//! carries every byte; a machine outside the account cannot open the port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cua_volume::config::{BackendKind, DriveConfig};
use cua_volume::replica::link::Stream;
use cua_volume::replica::migrate::{Decision, decide};
use cua_volume::replica::{Config, Facts, Member, ReplicaNode, Transport};

/// The mesh port every machine registers for this service (an arbitrary
/// number in the mesh port registry's namespace).
pub const MESH_PORT: u16 = 47_731;

/// A machine of the account as the mesh node reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRef {
    pub id: String,
    pub name: String,
    pub online: bool,
}

/// What the runtime needs from the mesh.
#[async_trait::async_trait]
pub trait MeshAccess: Send + Sync + 'static {
    /// This account's other machines; `None` when this machine is not signed
    /// in (no mesh node, no control socket).
    async fn machines(&self) -> Option<Vec<MachineRef>>;
    /// A stream to the replica service of `machine`.
    async fn dial(&self, machine: &str) -> std::io::Result<Stream>;
    /// Makes `local` reachable by the account's other machines at
    /// [`MESH_PORT`] until the returned guard is dropped.
    fn register(&self, local: SocketAddr) -> std::io::Result<Box<dyn std::any::Any + Send + Sync>>;
}

/// The real thing: this machine's mesh node through its control socket.
// The control socket is a Unix socket; elsewhere the fields are unused.
#[cfg_attr(not(unix), allow(dead_code))]
pub struct ControlMesh {
    home: PathBuf,
    forwards: tokio::sync::Mutex<std::collections::HashMap<String, SocketAddr>>,
}

impl ControlMesh {
    pub fn new(home: &Path) -> Arc<ControlMesh> {
        Arc::new(ControlMesh {
            home: home.to_path_buf(),
            forwards: Default::default(),
        })
    }
}

#[cfg(unix)]
#[async_trait::async_trait]
impl MeshAccess for ControlMesh {
    async fn machines(&self) -> Option<Vec<MachineRef>> {
        let mut c =
            cua_mesh::gang::ControlClient::connect(&cua_mesh::gang::control_path(&self.home))
                .await
                .ok()?;
        let (_, paths) = c.status().await.ok()?;
        Some(
            paths
                .into_iter()
                .filter(|p| !p.machine.is_empty())
                .map(|p| MachineRef {
                    id: p.machine,
                    name: p.name,
                    online: p.online,
                })
                .collect(),
        )
    }

    async fn dial(&self, machine: &str) -> std::io::Result<Stream> {
        let known = self.forwards.lock().await.get(machine).copied();
        let addr = match known {
            Some(a) => a,
            None => {
                let mut c = cua_mesh::gang::ControlClient::connect(&cua_mesh::gang::control_path(
                    &self.home,
                ))
                .await?;
                let info = c
                    .forward(machine, MESH_PORT)
                    .await
                    .map_err(std::io::Error::other)?;
                self.forwards
                    .lock()
                    .await
                    .insert(machine.to_string(), info.local);
                info.local
            }
        };
        match tokio::net::TcpStream::connect(addr).await {
            Ok(s) => {
                s.set_nodelay(true)?;
                Ok(Box::new(s))
            }
            Err(e) => {
                // The forward may have gone with a mesh restart: ask again next time.
                self.forwards.lock().await.remove(machine);
                Err(e)
            }
        }
    }

    fn register(&self, local: SocketAddr) -> std::io::Result<Box<dyn std::any::Any + Send + Sync>> {
        let reg = cua_mesh::gang::PortRegistry::new(cua_mesh::gang::ports_dir(&self.home))
            .register_to(MESH_PORT, "cua volume replica", Some(local))?;
        Ok(Box::new(reg))
    }
}

#[cfg(not(unix))]
#[async_trait::async_trait]
impl MeshAccess for ControlMesh {
    async fn machines(&self) -> Option<Vec<MachineRef>> {
        None
    }
    async fn dial(&self, _machine: &str) -> std::io::Result<Stream> {
        Err(std::io::Error::other(
            "the replicated Volume needs the mesh control socket (not on this platform yet)",
        ))
    }
    fn register(
        &self,
        _local: SocketAddr,
    ) -> std::io::Result<Box<dyn std::any::Any + Send + Sync>> {
        Err(std::io::Error::other("not on this platform yet"))
    }
}

struct MeshTransport(Arc<dyn MeshAccess>);

#[async_trait::async_trait]
impl Transport for MeshTransport {
    async fn dial(&self, machine: &str) -> std::io::Result<Stream> {
        self.0.dial(machine).await
    }
}

/// What is running.
struct Running {
    node: ReplicaNode,
    _port: Box<dyn std::any::Any + Send + Sync>,
    accept: tokio::task::JoinHandle<()>,
}

/// One machine's replication manager.
pub struct ReplicaRuntime {
    home: PathBuf,
    /// This machine's id on the account; `None` while signed out.
    me: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    name: String,
    mesh: Arc<dyn MeshAccess>,
    drive: cua_volume::Drive,
    cfg: Config,
    running: tokio::sync::Mutex<Option<(String, Running)>>,
    /// Facts about this machine (measured; tests set them).
    facts: Arc<dyn Fn() -> Facts + Send + Sync>,
}

impl ReplicaRuntime {
    pub fn new(
        home: &Path,
        me: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        name: &str,
        mesh: Arc<dyn MeshAccess>,
        drive: cua_volume::Drive,
        cfg: Config,
        facts: Arc<dyn Fn() -> Facts + Send + Sync>,
    ) -> Arc<ReplicaRuntime> {
        Arc::new(ReplicaRuntime {
            home: home.to_path_buf(),
            me,
            name: name.to_string(),
            mesh,
            drive,
            cfg,
            running: Default::default(),
            facts,
        })
    }

    /// This machine's facts as the Volume wants them, measured now.
    pub fn measured_facts(store_free: Option<u64>) -> Facts {
        let f = cua_cluster::reliability::probe();
        Facts {
            rank: cua_cluster::reliability::score(&f),
            free_disk: store_free.unwrap_or(f.free_disk_gb << 30),
            on_battery: f.on_battery,
            sleep_prone: f.sleep_prone(),
            user_here: f.user_active,
            sleep_prevented: !cua_cluster::power::active_reasons().is_empty(),
            arch: cua_volume::replica::roles::normalize_arch(std::env::consts::ARCH),
        }
    }

    fn bucket_configured(&self) -> bool {
        DriveConfig::load(&self.home).is_ok_and(|c| c.backend != BackendKind::Fs)
    }

    /// One look: start, update or stop replication to match the machines list.
    pub async fn tick(&self) {
        // Signed out (no id on the account) or no mesh node: no machines list.
        let me = (self.me)();
        let machines = match &me {
            Some(_) => self.mesh.machines().await,
            None => None,
        };
        let my_id = me.clone().unwrap_or_default();
        let others: Vec<&MachineRef> = machines
            .iter()
            .flatten()
            .filter(|m| m.id != my_id)
            .collect();
        let decision = decide(self.bucket_configured(), machines.is_some(), others.len());
        let mut running = self.running.lock().await;
        // A different id (signed in as another machine) is a different group.
        if running.as_ref().is_some_and(|(id, _)| *id != my_id)
            && let Some((_, r)) = running.take()
        {
            self.stop(r).await;
        }
        match decision {
            Decision::Off(why) => {
                if let Some((_, r)) = running.take() {
                    tracing::info!(
                        why,
                        "volume: replication stopped; the Volume is a plain local store again"
                    );
                    self.stop(r).await;
                }
            }
            Decision::On => {
                let mut members: Vec<Member> = others
                    .iter()
                    .map(|m| Member {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        ..Default::default()
                    })
                    .collect();
                members.push(Member {
                    id: my_id.clone(),
                    name: self.name.clone(),
                    ..Default::default()
                });
                if running.is_none() {
                    match self.start(&my_id, members.clone()).await {
                        Ok(r) => {
                            tracing::info!(
                                machines = members.len(),
                                "volume: replicating across your machines"
                            );
                            *running = Some((my_id.clone(), r));
                        }
                        Err(e) => tracing::warn!(error = %e, "volume: replication did not start"),
                    }
                } else if let Some((_, r)) = running.as_ref() {
                    r.node.set_members(members).await;
                    r.node.set_facts((self.facts)());
                }
            }
        }
    }

    async fn start(&self, me: &str, members: Vec<Member>) -> Result<Running, String> {
        let state = cua_volume::state_dir(&self.home);
        let store = cua_volume::fs::FsBackend::new(state.join("data"));
        let node = ReplicaNode::open(
            self.cfg.clone(),
            me,
            &self.name,
            &state,
            store,
            Arc::new(MeshTransport(self.mesh.clone())),
            members,
        )
        .await
        .map_err(|e| e.to_string())?;
        node.set_facts((self.facts)());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let local = listener.local_addr().map_err(|e| e.to_string())?;
        let port = self.mesh.register(local).map_err(|e| e.to_string())?;
        let n2 = node.clone();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else {
                    return;
                };
                let _ = s.set_nodelay(true);
                n2.accept(Box::new(s)).await;
            }
        });
        node.start();
        self.drive.set_backend(node.backend());
        Ok(Running {
            node,
            _port: port,
            accept,
        })
    }

    async fn stop(&self, r: Running) {
        // The same directory goes back to being the whole Volume.
        let state = cua_volume::state_dir(&self.home);
        self.drive
            .set_backend(Arc::new(cua_volume::fs::FsBackend::new(state.join("data"))));
        r.accept.abort();
        r.node.shutdown().await;
    }

    /// What the app shows: the replication status and the one paragraph.
    pub async fn status(&self) -> Option<(cua_volume::replica::ReplicaStatus, String)> {
        let running = self.running.lock().await;
        let (_, r) = running.as_ref()?;
        let s = r.node.status().await;
        let words = cua_volume::replica::words::closing_summary(&r.node.summary(None).await);
        Some((s, words))
    }

    pub async fn shutdown(&self) {
        if let Some((_, r)) = self.running.lock().await.take() {
            self.stop(r).await;
        }
    }
}

/// Runs the manager for the life of the daemon: looks every few seconds.
pub async fn run(runtime: Arc<ReplicaRuntime>) {
    let mut every = tokio::time::interval(Duration::from_secs(5));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        runtime.tick().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_volume::Condition;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// The account's machines over loopback TCP, shared by every test machine.
    #[derive(Default)]
    struct World {
        listeners: Mutex<HashMap<String, SocketAddr>>,
        machines: Mutex<Vec<MachineRef>>,
    }

    struct Fake {
        world: Arc<World>,
        me: String,
        signed_in: bool,
    }

    #[async_trait::async_trait]
    impl MeshAccess for Fake {
        async fn machines(&self) -> Option<Vec<MachineRef>> {
            self.signed_in.then(|| {
                self.world
                    .machines
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|m| m.id != self.me)
                    .cloned()
                    .collect()
            })
        }

        async fn dial(&self, machine: &str) -> std::io::Result<Stream> {
            let a = *self
                .world
                .listeners
                .lock()
                .unwrap()
                .get(machine)
                .ok_or_else(|| std::io::Error::other("not registered"))?;
            Ok(Box::new(tokio::net::TcpStream::connect(a).await?))
        }

        fn register(
            &self,
            local: SocketAddr,
        ) -> std::io::Result<Box<dyn std::any::Any + Send + Sync>> {
            self.world
                .listeners
                .lock()
                .unwrap()
                .insert(self.me.clone(), local);
            Ok(Box::new(()))
        }
    }

    fn machine(
        world: &Arc<World>,
        home: &Path,
        id: &str,
        rank: u64,
        signed_in: bool,
    ) -> Arc<ReplicaRuntime> {
        world.machines.lock().unwrap().push(MachineRef {
            id: id.into(),
            name: id.into(),
            online: true,
        });
        let drive = cua_volume::Drive::open_local(home);
        let id2 = id.to_string();
        ReplicaRuntime::new(
            home,
            Arc::new(move || signed_in.then(|| id2.clone())),
            id,
            Arc::new(Fake {
                world: world.clone(),
                me: id.into(),
                signed_in,
            }),
            drive,
            Config::fast(),
            Arc::new(move || Facts {
                rank,
                free_disk: 100 << 30,
                ..Default::default()
            }),
        )
    }

    #[tokio::test]
    async fn nothing_starts_signed_out_alone_or_with_a_bucket() {
        let home = tempfile::tempdir().unwrap();
        let world = Arc::new(World::default());
        let rr = machine(&world, home.path(), "solo", 5000, false);
        rr.tick().await;
        assert!(rr.status().await.is_none(), "signed out: nothing runs");
        assert_eq!(rr.drive.backend().kind(), "fs");
        assert!(
            !home.path().join("volume/replica").exists(),
            "no replication files appear"
        );
        // Signed in but the only machine.
        let home2 = tempfile::tempdir().unwrap();
        let world2 = Arc::new(World::default());
        let rr = machine(&world2, home2.path(), "solo", 5000, true);
        rr.tick().await;
        assert!(rr.status().await.is_none());
        assert!(!home2.path().join("volume/replica").exists());
        // Two machines, but a bucket is configured: the bucket is the store.
        let home3 = tempfile::tempdir().unwrap();
        let world3 = Arc::new(World::default());
        std::fs::create_dir_all(home3.path().join("volume")).unwrap();
        std::fs::write(
            home3.path().join("volume/config.json"),
            br#"{"backend":"s3","s3":{"bucket":"mine","region":"us-east-1"}}"#,
        )
        .unwrap();
        let _other = machine(
            &world3,
            tempfile::tempdir().unwrap().path(),
            "other",
            9000,
            true,
        );
        let rr = machine(&world3, home3.path(), "me", 5000, true);
        rr.tick().await;
        assert!(rr.status().await.is_none(), "a bucket means no replication");
        assert!(!home3.path().join("volume/replica").exists());
    }

    #[tokio::test]
    async fn a_second_machine_starts_replication_and_leaving_stops_it_without_losing_files() {
        let (ha, hb) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let world = Arc::new(World::default());
        let a = machine(&world, ha.path(), "laptop", 2000, true);
        let b = machine(&world, hb.path(), "studio", 9000, true);
        // The laptop already has a file from before.
        a.drive
            .backend()
            .put("agents/ada/old.txt", b"before".to_vec(), Condition::None)
            .await
            .unwrap();
        a.tick().await;
        b.tick().await;
        assert_eq!(a.drive.backend().kind(), "mesh");
        // The files meet, and writes reach both.
        let end = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            a.tick().await;
            b.tick().await;
            let there = b
                .drive
                .backend()
                .head("agents/ada/old.txt")
                .await
                .ok()
                .flatten();
            if there.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < end,
                "the old file never reached the studio"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        b.drive
            .backend()
            .put("agents/ada/new.txt", b"after".to_vec(), Condition::None)
            .await
            .unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(30);
        while a
            .drive
            .backend()
            .head("agents/ada/new.txt")
            .await
            .unwrap()
            .is_none()
        {
            assert!(
                std::time::Instant::now() < end,
                "the new file never reached the laptop"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (st, words) = a.status().await.unwrap();
        assert!(st.copies_wanted == 2, "{st:?}");
        assert!(words.contains("Mac") || !words.is_empty());
        // The studio leaves the account: the laptop is a plain local Volume again.
        world.machines.lock().unwrap().retain(|m| m.id != "studio");
        a.tick().await;
        assert!(a.status().await.is_none());
        assert_eq!(a.drive.backend().kind(), "fs");
        assert_eq!(
            a.drive
                .backend()
                .get("agents/ada/old.txt", None)
                .await
                .unwrap()
                .0,
            b"before"
        );
        assert_eq!(
            a.drive
                .backend()
                .get("agents/ada/new.txt", None)
                .await
                .unwrap()
                .0,
            b"after"
        );
        a.shutdown().await;
        b.shutdown().await;
    }
}
