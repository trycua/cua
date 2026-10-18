// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `<cua home>/cloud/state.json` (0600): this home's owner id, the
//! connected clouds (names only) and every resource Cua created, with its
//! cloud id. A resource is recorded before it exists in the cloud (by the
//! name Cua chose), so a create that dies half way is still found and
//! cleaned up.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{Connection, Resource};

/// The file's content.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    /// A random id for this cua home, the `cua-owner` tag value.
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    resources: Vec<Resource>,
}

/// This home's cloud state. Every change is a read-modify-write under a
/// process lock with an atomic rename.
pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Store {
    /// The store under `home` (`<home>/cloud/state.json`).
    pub fn new(home: &Path) -> Self {
        Store {
            path: home.join("cloud").join("state.json"),
            lock: Mutex::new(()),
        }
    }

    /// Where it lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cua home it belongs to.
    pub fn home(&self) -> Option<PathBuf> {
        self.path.parent()?.parent().map(Path::to_path_buf)
    }

    fn read(&self) -> std::io::Result<State> {
        match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", self.path.display()),
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e),
        }
    }

    fn write(&self, s: &State) -> std::io::Result<()> {
        let dir = self.path.parent().expect("state dir");
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".state.{}.tmp", std::process::id()));
        {
            use std::io::Write as _;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                o.mode(0o600);
            }
            let mut f = o.open(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(s).expect("state serializes"))?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
    }

    fn update<T>(&self, f: impl FnOnce(&mut State) -> T) -> std::io::Result<T> {
        let _g = self.lock.lock().expect("cloud state lock");
        let mut s = self.read()?;
        let out = f(&mut s);
        self.write(&s)?;
        Ok(out)
    }

    /// This home's owner id (created on first use): the `cua-owner` tag.
    pub fn owner_id(&self) -> std::io::Result<String> {
        {
            let _g = self.lock.lock().expect("cloud state lock");
            let s = self.read()?;
            if !s.owner_id.is_empty() {
                return Ok(s.owner_id);
            }
        }
        self.update(|s| {
            if s.owner_id.is_empty() {
                s.owner_id = format!("{:016x}", rand::random::<u64>());
            }
            s.owner_id.clone()
        })
    }

    /// The connection for `provider`, if connected.
    pub fn connection(&self, provider: &str) -> std::io::Result<Option<Connection>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self
            .read()?
            .connections
            .into_iter()
            .find(|c| c.provider == provider))
    }

    /// Every connection.
    pub fn connections(&self) -> std::io::Result<Vec<Connection>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self.read()?.connections)
    }

    /// Saves `c` (replacing the provider's earlier connection).
    pub fn connect(&self, c: Connection) -> std::io::Result<()> {
        self.update(|s| {
            s.connections.retain(|x| x.provider != c.provider);
            s.connections.push(c);
        })
    }

    /// Forgets `provider`'s connection; whether there was one.
    pub fn disconnect(&self, provider: &str) -> std::io::Result<bool> {
        self.update(|s| {
            let before = s.connections.len();
            s.connections.retain(|x| x.provider != provider);
            before != s.connections.len()
        })
    }

    /// Every recorded resource.
    pub fn resources(&self) -> std::io::Result<Vec<Resource>> {
        let _g = self.lock.lock().expect("cloud state lock");
        Ok(self.read()?.resources)
    }

    /// Records `r` (by provider, type and name, or id): inserts or replaces.
    pub fn record(&self, r: Resource) -> std::io::Result<()> {
        self.update(|s| {
            s.resources.retain(|x| !same(x, &r));
            s.resources.push(r);
        })
    }

    /// Forgets `r`.
    pub fn forget(&self, r: &Resource) -> std::io::Result<()> {
        self.update(|s| s.resources.retain(|x| !same(x, r)))
    }
}

/// The same resource: same provider and type, and the same cloud id (or,
/// while one is pending, the same name).
fn same(a: &Resource, b: &Resource) -> bool {
    a.provider == b.provider
        && a.resource_type == b.resource_type
        && ((!a.id.is_empty() && a.id == b.id) || (!a.name.is_empty() && a.name == b.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_a_reopen_and_pending_ones_resolve_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        let owner = s.owner_id().unwrap();
        assert_eq!(owner.len(), 16);
        assert_eq!(Store::new(dir.path()).owner_id().unwrap(), owner);

        let mut r = Resource {
            provider: "aws".into(),
            resource_type: "instance".into(),
            name: "cua-space-1".into(),
            ..Default::default()
        };
        s.record(r.clone()).unwrap();
        assert!(s.resources().unwrap()[0].pending());
        r.id = "i-1".into();
        s.record(r.clone()).unwrap();
        let all = Store::new(dir.path()).resources().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "i-1");
        s.forget(&r).unwrap();
        assert!(s.resources().unwrap().is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(s.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn one_connection_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path());
        for region in ["us-east-1", "us-west-2"] {
            s.connect(Connection {
                provider: "aws".into(),
                region: region.into(),
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(s.connections().unwrap().len(), 1);
        assert_eq!(s.connection("aws").unwrap().unwrap().region, "us-west-2");
        assert!(s.disconnect("aws").unwrap());
        assert!(!s.disconnect("aws").unwrap());
    }
}
