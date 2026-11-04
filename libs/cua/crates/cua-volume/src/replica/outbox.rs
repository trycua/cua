// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Changes this machine saved that the owner has not taken yet.
//!
//! One JSON file per change in `<state>/outbox/`. A change survives a
//! restart; it leaves the outbox only when the owner says what it did with
//! it. The bytes of a pending write are a version in the spool store (or, for
//! the changes a machine hands over when it steps down, a version in its data
//! store), so history is never rewritten.

use std::path::{Path, PathBuf};

use super::wire::Pending;
use crate::{Error, Result};

#[derive(Debug)]
pub struct Outbox {
    dir: PathBuf,
    items: Vec<Pending>,
}

impl Outbox {
    pub fn open(state: &Path) -> Result<Outbox> {
        let dir = state.join("outbox");
        std::fs::create_dir_all(&dir)?;
        let mut items = vec![];
        for e in std::fs::read_dir(&dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(b) = std::fs::read(&p)
                && let Ok(item) = serde_json::from_slice::<Pending>(&b)
            {
                items.push(item);
            }
        }
        items.sort_by(|a, b| (a.ts_ms, &a.id).cmp(&(b.ts_ms, &b.id)));
        Ok(Outbox { dir, items })
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn add(&mut self, p: Pending) -> Result<()> {
        let f = self.file(&p.id);
        cua_home::guard_write(&f)?;
        let tmp = f.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&p)?)?;
        std::fs::rename(&tmp, &f)?;
        self.items.push(p);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<Option<Pending>> {
        let Some(i) = self.items.iter().position(|p| p.id == id) else {
            return Ok(None);
        };
        let p = self.items.remove(i);
        match std::fs::remove_file(self.file(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::from(e)),
        }
        Ok(Some(p))
    }

    /// Pending changes of the same writer chain on after `old` (a version
    /// of ours the owner stored as `new`): they now follow `new`.
    pub fn rebase(&mut self, old: &str, new: &str) -> Result<()> {
        for i in 0..self.items.len() {
            if self.items[i].base.as_deref() == Some(old) {
                self.items[i].base = Some(new.to_string());
                let f = self.file(&self.items[i].id);
                std::fs::write(&f, serde_json::to_vec(&self.items[i])?)?;
            }
        }
        Ok(())
    }

    pub fn items(&self) -> &[Pending] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.items.iter().any(|p| p.key == key)
    }

    /// Bytes of the writes waiting (what a status line shows).
    pub fn bytes(&self) -> u64 {
        self.items
            .iter()
            .filter_map(|p| p.meta.as_ref().map(|m| m.size))
            .sum()
    }
}
