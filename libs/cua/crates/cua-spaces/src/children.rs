//! Which Spaces may make child Spaces of their own.
//!
//! The setting is the owner's (`SetSpaceChildren`, `cua spaces children`,
//! `create --children`), and an agent run turns it on for its Space. The
//! cua daemon of the machine that runs the Space keeps the Space's spawn
//! channel while it is on. Kept in `<home>/spawn/enabled.json` with each
//! Space's creation time, so a Space deleted and made again under the same
//! name starts with it off.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Spaces;

/// The file, in the Spaces home.
pub const ENABLED_FILE: &str = "spawn/enabled.json";

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Woken when a Space's setting changes (the daemon's channel loop waits on
/// it).
pub static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    spaces: BTreeMap<String, u64>,
}

fn path(home: &Path) -> PathBuf {
    home.join(ENABLED_FILE)
}

fn load(home: &Path) -> BTreeMap<String, u64> {
    std::fs::read(path(home))
        .ok()
        .and_then(|b| serde_json::from_slice::<File>(&b).ok())
        .unwrap_or_default()
        .spaces
}

fn save(home: &Path, spaces: &BTreeMap<String, u64>) -> std::io::Result<()> {
    let p = path(home);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&File {
            spaces: spaces.clone(),
        })
        .unwrap_or_default(),
    )?;
    std::fs::rename(&tmp, &p)
}

impl Spaces {
    fn created_at(&self, id: &str) -> u64 {
        self.registry()
            .meta(id)
            .map(|m| m.created_at_unix)
            .unwrap_or(0)
    }

    /// Turns child Spaces on or off for registered Space `id`.
    pub fn set_children(&self, id: &str, on: bool) -> crate::Result<()> {
        let at = self.created_at(id);
        {
            let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut all = load(self.home_dir());
            let changed = if on {
                all.insert(id.to_owned(), at) != Some(at)
            } else {
                all.remove(id).is_some()
            };
            if changed {
                save(self.home_dir(), &all)?;
            }
        }
        CHANGED.notify_waiters();
        Ok(())
    }

    /// Whether `id` (this very Space, not an earlier one of the same name)
    /// may make children.
    pub fn children_on(&self, id: &str) -> bool {
        load(self.home_dir())
            .get(id)
            .is_some_and(|at| *at == self.created_at(id))
    }

    /// Every Space with children on.
    pub fn children_enabled(&self) -> Vec<String> {
        load(self.home_dir())
            .into_iter()
            .filter(|(id, at)| *at == self.created_at(id))
            .map(|(id, _)| id)
            .collect()
    }

    /// Drops the settings of Spaces that are no longer registered.
    pub fn forget_children_of_gone(&self, registered: &std::collections::BTreeSet<String>) {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = load(self.home_dir());
        let before = all.len();
        all.retain(|id, _| registered.contains(id));
        if all.len() != before {
            let _ = save(self.home_dir(), &all);
        }
    }
}
