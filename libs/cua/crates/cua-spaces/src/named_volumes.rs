// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named volumes: folders of the user's Cua Volume (`volumes/<name>/`)
//! mounted into Spaces.
//!
//! A [`VolumeMount`] says which volume appears where in a Space. The Space
//! record keeps the list; the Cua Volume extension grants the Space exactly
//! those folders (read-write or read-only) on top of its own, and the guest
//! binds them at their paths once the volume is mounted: a bind mount on
//! Linux (containers, microVMs and VMs alike), a symbolic link on macOS.
//! Naming and storage of the volumes themselves is the extension's
//! (`volumes_list`, `volumes_create`, `volumes_delete`).

use std::collections::BTreeMap;
use std::path::{Component, Path};

use cua_proto::env::v1 as pb;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, Spaces};

/// Most volumes one Space mounts.
pub const MAX_MOUNTS: usize = 32;

/// A named volume mounted in a Space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeMount {
    /// The volume: 1 to 63 of `a-z`, `0-9`, `.`, `_`, `-`, starting with a
    /// letter or digit.
    pub name: String,
    /// Where it appears in the Space (absolute).
    pub path: String,
    /// Mounted read-only.
    #[serde(default)]
    pub read_only: bool,
    /// A folder inside the volume to mount instead of all of it (relative;
    /// no `..`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subpath: String,
}

/// Whether `name` is a valid volume name.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && !name.ends_with('.')
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// The folder key of volume `name` in the Cua Volume: `volumes/<name>/`.
pub fn folder_of(name: &str) -> String {
    format!("volumes/{name}/")
}

impl VolumeMount {
    /// A mount of all of `name` at `path`.
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            ..Default::default()
        }
    }

    /// The folder this mount covers, as a Cua Volume key:
    /// `volumes/<name>/<subpath>/`.
    pub fn folder(&self) -> String {
        let sub = self.subpath.trim().trim_matches('/');
        if sub.is_empty() {
            folder_of(&self.name)
        } else {
            format!("{}{sub}/", folder_of(&self.name))
        }
    }

    fn check(&self) -> Result<()> {
        if !valid_name(&self.name) {
            return Err(Error::invalid(format!(
                "volume name {:?}: use 1 to 63 of a-z, 0-9, `.`, `_` and `-`, starting with a \
                 letter or digit",
                self.name
            )));
        }
        // The path is inside the guest, where paths are POSIX whatever this
        // machine is: a Windows host's `Path` calls `/mnt/a` relative.
        if !self.path.starts_with('/')
            || self.path.trim_matches('/').is_empty()
            || self.path.split('/').any(|seg| seg == "." || seg == "..")
        {
            return Err(Error::invalid(format!(
                "volume {:?}: the path {:?} must be absolute, without `.` or `..` (for example \
                 /mnt/{})",
                self.name, self.path, self.name
            )));
        }
        let sub = self.subpath.trim().trim_matches('/');
        if !sub.is_empty()
            && (self.subpath.trim().starts_with("//")
                || !Path::new(sub)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                || sub.chars().any(|c| c.is_control() || c == '\\'))
        {
            return Err(Error::invalid(format!(
                "volume {:?}: the subpath {:?} must be a relative folder without `.` or `..`",
                self.name, self.subpath
            )));
        }
        Ok(())
    }
}

/// The folders of a guest path, in order (empty ones, from `//`, dropped).
fn guest_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Checks a Space's mounts: each valid, no two at the same or nested paths.
pub fn validate(mounts: &[VolumeMount]) -> Result<()> {
    if mounts.len() > MAX_MOUNTS {
        return Err(Error::invalid(format!(
            "{} volumes; a Space mounts at most {MAX_MOUNTS}",
            mounts.len()
        )));
    }
    for (i, m) in mounts.iter().enumerate() {
        m.check()?;
        let p = guest_segments(&m.path);
        for other in &mounts[..i] {
            let q = guest_segments(&other.path);
            if p.starts_with(&q) || q.starts_with(&p) {
                return Err(Error::invalid(format!(
                    "volumes {:?} and {:?} are mounted at {} and {}: paths cannot be the same \
                     or inside one another",
                    other.name, m.name, other.path, m.path
                )));
            }
        }
    }
    Ok(())
}

impl From<&VolumeMount> for pb::VolumeMount {
    fn from(m: &VolumeMount) -> Self {
        pb::VolumeMount {
            name: m.name.clone(),
            path: m.path.clone(),
            read_only: m.read_only,
            subpath: m.subpath.clone(),
        }
    }
}

impl From<pb::VolumeMount> for VolumeMount {
    fn from(m: pb::VolumeMount) -> Self {
        VolumeMount {
            name: m.name,
            path: m.path,
            read_only: m.read_only,
            subpath: m.subpath,
        }
    }
}

/// The guest binds for `mounts`: each folder of the volume at its path.
pub fn binds(mounts: &[VolumeMount]) -> Vec<pb::VolumeBind> {
    mounts
        .iter()
        .map(|m| pb::VolumeBind {
            source: m.folder(),
            target: m.path.clone(),
            read_only: m.read_only,
        })
        .collect()
}

/// The guest feature of binds.
pub const BINDS_FEATURE: &str = "volume.binds";

