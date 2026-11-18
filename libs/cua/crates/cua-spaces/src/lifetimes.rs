//! Space lifetimes: a Space ends once its lifetime passes, deleted or
//! turned off ([`LocalExpiry`]), whether or not the client that set it is
//! still running. The record lives next to the registry
//! (`space-lifetimes.json`) of the machine where the Space is registered,
//! and the cua daemon there ends expired Spaces ([`Spaces::reap_expired`]):
//! a Space on one of your other machines is ended through its host the way
//! `delete` and `stop` reach it.
//!
//! One store, one view: a Space on this machine (`local:<name>`) has the
//! lifetime of its sandbox, the `expires_at_unix` / `on_expire` fields of
//! the sandbox's own state (`cua_sandbox_core::Sandboxes::set_local_expiry`),
//! so `Sandbox.set_lifetime`, `Spaces.set_lifetime`, the sandbox's info and
//! the Space's info all read and write the same record, and the daemon's
//! sandbox reaper ends it. The `space-lifetimes.json` file keeps the
//! lifetimes of the other Spaces (a machine of yours, a Space added by
//! address, or a local Space with no sandbox of this home).

use crate::{Error, Result, Spaces};
use cua_sandbox_core::LocalExpiry;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The file, in the Spaces home.
pub const LIFETIMES_FILE: &str = "space-lifetimes.json";

/// One Space's lifetime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifetime {
    /// When it ends (Unix seconds).
    pub expires_at_unix: u64,
    /// `delete` or `suspend`.
    pub on_expire: String,
}

impl Lifetime {
    /// When it ends.
    pub fn expires_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.expires_at_unix)
    }

    /// What ends it.
    pub fn action(&self) -> LocalExpiry {
        LocalExpiry::parse(&self.on_expire).unwrap_or_default()
    }
}

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn path(home: &Path) -> PathBuf {
    home.join(LIFETIMES_FILE)
}

