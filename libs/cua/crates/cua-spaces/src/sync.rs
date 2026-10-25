// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Space::sync_dir`: a folder on this computer, as it is now, into a
//! folder of a Space, sending only what changed since the last sync.
//!
//! - The host side walks the folder the way git sees it: files `.gitignore`
//!   (and `.git/info/exclude`) leave out stay behind, and so does `.git`
//!   itself, unless asked for. Each file is hashed (SHA-256), with a cache
//!   of size and modification time so an unchanged file is not read again.
//! - The Space side keeps a manifest of what the last sync put there
//!   (`<dest>/.cua-sync.json`). The files whose content differs from it are
//!   sent as one gzipped tar through the Space's file service and unpacked
//!   with the time of the unpacking, never the host's: a build tool that
//!   compares times (cargo, make, tsc) sees exactly the changed files as
//!   changed, and an unchanged file keeps its time. Files the last sync put
//!   there and that are gone here are deleted; anything else in the
//!   destination (a `target/`, a `node_modules/`) is never touched.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::space::Space;

/// The manifest a sync keeps in the destination.
pub const MANIFEST: &str = ".cua-sync.json";

/// Most files one sync takes.
pub const MAX_FILES: usize = 500_000;

/// What to take from the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncOptions {
    /// Leave out what `.gitignore` files and `.git/info/exclude` leave out.
    pub gitignore: bool,
    /// Take the `.git` directory too.
    pub include_git: bool,
    /// Delete files the last sync put in the Space that are gone here.
    pub delete: bool,
    /// More to leave out: gitignore-style patterns, relative to the folder.
    pub exclude: Vec<String>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            gitignore: true,
            include_git: false,
            delete: true,
            exclude: Vec::new(),
        }
    }
}

/// One file of a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// SHA-256 of the content (of the link target for a symlink).
    pub sha256: String,
    /// Bytes.
    pub size: u64,
    /// Permission bits.
    pub mode: u32,
    /// A symbolic link's target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// What a sync put in a destination, by path relative to it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format.
    pub version: u32,
    /// Files.
    pub files: BTreeMap<String, FileEntry>,
}

/// What one sync did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SyncReport {
    /// The Space.
    pub space: String,
    /// The folder in it.
    pub dest: String,
    /// Files the folder has.
    pub files: usize,
    /// Files sent (new or changed).
    pub sent: usize,
    /// Their bytes, before compression.
    pub sent_bytes: u64,
    /// Bytes that went over the wire (the compressed tar).
    pub transferred_bytes: u64,
    /// Files deleted in the Space.
    pub deleted: usize,
    /// Files already up to date.
    pub unchanged: usize,
    /// Seconds it took.
    pub seconds: f64,
}

/// What to send and delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Paths to send.
    pub send: Vec<String>,
    /// Paths to delete in the Space.
    pub delete: Vec<String>,
}

/// The files to send (new, or different from what the Space has) and to
/// delete (in `remote`, gone from `local`, when `delete`).
pub fn plan(local: &Manifest, remote: &Manifest, delete: bool) -> Plan {
    let send = local
        .files
        .iter()
        .filter(|(p, e)| remote.files.get(*p) != Some(e))
        .map(|(p, _)| p.clone())
        .collect();
    let delete = if delete {
        remote
            .files
            .keys()
            .filter(|p| !local.files.contains_key(*p))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    Plan { send, delete }
}

/// Hashes this computer already worked out: relative path to (size,
/// modification time in ns, SHA-256).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HashCache {
    entries: BTreeMap<String, (u64, u128, String)>,
}