impl Spaces {
    /// The named volumes mounted in `space`, as its record has them.
    pub fn volume_mounts(&self, space: &str) -> Result<Vec<VolumeMount>> {
        let id = self.resolve(space)?.to_string();
        let own: Vec<VolumeMount> = self
            .inner
            .registry
            .get(&id)?
            .map(|r| r.volumes.into_iter().map(Into::into).collect())
            .unwrap_or_default();
        if !own.is_empty() {
            return Ok(own);
        }
        Ok(self
            .inner
            .registry
            .volume_overlay()?
            .remove(&id)
            .unwrap_or_default())
    }

    /// Fills in the mounts of a Space that has no record of its own.
    pub(crate) fn apply_volumes(&self, info: &mut crate::SpaceInfo) {
        if info.volumes.is_empty()
            && let Ok(mut o) = self.inner.registry.volume_overlay()
            && let Some(v) = o.remove(&info.id)
        {
            info.volumes = v;
        }
    }

    /// Which Spaces mount which named volume: volume name to the Space ids.
    pub fn volume_users(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in self.inner.registry.list()? {
            for m in &r.volumes {
                let users = out.entry(m.name.clone()).or_default();
                if !users.contains(&r.id) {
                    users.push(r.id.clone());
                }
            }
        }
        for (id, mounts) in self.inner.registry.volume_overlay()? {
            for m in mounts {
                let users = out.entry(m.name).or_default();
                if !users.contains(&id) {
                    users.push(id.clone());
                }
            }
        }
        Ok(out)
    }

    /// Remembers the mounts of a Space about to be registered (its record
    /// does not exist yet; the volume attaches the moment it connects).
    pub(crate) fn expect_volumes(&self, id: &str, mounts: Vec<VolumeMount>) {
        if id.is_empty() {
            return;
        }
        let mut pending = self.inner.pending_volumes.lock().expect("pending volumes");
        if mounts.is_empty() {
            pending.remove(id);
        } else {
            pending.insert(id.to_string(), mounts);
        }
    }

    /// The mounts a record being written for `id` starts with.
    pub(crate) fn take_expected_volumes(&self, id: &str) -> Vec<pb::VolumeMount> {
        self.inner
            .pending_volumes
            .lock()
            .expect("pending volumes")
            .remove(id)
            .map(|v| v.iter().map(Into::into).collect())
            .unwrap_or_default()
    }

    /// Mounts the Cua Volume in `space` now and waits until the named
    /// volumes are in place (the Cua Volume extension; an error says why
    /// not: no FUSE in the guest, an older cua-spacesd, a refused bind).
    pub async fn ensure_volumes(&self, space: &str) -> Result<()> {
        self.call_extension(
            "Cua Volume (volumes in Spaces)",
            "volume.attach",
            serde_json::json!({ "space": space }),
        )
        .await
        .map(|_| ())
    }

    /// Refuses mounts this runtime cannot serve: no Cua Volume extension.
    pub(crate) fn require_volumes(&self, mounts: &[VolumeMount]) -> Result<()> {
        validate(mounts)?;
        if !mounts.is_empty() && !self.inner.extensions.iter().any(|e| e.name() == "drive") {
            return Err(Error::needs_cua_spaces("Cua Volume (volumes in Spaces)"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, path: &str) -> VolumeMount {
        VolumeMount::new(name, path)
    }

    #[test]
    fn names_follow_the_volume_rules() {
        for ok in ["shared", "a", "my-data.v2", "0abc", "a_b"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Shared",
            "-x",
            ".x",
            "a/b",
            "a b",
            "x.",
            &"a".repeat(64),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_mount_covers_its_folder() {
        assert_eq!(m("shared", "/mnt/s").folder(), "volumes/shared/");
        let mut sub = m("shared", "/mnt/s");
        sub.subpath = "/data/in/".into();
        assert_eq!(sub.folder(), "volumes/shared/data/in/");
        let b = binds(&[sub]);
        assert_eq!(b[0].source, "volumes/shared/data/in/");
        assert_eq!(b[0].target, "/mnt/s");
        assert!(!b[0].read_only);
    }

    #[test]
    fn validation_names_the_problem() {
        assert!(validate(&[]).is_ok());
        assert!(validate(&[m("a", "/mnt/a"), m("b", "/mnt/b")]).is_ok());
        let e = |v: &[VolumeMount]| validate(v).unwrap_err().to_string();
        assert!(e(&[m("Bad", "/mnt/a")]).contains("volume name"));
        assert!(e(&[m("a", "mnt/a")]).contains("absolute"));
        assert!(e(&[m("a", "/")]).contains("absolute"));
        assert!(e(&[m("a", "/mnt/../etc")]).contains("absolute"));
        assert!(e(&[m("a", "/mnt/./a")]).contains("absolute"));
        assert!(e(&[m("a", "/mnt/a"), m("b", "/mnt/a")]).contains("same"));
        assert!(e(&[m("a", "/mnt"), m("b", "/mnt/b")]).contains("inside"));
        let mut s = m("a", "/mnt/a");
        s.subpath = "../x".into();
        assert!(e(&[s]).contains("subpath"));
        let many: Vec<_> = (0..=MAX_MOUNTS)
            .map(|i| m("a", &format!("/mnt/{i}")))
            .collect();
        assert!(e(&many).contains("at most"));
        // The same volume at two places is fine.
        assert!(validate(&[m("a", "/mnt/a1"), m("a", "/mnt/a2")]).is_ok());
    }

    #[test]
    fn the_wire_type_round_trips() {
        let mut v = m("shared", "/mnt/s");
        v.read_only = true;
        v.subpath = "x".into();
        let p: pb::VolumeMount = (&v).into();
        assert_eq!(VolumeMount::from(p), v);
    }
}
