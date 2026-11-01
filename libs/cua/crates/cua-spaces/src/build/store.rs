//! What builds leave on disk besides their checkpoints: one record per
//! build (its layers and image configuration), the tags that name builds,
//! and the user cua-spacesd runs as in each base layer.
//!
//! `<home>/builds/records/<key12>.json`, `<home>/builds/tags.json`,
//! `<home>/builds/bases/<key12>.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::recipe::ImageConfig;
use crate::error::{Error, Result};

/// A finished build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildRecord {
    /// The cache key of the whole recipe.
    pub key: String,
    /// The final layer's checkpoint.
    pub checkpoint: String,
    /// Every layer's checkpoint, base first.
    pub layers: Vec<String>,
    /// The image configuration.
    pub config: ImageConfig,
    /// RFC 3339 time of the build.
    pub created_at: String,
}

/// A build as listed ([`crate::Spaces::builds`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildInfo {
    /// The final layer's checkpoint.
    pub checkpoint: String,
    /// The cache key of the recipe.
    pub key: String,
    /// Tags naming it.
    pub tags: Vec<String>,
    /// The `FROM` reference.
    pub base: String,
    /// Layers it is made of (checkpoints).
    pub layers: Vec<String>,
    /// When it was built.
    pub created_at: String,
    /// Image configuration.
    pub config: ImageConfig,
}

fn dir(home: &Path) -> PathBuf {
    home.join("builds")
}