impl HashCache {
    /// The cache at `path` (empty when there is none or it cannot be read).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes it to `path` (best effort).
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[cfg(unix)]
fn mode_of(m: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(m: &std::fs::Metadata) -> u32 {
    if m.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

/// The folder `root` as a manifest, the way [`SyncOptions`] says, hashing
/// only files `cache` does not know at their size and time.
pub fn scan(root: &Path, opts: &SyncOptions, cache: &mut HashCache) -> Result<Manifest> {
    if !root.is_dir() {
        return Err(Error::invalid(format!("{} is not a folder", root.display())));
    }
    let mut overrides = ignore::overrides::OverrideBuilder::new(root);
    for pattern in &opts.exclude {
        overrides
            .add(&format!("!{}", pattern.trim_start_matches('!')))
            .map_err(|e| Error::invalid(format!("exclude {pattern}: {e}")))?;
    }
    let overrides = overrides
        .build()
        .map_err(|e| Error::invalid(format!("exclude: {e}")))?;
    let include_git = opts.include_git;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(opts.gitignore)
        .ignore(false)
        .git_ignore(opts.gitignore)
        .git_exclude(opts.gitignore)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .overrides(overrides)
        .filter_entry(move |e| include_git || e.file_name() != ".git")
        .build();
    let mut manifest = Manifest {
        version: 1,
        files: BTreeMap::new(),
    };
    let mut seen = BTreeMap::new();
    for entry in walker {
        let entry = entry.map_err(|e| Error::invalid(format!("reading the folder: {e}")))?;
        let Some(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|_| Error::invalid(format!("{} is outside the folder", path.display())))?;
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if rel.is_empty() || rel == MANIFEST {
            continue;
        }
        let meta = std::fs::symlink_metadata(path)?;
        let file = if ft.is_symlink() {
            let target = std::fs::read_link(path)?.to_string_lossy().into_owned();
            FileEntry {
                sha256: hex::encode(Sha256::digest(target.as_bytes())),
                size: 0,
                mode: 0o777,
                link: Some(target),
            }
        } else if ft.is_file() {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let size = meta.len();
            let sha256 = match cache.entries.get(&rel) {
                Some((s, t, h)) if *s == size && *t == mtime && mtime != 0 => h.clone(),
                _ => {
                    let h = sha256_of_file(path)?;
                    cache.entries.insert(rel.clone(), (size, mtime, h.clone()));
                    h
                }
            };
            FileEntry {
                sha256,
                size,
                mode: mode_of(&meta),
                link: None,
            }
        } else {
            continue;
        };
        seen.insert(rel.clone(), ());
        manifest.files.insert(rel, file);
        if manifest.files.len() > MAX_FILES {
            return Err(Error::invalid(format!(
                "{} has more than {MAX_FILES} files to sync; leave some out (--exclude)",
                root.display()
            )));
        }
    }
    // Forget hashes of files that are gone.
    cache.entries.retain(|k, _| seen.contains_key(k));
    Ok(manifest)
}

/// The files `send` names, from `root`, as a gzipped tar (paths relative
/// to the destination; content checked against `local` while packing).
pub fn pack(root: &Path, local: &Manifest, send: &[String]) -> Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for rel in send {
        let Some(e) = local.files.get(rel) else {
            continue;
        };
        let mut h = tar::Header::new_gnu();
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        match &e.link {
            Some(target) => {
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_mode(0o777);
                h.set_size(0);
                tar.append_link(&mut h, rel, target)?;
            }
            None => {
                let path = root.join(rel);
                let f = std::fs::File::open(&path)?;
                let len = f.metadata()?.len();
                if len != e.size {
                    return Err(Error::invalid(format!(
                        "{} changed while it was being synced; sync again",
                        path.display()
                    )));
                }
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(e.mode);
                h.set_size(len);
                tar.append_data(&mut h, rel, f)?;
            }
        }
    }
    let gz = tar.into_inner()?;
    Ok(gz.finish()?)
}

/// POSIX single-quoting.
fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,@%+".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where this computer keeps its hashes of `local` for syncs to `space`
/// and `dest`, under `home`.
pub fn cache_path(home: &Path, local: &Path, space: &str, dest: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(
        format!("{}\n{space}\n{dest}", local.display()).as_bytes(),
    ));
    home.join("sync").join(format!("{}.json", &key[..32]))
}

impl Space {
    /// Makes `dest` in this Space hold what the folder `local` holds now,
    /// sending only what changed since the last sync to it (see the module
    /// docs). `dest` is absolute, or relative to the Space user's home.
    /// `cache` is where this computer keeps the folder's hashes between
    /// syncs (`None`: hash everything).
    pub async fn sync_dir(
        &self,
        local: &Path,
        dest: &str,
        opts: &SyncOptions,
        cache: Option<&Path>,
    ) -> Result<SyncReport> {
        if self.is_windows() {
            return Err(Error::invalid(
                "syncing a folder into a Windows Space is not supported yet; use upload",
            ));
        }
        let started = Instant::now();
        let dest = if dest.starts_with('/') {
            dest.trim_end_matches('/').to_string()
        } else {
            let home = self.home().await?;
            format!("{}/{}", home.trim_end_matches('/'), dest.trim_end_matches('/'))
        };
        let local_root = local.to_path_buf();
        let opts2 = opts.clone();
        let cache_file = cache.map(Path::to_path_buf);
        let (manifest, cache_state) = tokio::task::spawn_blocking(move || {
            let mut c = cache_file
                .as_deref()
                .map(HashCache::load)
                .unwrap_or_default();
            scan(&local_root, &opts2, &mut c).map(|m| (m, c))
        })
        .await
        .map_err(|e| Error::invalid(format!("scanning the folder: {e}")))??;

        let env = self.spacesd()?;
        let remote: Manifest = match env.download(&format!("{dest}/{MANIFEST}")).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Manifest::default(),
        };
        let p = plan(&manifest, &remote, opts.delete);
        let sent_bytes: u64 = p
            .send
            .iter()
            .filter_map(|r| manifest.files.get(r))
            .map(|e| e.size)
            .sum();
        let tag: u64 = rand::random();
        let tgz = format!("/tmp/.cua-sync-{tag:016x}.tgz");
        let del = format!("/tmp/.cua-sync-{tag:016x}.del");
        let mut transferred = 0u64;
        if !p.send.is_empty() {
            let root = local.to_path_buf();
            let m = manifest.clone();
            let send = p.send.clone();
            let bytes = tokio::task::spawn_blocking(move || pack(&root, &m, &send))
                .await
                .map_err(|e| Error::invalid(format!("packing: {e}")))??;
            transferred += bytes.len() as u64;
            self.write(&tgz, bytes).await?;
        }
        if !p.delete.is_empty() {
            let mut list = Vec::new();
            for d in &p.delete {
                list.extend_from_slice(d.as_bytes());
                list.push(0);
            }
            transferred += list.len() as u64;
            self.write(&del, list).await?;
        }
        let d = sh_quote(&dest);
        let mut script = format!("set -e; mkdir -p -- {d}; cd -- {d}");
        if !p.send.is_empty() {
            // -m: the time of the unpacking, so changed files are newer than
            // what was built from them.
            script.push_str(&format!(
                "; tar -xzmf {t} --no-same-owner; rm -f -- {t}",
                t = sh_quote(&tgz)
            ));
        }
        if !p.delete.is_empty() {
            script.push_str(&format!(
                "; xargs -0 rm -f -- < {f}; rm -f -- {f}",
                f = sh_quote(&del)
            ));
        }
        let out = self.bash(&script, Duration::from_secs(1800)).await?;
        if !out.success() {
            return Err(Error::invalid(format!(
                "unpacking in the Space failed: {}",
                out.render()
            )));
        }
        let json = serde_json::to_vec(&manifest)?;
        self.write(&format!("{dest}/{MANIFEST}"), json).await?;
        if let Some(c) = cache {
            cache_state.save(c);
        }
        Ok(SyncReport {
            space: self.id().to_string(),
            dest,
            files: manifest.files.len(),
            sent: p.send.len(),
            sent_bytes,
            transferred_bytes: transferred,
            deleted: p.delete.len(),
            unchanged: manifest.files.len() - p.send.len(),
            seconds: started.elapsed().as_secs_f64(),
        })
    }
}

