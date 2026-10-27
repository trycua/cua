//! A host's snapshot directory (`$CUA_HOME/snapshots`, owner-only):
//!
//! ```text
//! blobs/<sha256>        files received from other hosts, by content
//! blobs/<sha256>.ok     the blob's stamp when it was verified
//! partial/<sha256>      downloads in progress (resumed from their length)
//! manifests/<digest>    manifests this host exported or received
//! exports/<digest>      where an exported manifest's files are here
//! names/<name>          the digest of the checkpoint of that name here
//! grant.key             the key read grants are signed with
//! hash-cache.json       file hashes by path, size and time
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hash::Stamp;
use crate::{Error, FileEntry, FileSpec, Manifest, Result, placeholder, valid_sha};

/// Where an exported manifest's file is on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exported {
    /// The file.
    pub path: PathBuf,
    /// Its stamp when it was hashed: served only while it still matches.
    pub stamp: Stamp,
}

/// Where the files of one exported manifest are, by SHA-256.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportIndex {
    /// The manifest's digest.
    pub digest: String,
    /// Files by SHA-256.
    pub files: BTreeMap<String, Exported>,
}

/// A host's snapshot directory.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

fn private_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `bytes` to `path` (owner-only), atomically.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{:08x}", rand::random::<u32>()));
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        || name.starts_with('.')
    {
        return Err(Error::Invalid(format!("bad checkpoint name {name:?}")));
    }
    Ok(())
}

impl Store {
    /// The snapshot directory of cua home `home`.
    pub fn of_home(home: &Path) -> Store {
        Store {
            root: home.join("snapshots"),
        }
    }

    /// A store at `root`.
    pub fn at(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// Its directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn sub(&self, d: &str) -> Result<PathBuf> {
        let p = self.root.join(d);
        private_dir(&self.root)?;
        private_dir(&p)?;
        Ok(p)
    }