fn key12(key: &str) -> &str {
    &key[..12.min(key.len())]
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Whether `tag` is a usable tag: 1-63 of `[A-Za-z0-9._-]`, starting with a
/// letter or digit.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 63
        && tag
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub(super) fn save_record(home: &Path, rec: &BuildRecord) -> Result<()> {
    write_atomic(
        &dir(home)
            .join("records")
            .join(format!("{}.json", key12(&rec.key))),
        &serde_json::to_vec_pretty(rec)?,
    )
}

pub(super) fn load_records(home: &Path) -> Vec<BuildRecord> {
    let Ok(rd) = std::fs::read_dir(dir(home).join("records")) else {
        return vec![];
    };
    let mut v: Vec<BuildRecord> = rd
        .flatten()
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    v
}

pub(super) fn load_record(home: &Path, key: &str) -> Option<BuildRecord> {
    let p = dir(home)
        .join("records")
        .join(format!("{}.json", key12(key)));
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

pub(super) fn remove_record(home: &Path, key: &str) {
    let _ = std::fs::remove_file(
        dir(home)
            .join("records")
            .join(format!("{}.json", key12(key))),
    );
}

pub(super) fn tags(home: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir(home).join("tags.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Points `tag` at the build with `key` (replacing what it named).
pub(super) fn set_tag(home: &Path, tag: &str, key: &str) -> Result<()> {
    if !valid_tag(tag) {
        return Err(Error::invalid(format!(
            "invalid tag {tag:?}: 1-63 characters of [A-Za-z0-9._-], starting with a letter or digit"
        )));
    }
    let mut t = tags(home);
    t.insert(tag.to_string(), key.to_string());
    write_atomic(
        &dir(home).join("tags.json"),
        &serde_json::to_vec_pretty(&t)?,
    )
}

pub(super) fn remove_tags_of(home: &Path, key: &str) -> Result<Vec<String>> {
    let mut t = tags(home);
    let gone: Vec<String> = t
        .iter()
        .filter(|(_, k)| key12(k) == key12(key))
        .map(|(n, _)| n.clone())
        .collect();
    for n in &gone {
        t.remove(n);
    }
    if !gone.is_empty() {
        write_atomic(
            &dir(home).join("tags.json"),
            &serde_json::to_vec_pretty(&t)?,
        )?;
    }
    Ok(gone)
}

/// What is known of a base layer: the user cua-spacesd runs as in it and
/// the digest of the image it was made from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BaseInfo {
    pub driver_user: String,
    #[serde(default)]
    pub image_digest: Option<String>,
}

pub(super) fn save_base(home: &Path, base_key: &str, info: &BaseInfo) -> Result<()> {
    write_atomic(
        &dir(home)
            .join("bases")
            .join(format!("{}.json", key12(base_key))),
        &serde_json::to_vec(info)?,
    )
}

pub(super) fn load_base(home: &Path, base_key: &str) -> Option<BaseInfo> {
    let b = std::fs::read(
        dir(home)
            .join("bases")
            .join(format!("{}.json", key12(base_key))),
    )
    .ok()?;
    serde_json::from_slice(&b).ok()
}

fn builder_file(home: &Path, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir(home).join("builders").join(format!("{safe}.json"))
}

/// Records that this process runs builder Space `id`, so a build cut off
/// (Ctrl-C, a crash) has its builder deleted by the next build.
pub(super) fn note_builder(home: &Path, id: &str) {
    let _ = write_atomic(
        &builder_file(home, id),
        &serde_json::to_vec(&serde_json::json!({"id": id, "pid": std::process::id()}))
            .unwrap_or_default(),
    );
}

pub(super) fn forget_builder(home: &Path, id: &str) {
    let _ = std::fs::remove_file(builder_file(home, id));
}

/// Builder Spaces whose build process is gone.
pub(super) fn stale_builders(home: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir(home).join("builders")) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| {
            let v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let pid = v.get("pid")?.as_u64()? as u32;
            let id = v.get("id")?.as_str()?.to_string();
            (pid != std::process::id() && !alive(pid)).then_some(id)
        })
        .collect()
}

/// The checkpoint a reference names: a tag, a build's recipe key (or its
/// first 12 characters), or a layer checkpoint (`build-<key12>`).
pub fn resolve_ref(home: &Path, reference: &str) -> Option<String> {
    let reference = reference.strip_prefix("build:").unwrap_or(reference);
    if let Some(key) = tags(home).get(reference)
        && let Some(rec) = load_record(home, key)
    {
        return Some(rec.checkpoint);
    }
    if let Some(rec) = load_record(home, reference)
        .filter(|r| key12(&r.key) == key12(reference) && reference.len() >= 12)
    {
        return Some(rec.checkpoint);
    }
    load_records(home)
        .into_iter()
        .find(|r| r.layers.iter().any(|l| l == reference))
        .map(|_| reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(key: &str, layers: &[&str]) -> BuildRecord {
        BuildRecord {
            key: key.into(),
            checkpoint: layers.last().unwrap().to_string(),
            layers: layers.iter().map(|s| s.to_string()).collect(),
            config: ImageConfig::default(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn tags_resolve_to_the_final_checkpoint() {
        let d = tempfile::tempdir().unwrap();
        let r = rec("aaaaaaaaaaaabbbb", &["build-1", "build-2"]);
        save_record(d.path(), &r).unwrap();
        set_tag(d.path(), "app", &r.key).unwrap();
        assert_eq!(resolve_ref(d.path(), "app").as_deref(), Some("build-2"));
        assert_eq!(
            resolve_ref(d.path(), "build:app").as_deref(),
            Some("build-2")
        );
        assert_eq!(
            resolve_ref(d.path(), "aaaaaaaaaaaa").as_deref(),
            Some("build-2")
        );
        assert_eq!(resolve_ref(d.path(), "build-1").as_deref(), Some("build-1"));
        assert_eq!(resolve_ref(d.path(), "other"), None);
        // Moving the tag.
        let r2 = rec("cccccccccccc0000", &["build-1", "build-3"]);
        save_record(d.path(), &r2).unwrap();
        set_tag(d.path(), "app", &r2.key).unwrap();
        assert_eq!(resolve_ref(d.path(), "app").as_deref(), Some("build-3"));
        assert_eq!(remove_tags_of(d.path(), &r2.key).unwrap(), ["app"]);
        assert_eq!(resolve_ref(d.path(), "app"), None);
    }

    #[test]
    fn builders_of_dead_processes_are_stale() {
        let d = tempfile::tempdir().unwrap();
        note_builder(d.path(), "local:build-aaa-1");
        // This process's own builders are never stale.
        assert!(stale_builders(d.path(), &|_| false).is_empty());
        let other = builder_file(d.path(), "local:build-bbb-2");
        std::fs::write(&other, r#"{"id":"local:build-bbb-2","pid":4000000}"#).unwrap();
        assert_eq!(stale_builders(d.path(), &|_| false), ["local:build-bbb-2"]);
        assert!(stale_builders(d.path(), &|_| true).is_empty());
        forget_builder(d.path(), "local:build-bbb-2");
        assert!(stale_builders(d.path(), &|_| false).is_empty());
    }

    #[test]
    fn tags_are_validated() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["", "-x", "a b", "a:b", "a/b", &"x".repeat(64)] {
            assert!(set_tag(d.path(), bad, "k").is_err(), "{bad:?}");
        }
        assert!(set_tag(d.path(), "app.v1-rc_2", "kkkkkkkkkkkk").is_ok());
    }

    #[test]
    fn base_info_round_trips() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load_base(d.path(), "abcdef012345xx"), None);
        let info = BaseInfo {
            driver_user: "cua".into(),
            image_digest: Some("sha256:ab".into()),
        };
        save_base(d.path(), "abcdef012345xx", &info).unwrap();
        assert_eq!(load_base(d.path(), "abcdef012345"), Some(info));
    }
}