impl crate::Spaces {
    /// [`Space::sync_dir`] for the Space `space`, with this home's hash
    /// cache, so a second sync of the same folder reads only changed files.
    pub async fn sync(
        &self,
        space: &str,
        local: &Path,
        dest: &str,
        opts: &SyncOptions,
    ) -> Result<SyncReport> {
        let s = self.space(space).await?;
        let local = std::fs::canonicalize(local)
            .map_err(|e| Error::invalid(format!("{}: {e}", local.display())))?;
        let cache = cache_path(self.home_dir(), &local, &s.id().to_string(), dest);
        s.sync_dir(&local, dest, opts, Some(&cache)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn the_folder_is_taken_the_way_git_sees_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", "target/\n*.log\n");
        write(root, "src/main.rs", "fn main() {}\n");
        write(root, "target/debug/x", "built\n");
        write(root, "debug.log", "noise\n");
        write(root, ".git/HEAD", "ref: refs/heads/main\n");
        write(root, "notes/todo.txt", "untracked but not ignored\n");
        write(root, "sub/.gitignore", "secret\n");
        write(root, "sub/secret", "x\n");
        write(root, "sub/kept", "y\n");
        write(root, MANIFEST, "{}");
        let mut cache = HashCache::default();
        let m = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        let names: Vec<&str> = m.files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [".gitignore", "notes/todo.txt", "src/main.rs", "sub/.gitignore", "sub/kept"]
        );
        let all = scan(
            root,
            &SyncOptions {
                gitignore: false,
                include_git: true,
                exclude: vec!["notes/".into()],
                ..Default::default()
            },
            &mut HashCache::default(),
        )
        .unwrap();
        assert!(all.files.contains_key("target/debug/x"));
        assert!(all.files.contains_key(".git/HEAD"));
        assert!(!all.files.contains_key("notes/todo.txt"));
        assert!(!all.files.contains_key(MANIFEST));
    }