    /// Where blob `sha` is kept.
    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join("blobs").join(sha)
    }

    /// Where a download of blob `sha` goes until it is verified.
    pub fn partial_path(&self, sha: &str) -> Result<PathBuf> {
        Ok(self.sub("partial")?.join(sha))
    }

    /// Whether this host has blob `sha` (of `size` bytes), verified and not
    /// changed since.
    pub fn has_blob(&self, sha: &str, size: u64) -> bool {
        if !valid_sha(sha) {
            return false;
        }
        let p = self.blob_path(sha);
        let Ok(now) = Stamp::of(&p) else {
            return false;
        };
        let ok: Option<Stamp> = std::fs::read(p.with_extension("ok"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        now.size == size && ok.as_ref() == Some(&now)
    }

    /// Checks the downloaded `partial/<sha>` against its SHA-256 and size
    /// and keeps it as a blob. A file that does not match is deleted.
    pub async fn finalize_partial(&self, sha: &str, size: u64) -> Result<()> {
        let part = self.partial_path(sha)?;
        let p2 = part.clone();
        let got = tokio::task::spawn_blocking(move || crate::hash::sha256_file(&p2))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))??;
        let len = std::fs::metadata(&part)?.len();
        if got != sha || len != size {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Corrupt(format!(
                "a copied file did not match its SHA-256 ({len} bytes, sha256 {got}, expected {sha}); it was discarded"
            )));
        }
        self.adopt(sha, &part)
    }

    /// Keeps file `path`, already verified to have SHA-256 `sha`, as a
    /// blob (moving it).
    pub fn adopt(&self, sha: &str, path: &Path) -> Result<()> {
        if !valid_sha(sha) {
            return Err(Error::Invalid(format!("bad sha256 {sha}")));
        }
        let blobs = self.sub("blobs")?;
        let dst = blobs.join(sha);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(path, &dst)?;
        let stamp = Stamp::of(&dst)?;
        write_private(&dst.with_extension("ok"), &serde_json::to_vec(&stamp)?)?;
        Ok(())
    }

    /// The key read grants of this host are signed with (made the first
    /// time, owner-only).
    pub fn grant_key(&self) -> Result<Vec<u8>> {
        let p = self.root.join("grant.key");
        if let Ok(k) = std::fs::read(&p)
            && k.len() == 32
        {
            return Ok(k);
        }
        private_dir(&self.root)?;
        let k: [u8; 32] = rand::random();
        // Two exports at once: the first key written wins, both read it.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(mut f) => {
                use std::io::Write as _;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
                }
                f.write_all(&k)?;
                f.sync_all()?;
                Ok(k.to_vec())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                for _ in 0..50 {
                    if let Ok(k) = std::fs::read(&p)
                        && k.len() == 32
                    {
                        return Ok(k);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(Error::Invalid(format!(
                    "{} is not a grant key",
                    p.display()
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Saves `m` (owner-only: it holds the guest token).
    pub fn save_manifest(&self, m: &Manifest) -> Result<String> {
        let d = m.digest();
        write_private(&self.sub("manifests")?.join(&d), &m.to_bytes())?;
        Ok(d)
    }

    /// The manifest of `digest`, if this host has it.
    pub fn manifest(&self, digest: &str) -> Result<Manifest> {
        if !valid_sha(digest) {
            return Err(Error::Invalid(format!("bad digest {digest}")));
        }
        let b = std::fs::read(self.root.join("manifests").join(digest))
            .map_err(|_| Error::NotFound(format!("snapshot manifest {digest}")))?;
        let m = Manifest::from_bytes(&b)?;
        if m.digest() != digest {
            return Err(Error::Corrupt(format!("manifest {digest} was changed")));
        }
        Ok(m)
    }

    /// The export index of `digest`.
    pub fn export_index(&self, digest: &str) -> Result<ExportIndex> {
        if !valid_sha(digest) {
            return Err(Error::Invalid(format!("bad digest {digest}")));
        }
        let b = std::fs::read(self.root.join("exports").join(digest))
            .map_err(|_| Error::NotFound(format!("no export of snapshot {digest} here")))?;
        Ok(serde_json::from_slice(&b)?)
    }

    /// Records that checkpoint `name` here is the manifest `digest`.
    pub fn set_name(&self, name: &str, digest: &str) -> Result<()> {
        valid_name(name)?;
        write_private(&self.sub("names")?.join(name), digest.as_bytes())?;
        Ok(())
    }

    /// Forgets checkpoint `name` (it was deleted).
    pub fn remove_name(&self, name: &str) {
        if valid_name(name).is_ok() {
            let _ = std::fs::remove_file(self.root.join("names").join(name));
        }
    }

    /// The digest of checkpoint `name` here, if it was exported or
    /// received.
    pub fn digest_of(&self, name: &str) -> Option<String> {
        valid_name(name).ok()?;
        std::fs::read_to_string(self.root.join("names").join(name))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|d| valid_sha(d))
    }

    /// Hashes a checkpoint's files and writes its manifest, export index
    /// and name. `vmm` is the runtime's state with every path replaced by
    /// the [`placeholder`] of its [`FileSpec::id`] (or directory id in
    /// `dirs`). Files are listed in the order given.
    #[allow(clippy::too_many_arguments)]
    pub async fn export(
        &self,
        name: &str,
        runtime_type: &str,
        abi: &str,
        vmm: serde_json::Value,
        record: serde_json::Value,
        dirs: Vec<String>,
        files: Vec<FileSpec>,
    ) -> Result<(Manifest, String)> {
        valid_name(name)?;
        let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        let hashed = crate::hash::hash_files(&self.root.join("hash-cache.json"), &paths).await?;
        let mut entries = Vec::with_capacity(files.len());
        let mut index = ExportIndex::default();
        for (f, (sha, stamp)) in files.iter().zip(hashed) {
            let fname = f
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::Invalid(format!("{} has no file name", f.path.display())))?
                .to_string();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::metadata(&f.path)?.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0o600;
            entries.push(FileEntry {
                id: f.id.clone(),
                sha256: sha.clone(),
                size: stamp.size,
                dir: f.dir.clone(),
                name: fname,
                qcow2_backing: f.qcow2_backing.clone(),
                mode,
            });
            index.files.insert(
                sha,
                Exported {
                    path: f.path.clone(),
                    stamp,
                },
            );
        }
        let m = Manifest {
            version: crate::MANIFEST_VERSION,
            name: name.into(),
            runtime_type: runtime_type.into(),
            abi: abi.into(),
            files: entries,
            dirs,
            vmm,
            record,
        };
        m.validate()?;
        let digest = self.save_manifest(&m)?;
        index.digest = digest.clone();
        write_private(
            &self.sub("exports")?.join(&digest),
            &serde_json::to_vec(&index)?,
        )?;
        self.set_name(name, &digest)?;
        Ok((m, digest))
    }

    /// Lays out `m`'s files under `dest` (each directory id a directory,
    /// other files at the top), from the blobs this host has. A file whose
    /// qcow2 backing is rebased later is a copy; every other file is a hard
    /// link to its blob (no copy). Returns each id's path (files and
    /// directories). Every blob must be here ([`Store::has_blob`]).
    pub fn materialize(&self, m: &Manifest, dest: &Path) -> Result<BTreeMap<String, PathBuf>> {
        m.validate()?;
        private_dir(dest)?;
        let mut out = BTreeMap::new();
        for d in &m.dirs {
            let p = dest.join(d);
            private_dir(&p)?;
            out.insert(d.clone(), p);
        }
        let top = dest.join("files");
        private_dir(&top)?;
        for f in &m.files {
            if !self.has_blob(&f.sha256, f.size) {
                return Err(Error::NotFound(format!(
                    "file {} ({}) of {} is not here",
                    f.id, f.name, m.name
                )));
            }
            let p = match &f.dir {
                Some(d) => dest.join(d).join(&f.name),
                // Files outside a directory may share a name (disk layers).
                None => top.join(format!("{}-{}", f.id, f.name)),
            };
            let blob = self.blob_path(&f.sha256);
            let _ = std::fs::remove_file(&p);
            if f.qcow2_backing.is_some() || std::fs::hard_link(&blob, &p).is_err() {
                std::fs::copy(&blob, &p)?;
            }
            #[cfg(unix)]
            if f.qcow2_backing.is_some() {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
            }
            out.insert(f.id.clone(), p);
        }
        Ok(out)
    }
}

/// Replaces every [`placeholder`] string in `v` with the path of its id in
/// `paths` (or fails on an id it does not know).
pub fn resolve_placeholders(
    v: &mut serde_json::Value,
    paths: &BTreeMap<String, PathBuf>,
) -> Result<()> {
    match v {
        serde_json::Value::String(s) => {
            if let Some(id) = crate::placeholder_id(s) {
                let p = paths
                    .get(id)
                    .ok_or_else(|| Error::Invalid(format!("unknown snapshot file {id}")))?;
                *s = p.to_string_lossy().into_owned();
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                resolve_placeholders(x, paths)?;
            }
        }
        serde_json::Value::Object(o) => {
            for (_, x) in o.iter_mut() {
                resolve_placeholders(x, paths)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replaces every string in `v` that is one of `paths` (a file or
/// directory path) with its [`placeholder`]. Returns the ids replaced.
pub fn insert_placeholders(
    v: &mut serde_json::Value,
    paths: &BTreeMap<PathBuf, String>,
) -> std::collections::BTreeSet<String> {
    let mut used = std::collections::BTreeSet::new();
    fn walk(
        v: &mut serde_json::Value,
        paths: &BTreeMap<PathBuf, String>,
        used: &mut std::collections::BTreeSet<String>,
    ) {
        match v {
            serde_json::Value::String(s) => {
                if let Some(id) = paths.get(Path::new(s.as_str())) {
                    used.insert(id.clone());
                    *s = placeholder(id);
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|x| walk(x, paths, used)),
            serde_json::Value::Object(o) => o.iter_mut().for_each(|(_, x)| walk(x, paths, used)),
            _ => {}
        }
    }
    walk(v, paths, &mut used);
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_then_materialize_on_another_store() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("mem")).unwrap();
        std::fs::write(src.join("mem/state.mig"), b"STATE").unwrap();
        std::fs::write(src.join("disk.qcow2"), b"DISK").unwrap();
        std::fs::write(src.join("base.qcow2"), b"BASE").unwrap();
        let mut vmm = serde_json::json!({
            "disk": src.join("disk.qcow2"),
            "memory": {"dir": src.join("mem")},
            "unrelated": "/not/a/snapshot/path",
        });
        let mut by_path = BTreeMap::new();
        by_path.insert(src.join("disk.qcow2"), "f1".to_string());
        by_path.insert(src.join("mem"), "d0".to_string());
        let used = insert_placeholders(&mut vmm, &by_path);
        assert_eq!(used.len(), 2);
        assert_eq!(vmm["disk"], placeholder("f1"));
        assert_eq!(vmm["unrelated"], "/not/a/snapshot/path");

        let a = Store::at(dir.path().join("a"));
        let (m, digest) = a
            .export(
                "w0",
                "qemu",
                "qemu/hvf/aarch64",
                vmm,
                serde_json::json!({"name": "w0"}),
                vec!["d0".into()],
                vec![
                    FileSpec {
                        id: "f0".into(),
                        path: src.join("mem/state.mig"),
                        dir: Some("d0".into()),
                        qcow2_backing: None,
                    },
                    FileSpec {
                        id: "f1".into(),
                        path: src.join("disk.qcow2"),
                        dir: None,
                        qcow2_backing: Some("f2".into()),
                    },
                    FileSpec {
                        id: "f2".into(),
                        path: src.join("base.qcow2"),
                        dir: None,
                        qcow2_backing: None,
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(a.digest_of("w0").as_deref(), Some(digest.as_str()));
        assert_eq!(a.manifest(&digest).unwrap(), m);
        let idx = a.export_index(&digest).unwrap();
        assert_eq!(idx.files.len(), 3);

        // Another host receives the blobs (here: copies them in).
        let b = Store::at(dir.path().join("b"));
        for f in &m.files {
            assert!(!b.has_blob(&f.sha256, f.size));
            let part = b.partial_path(&f.sha256).unwrap();
            std::fs::copy(&idx.files[&f.sha256].path, &part).unwrap();
            b.finalize_partial(&f.sha256, f.size).await.unwrap();
            assert!(b.has_blob(&f.sha256, f.size));
        }
        let dest = dir.path().join("dest");
        let paths = b.materialize(&m, &dest).unwrap();
        assert_eq!(std::fs::read(&paths["f0"]).unwrap(), b"STATE");
        assert_eq!(paths["f0"], paths["d0"].join("state.mig"));
        assert_eq!(std::fs::read(&paths["f1"]).unwrap(), b"DISK");
        let mut v = m.vmm.clone();
        resolve_placeholders(&mut v, &paths).unwrap();
        assert_eq!(v["disk"], paths["f1"].to_string_lossy().as_ref());
        assert_eq!(v["memory"]["dir"], paths["d0"].to_string_lossy().as_ref());
        // The rebased overlay is a copy; the stream a link to its blob.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
            assert_eq!(ino(&paths["f0"]), ino(&b.blob_path(&m.files[0].sha256)));
            assert_ne!(ino(&paths["f1"]), ino(&b.blob_path(&m.files[1].sha256)));
        }
    }

    #[tokio::test]
    async fn a_corrupt_download_is_discarded_and_a_changed_blob_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::at(dir.path());
        let sha = crate::hash::sha256_file(&{
            let p = dir.path().join("x");
            std::fs::write(&p, b"good").unwrap();
            p
        })
        .unwrap();
        std::fs::write(s.partial_path(&sha).unwrap(), b"evil").unwrap();
        let e = s.finalize_partial(&sha, 4).await.unwrap_err();
        assert!(matches!(e, Error::Corrupt(_)), "{e}");
        assert!(!s.partial_path(&sha).unwrap().exists());
        std::fs::write(s.partial_path(&sha).unwrap(), b"good").unwrap();
        s.finalize_partial(&sha, 4).await.unwrap();
        assert!(s.has_blob(&sha, 4));
        // Written to after it was verified: not trusted any more.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(s.blob_path(&sha), b"bad!").unwrap();
        assert!(!s.has_blob(&sha, 4));
    }

    #[test]
    fn grant_key_is_stable_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::at(dir.path().join("snap"));
        let k = s.grant_key().unwrap();
        assert_eq!(k.len(), 32);
        assert_eq!(s.grant_key().unwrap(), k);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let m = std::fs::metadata(s.root().join("grant.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
        assert!(s.set_name("../x", "y").is_err());
        assert!(s.digest_of("nope").is_none());
    }
}