fn load(home: &Path) -> BTreeMap<String, Lifetime> {
    std::fs::read(path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Every Space's lifetime, by canonical id.
pub(crate) fn all(home: &Path) -> BTreeMap<String, Lifetime> {
    load(home)
}

/// The sandbox a local Space id (`local:<name>`) names.
fn local_name(id: &str) -> Option<&str> {
    id.strip_prefix("local:").filter(|n| !n.is_empty())
}

/// Drops a Space's lifetime record (the Space is gone).
pub(crate) fn forget(home: &Path, id: &str) -> Result<()> {
    if !path(home).exists() {
        return Ok(());
    }
    update(home, |all| {
        all.remove(id);
    })
}

fn save(home: &Path, all: &BTreeMap<String, Lifetime>) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let target = path(home);
    let tmp = target.with_extension(format!("tmp-{:08x}", rand::random::<u32>()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(all)?)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

fn update<T>(home: &Path, f: impl FnOnce(&mut BTreeMap<String, Lifetime>) -> T) -> Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut all = load(home);
    let out = f(&mut all);
    save(home, &all)?;
    Ok(out)
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Spaces {
    /// Gives a registered Space a lifetime of `duration` from now, replacing
    /// any earlier one (shorter or longer). When it passes, the cua daemon
    /// of this machine deletes the Space (`on_expire` [`LocalExpiry::Delete`],
    /// the default) or turns it off ([`LocalExpiry::Suspend`]; `start`
    /// brings it back). `None` keeps the Space's current choice. Returns
    /// when it ends.
    pub fn set_lifetime(
        &self,
        space: &str,
        duration: Duration,
        on_expire: Option<LocalExpiry>,
    ) -> Result<SystemTime> {
        let id = self.resolve(space)?.to_string();
        if self.registry().get(&id)?.is_none() {
            return Err(Error::NotFound(format!("Space {id}")));
        }
        // A local Space's lifetime is its sandbox's: one record.
        if let Some(name) = local_name(&id) {
            match self
                .inner
                .sandboxes
                .set_local_expiry(name, duration, on_expire)
            {
                Ok(()) => {
                    // An earlier record in the file would be a second copy.
                    let _ = forget(self.home_dir(), &id);
                    if let Some(l) = self.sandbox_lifetime(&id) {
                        return Ok(l.expires_at());
                    }
                }
                Err(cua_sandbox_core::Error::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let until = unix(SystemTime::now()).saturating_add(duration.as_secs_f64().ceil() as u64);
        let record = update(self.home_dir(), |all| {
            let action = on_expire
                .or_else(|| all.get(&id).map(Lifetime::action))
                .unwrap_or_default();
            let record = Lifetime {
                expires_at_unix: until,
                on_expire: action.as_str().into(),
            };
            all.insert(id.clone(), record.clone());
            record
        })?;
        Ok(record.expires_at())
    }

    /// A Space's lifetime ([`Spaces::set_lifetime`]), if it has one.
    pub fn lifetime(&self, space: &str) -> Option<Lifetime> {
        let id = self.resolve(space).ok()?.to_string();
        self.lifetime_of(&id, &load(self.home_dir()))
    }

    /// The lifetime a local Space's sandbox carries.
    fn sandbox_lifetime(&self, id: &str) -> Option<Lifetime> {
        let (at, action) = self.inner.sandboxes.local_expiry(local_name(id)?)?;
        Some(Lifetime {
            expires_at_unix: unix(at),
            on_expire: action.as_str().into(),
        })
    }

    /// The one lifetime of Space `id`: its sandbox's when it is a local
    /// Space with one, else the file's (`file` is [`all`]).
    pub(crate) fn lifetime_of(
        &self,
        id: &str,
        file: &BTreeMap<String, Lifetime>,
    ) -> Option<Lifetime> {
        self.sandbox_lifetime(id).or_else(|| file.get(id).cloned())
    }

    /// Removes a Space's lifetime (it then runs until deleted).
    pub fn clear_lifetime(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?.to_string();
        if let Some(name) = local_name(&id) {
            self.inner.sandboxes.clear_local_expiry(name)?;
        }
        update(self.home_dir(), |all| {
            all.remove(&id);
        })
    }

    /// The Spaces whose lifetime ended at or before `now`, except the local
    /// Spaces that carry theirs in their sandbox: the daemon's sandbox
    /// reaper ends those (`Runtime::reap_expired`).
    pub fn expired(&self, now: SystemTime) -> Vec<(String, LocalExpiry)> {
        load(self.home_dir())
            .into_iter()
            .filter(|(id, _)| self.sandbox_lifetime(id).is_none())
            .filter(|(_, l)| l.expires_at() <= now)
            .map(|(id, l)| (id, l.action()))
            .collect()
    }

    /// Ends every Space whose lifetime passed (the cua daemon calls this
    /// every few seconds): deletes it or turns it off, then drops its
    /// lifetime. A Space that is gone already only loses its record; one
    /// that could not be ended is tried again on the next call. Returns the
    /// Spaces it ended.
    pub async fn reap_expired(&self) -> Vec<String> {
        let mut ended = Vec::new();
        for (id, action) in self.expired(SystemTime::now()) {
            let registered = matches!(self.registry().get(&id), Ok(Some(_)));
            let r = if !registered {
                Ok(())
            } else {
                match action {
                    LocalExpiry::Delete => self.delete(&id).await.map(drop),
                    LocalExpiry::Suspend => self.stop(&id).await.map(drop),
                }
            };
            match r {
                Ok(()) | Err(Error::NotFound(_)) => {
                    let _ = update(self.home_dir(), |all| {
                        all.remove(&id);
                    });
                    if registered {
                        tracing::info!(space = %id, action = action.as_str(), "Space lifetime ended");
                        ended.push(id);
                    }
                }
                Err(e) => tracing::warn!(space = %id, error = %e, "could not end an expired Space"),
            }
        }
        ended
    }
}