    #[test]
    fn only_changed_files_are_sent_and_only_synced_files_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a", "1");
        write(root, "b", "2");
        let mut cache = HashCache::default();
        let first = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        let p = plan(&first, &Manifest::default(), true);
        assert_eq!(p.send, ["a", "b"]);
        assert!(p.delete.is_empty());

        write(root, "b", "changed");
        std::fs::remove_file(root.join("a")).unwrap();
        write(root, "c", "new");
        let second = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        let p = plan(&second, &first, true);
        assert_eq!(p.send, ["b", "c"]);
        assert_eq!(p.delete, ["a"]);
        assert!(plan(&second, &first, false).delete.is_empty());
        // Nothing changed: nothing to do.
        assert_eq!(plan(&second, &second, true), Plan::default());
    }

    #[test]
    fn the_cache_skips_rereading_but_a_new_size_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "f", "one");
        let mut cache = HashCache::default();
        let a = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        let path = dir.path().join("cache.json");
        cache.save(&path);
        let mut cache = HashCache::load(&path);
        let b = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        assert_eq!(a, b);
        write(root, "f", "longer now");
        let c = scan(root, &SyncOptions::default(), &mut cache).unwrap();
        assert_ne!(a.files["f"].sha256, c.files["f"].sha256);
    }

    #[test]
    fn the_tar_holds_what_is_sent_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "keep/x", "x");
        write(root, "send/y", "yy");
        let m = scan(root, &SyncOptions::default(), &mut HashCache::default()).unwrap();
        let tgz = pack(root, &m, &["send/y".to_string()]).unwrap();
        let mut a = tar::Archive::new(flate2::read::GzDecoder::new(&tgz[..]));
        let names: Vec<String> = a
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["send/y"]);
    }

    #[test]
    fn quoting_keeps_odd_paths_whole() {
        assert_eq!(sh_quote("/home/cua/src"), "/home/cua/src");
        assert_eq!(sh_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
    }
}
